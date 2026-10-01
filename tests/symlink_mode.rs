//! Integration coverage for the multi-call launcher contract.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[allow(
    clippy::literal_string_with_formatting_args,
    reason = "the braces are POSIX shell parameter expansion in a test fixture"
)]
/// Fixed launch identifier for tests that supply inherited markers rather
/// than letting clanker mint one.
const TEST_SESSION_ID: &str = "01a06ebb-0000-7000-8000-000000000000";

const STUB_SCRIPT: &str = r#"#!/bin/sh
printf 'CONTEXT=%s\n' "${CONTEXT:-}"
printf 'CLANKER_CONTEXT=%s\n' "${CLANKER_CONTEXT:-}"
printf 'PLAYWRIGHT_MCP_BROWSER=%s\n' "${PLAYWRIGHT_MCP_BROWSER:-}"
printf 'PLAYWRIGHT_MCP_USER_DATA_DIR=%s\n' "${PLAYWRIGHT_MCP_USER_DATA_DIR:-}"
printf 'CLAUDE_CONFIG_DIR=%s\n' "${CLAUDE_CONFIG_DIR:-}"
printf 'CODEX_HOME=%s\n' "${CODEX_HOME:-}"
printf 'GEMINI_SYSTEM_MD=%s\n' "${GEMINI_SYSTEM_MD:-}"
printf 'OPENCODE_CONFIG_CONTENT=%s\n' "${OPENCODE_CONFIG_CONTENT:-}"
printf 'OPENCODE_CONFIG_DIR=%s\n' "${OPENCODE_CONFIG_DIR:-}"
printf 'POLYTOKEN_CONFIG_PATH=%s\n' "${POLYTOKEN_CONFIG_PATH:-}"
printf 'POLYTOKEN_DATA_PATH=%s\n' "${POLYTOKEN_DATA_PATH:-}"
printf 'POLYTOKEN_CACHE_PATH=%s\n' "${POLYTOKEN_CACHE_PATH:-}"
printf 'CLANKER_PROMPT_FILE=%s\n' "${CLANKER_PROMPT_FILE:-}"
printf 'ANTHROPIC_BASE_URL=%s\n' "${ANTHROPIC_BASE_URL:-}"
printf 'ANTHROPIC_MODEL=%s\n' "${ANTHROPIC_MODEL:-}"
printf 'CLANKER_TEST_DOMAIN=%s\n' "${CLANKER_TEST_DOMAIN:-}"
printf 'CLANKER_SESSION=%s\n' "${CLANKER_SESSION:-}"
printf 'CLANKER_SESSION_VERSION=%s\n' "${CLANKER_SESSION_VERSION:-}"
printf 'CLANKER_SESSION_ID=%s\n' "${CLANKER_SESSION_ID:-}"
printf 'CLANKER_SESSION_INVOCATION=%s\n' "${CLANKER_SESSION_INVOCATION:-}"
printf 'CLANKER_SESSION_HARNESS=%s\n' "${CLANKER_SESSION_HARNESS:-}"
printf 'CLANKER_SESSION_CONTEXT=%s\n' "${CLANKER_SESSION_CONTEXT:-}"
printf 'CLANKER_SESSION_CONTEXT_SOURCE=%s\n' "${CLANKER_SESSION_CONTEXT_SOURCE:-}"
printf 'CLANKER_SESSION_DOMAINS=%s\n' "${CLANKER_SESSION_DOMAINS:-}"
printf 'CLANKER_SESSION_DOMAIN_SOURCE=%s\n' "${CLANKER_SESSION_DOMAIN_SOURCE:-}"
printf 'CLANKER_SESSION_MODEL=%s\n' "${CLANKER_SESSION_MODEL:-}"
printf 'CLANKER_SESSION_MODEL_SOURCE=%s\n' "${CLANKER_SESSION_MODEL_SOURCE:-}"
printf 'CLANKER_SESSION_FAMILY=%s\n' "${CLANKER_SESSION_FAMILY:-}"
printf 'CLANKER_SESSION_PROJECT=%s\n' "${CLANKER_SESSION_PROJECT:-}"
printf 'CLANKER_SESSION_PROJECT_SOURCE=%s\n' "${CLANKER_SESSION_PROJECT_SOURCE:-}"
printf 'CLANKER_SESSION_REMOTE=%s\n' "${CLANKER_SESSION_REMOTE:-}"
printf 'MNENE_AGENT=%s\n' "${MNENE_AGENT:-}"
printf 'MNENE_CONTEXT=%s\n' "${MNENE_CONTEXT:-}"
printf 'MNENE_SESSION=%s\n' "${MNENE_SESSION:-}"
printf 'MNENE_SCOPE=%s\n' "${MNENE_SCOPE:-}"
printf 'PATH=%s\n' "${PATH:-}"
printf 'ARGS='
printf '%s|' "$@"
printf '\n'
"#;

const fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_clanker")
}

fn json_field<'a>(
    value: &'a serde_json::Value,
    pointer: &str,
) -> Result<&'a serde_json::Value, Box<dyn std::error::Error>> {
    value
        .pointer(pointer)
        .ok_or_else(|| format!("missing json field {pointer}").into())
}

fn fixture_config() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config.toml")
}

fn write_harness_stubs(bin: &Path) -> std::io::Result<()> {
    for harness in ["claude", "codex", "gemini", "opencode", "polytoken"] {
        let stub = bin.join(harness);
        fs::write(&stub, STUB_SCRIPT)?;
        let mut permissions = fs::metadata(&stub)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&stub, permissions)?;
    }
    for launcher in [
        "claude-launch",
        "codex-launch",
        "gemini-launch",
        "opencode-launch",
        "polytoken-launch",
    ] {
        symlink(binary(), bin.join(launcher))?;
    }
    Ok(())
}

fn write_prompter_bundle(home: &Path) -> std::io::Result<()> {
    fs::write(
        home.join(".local/prompter/config.toml"),
        r#"
library = "library"

[core.base]
depends_on = ["base.md"]

[domain.eng]
depends_on = ["eng.md"]

[domain.research]
depends_on = ["research.md"]

[extra]
depends_on = ["extra.md"]
"#,
    )?;
    fs::write(home.join(".local/prompter/library/base.md"), "BASE\n")?;
    fs::write(home.join(".local/prompter/library/eng.md"), "ENG\n")?;
    fs::write(
        home.join(".local/prompter/library/research.md"),
        "RESEARCH\n",
    )?;
    fs::write(home.join(".local/prompter/library/extra.md"), "EXTRA\n")?;
    fs::write(
        home.join(".local/prompter/library/families/gpt/base.md"),
        "GPT BASE\n",
    )?;
    Ok(())
}

/// Write a real (non-worktree) `.git` layout at `repo_dir`, with `origin`
/// as the sole remote, by hand -- the sandbox has no `git` binary on its
/// `PATH`, matching `tftio_lib::project::git`'s own subprocess-free
/// discovery, which this exercises end to end through clanker.
fn write_git_repo(repo_dir: &Path, origin: &str) -> std::io::Result<()> {
    let git_dir = repo_dir.join(".git");
    fs::create_dir_all(&git_dir)?;
    fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n")?;
    fs::write(
        git_dir.join("config"),
        format!("[remote \"origin\"]\n\turl = {origin}\n"),
    )
}

/// Write a project registry under the sandbox's `HOME`-derived
/// `${XDG_CONFIG_HOME:-~/.config}/tftio/projects.toml`.
fn write_project_registry(home: &Path, body: &str) -> std::io::Result<()> {
    let dir = home.join(".config/tftio");
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("projects.toml"), body)
}

fn write_sandbox_wrapper(home: &Path) -> std::io::Result<()> {
    let sandbox_wrapper = home.join(".config/sandbox-exec/run-sandboxed.sh");
    fs::write(
        &sandbox_wrapper,
        "#!/bin/sh\nprintf 'SANDBOX=%s\\n' \"$1\"\nexec \"$@\"\n",
    )?;
    let mut permissions = fs::metadata(&sandbox_wrapper)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&sandbox_wrapper, permissions)?;
    Ok(())
}

struct Sandbox {
    _temp: TempDir,
    home: PathBuf,
    bin: PathBuf,
    workdir: PathBuf,
}

