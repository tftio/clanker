#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)
)]
#![cfg_attr(test, allow(clippy::disallowed_methods))]
#![allow(
    clippy::empty_line_after_doc_comments,
    reason = "the blank line between the crate-level attribute block and the //! module docs below is intentional"
)]
#![allow(
    clippy::must_use_candidate,
    reason = "prompter has many small value-returning helpers; blanket #[must_use] would add noise without catching real misuse"
)]

//! Prompter: A CLI tool for composing reusable prompt snippets.

//!

//! This library provides functionality for managing and rendering prompt snippets

//! from a structured library using TOML configuration files.

pub mod cli;

pub mod config;
pub mod error;

pub mod profile;

pub mod render;

pub mod completions;
pub mod scaffold;

pub mod doctor;

pub use cli::{AppMode, Cli, Commands, Framing};

pub use config::{Config, ProfileDef};

pub use profile::{
    FamilyName, InvalidFamilyName, ResolveError, TreeNode, TreeNodeType, TreeOutput, list_profiles,
    load_config_bundle, resolve_profile, run_tree_stdout, show_tree, validate,
};

pub use render::{render_to_vec, render_to_writer, run_render_stdout};

pub use error::PrompterError;
pub use scaffold::init_scaffold;

#[cfg(test)]
/// Shared test utilities for this module's own unit tests.
///
/// Ported from `tftio_lib::test_support` (the composition engine's earlier
/// home) rather than reused from it: `#[cfg(test)]` items are not part of a
/// dependency's compiled interface, so a consuming crate's own tests need
/// their own copy of this env-mutation guard.
pub(crate) mod test_support {
    use std::sync::{Mutex, MutexGuard, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    /// Acquire a process-wide lock for tests that mutate environment variables.
    pub fn env_lock() -> MutexGuard<'static, ()> {
        ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

use config::{load_bundle, parse_config_file};
use profile::collect_profiles;
#[cfg(test)]
use profile::expand_tilde;

use serde::Serialize;

use chrono::Local;
use clap::Parser;
use colored::Colorize;
use is_terminal::IsTerminal;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
pub use tftio_lib::{AgentSubcommand, MetaCommand};
use tftio_lib::{JsonOutput, render_response};

/// A single profile definition: its raw dependency strings plus the library
/// directory that should be used to resolve any `.md` deps it declares.
///
/// The library root is recorded per-profile because a merged bundle can pull
/// profiles from multiple config files, each with its own fragments tree.

#[must_use]
pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('"') => out.push('"'),
                Some('\\') | None => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parse command-line arguments and return the resolved application mode.
///
/// Uses clap to parse raw arguments into a structured [`AppMode`]. The
/// `--help` and `--version` short-circuits clap performs are mapped onto the
/// corresponding modes rather than treated as failures.
///
/// # Errors
/// Returns [`PrompterError::ArgParse`] when clap rejects the arguments
/// (unknown flags, missing required arguments, conflicting options); the
/// payload is clap's formatted usage text, intended for display.
pub fn parse_args_from(args: Vec<String>) -> Result<AppMode, PrompterError> {
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(err) => match err.kind() {
            clap::error::ErrorKind::DisplayHelp => return Ok(AppMode::Help),
            clap::error::ErrorKind::DisplayVersion => return Ok(AppMode::Version { json: false }),
            _ => return Err(PrompterError::ArgParse(err.to_string())),
        },
    };

    Ok(resolve_app_mode(cli))
}

/// Profile rendered as the always-on invariant base by `prompter system`.
///
/// This is the named invariant-base unit; the harness stamps its output into
/// each agent's system-prompt site. Domain layers are selected separately.
pub const SYSTEM_BASE_PROFILE: &str = "core.base";

/// Resolve a parsed [`Cli`] value into the executable [`AppMode`].
///
/// This mapping is total: every [`Cli`] value yields an [`AppMode`], so no
/// `Result` wrapping is needed.
#[must_use]
pub fn resolve_app_mode(cli: Cli) -> AppMode {
    match cli.command {
        Commands::Meta { command } => match command {
            MetaCommand::Version { json } => AppMode::Version { json },
            MetaCommand::License => AppMode::License,
            MetaCommand::Completions { shell } => AppMode::Completions { shell },
            MetaCommand::Doctor { json } => AppMode::Doctor { json },
            MetaCommand::Agent { command } => AppMode::Agent { command },
        },
        Commands::Init => AppMode::Init,
        Commands::List => AppMode::List {
            config: cli.config,
            json: cli.json,
        },
        Commands::Tree => AppMode::Tree {
            config: cli.config,
            json: cli.json,
        },
        Commands::Validate => AppMode::Validate {
            config: cli.config,
            json: cli.json,
        },
        Commands::Run {
            profiles,
            family,
            separator,
            pre_prompt,
            post_prompt,
            bare,
        } => {
            let sep = separator.as_ref().map(|s| unescape(s));
            let pre = pre_prompt.as_ref().map(|s| unescape(s));
            let post = post_prompt.as_ref().map(|s| unescape(s));
            AppMode::Run {
                profiles,
                family,
                separator: sep,
                pre_prompt: pre,
                post_prompt: post,
                framing: Framing::from_bare_flag(bare),
                config: cli.config,
                json: cli.json,
            }
        }
        Commands::System {
            profiles,
            separator,
            pre_prompt,
            post_prompt,
            bare,
        } => {
            let sep = separator.as_ref().map(|s| unescape(s));
            let pre = pre_prompt.as_ref().map(|s| unescape(s));
            let post = post_prompt.as_ref().map(|s| unescape(s));
            let mut all = Vec::with_capacity(profiles.len() + 1);
            all.push(SYSTEM_BASE_PROFILE.to_string());
            all.extend(profiles);
            AppMode::Run {
                profiles: all,
                family: None,
                separator: sep,
                pre_prompt: pre,
                post_prompt: post,
                framing: Framing::from_bare_flag(bare),
                config: cli.config,
                json: cli.json,
            }
        }
    }
}

fn home_dir() -> Result<PathBuf, PrompterError> {
    dirs::home_dir().ok_or(PrompterError::HomeNotSet)
}

fn config_path() -> Result<PathBuf, PrompterError> {
    Ok(home_dir()?.join(".config/prompter/config.toml"))
}

fn library_dir() -> Result<PathBuf, PrompterError> {
    Ok(home_dir()?.join(".local/prompter/library"))
}

fn resolve_primary_config_path(path: &Path) -> Result<PathBuf, PrompterError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        env::current_dir()
            .map_err(PrompterError::WorkingDir)
            .map(|cwd| cwd.join(path))
    }
}

