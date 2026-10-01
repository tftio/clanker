//! Command-line definitions and launch-argument parsing for clanker.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use tftio_lib::MetaCommand;

use crate::config::{ContextName, DomainName, HarnessName, InvalidName, ModelName};
use crate::promptlib::FamilyName;

/// Top-level clanker command-mode arguments.
#[derive(Debug, Parser)]
#[command(name = "clanker")]
#[command(about = env!("CARGO_PKG_DESCRIPTION"))]
#[command(version)]
pub struct Cli {
    /// Override the required runtime configuration path.
    #[arg(long, value_name = "FILE", global = true)]
    pub config: Option<PathBuf>,
    /// Emit machine-readable JSON for supported commands.
    #[arg(long, global = true)]
    pub json: bool,
    /// Command or configured harness to execute.
    #[command(subcommand)]
    pub command: Command,
}

/// Clanker utilities plus dynamic harness subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Shared metadata commands.
    Meta {
        /// Shared metadata command to execute.
        #[command(subcommand)]
        command: MetaCommand,
    },
    /// Validate the merged runtime configuration.
    Doctor,
    /// List configured harnesses, models, and domains.
    List,
    /// Report how clanker launched the current process.
    Current,
    /// Print shell exports for a resolved harness environment.
    Env {
        /// Configured harness name.
        harness: OsString,
        /// Launch-selection flags; prompt injection is always disabled.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<OsString>,
    },
    /// Run a command with every harness config directory for one context.
    ///
    /// Place the command argv after `--`. Clanker resolves the context, exports
    /// every available harness config directory, and execs the argv directly
    /// without a shell or clanker session markers.
    Exec {
        /// Highest-precedence context override.
        #[arg(long, value_name = "CONTEXT")]
        context: Option<OsString>,
        /// Verbatim command argv beginning after `--`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        argv: Vec<OsString>,
    },
    /// Resolve or list projects known to the project registry.
    Project {
        /// Project operation to run.
        #[command(subcommand)]
        command: ProjectCommand,
    },
    /// Compose or inspect prompt profiles from the configured prompter bundle.
    ///
    /// Replaces the retired `prompter` binary. `--config` here (from the
    /// top-level flag) overrides the prompter bundle's own `config.toml`,
    /// exactly as `prompter -c` did; omit it to use the bundle named by
    /// `defaults.prompter_bundle` in clanker's own configuration.
    Prompt {
        /// Prompt operation to run.
        #[command(subcommand)]
        command: PromptCommand,
    },
    /// Launch a configured harness. The first value is its name.
    #[command(external_subcommand)]
    Harness(Vec<OsString>),
}

/// Project registry operations.
#[derive(Debug, Subcommand)]
pub enum ProjectCommand {
    /// Resolve the project for a directory.
    ///
    /// Applies the same derivation `clanker` uses at launch: a `.clanker`
    /// declaration at the directory's project root, then a registered
    /// registry path, then a registered remote, then a slug derived from an
    /// unregistered remote, then none. Exits `0` whether or not a project
    /// resolves; text output is a single tab-separated line
    /// `<slug>\t<source>\t<remote>` with empty fields when unresolved, so a
    /// shell hook can consume it directly.
    Resolve {
        /// Directory to resolve, defaulting to the current directory.
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
    },
    /// List every project the registry knows about.
    List,
}

/// Prompt-composition operations that replace the retired `prompter` CLI.
#[derive(Debug, Subcommand)]
pub enum PromptCommand {
    /// List available profiles.
    List,
    /// Show the dependency tree for every profile.
    Tree,
    /// Validate configuration and library references.
    Validate,
    /// Render one or more profiles (concatenated, deduplicated file contents).
    Run {
        /// Profile name(s) to render.
        #[arg(required = true)]
        profiles: Vec<String>,
        /// Model family used to select fragment variants.
        #[arg(long, value_name = "NAME")]
        family: Option<FamilyName>,
        /// Separator between files.
        #[arg(short, long)]
        separator: Option<String>,
        /// Pre-prompt text to inject at the beginning.
        #[arg(short = 'p', long)]
        pre_prompt: Option<String>,
        /// Post-prompt text to inject at the end.
        #[arg(short = 'P', long)]
        post_prompt: Option<String>,
        /// Emit only the deduplicated fragments, suppressing the auto-injected
        /// default pre-prompt, the date/system context, and the post-prompt
        /// (an explicit --pre-prompt/--post-prompt still applies).
        #[arg(short = 'b', long)]
        bare: bool,
    },
    /// Render the always-on invariant base (the agent system prompt),
    /// optionally followed by additional profiles.
    System {
        /// Additional profile(s) to append after the system base.
        profiles: Vec<String>,
        /// Separator between files.
        #[arg(short, long)]
        separator: Option<String>,
        /// Pre-prompt text to inject at the beginning.
        #[arg(short = 'p', long)]
        pre_prompt: Option<String>,
        /// Post-prompt text to inject at the end.
        #[arg(short = 'P', long)]
        post_prompt: Option<String>,
        /// Emit only the deduplicated fragments, suppressing the auto-injected
        /// default pre-prompt, the date/system context, and the post-prompt
        /// (an explicit --pre-prompt/--post-prompt still applies).
        #[arg(short = 'b', long)]
        bare: bool,
    },
}

