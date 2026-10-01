//! Integration coverage for the `clanker exec` command.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct ExecSandbox {
    _temp: TempDir,
    home: PathBuf,
    workdir: PathBuf,
    config: PathBuf,
}

impl ExecSandbox {
    fn new() -> TestResult<Self> {
        let temp = TempDir::new()?;
        let home = temp.path().join("home");
        let workdir = temp.path().join("cwd");
        for directory in [
            home.join(".config/alpha/personal"),
            home.join(".config/beta/personal"),
            home.join(".config/gamma/personal"),
            home.join(".config/alpha/work"),
            home.join(".config/beta/work"),
            home.join(".config/gamma/work"),
            home.join(".local/clankers/bin"),
            workdir.clone(),
        ] {
            fs::create_dir_all(directory)?;
        }
        let config = temp.path().join("config.toml");
        write_config(&config, false)?;
        Ok(Self {
            _temp: temp,
            home,
            workdir,
            config,
        })
    }

    fn command(&self) -> TestResult<Command> {
        let binary = Path::new(env!("CARGO_BIN_EXE_clanker"));
        let binary_dir = binary.parent().ok_or("clanker binary has no parent")?;
        let inherited = inherited_environment("PATH").unwrap_or_default();
        let path = std::env::join_paths(
            std::iter::once(binary_dir.to_path_buf()).chain(std::env::split_paths(&inherited)),
        )?;
        let mut command = Command::new(binary);
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", path)
            .env("CLANKER_CONFIG", &self.config)
            .env("CLANKER_CONTEXT", "personal")
            .current_dir(&self.workdir);
        forward_coverage_env(&mut command);
        Ok(command)
    }
}

fn write_config(path: &Path, collision: bool) -> std::io::Result<()> {
    let beta_environment = if collision { "ALPHA_HOME" } else { "BETA_HOME" };
    fs::write(
        path,
        format!(
            r#"
[defaults]
domain = "eng"
contexts = ["personal", "work"]
shim_path = "~/.local/clankers/bin"
sandbox_wrapper = "~/unused"
prompter_bundle = "~/unused"
prompt_cache = "~/unused"
prompt_cache_ttl_seconds = 1

[harness.alpha]
bin = "alpha"
family = "gpt"
default_args = []
[harness.alpha.injection]
kind = "arg-text"
args = ["{{text}}"]
[harness.alpha.config_dir]
environment = "ALPHA_HOME"
path = "~/.config/alpha/{{context}}"

[harness.beta]
bin = "beta"
family = "gpt"
default_args = []
[harness.beta.injection]
kind = "arg-text"
args = ["{{text}}"]
[harness.beta.config_dir]
environment = "{beta_environment}"
path = "~/.config/beta/{{context}}"

[harness.gamma]
bin = "gamma"
family = "gpt"
default_args = []
[harness.gamma.injection]
kind = "arg-text"
args = ["{{text}}"]
[harness.gamma.config_dir]
environment = "GAMMA_HOME"
path = "~/.config/gamma/{{context}}"

[harness.plain]
bin = "plain"
family = "gpt"
default_args = []
[harness.plain.injection]
kind = "arg-text"
args = ["{{text}}"]

[model]

[domain.eng]
profiles = ["core.base"]
env = {{}}
"#
        ),
    )
}

#[allow(
    clippy::disallowed_methods,
    reason = "integration tests capture PATH at the subprocess boundary"
)]
fn inherited_environment(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(name)
}