impl Sandbox {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let root = temp.path();
        let home = root.join("home");
        let bin = root.join("bin");
        let workdir = root.join("cwd");
        for directory in [
            home.join(".config/claude/personal"),
            home.join(".config/claude/work"),
            home.join(".config/codex/personal"),
            home.join(".config/codex/work"),
            home.join(".config/opencode/personal"),
            home.join(".config/opencode/work"),
            home.join(".config/polytoken/personal"),
            home.join(".config/polytoken/work"),
            home.join(".local/share/polytoken/personal"),
            home.join(".local/share/polytoken/work"),
            home.join(".cache/polytoken/personal"),
            home.join(".cache/polytoken/work"),
            home.join(".config/sandbox-exec"),
            home.join(".local/clankers/bin"),
            home.join(".local/prompter/library/families/gpt"),
            bin.clone(),
            workdir.clone(),
        ] {
            fs::create_dir_all(directory)?;
        }

        write_harness_stubs(&bin)?;
        write_prompter_bundle(&home)?;
        write_sandbox_wrapper(&home)?;

        Ok(Self {
            _temp: temp,
            home,
            bin,
            workdir,
        })
    }

    /// A symlink-mode launcher with no context supplied by any tier.
    fn bare_launch(&self, launcher: &str) -> Result<Command, Box<dyn std::error::Error>> {
        let inherited = inherited_path();
        let path = std::env::join_paths(
            std::iter::once(self.bin.clone()).chain(std::env::split_paths(&inherited)),
        )?;
        let mut command = Command::new(self.bin.join(launcher));
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", path)
            .env("CLANKER_CONFIG", fixture_config())
            .current_dir(&self.workdir);
        forward_coverage_env(&mut command);
        Ok(command)
    }

    /// A symlink-mode launcher with the ambient host-default context set.
    fn launch(&self, launcher: &str) -> Result<Command, Box<dyn std::error::Error>> {
        let mut command = self.bare_launch(launcher)?;
        command.env("CLANKER_CONTEXT", "personal");
        Ok(command)
    }

    /// Command mode with no config and no context.
    fn bare_command(&self) -> Result<Command, Box<dyn std::error::Error>> {
        let inherited = inherited_path();
        let path = std::env::join_paths(
            std::iter::once(self.bin.clone()).chain(std::env::split_paths(&inherited)),
        )?;
        let mut command = Command::new(binary());
        command
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", path)
            .current_dir(&self.workdir);
        forward_coverage_env(&mut command);
        Ok(command)
    }

    /// Command mode with the ambient host-default context set.
    fn command(&self) -> Result<Command, Box<dyn std::error::Error>> {
        let mut command = self.bare_command()?;
        command.env("CLANKER_CONTEXT", "personal");
        Ok(command)
    }

    /// Command mode with the fixture config and the ambient context set.
    fn configured_command(&self) -> Result<Command, Box<dyn std::error::Error>> {
        let mut command = self.command()?;
        command.env("CLANKER_CONFIG", fixture_config());
        Ok(command)
    }
}

#[allow(
    clippy::disallowed_methods,
    reason = "integration tests capture PATH at the subprocess boundary"
)]
fn inherited_path() -> std::ffi::OsString {
    std::env::var_os("PATH").unwrap_or_default()
}

/// Preserve the coverage instrumentation profile path across `env_clear`, so a
/// spawned instrumented binary still records line coverage under `llvm-cov`.
#[allow(
    clippy::disallowed_methods,
    reason = "coverage profile path is forwarded once at the subprocess boundary"
)]
fn forward_coverage_env(command: &mut Command) {
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
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
fn clanker_context_supplies_the_host_default() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox
        .bare_launch("claude-launch")?
        .env("CLANKER_CONTEXT", "work")
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("CONTEXT=work\n"));
    assert!(stdout.contains("CLANKER_CONTEXT=work\n"));
    assert!(stdout.contains(&format!(
        "CLAUDE_CONFIG_DIR={}/.config/claude/work\n",
        sandbox.home.display()
    )));
    Ok(())
}

#[test]
fn playwright_mcp_uses_managed_chromium_and_a_context_profile() -> TestResult {
    let sandbox = Sandbox::new()?;
    for context in ["personal", "work"] {
        let output = sandbox
            .bare_launch("claude-launch")?
            .env("CLANKER_CONTEXT", context)
            .output()?;
        assert!(output.status.success(), "stderr: {}", stderr(&output));
        let output = stdout(&output);
        assert!(output.contains("PLAYWRIGHT_MCP_BROWSER=chromium\n"));
        assert!(output.contains(&format!(
            "PLAYWRIGHT_MCP_USER_DATA_DIR={}/.local/share/clanker/playwright/{context}/chromium\n",
            sandbox.home.display()
        )));
    }
    Ok(())
}

#[test]
fn a_bare_context_variable_does_not_influence_resolution() -> TestResult {
    let sandbox = Sandbox::new()?;
    // `CONTEXT` is exported for downstream consumers but is never read back.
    let output = sandbox
        .bare_launch("claude-launch")?
        .env("CONTEXT", "work")
        .output()?;

    assert!(!output.status.success());
    let error = stderr(&output);
    assert!(error.contains("no context selected"), "stderr: {error}");
    Ok(())
}

#[test]
fn an_unresolved_context_fails_closed_in_both_modes() -> TestResult {
    let sandbox = Sandbox::new()?;

    let symlink_mode = sandbox.bare_launch("claude-launch")?.output()?;
    assert!(!symlink_mode.status.success());
    let error = stderr(&symlink_mode);
    assert!(error.contains("--context"), "stderr: {error}");
    assert!(error.contains(".clanker"), "stderr: {error}");
    assert!(error.contains("CLANKER_CONTEXT"), "stderr: {error}");

    let command_mode = sandbox
        .bare_command()?
        .env("CLANKER_CONFIG", fixture_config())
        .args(["claude", "--dry-run"])
        .output()?;
    assert!(!command_mode.status.success());
    assert!(stderr(&command_mode).contains("no context selected"));
    Ok(())
}

#[test]
fn clanker_file_beats_the_environment() -> TestResult {
    let sandbox = Sandbox::new()?;
    fs::write(sandbox.workdir.join(".clanker"), "context = \"work\"\n")?;
    let output = sandbox
        .bare_launch("claude-launch")?
        .env("CLANKER_CONTEXT", "personal")
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("CONTEXT=work\n"));
    Ok(())
}

#[test]
fn a_directory_form_clanker_fails_closed() -> TestResult {
    let sandbox = Sandbox::new()?;
    fs::create_dir_all(sandbox.workdir.join(".clanker"))?;
    fs::write(
        sandbox.workdir.join(".clanker/config.toml"),
        "context = \"work\"\n",
    )?;

    let output = sandbox.launch("claude-launch")?.output()?;

    assert!(!output.status.success());
    let error = stderr(&output);
    assert!(error.contains("is a directory"), "stderr: {error}");
    assert!(error.contains("flat .clanker TOML file"), "stderr: {error}");
    Ok(())
}

#[test]
fn ancestor_clanker_applies_all_directory_axes() -> TestResult {
    let sandbox = Sandbox::new()?;
    fs::write(
        sandbox.workdir.join(".clanker"),
        "context = \"work\"\ndomain = \"research\"\n",
    )?;
    let nested = sandbox.workdir.join("repo/src");
    fs::create_dir_all(&nested)?;

    let output = sandbox
        .configured_command()?
        .current_dir(nested)
        .env("CLANKER_DOMAIN", "eng")
        .args(["codex", "--dry-run"])
        .output()?;

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let output = stdout(&output);
    assert!(output.contains("context=work\n"));
    assert!(output.contains("domain=research\n"));
    assert!(output.contains("context_source=.clanker\n"));
    assert!(output.contains("domain_source=.clanker\n"));
    Ok(())
}

#[test]
fn empty_clanker_file_falls_through_to_environment() -> TestResult {
    let sandbox = Sandbox::new()?;
    fs::write(sandbox.workdir.join(".clanker"), "")?;
    let output = sandbox
        .bare_launch("claude-launch")?
        .env("CLANKER_CONTEXT", "work")
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("CONTEXT=work\n"));
    Ok(())
}

