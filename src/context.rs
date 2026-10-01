//! Context resolution and `.clanker` discovery for harness launches.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::axis::{self, InvalidAxisValue, Resolved};
use crate::config::{Config, ContextName, DomainName, ModelName};

/// A resolved context and the precedence tier that supplied it.
pub type ResolvedContext = Resolved<ContextName>;

/// Parsed `.clanker` values shared by all launch axes.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryConfig {
    /// Optional context selection.
    pub context: Option<ContextName>,
    /// Optional domain selection used by subcommand mode.
    pub domain: Option<DomainName>,
    /// Optional model selection used by subcommand mode.
    pub model: Option<ModelName>,
    /// Optional project declaration.
    ///
    /// Present only so this strict parser accepts a `.clanker` carrying a
    /// `project` key; the value is never read from here. The exported
    /// project comes solely from [`tftio_lib::project`]'s root-anchored
    /// read of `<project root>/.clanker`, which does not walk ancestors the
    /// way [`read_directory_config`] does, so this field is excluded from
    /// the private `inherit_from` merge across ancestors.
    pub project: Option<tftio_lib::project::Slug>,
}

impl DirectoryConfig {
    /// Fill unset fields from a config found farther up the directory chain.
    ///
    /// `project` is deliberately excluded from this merge: it keeps only
    /// the nearer file's own value rather than inheriting a farther
    /// ancestor's, because it is never consumed for behavior here anyway.
    fn inherit_from(self, farther: Self) -> Self {
        Self {
            context: self.context.or(farther.context),
            domain: self.domain.or(farther.domain),
            model: self.model.or(farther.model),
            project: self.project,
        }
    }
}

/// Context resolution failures at environment and dotfile boundaries.
#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    /// An axis environment value is not a safe name.
    #[error(transparent)]
    Axis(#[from] InvalidAxisValue),
    /// A context file could not be read.
    #[error("failed to read context file {path}: {source}")]
    Read {
        /// Failing path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// `.clanker` TOML is invalid.
    #[error("failed to parse directory config {path}: {source}")]
    Parse {
        /// Failing path.
        path: PathBuf,
        /// TOML parse error.
        source: toml::de::Error,
    },
    /// A `.clanker` exists as a directory rather than a flat TOML file.
    #[error(
        "{path} is a directory; clanker reads a flat .clanker TOML file, so replace the directory with a file holding its config.toml contents"
    )]
    DirectoryForm {
        /// Offending path.
        path: PathBuf,
    },
    /// No tier supplied a context.
    #[error(
        "no context selected; pass --context, add a .clanker holding `context = \"...\"` to this directory or an ancestor, or set CLANKER_CONTEXT"
    )]
    Unresolved,
    /// The resolved context is not in the configured registry.
    #[error(
        "unknown context `{name}` from {origin}; add it to defaults.contexts or fix the selection"
    )]
    UnknownContext {
        /// Rejected context name.
        name: ContextName,
        /// Winning precedence tier that supplied it.
        origin: &'static str,
    },
}

/// Resolve context using the pinned precedence chain.
///
/// # Errors
/// Returns [`ContextError`] when a supplied name is invalid, a present dotfile
/// cannot be read or parsed, no tier supplies a context, or the resolved
/// context is not registered.
pub fn resolve_context(
    current_dir: &Path,
    command_line: Option<&ContextName>,
    context_environment: Option<&str>,
    config: &Config,
) -> Result<ResolvedContext, ContextError> {
    let directory_config = read_directory_config(current_dir)?;
    resolve_context_with_directory(
        command_line,
        context_environment,
        config,
        directory_config.as_ref(),
    )
}

/// Resolve context using an already-read `.clanker` document.
///
/// # Errors
/// Returns [`ContextError`] when the environment value is invalid, no tier
/// supplies a context, or the resolved context is not in the configured
/// registry.
pub fn resolve_context_with_directory(
    command_line: Option<&ContextName>,
    context_environment: Option<&str>,
    config: &Config,
    directory_config: Option<&DirectoryConfig>,
) -> Result<ResolvedContext, ContextError> {
    let resolved = axis::resolve(
        axis::CONTEXT,
        command_line.cloned(),
        directory_config.and_then(|directory| directory.context.clone()),
        context_environment,
        ContextName::new,
        None,
    )?
    .ok_or(ContextError::Unresolved)?;

    if config.defaults.contexts.contains(&resolved.value) {
        Ok(resolved)
    } else {
        Err(ContextError::UnknownContext {
            name: resolved.value,
            origin: resolved.source.label(axis::CONTEXT),
        })
    }
}

