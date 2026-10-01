//! Health check and diagnostics module.

use super::{Config, PrompterError, load_config_bundle};
use serde_json::json;
use std::path::{Path, PathBuf};
use tftio_lib::{DoctorCheck, DoctorChecks, DoctorReport, JsonOutput, RepoInfo};

/// Doctor checks provider for the prompter tool.
pub struct PrompterDoctor;

impl DoctorChecks for PrompterDoctor {
    fn repo_info() -> RepoInfo {
        RepoInfo::new("tftio", "prompter")
    }

    fn current_version() -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn tool_checks(&self) -> Vec<DoctorCheck> {
        checks_for_state(&collect_doctor_state())
    }
}

struct DoctorState {
    config_path: PathBuf,
    default_library_path: PathBuf,
    config_file_exists: bool,
    bundle_result: Option<Result<Config, PrompterError>>,
    library_roots: Vec<PathBuf>,
    errors: Vec<String>,
    warnings: Vec<String>,
}

/// Build the user-facing check list from a collected doctor state.
fn checks_for_state(state: &DoctorState) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();

    if state.config_file_exists {
        checks.push(DoctorCheck::pass(format!(
            "Config file: {}",
            state.config_path.display()
        )));
    } else {
        checks.push(DoctorCheck::fail(
            "Config file",
            format!("Config file not found: {}", state.config_path.display()),
        ));
    }

    match &state.bundle_result {
        Some(Ok(_)) => {
            checks.push(DoctorCheck::pass("Config bundle loads (TOML + imports)"));
        }
        Some(Err(e)) => {
            checks.push(DoctorCheck::fail(
                "Config bundle loads (TOML + imports)",
                format!("Bundle load failed: {e}"),
            ));
        }
        None => {}
    }

    if state.library_roots.is_empty() {
        checks.push(DoctorCheck::fail(
            "Library directory",
            format!(
                "Library directory not found: {}",
                state.default_library_path.display()
            ),
        ));
    } else {
        for root in &state.library_roots {
            if root.exists() {
                checks.push(DoctorCheck::pass(format!(
                    "Library directory: {}",
                    root.display()
                )));
            } else {
                checks.push(DoctorCheck::fail(
                    "Library directory",
                    format!("Library directory not found: {}", root.display()),
                ));
            }
        }
    }

    checks
}

fn collect_doctor_state() -> DoctorState {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("~"));
    let config_path = home.join(".config/prompter/config.toml");
    let default_library_path = home.join(".local/prompter/library");
    collect_doctor_state_at(&config_path, &default_library_path)
}

/// Collect doctor state for explicit config and default-library paths.
///
/// Kept separate from [`collect_doctor_state`] so unit tests can drive every
/// branch (missing config, load failure, present/absent library roots) against
/// temporary fixtures without touching the real home directory.
fn collect_doctor_state_at(config_path: &Path, default_library_path: &Path) -> DoctorState {
    let config_file_exists = config_path.exists();
    let mut errors = Vec::new();
    let warnings = Vec::new();

    let (bundle_result, library_roots) = if config_file_exists {
        match load_config_bundle(config_path, Some(default_library_path)) {
            Ok(cfg) => {
                let roots = cfg.library_roots();
                for root in &roots {
                    if !root.exists() {
                        errors.push(format!("Library directory not found: {}", root.display()));
                    }
                }
                (Some(Ok(cfg)), roots)
            }
            Err(e) => {
                errors.push(format!("Bundle load failed: {e}"));
                (Some(Err(e)), Vec::new())
            }
        }
    } else {
        errors.push(format!("Config file not found: {}", config_path.display()));
        (None, Vec::new())
    };

    DoctorState {
        config_path: config_path.to_path_buf(),
        default_library_path: default_library_path.to_path_buf(),
        config_file_exists,
        bundle_result,
        library_roots,
        errors,
        warnings,
    }
}

/// Build the structured doctor report from a collected state.
fn report_for_state(state: &DoctorState) -> DoctorReport {
    report_for_state_with_version(state, env!("CARGO_PKG_VERSION"))
}