#[test]
fn codex_derives_home_from_context() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox
        .bare_launch("codex-launch")?
        .env("CLANKER_CONTEXT", "work")
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("CONTEXT=work\n"));
    assert!(stdout.contains(&format!(
        "CODEX_HOME={}/.config/codex/work\n",
        sandbox.home.display()
    )));
    Ok(())
}

#[test]
fn missing_codex_home_fails_closed() -> TestResult {
    let sandbox = Sandbox::new()?;
    fs::remove_dir_all(sandbox.home.join(".config/codex/personal"))?;
    let output = sandbox.launch("codex-launch")?.output()?;
    assert!(!output.status.success());
    let error = stderr(&output);
    assert!(error.contains("config directory"));
    assert!(error.contains(".config/codex/personal"));
    Ok(())
}

#[test]
fn missing_claude_config_fails_closed() -> TestResult {
    let sandbox = Sandbox::new()?;
    fs::remove_dir_all(sandbox.home.join(".config/claude/work"))?;
    let output = sandbox
        .bare_launch("claude-launch")?
        .env("CLANKER_CONTEXT", "work")
        .output()?;
    assert!(!output.status.success());
    let error = stderr(&output);
    assert!(error.contains("config directory"));
    assert!(error.contains(".config/claude/work"));
    Ok(())
}

#[test]
fn opencode_derives_config_dir_from_context() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox
        .bare_launch("opencode-launch")?
        .env("CLANKER_CONTEXT", "work")
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("CONTEXT=work\n"));
    assert!(stdout.contains(&format!(
        "OPENCODE_CONFIG_DIR={}/.config/opencode/work\n",
        sandbox.home.display()
    )));
    Ok(())
}

#[test]
fn missing_opencode_config_dir_fails_closed() -> TestResult {
    let sandbox = Sandbox::new()?;
    fs::remove_dir_all(sandbox.home.join(".config/opencode/work"))?;
    let output = sandbox
        .bare_launch("opencode-launch")?
        .env("CLANKER_CONTEXT", "work")
        .output()?;
    assert!(!output.status.success());
    let error = stderr(&output);
    assert!(error.contains("config directory"));
    assert!(error.contains(".config/opencode/work"));
    Ok(())
}

#[test]
fn polytoken_derives_every_context_directory() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox
        .bare_launch("polytoken-launch")?
        .env("CLANKER_CONTEXT", "work")
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("CONTEXT=work\n"));
    for (variable, directory) in [
        ("POLYTOKEN_CONFIG_PATH", ".config/polytoken/work"),
        ("POLYTOKEN_DATA_PATH", ".local/share/polytoken/work"),
        ("POLYTOKEN_CACHE_PATH", ".cache/polytoken/work"),
    ] {
        assert!(
            stdout.contains(&format!(
                "{variable}={}/{directory}\n",
                sandbox.home.display()
            )),
            "missing {variable} in: {stdout}"
        );
    }
    Ok(())
}

#[test]
fn a_missing_polytoken_data_directory_fails_closed() -> TestResult {
    let sandbox = Sandbox::new()?;
    fs::remove_dir_all(sandbox.home.join(".local/share/polytoken/work"))?;
    let output = sandbox
        .bare_launch("polytoken-launch")?
        .env("CLANKER_CONTEXT", "work")
        .output()?;
    assert!(!output.status.success());
    let error = stderr(&output);
    assert!(error.contains("config directory"));
    assert!(error.contains(".local/share/polytoken/work"));
    Ok(())
}

#[test]
fn a_missing_polytoken_cache_directory_fails_closed_and_exports_nothing() -> TestResult {
    let sandbox = Sandbox::new()?;
    fs::remove_dir_all(sandbox.home.join(".cache/polytoken/work"))?;
    let output = sandbox
        .bare_launch("polytoken-launch")?
        .env("CLANKER_CONTEXT", "work")
        .output()?;
    assert!(!output.status.success());
    let error = stderr(&output);
    assert!(error.contains(".cache/polytoken/work"));
    assert!(
        stdout(&output).is_empty(),
        "a failed launch must not run the harness"
    );
    Ok(())
}

#[test]
fn polytoken_injection_exports_a_readable_prompt_file() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox.configured_command()?.arg("polytoken").output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("ARGS=new|\n"));
    let prompt_path = stdout
        .lines()
        .find_map(|line| line.strip_prefix("CLANKER_PROMPT_FILE="))
        .ok_or("missing CLANKER_PROMPT_FILE")?;
    let prompt = fs::read_to_string(prompt_path)?;
    assert!(prompt.contains("GPT BASE\n"), "unexpected prompt: {prompt}");
    assert!(prompt.contains("ENG\n"), "unexpected prompt: {prompt}");
    Ok(())
}

#[test]
fn injected_prompt_carries_no_interactive_framing() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox.configured_command()?.arg("polytoken").output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let prompt_path = stdout(&output)
        .lines()
        .find_map(|line| line.strip_prefix("CLANKER_PROMPT_FILE="))
        .map(str::to_owned)
        .ok_or("missing CLANKER_PROMPT_FILE")?;
    let prompt = fs::read_to_string(prompt_path)?;

    // A launcher stamps this text into a harness system-prompt site, where the
    // CLI's framing does not survive contact: the handshake cannot be answered,
    // the standing prohibition contradicts the user's first message, the closing
    // instruction asks for a file the harness already loads, and the date stamp
    // busts the prompt cache. Bare framing drops all four.
    assert!(
        !prompt.contains("respond with 'Got it'"),
        "prompt carries the pre-prompt handshake: {prompt}"
    );
    assert!(
        !prompt.contains("not to do anything to the contents of this directory"),
        "prompt carries the standing prohibition: {prompt}"
    );
    assert!(
        !prompt.contains("read the @AGENTS.md and @CLAUDE.md files"),
        "prompt carries the post-prompt: {prompt}"
    );
    assert!(
        !prompt.contains("Today is"),
        "prompt carries the date stamp: {prompt}"
    );
    assert!(
        prompt.starts_with("GPT BASE\n"),
        "prompt does not begin with its first fragment: {prompt}"
    );
    Ok(())
}

#[test]
fn unknown_context_fails_closed() -> TestResult {
    let sandbox = Sandbox::new()?;
    let symlink_mode = sandbox
        .bare_launch("claude-launch")?
        .env("CLANKER_CONTEXT", "staging")
        .output()?;
    assert!(!symlink_mode.status.success());
    let error = stderr(&symlink_mode);
    assert!(error.contains("unknown context `staging`"));
    assert!(error.contains("CLANKER_CONTEXT"));

    let command_mode = sandbox
        .configured_command()?
        .args(["claude", "--context", "staging", "--dry-run"])
        .output()?;
    assert!(!command_mode.status.success());
    assert!(stderr(&command_mode).contains("unknown context `staging`"));
    Ok(())
}

#[test]
fn shim_path_is_first_and_arguments_are_verbatim() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox
        .launch("claude-launch")?
        .args(["--version", "two words"])
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains(&format!(
        "PATH={}/.local/clankers/bin:",
        sandbox.home.display()
    )));
    assert!(stdout.contains("ARGS=--version|two words|\n"));
    Ok(())
}

#[test]
fn doctor_accepts_valid_and_rejects_semantically_corrupt_config() -> TestResult {
    let sandbox = Sandbox::new()?;
    let fixture = fixture_config();
    let fixture = fixture.to_str().ok_or("non-utf8 fixture path")?;
    let valid = sandbox
        .command()?
        .args(["--config", fixture, "doctor"])
        .output()?;
    assert!(valid.status.success(), "stdout: {}", stdout(&valid));

    let corrupt = sandbox.workdir.join("corrupt.toml");
    fs::write(
        &corrupt,
        r#"
[defaults]
domain = "missing"
contexts = ["personal"]
shim_path = ""
sandbox_wrapper = ""
prompter_bundle = ""
prompt_cache = ""
prompt_cache_ttl_seconds = 86400

[harness.claude]
bin = ""
family = "claude"
default_args = []
[harness.claude.injection]
kind = "arg-text"
args = []

[model]

[domain.eng]
profiles = []
env = { PATH = "/tmp" }
"#,
    )?;
    let corrupt_arg = corrupt.to_str().ok_or("non-utf8 corrupt path")?;
    let invalid = sandbox
        .command()?
        .args(["--config", corrupt_arg, "doctor"])
        .output()?;
    assert!(!invalid.status.success());
    let output = stdout(&invalid);
    for key in [
        "defaults.shim_path",
        "defaults.sandbox_wrapper",
        "defaults.prompter_bundle",
        "defaults.prompt_cache",
        "defaults.domain",
        "harness.claude.bin",
        "harness.claude.injection.args",
        "domain.eng.profiles",
        "domain.eng.env.PATH",
    ] {
        assert!(output.contains(key), "missing {key} in: {output}");
    }
    assert!(output.contains("computed by clanker"));
    Ok(())
}

