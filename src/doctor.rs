//! Runtime-configuration doctor integration.

use std::path::PathBuf;

use tftio_lib::project::{discover_inputs, load_registry};
use tftio_lib::{DoctorCheck, DoctorChecks, RepoInfo};

use crate::config::load_config;

/// Everything [`Doctor`] needs to build its checks.
#[derive(Debug, Clone)]
pub struct DoctorInputs {
    /// Resolved runtime configuration path.
    pub config_path: Option<PathBuf>,
    /// Directory holding `projects.toml`, when computable.
    pub registry_dir: Option<PathBuf>,
    /// Home directory, used to expand `~`-prefixed registry paths.
    pub home: Option<PathBuf>,
    /// Directory whose `.clanker` declaration is checked against the
    /// registry's remote mapping.
    pub current_dir: PathBuf,
}

/// Doctor checks bound to one runtime configuration path and project registry.
pub struct Doctor {
    inputs: DoctorInputs,
}

impl Doctor {
    /// Create doctor checks for the given inputs.
    #[must_use]
    pub const fn new(inputs: DoctorInputs) -> Self {
        Self { inputs }
    }
}

impl DoctorChecks for Doctor {
    fn repo_info() -> RepoInfo {
        RepoInfo::new("tftio", "clanker")
    }

    fn current_version() -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn tool_checks(&self) -> Vec<DoctorCheck> {
        let mut checks = self.config_checks();
        checks.extend(self.project_registry_checks());
        checks
    }
}

impl Doctor {
    fn config_checks(&self) -> Vec<DoctorCheck> {
        self.inputs.config_path.as_ref().map_or_else(
            || {
                vec![DoctorCheck::fail(
                    "Clanker config",
                    "$HOME is not set and CLANKER_CONFIG was not provided",
                )]
            },
            |path| match load_config(path) {
                Ok(loaded) => {
                    let mut checks = vec![DoctorCheck::pass(format!(
                        "Config file: {}",
                        loaded.base_path.display()
                    ))];
                    if let Some(overlay) = loaded.overlay_path {
                        checks.push(DoctorCheck::pass(format!(
                            "Local overlay: {}",
                            overlay.display()
                        )));
                    }
                    checks.push(DoctorCheck::pass("Runtime config schema and invariants"));
                    checks
                }
                Err(error) => vec![DoctorCheck::fail("Clanker config", error.to_string())],
            },
        )
    }

    /// Validate the installed project registry and cross-check the working
    /// directory's `.clanker` declaration against it.
    ///
    /// Absent a computable registry directory (no `HOME`), no check is
    /// added: there is no default location to report on, and `HOME` being
    /// unset is already reported by [`Doctor::config_checks`].
    fn project_registry_checks(&self) -> Vec<DoctorCheck> {
        let Some(registry_dir) = &self.inputs.registry_dir else {
            return Vec::new();
        };

        let registry = match load_registry(registry_dir, self.inputs.home.as_deref()) {
            Ok(registry) => registry,
            Err(error) => return vec![DoctorCheck::fail("Project registry", error.to_string())],
        };

        let mut checks = Vec::new();
        let issues = registry.validate();
        if issues.is_empty() {
            checks.push(DoctorCheck::pass("Project registry"));
        } else {
            for issue in issues {
                checks.push(DoctorCheck::fail("Project registry", issue.to_string()));
            }
        }

        for (slug, project) in registry.projects() {
            for path in &project.paths {
                if !path.exists() {
                    checks.push(DoctorCheck {
                        name: format!("Project path ({slug})"),
                        passed: true,
                        message: Some(format!(
                            "warning: {} is registered for `{slug}` but does not exist on this machine",
                            path.display()
                        )),
                    });
                }
            }
        }

        checks.push(self.declaration_agreement_check(&registry));
        checks
    }

