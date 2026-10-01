//! Pi integration against the shipped config, without provider access.

use std::fs;
use std::os::unix::fs::symlink;
use std::process::Command;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

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
        fs::create_dir_all(temp.path().join(".pi/agent"))?;
        let bundle = temp.path().join(".local/prompter");
        fs::create_dir_all(bundle.join("library"))?;
        fs::write(
            bundle.join("config.toml"),
            "library = \"library\"\n[core.base]\ndepends_on = [\"base.md\"]\n[domain.eng]\ndepends_on = []\n",
        )?;
        fs::write(bundle.join("library/base.md"), "NEUTRAL PI PROMPT\n")?;
        Ok(Self { temp })
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_clanker"));
        self.configure(&mut command);
        command
    }

    fn configure(&self, command: &mut Command) {
        command
            .env_clear()
            .env("HOME", self.temp.path())
            .env("PATH", "/usr/bin:/bin")
            .env("CLANKER_CONFIG", self.temp.path().join("config.toml"))
            .env("CLANKER_CONTEXT", "personal")
            .current_dir(self.temp.path());
    }
}

#[test]
fn personal_pi_uses_existing_root_neutral_prompt_and_preserves_native_model_selection() -> TestResult
{
    let sandbox = Sandbox::new()?;
    let output = sandbox
        .command()
        .args(["--json", "pi", "--dry-run", "--", "--model", "other/model"])
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let plan = plan.get("data").unwrap_or(&plan);
    assert_eq!(plan["harness"], "pi");
    assert_eq!(plan["family"], "deepseek");
    assert_eq!(
        plan.pointer("/environment/PI_CODING_AGENT_DIR/value")
            .and_then(serde_json::Value::as_str)
            .ok_or("no Pi config directory")?,
        sandbox.temp.path().join(".pi/agent").display().to_string()
    );
    assert!(
        plan["prompt"]
            .as_str()
            .ok_or("no prompt")?
            .contains("NEUTRAL PI PROMPT")
    );
    let args = plan["args"].as_array().ok_or("no args")?;
    assert_eq!(
        args.first(),
        Some(&serde_json::json!("--append-system-prompt"))
    );
    assert_eq!(args.last(), Some(&serde_json::json!("other/model")));
    assert_eq!(
        plan.pointer("/environment/CLANKER_SESSION_HARNESS/value")
            .and_then(serde_json::Value::as_str)
            .ok_or("no session harness")?,
        "pi"
    );
    Ok(())
}

#[test]
fn pi_command_and_symlink_reject_work_even_with_existing_personal_root() -> TestResult {
    let sandbox = Sandbox::new()?;
    let output = sandbox
        .command()
        .args(["pi", "--context", "work", "--dry-run"])
        .output()?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not enabled in context `work`"));
    let link = sandbox.temp.path().join("pi-launch");
    symlink(env!("CARGO_BIN_EXE_clanker"), &link)?;
    let mut command = Command::new(link);
    sandbox.configure(&mut command);
    let output = command.env("CLANKER_CONTEXT", "work").output()?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not enabled in context `work`"));
    Ok(())
}

#[test]
fn exec_exports_pi_only_in_personal_and_removes_inherited_personal_root_in_work() -> TestResult {
    let sandbox = Sandbox::new()?;
    for context in ["personal", "work"] {
        let output = sandbox
            .command()
            .env("PI_CODING_AGENT_DIR", "/inherited-personal")
            .args(["exec", "--context", context, "--", "/usr/bin/env"])
            .output()?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8_lossy(&output.stdout);
        assert_eq!(text.contains("PI_CODING_AGENT_DIR="), context == "personal");
        assert!(!text.contains("/inherited-personal"));
    }
    Ok(())
}