#[test]
fn configless_commands_fail_closed_but_help_and_version_work() -> TestResult {
    let sandbox = Sandbox::new()?;
    let missing = sandbox.command()?.arg("list").output()?;
    assert!(!missing.status.success());
    let missing_error = stderr(&missing);
    assert!(missing_error.contains(".config/clanker/config.toml"));
    assert!(missing_error.contains("set CLANKER_CONFIG"));

    let help = sandbox.command()?.arg("--help").output()?;
    assert!(help.status.success(), "stderr: {}", stderr(&help));
    let version = sandbox.command()?.arg("--version").output()?;
    assert!(version.status.success(), "stderr: {}", stderr(&version));
    Ok(())
}

#[test]
fn current_is_configless_and_reports_inactive_without_launch_markers() -> TestResult {
    let sandbox = Sandbox::new()?;

    let text = sandbox.bare_command()?.arg("current").output()?;
    assert!(text.status.success(), "stderr: {}", stderr(&text));
    assert_eq!(stdout(&text), "active=false\n");

    let json = sandbox
        .bare_command()?
        .args(["--json", "current"])
        .output()?;
    assert!(json.status.success(), "stderr: {}", stderr(&json));
    let value: serde_json::Value = serde_json::from_slice(&json.stdout)?;
    assert_eq!(
        json_field(&value, "/command")?,
        &serde_json::json!("current")
    );
    assert_eq!(
        json_field(&value, "/data/active")?,
        &serde_json::json!(false)
    );
    assert!(json_field(&value, "/data/context")?.is_null());
    Ok(())
}

#[test]
fn current_reports_inherited_launch_markers_without_recomputing() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox
        .bare_command()?
        .env("CLANKER_SESSION", "1")
        .env("CLANKER_SESSION_VERSION", "5")
        .env("CLANKER_SESSION_ID", TEST_SESSION_ID)
        .env("CLANKER_SESSION_INVOCATION", "clanker claude")
        .env("CLANKER_SESSION_HARNESS", "claude")
        .env("CLANKER_SESSION_CONTEXT", "work")
        .env("CLANKER_SESSION_CONTEXT_SOURCE", ".clanker")
        .env("CLANKER_SESSION_DOMAINS", "eng,research")
        .env("CLANKER_SESSION_DOMAIN_SOURCE", "--domain")
        .env("CLANKER_SESSION_MODEL", "claude-alt")
        .env("CLANKER_SESSION_MODEL_SOURCE", "CLANKER_MODEL")
        .env("CLANKER_SESSION_FAMILY", "gpt")
        .env("CLANKER_CONTEXT", "personal")
        .args(["--json", "current"])
        .output()?;

    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        json_field(&value, "/data/active")?,
        &serde_json::json!(true)
    );
    assert_eq!(
        json_field(&value, "/data/id")?,
        &serde_json::json!(TEST_SESSION_ID)
    );
    assert_eq!(
        json_field(&value, "/data/context")?,
        &serde_json::json!("work")
    );
    assert_eq!(
        json_field(&value, "/data/context_source")?,
        &serde_json::json!(".clanker")
    );
    assert_eq!(
        json_field(&value, "/data/domains")?,
        &serde_json::json!(["eng", "research"])
    );
    assert_eq!(
        json_field(&value, "/data/domain_source")?,
        &serde_json::json!("--domain")
    );
    assert_eq!(
        json_field(&value, "/data/model_source")?,
        &serde_json::json!("CLANKER_MODEL")
    );
    Ok(())
}

#[test]
fn a_superseded_marker_version_is_rejected() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox
        .bare_command()?
        .env("CLANKER_SESSION", "1")
        .env("CLANKER_SESSION_VERSION", "2")
        .arg("current")
        .output()?;

    assert!(!output.status.success());
    assert!(stderr(&output).contains("unsupported clanker session marker version `2`"));
    Ok(())
}

#[test]
fn launch_modes_export_complete_current_session_markers() -> TestResult {
    let sandbox = Sandbox::new()?;

    let symlink = sandbox
        .bare_launch("claude-launch")?
        .env("CLANKER_CONTEXT", "work")
        .env("CLANKER_SESSION_DOMAINS", "stale-domain")
        .env("CLANKER_SESSION_MODEL", "stale-model")
        .output()?;
    assert!(symlink.status.success(), "stderr: {}", stderr(&symlink));
    let symlink = stdout(&symlink);
    for expected in [
        "CLANKER_SESSION=1\n",
        "CLANKER_SESSION_VERSION=5\n",
        "CLANKER_SESSION_INVOCATION=claude-launch\n",
        "CLANKER_SESSION_HARNESS=claude\n",
        "CLANKER_SESSION_CONTEXT=work\n",
        "CLANKER_SESSION_CONTEXT_SOURCE=CLANKER_CONTEXT\n",
        "CLANKER_SESSION_DOMAINS=\n",
        "CLANKER_SESSION_DOMAIN_SOURCE=\n",
        "CLANKER_SESSION_MODEL=\n",
        "CLANKER_SESSION_MODEL_SOURCE=\n",
        "CLANKER_SESSION_FAMILY=\n",
        "CLANKER_SESSION_PROJECT=\n",
        "CLANKER_SESSION_PROJECT_SOURCE=\n",
        "CLANKER_SESSION_REMOTE=\n",
        "MNENE_AGENT=claude\n",
        "MNENE_CONTEXT=work\n",
        "MNENE_SESSION=",
        "MNENE_SCOPE=\n",
    ] {
        assert!(
            symlink.contains(expected),
            "missing {expected:?} in: {symlink}"
        );
    }
    assert_minted_session_id(&symlink);

    fs::write(
        sandbox.workdir.join(".clanker"),
        "context = \"work\"\ndomain = \"eng\"\n",
    )?;
    let command = sandbox
        .configured_command()?
        .args(["claude", "--no-prompt"])
        .output()?;
    assert!(command.status.success(), "stderr: {}", stderr(&command));
    let command = stdout(&command);
    for expected in [
        "CLANKER_SESSION=1\n",
        "CLANKER_SESSION_VERSION=5\n",
        "CLANKER_SESSION_INVOCATION=clanker claude\n",
        "CLANKER_SESSION_HARNESS=claude\n",
        "CLANKER_SESSION_CONTEXT=work\n",
        "CLANKER_SESSION_CONTEXT_SOURCE=.clanker\n",
        "CLANKER_SESSION_DOMAINS=eng\n",
        "CLANKER_SESSION_DOMAIN_SOURCE=.clanker\n",
        "CLANKER_SESSION_MODEL=\n",
        "CLANKER_SESSION_FAMILY=claude\n",
        "CLANKER_SESSION_PROJECT=\n",
        "CLANKER_SESSION_PROJECT_SOURCE=\n",
        "CLANKER_SESSION_REMOTE=\n",
        "MNENE_AGENT=claude\n",
        "MNENE_CONTEXT=work\n",
        "MNENE_SESSION=",
        "MNENE_SCOPE=\n",
    ] {
        assert!(
            command.contains(expected),
            "missing {expected:?} in: {command}"
        );
    }
    assert_minted_session_id(&command);
    Ok(())
}

