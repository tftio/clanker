//! Binary entrypoint for clanker command and multi-call symlink modes.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        reason = "test code uses fail-fast assertions against temp fixtures"
    )
)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use clanker::error::ClankerError;
use clanker::{RuntimeContext, command_exit_code, invocation_name, run_symlink_mode};
use tftio_lib::{JsonOutput, ProcessEnv, run_with_display_error_handler};

/// Prefix of every environment variable clanker reads back from its own
/// launches. Nothing outside `HOME`, `PATH`, and this prefix has its *value*
/// examined; variable names are additionally collected so a launch can remove
/// the ones a domain was not granted.
const CLANKER_ENVIRONMENT_PREFIX: &[u8] = b"CLANKER_";

fn main() {
    let argv: Vec<OsString> = std::env::args_os().collect();
    let invocation = argv
        .first()
        .map_or_else(|| "clanker".to_string(), |value| invocation_name(value));
    let process_env = process_env();
    let runtime = match runtime_context() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    };

    let exit_code = if invocation.ends_with("-launch") {
        let harness_arguments = argv.into_iter().skip(1).collect();
        run_with_display_error_handler("launch", JsonOutput::Text, || {
            run_symlink_mode(&invocation, harness_arguments, &runtime)
        })
    } else {
        command_exit_code(argv, &process_env, &runtime)
    };
    std::process::exit(exit_code);
}

#[allow(
    clippy::disallowed_methods,
    reason = "process environment is read once at the binary edge"
)]
fn process_env() -> ProcessEnv {
    ProcessEnv {
        agent: tftio_lib::AgentModeContext::from_tokens(
            std::env::var(tftio_lib::AGENT_TOKEN_ENV).ok(),
            std::env::var(tftio_lib::AGENT_TOKEN_EXPECTED_ENV).ok(),
        ),
        home: std::env::var_os("HOME").map(PathBuf::from),
    }
}

#[allow(
    clippy::disallowed_methods,
    reason = "launch environment and working directory are read once at the binary edge"
)]
fn runtime_context() -> Result<RuntimeContext, ClankerError> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let current_dir = std::env::current_dir().map_err(ClankerError::CurrentDirectory)?;
    let clanker_environment = std::env::vars_os()
        .filter(|(key, _)| {
            key.as_encoded_bytes()
                .starts_with(CLANKER_ENVIRONMENT_PREFIX)
        })
        .collect();
    let xdg_config_home = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
    let project = project_inputs(xdg_config_home.as_deref(), home.as_deref(), &current_dir);
    Ok(RuntimeContext {
        home,
        session_id: uuid::Uuid::now_v7().to_string(),
        current_dir,
        config_environment: std::env::var_os("CLANKER_CONFIG").map(PathBuf::from),
        context_environment: environment_text("CLANKER_CONTEXT")?,
        domain_environment: environment_text("CLANKER_DOMAIN")?,
        model_environment: environment_text("CLANKER_MODEL")?,
        inherited_path: std::env::var_os("PATH"),
        clanker_environment,
        // Names only. Reading values here would pull every ambient credential
        // into this process merely to decide which ones not to pass on.
        inherited_names: std::env::vars_os().map(|(key, _)| key).collect(),
        project_registry_dir: project.registry_dir,
        project_registry: project.registry,
        declared_project: project.declared,
        origin_remote: project.remote,
        project_dir: project.dir,
    })
}

/// Everything [`project_inputs`] resolves for one launch.
struct ProjectInputs {
    registry_dir: Option<PathBuf>,
    registry: tftio_lib::project::Registry,
    declared: Option<tftio_lib::project::Slug>,
    remote: Option<tftio_lib::project::NormalizedRemote>,
    dir: PathBuf,
}

/// Resolve the project registry and directory-discovery inputs for the
/// current launch.
///
/// A registry load failure or a discovery failure must never prevent a
/// launch (ENG-004): each is reported on stderr, naming the file and the
/// error, and the launch proceeds as though that tier resolved nothing.
/// `clanker doctor` reports the same failures as check failures rather than
/// warnings, since a doctor run is exactly the moment to surface them.
fn project_inputs(
    xdg_config_home: Option<&Path>,
    home: Option<&Path>,
    current_dir: &Path,
) -> ProjectInputs {
    let dir = current_dir
        .canonicalize()
        .unwrap_or_else(|_| current_dir.to_path_buf());

    let registry_dir = tftio_lib::project::default_registry_dir(xdg_config_home, home);
    let registry = registry_dir.as_deref().map_or_else(
        tftio_lib::project::Registry::default,
        |registry_dir| {
            tftio_lib::project::load_registry(registry_dir, home).unwrap_or_else(|error| {
                eprintln!(
                    "warning: project registry {}: {error}",
                    registry_dir.join("projects.toml").display()
                );
                tftio_lib::project::Registry::default()
            })
        },
    );

    let (declared, remote) = match tftio_lib::project::discover_inputs(&dir) {
        Ok(inputs) => (inputs.declared, inputs.remote),
        Err(error) => {
            eprintln!("warning: project discovery at {}: {error}", dir.display());
            (None, None)
        }
    };

    ProjectInputs {
        registry_dir,
        registry,
        declared,
        remote,
        dir,
    }
}

#[allow(
    clippy::disallowed_methods,
    reason = "environment value is parsed at the binary edge"
)]
fn environment_text(name: &'static str) -> Result<Option<String>, ClankerError> {
    std::env::var_os(name)
        .map(|value| {
            value
                .into_string()
                .map_err(|_| ClankerError::NonUtf8Environment(name))
        })
        .transpose()
}