    /// Fail when the working directory's `.clanker` declares a project that
    /// disagrees with what the registry maps its origin remote to.
    fn declaration_agreement_check(&self, registry: &tftio_lib::project::Registry) -> DoctorCheck {
        let canonical = self
            .inputs
            .current_dir
            .canonicalize()
            .unwrap_or_else(|_| self.inputs.current_dir.clone());
        let inputs = match discover_inputs(&canonical) {
            Ok(inputs) => inputs,
            Err(error) => return DoctorCheck::fail("Project declaration", error.to_string()),
        };
        let (Some(declared), Some(remote)) = (&inputs.declared, &inputs.remote) else {
            return DoctorCheck::pass("Project declaration");
        };
        for (slug, project) in registry.projects() {
            if project.remotes.contains(remote.as_str()) && slug != declared.as_str() {
                return DoctorCheck::fail(
                    "Project declaration",
                    format!(
                        "{} declares project `{declared}` but the registry maps remote `{remote}` to `{slug}`",
                        canonical.display()
                    ),
                );
            }
        }
        DoctorCheck::pass("Project declaration")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use tftio_lib::DoctorChecks;

    const VALID_CONFIG: &str = r#"
[defaults]
domain = "eng"
contexts = ["personal"]
shim_path = "~/.local/clankers/bin"
sandbox_wrapper = "~/unused"
prompter_bundle = "~/.local/prompter"
prompt_cache = "~/.cache/clanker"
prompt_cache_ttl_seconds = 1

[harness.claude]
bin = "claude"
family = "claude"
default_args = []
[harness.claude.injection]
kind = "arg-text"
args = ["{text}"]

[model]

[domain.eng]
profiles = ["core.base"]
env = {}
"#;

    fn write_config(dir: &std::path::Path, body: &str) -> PathBuf {
        let path = dir.join("config.toml");
        fs::write(&path, body).unwrap();
        path
    }

    fn doctor_inputs(config_path: Option<PathBuf>) -> DoctorInputs {
        DoctorInputs {
            config_path,
            registry_dir: None,
            home: None,
            current_dir: std::env::temp_dir(),
        }
    }

    #[test]
    fn valid_config_with_overlay_passes_every_check() {
        let temp = TempDir::new().unwrap();
        let config_path = write_config(temp.path(), VALID_CONFIG);
        fs::write(
            temp.path().join("local.toml"),
            "[defaults]\ndomain = \"eng\"\n",
        )
        .unwrap();

        let checks = Doctor::new(doctor_inputs(Some(config_path))).tool_checks();

        assert!(checks.iter().all(|check| check.passed));
        assert!(
            checks
                .iter()
                .any(|check| check.name.contains("Local overlay"))
        );
    }

    #[test]
    fn absent_config_path_fails_closed() {
        let checks = Doctor::new(doctor_inputs(None)).tool_checks();
        assert!(
            checks
                .iter()
                .any(|check| check.name == "Clanker config" && !check.passed)
        );
    }

    #[test]
    fn unparsable_config_fails_closed() {
        let temp = TempDir::new().unwrap();
        let config_path = write_config(temp.path(), "this is = not valid = toml");

        let checks = Doctor::new(doctor_inputs(Some(config_path))).tool_checks();

        assert!(
            checks
                .iter()
                .any(|check| check.name == "Clanker config" && !check.passed)
        );
    }

    #[test]
    fn semantically_invalid_config_fails_closed() {
        let temp = TempDir::new().unwrap();
        let config_path = write_config(
            temp.path(),
            &VALID_CONFIG.replace("domain = \"eng\"", "domain = \"ghost\""),
        );

        let checks = Doctor::new(doctor_inputs(Some(config_path))).tool_checks();

        assert!(checks.iter().any(|check| {
            check.name == "Clanker config"
                && check
                    .message
                    .as_deref()
                    .is_some_and(|message| message.contains("defaults.domain"))
        }));
    }

    #[test]
    fn no_registry_check_is_added_without_a_computable_registry_directory() {
        let checks = Doctor::new(doctor_inputs(None)).tool_checks();
        assert!(!checks.iter().any(|check| check.name == "Project registry"));
    }

    #[test]
    fn a_clean_registry_passes() {
        let temp = TempDir::new().unwrap();
        fs::write(
            temp.path().join("projects.toml"),
            "[project.kb]\nremotes = [\"github.com/tftio/kb\"]\n",
        )
        .unwrap();

        let checks = Doctor::new(DoctorInputs {
            config_path: None,
            registry_dir: Some(temp.path().to_path_buf()),
            home: None,
            current_dir: temp.path().to_path_buf(),
        })
        .tool_checks();

        assert!(
            checks
                .iter()
                .any(|check| check.name == "Project registry" && check.passed)
        );
    }

    #[test]
    fn registry_validation_issues_are_reported_as_failures() {
        let temp = TempDir::new().unwrap();
        fs::write(
            temp.path().join("projects.toml"),
            "[project.\"Not Valid\"]\n",
        )
        .unwrap();

        let checks = Doctor::new(DoctorInputs {
            config_path: None,
            registry_dir: Some(temp.path().to_path_buf()),
            home: None,
            current_dir: temp.path().to_path_buf(),
        })
        .tool_checks();

        assert!(
            checks
                .iter()
                .any(|check| check.name == "Project registry" && !check.passed)
        );
    }

    #[test]
    fn a_malformed_registry_file_fails_closed() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("projects.toml"), "not valid toml =").unwrap();