/// Assert the launched process received a non-empty minted identifier.
///
/// The value is minted per launch, so a test can assert its presence and
/// shape but never its content.
fn assert_minted_session_id(rendered: &str) {
    let identifier = rendered
        .lines()
        .find_map(|line| line.strip_prefix("CLANKER_SESSION_ID="))
        .unwrap_or_default();
    assert_eq!(
        identifier.len(),
        36,
        "expected a minted identifier in: {rendered}"
    );
}

#[test]
fn a_launch_from_a_registered_remote_resolves_the_project_and_exports_its_markers() -> TestResult {
    let sandbox = Sandbox::new()?;
    write_git_repo(&sandbox.workdir, "git@github.com:tftio/kb.git")?;
    write_project_registry(
        &sandbox.home,
        "[project.kb]\nremotes = [\"github.com/tftio/kb\"]\n",
    )?;

    let output = sandbox.launch("claude-launch")?.output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let rendered = stdout(&output);

    for expected in [
        "CLANKER_SESSION_PROJECT=kb\n",
        "CLANKER_SESSION_PROJECT_SOURCE=remote\n",
        "CLANKER_SESSION_REMOTE=github.com/tftio/kb\n",
    ] {
        assert!(
            rendered.contains(expected),
            "missing {expected:?} in: {rendered}"
        );
    }
    Ok(())
}

#[test]
fn a_launch_with_no_repository_and_no_registered_path_exports_empty_project_markers() -> TestResult
{
    let sandbox = Sandbox::new()?;

    let output = sandbox.launch("claude-launch")?.output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let rendered = stdout(&output);

    for expected in [
        "CLANKER_SESSION_PROJECT=\n",
        "CLANKER_SESSION_PROJECT_SOURCE=\n",
        "CLANKER_SESSION_REMOTE=\n",
    ] {
        assert!(
            rendered.contains(expected),
            "missing {expected:?} in: {rendered}"
        );
    }
    Ok(())
}

#[test]
fn a_launch_from_a_registered_non_repository_path_resolves_by_path() -> TestResult {
    let sandbox = Sandbox::new()?;
    // Registered against the canonicalized form, matching what clanker
    // itself resolves the working directory to (`main.rs` canonicalizes
    // the working directory before matching registry paths), so this
    // holds even where the sandbox's temp root is itself a symlink (as
    // `/var` is to `/private/var` on macOS).
    let canonical_workdir = sandbox.workdir.canonicalize()?;
    write_project_registry(
        &sandbox.home,
        &format!(
            "[project.notes]\npaths = [\"{}\"]\n",
            canonical_workdir.display()
        ),
    )?;

    let output = sandbox.launch("claude-launch")?.output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let rendered = stdout(&output);

    for expected in [
        "CLANKER_SESSION_PROJECT=notes\n",
        "CLANKER_SESSION_PROJECT_SOURCE=path\n",
        "CLANKER_SESSION_REMOTE=\n",
    ] {
        assert!(
            rendered.contains(expected),
            "missing {expected:?} in: {rendered}"
        );
    }
    Ok(())
}

#[test]
fn a_declared_project_wins_over_a_disagreeing_registered_remote_and_doctor_reports_it() -> TestResult
{
    let sandbox = Sandbox::new()?;
    write_git_repo(&sandbox.workdir, "git@github.com:tftio/kb.git")?;
    fs::write(
        sandbox.workdir.join(".clanker"),
        "project = \"kb-declared\"\n",
    )?;
    write_project_registry(
        &sandbox.home,
        "[project.kb-registered]\nremotes = [\"github.com/tftio/kb\"]\n",
    )?;

    let output = sandbox.launch("claude-launch")?.output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let rendered = stdout(&output);
    for expected in [
        "CLANKER_SESSION_PROJECT=kb-declared\n",
        "CLANKER_SESSION_PROJECT_SOURCE=declared\n",
        "CLANKER_SESSION_REMOTE=github.com/tftio/kb\n",
    ] {
        assert!(
            rendered.contains(expected),
            "missing {expected:?} in: {rendered}"
        );
    }

    let doctor = sandbox
        .configured_command()?
        .current_dir(&sandbox.workdir)
        .arg("doctor")
        .output()?;
    assert!(
        !doctor.status.success(),
        "doctor should fail on disagreement"
    );
    let doctor_text = format!("{}{}", stdout(&doctor), stderr(&doctor));
    assert!(doctor_text.contains("kb-declared"));
    assert!(doctor_text.contains("kb-registered"));
    Ok(())
}

#[test]
fn a_malformed_registry_warns_but_still_launches() -> TestResult {
    let sandbox = Sandbox::new()?;
    write_project_registry(&sandbox.home, "not valid toml =")?;

    let output = sandbox.launch("claude-launch")?.output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stderr(&output).contains("warning: project registry"));
    let rendered = stdout(&output);
    assert!(rendered.contains("CLANKER_SESSION_PROJECT=\n"));
    Ok(())
}

#[test]
fn an_unnormalizable_origin_remote_warns_but_still_launches() -> TestResult {
    // The declaration file itself is read identically by clanker's own
    // strict `.clanker` parser (which would already fail closed on a badly
    // formed `project` key, exercised elsewhere), so the tolerant
    // "discovery error never blocks a launch" path is exercised here
    // through a malformed *origin remote* instead: a `[remote "origin"]`
    // URL that cannot be normalized (empty).
    let sandbox = Sandbox::new()?;
    write_git_repo(&sandbox.workdir, "")?;

    let output = sandbox.launch("claude-launch")?.output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stderr(&output).contains("warning: project discovery"));
    let rendered = stdout(&output);
    assert!(rendered.contains("CLANKER_SESSION_PROJECT=\n"));
    Ok(())
}

#[test]
fn clanker_project_resolve_prints_a_tab_separated_line() -> TestResult {
    let sandbox = Sandbox::new()?;
    write_git_repo(&sandbox.workdir, "git@github.com:tftio/kb.git")?;
    write_project_registry(
        &sandbox.home,
        "[project.kb]\nremotes = [\"github.com/tftio/kb\"]\n",
    )?;

    let output = sandbox
        .bare_command()?
        .args(["project", "resolve", "--dir"])
        .arg(&sandbox.workdir)
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "kb\tremote\tgithub.com/tftio/kb\n");
    Ok(())
}

#[test]
fn clanker_project_resolve_without_dir_uses_the_current_directory() -> TestResult {
    let sandbox = Sandbox::new()?;
    write_git_repo(&sandbox.workdir, "git@github.com:tftio/kb.git")?;
    write_project_registry(
        &sandbox.home,
        "[project.kb]\nremotes = [\"github.com/tftio/kb\"]\n",
    )?;

    let output = sandbox
        .bare_command()?
        .args(["project", "resolve"])
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "kb\tremote\tgithub.com/tftio/kb\n");
    Ok(())
}

#[test]
fn clanker_project_resolve_json_carries_the_resolution() -> TestResult {
    let sandbox = Sandbox::new()?;
    write_git_repo(&sandbox.workdir, "git@github.com:tftio/kb.git")?;
    write_project_registry(
        &sandbox.home,
        "[project.kb]\nremotes = [\"github.com/tftio/kb\"]\n",
    )?;

    let output = sandbox
        .bare_command()?
        .args(["--json", "project", "resolve"])
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        json_field(&value, "/command")?,
        &serde_json::json!("project resolve")
    );
    assert_eq!(
        json_field(&value, "/data/project")?,
        &serde_json::json!("kb")
    );
    assert_eq!(
        json_field(&value, "/data/source")?,
        &serde_json::json!("remote")
    );
    assert_eq!(
        json_field(&value, "/data/remote")?,
        &serde_json::json!("github.com/tftio/kb")
    );
    Ok(())
}

#[test]
fn clanker_project_resolve_exits_zero_and_empty_when_unresolved() -> TestResult {
    let sandbox = Sandbox::new()?;

    let output = sandbox
        .bare_command()?
        .args(["project", "resolve", "--dir"])
        .arg(&sandbox.workdir)
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "\t\t\n");
    Ok(())
}

