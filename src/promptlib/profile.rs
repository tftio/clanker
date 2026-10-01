//! Profile resolution, dependency trees, and listing.
use super::config::Config;
use super::config::load_bundle;
use super::{
    ProfileDef, PrompterError, home_dir, parse_config_file, read_config_with_path, unescape,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use tftio_lib::{JsonOutput, render_response};

/// A model-family directory name used for fragment substitution.
///
/// Family names are restricted to one non-empty path component so callers
/// cannot escape a fragment's `families/` directory.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FamilyName(String);

impl FamilyName {
    /// Parse and validate a family name.
    ///
    /// # Errors
    /// Returns [`InvalidFamilyName`] when `value` is empty, absolute, or contains
    /// path separators or traversal components.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidFamilyName> {
        let value = value.into();
        let mut components = Path::new(&value).components();
        let is_single_component = matches!(
            components.next(),
            Some(Component::Normal(component)) if component == OsStr::new(&value)
        ) && components.next().is_none();

        if is_single_component {
            Ok(Self(value))
        } else {
            Err(InvalidFamilyName(value))
        }
    }

    /// Return the validated directory name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for FamilyName {
    type Err = InvalidFamilyName;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl std::fmt::Display for FamilyName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Error returned when a family name is not a single safe path component.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("family name must be one non-empty path component: {0}")]
pub struct InvalidFamilyName(String);

pub(crate) fn collect_profiles(
    prefix: &str,
    value: &toml::Value,
    out: &mut HashMap<String, Vec<String>>,
) -> Result<(), PrompterError> {
    let Some(table) = value.as_table() else {
        return Ok(());
    };

    if let Some(deps_value) = table.get("depends_on") {
        let arr = deps_value.as_array().ok_or_else(|| {
            PrompterError::ConfigField(format!(
                "`depends_on` for [{prefix}] must be an array of strings"
            ))
        })?;
        let mut deps = Vec::with_capacity(arr.len());
        for entry in arr {
            let s = entry.as_str().ok_or_else(|| {
                PrompterError::ConfigField(format!(
                    "`depends_on` entries for [{prefix}] must be strings"
                ))
            })?;
            deps.push(s.to_string());
        }
        if out.insert(prefix.to_string(), deps).is_some() {
            return Err(PrompterError::DuplicateProfile(format!(
                "Duplicate profile definition: [{prefix}]"
            )));
        }
    }

    for (sub_key, sub_val) in table {
        if sub_key == "depends_on" {
            continue;
        }
        let new_prefix = if prefix.is_empty() {
            sub_key.clone()
        } else {
            format!("{prefix}.{sub_key}")
        };
        collect_profiles(&new_prefix, sub_val, out)?;
    }
    Ok(())
}

/// Expand a leading `~` or `~/` to the home directory.
///
/// Paths without a leading `~` are returned unchanged.
pub(crate) fn expand_tilde(s: &str) -> Result<PathBuf, PrompterError> {
    if let Some(rest) = s.strip_prefix("~/") {
        Ok(home_dir()?.join(rest))
    } else if s == "~" {
        home_dir()
    } else {
        Ok(PathBuf::from(s))
    }
}

/// Resolve an `import = [...]` entry against the importing config's directory.
///
/// Absolute paths are returned as-is (after `~` expansion); relative paths
/// are joined onto `importer_dir`.
fn resolve_import_path(importer_dir: &Path, import_str: &str) -> Result<PathBuf, PrompterError> {
    let p = expand_tilde(import_str)?;
    if p.is_absolute() {
        Ok(p)
    } else {
        Ok(importer_dir.join(p))
    }
}

/// Determine the library root for a loaded config file.
///
/// * If the config has an explicit `library = "..."` key, expand tilde and
///   resolve it relative to the config file's directory.
/// * Otherwise, if `default_library` is provided (only the primary config
///   with no `-c` override gets this), use that.
/// * Otherwise, default to `<config-dir>/library`.
fn library_root_for(
    config_path: &Path,
    raw_library: Option<&str>,
    default_library: Option<&Path>,
) -> Result<PathBuf, PrompterError> {
    let config_dir = config_path
        .parent()
        .ok_or_else(|| PrompterError::NoParentDir(config_path.to_path_buf()))?;

    if let Some(lib_str) = raw_library {
        let p = expand_tilde(lib_str)?;
        return Ok(if p.is_absolute() {
            p
        } else {
            config_dir.join(p)
        });
    }

    if let Some(default) = default_library {
        return Ok(default.to_path_buf());
    }

    Ok(config_dir.join("library"))
}