/// Build the structured doctor report with a caller-supplied tool version.
fn report_for_state_with_version(state: &DoctorState, version: &str) -> DoctorReport {
    let bundle_loads = matches!(state.bundle_result, Some(Ok(_)));
    let library_directory_exists =
        !state.library_roots.is_empty() && state.library_roots.iter().all(|p| p.exists());
    // Build the report from THIS state's checks, not from
    // `DoctorReport::for_tool(&PrompterDoctor)` — the latter runs
    // `PrompterDoctor::tool_checks()`, which re-reads the real `$HOME` and made
    // the exit code depend on the ambient environment (green on a configured
    // machine, red in a clean CI runner). `run_doctor` passes
    // `collect_doctor_state()`, so production output is unchanged.
    let mut report = DoctorReport::new(format!(
        "🏥 {} health check",
        <PrompterDoctor as DoctorChecks>::repo_info().name
    ))
    .with_checks(checks_for_state(state))
    .with_version(version)
    .with_detail("config_file_exists", json!(state.config_file_exists))
    .with_detail("bundle_loads", json!(bundle_loads))
    .with_detail("library_directory_exists", json!(library_directory_exists))
    .with_detail(
        "library_roots",
        json!(
            state
                .library_roots
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
        ),
    );

    for error in &state.errors {
        report = report.with_error(error.clone());
    }
    for warning in &state.warnings {
        report = report.with_warning(warning.clone());
    }

    report
        .with_info(format!("Current version: v{version}"))
        .with_info("Check https://github.com/tftio/prompter/releases for updates")
}

/// Run doctor command to check health and configuration.
///
/// Returns exit code: 0 if healthy, 1 if issues found.
#[must_use]
pub fn run_doctor(output: JsonOutput) -> i32 {
    report_for_state(&collect_doctor_state()).emit_output(output)
}