        let checks = Doctor::new(DoctorInputs {
            config_path: None,
            registry_dir: Some(temp.path().to_path_buf()),
            home: None,
            current_dir: temp.path().to_path_buf(),
        })
        .tool_checks();

        assert!(
            checks
                .iter()
                .any(|check| check.name == "Project registry" && !check.passed)
        );
    }

    #[test]
    fn a_registered_path_absent_on_this_machine_is_a_warning_not_a_failure() {
        let temp = TempDir::new().unwrap();
        let missing = temp.path().join("does-not-exist");
        fs::write(
            temp.path().join("projects.toml"),
            format!("[project.kb]\npaths = [\"{}\"]\n", missing.display()),
        )
        .unwrap();

        let checks = Doctor::new(DoctorInputs {
            config_path: None,
            registry_dir: Some(temp.path().to_path_buf()),
            home: None,
            current_dir: temp.path().to_path_buf(),
        })
        .tool_checks();

        let path_check = checks
            .iter()
            .find(|check| check.name == "Project path (kb)")
            .expect("a warning check for the missing path");
        assert!(path_check.passed, "a missing registered path is a warning");
        assert!(
            path_check
                .message
                .as_deref()
                .is_some_and(|message| message.starts_with("warning:"))
        );
    }

    #[test]
    fn declaration_and_registry_remote_agreeing_passes() {
        let temp = TempDir::new().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::write(repo.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(
            repo.join(".git").join("config"),
            "[remote \"origin\"]\n\turl = git@github.com:tftio/kb.git\n",
        )
        .unwrap();
        fs::write(repo.join(".clanker"), "project = \"kb\"\n").unwrap();
        fs::write(
            temp.path().join("projects.toml"),
            "[project.kb]\nremotes = [\"github.com/tftio/kb\"]\n",
        )
        .unwrap();

        let checks = Doctor::new(DoctorInputs {
            config_path: None,
            registry_dir: Some(temp.path().to_path_buf()),
            home: None,
            current_dir: repo,
        })
        .tool_checks();

        assert!(
            checks
                .iter()
                .any(|check| check.name == "Project declaration" && check.passed)
        );
    }

    #[test]
    fn a_declaration_disagreeing_with_the_registries_remote_mapping_fails() {
        let temp = TempDir::new().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::write(repo.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(
            repo.join(".git").join("config"),
            "[remote \"origin\"]\n\turl = git@github.com:tftio/kb.git\n",
        )
        .unwrap();
        fs::write(repo.join(".clanker"), "project = \"kb-declared\"\n").unwrap();
        fs::write(
            temp.path().join("projects.toml"),
            "[project.kb-registered]\nremotes = [\"github.com/tftio/kb\"]\n",
        )
        .unwrap();

        let checks = Doctor::new(DoctorInputs {
            config_path: None,
            registry_dir: Some(temp.path().to_path_buf()),
            home: None,
            current_dir: repo,
        })
        .tool_checks();

        let declaration_check = checks
            .iter()
            .find(|check| check.name == "Project declaration")
            .expect("a declaration check");
        assert!(!declaration_check.passed);
        assert!(declaration_check.message.as_deref().is_some_and(|message| {
            message.contains("kb-declared") && message.contains("kb-registered")
        }));
    }

    #[test]
    fn an_unnormalizable_origin_remote_fails_the_declaration_check() {
        let temp = TempDir::new().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::write(repo.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
        // An empty `url` does not normalize, which surfaces as a
        // `DiscoveryError` distinct from the register-mismatch case above.
        fs::write(
            repo.join(".git").join("config"),
            "[remote \"origin\"]\n\turl = \n",
        )
        .unwrap();
        fs::write(temp.path().join("projects.toml"), "").unwrap();

        let checks = Doctor::new(DoctorInputs {
            config_path: None,
            registry_dir: Some(temp.path().to_path_buf()),
            home: None,
            current_dir: repo,
        })
        .tool_checks();

        assert!(
            checks
                .iter()
                .any(|check| check.name == "Project declaration" && !check.passed)
        );
    }
}