/// Load a primary config file plus all of its transitive imports and merge them.
///
/// Invariants enforced:
/// * Profile names must be unique across the entire merged bundle (hard error).
/// * Import cycles are detected and reported.
/// * Only the primary file's `post_prompt` is kept; imports' are ignored.
///
/// `primary_default_library` is the library root to use for the primary
/// config when it does not declare `library = "..."`. Pass
/// `Some(~/.local/prompter/library)` when loading the default config, or
/// `None` when the user supplied `-c FILE` (in which case the primary falls
/// back to `<FILE-dir>/library`).
///
/// # Errors
/// Returns an error on I/O failure, TOML parse failure, unresolved import
/// paths, import cycles, or duplicate profile names.
pub fn load_config_bundle(
    primary_path: &Path,
    primary_default_library: Option<&Path>,
) -> Result<Config, PrompterError> {
    let mut profiles: HashMap<String, ProfileDef> = HashMap::new();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut post_prompt: Option<String> = None;

    load_one(
        primary_path,
        primary_default_library,
        true,
        &mut profiles,
        &mut visited,
        &mut post_prompt,
    )?;

    Ok(Config {
        profiles,
        post_prompt,
    })
}

fn load_one(
    path: &Path,
    default_library: Option<&Path>,
    is_primary: bool,
    profiles: &mut HashMap<String, ProfileDef>,
    visited: &mut HashSet<PathBuf>,
    post_prompt: &mut Option<String>,
) -> Result<(), PrompterError> {
    let text = read_config_with_path(path)?;
    let canonical = fs::canonicalize(path).map_err(|source| PrompterError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    if !visited.insert(canonical.clone()) {
        return Err(PrompterError::ImportCycle(canonical));
    }

    let raw = parse_config_file(&text)?;

    let library_root = library_root_for(&canonical, raw.library.as_deref(), default_library)?;

    for (name, deps) in raw.profiles {
        if let Some(existing) = profiles.get(&name) {
            return Err(PrompterError::DuplicateProfile(format!(
                "Duplicate profile `{}` defined in both {} and {}",
                name,
                existing.library_root.display(),
                library_root.display()
            )));
        }
        profiles.insert(
            name,
            ProfileDef {
                deps,
                library_root: library_root.clone(),
            },
        );
    }

    if is_primary {
        *post_prompt = raw.post_prompt.map(|s| unescape(&s));
    }

    let importer_dir = canonical
        .parent()
        .ok_or_else(|| PrompterError::NoParentDir(canonical.clone()))?
        .to_path_buf();
    for import_str in raw.imports {
        let import_path = resolve_import_path(&importer_dir, &import_str)?;
        // Imported configs never use the primary default library; they always
        // resolve their library via `library = "..."` or `<their-dir>/library`.
        load_one(&import_path, None, false, profiles, visited, post_prompt)?;
    }

    Ok(())
}

/// Errors that can occur during profile resolution.
///
/// These errors represent various failure modes when resolving
/// profile dependencies and validating file references.
#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum ResolveError {
    /// Referenced profile name does not exist in configuration
    #[error("Unknown profile: {0}")]
    UnknownProfile(String),
    /// Circular dependency detected in profile references
    #[error("Cycle detected: {}", .0.join(" -> "))]
    Cycle(Vec<String>),
    /// Referenced markdown file does not exist
    #[error("Missing file: {path} (referenced by [{referenced_by}])", path = .0.display(), referenced_by = .1)]
    MissingFile(PathBuf, String), // (path, referenced_by)
}

/// Node type in the dependency tree
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TreeNodeType {
    /// Profile node
    Profile,
    /// Fragment (markdown file) node
    Fragment,
}

/// Tree node representing a profile or fragment in the dependency tree
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNode {
    /// Type of node (profile or fragment)
    #[serde(rename = "type")]
    pub node_type: TreeNodeType,
    /// Name of profile or path of fragment
    pub name: String,
    /// Children of this node
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Self>,
}

/// Complete tree structure for JSON output
#[derive(Debug, Serialize, Deserialize)]
pub struct TreeOutput {
    /// List of root trees (top-level profiles)
    pub trees: Vec<TreeNode>,
}