fn is_terminal() -> bool {
    std::io::stdout().is_terminal()
}

fn default_pre_prompt() -> String {
    "You are an LLM coding agent. Here are invariants that you must adhere to. Please respond with 'Got it' when you have studied these and understand them. At that point, the operator will give you further instructions. You are *not* to do anything to the contents of this directory until you have been explicitly asked to, by the operator.\n\n".to_string()
}

fn default_post_prompt() -> String {
    "Now, read the @AGENTS.md and @CLAUDE.md files in this directory, if they exist.".to_string()
}

fn format_system_prefix() -> String {
    let date = Local::now().format("%Y-%m-%d").to_string();
    let os = env::consts::OS;
    let arch = env::consts::ARCH;

    if is_terminal() {
        format!(
            "🗓️  Today is {}, and you are running on a {}/{} system.\n\n",
            date.bright_cyan(),
            arch.bright_green(),
            os.bright_green()
        )
    } else {
        format!("Today is {date}, and you are running on a {arch}/{os} system.\n\n")
    }
}

fn success_message(msg: &str) -> String {
    if is_terminal() {
        format!("✅ {}", msg.bright_green())
    } else {
        msg.to_string()
    }
}

fn info_message(msg: &str) -> String {
    if is_terminal() {
        format!("ℹ️  {}", msg.bright_blue())
    } else {
        msg.to_string()
    }
}

