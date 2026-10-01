//! Fail-closed prompter composition and harness-specific prompt injection.
//!
//! Any composition or injection failure aborts the launch; `--no-prompt` is
//! the only way to launch without a composed prompt.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::config::{Config, HarnessConfig, InjectionConfig, ModelFamily};
use crate::launch::expand_path;

const FILE_PLACEHOLDER: &str = "{file}";
const TEXT_PLACEHOLDER: &str = "{text}";

/// Prompt material and process mutations produced for one launch.
#[derive(Debug, Default)]
pub struct PromptInjection {
    /// Rendered prompt text, when composition and injection succeeded.
    pub prompt: Option<String>,
    /// Arguments prepended before model and harness arguments.
    pub arguments: Vec<OsString>,
    /// Environment applied for env-file injection.
    pub environment: BTreeMap<OsString, OsString>,
    /// Prompt cache path used by file-based injection.
    pub cache_path: Option<PathBuf>,
}

/// Failures composing or injecting the launch prompt.
#[derive(Debug, thiserror::Error)]
pub enum PromptError {
    /// A configured prompter path is invalid.
    #[error("prompter path: {0}")]
    Path(String),
    /// The prompter bundle could not be located or loaded.
    #[error("prompter unavailable: {0}")]
    Unavailable(String),
    /// Requested profiles are absent from the prompter bundle.
    #[error("unknown prompter profile(s): {}", profiles.join(", "))]
    UnknownProfiles {
        /// Every requested profile missing from the bundle.
        profiles: Vec<String>,
    },
    /// Prompt rendering failed.
    #[error("prompt rendering failed: {0}")]
    Render(String),
    /// Rendered prompt bytes are not valid UTF-8.
    #[error("rendered prompt is not valid UTF-8: {0}")]
    NonUtf8(#[from] std::string::FromUtf8Error),
    /// Prompt cache directory could not be created.
    #[error("failed to create prompt cache {path}: {source}")]
    CacheDirectory {
        /// Cache directory path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// Prompt file could not be created or written.
    #[error("failed to write prompt file {path}: {source}")]
    CacheFile {
        /// Prompt file path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// `OpenCode` inline configuration could not be serialized.
    #[error("failed to serialize OpenCode inline configuration: {0}")]
    OpenCodeConfig(#[from] serde_json::Error),
}

#[derive(Serialize)]
struct OpenCodeConfig<'a> {
    instructions: [&'a Path; 1],
}

/// Compose configured profiles and build the harness injection mutation.
///
/// # Errors
/// Returns [`PromptError`] when the bundle is unavailable, any requested
/// profile is unknown, rendering fails, or the prompt file cannot be written.
pub fn compose_and_inject(
    profiles: &[String],
    family: &ModelFamily,
    harness: &HarnessConfig,
    config: &Config,
    home: &Path,
    persist_file: bool,
) -> Result<PromptInjection, PromptError> {
    let bundle = expand_path(&config.defaults.prompter_bundle, home)
        .map_err(|error| PromptError::Path(error.to_string()))?;
    let bundle_config = bundle.join("config.toml");
    let available = crate::promptlib::available_profiles(Some(&bundle_config))
        .map_err(|error| PromptError::Unavailable(error.to_string()))?
        .into_iter()
        .collect::<BTreeSet<_>>();

    let unknown: Vec<String> = profiles
        .iter()
        .filter(|profile| !available.contains(*profile))
        .cloned()
        .collect();
    if !unknown.is_empty() {
        return Err(PromptError::UnknownProfiles { profiles: unknown });
    }

    let mut injection = PromptInjection::default();
    if profiles.is_empty() {
        return Ok(injection);
    }

    // Bare framing, deliberately: `render_to_vec` renders with `Framing::Full`,
    // which is right for a human running `prompter run` at a terminal and wrong
    // for a launcher stamping text into a harness system-prompt site. Full
    // framing opens with a handshake the model cannot answer from a system
    // prompt, adds a standing prohibition on touching the directory that the
    // user's first message contradicts, closes by asking for a file the harness
    // already loads, and stamps a date that busts the prompt cache. None of that
    // survives contact with this position, so the choice is made here rather
    // than in the engine, whose defaults stay correct for the CLI.
    let bundle_cfg = crate::promptlib::load_config_bundle(&bundle_config, None)
        .map_err(|error| PromptError::Unavailable(error.to_string()))?;
    let mut rendered = Vec::new();
    crate::promptlib::render_to_writer(
        &bundle_cfg,
        &mut rendered,
        profiles,
        Some(family.as_prompter_family()),
        None,
        None,
        None,
        crate::promptlib::Framing::Bare,
        tftio_lib::JsonOutput::Text,
    )
    .map_err(|error| PromptError::Render(error.to_string()))?;
    let prompt = String::from_utf8(rendered)?;

    match &harness.injection {
        InjectionConfig::ArgText { args } => {
            injection.arguments = args
                .iter()
                .map(|argument| OsString::from(argument.replace(TEXT_PLACEHOLDER, &prompt)))
                .collect();
        }
        InjectionConfig::ArgFile { args } => {
            let path = prepare_prompt_file(config, home, &prompt, persist_file)?;
            let display = path.display().to_string();
            injection.arguments = args
                .iter()
                .map(|argument| OsString::from(argument.replace(FILE_PLACEHOLDER, &display)))
                .collect();
            injection.cache_path = Some(path);
        }
        InjectionConfig::EnvFile { environment } => {
            let path = prepare_prompt_file(config, home, &prompt, persist_file)?;
            injection.environment.insert(
                OsString::from(environment),
                OsString::from(path.as_os_str()),
            );
            injection.cache_path = Some(path);
        }
        InjectionConfig::OpencodeInstructions { environment } => {
            let path = prepare_prompt_file(config, home, &prompt, persist_file)?;
            let inline_config = serde_json::to_string(&OpenCodeConfig {
                instructions: [&path],
            })?;
            injection
                .environment
                .insert(OsString::from(environment), OsString::from(inline_config));
            injection.cache_path = Some(path);
        }
    }
    injection.prompt = Some(prompt);
    Ok(injection)
}

fn prepare_prompt_file(
    config: &Config,
    home: &Path,
    prompt: &str,
    persist: bool,
) -> Result<PathBuf, PromptError> {
    let cache = expand_path(&config.defaults.prompt_cache, home)
        .map_err(|error| PromptError::Path(error.to_string()))?;
    if !persist {
        return Ok(cache.join("dry-run-prompt.md"));
    }

    fs::create_dir_all(&cache).map_err(|source| PromptError::CacheDirectory {
        path: cache.clone(),
        source,
    })?;
    let now = SystemTime::now();
    gc_prompt_cache(
        &cache,
        Duration::from_secs(config.defaults.prompt_cache_ttl_seconds),
        now,
    );
    let timestamp = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = cache.join(format!("{}-{timestamp}.md", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|source| PromptError::CacheFile {
            path: path.clone(),
            source,
        })?;
    file.write_all(prompt.as_bytes())
        .map_err(|source| PromptError::CacheFile {
            path: path.clone(),
            source,
        })?;
    Ok(path)
}

fn gc_prompt_cache(cache: &Path, ttl: Duration, now: SystemTime) {
    let Ok(entries) = fs::read_dir(cache) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age > ttl {
            let _ignored = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use tempfile::TempDir;

    fn write_bundle(home: &Path) {
        let prompter = home.join(".local/prompter");
        fs::create_dir_all(prompter.join("library/families/gpt")).unwrap();
        fs::write(
            prompter.join("config.toml"),
            r#"
library = "library"

[core.base]
depends_on = ["base.md"]
"#,
        )
        .unwrap();
        fs::write(prompter.join("library/base.md"), "BASE\n").unwrap();
        fs::write(prompter.join("library/families/gpt/base.md"), "GPT BASE\n").unwrap();
    }

    fn sample_config() -> Config {
        toml::from_str(
            r#"
[defaults]
domain = "eng"
contexts = ["personal"]
shim_path = "~/unused"
sandbox_wrapper = "~/unused"
prompter_bundle = "~/.local/prompter"
prompt_cache = "~/.cache/clanker/prompts"
prompt_cache_ttl_seconds = 3600

[harness.claude]
bin = "claude"
family = "claude"
default_args = []
[harness.claude.injection]
kind = "arg-text"
args = ["--append-system-prompt", "{text}"]

[harness.codex]
bin = "codex"
family = "gpt"
default_args = []
[harness.codex.injection]
kind = "arg-file"
args = ["-c", "model_instructions_file={file}"]

[harness.gemini]
bin = "gemini"
family = "gemini"
default_args = []
[harness.gemini.injection]
kind = "env-file"
environment = "GEMINI_SYSTEM_MD"

[harness.opencode]
bin = "opencode"
family = "gpt"
default_args = []
[harness.opencode.injection]
kind = "opencode-instructions"
environment = "OPENCODE_CONFIG_CONTENT"

[model]

[domain.eng]
profiles = ["core.base"]
env = {}
"#,
        )
        .unwrap()
    }

    fn harness<'a>(config: &'a Config, name: &str) -> &'a HarnessConfig {
        config.harness.get(name).unwrap()
    }

    #[test]
    fn arg_text_injection_substitutes_prompt_into_arguments() {
        let home = TempDir::new().unwrap();
        write_bundle(home.path());
        let config = sample_config();
        let claude = harness(&config, "claude");

        let injection = compose_and_inject(
            &["core.base".to_string()],
            &claude.family,
            claude,
            &config,
            home.path(),
            false,
        )
        .unwrap();

        assert!(
            injection
                .prompt
                .as_deref()
                .is_some_and(|prompt| prompt.contains("BASE\n"))
        );
        assert!(
            injection
                .arguments
                .iter()
                .any(|argument| argument.to_string_lossy() == "--append-system-prompt")
        );
        assert!(
            injection
                .arguments
                .iter()
                .any(|argument| argument.to_string_lossy().contains("BASE\n"))
        );
        assert!(injection.cache_path.is_none());
    }

    #[test]
    fn arg_file_injection_persists_prompt_and_injects_path() {
        let home = TempDir::new().unwrap();
        write_bundle(home.path());
        let config = sample_config();
        let codex = harness(&config, "codex");

        let injection = compose_and_inject(
            &["core.base".to_string()],
            &codex.family,
            codex,
            &config,
            home.path(),
            true,
        )
        .unwrap();

        let cache_path = injection
            .cache_path
            .expect("arg-file injection writes a file");
        assert!(cache_path.is_file());
        assert!(
            fs::read_to_string(&cache_path)
                .unwrap()
                .contains("GPT BASE\n")
        );
        let rendered = cache_path.display().to_string();
        assert!(
            injection
                .arguments
                .iter()
                .any(|argument| argument.to_string_lossy()
                    == format!("model_instructions_file={rendered}"))
        );
    }

    #[test]
    fn env_file_injection_exports_prompt_path() {
        let home = TempDir::new().unwrap();
        write_bundle(home.path());
        let config = sample_config();
        let gemini = harness(&config, "gemini");

        let injection = compose_and_inject(
            &["core.base".to_string()],
            &gemini.family,
            gemini,
            &config,
            home.path(),
            true,
        )
        .unwrap();

        let cache_path = injection
            .cache_path
            .expect("env-file injection writes a file");
        assert_eq!(
            injection
                .environment
                .get(std::ffi::OsStr::new("GEMINI_SYSTEM_MD")),
            Some(&OsString::from(cache_path.as_os_str()))
        );
        assert!(injection.arguments.is_empty());
    }

    #[test]
    fn opencode_instructions_injection_exports_serialized_config() {
        let home = TempDir::new().unwrap();
        write_bundle(home.path());
        let config = sample_config();
        let opencode = harness(&config, "opencode");

        let injection = compose_and_inject(
            &["core.base".to_string()],
            &opencode.family,
            opencode,
            &config,
            home.path(),
            false,
        )
        .unwrap();

        let cache_path = injection
            .cache_path
            .expect("OpenCode instruction injection writes a file");
        let raw = injection
            .environment
            .get(std::ffi::OsStr::new("OPENCODE_CONFIG_CONTENT"))
            .expect("OpenCode inline configuration is exported");
        let value: serde_json::Value = serde_json::from_str(&raw.to_string_lossy()).unwrap();
        assert_eq!(
            value.pointer("/instructions/0"),
            Some(&serde_json::json!(cache_path))
        );
        assert!(!cache_path.exists());
        assert!(injection.arguments.is_empty());
    }

    #[test]
    fn unknown_profile_fails_closed() {
        let home = TempDir::new().unwrap();
        write_bundle(home.path());
        let config = sample_config();
        let claude = harness(&config, "claude");

        let error = compose_and_inject(
            &["core.base".to_string(), "missing.profile".to_string()],
            &claude.family,
            claude,
            &config,
            home.path(),
            false,
        )
        .unwrap_err();

        assert!(matches!(error, PromptError::UnknownProfiles { .. }));
        assert!(error.to_string().contains("missing.profile"));
    }

    #[test]
    fn empty_profiles_yield_no_prompt() {
        let home = TempDir::new().unwrap();
        write_bundle(home.path());
        let config = sample_config();
        let claude = harness(&config, "claude");

        let injection =
            compose_and_inject(&[], &claude.family, claude, &config, home.path(), true).unwrap();

        assert!(injection.prompt.is_none());
        assert!(injection.arguments.is_empty());
        assert!(injection.cache_path.is_none());
    }

    #[test]
    fn missing_bundle_reports_unavailable() {
        let home = TempDir::new().unwrap();
        // No prompter bundle written: the configured path does not exist.
        let config = sample_config();
        let claude = harness(&config, "claude");

        let error = compose_and_inject(
            &["core.base".to_string()],
            &claude.family,
            claude,
            &config,
            home.path(),
            false,
        )
        .unwrap_err();

        assert!(matches!(error, PromptError::Unavailable(_)));
    }

    #[test]
    fn prompt_cache_gc_removes_only_expired_files() {
        let temp = TempDir::new().unwrap();
        let expired = temp.path().join("expired.md");
        let current = temp.path().join("current.md");
        fs::write(&expired, "old").unwrap();
        std::thread::sleep(Duration::from_millis(5));
        fs::write(&current, "new").unwrap();
        let now = SystemTime::now();
        let expired_modified = fs::metadata(&expired).unwrap().modified().unwrap();
        let current_modified = fs::metadata(&current).unwrap().modified().unwrap();
        let threshold = now
            .duration_since(expired_modified)
            .unwrap()
            .checked_sub(Duration::from_millis(1))
            .unwrap();
        assert!(now.duration_since(current_modified).unwrap() < threshold);

        gc_prompt_cache(temp.path(), threshold, now);

        assert!(!expired.exists());
        assert!(current.exists());
    }

    #[test]
    fn gc_ignores_a_missing_cache_directory() {
        let temp = TempDir::new().unwrap();
        let absent = temp.path().join("no-such-cache");
        gc_prompt_cache(&absent, Duration::from_secs(0), SystemTime::now());
        assert!(!absent.exists());
    }

    #[test]
    fn gc_keeps_files_modified_after_the_reference_instant() {
        let temp = TempDir::new().unwrap();
        let kept = temp.path().join("future.md");
        fs::write(&kept, "keep").unwrap();
        // A reference instant before the file's modification time makes the age
        // computation fail, so the entry is skipped rather than deleted.
        gc_prompt_cache(temp.path(), Duration::from_secs(0), UNIX_EPOCH);
        assert!(kept.exists());
    }
}
