//! Agent-mode surface coverage for clanker.

use std::process::Command;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const TOKEN: &str = "clanker-agent-surface-test";

fn command(args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_clanker"));
    command.args(args);
    command
        .env("TFTIO_AGENT_TOKEN", TOKEN)
        .env("TFTIO_AGENT_TOKEN_EXPECTED", TOKEN);
    command
}

#[test]
fn agent_help_exposes_read_only_listing_and_current_session() -> TestResult {
    let output = command(&["--agent-help"]).output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("list-configuration"));
    assert!(stdout.contains("current-session"));
    assert!(!stdout.contains("doctor"));
    Ok(())
}

#[test]
fn agent_skill_renders_list_contract() -> TestResult {
    let output = command(&["--agent-skill", "list-configuration"]).output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("capability:"));
    assert!(stdout.contains("commands:\n- list"));
    Ok(())
}

#[test]
fn agent_skill_renders_current_session_contract() -> TestResult {
    let output = command(&["--agent-skill", "current-session"]).output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("commands:\n- current"));
    assert!(stdout.contains("flags:\n- --json"));
    assert!(stdout.contains("examples:\n- clanker --json current"));
    assert!(stdout.contains("never recompute"));
    Ok(())
}

#[test]
fn meta_agent_renders_current_session_skill_artifact() -> TestResult {
    let output = Command::new(env!("CARGO_BIN_EXE_clanker"))
        .args([
            "meta",
            "agent",
            "describe",
            "current-session",
            "--format",
            "skill-md",
        ])
        .output()?;
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("---\nname: clanker-current-session\n"));
    assert!(stdout.contains("- `clanker current`"));
    assert!(stdout.contains("- `clanker --json current`"));
    assert!(stdout.contains("active=false"));
    assert!(stdout.contains("never recompute"));
    Ok(())
}