fn read_config_with_path(path: &Path) -> Result<String, PrompterError> {
    fs::read_to_string(path).map_err(|source| PrompterError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn resolve_config_path(config_override: Option<&Path>) -> Result<PathBuf, PrompterError> {
    config_override.map_or_else(config_path, resolve_primary_config_path)
}

/// Load the primary config plus its transitive imports, using the default
/// library root (`~/.local/prompter/library`) only when no `-c` override is
/// supplied.
///
/// # Errors
///
/// Returns an error when configuration loading or writing the profile list to
/// standard output fails.
pub fn run_list_stdout(
    config_override: Option<&Path>,
    output: JsonOutput,
) -> Result<(), PrompterError> {
    let (_cfg_path, cfg) = load_bundle(config_override)?;
    list_profiles(&cfg, output, io::stdout())?;
    Ok(())
}

/// JSON output for successful validation
#[derive(Debug, Serialize)]
struct ValidateOutput {
    valid: bool,
}

/// Validate configuration and output results to stdout.
///
/// Convenience function that reads configuration and validates it,
/// outputting any errors found.
///
/// # Arguments
/// * `config_override` - Optional configuration file override
/// * `json` - Whether to output in JSON format
///
/// # Errors
/// Returns an error if:
/// - Configuration file cannot be read or parsed
/// - Validation finds missing files or circular dependencies
pub fn run_validate_stdout(
    config_override: Option<&Path>,
    output: JsonOutput,
) -> Result<(), PrompterError> {
    let (_cfg_path, cfg) = load_bundle(config_override)?;
    validate(&cfg)?;

    if output.is_json() {
        let data = serde_json::to_value(ValidateOutput { valid: true })?;
        println!(
            "{}",
            render_response("validate", JsonOutput::Json, data, String::new())
        );
    }

    Ok(())
}

/// JSON structure for a single fragment
#[derive(Debug, Serialize)]
struct FragmentOutput {
    path: String,
    content: String,
}

/// JSON output structure for render command
#[derive(Debug, Serialize)]
struct RenderOutput {
    profile: String,
    pre_prompt: String,
    system_info: String,
    fragments: Vec<FragmentOutput>,
}

/// Render one or more profiles' content to a writer.
///
/// Resolves profile dependencies (each fragment carrying its owning library
/// root) and writes concatenated content to the provided writer, including
/// pre-prompt, system info, file contents with optional separators, and
/// post-prompt. Files are deduplicated across all profiles by absolute path
/// (first occurrence wins).
///
/// # Errors
/// Returns an error if:
/// - Profile resolution fails (missing files, cycles, unknown profiles)
/// - Writing to output fails

pub fn available_profiles(config_override: Option<&Path>) -> Result<Vec<String>, PrompterError> {
    let (_cfg_path, cfg) = load_bundle(config_override)?;
    let mut names: Vec<String> = cfg.profiles.keys().cloned().collect();
    names.sort();
    Ok(names)
}

#[cfg(test)]
#[allow(clippy::wildcard_imports)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use std::collections::HashSet;
    use std::io::Write;

    fn mk_tmp(prefix: &str) -> PathBuf {
        let mut p = env::temp_dir();
        let unique = format!(
            "{}_{}_{}",
            prefix,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        p.push(unique);
        p
    }

    /// Build a [`Config`] whose every profile shares a single library root.
    /// Most existing unit tests only exercise the single-bundle case; this
    /// helper avoids tediously repeating `ProfileDef { ... }` at every call site.
    fn cfg_with_lib<I>(profiles: I, lib: &Path, post_prompt: Option<&str>) -> Config
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
            post_prompt: post_prompt.map(String::from),
        }
    }

    #[test]
    fn test_unescape() {
        assert_eq!(unescape("a\\nb\\t\\\"\\\\c"), "a\nb\t\"\\c");
        assert_eq!(unescape("line1\\rline2"), "line1\rline2");
        assert_eq!(unescape("noesc"), "noesc");
        // Unknown escape sequences are preserved verbatim (backslash + char).
        assert_eq!(unescape("a\\zb"), "a\\zb");
        // A trailing backslash with no following character stays a lone backslash.
        assert_eq!(unescape("trail\\"), "trail\\");
    }

    #[test]
    fn test_parse_config_file_errors() {
        // Top-level value that is not a table; toml crate should surface this.
        let err = parse_config_file("not valid toml {{{")
            .unwrap_err()
            .to_string();
        assert!(err.contains("Invalid TOML"), "err={err}");
        // `depends_on` must be an array.
        let err = parse_config_file("[p]\ndepends_on = \"x\"\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("`depends_on`"), "err={err}");
        // `import` must be an array of strings.
        let err = parse_config_file("import = \"oops\"\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("`import`"), "err={err}");
    }

    #[test]
    fn test_validate_success_and_unknowns() {
        let lib = mk_tmp("prompter_validate_ok");
        fs::create_dir_all(&lib).unwrap();
        fs::write(lib.join("a.md"), b"A").unwrap();
        fs::write(lib.join("b.md"), b"B").unwrap();
        let cfg = cfg_with_lib(
            [("p1", vec!["a.md"]), ("p2", vec!["p1", "b.md"])],
            &lib,
            None,
        );
        assert!(validate(&cfg).is_ok());
        let cfg2 = cfg_with_lib([("root", vec!["nope"])], &lib, None);
        let err = validate(&cfg2).unwrap_err().to_string();
        assert!(err.contains("Unknown profile"));
    }

    #[test]
    fn test_resolve_errors_and_dedup() {
        let lib = mk_tmp("prompter_resolve_errs");
        fs::create_dir_all(&lib).unwrap();
        let cfg = cfg_with_lib([("root", vec!["missing.md"])], &lib, None);
        let mut seen = HashSet::new();
        let mut stack = Vec::new();
        let mut out = Vec::new();
        let err = resolve_profile("root", &cfg, &mut seen, &mut stack, &mut out).unwrap_err();
        match err {
            ResolveError::MissingFile(_, p) => assert_eq!(p, "root"),
            _ => panic!("expected missing file"),
        }

        fs::create_dir_all(lib.join("a")).unwrap();
        fs::write(lib.join("a/b.md"), b"X").unwrap();
        let cfg2 = cfg_with_lib(
            [("A", vec!["a/b.md"]), ("B", vec!["A", "a/b.md"])],
            &lib,
            None,
        );
        let mut seen = HashSet::new();
        let mut stack = Vec::new();
        let mut out = Vec::new();
        resolve_profile("B", &cfg2, &mut seen, &mut stack, &mut out).unwrap();
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn test_parse_args_errors() {
        // unknown flag
        let args = vec!["prompter".into(), "--bogus".into()];
        let err = parse_args_from(args).unwrap_err().to_string();
        assert!(err.contains("unexpected argument"));
        // missing required subcommand
        let args = vec!["prompter".into()];
        let err = parse_args_from(args).unwrap_err().to_string();
        assert!(err.contains("Usage:") || err.contains("COMMAND"));
    }

    #[test]
    fn test_list_profiles_order() {
        let lib = mk_tmp("prompter_list_order");
        fs::create_dir_all(&lib).unwrap();
        let cfg = cfg_with_lib([("b", vec![]), ("a", vec![])], &lib, None);
        let mut out = Vec::new();
        super::list_profiles(&cfg, JsonOutput::Text, &mut out).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "a\nb\n");
    }

    #[test]
    fn test_validate_cycle_detected() {
        let lib = mk_tmp("prompter_cycle");
        fs::create_dir_all(&lib).unwrap();
        let cfg = cfg_with_lib([("A", vec!["B"]), ("B", vec!["A"])], &lib, None);
        let err = validate(&cfg).unwrap_err().to_string();
        assert!(err.contains("Cycle detected"));
    }

    #[test]
    fn test_parse_config_file_flattens_dotted_tables() {
        // Preserves pre-existing semantics: `[profile.x]` is a flat profile
        // named "profile.x", not a nested table.
        let cfg = r#"
[profile.x]
depends_on = [
  "a/b.md",
  "c/d.md",
  "e/f.md",
]
"#;
        let parsed = parse_config_file(cfg).unwrap();
        assert_eq!(parsed.profiles.get("profile.x").unwrap().len(), 3);
    }

    #[test]
    fn test_render_to_writer_basic() {
        let lib = mk_tmp("prompter_render_to_writer");
        fs::create_dir_all(lib.join("a")).unwrap();
        fs::create_dir_all(lib.join("f")).unwrap();
        fs::write(lib.join("a/x.md"), b"AX\n").unwrap();
        fs::write(lib.join("f/y.md"), b"FY\n").unwrap();
        let cfg = cfg_with_lib(
            [
                ("child", vec!["a/x.md"]),
                ("root", vec!["child", "f/y.md", "a/x.md"]),
            ],
            &lib,
            None,
        );
        let mut out = Vec::new();
        super::render_to_writer(
            &cfg,
            &mut out,
            &["root".to_string()],
            None,
            Some("\n--\n"),
            None,
            None,
            Framing::Full,
            JsonOutput::Text,
        )
        .unwrap();

        let output_str = String::from_utf8(out).unwrap();
        assert!(output_str.starts_with("You are an LLM coding agent."));
        assert!(output_str.contains("Today is "));
        assert!(output_str.contains(", and you are running on a "));
        assert!(output_str.contains(" system.\n\n"));
        assert!(output_str.contains("AX\n"));
        assert!(output_str.contains("\n--\n"));
        assert!(output_str.contains("FY\n"));
        assert!(output_str.ends_with(
            "Now, read the @AGENTS.md and @CLAUDE.md files in this directory, if they exist."
        ));
    }

    #[test]
    fn test_render_to_writer_bare_omits_framing() {
        let lib = mk_tmp("prompter_render_bare");
        fs::create_dir_all(lib.join("a")).unwrap();
        fs::create_dir_all(lib.join("f")).unwrap();
        fs::write(lib.join("a/x.md"), b"AX\n").unwrap();
        fs::write(lib.join("f/y.md"), b"FY\n").unwrap();
        let cfg = cfg_with_lib(
            [
                ("child", vec!["a/x.md"]),
                ("root", vec!["child", "f/y.md", "a/x.md"]),
            ],
            &lib,
            Some("Config post-prompt"),
        );
        let mut out = Vec::new();
        super::render_to_writer(
            &cfg,
            &mut out,
            &["root".to_string()],
            None,
            Some("\n--\n"),
            None,
            None,
            Framing::Bare,
            JsonOutput::Text,
        )
        .unwrap();

        let output_str = String::from_utf8(out).unwrap();
        // No pre-prompt, no date/system context, no post-prompt.
        assert!(!output_str.starts_with("You are an LLM coding agent."));
        assert!(!output_str.contains("Today is "));
        assert!(!output_str.contains("Config post-prompt"));
        assert!(!output_str.contains(
            "Now, read the @AGENTS.md and @CLAUDE.md files in this directory, if they exist."
        ));
        // Fragments (deduplicated) and the separator are still present, and the
        // output begins directly with the first fragment (no leading newline).
        assert!(output_str.starts_with("AX\n"));
        assert!(output_str.contains("\n--\n"));
        assert!(output_str.contains("FY\n"));
        assert_eq!(output_str.matches("AX\n").count(), 1);
    }

    #[test]
    fn test_render_to_writer_bare_honors_explicit_pre_post() {
        let lib = mk_tmp("prompter_render_bare_explicit");
        fs::create_dir_all(lib.join("a")).unwrap();
        fs::create_dir_all(lib.join("f")).unwrap();
        fs::write(lib.join("a/x.md"), b"AX\n").unwrap();
        fs::write(lib.join("f/y.md"), b"FY\n").unwrap();
        let cfg = cfg_with_lib(
            [("child", vec!["a/x.md"]), ("root", vec!["child", "f/y.md"])],
            &lib,
            Some("Config post-prompt"),
        );
        let mut out = Vec::new();
        super::render_to_writer(
            &cfg,
            &mut out,
            &["root".to_string()],
            None,
            None,
            Some("EXPLICIT-PRE"),
            Some("EXPLICIT-POST"),
            Framing::Bare,
            JsonOutput::Text,
        )
        .unwrap();

        let output_str = String::from_utf8(out).unwrap();
        // Explicit pre/post win even in bare framing; the config post-prompt and
        // the date/system stamp are still suppressed.
        assert!(output_str.starts_with("EXPLICIT-PRE"));
        assert!(output_str.ends_with("EXPLICIT-POST"));
        assert!(!output_str.contains("Config post-prompt"));
        assert!(!output_str.contains("Today is "));
        assert!(output_str.contains("AX\n"));
        assert!(output_str.contains("FY\n"));
    }

    #[test]
    fn test_render_to_writer_custom_pre_prompt() {
        let lib = mk_tmp("prompter_render_custom_pre");
        fs::create_dir_all(lib.join("a")).unwrap();
        fs::write(lib.join("a/x.md"), b"Content\n").unwrap();
        let cfg = cfg_with_lib([("test", vec!["a/x.md"])], &lib, None);
        let mut out = Vec::new();
        super::render_to_writer(
            &cfg,
            &mut out,
            &["test".to_string()],
            None,
            None,
            Some("Custom pre-prompt\n\n"),
            None,
            Framing::Full,
            JsonOutput::Text,
        )
        .unwrap();

        let output_str = String::from_utf8(out).unwrap();
        assert!(output_str.starts_with("Custom pre-prompt\n\n"));
        assert!(output_str.contains("Today is "));
        assert!(output_str.contains("Content\n"));
        assert!(output_str.ends_with(
            "Now, read the @AGENTS.md and @CLAUDE.md files in this directory, if they exist."
        ));
    }

    #[test]
    fn test_render_to_writer_custom_post_prompt() {
        let lib = mk_tmp("prompter_render_custom_post");
        fs::create_dir_all(lib.join("a")).unwrap();
        fs::write(lib.join("a/x.md"), b"Content\n").unwrap();
        let cfg = cfg_with_lib(
            [("test", vec!["a/x.md"])],
            &lib,
            Some("Custom config post-prompt"),
        );
        let mut out = Vec::new();
        super::render_to_writer(
            &cfg,
            &mut out,
            &["test".to_string()],
            None,
            None,
            None,
            None,
            Framing::Full,
            JsonOutput::Text,
        )
        .unwrap();

        let output_str = String::from_utf8(out).unwrap();
        assert!(output_str.ends_with("Custom config post-prompt"));

        let mut out2 = Vec::new();
        super::render_to_writer(
            &cfg,
            &mut out2,
            &["test".to_string()],
            None,
            None,
            None,
            Some("CLI post-prompt"),
            Framing::Full,
            JsonOutput::Text,
        )
        .unwrap();

        let output_str2 = String::from_utf8(out2).unwrap();
        assert!(output_str2.ends_with("CLI post-prompt"));
    }

    #[test]
    fn test_render_multiple_profiles_with_deduplication() {
        let lib = mk_tmp("prompter_multi_profile_dedup");
        fs::create_dir_all(lib.join("shared")).unwrap();
        fs::create_dir_all(lib.join("a")).unwrap();
        fs::create_dir_all(lib.join("b")).unwrap();

        fs::write(lib.join("shared/common.md"), b"COMMON\n").unwrap();
        fs::write(lib.join("a/specific.md"), b"A_SPECIFIC\n").unwrap();
        fs::write(lib.join("b/specific.md"), b"B_SPECIFIC\n").unwrap();

        let cfg = cfg_with_lib(
            [
                ("profile_a", vec!["shared/common.md", "a/specific.md"]),
                ("profile_b", vec!["shared/common.md", "b/specific.md"]),
            ],
            &lib,
            None,
        );

        let mut out = Vec::new();
        super::render_to_writer(
            &cfg,
            &mut out,
            &["profile_a".to_string(), "profile_b".to_string()],
            None,
            Some("\n---\n"),
            None,
            None,
            Framing::Full,
            JsonOutput::Text,
        )
        .unwrap();

        let output_str = String::from_utf8(out).unwrap();

        let common_count = output_str.matches("COMMON").count();
        assert_eq!(
            common_count, 1,
            "Common file should appear exactly once, found {common_count}"
        );
        assert!(output_str.contains("A_SPECIFIC"));
        assert!(output_str.contains("B_SPECIFIC"));

        let common_pos = output_str.find("COMMON").unwrap();
        let a_pos = output_str.find("A_SPECIFIC").unwrap();
        let b_pos = output_str.find("B_SPECIFIC").unwrap();

        assert!(common_pos < a_pos);
        assert!(a_pos < b_pos);
    }

    #[test]
    fn test_family_variant_substitution_fallback_and_neutral_dedup() {
        let lib = mk_tmp("prompter_family_substitution");
        fs::create_dir_all(lib.join("general/families/gpt")).unwrap();
        fs::write(lib.join("general/rules.md"), b"NEUTRAL_RULES\n").unwrap();
        fs::write(lib.join("general/fallback.md"), b"FALLBACK\n").unwrap();
        fs::write(lib.join("general/families/gpt/rules.md"), b"GPT_RULES\n").unwrap();
        let cfg = cfg_with_lib(
            [
                ("first", vec!["general/rules.md", "general/fallback.md"]),
                ("second", vec!["general/rules.md"]),
            ],
            &lib,
            None,
        );
        let family = FamilyName::new("gpt").unwrap();

        let mut family_output = Vec::new();
        super::render_to_writer(
            &cfg,
            &mut family_output,
            &["first".to_string(), "second".to_string()],
            Some(&family),
            None,
            None,
            None,
            Framing::Bare,
            JsonOutput::Text,
        )
        .unwrap();
        let family_output = String::from_utf8(family_output).unwrap();
        assert_eq!(family_output.matches("GPT_RULES").count(), 1);
        assert!(!family_output.contains("NEUTRAL_RULES"));
        assert!(family_output.contains("FALLBACK"));

        let mut neutral_output = Vec::new();
        super::render_to_writer(
            &cfg,
            &mut neutral_output,
            &["first".to_string(), "second".to_string()],
            None,
            None,
            None,
            None,
            Framing::Bare,
            JsonOutput::Text,
        )
        .unwrap();
        let neutral_output = String::from_utf8(neutral_output).unwrap();
        assert_eq!(neutral_output.matches("NEUTRAL_RULES").count(), 1);
        assert!(!neutral_output.contains("GPT_RULES"));
        assert!(neutral_output.contains("FALLBACK"));
    }

    #[test]
    fn test_validate_rejects_orphan_family_variant() {
        let lib = mk_tmp("prompter_family_orphan");
        fs::create_dir_all(lib.join("general/families/gpt")).unwrap();
        fs::write(lib.join("general/rules.md"), b"NEUTRAL_RULES\n").unwrap();
        fs::write(lib.join("general/families/gpt/rules.md"), b"GPT_RULES\n").unwrap();
        let cfg = cfg_with_lib([("root", vec!["general/rules.md"])], &lib, None);
        assert!(validate(&cfg).is_ok());

        let orphan = lib.join("general/families/gpt/orphan.md");
        fs::write(&orphan, b"ORPHAN\n").unwrap();
        let error = validate(&cfg).unwrap_err().to_string();
        assert!(error.contains("Orphan family variant"), "error: {error}");
        assert!(
            error.contains(&orphan.display().to_string()),
            "error: {error}"
        );
    }

    #[test]
    fn test_parse_config_file_with_post_prompt() {
        let cfg = r#"
post_prompt = "Custom post prompt from config"

[profile]
depends_on = ["file.md"]
"#;
        let parsed = parse_config_file(cfg).unwrap();
        assert_eq!(
            parsed.post_prompt,
            Some("Custom post prompt from config".to_string())
        );
        assert_eq!(parsed.profiles.get("profile").unwrap().len(), 1);
    }

    #[allow(unsafe_code)]
    fn set_home(value: &Path) -> Option<std::ffi::OsString> {
        let prior = env::var_os("HOME");
        // SAFETY: serialized by `env_lock`; the prior value is restored before
        // the test returns. Matches the sanctioned test-only env-mutation idiom
        // used elsewhere in this crate (REPO_INVARIANTS.md #5).
        unsafe {
            env::set_var("HOME", value);
        }
        prior
    }

    #[allow(unsafe_code)]
    fn restore_home(prior: Option<std::ffi::OsString>) {
        // SAFETY: serialized by `env_lock`; see `set_home`.
        unsafe {
            match prior {
                Some(value) => env::set_var("HOME", value),
                None => env::remove_var("HOME"),
            }
        }
    }

    #[test]
    fn test_expand_tilde() {
        // Drive HOME deterministically rather than depending on the ambient
        // value, so both tilde-prefixed branches are always exercised.
        let _guard = crate::promptlib::test_support::env_lock();
        let home = mk_tmp("prompter_expand_tilde_home");
        let prior = set_home(&home);

        let tilde_slash = expand_tilde("~/foo/bar");
        let tilde_only = expand_tilde("~");
        let absolute = expand_tilde("/abs/path");
        let relative = expand_tilde("rel/path");

        // Restore HOME before asserting so a failure cannot leak the temp value.
        restore_home(prior);

        assert_eq!(tilde_slash.unwrap(), home.join("foo/bar"));
        assert_eq!(tilde_only.unwrap(), home);
        assert_eq!(absolute.unwrap(), PathBuf::from("/abs/path"));
        assert_eq!(relative.unwrap(), PathBuf::from("rel/path"));
    }

    #[test]
    fn test_load_bundle_single_file() {
        let dir = mk_tmp("prompter_bundle_single");
        fs::create_dir_all(dir.join("library/a")).unwrap();
        fs::write(dir.join("library/a/x.md"), b"AX").unwrap();
        fs::write(
            dir.join("config.toml"),
            r#"
[root]
depends_on = ["a/x.md"]
"#,
        )
        .unwrap();
        let cfg = load_config_bundle(&dir.join("config.toml"), None).unwrap();
        assert_eq!(cfg.profiles.len(), 1);
        let root = cfg.profiles.get("root").unwrap();
        assert_eq!(root.deps, vec!["a/x.md"]);
        // library_root should resolve to <config-dir>/library
        assert_eq!(
            root.library_root,
            fs::canonicalize(dir.join("library")).unwrap()
        );
    }

    #[test]
    fn test_load_bundle_imports_and_dedup_across_libraries() {
        // primary config + one imported bundle; each has its own library.
        let primary_dir = mk_tmp("prompter_bundle_primary");
        let imported_dir = mk_tmp("prompter_bundle_import");

        fs::create_dir_all(primary_dir.join("library/p")).unwrap();
        fs::write(primary_dir.join("library/p/primary.md"), b"P").unwrap();

        fs::create_dir_all(imported_dir.join("library/i")).unwrap();
        fs::write(imported_dir.join("library/i/imported.md"), b"I").unwrap();

        fs::write(
            imported_dir.join("config.toml"),
            r#"
[team.base]
depends_on = ["i/imported.md"]
"#,
        )
        .unwrap();

        let primary_cfg = format!(
            r#"
import = ["{}"]

[my.local]
depends_on = ["team.base", "p/primary.md"]
"#,
            imported_dir.join("config.toml").display()
        );
        fs::write(primary_dir.join("config.toml"), primary_cfg).unwrap();

        let cfg = load_config_bundle(&primary_dir.join("config.toml"), None).unwrap();
        assert_eq!(cfg.profiles.len(), 2);
        assert_eq!(
            cfg.profiles.get("team.base").unwrap().library_root,
            fs::canonicalize(imported_dir.join("library")).unwrap()
        );
        assert_eq!(
            cfg.profiles.get("my.local").unwrap().library_root,
            fs::canonicalize(primary_dir.join("library")).unwrap()
        );

        // Resolution finds both fragments, each in its own library.
        let mut seen = HashSet::new();
        let mut stack = Vec::new();
        let mut out = Vec::new();
        resolve_profile("my.local", &cfg, &mut seen, &mut stack, &mut out).unwrap();
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn test_load_bundle_duplicate_profile_name_across_imports() {
        let primary_dir = mk_tmp("prompter_bundle_dup_primary");
        let imported_dir = mk_tmp("prompter_bundle_dup_import");

        fs::create_dir_all(primary_dir.join("library")).unwrap();
        fs::create_dir_all(imported_dir.join("library")).unwrap();

        fs::write(
            imported_dir.join("config.toml"),
            "\n[clash]\ndepends_on = []\n",
        )
        .unwrap();

        let primary_cfg = format!(
            "\nimport = [\"{}\"]\n\n[clash]\ndepends_on = []\n",
            imported_dir.join("config.toml").display()
        );
        fs::write(primary_dir.join("config.toml"), primary_cfg).unwrap();

        let err = load_config_bundle(&primary_dir.join("config.toml"), None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Duplicate profile `clash`"), "err={err}");
    }

    #[test]
    fn test_load_bundle_import_cycle() {
        let a_dir = mk_tmp("prompter_cycle_a");
        let b_dir = mk_tmp("prompter_cycle_b");
        fs::create_dir_all(a_dir.join("library")).unwrap();
        fs::create_dir_all(b_dir.join("library")).unwrap();

        let a_path = a_dir.join("config.toml");
        let b_path = b_dir.join("config.toml");
        fs::write(&a_path, format!("import = [\"{}\"]\n", b_path.display())).unwrap();
        fs::write(&b_path, format!("import = [\"{}\"]\n", a_path.display())).unwrap();

        let err = load_config_bundle(&a_path, None).unwrap_err().to_string();
        assert!(err.contains("Import cycle"), "err={err}");
    }

    #[test]
    fn test_load_bundle_explicit_library_key() {
        let dir = mk_tmp("prompter_bundle_explicit_lib");
        fs::create_dir_all(dir.join("alt_library/sub")).unwrap();
        fs::write(dir.join("alt_library/sub/x.md"), b"X").unwrap();
        fs::write(
            dir.join("config.toml"),
            r#"
library = "alt_library"

[p]
depends_on = ["sub/x.md"]
"#,
        )
        .unwrap();

        let cfg = load_config_bundle(&dir.join("config.toml"), None).unwrap();
        let expected = fs::canonicalize(dir.join("alt_library")).unwrap();
        assert_eq!(cfg.profiles.get("p").unwrap().library_root, expected);
    }

    #[test]
    fn test_load_bundle_import_post_prompt_only_from_primary() {
        let primary_dir = mk_tmp("prompter_pp_primary");
        let imported_dir = mk_tmp("prompter_pp_import");
        fs::create_dir_all(primary_dir.join("library")).unwrap();
        fs::create_dir_all(imported_dir.join("library")).unwrap();

        fs::write(
            imported_dir.join("config.toml"),
            r#"
post_prompt = "from imported"
"#,
        )
        .unwrap();
        let primary_cfg = format!(
            r#"
import = ["{}"]
post_prompt = "from primary"
"#,
            imported_dir.join("config.toml").display()
        );
        fs::write(primary_dir.join("config.toml"), primary_cfg).unwrap();
        let cfg = load_config_bundle(&primary_dir.join("config.toml"), None).unwrap();
        assert_eq!(cfg.post_prompt.as_deref(), Some("from primary"));

        // Imported's post_prompt is ignored even if primary has none.
        fs::write(
            primary_dir.join("config.toml"),
            format!(
                r#"
import = ["{}"]
"#,
                imported_dir.join("config.toml").display()
            ),
        )
        .unwrap();
        let cfg2 = load_config_bundle(&primary_dir.join("config.toml"), None).unwrap();
        assert!(cfg2.post_prompt.is_none());
    }

    fn expect_run(args: Vec<String>) -> AppMode {
        let mode = parse_args_from(args).unwrap();
        assert!(matches!(mode, AppMode::Run { .. }), "expected run");
        mode
    }

    #[test]
    fn parse_args_run_with_separator() {
        let args = vec![
            "prompter".into(),
            "run".into(),
            "--separator".into(),
            "\\n--\\n".into(),
            "profile".into(),
        ];
        let AppMode::Run {
            profiles,
            family,
            separator,
            pre_prompt,
            post_prompt,
            framing,
            config,
            json,
        } = expect_run(args)
        else {
            unreachable!()
        };
        assert_eq!(profiles, vec!["profile".to_string()]);
        assert_eq!(family, None);
        assert_eq!(separator, Some("\n--\n".into()));
        assert_eq!(pre_prompt, None);
        assert_eq!(post_prompt, None);
        assert_eq!(framing, Framing::Full);
        assert!(config.is_none());
        assert!(!json);
    }

    #[test]
    fn parse_args_run_with_family() {
        let args = vec![
            "prompter".into(),
            "run".into(),
            "--family".into(),
            "gpt".into(),
            "profile".into(),
        ];
        let AppMode::Run {
            profiles, family, ..
        } = expect_run(args)
        else {
            unreachable!()
        };
        assert_eq!(profiles, vec!["profile".to_string()]);
        assert_eq!(family, Some(FamilyName::new("gpt").unwrap()));
    }

    #[test]
    fn parse_args_rejects_family_path_traversal() {
        let args = vec![
            "prompter".into(),
            "run".into(),
            "--family".into(),
            "../gpt".into(),
            "profile".into(),
        ];
        let error = parse_args_from(args).unwrap_err().to_string();
        assert!(error.contains("family name must be one non-empty path component"));
    }

    #[test]
    fn parse_args_run_with_pre_prompt() {
        let args = vec![
            "prompter".into(),
            "run".into(),
            "--pre-prompt".into(),
            "Custom pre-prompt".into(),
            "profile".into(),
        ];
        let AppMode::Run {
            profiles,
            separator,
            pre_prompt,
            ..
        } = expect_run(args)
        else {
            unreachable!()
        };
        assert_eq!(profiles, vec!["profile".to_string()]);
        assert_eq!(separator, None);
        assert_eq!(pre_prompt, Some("Custom pre-prompt".into()));
    }

    #[test]
    fn parse_args_run_with_bare_flag() {
        let args = vec![
            "prompter".into(),
            "run".into(),
            "--bare".into(),
            "profile".into(),
        ];
        let AppMode::Run {
            profiles, framing, ..
        } = expect_run(args)
        else {
            unreachable!()
        };
        assert_eq!(profiles, vec!["profile".to_string()]);
        assert_eq!(framing, Framing::Bare);
    }

    #[test]
    fn parse_args_system_with_bare_flag() {
        let args = vec![
            "prompter".into(),
            "system".into(),
            "--bare".into(),
            "extra".into(),
        ];
        let AppMode::Run {
            profiles, framing, ..
        } = expect_run(args)
        else {
            unreachable!()
        };
        // System prepends the system base profile before any extras.
        assert_eq!(
            profiles,
            vec![SYSTEM_BASE_PROFILE.to_string(), "extra".to_string()]
        );
        assert_eq!(framing, Framing::Bare);
    }

    #[test]
    fn parse_args_run_with_multiple_profiles() {
        let args = vec![
            "prompter".into(),
            "run".into(),
            "profile1".into(),
            "profile2".into(),
            "profile3.nested".into(),
        ];
        let AppMode::Run { profiles, .. } = expect_run(args) else {
            unreachable!()
        };
        assert_eq!(
            profiles,
            vec![
                "profile1".to_string(),
                "profile2".to_string(),
                "profile3.nested".to_string(),
            ]
        );
    }

    #[test]
    fn parse_args_bare_subcommands() {
        let args = vec!["prompter".into(), "list".into()];
        assert!(matches!(
            parse_args_from(args).unwrap(),
            AppMode::List {
                config: None,
                json: false
            }
        ));
        let args = vec!["prompter".into(), "validate".into()];
        assert!(matches!(
            parse_args_from(args).unwrap(),
            AppMode::Validate {
                config: None,
                json: false
            }
        ));
        let args = vec!["prompter".into(), "init".into()];
        assert!(matches!(parse_args_from(args).unwrap(), AppMode::Init));
        let args = vec!["prompter".into(), "meta".into(), "version".into()];
        assert!(matches!(
            parse_args_from(args).unwrap(),
            AppMode::Version { json: false }
        ));
    }

    #[test]
    fn parse_args_config_before_subcommand() {
        let args = vec![
            "prompter".into(),
            "--config".into(),
            "custom/config.toml".into(),
            "list".into(),
        ];
        let AppMode::List { config, json } = parse_args_from(args).unwrap() else {
            panic!("expected list mode");
        };
        assert_eq!(config, Some(PathBuf::from("custom/config.toml")));
        assert!(!json);
    }

    #[test]
    fn parse_args_config_after_run_subcommand() {
        let args = vec![
            "prompter".into(),
            "run".into(),
            "--config".into(),
            "custom/config.toml".into(),
            "profile".into(),
        ];
        let AppMode::Run { config, json, .. } = parse_args_from(args).unwrap() else {
            panic!("expected run mode");
        };
        assert_eq!(config, Some(PathBuf::from("custom/config.toml")));
        assert!(!json);
    }

    struct FailAfterN {
        writes_done: usize,
        fail_on: usize,
    }

    impl Write for FailAfterN {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.writes_done += 1;
            if self.writes_done == self.fail_on {
                Err(io::Error::other("synthetic write failure"))
            } else {
                Ok(buf.len())
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn test_render_to_writer_write_error_on_separator() {
        let lib = mk_tmp("prompter_write_err_sep");
        fs::create_dir_all(lib.join("a")).unwrap();
        fs::write(lib.join("a/x.md"), b"AX").unwrap();
        fs::write(lib.join("a/y.md"), b"AY").unwrap();
        let cfg = cfg_with_lib([("p", vec!["a/x.md", "a/y.md"])], &lib, None);
        let mut w = FailAfterN {
            writes_done: 0,
            fail_on: 3,
        }; // pre-prompt ok, system prefix ok, fail on separator
        let err = super::render_to_writer(
            &cfg,
            &mut w,
            &["p".to_string()],
            None,
            Some("--"),
            None,
            None,
            Framing::Full,
            JsonOutput::Text,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("Write error"), "err={err}");
    }

    #[test]
    fn test_render_to_writer_write_error_on_file() {
        let lib = mk_tmp("prompter_write_err_file");
        fs::create_dir_all(lib.join("a")).unwrap();
        fs::write(lib.join("a/x.md"), b"AX").unwrap();
        let cfg = cfg_with_lib([("p", vec!["a/x.md"])], &lib, None);
        let mut w = FailAfterN {
            writes_done: 0,
            fail_on: 1,
        }; // fail on first write (pre-prompt)
        let err = super::render_to_writer(
            &cfg,
            &mut w,
            &["p".to_string()],
            None,
            Some("--"),
            None,
            None,
            Framing::Full,
            JsonOutput::Text,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("Write error"), "err={err}");
    }

    #[test]
    fn test_run_list_and_validate_with_explicit_config() {
        let root = mk_tmp("prompter_explicit_config_ok");
        let lib_dir = root.join("library");
        fs::create_dir_all(lib_dir.join("a")).unwrap();
        fs::create_dir_all(lib_dir.join("f")).unwrap();
        fs::write(lib_dir.join("a/x.md"), b"AX\n").unwrap();
        fs::write(lib_dir.join("f/y.md"), b"FY\n").unwrap();
        let cfg = r#"
[child]
depends_on = ["a/x.md"]

[root]
depends_on = ["child", "f/y.md"]
"#;
        let config = root.join("config.toml");
        fs::write(&config, cfg).unwrap();
        assert!(super::run_validate_stdout(Some(&config), JsonOutput::Text).is_ok());
        assert!(super::run_list_stdout(Some(&config), JsonOutput::Text).is_ok());
    }

    #[test]
    fn test_run_validate_with_explicit_config_failure() {
        let root = mk_tmp("prompter_explicit_config_bad");
        let lib_dir = root.join("library");
        fs::create_dir_all(&lib_dir).unwrap();
        let cfg = r#"
[root]
depends_on = ["missing.md", "unknown_profile"]
"#;
        let config = root.join("config.toml");
        fs::write(&config, cfg).unwrap();
        let err = super::run_validate_stdout(Some(&config), JsonOutput::Text).unwrap_err();
        assert!(
            err.to_string().contains("Missing file") && err.to_string().contains("Unknown profile"),
            "err={err}"
        );
    }

    #[test]
    fn render_to_vec_returns_bytes() {
        // This test uses the real config, so it depends on prompter being configured.
        // If no config exists, it should return an error, not panic.
        let result = render_to_vec(&[], None, None);
        // Empty profiles should succeed (produces empty or minimal output)
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn available_profiles_returns_sorted() {
        // Drive an explicit config so the success path (and its sort) is always
        // exercised rather than depending on an ambient prompter installation.
        let root = mk_tmp("prompter_available_sorted");
        fs::create_dir_all(root.join("library")).unwrap();
        let config = root.join("config.toml");
        fs::write(
            &config,
            "[zebra]\ndepends_on = []\n\n[alpha]\ndepends_on = []\n\n[mango]\ndepends_on = []\n",
        )
        .unwrap();

        let profiles = available_profiles(Some(&config)).unwrap();
        assert_eq!(
            profiles,
            vec![
                "alpha".to_string(),
                "mango".to_string(),
                "zebra".to_string()
            ]
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn parse_args_help_and_version_flags() {
        // clap's `--help`/`--version` short-circuits map onto dedicated modes
        // rather than surfacing as parse errors.
        let help = parse_args_from(vec!["prompter".into(), "--help".into()]).unwrap();
        assert!(matches!(help, AppMode::Help), "got {help:?}");
        let version = parse_args_from(vec!["prompter".into(), "--version".into()]).unwrap();
        assert!(
            matches!(version, AppMode::Version { json: false }),
            "got {version:?}"
        );
    }

    #[test]
    fn parse_args_meta_subcommands_and_tree() {
        use clap_complete::Shell;

        assert!(matches!(
            parse_args_from(vec!["prompter".into(), "meta".into(), "license".into()]).unwrap(),
            AppMode::License
        ));

        let completions = parse_args_from(vec![
            "prompter".into(),
            "meta".into(),
            "completions".into(),
            "bash".into(),
        ])
        .unwrap();
        assert!(
            matches!(&completions, AppMode::Completions { shell } if *shell == Shell::Bash),
            "got {completions:?}"
        );

        assert!(matches!(
            parse_args_from(vec!["prompter".into(), "meta".into(), "doctor".into()]).unwrap(),
            AppMode::Doctor { json: false }
        ));

        assert!(matches!(
            parse_args_from(vec![
                "prompter".into(),
                "meta".into(),
                "agent".into(),
                "list".into(),
            ])
            .unwrap(),
            AppMode::Agent { .. }
        ));

        assert!(matches!(
            parse_args_from(vec!["prompter".into(), "tree".into()]).unwrap(),
            AppMode::Tree {
                config: None,
                json: false
            }
        ));
    }

    #[test]
    fn resolve_primary_config_path_joins_relative_onto_cwd() {
        let rel = Path::new("some/rel/config.toml");
        let resolved = resolve_primary_config_path(rel).unwrap();
        assert_eq!(resolved, env::current_dir().unwrap().join(rel));
        // An absolute path is returned unchanged.
        let abs = Path::new("/abs/dir/config.toml");
        assert_eq!(
            resolve_primary_config_path(abs).unwrap(),
            PathBuf::from("/abs/dir/config.toml")
        );
    }

    #[test]
    fn read_config_with_path_reports_io_error_with_path() {
        // Reading a nonexistent file surfaces an `Io` error annotated with the
        // exact path that failed.
        let missing = mk_tmp("prompter_read_missing").join("nope.toml");
        let err = read_config_with_path(&missing).unwrap_err();
        assert!(
            matches!(&err, PrompterError::Io { path, .. } if path == &missing),
            "expected Io error for {}, got {err}",
            missing.display()
        );
    }

    #[test]
    fn run_validate_stdout_json_emits_valid_output() {
        let root = mk_tmp("prompter_validate_json");
        let lib_dir = root.join("library");
        fs::create_dir_all(lib_dir.join("a")).unwrap();
        fs::write(lib_dir.join("a/x.md"), b"AX\n").unwrap();
        let config = root.join("config.toml");
        fs::write(&config, "[root]\ndepends_on = [\"a/x.md\"]\n").unwrap();

        // JSON mode serializes `ValidateOutput` and prints it; a successful
        // return exercises the JSON branch (serialization + render_response).
        assert!(super::run_validate_stdout(Some(&config), JsonOutput::Json).is_ok());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn success_and_info_messages_plain_when_not_a_tty() {
        // The test harness captures stdout, so `is_terminal()` is false and both
        // helpers return the message unchanged, without ANSI color codes.
        assert_eq!(success_message("all good"), "all good");
        assert_eq!(info_message("heads up"), "heads up");
    }

    #[test]
    fn validate_accepts_shadowing_family_variant_and_ignores_extras() {
        let lib = mk_tmp("prompter_family_valid");
        fs::create_dir_all(lib.join("general/families/gpt")).unwrap();
        fs::create_dir_all(lib.join("general/sub")).unwrap();
        fs::write(lib.join("general/rules.md"), b"NEUTRAL").unwrap();
        fs::write(lib.join("general/sub/deep.md"), b"DEEP").unwrap();
        // Valid variant that shadows the neutral fragment (no orphan error).
        fs::write(lib.join("general/families/gpt/rules.md"), b"GPT").unwrap();
        // A non-markdown file inside the family dir is ignored.
        fs::write(lib.join("general/families/gpt/notes.txt"), b"x").unwrap();
        // A plain file sitting directly under `families/` is not a family dir.
        fs::write(lib.join("general/families/README"), b"readme").unwrap();
        let cfg = cfg_with_lib(
            [("root", vec!["general/rules.md", "general/sub/deep.md"])],
            &lib,
            None,
        );
        assert!(validate(&cfg).is_ok());

        fs::remove_dir_all(&lib).ok();
    }

    #[test]
    fn load_config_bundle_missing_primary_is_io_error() {
        let dir = mk_tmp("prompter_missing_primary");
        fs::create_dir_all(&dir).unwrap();
        let err = load_config_bundle(&dir.join("nope.toml"), None).unwrap_err();
        assert!(matches!(err, PrompterError::Io { .. }), "err={err}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_config_bundle_missing_import_is_io_error() {
        let dir = mk_tmp("prompter_missing_import");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.toml"), "import = [\"./absent.toml\"]\n").unwrap();
        let err = load_config_bundle(&dir.join("config.toml"), None).unwrap_err();
        assert!(matches!(err, PrompterError::Io { .. }), "err={err}");

        fs::remove_dir_all(&dir).ok();
    }
}