#[test]
fn clanker_project_list_reports_registered_projects() -> TestResult {
    let sandbox = Sandbox::new()?;
    write_project_registry(
        &sandbox.home,
        "[project.kb]\nremotes = [\"github.com/tftio/kb\"]\npaths = [\"/repos/kb\"]\n",
    )?;

    let output = sandbox.bare_command()?.args(["project", "list"]).output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let rendered = stdout(&output);
    assert!(rendered.contains("kb\t"));
    assert!(rendered.contains("github.com/tftio/kb"));
    assert!(rendered.contains("/repos/kb"));

    let json = sandbox
        .bare_command()?
        .args(["--json", "project", "list"])
        .output()?;
    assert!(json.status.success(), "stderr: {}", stderr(&json));
    let value: serde_json::Value = serde_json::from_slice(&json.stdout)?;
    assert_eq!(
        json_field(&value, "/command")?,
        &serde_json::json!("project list")
    );
    assert!(json_field(&value, "/data")?.is_array());
    Ok(())
}

#[test]
fn command_mode_dry_run_resolves_without_exec() -> TestResult {
    let sandbox = Sandbox::new()?;
    let fixture = fixture_config();
    let fixture = fixture.to_str().ok_or("non-utf8 fixture path")?;
    let output = sandbox
        .command()?
        .args(["--config", fixture, "claude", "--dry-run", "--version"])
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("harness=claude"));
    assert!(stdout.contains("arg=--version"));
    assert!(stdout.contains("env.PATH="));
    assert!(stdout.contains("prompt<<CLANKER_PROMPT"));
    assert!(stdout.contains("BASE\n"));
    assert!(stdout.contains("ENG\n"));
    Ok(())
}

#[test]
fn dry_run_prints_the_resolved_project_and_remote_when_one_resolves() -> TestResult {
    let sandbox = Sandbox::new()?;
    let canonical_workdir = sandbox.workdir.canonicalize()?;
    write_project_registry(
        &sandbox.home,
        &format!(
            "[project.notes]\npaths = [\"{}\"]\n",
            canonical_workdir.display()
        ),
    )?;
    let output = sandbox
        .configured_command()?
        .args(["claude", "--no-prompt", "--dry-run"])
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("project=notes\n"));
    assert!(stdout.contains("project_source=path\n"));
    assert!(
        !stdout.contains("\nremote="),
        "no origin remote resolved for a path-only match"
    );
    Ok(())
}

#[test]
fn dry_run_prints_the_normalized_remote_when_the_project_resolves_by_remote() -> TestResult {
    let sandbox = Sandbox::new()?;
    write_git_repo(&sandbox.workdir, "git@github.com:tftio/kb.git")?;
    write_project_registry(
        &sandbox.home,
        "[project.kb]\nremotes = [\"github.com/tftio/kb\"]\n",
    )?;
    let output = sandbox
        .configured_command()?
        .args(["claude", "--no-prompt", "--dry-run"])
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("project=kb\n"));
    assert!(stdout.contains("project_source=remote\n"));
    assert!(stdout.contains("remote=github.com/tftio/kb\n"));
    Ok(())
}

#[test]
fn dry_run_attributes_every_variable_to_its_source() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox
        .configured_command()?
        .args([
            "claude",
            "--domain",
            "admin",
            "--model",
            "claude-alt",
            "--no-prompt",
            "--dry-run",
        ])
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let stdout = stdout(&output);

    for expected in [
        "env.PATH=",
        "env.CONTEXT=personal (clanker)",
        "env.CLANKER_CONTEXT=personal (clanker)",
        "env.PLAYWRIGHT_MCP_BROWSER=chromium (clanker)",
        "env.CLANKER_TEST_DOMAIN=admin (domain)",
        "env.ANTHROPIC_MODEL=alt-model (model)",
        "env.CLANKER_SESSION=1 (session)",
    ] {
        assert!(stdout.contains(expected), "missing {expected} in: {stdout}");
    }
    assert!(stdout.contains("(clanker)"));

    let json = sandbox
        .configured_command()?
        .args([
            "--json",
            "claude",
            "--model",
            "claude-alt",
            "--no-prompt",
            "--dry-run",
        ])
        .output()?;
    assert!(json.status.success(), "stderr: {}", stderr(&json));
    let value: serde_json::Value = serde_json::from_slice(&json.stdout)?;
    assert_eq!(
        json_field(&value, "/data/environment/ANTHROPIC_MODEL/provenance")?,
        &serde_json::json!("model")
    );
    assert_eq!(
        json_field(&value, "/data/environment/CONTEXT/provenance")?,
        &serde_json::json!("clanker")
    );
    Ok(())
}

#[test]
fn gemini_injection_variable_is_attributed_to_injection() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox
        .configured_command()?
        .args(["--json", "gemini", "--dry-run"])
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        json_field(&value, "/data/environment/GEMINI_SYSTEM_MD/provenance")?,
        &serde_json::json!("injection")
    );
    Ok(())
}

#[test]
fn domain_precedence_is_explicit_then_file_then_environment_then_configured() -> TestResult {
    let sandbox = Sandbox::new()?;

    let configured = sandbox
        .configured_command()?
        .args(["claude", "--dry-run"])
        .output()?;
    assert!(
        configured.status.success(),
        "stderr: {}",
        stderr(&configured)
    );
    let configured = stdout(&configured);
    assert!(configured.contains("domain=eng\n"));
    assert!(configured.contains("domain_source=defaults.domain\n"));

    let environment = sandbox
        .configured_command()?
        .env("CLANKER_DOMAIN", "research")
        .args(["codex", "--dry-run"])
        .output()?;
    let environment = stdout(&environment);
    assert!(environment.contains("domain=research\n"));
    assert!(environment.contains("domain_source=CLANKER_DOMAIN\n"));
    assert!(environment.contains("model=codex-native\n"));
    assert!(environment.contains("model_source=domain.default_model\n"));

    fs::write(
        sandbox.workdir.join(".clanker"),
        "context = \"personal\"\ndomain = \"eng\"\n",
    )?;
    let directory = sandbox
        .configured_command()?
        .env("CLANKER_DOMAIN", "research")
        .args(["claude", "--dry-run"])
        .output()?;
    assert!(stdout(&directory).contains("domain=eng\n"));

    let command_line = sandbox
        .configured_command()?
        .env("CLANKER_DOMAIN", "research")
        .args([
            "claude",
            "--domain",
            "eng",
            "--context",
            "work",
            "--dry-run",
        ])
        .output()?;
    let output = stdout(&command_line);
    assert!(output.contains("domain=eng\n"));
    assert!(output.contains("domain_source=--domain\n"));
    assert!(output.contains("context=work\n"));
    assert!(output.contains("context_source=--context\n"));
    Ok(())
}

#[test]
fn repeated_domains_stack_prompts_in_order_and_resolve_conflicts_first_wins() -> TestResult {
    let sandbox = Sandbox::new()?;
    let stacked = sandbox
        .configured_command()?
        .args([
            "codex",
            "--domain",
            "eng",
            "--domain",
            "research",
            "--dry-run",
        ])
        .output()?;
    assert!(stacked.status.success(), "stderr: {}", stderr(&stacked));
    let stacked = stdout(&stacked);
    let engineering = stacked.find("ENG\n").ok_or("missing ENG marker")?;
    let research = stacked
        .find("RESEARCH\n")
        .ok_or("missing RESEARCH marker")?;
    assert!(engineering < research);
    assert_eq!(stacked.matches("GPT BASE\n").count(), 1);
    assert!(stacked.contains("domain=eng\ndomain=research\n"));
    assert!(stacked.contains("env.CLANKER_TEST_DOMAIN=engineering (domain)"));
    assert!(stacked.contains("env.CLANKER_SESSION_DOMAINS=eng,research (session)"));

    let json = sandbox
        .configured_command()?
        .args([
            "--json",
            "codex",
            "--domain",
            "eng",
            "--domain",
            "research",
            "--no-prompt",
            "--dry-run",
        ])
        .output()?;
    assert!(json.status.success(), "stderr: {}", stderr(&json));
    let json: serde_json::Value = serde_json::from_slice(&json.stdout)?;
    assert_eq!(
        json_field(&json, "/data/domains")?,
        &serde_json::json!(["eng", "research"])
    );

    let research_first = sandbox
        .configured_command()?
        .args([
            "codex",
            "--domain",
            "research",
            "--domain",
            "admin",
            "--no-prompt",
            "--dry-run",
        ])
        .output()?;
    assert!(
        research_first.status.success(),
        "stderr: {}",
        stderr(&research_first)
    );
    let research_first = stdout(&research_first);
    assert!(research_first.contains("model=codex-native\n"));
    assert!(research_first.contains("env.CLANKER_TEST_DOMAIN=research (domain)"));

    let admin_first = sandbox
        .configured_command()?
        .args([
            "claude",
            "--domain",
            "admin",
            "--domain",
            "research",
            "--no-prompt",
            "--dry-run",
        ])
        .output()?;
    assert!(
        admin_first.status.success(),
        "stderr: {}",
        stderr(&admin_first)
    );
    let admin_first = stdout(&admin_first);
    assert!(admin_first.contains("model=claude-alt\n"));
    assert!(admin_first.contains("env.CLANKER_TEST_DOMAIN=admin (domain)"));
    Ok(())
}

