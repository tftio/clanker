//! Resolved harness configuration-directory inventory.
//!
//! Launch resolves each harness's `config_dir` template, applying the context
//! substitution and provider derivation owned by [`crate::config`]. This module
//! exposes the same resolution as data, including directories that do not
//! exist, so tooling can enumerate every place persistent harness content may
//! live.

use std::path::{Path, PathBuf};

use crate::config::{Config, ContextName, HarnessName, ProviderName, provider_template};
use crate::launch::{LaunchError, expand_path};

const CONTEXT_PLACEHOLDER: &str = "{context}";

/// One resolved configuration directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    /// Harness that declares the directory.
    pub harness: HarnessName,
    /// Context the directory was rendered for.
    pub context: ContextName,
    /// Provider whose derived directory this is, or `None` for the harness's
    /// own directory.
    pub provider: Option<ProviderName>,
    /// Environment variable the harness receives for this directory.
    pub variable: String,
    /// Absolute or `~`-expanded path, as a launch would export it.
    pub path: PathBuf,
    /// Whether the path currently exists as a directory.
    pub exists: bool,
}

/// Optional restrictions on the inventory.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Filter<'a> {
    /// Include only this context.
    pub context: Option<&'a str>,
    /// Include only rows derived for this provider. Native rows are excluded
    /// when this is set.
    pub provider: Option<&'a str>,
}

/// Failures resolving the configuration-directory inventory.
#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    /// A directory template uses `~` but the home directory is unavailable.
    #[error("$HOME is not set; a configuration directory template uses `~`")]
    HomeNotSet,
    /// The context filter names a context absent from `defaults.contexts`.
    #[error("unknown context `{0}`; it is not in defaults.contexts")]
    UnknownContext(String),
    /// The provider filter names a provider absent from `[provider.*]`.
    #[error("unknown provider `{0}`; add [provider.{0}] to clanker config")]
    UnknownProvider(String),
    /// A configured path could not be expanded.
    #[error(transparent)]
    Path(#[from] LaunchError),
}

/// Enumerate every resolved configuration directory.
///
/// Native rows are emitted for every harness that declares a `config_dir`, in
/// every context the harness supports. A provider row is emitted for every
/// configured provider, derived from the harness's own template exactly as a
/// provider launch derives it. Missing directories are reported with
/// [`DirectoryEntry::exists`] set to `false`; they do not fail resolution.
///
/// # Errors
/// Returns [`LayoutError`] when a filter names an unconfigured context or
/// provider, when a template using `~` has no home directory, or when a
/// configured path uses unsupported tilde syntax.
pub fn directories(
    config: &Config,
    home: Option<&Path>,
    filter: Filter<'_>,
) -> Result<Vec<DirectoryEntry>, LayoutError> {
    if let Some(context) = filter.context
        && !config
            .defaults
            .contexts
            .iter()
            .any(|configured| configured.as_str() == context)
    {
        return Err(LayoutError::UnknownContext(context.to_string()));
    }
    if let Some(provider) = filter.provider
        && !config
            .provider
            .keys()
            .any(|configured| configured.as_str() == provider)
    {
        return Err(LayoutError::UnknownProvider(provider.to_string()));
    }

    let mut entries = Vec::new();
    for (harness_name, harness) in &config.harness {
        let Some(config_dir) = &harness.config_dir else {
            continue;
        };
        for context in &config.defaults.contexts {
            if !harness.supports_context(context)
                || filter
                    .context
                    .is_some_and(|wanted| wanted != context.as_str())
            {
                continue;
            }
            for (variable, template) in config_dir.entries() {
                if filter.provider.is_none() {
                    push_entry(
                        &mut entries,
                        harness_name,
                        context,
                        None,
                        variable,
                        template,
                        home,
                    )?;
                }
                for provider_name in config.provider.keys() {
                    if filter
                        .provider
                        .is_some_and(|wanted| wanted != provider_name.as_str())
                    {
                        continue;
                    }
                    let derived = provider_template(template, provider_name);
                    push_entry(
                        &mut entries,
                        harness_name,
                        context,
                        Some(provider_name),
                        variable,
                        &derived,
                        home,
                    )?;
                }
            }
        }
    }

    entries.sort_by(|left, right| {
        (
            left.harness.as_str(),
            left.context.as_str(),
            left.provider.as_ref().map(ProviderName::as_str),
            left.variable.as_str(),
        )
            .cmp(&(
                right.harness.as_str(),
                right.context.as_str(),
                right.provider.as_ref().map(ProviderName::as_str),
                right.variable.as_str(),
            ))
    });
    Ok(entries)
}