/// Read every `.clanker` at or above the current directory, nearest wins per field.
///
/// Each ancestor that carries a `.clanker` contributes only the fields it sets,
/// so a repo-local config that selects a domain does not discard a context
/// inherited from an umbrella directory above it. The walk reaches the
/// filesystem root, so a `~/.clanker` applies to everything under it.
///
/// # Errors
/// Returns [`ContextError`] when a config exists but cannot be read or parsed,
/// or when a `.clanker` is a directory rather than a flat file.
pub fn read_directory_config(current_dir: &Path) -> Result<Option<DirectoryConfig>, ContextError> {
    let mut merged: Option<DirectoryConfig> = None;
    for directory in current_dir.ancestors() {
        let Some(farther) = read_one_directory_config(directory)? else {
            continue;
        };
        merged = Some(match merged {
            Some(nearer) => nearer.inherit_from(farther),
            None => farther,
        });
    }
    Ok(merged)
}

fn read_one_directory_config(directory: &Path) -> Result<Option<DirectoryConfig>, ContextError> {
    let path = directory.join(".clanker");
    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(ContextError::Read { path, source }),
    };

    // A directory-form `.clanker` is a stale artifact of the removed bake
    // command. Refusing it is deliberate: silently skipping would let an
    // out-of-date selection be ignored rather than corrected.
    if metadata.is_dir() {
        return Err(ContextError::DirectoryForm { path });
    }

    parse_directory_config_file(&path).map(Some)
}