/// Whether a launch applies the harness's configured default arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultArgsMode {
    /// Prepend the harness's configured default arguments.
    Apply,
    /// Suppress the harness's configured default arguments.
    Suppress,
}

/// Parsed clanker flags plus the untouched harness argument suffix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchRequest {
    /// Selected harness.
    pub harness: HarnessName,
    /// Highest-precedence context override.
    pub context: Option<ContextName>,
    /// Highest-precedence domain stack, in request order.
    pub domains: Vec<DomainName>,
    /// Highest-precedence model override.
    pub model: Option<ModelName>,
    /// Additional prompter profiles, in request order.
    pub profiles: Vec<String>,
    /// Wrap the final executable with the configured sandbox command.
    pub sandbox: bool,
    /// Disable prompt composition and injection.
    pub no_prompt: bool,
    /// Whether the harness's configured default arguments are applied.
    pub default_args: DefaultArgsMode,
    /// Resolve and print the plan without exec.
    pub dry_run: bool,
    /// Harness arguments beginning at `--` or the first unrecognized token.
    pub passthrough: Vec<OsString>,
}

/// Parsed request to execute an arbitrary command under one context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecRequest {
    /// Highest-precedence context override.
    pub context: Option<ContextName>,
    /// Executable name or path from the first argv value.
    pub executable: OsString,
    /// Verbatim argv values after the executable.
    pub args: Vec<OsString>,
}