fn push_entry(
    entries: &mut Vec<DirectoryEntry>,
    harness: &HarnessName,
    context: &ContextName,
    provider: Option<&ProviderName>,
    variable: &str,
    template: &str,
    home: Option<&Path>,
) -> Result<(), LayoutError> {
    let configured = template.replace(CONTEXT_PLACEHOLDER, context.as_str());
    let path = resolve_path(&configured, home)?;
    let exists = path.is_dir();
    entries.push(DirectoryEntry {
        harness: harness.clone(),
        context: context.clone(),
        provider: provider.cloned(),
        variable: variable.to_string(),
        path,
        exists,
    });
    Ok(())
}

fn resolve_path(configured: &str, home: Option<&Path>) -> Result<PathBuf, LayoutError> {
    match home {
        Some(home) => expand_path(configured, home).map_err(Into::into),
        None if configured.starts_with('~') => Err(LayoutError::HomeNotSet),
        None => Ok(PathBuf::from(configured)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const CONFIG: &str = r#"
[defaults]
domain = "eng"
contexts = ["personal", "work"]
shim_path = "~/unused"
sandbox_wrapper = "~/unused"
prompter_bundle = "~/unused"
prompt_cache = "~/unused"
prompt_cache_ttl_seconds = 1

[harness.claude]
bin = "claude"
family = "claude"
default_args = []
[harness.claude.injection]
kind = "arg-text"
args = ["{text}"]
[harness.claude.config_dir]
environment = "CLAUDE_CONFIG_DIR"
path = "~/.config/claude/{context}"

[harness.fixed]
bin = "fixed"
family = "gpt"
contexts = ["personal"]
default_args = []
[harness.fixed.injection]
kind = "arg-text"
args = ["{text}"]
[harness.fixed.config_dir]
environment = "FIXED_CONFIG"
path = "~/.fixed"

[harness.polytoken]
bin = "polytoken"
family = "gpt"
default_args = ["new"]
[harness.polytoken.injection]
kind = "env-file"
environment = "CLANKER_PROMPT_FILE"
[harness.polytoken.config_dir.directories]
POLYTOKEN_CONFIG_PATH = "~/.config/polytoken/{context}"
POLYTOKEN_DATA_PATH = "~/.local/share/polytoken/{context}"

[harness.plain]
bin = "plain"
family = "gpt"
default_args = []
[harness.plain.injection]
kind = "arg-text"
args = ["{text}"]

[provider.zai]
[provider.deepseek]

[model]
[domain.eng]
profiles = ["core.base"]
env = {}
"#;

    fn config() -> Config {
        toml::from_str(CONFIG).unwrap()
    }

    fn entries(filter: Filter<'_>) -> Vec<DirectoryEntry> {
        directories(&config(), Some(Path::new("/home")), filter).unwrap()
    }

    fn find<'a>(
        entries: &'a [DirectoryEntry],
        harness: &str,
        context: &str,
        provider: Option<&str>,
    ) -> &'a DirectoryEntry {
        entries
            .iter()
            .find(|entry| {
                entry.harness.as_str() == harness
                    && entry.context.as_str() == context
                    && entry.provider.as_ref().map(ProviderName::as_str) == provider
            })
            .unwrap()
    }

    #[test]
    fn native_rows_cover_every_supported_context() {
        let rows = entries(Filter::default());
        assert_eq!(
            find(&rows, "claude", "personal", None).path,
            PathBuf::from("/home/.config/claude/personal")
        );
        assert_eq!(
            find(&rows, "claude", "work", None).path,
            PathBuf::from("/home/.config/claude/work")
        );
        assert_eq!(
            find(&rows, "claude", "personal", None).variable,
            "CLAUDE_CONFIG_DIR"
        );
    }

    #[test]
    fn provider_rows_are_derived_for_every_context_and_provider() {
        let rows = entries(Filter::default());
        for (context, provider, expected) in [
            ("personal", "zai", "/home/.config/claude-zai/personal"),
            ("work", "zai", "/home/.config/claude-zai/work"),
            (
                "personal",
                "deepseek",
                "/home/.config/claude-deepseek/personal",
            ),
            ("work", "deepseek", "/home/.config/claude-deepseek/work"),
        ] {
            assert_eq!(
                find(&rows, "claude", context, Some(provider)).path,
                PathBuf::from(expected)
            );
        }
    }

    #[test]
    fn both_directory_shapes_and_restricted_contexts_are_enumerated() {
        let rows = entries(Filter::default());
        for (variable, expected) in [
            ("POLYTOKEN_CONFIG_PATH", "/home/.config/polytoken/personal"),
            (
                "POLYTOKEN_DATA_PATH",
                "/home/.local/share/polytoken/personal",
            ),
        ] {
            let row = rows
                .iter()
                .find(|entry| {
                    entry.harness.as_str() == "polytoken"
                        && entry.variable == variable
                        && entry.provider.is_none()
                })
                .unwrap();
            assert_eq!(row.path, PathBuf::from(expected));
        }
        assert_eq!(
            find(&rows, "fixed", "personal", None).path,
            PathBuf::from("/home/.fixed")
        );
        assert!(
            rows.iter()
                .all(|entry| entry.harness.as_str() != "fixed"
                    || entry.context.as_str() == "personal")
        );
        assert!(rows.iter().all(|entry| entry.harness.as_str() != "plain"));
    }

    #[test]
    fn provider_derivation_handles_a_fixed_path() {
        let rows = entries(Filter::default());
        assert_eq!(
            find(&rows, "fixed", "personal", Some("zai")).path,
            PathBuf::from("/home/.fixed-zai")
        );
    }

    #[test]
    fn filters_restrict_context_and_provider() {
        let work = entries(Filter {
            context: Some("work"),
            provider: None,
        });
        assert!(work.iter().all(|entry| entry.context.as_str() == "work"));
        assert!(work.iter().any(|entry| entry.provider.is_none()));

        let zai = entries(Filter {
            context: None,
            provider: Some("zai"),
        });
        assert!(
            zai.iter()
                .all(|entry| entry.provider.as_ref().map(ProviderName::as_str) == Some("zai"))
        );
        assert!(!zai.is_empty());
    }

    #[test]
    fn unknown_filters_fail_loudly() {
        let error = directories(
            &config(),
            Some(Path::new("/home")),
            Filter {
                context: Some("staging"),
                provider: None,
            },
        )
        .unwrap_err();
        assert!(matches!(error, LayoutError::UnknownContext(_)));

        let error = directories(
            &config(),
            Some(Path::new("/home")),
            Filter {
                context: None,
                provider: Some("ghost"),
            },
        )
        .unwrap_err();
        assert!(matches!(error, LayoutError::UnknownProvider(_)));
    }

    #[test]
    fn existence_is_reported_not_enforced() {
        let home = TempDir::new().unwrap();
        std::fs::create_dir_all(home.path().join(".config/claude/personal")).unwrap();
        let rows = directories(&config(), Some(home.path()), Filter::default()).unwrap();
        assert!(find(&rows, "claude", "personal", None).exists);
        assert!(!find(&rows, "claude", "work", None).exists);
        assert!(!find(&rows, "claude", "personal", Some("zai")).exists);
    }

    #[test]
    fn absolute_templates_do_not_require_home() {
        let mut config = config();
        config.harness.get_mut("claude").unwrap().config_dir = Some(
            crate::config::ConfigDirectory::Single(crate::config::SingleDirectory {
                environment: "CLAUDE_CONFIG_DIR".to_string(),
                path: "/var/lib/claude/{context}".to_string(),
            }),
        );
        config.harness.retain(|name, _| name.as_str() == "claude");
        let rows = directories(&config, None, Filter::default()).unwrap();
        assert_eq!(
            find(&rows, "claude", "personal", None).path,
            PathBuf::from("/var/lib/claude/personal")
        );
    }

    #[test]
    fn a_tilde_template_without_home_fails() {
        let error = directories(&config(), None, Filter::default()).unwrap_err();
        assert!(matches!(error, LayoutError::HomeNotSet));
    }
}