fn parse_directory_config_file(path: &Path) -> Result<DirectoryConfig, ContextError> {
    let text = fs::read_to_string(path).map_err(|source| ContextError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    toml::from_str(&text).map_err(|source| ContextError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axis::AxisSource;
    use crate::config::load_config_with_overlay;
    use tempfile::TempDir;

    fn fixture_config(directory: &Path) -> Config {
        let path = directory.join("config.toml");
        fs::write(
            &path,
            r#"
[defaults]
domain = "eng"
contexts = ["personal", "work"]
shim_path = "~/.local/clankers/bin"
sandbox_wrapper = "~/.config/sandbox-exec/run-sandboxed.sh"
prompter_bundle = "~/.local/prompter"
prompt_cache = "~/.cache/clanker/prompts"
prompt_cache_ttl_seconds = 86400

[harness.claude]
bin = "claude"
family = "claude"
default_args = []
[harness.claude.injection]
kind = "arg-text"
args = ["--append-system-prompt", "{text}"]

[model]

[domain.eng]
profiles = ["core.base", "domain.eng"]
env = {}
"#,
        )
        .unwrap();
        load_config_with_overlay(&path, None).unwrap().config
    }

    #[test]
    fn clanker_file_beats_environment() {
        let temp = TempDir::new().unwrap();
        let config = fixture_config(temp.path());
        fs::write(temp.path().join(".clanker"), "context = \"work\"\n").unwrap();

        let resolved = resolve_context(temp.path(), None, Some("personal"), &config).unwrap();

        assert_eq!(resolved.value.as_str(), "work");
        assert_eq!(resolved.source, AxisSource::ClankerFile);
    }

    #[test]
    fn nearest_ancestor_clanker_wins() {
        let temp = TempDir::new().unwrap();
        let config = fixture_config(temp.path());
        let parent = temp.path().join("work");
        let current = parent.join("repo/src");
        fs::create_dir_all(&current).unwrap();
        fs::write(parent.join(".clanker"), "context = \"work\"\n").unwrap();

        let inherited = resolve_context(&current, None, Some("personal"), &config).unwrap();
        assert_eq!(inherited.value.as_str(), "work");
        assert_eq!(inherited.source, AxisSource::ClankerFile);

        fs::write(current.join(".clanker"), "context = \"personal\"\n").unwrap();
        let nearest = resolve_context(&current, None, Some("work"), &config).unwrap();
        assert_eq!(nearest.value.as_str(), "personal");
    }

    #[test]
    fn a_directory_form_clanker_fails_closed() {
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.path().join(".clanker")).unwrap();

        let error = read_directory_config(temp.path()).unwrap_err();

        let message = error.to_string();
        assert!(message.contains("is a directory"));
        assert!(message.contains("flat .clanker TOML file"));
    }

    #[test]
    fn nearer_config_inherits_the_fields_it_leaves_unset() {
        let temp = TempDir::new().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(
            temp.path().join(".clanker"),
            "context = \"work\"\ndomain = \"research\"\nmodel = \"codex-native\"\n",
        )
        .unwrap();
        fs::write(repo.join(".clanker"), "domain = \"eng\"\n").unwrap();

        let merged = read_directory_config(&repo).unwrap().unwrap();

        assert_eq!(merged.domain.unwrap().as_str(), "eng", "nearer wins");
        assert_eq!(merged.context.unwrap().as_str(), "work", "farther fills in");
        assert_eq!(merged.model.unwrap().as_str(), "codex-native");
    }

    #[test]
    fn three_ancestor_levels_merge_per_field_nearest_first() {
        let temp = TempDir::new().unwrap();
        let middle = temp.path().join("middle");
        let leaf = middle.join("leaf");
        fs::create_dir_all(&leaf).unwrap();
        fs::write(temp.path().join(".clanker"), "context = \"work\"\n").unwrap();
        fs::write(middle.join(".clanker"), "domain = \"research\"\n").unwrap();
        fs::write(leaf.join(".clanker"), "model = \"codex-native\"\n").unwrap();

        let merged = read_directory_config(&leaf).unwrap().unwrap();

        assert_eq!(merged.context.unwrap().as_str(), "work");
        assert_eq!(merged.domain.unwrap().as_str(), "research");
        assert_eq!(merged.model.unwrap().as_str(), "codex-native");
    }

    #[test]
    fn a_field_no_ancestor_sets_stays_unset() {
        let temp = TempDir::new().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(temp.path().join(".clanker"), "context = \"work\"\n").unwrap();

        let merged = read_directory_config(&repo).unwrap().unwrap();

        assert_eq!(merged.context.unwrap().as_str(), "work");
        assert!(merged.domain.is_none());
        assert!(merged.model.is_none());
    }

    #[test]
    fn an_unresolved_context_names_every_supplying_tier() {
        let temp = TempDir::new().unwrap();
        let config = fixture_config(temp.path());

        let error = resolve_context(temp.path(), None, None, &config).unwrap_err();

        let message = error.to_string();
        assert!(message.contains("--context"));
        assert!(message.contains(".clanker"));
        assert!(message.contains("CLANKER_CONTEXT"));
    }

    #[test]
    fn unregistered_contexts_are_rejected_per_tier() {
        let temp = TempDir::new().unwrap();
        let config = fixture_config(temp.path());

        let command_line = ContextName::new("staging").unwrap();
        let error = resolve_context(temp.path(), Some(&command_line), None, &config).unwrap_err();
        assert!(error.to_string().contains("unknown context `staging`"));
        assert!(error.to_string().contains("--context"));

        let error = resolve_context(temp.path(), None, Some("staging"), &config).unwrap_err();
        assert!(error.to_string().contains("CLANKER_CONTEXT"));

        fs::write(temp.path().join(".clanker"), "context = \"staging\"\n").unwrap();
        let error = resolve_context(temp.path(), None, None, &config).unwrap_err();
        assert!(error.to_string().contains(".clanker"));
    }

    #[test]
    fn an_invalid_environment_context_names_the_variable() {
        let temp = TempDir::new().unwrap();
        let config = fixture_config(temp.path());

        let error = resolve_context(temp.path(), None, Some("../escape"), &config).unwrap_err();

        assert!(matches!(error, ContextError::Axis(_)));
        assert!(error.to_string().contains("CLANKER_CONTEXT"));
    }

    #[test]
    fn an_unreadable_clanker_file_is_a_read_error() {
        let temp = TempDir::new().unwrap();
        // A `.clanker` whose bytes cannot be read as UTF-8 text fails at the read
        // boundary rather than resolving to a default.
        fs::write(temp.path().join(".clanker"), [0xff, 0xfe, 0x00]).unwrap();

        let error = read_directory_config(temp.path()).unwrap_err();
        assert!(matches!(error, ContextError::Read { .. }));
    }

    #[test]
    fn a_clanker_holding_only_a_project_key_parses() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join(".clanker"), "project = \"kb\"\n").unwrap();

        let merged = read_directory_config(temp.path()).unwrap().unwrap();

        assert_eq!(merged.project.unwrap().as_str(), "kb");
        assert!(merged.context.is_none());
        assert!(merged.domain.is_none());
        assert!(merged.model.is_none());
    }

    #[test]
    fn a_directory_config_project_key_does_not_inherit_from_a_farther_ancestor() {
        let temp = TempDir::new().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(temp.path().join(".clanker"), "project = \"umbrella\"\n").unwrap();
        fs::write(repo.join(".clanker"), "context = \"work\"\n").unwrap();

        let merged = read_directory_config(&repo).unwrap().unwrap();

        assert_eq!(merged.context.unwrap().as_str(), "work");
        assert!(
            merged.project.is_none(),
            "the nearer file set no project, so the farther one is not inherited"
        );
    }

    #[test]
    fn invalid_clanker_toml_is_a_parse_error() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join(".clanker"), "context = \n").unwrap();

        let error = read_directory_config(temp.path()).unwrap_err();
        assert!(matches!(error, ContextError::Parse { .. }));
    }
}
