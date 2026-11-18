//! Provider axis against the shipped config, with the provider declared in a
//! machine overlay the way an operator would.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const OVERLAY: &str = r#"
[provider.zai.harness.claude]
env = { ANTHROPIC_BASE_URL = "https://api.z.ai/api/anthropic" }
harness_args = ["--settings", "provider-settings.json"]

[provider.zai.harness.claude.config_dir]
environment = "CLAUDE_CONFIG_DIR"
path = "~/.config/claude-zai/{context}"
"#;

struct Sandbox {
    temp: TempDir,
}

impl Sandbox {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        fs::write(
            temp.path().join("config.toml"),
            include_str!("../config/config.toml"),
        )?;
        fs::write(temp.path().join("local.toml"), OVERLAY)?;
        for directory in [
            ".config/claude/personal",
            ".config/claude/work",
            ".config/claude-zai/personal",
        ] {
            fs::create_dir_all(temp.path().join(directory))?;
        }
        Ok(Self { temp })
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.temp.path().join(relative)
    }

    fn run(&self, arguments: &[&str]) -> std::io::Result<Output> {
        Command::new(env!("CARGO_BIN_EXE_clanker"))
            .env_clear()
            .env("HOME", self.temp.path())
            .env("PATH", "/usr/bin:/bin")
            .env("CLANKER_CONFIG", self.temp.path().join("config.toml"))
            .env("CLANKER_CONTEXT", "personal")
            .current_dir(self.temp.path())
            .args(arguments)
            .output()
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn text<'a>(plan: &'a serde_json::Value, pointer: &str) -> Result<&'a str, String> {
    plan.pointer(pointer)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("missing {pointer}"))
}

#[test]
fn a_provider_dry_run_reports_the_provider_directory_env_and_arguments() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox.run(&[
        "--json",
        "claude",
        "--provider",
        "zai",
        "--no-prompt",
        "--dry-run",
    ])?;
    assert!(output.status.success(), "{}", stderr(&output));
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let plan = plan.get("data").unwrap_or(&plan);

    assert_eq!(plan["provider"], "zai");
    assert_eq!(plan["provider_source"], "--provider");
    assert_eq!(
        text(plan, "/environment/CLAUDE_CONFIG_DIR/value")?,
        sandbox
            .path(".config/claude-zai/personal")
            .display()
            .to_string()
    );
    assert_eq!(
        text(plan, "/environment/ANTHROPIC_BASE_URL/provenance")?,
        "provider"
    );
    assert_eq!(text(plan, "/environment/CLANKER_PROVIDER/value")?, "zai");
    let args = plan["args"].as_array().ok_or("no args")?;
    assert!(
        args.windows(2).any(|pair| pair
            == [
                serde_json::json!("--settings"),
                serde_json::json!("provider-settings.json")
            ]),
        "{args:?}"
    );
    Ok(())
}

#[test]
fn the_same_launch_without_a_provider_keeps_the_native_directory() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox.run(&["--json", "claude", "--no-prompt", "--dry-run"])?;
    assert!(output.status.success(), "{}", stderr(&output));
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let plan = plan.get("data").unwrap_or(&plan);

    assert!(plan["provider"].is_null());
    assert_eq!(
        text(plan, "/environment/CLAUDE_CONFIG_DIR/value")?,
        sandbox
            .path(".config/claude/personal")
            .display()
            .to_string()
    );
    assert!(plan.pointer("/environment/ANTHROPIC_BASE_URL").is_none());
    assert!(plan.pointer("/environment/CLANKER_PROVIDER").is_none());
    Ok(())
}

#[test]
fn the_text_dry_run_names_the_provider_and_its_tier() -> TestResult {
    let sandbox = Sandbox::new()?;
    fs::write(sandbox.path(".clanker"), "provider = \"zai\"\n")?;
    let output = sandbox.run(&["claude", "--no-prompt", "--dry-run"])?;
    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.lines().any(|line| line == "provider=zai"),
        "{stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|line| line == "provider_source=.clanker"),
        "{stdout}"
    );
    Ok(())
}

#[test]
fn a_provider_without_a_directory_for_the_context_fails_closed() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox.run(&[
        "claude",
        "--provider",
        "zai",
        "--context",
        "work",
        "--no-prompt",
        "--dry-run",
    ])?;
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains(
            &sandbox
                .path(".config/claude-zai/work")
                .display()
                .to_string()
        ),
        "{}",
        stderr(&output)
    );
    Ok(())
}

#[test]
fn an_unknown_or_uncovered_provider_is_refused() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox.run(&["claude", "--provider", "ghost", "--dry-run"])?;
    assert!(!output.status.success());
    assert!(stderr(&output).contains("unknown provider `ghost` from --provider"));

    let output = sandbox.run(&["codex", "--provider", "zai", "--dry-run"])?;
    assert!(!output.status.success());
    assert!(stderr(&output).contains("does not cover harness `codex`"));
    Ok(())
}

#[test]
fn exec_with_a_provider_exports_the_provider_directory() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox.run(&["exec", "--provider", "zai", "--", "/usr/bin/env"])?;
    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let expected = format!(
        "CLAUDE_CONFIG_DIR={}",
        sandbox.path(".config/claude-zai/personal").display()
    );
    assert!(stdout.lines().any(|line| line == expected), "{stdout}");
    assert!(
        stdout.lines().any(|line| line == "CLANKER_PROVIDER=zai"),
        "{stdout}"
    );
    Ok(())
}

#[test]
fn env_accepts_the_provider_flag() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox.run(&["env", "claude", "--provider", "zai"])?;
    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let directory = sandbox
        .path(".config/claude-zai/personal")
        .display()
        .to_string();
    assert!(
        stdout
            .lines()
            .any(|line| line.contains("CLAUDE_CONFIG_DIR") && line.contains(&directory)),
        "{stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|line| line.contains("CLANKER_SESSION_PROVIDER=") && line.contains("zai")),
        "{stdout}"
    );
    Ok(())
}

#[test]
fn list_reports_each_provider_and_the_harnesses_it_covers() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox.run(&["list"])?;
    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Providers:\n  zai (claude)\n"), "{stdout}");

    let output = sandbox.run(&["--json", "list"])?;
    assert!(output.status.success(), "{}", stderr(&output));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        value.pointer("/data/providers"),
        Some(&serde_json::json!([{ "name": "zai", "harnesses": ["claude"] }]))
    );
    Ok(())
}

#[test]
fn current_reports_inherited_provider_markers_as_json() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = Command::new(env!("CARGO_BIN_EXE_clanker"))
        .env_clear()
        .env("HOME", sandbox.temp.path())
        .env("PATH", "/usr/bin:/bin")
        .env("CLANKER_SESSION", "1")
        .env("CLANKER_SESSION_VERSION", "6")
        .env("CLANKER_SESSION_ID", "01a06ebb-0000-7000-8000-000000000000")
        .env("CLANKER_SESSION_INVOCATION", "clanker claude")
        .env("CLANKER_SESSION_HARNESS", "claude")
        .env("CLANKER_SESSION_CONTEXT", "personal")
        .env("CLANKER_SESSION_CONTEXT_SOURCE", "CLANKER_CONTEXT")
        .env("CLANKER_SESSION_PROVIDER", "zai")
        .env("CLANKER_SESSION_PROVIDER_SOURCE", "--provider")
        .args(["--json", "current"])
        .output()?;
    assert!(output.status.success(), "{}", stderr(&output));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(text(&value, "/data/provider")?, "zai");
    assert_eq!(text(&value, "/data/provider_source")?, "--provider");
    Ok(())
}