/// Recursively resolve a profile's dependencies into a list of
/// `(fragment_path, owning_library_root)` pairs.
///
/// Each `.md` dep is resolved against the owning profile's `library_root`,
/// so profiles imported from different bundles still find their fragments.
/// Profile-to-profile refs are looked up by name in the merged map.
///
/// Cycle detection and fragment deduplication (by absolute path) are
/// performed across the full traversal.
///
/// # Errors
/// Returns an error if:
/// - Profile name is not found in configuration
/// - Circular dependency is detected
/// - Referenced markdown file does not exist
#[allow(
    clippy::implicit_hasher,
    reason = "public resolver API intentionally fixes the std default HashMap hasher rather than exposing a hasher type parameter"
)]
pub fn resolve_profile(
    name: &str,
    cfg: &Config,
    seen_files: &mut HashSet<PathBuf>,
    stack: &mut Vec<String>,
    out: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), ResolveError> {
    resolve_profile_for_family(name, cfg, None, seen_files, stack, out)
}

pub(crate) fn resolve_profile_for_family(
    name: &str,
    cfg: &Config,
    family: Option<&FamilyName>,
    seen_files: &mut HashSet<PathBuf>,
    stack: &mut Vec<String>,
    out: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), ResolveError> {
    if stack.contains(&name.to_string()) {
        let mut cycle = stack.clone();
        cycle.push(name.to_string());
        return Err(ResolveError::Cycle(cycle));
    }
    let profile = cfg
        .profiles
        .get(name)
        .ok_or_else(|| ResolveError::UnknownProfile(name.to_string()))?;
    stack.push(name.to_string());
    for dep in &profile.deps {
        if std::path::Path::new(dep)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            let path = profile.library_root.join(dep);
            if !path.exists() {
                return Err(ResolveError::MissingFile(path, name.to_string()));
            }
            if seen_files.insert(path.clone()) {
                let rendered_path = family.map_or_else(
                    || path.clone(),
                    |family| {
                        let variant = path.parent().map_or_else(
                            || path.clone(),
                            |parent| {
                                parent
                                    .join("families")
                                    .join(family.as_str())
                                    .join(path.file_name().unwrap_or_default())
                            },
                        );
                        if variant.is_file() {
                            variant
                        } else {
                            path.clone()
                        }
                    },
                );
                out.push((rendered_path, profile.library_root.clone()));
            }
        } else {
            resolve_profile_for_family(dep, cfg, family, seen_files, stack, out)?;
        }
    }
    stack.pop();
    Ok(())
}

/// JSON output structure for list command.
#[derive(Debug, Serialize)]
struct ListOutput {
    profiles: Vec<ProfileInfo>,
    libraries: Vec<LibraryOutput>,
}

/// Profile information for JSON output.
#[derive(Debug, Serialize)]
struct ProfileInfo {
    name: String,
    dependencies: Vec<String>,
    library_root: String,
}

/// One library directory plus the fragment paths it contains.
#[derive(Debug, Serialize)]
struct LibraryOutput {
    path: String,
    fragments: Vec<String>,
}

/// List all available profiles to a writer.
///
/// Text mode: one profile name per line, sorted alphabetically.
///
/// JSON mode: emits profiles (each tagged with the library root it was loaded
/// against) plus a `libraries` array, one entry per unique library root seen
/// across the merged bundle.
///
/// # Errors
/// Returns an error if writing to the output fails or a library directory
/// cannot be read.
pub fn list_profiles(
    cfg: &Config,
    output: JsonOutput,
    mut w: impl Write,
) -> Result<(), PrompterError> {
    if output.is_json() {
        // Gather unique library roots from all loaded profiles.
        let mut unique_roots: Vec<PathBuf> = Vec::new();
        for profile in cfg.profiles.values() {
            if !unique_roots.contains(&profile.library_root) {
                unique_roots.push(profile.library_root.clone());
            }
        }
        unique_roots.sort();

        let mut libraries = Vec::with_capacity(unique_roots.len());
        for root in &unique_roots {
            let mut fragments = Vec::new();
            if root.exists() {
                collect_fragments(root, root, &mut fragments)?;
            }
            fragments.sort();
            libraries.push(LibraryOutput {
                path: root.display().to_string(),
                fragments,
            });
        }

        let mut profiles: Vec<ProfileInfo> = cfg
            .profiles
            .iter()
            .map(|(name, def)| ProfileInfo {
                name: name.clone(),
                dependencies: def.deps.clone(),
                library_root: def.library_root.display().to_string(),
            })
            .collect();
        profiles.sort_by(|a, b| a.name.cmp(&b.name));

        let data = serde_json::to_value(ListOutput {
            profiles,
            libraries,
        })?;
        writeln!(
            &mut w,
            "{}",
            render_response("list", JsonOutput::Json, data, String::new())
        )
        .map_err(PrompterError::Write)?;
    } else {
        let mut names: Vec<_> = cfg.profiles.keys().cloned().collect();
        names.sort();
        for n in names {
            writeln!(&mut w, "{n}").map_err(PrompterError::Write)?;
        }
    }
    Ok(())
}