fn forward_coverage_env(command: &mut Command) {
    if let Some(profile) = inherited_environment("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn missing_directory_warning_uses_only_stderr() -> TestResult {
    let sandbox = ExecSandbox::new()?;
    fs::remove_dir(sandbox.home.join(".config/beta/personal"))?;
    let output = sandbox
        .command()?
        .args(["exec", "--", "/usr/bin/env"])
        .output()?;

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let expected_warning = format!(
        "warning: skipping harness `beta`: config directory {} does not exist; no variables from this harness will be exported\n",
        sandbox.home.join(".config/beta/personal").display()
    );
    assert_eq!(stderr(&output), expected_warning);
    assert!(!stdout(&output).contains("warning: skipping harness"));
    assert!(stdout(&output).contains(&format!(
        "ALPHA_HOME={}",
        sandbox.home.join(".config/alpha/personal").display()
    )));
    assert!(!stdout(&output).contains("BETA_HOME="));
    assert!(stdout(&output).contains(&format!(
        "GAMMA_HOME={}",
        sandbox.home.join(".config/gamma/personal").display()
    )));
    Ok(())
}

#[test]
fn exec_exports_every_harness_directory_for_the_resolved_context() -> TestResult {
    let sandbox = ExecSandbox::new()?;
    let output = sandbox
        .command()?
        .env("MNENE_AGENT", "stale-agent")
        .env("MNENE_CONTEXT", "stale-context")
        .env("MNENE_SESSION", "stale-session")
        .args(["exec", "--context", "work", "--", "/usr/bin/env"])
        .output()?;

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stderr(&output).is_empty());
    let environment = stdout(&output);
    for (variable, relative) in [
        ("ALPHA_HOME", ".config/alpha/work"),
        ("BETA_HOME", ".config/beta/work"),
        ("GAMMA_HOME", ".config/gamma/work"),
    ] {
        assert!(environment.contains(&format!(
            "{variable}={}",
            sandbox.home.join(relative).display()
        )));
    }
    assert!(environment.contains("CONTEXT=work\n"));
    assert!(environment.contains("CLANKER_CONTEXT=work\n"));
    assert!(!environment.contains("MNENE_AGENT="));
    assert!(!environment.contains("MNENE_CONTEXT="));
    assert!(!environment.contains("MNENE_SESSION="));
    assert!(environment.contains("PLAYWRIGHT_MCP_BROWSER=chromium\n"));
    assert!(environment.contains(&format!(
        "PLAYWRIGHT_MCP_USER_DATA_DIR={}",
        sandbox
            .home
            .join(".local/share/clanker/playwright/work/chromium")
            .display()
    )));
    let path = environment
        .lines()
        .find_map(|line| line.strip_prefix("PATH="))
        .ok_or("exec output did not contain PATH")?;
    assert_eq!(
        std::env::split_paths(path).next(),
        Some(sandbox.home.join(".local/clankers/bin"))
    );
    Ok(())
}

#[test]
fn conflicting_directory_variable_prevents_exec() -> TestResult {
    let sandbox = ExecSandbox::new()?;
    write_config(&sandbox.config, true)?;
    let output = sandbox
        .command()?
        .args(["exec", "--", "/usr/bin/env"])
        .output()?;

    assert!(!output.status.success());
    assert!(stdout(&output).is_empty(), "command unexpectedly executed");
    let error = stderr(&output);
    assert!(error.contains("config directory variable `ALPHA_HOME` resolves differently"));
    assert!(error.contains("harnesses `alpha`"));
    assert!(error.contains("and `beta`"));
    assert!(
        error.contains(
            &sandbox
                .home
                .join(".config/alpha/personal")
                .display()
                .to_string()
        )
    );
    assert!(
        error.contains(
            &sandbox
                .home
                .join(".config/beta/personal")
                .display()
                .to_string()
        )
    );
    Ok(())
}

#[test]
fn unresolved_context_prevents_exec() -> TestResult {
    let sandbox = ExecSandbox::new()?;
    let output = sandbox
        .command()?
        .env_remove("CLANKER_CONTEXT")
        .args(["exec", "--", "/usr/bin/env"])
        .output()?;

    assert!(!output.status.success());
    assert!(stdout(&output).is_empty(), "command unexpectedly executed");
    let error = stderr(&output);
    assert!(error.contains("no context selected"), "error: {error}");
    assert!(error.contains("--context"));
    assert!(error.contains(".clanker"));
    assert!(error.contains("CLANKER_CONTEXT"));
    Ok(())
}

#[test]
fn empty_exec_argv_reports_the_boundary_error() -> TestResult {
    let sandbox = ExecSandbox::new()?;
    let output = sandbox.command()?.arg("exec").output()?;

    assert!(!output.status.success());
    assert!(stdout(&output).is_empty());
    assert!(stderr(&output).contains("`clanker exec` requires at least one argv value after `--`"));
    Ok(())
}

#[test]
fn current_reports_inactive_inside_exec() -> TestResult {
    let sandbox = ExecSandbox::new()?;
    let output = sandbox
        .command()?
        .env("CLANKER_SESSION", "1")
        .env("CLANKER_SESSION_VERSION", "3")
        .env("CLANKER_SESSION_INVOCATION", "clanker alpha")
        .env("CLANKER_SESSION_HARNESS", "alpha")
        .env("CLANKER_SESSION_CONTEXT", "personal")
        .env("CLANKER_SESSION_CONTEXT_SOURCE", "CLANKER_CONTEXT")
        .env("CLANKER_SESSION_DOMAINS", "eng")
        .env("CLANKER_SESSION_DOMAIN_SOURCE", "defaults.domain")
        .env("CLANKER_SESSION_MODEL", "")
        .env("CLANKER_SESSION_MODEL_SOURCE", "")
        .env("CLANKER_SESSION_FAMILY", "gpt")
        .args(["exec", "--", "clanker", "current"])
        .output()?;

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "active=false\n");
    assert!(stderr(&output).is_empty());
    Ok(())
}