#[test]
fn unknown_directory_parameter_is_a_hard_failure() -> TestResult {
    let sandbox = Sandbox::new()?;
    fs::write(
        sandbox.workdir.join(".clanker"),
        "context = \"personal\"\nunknown_parameter = true\n",
    )?;

    let output = sandbox
        .configured_command()?
        .args(["claude", "--dry-run"])
        .output()?;

    assert!(!output.status.success());
    assert!(stderr(&output).contains("unknown field `unknown_parameter`"));
    Ok(())
}

#[test]
fn model_precedence_is_explicit_then_file_then_environment_then_domain_default() -> TestResult {
    let sandbox = Sandbox::new()?;
    let domain_default = sandbox
        .configured_command()?
        .args(["codex", "--domain", "research", "--no-prompt", "--dry-run"])
        .output()?;
    assert!(stdout(&domain_default).contains("model=codex-native\n"));

    let environment = sandbox
        .configured_command()?
        .env("CLANKER_MODEL", "claude-alt")
        .args(["claude", "--no-prompt", "--dry-run"])
        .output()?;
    let environment = stdout(&environment);
    assert!(environment.contains("model=claude-alt\n"));
    assert!(environment.contains("model_source=CLANKER_MODEL\n"));

    fs::write(
        sandbox.workdir.join(".clanker"),
        "model = \"codex-native\"\n",
    )?;
    let directory = sandbox
        .configured_command()?
        .env("CLANKER_MODEL", "claude-alt")
        .args(["codex", "--no-prompt", "--dry-run"])
        .output()?;
    let directory = stdout(&directory);
    assert!(directory.contains("model=codex-native\n"));
    assert!(directory.contains("model_source=.clanker\n"));

    let command_line = sandbox
        .configured_command()?
        .env("CLANKER_MODEL", "codex-native")
        .args([
            "claude",
            "--model",
            "claude-alt",
            "--no-prompt",
            "--dry-run",
        ])
        .output()?;
    let command_line = stdout(&command_line);
    assert!(command_line.contains("model=claude-alt\n"));
    assert!(command_line.contains("model_source=--model\n"));
    Ok(())
}

#[test]
fn explicit_model_mismatch_fails_closed() -> TestResult {
    let sandbox = Sandbox::new()?;
    let mismatch = sandbox
        .configured_command()?
        .args(["codex", "--model", "claude-alt", "--dry-run"])
        .output()?;
    assert!(!mismatch.status.success());
    assert!(stderr(&mismatch).contains("requires harness `claude`, not `codex`"));

    let domain_default_mismatch = sandbox
        .configured_command()?
        .args(["claude", "--domain", "research", "--no-prompt", "--dry-run"])
        .output()?;
    assert!(!domain_default_mismatch.status.success());
    assert!(stderr(&domain_default_mismatch).contains("requires harness `codex`, not `claude`"));
    Ok(())
}

#[test]
fn model_environment_reaches_a_live_launch() -> TestResult {
    let sandbox = Sandbox::new()?;
    let launched = sandbox
        .configured_command()?
        .args([
            "claude",
            "--model",
            "claude-alt",
            "--no-prompt",
            "--",
            "hello",
        ])
        .output()?;
    assert!(launched.status.success(), "stderr: {}", stderr(&launched));
    let output = stdout(&launched);
    for expected in [
        "ANTHROPIC_BASE_URL=https://example.invalid/anthropic",
        "ANTHROPIC_MODEL=alt-model",
        "ARGS=--dangerously-skip-permissions|--model|alt|hello|",
    ] {
        assert!(output.contains(expected), "missing {expected} in: {output}");
    }
    Ok(())
}

#[test]
fn command_mode_orders_default_args_before_model_args_and_passthrough() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox
        .configured_command()?
        .args([
            "codex",
            "--model",
            "codex-native",
            "--no-prompt",
            "--",
            "passthrough-arg",
        ])
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(
        stdout(&output).contains("ARGS=-s|danger-full-access|--model|gpt-test|passthrough-arg|\n")
    );
    Ok(())
}

#[test]
fn no_default_args_suppresses_only_harness_defaults() -> TestResult {
    let sandbox = Sandbox::new()?;
    let with_defaults = sandbox
        .configured_command()?
        .args(["claude", "--no-prompt", "--dry-run"])
        .output()?;
    assert!(
        with_defaults.status.success(),
        "stderr: {}",
        stderr(&with_defaults)
    );
    assert!(stdout(&with_defaults).contains("arg=--dangerously-skip-permissions"));

    let suppressed = sandbox
        .configured_command()?
        .args(["claude", "--no-default-args", "--dry-run"])
        .output()?;
    assert!(
        suppressed.status.success(),
        "stderr: {}",
        stderr(&suppressed)
    );
    let suppressed = stdout(&suppressed);
    assert!(!suppressed.contains("--dangerously-skip-permissions"));
    assert!(suppressed.contains("arg=--append-system-prompt"));
    Ok(())
}

#[test]
fn all_prompt_injection_modes_reach_live_stubs() -> TestResult {
    let sandbox = Sandbox::new()?;

    let claude = sandbox.configured_command()?.arg("claude").output()?;
    assert!(claude.status.success(), "stderr: {}", stderr(&claude));
    let claude_output = stdout(&claude);
    assert!(claude_output.contains("ARGS=--dangerously-skip-permissions|--append-system-prompt|"));
    assert!(claude_output.contains("BASE\n"));
    assert!(claude_output.contains("ENG\n"));

    let codex = sandbox.configured_command()?.arg("codex").output()?;
    assert!(codex.status.success(), "stderr: {}", stderr(&codex));
    assert!(stdout(&codex).contains("model_instructions_file="));

    let gemini = sandbox.configured_command()?.arg("gemini").output()?;
    assert!(gemini.status.success(), "stderr: {}", stderr(&gemini));
    assert!(stdout(&gemini).contains("GEMINI_SYSTEM_MD="));

    let opencode = sandbox.configured_command()?.arg("opencode").output()?;
    assert!(opencode.status.success(), "stderr: {}", stderr(&opencode));
    let opencode_output = stdout(&opencode);
    let inline_config = opencode_output
        .lines()
        .find_map(|line| line.strip_prefix("OPENCODE_CONFIG_CONTENT="))
        .ok_or("missing OpenCode inline configuration")?;
    let inline_config: serde_json::Value = serde_json::from_str(inline_config)?;
    let instruction_path = json_field(&inline_config, "/instructions/0")?
        .as_str()
        .ok_or("OpenCode instruction path is not a string")?;
    assert!(fs::read_to_string(instruction_path)?.contains("GPT BASE\n"));
    assert!(opencode_output.contains("ARGS=|\n"));
    assert!(opencode_output.contains(&format!(
        "OPENCODE_CONFIG_DIR={}/.config/opencode/personal\n",
        sandbox.home.display()
    )));

    let cache = sandbox.home.join(".cache/clanker/prompts");
    let mut cached: Vec<String> = Vec::new();
    for entry in fs::read_dir(cache)? {
        cached.push(fs::read_to_string(entry?.path())?);
    }
    assert_eq!(cached.len(), 3);
    assert!(cached.iter().all(|prompt| prompt.contains("ENG\n")));
    Ok(())
}