/// Recursively collect all .md files from a directory
fn collect_fragments(
    root: &Path,
    dir: &Path,
    fragments: &mut Vec<String>,
) -> Result<(), PrompterError> {
    let entries = fs::read_dir(dir).map_err(|source| PrompterError::Io {
        path: dir.to_path_buf(),
        source,
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| PrompterError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();

        if path.is_dir() {
            collect_fragments(root, &path, fragments)?;
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            && let Ok(rel_path) = path.strip_prefix(root)
        {
            fragments.push(rel_path.display().to_string());
        }
    }

    Ok(())
}

fn inspect_family_variants(
    directory: &Path,
    families: &mut HashSet<FamilyName>,
    errors: &mut Vec<String>,
) -> Result<(), PrompterError> {
    let entries = fs::read_dir(directory).map_err(|source| PrompterError::Io {
        path: directory.to_path_buf(),
        source,
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| PrompterError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| PrompterError::Io {
            path: path.clone(),
            source,
        })?;
        if !file_type.is_dir() {
            continue;
        }

        if entry.file_name() == OsStr::new("families") {
            inspect_families_directory(directory, &path, families, errors)?;
        } else {
            inspect_family_variants(&path, families, errors)?;
        }
    }

    Ok(())
}

fn inspect_families_directory(
    fragment_directory: &Path,
    families_directory: &Path,
    families: &mut HashSet<FamilyName>,
    errors: &mut Vec<String>,
) -> Result<(), PrompterError> {
    let entries = fs::read_dir(families_directory).map_err(|source| PrompterError::Io {
        path: families_directory.to_path_buf(),
        source,
    })?;

    for entry in entries {
        let entry = entry.map_err(|source| PrompterError::Io {
            path: families_directory.to_path_buf(),
            source,
        })?;
        let family_directory = entry.path();
        let file_type = entry.file_type().map_err(|source| PrompterError::Io {
            path: family_directory.clone(),
            source,
        })?;
        if !file_type.is_dir() {
            continue;
        }

        let family_os = entry.file_name();
        let Some(family_text) = family_os.to_str() else {
            errors.push(format!(
                "Invalid non-UTF-8 family directory: {}",
                family_directory.display()
            ));
            continue;
        };
        let family = match FamilyName::new(family_text) {
            Ok(family) => family,
            Err(error) => {
                errors.push(format!("{error}: {}", family_directory.display()));
                continue;
            }
        };
        families.insert(family);

        let variants = fs::read_dir(&family_directory).map_err(|source| PrompterError::Io {
            path: family_directory.clone(),
            source,
        })?;
        for variant in variants {
            let variant = variant.map_err(|source| PrompterError::Io {
                path: family_directory.clone(),
                source,
            })?;
            let variant_path = variant.path();
            let variant_type = variant.file_type().map_err(|source| PrompterError::Io {
                path: variant_path.clone(),
                source,
            })?;
            if !variant_type.is_file()
                || !variant_path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
            {
                continue;
            }

            let neutral_path = fragment_directory.join(variant.file_name());
            if !neutral_path.is_file() {
                errors.push(format!(
                    "Orphan family variant: {} does not shadow neutral fragment {}",
                    variant_path.display(),
                    neutral_path.display()
                ));
            }
        }
    }

    Ok(())
}

/// Validate configuration and library file references.
///
/// Checks that all profile dependencies are valid, including:
/// - Referenced profiles exist in configuration
/// - Referenced markdown files exist in their owning library root
/// - No circular dependencies exist
///
/// # Errors
/// Returns an error if:
/// - Referenced profiles don't exist
/// - Referenced files don't exist
/// - Circular dependencies are detected
pub fn validate(cfg: &Config) -> Result<(), PrompterError> {
    let mut errors: Vec<String> = Vec::new();

    for (profile_name, profile) in &cfg.profiles {
        for dep in &profile.deps {
            if std::path::Path::new(dep)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            {
                let path = profile.library_root.join(dep);
                if !path.exists() {
                    errors.push(format!(
                        "Missing file: {} (referenced by [{}])",
                        path.display(),
                        profile_name
                    ));
                }
            } else if !cfg.profiles.contains_key(dep) {
                errors.push(format!(
                    "Unknown profile: {dep} (referenced by [{profile_name}])"
                ));
            }
        }
    }

    let mut family_set = HashSet::new();
    for library_root in cfg.library_roots() {
        inspect_family_variants(&library_root, &mut family_set, &mut errors)?;
    }
    let mut families: Vec<_> = family_set.into_iter().collect();
    families.sort();

    for name in cfg.profiles.keys() {
        for family in std::iter::once(None).chain(families.iter().map(Some)) {
            let mut seen_files = HashSet::new();
            let mut stack = Vec::new();
            let mut out = Vec::new();
            if let Err(error) =
                resolve_profile_for_family(name, cfg, family, &mut seen_files, &mut stack, &mut out)
            {
                let message = error.to_string();
                if !errors.contains(&message) {
                    errors.push(message);
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(PrompterError::Validation(errors.join("\n")))
    }
}

/// Build a tree node for a profile or fragment
fn build_tree_node(name: &str, cfg: &Config) -> TreeNode {
    // Check if it's a fragment (ends with .md)
    if std::path::Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
    {
        return TreeNode {
            node_type: TreeNodeType::Fragment,
            name: name.to_string(),
            children: Vec::new(),
        };
    }

    // It's a profile - recursively build children
    let children = cfg
        .profiles
        .get(name)
        .map(|def| {
            def.deps
                .iter()
                .map(|dep| build_tree_node(dep, cfg))
                .collect()
        })
        .unwrap_or_default();

    TreeNode {
        node_type: TreeNodeType::Profile,
        name: name.to_string(),
        children,
    }
}

/// Find root profiles (profiles that are not referenced by any other profile)
fn find_root_profiles(cfg: &Config) -> Vec<String> {
    let mut referenced = HashSet::new();

    // Collect all profiles that are referenced by others
    for profile in cfg.profiles.values() {
        for dep in &profile.deps {
            // Only track profile references (not .md files)
            if !std::path::Path::new(dep)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            {
                referenced.insert(dep.clone());
            }
        }
    }

    // Find profiles that are never referenced
    let mut roots: Vec<String> = cfg
        .profiles
        .keys()
        .filter(|profile| !referenced.contains(*profile))
        .cloned()
        .collect();

    roots.sort();
    roots
}

/// Build complete tree structure for all root profiles
fn build_trees(cfg: &Config) -> TreeOutput {
    let root_profiles = find_root_profiles(cfg);
    let trees = root_profiles
        .iter()
        .map(|profile| build_tree_node(profile, cfg))
        .collect();

    TreeOutput { trees }
}

/// Print tree structure in traditional tree format
fn print_tree(node: &TreeNode, prefix: &str, is_last: bool, w: &mut impl Write) -> io::Result<()> {
    // Print current node with appropriate connector
    let connector = if is_last { "└── " } else { "├── " };
    writeln!(w, "{prefix}{connector}{}", node.name)?;

    // Prepare prefix for children
    let child_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });

    // Print children
    for (i, child) in node.children.iter().enumerate() {
        let is_last_child = i == node.children.len() - 1;
        print_tree(child, &child_prefix, is_last_child, w)?;
    }

    Ok(())
}

/// Show tree structure for all profiles.
///
/// # Errors
///
/// Returns an error when tree serialization or writing to the supplied output
/// fails.
pub fn show_tree(cfg: &Config, output: JsonOutput, mut w: impl Write) -> Result<(), PrompterError> {
    let trees = build_trees(cfg);

    if output.is_json() {
        let data = serde_json::to_value(&trees)?;
        writeln!(
            &mut w,
            "{}",
            render_response("tree", JsonOutput::Json, data, String::new())
        )
        .map_err(PrompterError::Write)?;
    } else {
        for (i, tree) in trees.trees.iter().enumerate() {
            // Print root profile name
            writeln!(&mut w, "{}", tree.name).map_err(PrompterError::Write)?;

            // Print children with tree structure
            for (j, child) in tree.children.iter().enumerate() {
                let is_last = j == tree.children.len() - 1;
                print_tree(child, "", is_last, &mut w).map_err(PrompterError::Write)?;
            }

            // Add blank line between trees (except after last one)
            if i < trees.trees.len() - 1 {
                writeln!(&mut w).map_err(PrompterError::Write)?;
            }
        }
    }

    Ok(())
}

/// Show tree structure to stdout.
///
/// # Errors
///
/// Returns an error when configuration loading, tree serialization, or writing
/// to standard output fails.
pub fn run_tree_stdout(
    config_override: Option<&Path>,
    output: JsonOutput,
) -> Result<(), PrompterError> {
    let (_cfg_path, cfg) = load_bundle(config_override)?;
    show_tree(&cfg, output, io::stdout())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique, per-call temp path that never collides across tests in this
    /// module. Mirrors the `src/types.rs` exemplar idiom.
    fn unique_temp_path(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "tftio-lib-profile-{label}-{}-{n}",
            std::process::id()
        ))
    }

    /// Build a [`Config`] whose every profile shares a single library root.
    fn cfg_with_lib<I>(profiles: I, lib: &Path) -> Config
    where
        I: IntoIterator<Item = (&'static str, Vec<&'static str>)>,
    {
        let profiles = profiles
            .into_iter()
            .map(|(name, deps)| {
                (
                    name.to_string(),
                    ProfileDef {
                        deps: deps.into_iter().map(String::from).collect(),
                        library_root: lib.to_path_buf(),
                    },
                )
            })
            .collect();
        Config {
            profiles,
            post_prompt: None,
        }
    }

    #[test]
    fn family_name_display_renders_inner_value() {
        let family = FamilyName::new("gpt").unwrap();
        assert_eq!(family.to_string(), "gpt");
        assert_eq!(format!("{family}"), "gpt");
    }

    #[test]
    fn collect_profiles_ignores_non_table_value() {
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        collect_profiles("scalar", &toml::Value::Integer(5), &mut out).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn collect_profiles_rejects_non_string_depends_on_entries() {
        let value: toml::Value = toml::from_str("depends_on = [1]").unwrap();
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        let err = collect_profiles("p", &value, &mut out).unwrap_err();
        assert!(matches!(err, PrompterError::ConfigField(_)));
        assert!(err.to_string().contains("must be strings"), "err={err}");
    }

    #[test]
    fn collect_profiles_detects_duplicate_prefix() {
        let value: toml::Value = toml::from_str("depends_on = [\"a.md\"]").unwrap();
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        out.insert("p".to_string(), vec!["preexisting".to_string()]);
        let err = collect_profiles("p", &value, &mut out).unwrap_err();
        assert!(matches!(err, PrompterError::DuplicateProfile(_)));
        assert!(err.to_string().contains("Duplicate profile"), "err={err}");
    }

    #[test]
    fn collect_profiles_handles_empty_prefix() {
        // With an empty prefix the nested key becomes the bare profile name
        // rather than a dotted path.
        let value: toml::Value = toml::from_str("[x]\ndepends_on = [\"a.md\"]").unwrap();
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        collect_profiles("", &value, &mut out).unwrap();
        assert_eq!(out.get("x").map(Vec::len), Some(1));
        assert_eq!(
            out.get("x").map(Vec::as_slice),
            Some(["a.md".to_string()].as_slice())
        );
    }

    #[test]
    fn load_config_bundle_resolves_relative_import() {
        let dir = unique_temp_path("relimport");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("child.toml"), "[imported]\ndepends_on = []\n").unwrap();
        fs::write(
            dir.join("config.toml"),
            "import = [\"child.toml\"]\n\n[main]\ndepends_on = []\n",
        )
        .unwrap();

        let cfg = load_config_bundle(&dir.join("config.toml"), None).unwrap();
        assert!(cfg.profiles.contains_key("main"));
        assert!(cfg.profiles.contains_key("imported"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_config_bundle_honors_absolute_library_key() {
        let dir = unique_temp_path("abslib-cfg");
        fs::create_dir_all(&dir).unwrap();
        let abs_lib = unique_temp_path("abslib-target");
        fs::create_dir_all(&abs_lib).unwrap();
        assert!(abs_lib.is_absolute());

        let config_text = format!(
            "library = \"{}\"\n\n[p]\ndepends_on = []\n",
            abs_lib.display()
        );
        fs::write(dir.join("config.toml"), config_text).unwrap();

        let cfg = load_config_bundle(&dir.join("config.toml"), None).unwrap();
        assert_eq!(cfg.profiles.get("p").unwrap().library_root, abs_lib);

        fs::remove_dir_all(&dir).ok();
        fs::remove_dir_all(&abs_lib).ok();
    }

    #[test]
    fn list_profiles_json_emits_sorted_libraries_and_fragments() {
        let lib = unique_temp_path("list-json");
        fs::create_dir_all(lib.join("sub")).unwrap();
        fs::write(lib.join("top.md"), b"T").unwrap();
        fs::write(lib.join("sub/nested.md"), b"N").unwrap();
        // A non-markdown sibling must be excluded from the fragment listing.
        fs::write(lib.join("notes.txt"), b"ignore").unwrap();

        // Two profiles share one library root, exercising both the insert and
        // the dedup-skip paths of the unique-root gather.
        let cfg = cfg_with_lib(
            [("beta", vec!["top.md"]), ("alpha", vec!["sub/nested.md"])],
            &lib,
        );

        let mut out = Vec::new();
        list_profiles(&cfg, JsonOutput::Json, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        let value: serde_json::Value = serde_json::from_str(text.trim()).unwrap();

        assert_eq!(value["ok"], serde_json::json!(true));
        assert_eq!(value["command"], serde_json::json!("list"));

        let profiles = value["data"]["profiles"].as_array().unwrap();
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0]["name"], serde_json::json!("alpha"));
        assert_eq!(profiles[1]["name"], serde_json::json!("beta"));

        let libraries = value["data"]["libraries"].as_array().unwrap();
        assert_eq!(libraries.len(), 1);
        assert_eq!(
            libraries[0]["path"],
            serde_json::json!(lib.display().to_string())
        );
        let fragments: Vec<&str> = libraries[0]["fragments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f.as_str().unwrap())
            .collect();
        assert_eq!(fragments, vec!["sub/nested.md", "top.md"]);

        fs::remove_dir_all(&lib).ok();
    }

    #[test]
    fn list_profiles_json_skips_absent_library_root() {
        // A library root that does not exist yields an empty fragment listing
        // rather than an error.
        let absent = unique_temp_path("absent-root");
        let cfg = cfg_with_lib([("p", vec![])], &absent);
        let mut out = Vec::new();
        list_profiles(&cfg, JsonOutput::Json, &mut out).unwrap();
        let envelope: serde_json::Value =
            serde_json::from_str(std::str::from_utf8(&out).unwrap().trim()).unwrap();
        let libraries = envelope["data"]["libraries"].as_array().unwrap();
        assert_eq!(libraries.len(), 1);
        assert!(libraries[0]["fragments"].as_array().unwrap().is_empty());
    }

    #[test]
    fn list_profiles_json_errors_when_library_root_is_a_file() {
        // A library root that exists but is a regular file surfaces the
        // directory-read failure as a typed I/O error.
        let dir = unique_temp_path("root-is-file");
        fs::create_dir_all(&dir).unwrap();
        let file_root = dir.join("not-a-dir");
        fs::write(&file_root, b"x").unwrap();
        let cfg = cfg_with_lib([("p", vec![])], &file_root);
        let mut out = Vec::new();
        let err = list_profiles(&cfg, JsonOutput::Json, &mut out).unwrap_err();
        assert!(matches!(err, PrompterError::Io { .. }), "err={err}");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn validate_errors_when_library_root_is_unreadable() {
        // Family-variant inspection reads each library root; a missing root
        // surfaces a typed I/O error rather than being silently skipped.
        let absent = unique_temp_path("validate-absent-root");
        let cfg = cfg_with_lib([("p", vec![])], &absent);
        let err = validate(&cfg).unwrap_err();
        assert!(matches!(err, PrompterError::Io { .. }), "err={err}");
    }

    #[test]
    fn collect_fragments_errors_on_missing_directory() {
        let missing = unique_temp_path("collect-missing");
        let mut fragments = Vec::new();
        let err = collect_fragments(&missing, &missing, &mut fragments).unwrap_err();
        match err {
            PrompterError::Io { path, .. } => assert_eq!(path, missing),
            other => panic!("expected Io error, got {other:?}"),
        }
    }

    #[test]
    fn inspect_family_variants_errors_on_missing_directory() {
        let missing = unique_temp_path("inspect-missing");
        let mut families = HashSet::new();
        let mut errors = Vec::new();
        let err = inspect_family_variants(&missing, &mut families, &mut errors).unwrap_err();
        match err {
            PrompterError::Io { path, .. } => assert_eq!(path, missing),
            other => panic!("expected Io error, got {other:?}"),
        }
    }

    #[test]
    fn inspect_families_directory_errors_on_missing_directory() {
        let fragment_dir = unique_temp_path("families-frag");
        let missing_families = fragment_dir.join("families");
        let mut families = HashSet::new();
        let mut errors = Vec::new();
        let err = inspect_families_directory(
            &fragment_dir,
            &missing_families,
            &mut families,
            &mut errors,
        )
        .unwrap_err();
        match err {
            PrompterError::Io { path, .. } => assert_eq!(path, missing_families),
            other => panic!("expected Io error, got {other:?}"),
        }
    }

    #[test]
    fn validate_skips_non_directory_family_entries() {
        let lib = unique_temp_path("family-skips");
        fs::create_dir_all(lib.join("general/families/gpt")).unwrap();
        fs::write(lib.join("general/rules.md"), b"NEUTRAL").unwrap();
        // Variant that correctly shadows the neutral fragment (no orphan).
        fs::write(lib.join("general/families/gpt/rules.md"), b"GPT").unwrap();
        // A plain file directly under `families/` is not a family directory.
        fs::write(lib.join("general/families/README.txt"), b"not a dir").unwrap();
        // A non-markdown entry inside a family directory is ignored.
        fs::write(lib.join("general/families/gpt/notes.txt"), b"not md").unwrap();

        let cfg = cfg_with_lib([("root", vec!["general/rules.md"])], &lib);
        assert!(validate(&cfg).is_ok());

        fs::remove_dir_all(&lib).ok();
    }

    #[test]
    fn show_tree_text_renders_deterministic_tree() {
        // `build_tree_node`/`find_root_profiles` do not touch the filesystem;
        // the library root is only a struct field here.
        let lib = unique_temp_path("tree-text");
        let cfg = cfg_with_lib(
            [
                ("alpha", vec!["beta", "x.md", "phantom"]),
                ("beta", vec!["y.md", "z.md"]),
                ("gamma", vec!["w.md"]),
            ],
            &lib,
        );

        let mut out = Vec::new();
        show_tree(&cfg, JsonOutput::Text, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();

        // Roots are sorted (alpha, gamma); `phantom` is an unknown profile with
        // no children; connectors and prefixes are deterministic.
        let expected = concat!(
            "alpha\n",
            "├── beta\n",
            "│   ├── y.md\n",
            "│   └── z.md\n",
            "├── x.md\n",
            "└── phantom\n",
            "\n",
            "gamma\n",
            "└── w.md\n",
        );
        assert_eq!(text, expected);
    }

    #[test]
    fn show_tree_json_serializes_trees() {
        let lib = unique_temp_path("tree-json");
        let cfg = cfg_with_lib(
            [("alpha", vec!["beta", "x.md"]), ("beta", vec!["y.md"])],
            &lib,
        );

        let mut out = Vec::new();
        show_tree(&cfg, JsonOutput::Json, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        let value: serde_json::Value = serde_json::from_str(text.trim()).unwrap();

        assert_eq!(value["ok"], serde_json::json!(true));
        assert_eq!(value["command"], serde_json::json!("tree"));

        let trees = value["data"]["trees"].as_array().unwrap();
        assert_eq!(trees.len(), 1);
        assert_eq!(trees[0]["name"], serde_json::json!("alpha"));
        assert_eq!(trees[0]["type"], serde_json::json!("profile"));

        let children = trees[0]["children"].as_array().unwrap();
        assert_eq!(children[0]["name"], serde_json::json!("beta"));
        assert_eq!(children[0]["type"], serde_json::json!("profile"));
        assert_eq!(
            children[0]["children"][0]["name"],
            serde_json::json!("y.md")
        );
        assert_eq!(
            children[0]["children"][0]["type"],
            serde_json::json!("fragment")
        );
        assert_eq!(children[1]["name"], serde_json::json!("x.md"));
        assert_eq!(children[1]["type"], serde_json::json!("fragment"));
        // A leaf fragment serializes without an empty `children` array.
        assert!(children[1].get("children").is_none());
    }

    #[test]
    fn run_tree_stdout_with_explicit_config_succeeds() {
        let root = unique_temp_path("tree-stdout");
        fs::create_dir_all(root.join("library/a")).unwrap();
        fs::write(root.join("library/a/x.md"), b"AX").unwrap();
        let config = root.join("config.toml");
        fs::write(
            &config,
            "[child]\ndepends_on = [\"a/x.md\"]\n\n[root]\ndepends_on = [\"child\"]\n",
        )
        .unwrap();

        assert!(run_tree_stdout(Some(&config), JsonOutput::Text).is_ok());
        assert!(run_tree_stdout(Some(&config), JsonOutput::Json).is_ok());

        fs::remove_dir_all(&root).ok();
    }
}
