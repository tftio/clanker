//! Integration coverage for `clanker config dirs`.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

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

[provider.zai]

[model]
[domain.eng]
profiles = ["core.base"]
env = {}
"#;

struct Sandbox {
    temp: TempDir,
    config: std::path::PathBuf,
}

impl Sandbox {
    fn new() -> std::io::Result<Self> {
        let temp = TempDir::new()?;
        let config = temp.path().join("config.toml");
        fs::write(&config, CONFIG)?;
        for directory in [".config/claude/personal", ".config/claude-zai/personal"] {
            fs::create_dir_all(temp.path().join(directory))?;
        }
        Ok(Self { temp, config })
    }

    fn run(&self, arguments: &[&str]) -> std::io::Result<Output> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_clanker"));
        command
            .env_clear()
            .env("HOME", self.temp.path())
            .arg("--config")
            .arg(&self.config)
            .args(arguments)
            .current_dir(self.temp.path());
        forward_coverage_env(&mut command);
        command.output()
    }

    fn path(&self, relative: &str) -> String {
        self.temp.path().join(relative).display().to_string()
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[allow(
    clippy::disallowed_methods,
    reason = "integration tests capture the coverage variable at the subprocess boundary"
)]
fn forward_coverage_env(command: &mut Command) {
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
}

#[test]
fn text_output_lists_native_and_provider_rows_with_existence() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox.run(&["config", "dirs"])?;
    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = String::from_utf8(output.stdout)?;

    for expected in [
        format!(
            "claude\tpersonal\t\tCLAUDE_CONFIG_DIR\t{}\ttrue",
            sandbox.path(".config/claude/personal")
        ),
        format!(
            "claude\twork\t\tCLAUDE_CONFIG_DIR\t{}\tfalse",
            sandbox.path(".config/claude/work")
        ),
        format!(
            "claude\tpersonal\tzai\tCLAUDE_CONFIG_DIR\t{}\ttrue",
            sandbox.path(".config/claude-zai/personal")
        ),
        format!(
            "claude\twork\tzai\tCLAUDE_CONFIG_DIR\t{}\tfalse",
            sandbox.path(".config/claude-zai/work")
        ),
    ] {
        assert!(
            stdout.lines().any(|line| line == expected),
            "missing line {expected:?} in:\n{stdout}"
        );
    }

    let polytoken = stdout
        .lines()
        .find(|line| line.starts_with("polytoken\tpersonal\t\tPOLYTOKEN_DATA_PATH\t"))
        .ok_or("missing polytoken data row")?;
    let fields: Vec<&str> = polytoken.split('\t').collect();
    assert_eq!(fields.len(), 6, "{polytoken:?}");
    assert_eq!(
        fields.get(4).copied(),
        Some(sandbox.path(".local/share/polytoken/personal").as_str())
    );
    Ok(())
}

#[test]
fn json_output_carries_every_field_and_null_provider_for_native_rows() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox.run(&["--json", "config", "dirs"])?;
    assert!(output.status.success(), "{}", stderr(&output));
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let rows = envelope
        .get("data")
        .and_then(serde_json::Value::as_array)
        .ok_or("missing data array")?;

    let native = rows
        .iter()
        .find(|row| {
            row["harness"] == "claude" && row["context"] == "personal" && row["provider"].is_null()
        })
        .ok_or("missing native claude row")?;
    assert_eq!(native["variable"], "CLAUDE_CONFIG_DIR");
    assert_eq!(native["path"], sandbox.path(".config/claude/personal"));
    assert_eq!(native["exists"], true);

    let provider = rows
        .iter()
        .find(|row| row["harness"] == "claude" && row["provider"] == "zai")
        .ok_or("missing provider claude row")?;
    assert_eq!(provider["context"], "personal");
    assert_eq!(
        provider["path"],
        sandbox.path(".config/claude-zai/personal")
    );
    assert_eq!(provider["exists"], true);
    Ok(())
}

#[test]
fn filters_restrict_rows_and_unknown_values_fail() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox.run(&["config", "dirs", "--provider", "zai"])?;
    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = String::from_utf8(output.stdout)?;
    assert!(
        stdout
            .lines()
            .all(|line| line.split('\t').nth(2) == Some("zai"))
    );

    let output = sandbox.run(&["config", "dirs", "--context", "work"])?;
    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = String::from_utf8(output.stdout)?;
    assert!(
        stdout
            .lines()
            .all(|line| line.split('\t').nth(1) == Some("work"))
    );

    let output = sandbox.run(&["config", "dirs", "--provider", "ghost"])?;
    assert!(!output.status.success());
    assert!(stderr(&output).contains("unknown provider `ghost`"));

    let output = sandbox.run(&["config", "dirs", "--context", "staging"])?;
    assert!(!output.status.success());
    assert!(stderr(&output).contains("unknown context `staging`"));
    Ok(())
}

#[test]
fn the_command_creates_nothing() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox.run(&["config", "dirs"])?;
    assert!(output.status.success(), "{}", stderr(&output));
    for missing in [
        ".config/claude/work",
        ".config/claude-zai/work",
        ".config/polytoken/personal",
        ".local/share/polytoken/personal",
    ] {
        assert!(
            !Path::new(&sandbox.temp.path().join(missing)).exists(),
            "{missing} was created"
        );
    }
    Ok(())
}