#[test]
fn domain_environment_sandbox_and_env_command_are_composed() -> TestResult {
    let sandbox = Sandbox::new()?;
    let sandboxed = sandbox
        .configured_command()?
        .args(["codex", "--domain", "research", "--no-prompt", "--sandbox"])
        .output()?;
    assert!(sandboxed.status.success(), "stderr: {}", stderr(&sandboxed));
    let output = stdout(&sandboxed);
    assert!(output.contains("SANDBOX=codex\n"));
    assert!(output.contains("CLANKER_TEST_DOMAIN=research\n"));

    let environment = sandbox
        .configured_command()?
        .args(["env", "claude", "--model", "claude-alt"])
        .output()?;
    assert!(
        environment.status.success(),
        "stderr: {}",
        stderr(&environment)
    );
    let output = stdout(&environment);
    assert!(output.contains("export CONTEXT='personal'"));
    assert!(output.contains("export CLANKER_CONTEXT='personal'"));
    assert!(output.contains("export ANTHROPIC_MODEL='alt-model'"));
    Ok(())
}

#[test]
fn unknown_profiles_fail_closed() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox
        .configured_command()?
        .args([
            "claude",
            "--profile",
            "missing.profile",
            "--profile",
            "extra",
            "--dry-run",
        ])
        .output()?;
    assert!(!output.status.success());
    let error = stderr(&output);
    assert!(error.contains("unknown prompter profile(s): missing.profile"));
    assert!(!error.contains("extra"));
    Ok(())
}

#[test]
fn file_injection_dry_run_does_not_write_cache() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox
        .configured_command()?
        .args(["codex", "--dry-run"])
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("prompt_file="));
    assert!(!sandbox.home.join(".cache/clanker/prompts").exists());
    Ok(())
}

#[test]
fn launch_fails_closed_when_the_configured_binary_is_missing() -> TestResult {
    let sandbox = Sandbox::new()?;
    let config = sandbox.workdir.join("missing-bin.toml");
    fs::write(
        &config,
        r#"
[defaults]
domain = "eng"
contexts = ["personal"]
shim_path = "~/.local/clankers/bin"
sandbox_wrapper = "~/unused"
prompter_bundle = "~/unused"
prompt_cache = "~/unused"
prompt_cache_ttl_seconds = 1

[harness.bogus]
bin = "/nonexistent/clanker-exec-should-fail"
family = "claude"
default_args = []
[harness.bogus.injection]
kind = "arg-text"
args = ["{text}"]

[model]

[domain.eng]
profiles = ["core.base"]
env = {}
"#,
    )?;
    let config_arg = config.to_str().ok_or("non-utf8 config path")?;
    let output = sandbox
        .command()?
        .args([
            "--config",
            config_arg,
            "bogus",
            "--no-prompt",
            "--no-default-args",
            "--",
            "hello",
        ])
        .output()?;
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("failed to exec"),
        "stderr: {}",
        stderr(&output)
    );
    Ok(())
}

#[test]
fn list_reports_configured_names_in_text_and_json() -> TestResult {
    let sandbox = Sandbox::new()?;
    let text = sandbox.configured_command()?.arg("list").output()?;
    assert!(text.status.success(), "stderr: {}", stderr(&text));
    let text = stdout(&text);
    assert!(text.contains("Harnesses:"));
    assert!(text.contains("claude"));
    assert!(text.contains("Models:"));
    assert!(text.contains("claude-alt"));
    assert!(text.contains("Domains:"));
    assert!(text.contains("eng"));

    let json = sandbox
        .configured_command()?
        .args(["--json", "list"])
        .output()?;
    assert!(json.status.success(), "stderr: {}", stderr(&json));
    let value: serde_json::Value = serde_json::from_slice(&json.stdout)?;
    assert_eq!(json_field(&value, "/command")?, &serde_json::json!("list"));
    assert!(json_field(&value, "/data/harnesses")?.is_array());
    Ok(())
}

#[test]
fn current_text_reports_full_inherited_markers() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox
        .bare_command()?
        .env("CLANKER_SESSION", "1")
        .env("CLANKER_SESSION_VERSION", "5")
        .env("CLANKER_SESSION_ID", TEST_SESSION_ID)
        .env("CLANKER_SESSION_INVOCATION", "clanker claude")
        .env("CLANKER_SESSION_HARNESS", "claude")
        .env("CLANKER_SESSION_CONTEXT", "work")
        .env("CLANKER_SESSION_CONTEXT_SOURCE", ".clanker")
        .env("CLANKER_SESSION_DOMAINS", "eng,research")
        .env("CLANKER_SESSION_DOMAIN_SOURCE", ".clanker")
        .env("CLANKER_SESSION_MODEL", "claude-alt")
        .env("CLANKER_SESSION_MODEL_SOURCE", "--model")
        .env("CLANKER_SESSION_FAMILY", "gpt")
        .env("CLANKER_SESSION_PROJECT", "kb")
        .env("CLANKER_SESSION_PROJECT_SOURCE", "declared")
        .env("CLANKER_SESSION_REMOTE", "github.com/tftio/kb")
        .arg("current")
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    for expected in [
        "active=true\n",
        "version=5\n",
        "invocation=clanker claude\n",
        "harness=claude\n",
        "context=work\n",
        "context_source=.clanker\n",
        "domain=eng\n",
        "domain=research\n",
        "domain_source=.clanker\n",
        "model=claude-alt\n",
        "model_source=--model\n",
        "family=gpt\n",
        "project=kb\n",
        "project_source=declared\n",
        "remote=github.com/tftio/kb\n",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in: {text}");
    }
    assert!(
        text.contains(&format!("id={TEST_SESSION_ID}\n")),
        "missing inherited identifier in: {text}"
    );
    Ok(())
}

#[test]
fn env_json_reports_environment_plan() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox
        .configured_command()?
        .args(["--json", "env", "claude", "--model", "claude-alt"])
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(json_field(&value, "/command")?, &serde_json::json!("env"));
    Ok(())
}

#[test]
fn prompt_list_reports_configured_profiles_from_the_default_bundle() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox
        .configured_command()?
        .args(["prompt", "list"])
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("core.base"), "missing core.base in: {text}");
    assert!(text.contains("domain.eng"), "missing domain.eng in: {text}");
    Ok(())
}

#[test]
fn prompt_validate_reports_success_from_the_default_bundle() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox
        .configured_command()?
        .args(["prompt", "validate"])
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("All profiles valid"));
    Ok(())
}

#[test]
fn prompt_run_bare_renders_composed_profiles_from_the_default_bundle() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox
        .configured_command()?
        .args(["prompt", "run", "--bare", "core.base", "domain.eng"])
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert_eq!(text, "BASE\n\nENG\n");
    Ok(())
}

#[test]
fn prompt_run_honors_family_variant_selection() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox
        .configured_command()?
        .args(["prompt", "run", "--bare", "--family", "gpt", "core.base"])
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "GPT BASE\n");
    Ok(())
}

#[test]
fn prompt_system_prepends_the_invariant_base_profile() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox
        .configured_command()?
        .args(["prompt", "system", "--bare", "domain.eng"])
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "BASE\n\nENG\n");
    Ok(())
}

#[test]
fn prompt_tree_shows_dependencies_from_the_default_bundle() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox
        .configured_command()?
        .args(["prompt", "tree"])
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert!(stdout(&output).contains("core.base"));
    Ok(())
}

#[test]
fn prompt_run_honors_an_explicit_config_override() -> TestResult {
    let sandbox = Sandbox::new()?;
    let alternate = sandbox.home.join(".local/prompter/config.toml");
    let output = sandbox
        .bare_command()?
        .args([
            "--config",
            alternate.to_str().ok_or("non-utf8 path")?,
            "prompt",
            "run",
            "--bare",
            "extra",
        ])
        .output()?;
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "EXTRA\n");
    Ok(())
}

#[test]
fn prompt_run_fails_closed_on_an_unknown_profile() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox
        .configured_command()?
        .args(["prompt", "run", "--bare", "no.such.profile"])
        .output()?;
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("no.such.profile"),
        "stderr: {}",
        stderr(&output)
    );
    Ok(())
}