/// Invalid clanker launch-selection arguments.
#[derive(Debug, thiserror::Error)]
pub enum LaunchArgumentError {
    /// Dynamic harness subcommand is missing its harness name.
    #[error("missing harness name")]
    MissingHarness,
    /// A clanker option lacks its required value.
    #[error("{0} requires a value")]
    MissingValue(&'static str),
    /// A clanker option value is not UTF-8.
    #[error("{0} value is not valid UTF-8")]
    NonUtf8Value(&'static str),
    /// An additional profile is empty.
    #[error("--profile requires a non-empty value")]
    EmptyProfile,
    /// Exec command is missing the argv that follows the explicit boundary.
    #[error("`clanker exec` requires at least one argv value after `--`")]
    MissingExecArgv,
    /// A context, domain, model, or harness name is invalid.
    #[error(transparent)]
    InvalidName(#[from] InvalidName),
}

/// Parse an exec request without interpreting its command argv.
///
/// # Errors
/// Returns [`LaunchArgumentError`] when the command argv is empty or the
/// context override is invalid.
pub fn parse_exec(
    context: Option<OsString>,
    argv: Vec<OsString>,
) -> Result<ExecRequest, LaunchArgumentError> {
    let mut argv = argv.into_iter();
    let executable = argv.next().ok_or(LaunchArgumentError::MissingExecArgv)?;
    let context = context
        .map(|value| parse_name(value, "--context", ContextName::new))
        .transpose()?;
    Ok(ExecRequest {
        context,
        executable,
        args: argv.collect(),
    })
}

/// Parse an external-subcommand vector containing harness then arguments.
///
/// # Errors
/// Returns [`LaunchArgumentError`] for a missing harness or invalid recognized
/// clanker flag. The first unrecognized token begins verbatim passthrough.
pub fn parse_external_launch(values: Vec<OsString>) -> Result<LaunchRequest, LaunchArgumentError> {
    let mut values = values.into_iter();
    let harness = values.next().ok_or(LaunchArgumentError::MissingHarness)?;
    parse_launch(harness, values.collect())
}

/// Parse launch-selection flags for one dynamic harness.
///
/// Recognized clanker flags are consumed until `--` or the first unrecognized
/// token. That boundary token and every later value are passed to the harness.
///
/// # Errors
/// Returns [`LaunchArgumentError`] for invalid recognized flag values.
pub fn parse_launch(
    harness: OsString,
    arguments: Vec<OsString>,
) -> Result<LaunchRequest, LaunchArgumentError> {
    let harness = parse_name(harness, "harness", HarnessName::new)?;
    let mut request = LaunchRequest {
        harness,
        context: None,
        domains: Vec::new(),
        model: None,
        profiles: Vec::new(),
        sandbox: false,
        no_prompt: false,
        default_args: DefaultArgsMode::Apply,
        dry_run: false,
        passthrough: Vec::new(),
    };
    let mut arguments = arguments.into_iter();

    while let Some(argument) = arguments.next() {
        let Some(text) = argument.to_str() else {
            request.passthrough.push(argument);
            request.passthrough.extend(arguments);
            break;
        };
        match text {
            "--" => {
                request.passthrough.extend(arguments);
                break;
            }
            "--context" => {
                request.context = Some(parse_next_name(
                    &mut arguments,
                    "--context",
                    ContextName::new,
                )?);
            }
            "--domain" => {
                request.domains.push(parse_next_name(
                    &mut arguments,
                    "--domain",
                    DomainName::new,
                )?);
            }
            "--model" => {
                request.model = Some(parse_next_name(&mut arguments, "--model", ModelName::new)?);
            }
            "--profile" => {
                request
                    .profiles
                    .push(parse_profile(next_value(&mut arguments, "--profile")?)?);
            }
            "--sandbox" => request.sandbox = true,
            "--no-prompt" => request.no_prompt = true,
            "--no-default-args" => request.default_args = DefaultArgsMode::Suppress,
            "--dry-run" => request.dry_run = true,
            _ => {
                if let Some(value) = text.strip_prefix("--context=") {
                    request.context = Some(ContextName::new(value)?);
                } else if let Some(value) = text.strip_prefix("--domain=") {
                    request.domains.push(DomainName::new(value)?);
                } else if let Some(value) = text.strip_prefix("--model=") {
                    request.model = Some(ModelName::new(value)?);
                } else if let Some(value) = text.strip_prefix("--profile=") {
                    request.profiles.push(parse_profile(value.to_string())?);
                } else {
                    request.passthrough.push(argument);
                    request.passthrough.extend(arguments);
                    break;
                }
            }
        }
    }

    Ok(request)
}

fn next_value(
    arguments: &mut impl Iterator<Item = OsString>,
    flag: &'static str,
) -> Result<String, LaunchArgumentError> {
    arguments
        .next()
        .ok_or(LaunchArgumentError::MissingValue(flag))?
        .into_string()
        .map_err(|_| LaunchArgumentError::NonUtf8Value(flag))
}

fn parse_next_name<T>(
    arguments: &mut impl Iterator<Item = OsString>,
    flag: &'static str,
    parse: impl FnOnce(String) -> Result<T, InvalidName>,
) -> Result<T, LaunchArgumentError> {
    parse(next_value(arguments, flag)?).map_err(Into::into)
}

fn parse_name<T>(
    value: OsString,
    label: &'static str,
    parse: impl FnOnce(String) -> Result<T, InvalidName>,
) -> Result<T, LaunchArgumentError> {
    let value = value
        .into_string()
        .map_err(|_| LaunchArgumentError::NonUtf8Value(label))?;
    parse(value).map_err(Into::into)
}

fn parse_profile(value: String) -> Result<String, LaunchArgumentError> {
    if value.trim().is_empty() {
        Err(LaunchArgumentError::EmptyProfile)
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn exec_rejects_an_empty_argv_with_boundary_context() {
        let error = parse_exec(None, Vec::new()).unwrap_err();
        assert!(matches!(error, LaunchArgumentError::MissingExecArgv));
        assert!(error.to_string().contains("after `--`"));
    }

    #[test]
    fn exec_captures_a_bare_argv_verbatim() {
        let cli = Cli::try_parse_from(["clanker", "exec", "--", "printf", "%s", "value"]).unwrap();
        let Command::Exec { context, argv } = cli.command else {
            panic!("expected exec command");
        };
        let request = parse_exec(context, argv).unwrap();
        assert_eq!(request.executable, OsString::from("printf"));
        assert_eq!(request.args, values(&["%s", "value"]));
    }

    #[test]
    fn exec_captures_a_hyphen_leading_executable() {
        let cli = Cli::try_parse_from(["clanker", "exec", "--", "--version"]).unwrap();
        let Command::Exec { context, argv } = cli.command else {
            panic!("expected exec command");
        };
        let request = parse_exec(context, argv).unwrap();
        assert_eq!(request.executable, OsString::from("--version"));
        assert!(request.args.is_empty());
    }

    #[test]
    fn exec_parses_a_context_override_before_the_argv_boundary() {
        let cli =
            Cli::try_parse_from(["clanker", "exec", "--context", "work", "--", "/usr/bin/env"])
                .unwrap();
        let Command::Exec { context, argv } = cli.command else {
            panic!("expected exec command");
        };
        let request = parse_exec(context, argv).unwrap();
        assert_eq!(
            request.context.map(|value| value.to_string()).as_deref(),
            Some("work")
        );
        assert_eq!(request.executable, OsString::from("/usr/bin/env"));
        assert!(request.args.is_empty());
    }

    #[test]
    fn parses_selection_flags_before_passthrough() {
        let request = parse_external_launch(values(&[
            "claude",
            "--domain",
            "research",
            "--model=codex-native",
            "--profile",
            "python.full",
            "--sandbox",
            "prompt",
            "--domain",
            "ignored",
        ]))
        .unwrap();

        assert_eq!(request.harness.as_str(), "claude");
        assert_eq!(request.domains[0].as_str(), "research");
        assert_eq!(request.model.unwrap().as_str(), "codex-native");
        assert_eq!(request.profiles, ["python.full"]);
        assert!(request.sandbox);
        assert_eq!(
            request.passthrough,
            values(&["prompt", "--domain", "ignored"])
        );
    }

    #[test]
    fn repeated_domains_preserve_request_order() {
        let request = parse_external_launch(values(&[
            "claude",
            "--domain",
            "eng",
            "--domain=research",
            "--dry-run",
        ]))
        .unwrap();

        let domains: Vec<_> = request.domains.iter().map(DomainName::as_str).collect();
        assert_eq!(domains, ["eng", "research"]);
    }

    #[test]
    fn explicit_separator_is_not_forwarded() {
        let request = parse_external_launch(values(&[
            "codex",
            "--no-prompt",
            "--",
            "--domain",
            "harness-value",
        ]))
        .unwrap();

        assert!(request.no_prompt);
        assert_eq!(request.passthrough, values(&["--domain", "harness-value"]));
    }

    #[test]
    fn no_default_args_is_consumed_only_before_passthrough() {
        let request = parse_external_launch(values(&[
            "claude",
            "--no-default-args",
            "--",
            "--no-default-args",
        ]))
        .unwrap();

        assert_eq!(request.default_args, DefaultArgsMode::Suppress);
        assert_eq!(request.passthrough, values(&["--no-default-args"]));

        let request =
            parse_external_launch(values(&["claude", "prompt", "--no-default-args"])).unwrap();
        assert_eq!(request.default_args, DefaultArgsMode::Apply);
        assert_eq!(
            request.passthrough,
            values(&["prompt", "--no-default-args"])
        );
    }

    #[test]
    fn missing_recognized_value_is_an_error() {
        let error = parse_external_launch(values(&["claude", "--domain"]))
            .unwrap_err()
            .to_string();
        assert_eq!(error, "--domain requires a value");
    }

    #[test]
    fn context_and_equals_forms_are_recognized() {
        let request = parse_external_launch(values(&[
            "claude",
            "--context",
            "work",
            "--context=personal",
            "--profile=python.full",
        ]))
        .unwrap();

        assert_eq!(request.context.unwrap().as_str(), "personal");
        assert_eq!(request.profiles, ["python.full"]);
    }

    #[test]
    fn empty_profile_value_is_rejected() {
        let error = parse_external_launch(values(&["claude", "--profile", ""])).unwrap_err();
        assert!(matches!(error, LaunchArgumentError::EmptyProfile));
    }

    #[test]
    fn non_utf8_argument_begins_passthrough() {
        use std::os::unix::ffi::OsStringExt;
        let non_utf8 = OsString::from_vec(vec![b'-', b'-', 0xff]);
        let request = parse_external_launch(vec![
            OsString::from("claude"),
            non_utf8.clone(),
            OsString::from("later"),
        ])
        .unwrap();

        assert_eq!(request.passthrough, vec![non_utf8, OsString::from("later")]);
    }
}
