//! Configuration types and config file loading.
use super::{
    PrompterError, collect_profiles, library_dir, load_config_bundle, resolve_config_path,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A profile definition carrying declared dependencies and its library root.
#[derive(Debug, Clone)]
pub struct ProfileDef {
    /// Declared dependencies: either profile names or `.md` paths.
    pub(crate) deps: Vec<String>,
    /// Absolute path to the library root this profile's `.md` deps resolve against.
    pub(crate) library_root: PathBuf,
}

/// Configuration structure holding merged profile definitions and primary post-prompt.
///
/// Profiles map names to [`ProfileDef`] values, where each profile carries the
/// library root it was loaded against. After a bundle load, profiles from all
/// imported configs are flattened into this one map; `post_prompt` only
/// reflects the *primary* config (imports' `post_prompt` values are ignored).
#[derive(Debug)]
pub struct Config {
    /// Map of profile names to their definitions.
    pub(crate) profiles: HashMap<String, ProfileDef>,
    /// Optional post-prompt text from the primary config.
    pub(crate) post_prompt: Option<String>,
}

impl Config {
    /// Return the unique library roots referenced by any loaded profile,
    /// sorted for stable output.
    #[must_use]
    pub fn library_roots(&self) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = Vec::new();
        for def in self.profiles.values() {
            if !roots.contains(&def.library_root) {
                roots.push(def.library_root.clone());
            }
        }
        roots.sort();
        roots
    }
}

pub(crate) fn load_bundle(
    config_override: Option<&Path>,
) -> Result<(PathBuf, Config), PrompterError> {
    let cfg_path = resolve_config_path(config_override)?;
    let default_library = if config_override.is_some() {
        None
    } else {
        Some(library_dir()?)
    };
    let cfg = load_config_bundle(&cfg_path, default_library.as_deref())?;
    Ok((cfg_path, cfg))
}

/// Raw parse of a single config file: profiles keyed by dotted name, plus
/// the top-level reserved keys (`import`, `library`, `post_prompt`).
///
/// Unlike the merged [`Config`], this represents one file on disk before
/// imports are resolved and before a library root is bound to each profile.
#[derive(Debug)]
pub(crate) struct RawConfigFile {
    /// Map of dotted profile name -> its `depends_on` list.
    pub(crate) profiles: HashMap<String, Vec<String>>,
    /// Optional list of paths to other config files to import.
    pub(crate) imports: Vec<String>,
    /// Optional explicit library-root override (relative to this file's dir,
    /// or absolute, or tilde-prefixed).
    pub(crate) library: Option<String>,
    /// Optional post-prompt (only honored for the primary config).
    pub(crate) post_prompt: Option<String>,
}

/// Parse a single config file's text into a [`RawConfigFile`].
///
/// Uses the `toml` crate. Any top-level table that (directly or transitively)
/// contains a `depends_on` array is treated as a profile; its dotted path
/// becomes the profile name. This preserves the pre-existing behavior where
/// `[python.api]` names a flat profile rather than a nested table.
///
/// # Errors
/// Returns an error if the TOML is syntactically invalid or if a field has
/// the wrong type (e.g. `import` is not an array of strings, `depends_on`
/// contains non-string entries).
pub(crate) fn parse_config_file(input: &str) -> Result<RawConfigFile, PrompterError> {
    let table: toml::Table = toml::from_str(input)?;

    let mut raw = RawConfigFile {
        profiles: HashMap::new(),
        imports: Vec::new(),
        library: None,
        post_prompt: None,
    };

    for (key, value) in &table {
        match key.as_str() {
            "import" => {
                let arr = value.as_array().ok_or_else(|| {
                    PrompterError::ConfigField("`import` must be an array of strings".to_string())
                })?;
                for entry in arr {
                    let s = entry.as_str().ok_or_else(|| {
                        PrompterError::ConfigField("`import` entries must be strings".to_string())
                    })?;
                    raw.imports.push(s.to_string());
                }
            }
            "library" => {
                let s = value.as_str().ok_or_else(|| {
                    PrompterError::ConfigField("`library` must be a string".to_string())
                })?;
                raw.library = Some(s.to_string());
            }
            "post_prompt" => {
                let s = value.as_str().ok_or_else(|| {
                    PrompterError::ConfigField("`post_prompt` must be a string".to_string())
                })?;
                raw.post_prompt = Some(s.to_string());
            }
            _ => collect_profiles(key, value, &mut raw.profiles)?,
        }
    }

    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_config_rejects_non_string_import_entry() {
        // `import` is an array here, but a non-string entry is rejected with a
        // `ConfigField` error naming the offending key.
        let err = parse_config_file("import = [123]\n").unwrap_err();
        assert!(matches!(err, PrompterError::ConfigField(_)), "err={err}");
        assert!(
            err.to_string().contains("`import` entries must be strings"),
            "err={err}"
        );
    }

    #[test]
    fn parse_config_rejects_non_string_library() {
        // The reserved `library` key must be a string, not an integer.
        let err = parse_config_file("library = 123\n").unwrap_err();
        assert!(matches!(err, PrompterError::ConfigField(_)), "err={err}");
        assert!(
            err.to_string().contains("`library` must be a string"),
            "err={err}"
        );
    }

    #[test]
    fn parse_config_rejects_non_string_post_prompt() {
        // The reserved `post_prompt` key must be a string, not a boolean.
        let err = parse_config_file("post_prompt = true\n").unwrap_err();
        assert!(matches!(err, PrompterError::ConfigField(_)), "err={err}");
        assert!(
            err.to_string().contains("`post_prompt` must be a string"),
            "err={err}"
        );
    }
}