/// Run the prompter doctor while reporting a caller-supplied package version.
///
/// This preserves a consuming CLI's package identity when it delegates doctor
/// behavior to this shared engine.
#[must_use]
pub fn run_doctor_with_version(output: JsonOutput, version: &str) -> i32 {
    report_for_state_with_version(&collect_doctor_state(), version).emit_output(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn test_run_doctor_returns_valid_exit_code() {
        let exit_code = run_doctor(JsonOutput::Text);
        assert!(exit_code == 0 || exit_code == 1);
    }

    #[test]
    fn test_run_doctor_json_returns_valid_exit_code() {
        let exit_code = run_doctor(JsonOutput::Json);
        assert!(exit_code == 0 || exit_code == 1);
    }

    /// Unique, per-call temp HOME root that never collides across tests.
    fn unique_home(label: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "tftio-lib-doctor-{label}-{}-{n}",
            std::process::id()
        ))
    }

    #[allow(unsafe_code)]
    fn set_home(value: &Path) -> Option<std::ffi::OsString> {
        let prior = std::env::var_os("HOME");
        // SAFETY: serialized by `env_lock`; the prior value is restored before
        // the test returns. Matches the sanctioned test-only env-mutation idiom
        // used elsewhere in this crate (REPO_INVARIANTS.md #5).
        unsafe {
            std::env::set_var("HOME", value);
        }
        prior
    }

    #[allow(unsafe_code)]
    fn restore_home(prior: Option<std::ffi::OsString>) {
        // SAFETY: serialized by `env_lock`; see `set_home`.
        unsafe {
            match prior {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    /// Write a prompter config under `<home>/.config/prompter/config.toml`.
    fn write_config(home: &Path, contents: &str) {
        let cfg_dir = home.join(".config/prompter");
        fs::create_dir_all(&cfg_dir).unwrap();
        fs::write(cfg_dir.join("config.toml"), contents).unwrap();
    }

    #[test]
    fn tool_checks_report_missing_config_and_library() {
        let _guard = crate::promptlib::test_support::env_lock();
        let home = unique_home("missing-config");
        // HOME exists but no prompter config or library is present.
        fs::create_dir_all(&home).unwrap();
        let prior = set_home(&home);

        let checks = PrompterDoctor.tool_checks();
        let report = report_for_state(&collect_doctor_state());

        restore_home(prior);

        // The first check fails: the config file is absent.
        let config_check = &checks[0];
        assert!(!config_check.passed, "config check should fail: {checks:?}");
        assert_eq!(config_check.name, "Config file");
        assert!(
            config_check
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("Config file not found"),
            "unexpected config message: {:?}",
            config_check.message
        );
        // No bundle check is emitted when the config file is missing.
        assert!(
            !checks.iter().any(|c| c.name.contains("Config bundle")),
            "no bundle check expected without a config file: {checks:?}"
        );
        // With no bundle there are no library roots, so that check fails too.
        assert!(
            checks
                .iter()
                .any(|c| !c.passed && c.name == "Library directory"),
            "expected a failing library directory check: {checks:?}"
        );
        assert_ne!(report.exit_code(), 0, "unhealthy report must exit nonzero");

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn tool_checks_pass_for_valid_config_and_existing_library() {
        let _guard = crate::promptlib::test_support::env_lock();
        let home = unique_home("valid");
        fs::create_dir_all(home.join(".local/prompter/library")).unwrap();
        write_config(&home, "[root]\ndepends_on = []\n");
        let prior = set_home(&home);

        let checks = PrompterDoctor.tool_checks();
        let report = report_for_state(&collect_doctor_state());

        restore_home(prior);

        assert!(
            checks.iter().all(|c| c.passed),
            "all checks should pass: {checks:?}"
        );
        assert!(
            checks
                .iter()
                .any(|c| c.passed && c.name.starts_with("Config file:")),
            "expected a passing config-file check: {checks:?}"
        );
        assert!(
            checks
                .iter()
                .any(|c| c.passed && c.name == "Config bundle loads (TOML + imports)"),
            "expected a passing bundle check: {checks:?}"
        );
        assert!(
            checks
                .iter()
                .any(|c| c.passed && c.name.starts_with("Library directory:")),
            "expected a passing library-directory check: {checks:?}"
        );
        assert_eq!(report.exit_code(), 0, "healthy report must exit zero");

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn tool_checks_fail_when_library_root_is_missing() {
        let _guard = crate::promptlib::test_support::env_lock();
        let home = unique_home("missing-lib");
        // Valid config, but the default library directory is never created.
        write_config(&home, "[root]\ndepends_on = []\n");
        let prior = set_home(&home);

        let checks = PrompterDoctor.tool_checks();
        let report = report_for_state(&collect_doctor_state());

        restore_home(prior);

        // The bundle still loads (the library root is not required at load time),
        // but the directory it points at is absent.
        assert!(
            checks
                .iter()
                .any(|c| c.passed && c.name == "Config bundle loads (TOML + imports)"),
            "bundle should still load: {checks:?}"
        );
        let lib_fail = checks
            .iter()
            .find(|c| !c.passed && c.name == "Library directory")
            .expect("expected a failing library-directory check");
        assert!(
            lib_fail
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("Library directory not found"),
            "unexpected library message: {:?}",
            lib_fail.message
        );
        assert_ne!(report.exit_code(), 0, "missing library must exit nonzero");

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn tool_checks_report_bundle_load_failure() {
        let _guard = crate::promptlib::test_support::env_lock();
        let home = unique_home("bad-toml");
        // The config file exists but its contents are not valid TOML.
        write_config(&home, "not valid toml {{{");
        let prior = set_home(&home);

        let checks = PrompterDoctor.tool_checks();
        let report = report_for_state(&collect_doctor_state());

        restore_home(prior);

        // The config-file presence check passes...
        assert!(
            checks
                .iter()
                .any(|c| c.passed && c.name.starts_with("Config file:")),
            "config file presence should pass: {checks:?}"
        );
        // ...but loading the bundle fails.
        let bundle_fail = checks
            .iter()
            .find(|c| !c.passed && c.name == "Config bundle loads (TOML + imports)")
            .expect("expected a failing bundle-load check");
        assert!(
            bundle_fail
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("Bundle load failed"),
            "unexpected bundle message: {:?}",
            bundle_fail.message
        );
        assert_ne!(report.exit_code(), 0, "bundle failure must exit nonzero");

        fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn state_reports_missing_config() {
        let dir = unique_home("state-missing");
        let state = collect_doctor_state_at(&dir.join("config.toml"), &dir.join("library"));
        assert!(!state.config_file_exists);
        assert!(state.bundle_result.is_none());
        assert!(state.library_roots.is_empty());
        let checks = checks_for_state(&state);
        assert_eq!(checks.len(), 2);
        assert!(checks.iter().all(|check| !check.passed));
        assert_eq!(report_for_state(&state).emit_output(JsonOutput::Text), 1);
    }

    #[test]
    fn state_reports_loaded_bundle() {
        let dir = unique_home("state-ok");
        fs::create_dir_all(dir.join("library/a")).unwrap();
        fs::write(dir.join("library/a/x.md"), b"AX\n").unwrap();
        fs::write(
            dir.join("config.toml"),
            "[root]\ndepends_on = [\"a/x.md\"]\n",
        )
        .unwrap();
        let state = collect_doctor_state_at(&dir.join("config.toml"), &dir.join("library"));
        assert!(state.config_file_exists);
        assert!(matches!(state.bundle_result, Some(Ok(_))));
        assert!(!state.library_roots.is_empty());
        let checks = checks_for_state(&state);
        assert!(
            checks.iter().all(|check| check.passed),
            "all checks should pass for a healthy config"
        );
        assert_eq!(report_for_state(&state).emit_output(JsonOutput::Json), 0);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn state_reports_bundle_error() {
        let dir = unique_home("state-badtoml");
        fs::create_dir_all(dir.join("library")).unwrap();
        fs::write(dir.join("config.toml"), "this is not valid toml {{{").unwrap();
        let state = collect_doctor_state_at(&dir.join("config.toml"), &dir.join("library"));
        assert!(state.config_file_exists);
        assert!(matches!(state.bundle_result, Some(Err(_))));
        let checks = checks_for_state(&state);
        assert!(
            checks
                .iter()
                .any(|check| !check.passed && check.name.contains("bundle")),
            "a bundle-load failure should surface a failing check"
        );
        assert_eq!(report_for_state(&state).emit_output(JsonOutput::Text), 1);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn state_reports_missing_library_root() {
        let dir = unique_home("state-nolib");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("config.toml"),
            "library = \"absent\"\n\n[root]\ndepends_on = []\n",
        )
        .unwrap();
        let state = collect_doctor_state_at(&dir.join("config.toml"), &dir.join("library"));
        assert!(matches!(state.bundle_result, Some(Ok(_))));
        assert!(!state.library_roots.is_empty());
        assert!(state.library_roots.iter().any(|root| !root.exists()));
        let checks = checks_for_state(&state);
        assert!(
            checks
                .iter()
                .any(|check| !check.passed && check.name.contains("Library")),
            "a missing library root should surface a failing check"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn report_threads_warnings_without_failing() {
        let dir = unique_home("state-warn");
        fs::create_dir_all(&dir).unwrap();
        let state = DoctorState {
            config_path: dir.join("config.toml"),
            default_library_path: dir.clone(),
            config_file_exists: true,
            bundle_result: None,
            library_roots: vec![dir.clone()],
            errors: Vec::new(),
            warnings: vec!["advisory note".to_string()],
        };
        assert_eq!(report_for_state(&state).emit_output(JsonOutput::Text), 0);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn explicit_state_collection_ignores_unrelated_home() {
        let _guard = crate::promptlib::test_support::env_lock();
        let fixture = unique_home("explicit-fixture");
        let unrelated_home = unique_home("unrelated-home");
        fs::create_dir_all(fixture.join("library")).unwrap();
        fs::create_dir_all(&unrelated_home).unwrap();
        fs::write(fixture.join("config.toml"), "[root]\ndepends_on = []\n").unwrap();
        let prior = set_home(&unrelated_home);

        let state = collect_doctor_state_at(&fixture.join("config.toml"), &fixture.join("library"));

        restore_home(prior);

        assert_eq!(state.config_path, fixture.join("config.toml"));
        assert!(checks_for_state(&state).iter().all(|check| check.passed));

        fs::remove_dir_all(&fixture).ok();
        fs::remove_dir_all(&unrelated_home).ok();
    }

    #[test]
    fn version_aware_report_preserves_consumer_identity() {
        let dir = unique_home("consumer-version");
        fs::create_dir_all(&dir).unwrap();
        let state = DoctorState {
            config_path: dir.join("config.toml"),
            default_library_path: dir.clone(),
            config_file_exists: true,
            bundle_result: None,
            library_roots: vec![dir.clone()],
            errors: Vec::new(),
            warnings: Vec::new(),
        };

        let value = report_for_state_with_version(&state, "4.0.2").to_json_value();

        assert_eq!(value["version"], serde_json::json!("4.0.2"));
        assert!(
            value["info"]
                .as_array()
                .is_some_and(|items| items.iter().any(|item| item == "Current version: v4.0.2"))
        );

        fs::remove_dir_all(&dir).ok();
    }
}
