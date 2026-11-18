//! Pi integration against the shipped config, without provider access.

use std::fs;
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
        for context in ["personal", "work"] {
            fs::create_dir_all(temp.path().join(".config/pi").join(context))?;
        }
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
fn personal_pi_uses_context_root_neutral_prompt_and_preserves_native_model_selection() -> TestResult
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
        sandbox
            .temp
            .path()
            .join(".config/pi/personal")
            .display()
            .to_string()
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

/// The `PI_CODING_AGENT_DIR` value in a dry-run plan.
fn planned_pi_dir(output: &std::process::Output) -> Result<String, Box<dyn std::error::Error>> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let plan = plan.get("data").unwrap_or(&plan);
    Ok(plan
        .pointer("/environment/PI_CODING_AGENT_DIR/value")
        .and_then(serde_json::Value::as_str)
        .ok_or("no Pi config directory")?
        .to_owned())
}

#[test]
fn pi_uses_a_separate_root_per_context() -> TestResult {
    let sandbox = Sandbox::new()?;
    for context in ["personal", "work"] {
        let output = sandbox
            .command()
            .args(["--json", "pi", "--context", context, "--dry-run"])
            .output()?;
        let expected = sandbox.temp.path().join(".config/pi").join(context);
        assert_eq!(planned_pi_dir(&output)?, expected.display().to_string());
    }
    Ok(())
}

#[test]
fn pi_fails_closed_when_the_context_root_is_missing() -> TestResult {
    let sandbox = Sandbox::new()?;
    fs::remove_dir_all(sandbox.temp.path().join(".config/pi/work"))?;
    let output = sandbox
        .command()
        .args(["pi", "--context", "work", "--dry-run"])
        .output()?;
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("config directory"), "{error}");
    assert!(error.contains(".config/pi/work"), "{error}");
    Ok(())
}

#[test]
fn exec_exports_the_context_pi_root_over_an_inherited_one() -> TestResult {
    let sandbox = Sandbox::new()?;
    for context in ["personal", "work"] {
        let output = sandbox
            .command()
            .env("PI_CODING_AGENT_DIR", "/inherited-root")
            .args(["exec", "--context", context, "--", "/usr/bin/env"])
            .output()?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8_lossy(&output.stdout);
        let expected = sandbox.temp.path().join(".config/pi").join(context);
        assert!(text.contains(&format!("PI_CODING_AGENT_DIR={}\n", expected.display())));
        assert!(!text.contains("/inherited-root"));
    }
    Ok(())
}
