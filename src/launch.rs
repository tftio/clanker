//! Harness launch planning and Unix exec handoff.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::axis::{self, AxisSource, InvalidAxisValue};
use crate::cli::{DefaultArgsMode, ExecRequest, LaunchRequest};
use crate::config::{
    Config, ConfigDirectory, DomainConfig, DomainName, HarnessConfig, HarnessName, ModelConfig,
    ModelFamily, ModelName, PLAYWRIGHT_MCP_BROWSER_ENVIRONMENT,
    PLAYWRIGHT_MCP_USER_DATA_DIR_ENVIRONMENT,
};
use crate::context::{
    ContextError, ResolvedContext, read_directory_config, resolve_context,
    resolve_context_with_directory,
};
use crate::prompt::{PromptError, compose_and_inject};

const CONTEXT_PLACEHOLDER: &str = "{context}";
const CONTEXT_ENVIRONMENT: &str = "CONTEXT";
const CLANKER_CONTEXT_ENVIRONMENT: &str = "CLANKER_CONTEXT";
const PLAYWRIGHT_BROWSER: &str = "chromium";
const PLAYWRIGHT_PROFILE_ROOT: &str = ".local/share/clanker/playwright";
const MNENE_AGENT_ENVIRONMENT: &str = "MNENE_AGENT";
const MNENE_CONTEXT_ENVIRONMENT: &str = "MNENE_CONTEXT";
const MNENE_SESSION_ENVIRONMENT: &str = "MNENE_SESSION";

pub(crate) const SESSION_ACTIVE_ENVIRONMENT: &str = "CLANKER_SESSION";
pub(crate) const SESSION_MARKER_VALUE: &str = "1";
pub(crate) const SESSION_VERSION_ENVIRONMENT: &str = "CLANKER_SESSION_VERSION";
pub(crate) const SESSION_VERSION: &str = "5";
pub(crate) const SESSION_ID_ENVIRONMENT: &str = "CLANKER_SESSION_ID";
pub(crate) const SESSION_INVOCATION_ENVIRONMENT: &str = "CLANKER_SESSION_INVOCATION";
pub(crate) const SESSION_HARNESS_ENVIRONMENT: &str = "CLANKER_SESSION_HARNESS";
pub(crate) const SESSION_CONTEXT_ENVIRONMENT: &str = "CLANKER_SESSION_CONTEXT";
pub(crate) const SESSION_CONTEXT_SOURCE_ENVIRONMENT: &str = "CLANKER_SESSION_CONTEXT_SOURCE";
pub(crate) const SESSION_DOMAINS_ENVIRONMENT: &str = "CLANKER_SESSION_DOMAINS";
pub(crate) const SESSION_DOMAIN_SOURCE_ENVIRONMENT: &str = "CLANKER_SESSION_DOMAIN_SOURCE";
pub(crate) const SESSION_MODEL_ENVIRONMENT: &str = "CLANKER_SESSION_MODEL";
pub(crate) const SESSION_MODEL_SOURCE_ENVIRONMENT: &str = "CLANKER_SESSION_MODEL_SOURCE";
pub(crate) const SESSION_FAMILY_ENVIRONMENT: &str = "CLANKER_SESSION_FAMILY";
pub(crate) const SESSION_PROJECT_ENVIRONMENT: &str = "CLANKER_SESSION_PROJECT";
pub(crate) const SESSION_PROJECT_SOURCE_ENVIRONMENT: &str = "CLANKER_SESSION_PROJECT_SOURCE";
pub(crate) const SESSION_REMOTE_ENVIRONMENT: &str = "CLANKER_SESSION_REMOTE";
const SESSION_ENVIRONMENTS: [&str; 15] = [
    SESSION_ACTIVE_ENVIRONMENT,
    SESSION_VERSION_ENVIRONMENT,
    SESSION_ID_ENVIRONMENT,
    SESSION_INVOCATION_ENVIRONMENT,
    SESSION_HARNESS_ENVIRONMENT,
    SESSION_CONTEXT_ENVIRONMENT,
    SESSION_CONTEXT_SOURCE_ENVIRONMENT,
    SESSION_DOMAINS_ENVIRONMENT,
    SESSION_DOMAIN_SOURCE_ENVIRONMENT,
    SESSION_MODEL_ENVIRONMENT,
    SESSION_MODEL_SOURCE_ENVIRONMENT,
    SESSION_FAMILY_ENVIRONMENT,
    SESSION_PROJECT_ENVIRONMENT,
    SESSION_PROJECT_SOURCE_ENVIRONMENT,
    SESSION_REMOTE_ENVIRONMENT,
];
const MNENE_ENVIRONMENTS: [&str; 3] = [
    MNENE_AGENT_ENVIRONMENT,
    MNENE_CONTEXT_ENVIRONMENT,
    MNENE_SESSION_ENVIRONMENT,
];

/// The layer that contributed one environment entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Provenance {
    /// Computed by clanker from the resolved context and configured paths.
    Clanker,
    /// Supplied by prompt injection.
    Injection,
    /// Supplied by a resolved domain.
    Domain,
    /// Supplied by the resolved model.
    Model,
    /// A launch-time session marker.
    Session,
}

impl Provenance {
    /// Stable label used in dry-run output.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Clanker => "clanker",
            Self::Injection => "injection",
            Self::Domain => "domain",
            Self::Model => "model",
            Self::Session => "session",
        }
    }
}

/// One resolved environment entry and the layer that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentValue {
    /// Value exported to the child process.
    pub value: OsString,
    /// Layer that contributed this entry.
    pub provenance: Provenance,
}

/// Process-edge values required to build a launch plan.
#[derive(Debug, Clone, Copy)]
pub struct LaunchInputs<'a> {
    /// Home directory used for `~` expansion.
    pub home: &'a Path,
    /// Identifier for this launch, minted once at the binary edge and
    /// exported as `CLANKER_SESSION_ID`.
    pub session_id: &'a str,
    /// Current working directory used for dotfile lookup.
    pub current_dir: &'a Path,
    /// `CLANKER_CONTEXT` value.
    pub context_environment: Option<&'a str>,
    /// `CLANKER_DOMAIN` value.
    pub domain_environment: Option<&'a str>,
    /// `CLANKER_MODEL` value.
    pub model_environment: Option<&'a str>,
    /// Inherited `PATH` value.
    pub inherited_path: Option<&'a std::ffi::OsStr>,
    /// Names of every variable in the ambient environment, with no values.
    ///
    /// A plan removes those its domain allowlist does not name, so a session
    /// carries only what it was granted.
    pub inherited_names: &'a [OsString],
    /// Loaded project registry, read once at the binary edge.
    pub project_registry: &'a tftio_lib::project::Registry,
    /// `.clanker` project declaration at this working directory's project
    /// root, read once at the binary edge by
    /// [`tftio_lib::project::discover_inputs`].
    pub declared_project: Option<&'a tftio_lib::project::Slug>,
    /// This working directory's normalized origin remote, when it is inside
    /// a repository with one, read once at the binary edge.
    pub origin_remote: Option<&'a tftio_lib::project::NormalizedRemote>,
    /// Canonicalized working directory used to match registry `paths`
    /// entries; canonicalized once at the binary edge so a registered path
    /// and the working directory compare on the same footing.
    pub project_dir: &'a Path,
}

/// Fully resolved process launch.
#[derive(Debug, Clone)]
pub struct LaunchPlan {
    /// Identifier for this launch, minted once at the binary edge.
    pub session_id: String,
    /// Name through which clanker was invoked.
    pub invocation: String,
    /// Configured harness key.
    pub harness: HarnessName,
    /// Resolved context.
    pub context: ResolvedContext,
    /// Resolved domains in command-mode composition order.
    pub domains: Vec<DomainName>,
    /// Tier that supplied the domain stack, when command mode selected one.
    pub domain_source: Option<AxisSource>,
    /// Resolved model alias in command mode.
    pub model: Option<ModelName>,
    /// Tier that supplied the model, when one was applied.
    pub model_source: Option<AxisSource>,
    /// Resolved prompter family in command mode.
    pub family: Option<String>,
    /// Resolved project slug, when one was resolved for the launch directory.
    pub project: Option<tftio_lib::project::Slug>,
    /// Tier that supplied the project slug, when one resolved.
    pub project_source: Option<tftio_lib::project::Source>,
    /// Normalized origin remote of the launch directory, when one exists,
    /// regardless of whether it supplied the resolved project.
    pub remote: Option<tftio_lib::project::NormalizedRemote>,
    /// Executable name or path.
    pub executable: OsString,
    /// Verbatim harness arguments.
    pub args: Vec<OsString>,
    /// Environment overrides applied to the inherited process environment.
    pub environment: BTreeMap<OsString, EnvironmentValue>,
    /// Rendered prompt text when injection succeeded.
    pub prompt: Option<String>,
    /// File used for file-based prompt injection.
    pub prompt_file: Option<PathBuf>,
    /// Ambient variables removed before the harness starts.
    ///
    /// Everything inherited that the resolved domains' allowlists do not name.
    /// Computed during plan construction from `defaults.passthrough_environment`
    /// and the resolved domains' `credentials` lists.
    pub removed_environment: Vec<OsString>,
}

/// Fully resolved process execution for [`crate::cli::Command::Exec`].
#[derive(Debug, Clone)]
pub struct ExecPlan {
    /// Resolved context whose harness directories are exported.
    pub context: ResolvedContext,
    /// Executable name or path from the first argv value.
    pub executable: OsString,
    /// Verbatim argv values after the executable.
    pub args: Vec<OsString>,
    /// Environment overrides applied to the inherited process environment.
    pub environment: BTreeMap<OsString, EnvironmentValue>,
    /// Inherited variables removed before process replacement.
    pub removed_environment: Vec<OsString>,
}

/// One harness omitted from an exec plan because a declared directory is absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedConfigDirectory {
    /// Configured harness whose directory set was omitted.
    pub harness: HarnessName,
    /// First declared directory that was absent.
    pub path: PathBuf,
}

/// Details of one conflicting config-directory variable declaration.
#[derive(Debug, thiserror::Error)]
#[error(
    "config directory variable `{environment}` resolves differently for harnesses `{first_harness}` ({}) and `{second_harness}` ({})",
    first_path.display(),
    second_path.display()
)]
pub struct ConfigDirectoryCollisionError {
    environment: String,
    first_harness: HarnessName,
    first_path: PathBuf,
    second_harness: HarnessName,
    second_path: PathBuf,
}

/// Process data consumed by the Unix exec handoff.
pub trait ExecutablePlan {
    /// Executable name or path.
    fn executable(&self) -> &std::ffi::OsStr;
    /// Arguments following argv zero.
    fn args(&self) -> &[OsString];
    /// Environment overrides for the replacement process.
    fn environment(&self) -> &BTreeMap<OsString, EnvironmentValue>;
    /// Inherited environment variables removed before process replacement.
    fn removed_environment(&self) -> &[OsString] {
        &[]
    }
}

impl ExecutablePlan for LaunchPlan {
    fn executable(&self) -> &std::ffi::OsStr {
        &self.executable
    }

    fn args(&self) -> &[OsString] {
        &self.args
    }

    fn environment(&self) -> &BTreeMap<OsString, EnvironmentValue> {
        &self.environment
    }

    fn removed_environment(&self) -> &[OsString] {
        &self.removed_environment
    }
}

impl ExecutablePlan for ExecPlan {
    fn executable(&self) -> &std::ffi::OsStr {
        &self.executable
    }

    fn args(&self) -> &[OsString] {
        &self.args
    }

    fn environment(&self) -> &BTreeMap<OsString, EnvironmentValue> {
        &self.environment
    }

    fn removed_environment(&self) -> &[OsString] {
        &self.removed_environment
    }
}

impl LaunchPlan {
    fn set_environment(
        &mut self,
        key: impl Into<OsString>,
        value: impl Into<OsString>,
        provenance: Provenance,
    ) {
        self.environment.insert(
            key.into(),
            EnvironmentValue {
                value: value.into(),
                provenance,
            },
        );
    }
}

/// Serializable dry-run projection of [`LaunchPlan`].
#[derive(Debug, Serialize)]
pub struct LaunchPlanOutput {
    invocation: String,
    harness: String,
    context: String,
    context_source: &'static str,
    domains: Vec<String>,
    domain_source: Option<&'static str>,
    model: Option<String>,
    model_source: Option<&'static str>,
    family: Option<String>,
    project: Option<String>,
    project_source: Option<&'static str>,
    remote: Option<String>,
    executable: String,
    args: Vec<String>,
    environment: BTreeMap<String, EnvironmentOutput>,
    /// Ambient variables this launch removes: everything inherited that the
    /// resolved domains did not grant. Present so an operator can see what a
    /// domain actually withholds without launching anything.
    removed_environment: Vec<String>,
    prompt: Option<String>,
    prompt_file: Option<String>,
}

/// Serializable environment entry carrying its provenance.
#[derive(Debug, Serialize)]
pub struct EnvironmentOutput {
    value: String,
    provenance: Provenance,
}

impl From<&LaunchPlan> for LaunchPlanOutput {
    fn from(plan: &LaunchPlan) -> Self {
        Self {
            invocation: plan.invocation.clone(),
            harness: plan.harness.to_string(),
            context: plan.context.value.to_string(),
            context_source: plan.context.source.label(axis::CONTEXT),
            domains: plan.domains.iter().map(ToString::to_string).collect(),
            domain_source: plan.domain_source.map(|source| source.label(axis::DOMAIN)),
            model: plan.model.as_ref().map(ToString::to_string),
            model_source: plan.model_source.map(|source| source.label(axis::MODEL)),
            family: plan.family.clone(),
            project: plan.project.as_ref().map(ToString::to_string),
            project_source: plan.project_source.map(tftio_lib::project::Source::label),
            remote: plan.remote.as_ref().map(ToString::to_string),
            executable: plan.executable.to_string_lossy().into_owned(),
            args: plan
                .args
                .iter()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect(),
            environment: plan
                .environment
                .iter()
                .map(|(key, entry)| {
                    (
                        key.to_string_lossy().into_owned(),
                        EnvironmentOutput {
                            value: entry.value.to_string_lossy().into_owned(),
                            provenance: entry.provenance,
                        },
                    )
                })
                .collect(),
            removed_environment: plan
                .removed_environment
                .iter()
                .map(|name| name.to_string_lossy().into_owned())
                .collect(),
            prompt: plan.prompt.clone(),
            prompt_file: plan
                .prompt_file
                .as_ref()
                .map(|path| path.display().to_string()),
        }
    }
}

/// Failures resolving or executing a launch.
#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    /// Symlink invocation name does not follow the multi-call contract.
    #[error("invoked as `{0}`; expected a <harness>-launch symlink")]
    InvalidInvocation(String),
    /// Harness key does not exist in runtime configuration.
    #[error("unknown harness `{0}` in clanker config")]
    UnknownHarness(String),
    /// The selected harness is disabled in the resolved context.
    #[error("harness `{harness}` is not enabled in context `{context}`")]
    UnsupportedContext {
        /// Selected harness.
        harness: HarnessName,
        /// Resolved context.
        context: crate::config::ContextName,
    },
    /// Domain key does not exist in runtime configuration.
    #[error("unknown domain `{0}` in clanker config")]
    UnknownDomain(String),
    /// Model key does not exist in runtime configuration.
    #[error("unknown model `{0}` in clanker config")]
    UnknownModel(String),
    /// A configured model cannot be used with the selected harness.
    #[error("model `{model}` requires harness `{required}`, not `{selected}`")]
    ModelHarnessMismatch {
        /// Selected model alias.
        model: ModelName,
        /// Harness required by the model.
        required: HarnessName,
        /// Harness selected for this launch.
        selected: HarnessName,
    },
    /// An ambient axis name is invalid.
    #[error(transparent)]
    Axis(#[from] InvalidAxisValue),
    /// Context resolution failed.
    #[error(transparent)]
    Context(#[from] ContextError),
    /// A context-derived harness config directory does not exist.
    #[error(
        "{invocation}: config directory {} does not exist; create it or select a different context",
        path.display()
    )]
    MissingConfigDirectory {
        /// Invocation name.
        invocation: String,
        /// Missing directory path.
        path: PathBuf,
    },
    /// Two harnesses resolve one exported variable to different directories.
    #[error(transparent)]
    ConfigDirectoryCollision(Box<ConfigDirectoryCollisionError>),
    /// Prompt composition or injection failed.
    #[error(transparent)]
    Prompt(#[from] PromptError),
    /// A runtime path uses unsupported tilde syntax.
    #[error("unsupported configured path `{0}`; use `~` or `~/...`")]
    UnsupportedTilde(String),
    /// Configured shim path could not be joined with inherited `PATH`.
    #[error("failed to build launch PATH: {0}")]
    PathList(#[from] std::env::JoinPathsError),
    /// Final process exec failed.
    #[error("failed to exec `{executable}`: {source}")]
    Exec {
        /// Executable that failed.
        executable: String,
        /// Underlying OS error.
        source: std::io::Error,
    },
}

/// Build a launch plan from a `<harness>-launch` invocation name.
///
/// # Errors
/// Returns [`LaunchError`] for invalid invocation names, unknown harnesses,
/// context failures, or invalid configured paths.
pub fn build_symlink_plan(
    invocation: &str,
    args: Vec<OsString>,
    config: &Config,
    inputs: LaunchInputs<'_>,
) -> Result<LaunchPlan, LaunchError> {
    let harness = invocation
        .strip_suffix("-launch")
        .filter(|name| !name.is_empty())
        .ok_or_else(|| LaunchError::InvalidInvocation(invocation.to_string()))?;
    let harness = HarnessName::new(harness)
        .map_err(|_| LaunchError::InvalidInvocation(invocation.to_string()))?;
    let context = resolve_context(inputs.current_dir, None, inputs.context_environment, config)?;
    let mut plan =
        build_harness_plan_for_context(invocation, harness, args, config, inputs, context)?;
    apply_session_environment(&mut plan);
    // Symlink mode resolves a context but no domains, so there is no domain
    // credential grant to apply: the session receives the base passthrough and
    // nothing else. Applying the allowlist here rather than skipping it is the
    // point -- a launch path that quietly bypassed the filter would be the one
    // hole worth having.
    apply_credential_allowlist(&mut plan, config, &[], inputs.inherited_names);

    Ok(plan)
}

fn build_harness_plan_for_context(
    invocation: &str,
    harness: HarnessName,
    args: Vec<OsString>,
    config: &Config,
    inputs: LaunchInputs<'_>,
    context: ResolvedContext,
) -> Result<LaunchPlan, LaunchError> {
    let harness_config = config
        .harness
        .get(&harness)
        .ok_or_else(|| LaunchError::UnknownHarness(harness.to_string()))?;
    if !harness_config.supports_context(&context.value) {
        return Err(LaunchError::UnsupportedContext {
            harness,
            context: context.value,
        });
    }
    let launch_path = build_launch_path(config, inputs)?;

    let config_directories = harness_config
        .config_dir
        .as_ref()
        .map(|config_dir| resolve_config_directories(config_dir, invocation, &context, inputs.home))
        .transpose()?
        .unwrap_or_default();

    let project_resolution = tftio_lib::project::resolve(
        inputs.declared_project,
        inputs.project_dir,
        inputs.origin_remote,
        inputs.project_registry,
    );

    let mut plan = LaunchPlan {
        session_id: inputs.session_id.to_string(),
        invocation: invocation.to_string(),
        harness,
        context,
        domains: Vec::new(),
        domain_source: None,
        model: None,
        model_source: None,
        family: None,
        project: project_resolution
            .as_ref()
            .map(|resolution| resolution.slug.clone()),
        project_source: project_resolution
            .as_ref()
            .map(|resolution| resolution.source),
        remote: project_resolution.and_then(|resolution| resolution.remote),
        executable: OsString::from(&harness_config.bin),
        args,
        environment: BTreeMap::new(),
        prompt: None,
        prompt_file: None,
        removed_environment: Vec::new(),
    };

    plan.set_environment("PATH", launch_path, Provenance::Clanker);
    let context_name = plan.context.value.as_str().to_string();
    plan.set_environment(
        CONTEXT_ENVIRONMENT,
        context_name.clone(),
        Provenance::Clanker,
    );
    plan.set_environment(
        CLANKER_CONTEXT_ENVIRONMENT,
        context_name,
        Provenance::Clanker,
    );
    for (environment, path) in config_directories {
        plan.set_environment(environment, path.into_os_string(), Provenance::Clanker);
    }
    for (environment, value) in playwright_environment(&plan.context, inputs.home) {
        plan.set_environment(environment, value, Provenance::Clanker);
    }

    Ok(plan)
}

fn build_launch_path(config: &Config, inputs: LaunchInputs<'_>) -> Result<OsString, LaunchError> {
    let shim_path = expand_path(&config.defaults.shim_path, inputs.home)?;
    let mut path_parts = vec![shim_path];
    if let Some(inherited) = inputs.inherited_path {
        path_parts.extend(std::env::split_paths(inherited));
    }
    std::env::join_paths(path_parts).map_err(Into::into)
}

fn playwright_environment(context: &ResolvedContext, home: &Path) -> [(&'static str, OsString); 2] {
    let profile = home
        .join(PLAYWRIGHT_PROFILE_ROOT)
        .join(context.value.as_str())
        .join(PLAYWRIGHT_BROWSER);
    [
        (
            PLAYWRIGHT_MCP_BROWSER_ENVIRONMENT,
            OsString::from(PLAYWRIGHT_BROWSER),
        ),
        (
            PLAYWRIGHT_MCP_USER_DATA_DIR_ENVIRONMENT,
            profile.into_os_string(),
        ),
    ]
}

/// Resolve every context-derived directory a harness declares.
///
/// Every declared directory is expanded and checked before any is returned, so
/// a partially-created context cannot produce a plan that exports one directory
/// and then fails on the next. Nothing is created: the directory selects the
/// harness credentials, and creating an empty one would turn a misconfiguration
/// into a session with no credentials and no explanation.
fn resolve_config_directories(
    config_dir: &ConfigDirectory,
    invocation: &str,
    context: &ResolvedContext,
    home: &Path,
) -> Result<Vec<(String, PathBuf)>, LaunchError> {
    let mut resolved = Vec::new();
    for (environment, template) in config_dir.entries() {
        let configured = template.replace(CONTEXT_PLACEHOLDER, context.value.as_str());
        let path = expand_path(&configured, home)?;
        if !path.is_dir() {
            return Err(LaunchError::MissingConfigDirectory {
                invocation: invocation.to_string(),
                path,
            });
        }
        resolved.push((environment.to_string(), path));
    }
    Ok(resolved)
}

/// Build a process plan exporting every available harness config directory.
///
/// Harnesses with no directory declaration contribute nothing. A harness with
/// any missing declared directory is returned in the skip list and contributes
/// none of its variables.
///
/// # Errors
/// Returns [`LaunchError`] when context or path resolution fails, or when two
/// harnesses resolve the same variable to different directories.
pub fn build_exec_plan(
    request: &ExecRequest,
    config: &Config,
    inputs: LaunchInputs<'_>,
) -> Result<(ExecPlan, Vec<SkippedConfigDirectory>), LaunchError> {
    let directory_config = read_directory_config(inputs.current_dir)?;
    let context = resolve_context_with_directory(
        request.context.as_ref(),
        inputs.context_environment,
        config,
        directory_config.as_ref(),
    )?;
    let launch_path = build_launch_path(config, inputs)?;
    let mut environment = BTreeMap::new();
    environment.insert(
        OsString::from("PATH"),
        EnvironmentValue {
            value: launch_path,
            provenance: Provenance::Clanker,
        },
    );
    let context_name = context.value.as_str().to_string();
    for key in [CONTEXT_ENVIRONMENT, CLANKER_CONTEXT_ENVIRONMENT] {
        environment.insert(
            OsString::from(key),
            EnvironmentValue {
                value: OsString::from(&context_name),
                provenance: Provenance::Clanker,
            },
        );
    }
    for (key, value) in playwright_environment(&context, inputs.home) {
        environment.insert(
            OsString::from(key),
            EnvironmentValue {
                value,
                provenance: Provenance::Clanker,
            },
        );
    }

    let mut owners: BTreeMap<String, (HarnessName, PathBuf)> = BTreeMap::new();
    let mut skipped = Vec::new();
    for (harness, harness_config) in &config.harness {
        if !harness_config.supports_context(&context.value) {
            continue;
        }
        let Some(config_dir) = &harness_config.config_dir else {
            continue;
        };
        let resolved = match resolve_config_directories(
            config_dir,
            &format!("clanker exec ({harness})"),
            &context,
            inputs.home,
        ) {
            Ok(resolved) => resolved,
            Err(LaunchError::MissingConfigDirectory { path, .. }) => {
                skipped.push(SkippedConfigDirectory {
                    harness: harness.clone(),
                    path,
                });
                continue;
            }
            Err(error) => return Err(error),
        };
        for (variable, path) in resolved {
            if let Some((first_harness, first_path)) = owners.get(&variable) {
                if first_path != &path {
                    return Err(LaunchError::ConfigDirectoryCollision(Box::new(
                        ConfigDirectoryCollisionError {
                            environment: variable,
                            first_harness: first_harness.clone(),
                            first_path: first_path.clone(),
                            second_harness: harness.clone(),
                            second_path: path,
                        },
                    )));
                }
                continue;
            }
            owners.insert(variable.clone(), (harness.clone(), path.clone()));
            environment.insert(
                OsString::from(variable),
                EnvironmentValue {
                    value: path.into_os_string(),
                    provenance: Provenance::Clanker,
                },
            );
        }
    }

    Ok((
        ExecPlan {
            removed_environment: exec_removed_environment(config, &context),
            context,
            executable: request.executable.clone(),
            args: request.args.clone(),
            environment,
        },
        skipped,
    ))
}

fn exec_removed_environment(config: &Config, context: &ResolvedContext) -> Vec<OsString> {
    let mut removed: Vec<OsString> = SESSION_ENVIRONMENTS
        .iter()
        .chain(MNENE_ENVIRONMENTS.iter())
        .map(OsString::from)
        .collect();
    for harness in config
        .harness
        .values()
        .filter(|harness| !harness.supports_context(&context.value))
    {
        if let Some(directory) = &harness.config_dir {
            removed.extend(
                directory
                    .entries()
                    .into_iter()
                    .map(|(key, _)| OsString::from(key)),
            );
        }
    }
    removed
}

/// Build a fully composed command-mode launch plan.
///
/// # Errors
/// Returns [`LaunchError`] for invalid or unknown axes, model/harness
/// incompatibility, context failures, or invalid paths.
pub fn build_command_plan(
    invocation: &str,
    request: &LaunchRequest,
    config: &Config,
    inputs: LaunchInputs<'_>,
    persist_prompt_file: bool,
) -> Result<LaunchPlan, LaunchError> {
    let directory_config = read_directory_config(inputs.current_dir)?;
    let context = resolve_context_with_directory(
        request.context.as_ref(),
        inputs.context_environment,
        config,
        directory_config.as_ref(),
    )?;
    let (domains, domain_source) =
        resolve_domains(request, directory_config.as_ref(), config, inputs)?;
    let domain_configs = domains
        .iter()
        .map(|domain| {
            config
                .domain
                .get(domain)
                .ok_or_else(|| LaunchError::UnknownDomain(domain.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let model = resolve_model(request, directory_config.as_ref(), &domain_configs, inputs)?;
    if let Some(model) = &model
        && !config.model.contains_key(&model.value)
    {
        return Err(LaunchError::UnknownModel(model.value.to_string()));
    }

    let mut plan = build_harness_plan_for_context(
        invocation,
        request.harness.clone(),
        request.passthrough.clone(),
        config,
        inputs,
        context,
    )?;
    plan.domains.clone_from(&domains);
    plan.domain_source = Some(domain_source);
    apply_model_selection(&mut plan, model, &request.harness, config)?;

    let harness_config = config
        .harness
        .get(&request.harness)
        .ok_or_else(|| LaunchError::UnknownHarness(request.harness.to_string()))?;
    let applied_model = plan.model.as_ref().and_then(|name| config.model.get(name));
    let family = applied_model
        .and_then(|model| model.family.as_ref())
        .unwrap_or(&harness_config.family);
    plan.family = Some(family.as_prompter_family().as_str().to_string());

    let arguments = compose_arguments(
        &mut plan,
        ArgumentSources {
            request,
            harness_config,
            domain_configs: &domain_configs,
            applied_model,
            family,
            config,
            inputs,
            persist_prompt_file,
        },
    )?;
    plan.args = arguments;

    apply_domain_environment(&mut plan, &domain_configs);
    apply_credential_allowlist(&mut plan, config, &domain_configs, inputs.inherited_names);
    if let Some(model_config) = applied_model {
        for (key, value) in &model_config.env {
            plan.set_environment(key.clone(), value.clone(), Provenance::Model);
        }
    }

    if request.sandbox {
        let wrapper = expand_path(&config.defaults.sandbox_wrapper, inputs.home)?;
        let mut sandbox_arguments = vec![plan.executable.clone()];
        sandbox_arguments.append(&mut plan.args);
        plan.executable = wrapper.into_os_string();
        plan.args = sandbox_arguments;
    }

    apply_session_environment(&mut plan);
    Ok(plan)
}

/// Bind a resolved model to the plan, rejecting one the harness cannot run.
fn apply_model_selection(
    plan: &mut LaunchPlan,
    model: Option<axis::Resolved<ModelName>>,
    harness: &HarnessName,
    config: &Config,
) -> Result<(), LaunchError> {
    let Some(resolved) = model else {
        return Ok(());
    };
    let model_config = config
        .model
        .get(&resolved.value)
        .ok_or_else(|| LaunchError::UnknownModel(resolved.value.to_string()))?;
    if &model_config.harness == harness {
        plan.model = Some(resolved.value);
        plan.model_source = Some(resolved.source);
        Ok(())
    } else {
        Err(LaunchError::ModelHarnessMismatch {
            model: resolved.value,
            required: model_config.harness.clone(),
            selected: harness.clone(),
        })
    }
}

/// Everything argument assembly reads, gathered so the caller stays legible.
#[derive(Clone, Copy)]
struct ArgumentSources<'a> {
    request: &'a LaunchRequest,
    harness_config: &'a HarnessConfig,
    domain_configs: &'a [&'a DomainConfig],
    applied_model: Option<&'a ModelConfig>,
    family: &'a ModelFamily,
    config: &'a Config,
    inputs: LaunchInputs<'a>,
    persist_prompt_file: bool,
}

/// Assemble harness defaults, injected prompt, model arguments, and passthrough.
///
/// Prompt injection also mutates the plan, since `env-file` harnesses receive
/// their prompt through the environment rather than through argv.
fn compose_arguments(
    plan: &mut LaunchPlan,
    sources: ArgumentSources<'_>,
) -> Result<Vec<OsString>, LaunchError> {
    let request = sources.request;
    let mut arguments: Vec<OsString> = match request.default_args {
        DefaultArgsMode::Suppress => Vec::new(),
        DefaultArgsMode::Apply => sources
            .harness_config
            .default_args
            .iter()
            .map(OsString::from)
            .collect(),
    };

    if !request.no_prompt {
        let mut profiles = sources
            .domain_configs
            .iter()
            .flat_map(|domain| domain.profiles.iter().cloned())
            .collect::<Vec<_>>();
        profiles.extend(request.profiles.iter().cloned());
        let injection = compose_and_inject(
            &profiles,
            sources.family,
            sources.harness_config,
            sources.config,
            sources.inputs.home,
            sources.persist_prompt_file,
        )?;
        arguments.extend(injection.arguments);
        for (key, value) in injection.environment {
            plan.set_environment(key, value, Provenance::Injection);
        }
        plan.prompt = injection.prompt;
        plan.prompt_file = injection.cache_path;
    }

    if let Some(model_config) = sources.applied_model {
        arguments.extend(model_config.harness_args.iter().map(OsString::from));
    }
    arguments.append(&mut plan.args);
    Ok(arguments)
}

fn resolve_domains(
    request: &LaunchRequest,
    directory_config: Option<&crate::context::DirectoryConfig>,
    config: &Config,
    inputs: LaunchInputs<'_>,
) -> Result<(Vec<DomainName>, AxisSource), LaunchError> {
    let (domains, source) = if request.domains.is_empty() {
        let resolved = axis::resolve(
            axis::DOMAIN,
            None,
            directory_config.and_then(|directory| directory.domain.clone()),
            inputs.domain_environment,
            DomainName::new,
            Some(config.defaults.domain.clone()),
        )?
        .ok_or_else(|| LaunchError::UnknownDomain(config.defaults.domain.to_string()))?;
        (vec![resolved.value], resolved.source)
    } else {
        (request.domains.clone(), AxisSource::CommandLine)
    };

    for domain in &domains {
        if !config.domain.contains_key(domain) {
            return Err(LaunchError::UnknownDomain(domain.to_string()));
        }
    }
    Ok((domains, source))
}

fn resolve_model(
    request: &LaunchRequest,
    directory_config: Option<&crate::context::DirectoryConfig>,
    domains: &[&DomainConfig],
    inputs: LaunchInputs<'_>,
) -> Result<Option<axis::Resolved<ModelName>>, LaunchError> {
    axis::resolve(
        axis::MODEL,
        request.model.clone(),
        directory_config.and_then(|directory| directory.model.clone()),
        inputs.model_environment,
        ModelName::new,
        domains
            .iter()
            .find_map(|domain| domain.default_model.clone()),
    )
    .map_err(Into::into)
}

/// Remove every inherited variable the resolved domains did not grant.
///
/// The effective allowlist is the union of `defaults.passthrough_environment` -
/// what any session needs to function - and each resolved domain's
/// `credentials` list. Everything else inherited is removed before the harness
/// starts.
///
/// Removal rather than a wholesale clear, and names rather than values, is
/// deliberate: clanker never reads the value of a credential it is not
/// granting, so a secret it does not pass on never enters this process. The
/// launched harness receives granted variables untouched, straight from the
/// ambient environment.
///
/// clanker's own overrides are applied *after* removal during exec, so they do
/// not need allowlist entries and cannot be removed by this pass.
fn apply_credential_allowlist(
    plan: &mut LaunchPlan,
    config: &Config,
    domains: &[&DomainConfig],
    inherited_names: &[OsString],
) {
    let mut allowed: BTreeSet<OsString> = config
        .defaults
        .passthrough_environment
        .iter()
        .map(OsString::from)
        .collect();
    for domain in domains {
        allowed.extend(domain.credentials.iter().map(OsString::from));
    }

    plan.removed_environment = inherited_names
        .iter()
        .filter(|name| !allowed.contains(*name))
        .cloned()
        .collect();
}

fn apply_domain_environment(plan: &mut LaunchPlan, domains: &[&DomainConfig]) {
    let mut environment: BTreeMap<String, String> = BTreeMap::new();
    for domain in domains {
        for (key, value) in &domain.env {
            environment
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
    }
    for (key, value) in environment {
        plan.set_environment(key, value, Provenance::Domain);
    }
}

fn apply_session_environment(plan: &mut LaunchPlan) {
    let domains = plan
        .domains
        .iter()
        .map(DomainName::as_str)
        .collect::<Vec<_>>()
        .join(",");
    let values = [
        (SESSION_ACTIVE_ENVIRONMENT, SESSION_MARKER_VALUE.to_string()),
        (SESSION_VERSION_ENVIRONMENT, SESSION_VERSION.to_string()),
        (SESSION_ID_ENVIRONMENT, plan.session_id.clone()),
        (SESSION_INVOCATION_ENVIRONMENT, plan.invocation.clone()),
        (SESSION_HARNESS_ENVIRONMENT, plan.harness.to_string()),
        (SESSION_CONTEXT_ENVIRONMENT, plan.context.value.to_string()),
        (
            SESSION_CONTEXT_SOURCE_ENVIRONMENT,
            plan.context.source.label(axis::CONTEXT).to_string(),
        ),
        (SESSION_DOMAINS_ENVIRONMENT, domains),
        (
            SESSION_DOMAIN_SOURCE_ENVIRONMENT,
            plan.domain_source
                .map(|source| source.label(axis::DOMAIN))
                .unwrap_or_default()
                .to_string(),
        ),
        (
            SESSION_MODEL_ENVIRONMENT,
            plan.model
                .as_ref()
                .map(ModelName::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
        (
            SESSION_MODEL_SOURCE_ENVIRONMENT,
            plan.model_source
                .map(|source| source.label(axis::MODEL))
                .unwrap_or_default()
                .to_string(),
        ),
        (
            SESSION_FAMILY_ENVIRONMENT,
            plan.family.clone().unwrap_or_default(),
        ),
        (
            SESSION_PROJECT_ENVIRONMENT,
            plan.project
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
        ),
        (
            SESSION_PROJECT_SOURCE_ENVIRONMENT,
            plan.project_source
                .map(tftio_lib::project::Source::label)
                .unwrap_or_default()
                .to_string(),
        ),
        (
            SESSION_REMOTE_ENVIRONMENT,
            plan.remote
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
        ),
    ];
    for (key, value) in values {
        plan.set_environment(key, value, Provenance::Session);
    }
    plan.set_environment(
        MNENE_AGENT_ENVIRONMENT,
        plan.harness.to_string(),
        Provenance::Session,
    );
    plan.set_environment(
        MNENE_CONTEXT_ENVIRONMENT,
        plan.context.value.to_string(),
        Provenance::Session,
    );
    plan.set_environment(
        MNENE_SESSION_ENVIRONMENT,
        plan.session_id.clone(),
        Provenance::Session,
    );
}

/// Replace the current process with the resolved harness.
///
/// # Errors
/// Returns [`LaunchError::Exec`] only when the operating system rejects the
/// exec; successful execution never returns.
#[cfg(unix)]
pub fn execute_plan(plan: &impl ExecutablePlan) -> Result<i32, LaunchError> {
    use std::os::unix::process::CommandExt;

    let mut command = Command::new(plan.executable());
    command.args(plan.args());
    for key in plan.removed_environment() {
        command.env_remove(key);
    }
    for (key, entry) in plan.environment() {
        command.env(key, &entry.value);
    }
    let executable = plan.executable().to_string_lossy().into_owned();
    let source = command.exec();
    Err(LaunchError::Exec { executable, source })
}

pub(crate) fn expand_path(value: &str, home: &Path) -> Result<PathBuf, LaunchError> {
    if value == "~" {
        return Ok(home.to_path_buf());
    }
    if let Some(relative) = value.strip_prefix("~/") {
        return Ok(home.join(relative));
    }
    if value.starts_with('~') {
        return Err(LaunchError::UnsupportedTilde(value.to_string()));
    }
    Ok(PathBuf::from(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{DefaultArgsMode, ExecRequest, LaunchRequest};
    use crate::config::{Config, ContextName, HarnessName, ModelName, SingleDirectory};
    use std::ffi::OsStr;
    use std::fs;
    use tempfile::TempDir;

    /// Fixed launch identifier, so plan construction stays assertable
    /// against a known value.
    const TEST_SESSION_ID: &str = "01a06ebb-0000-7000-8000-000000000000";

    const CONFIG: &str = r#"
[defaults]
domain = "eng"
contexts = ["personal", "work"]
shim_path = "~/.local/clankers/bin"
sandbox_wrapper = "~/run-sandboxed.sh"
prompter_bundle = "~/.local/prompter"
prompt_cache = "~/.cache/clanker"
prompt_cache_ttl_seconds = 1

[harness.claude]
bin = "claude"
family = "claude"
default_args = ["--flag"]
[harness.claude.injection]
kind = "arg-text"
args = ["{text}"]

[harness.codex]
bin = "codex"
family = "gpt"
default_args = []
[harness.codex.injection]
kind = "arg-text"
args = ["{text}"]

[model.alt]
harness = "claude"
family = "gpt"
env = { LITERAL = "value" }
harness_args = []

[domain.eng]
profiles = ["core.base"]
env = {}
"#;

    const EXEC_CONFIG: &str = r#"
[defaults]
domain = "eng"
contexts = ["personal"]
shim_path = "~/.local/clankers/bin"
sandbox_wrapper = "~/run-sandboxed.sh"
prompter_bundle = "~/.local/prompter"
prompt_cache = "~/.cache/clanker"
prompt_cache_ttl_seconds = 1

[harness.alpha]
bin = "alpha"
family = "gpt"
default_args = []
[harness.alpha.injection]
kind = "arg-text"
args = ["{text}"]
[harness.alpha.config_dir]
environment = "ALPHA_HOME"
path = "~/.alpha/{context}"

[harness.beta]
bin = "beta"
family = "gpt"
default_args = []
[harness.beta.injection]
kind = "arg-text"
args = ["{text}"]
[harness.beta.config_dir]
environment = "BETA_HOME"
path = "~/.beta/{context}"

[harness.gamma]
bin = "gamma"
family = "gpt"
default_args = []
[harness.gamma.injection]
kind = "arg-text"
args = ["{text}"]
[harness.gamma.config_dir]
environment = "GAMMA_HOME"
path = "~/.gamma/{context}"

[harness.plain]
bin = "plain"
family = "gpt"
default_args = []
[harness.plain.injection]
kind = "arg-text"
args = ["{text}"]

[model]

[domain.eng]
profiles = ["core.base"]
env = {}
"#;

    fn config() -> Config {
        toml::from_str(CONFIG).unwrap()
    }

    fn exec_config() -> Config {
        toml::from_str(EXEC_CONFIG).unwrap()
    }

    fn exec_home() -> TempDir {
        let home = TempDir::new().unwrap();
        for directory in [
            ".alpha/personal",
            ".beta/personal",
            ".gamma/personal",
            ".local/clankers/bin",
        ] {
            fs::create_dir_all(home.path().join(directory)).unwrap();
        }
        home
    }

    fn exec_request() -> ExecRequest {
        ExecRequest {
            context: None,
            executable: OsString::from("/usr/bin/env"),
            args: Vec::new(),
        }
    }

    /// An empty project registry shared by every test's [`LaunchInputs`],
    /// since these unit tests exercise axis resolution rather than project
    /// resolution -- that lives in `tftio_lib::project` and is exercised
    /// end-to-end by `tests/symlink_mode.rs`.
    fn empty_registry() -> &'static tftio_lib::project::Registry {
        static REGISTRY: std::sync::OnceLock<tftio_lib::project::Registry> =
            std::sync::OnceLock::new();
        REGISTRY.get_or_init(tftio_lib::project::Registry::default)
    }

    fn inputs<'a>(home: &'a Path, current_dir: &'a Path) -> LaunchInputs<'a> {
        LaunchInputs {
            inherited_names: &[],
            home,
            session_id: TEST_SESSION_ID,
            current_dir,
            context_environment: Some("personal"),
            domain_environment: None,
            model_environment: None,
            inherited_path: None,
            project_registry: empty_registry(),
            declared_project: None,
            origin_remote: None,
            project_dir: current_dir,
        }
    }

    fn request(harness: &str) -> LaunchRequest {
        LaunchRequest {
            harness: HarnessName::new(harness).unwrap(),
            context: None,
            domains: Vec::new(),
            model: None,
            profiles: Vec::new(),
            sandbox: false,
            no_prompt: true,
            default_args: DefaultArgsMode::Apply,
            dry_run: true,
            passthrough: Vec::new(),
        }
    }

    fn value<'a>(plan: &'a LaunchPlan, key: &str) -> Option<&'a EnvironmentValue> {
        plan.environment.get(OsStr::new(key))
    }

    fn exec_value<'a>(plan: &'a ExecPlan, key: &str) -> Option<&'a EnvironmentValue> {
        plan.environment.get(OsStr::new(key))
    }

    #[test]
    fn exec_plan_unions_every_configured_harness_directory() {
        let home = exec_home();
        let (plan, skipped) = build_exec_plan(
            &exec_request(),
            &exec_config(),
            inputs(home.path(), home.path()),
        )
        .unwrap();

        assert!(skipped.is_empty());
        for (variable, relative) in [
            ("ALPHA_HOME", ".alpha/personal"),
            ("BETA_HOME", ".beta/personal"),
            ("GAMMA_HOME", ".gamma/personal"),
        ] {
            assert_eq!(
                exec_value(&plan, variable).map(|entry| entry.value.clone()),
                Some(home.path().join(relative).into_os_string())
            );
        }
    }

    #[test]
    fn exec_plan_ignores_a_harness_without_config_directories() {
        let home = exec_home();
        let (plan, skipped) = build_exec_plan(
            &exec_request(),
            &exec_config(),
            inputs(home.path(), home.path()),
        )
        .unwrap();

        assert!(skipped.is_empty());
        assert_eq!(plan.environment.len(), 8);
    }

    #[test]
    fn exec_plan_skips_one_harness_whose_directory_is_missing() {
        let home = exec_home();
        fs::remove_dir(home.path().join(".beta/personal")).unwrap();
        let (plan, skipped) = build_exec_plan(
            &exec_request(),
            &exec_config(),
            inputs(home.path(), home.path()),
        )
        .unwrap();

        assert_eq!(
            skipped,
            [SkippedConfigDirectory {
                harness: HarnessName::new("beta").unwrap(),
                path: home.path().join(".beta/personal"),
            }]
        );
        assert!(exec_value(&plan, "BETA_HOME").is_none());
        assert!(exec_value(&plan, "ALPHA_HOME").is_some());
        assert!(exec_value(&plan, "GAMMA_HOME").is_some());
    }

    #[test]
    fn exec_plan_rejects_only_different_paths_for_one_variable() {
        let home = exec_home();
        let mut config = exec_config();
        config.harness.get_mut("beta").unwrap().config_dir =
            Some(ConfigDirectory::Single(SingleDirectory {
                environment: "ALPHA_HOME".to_string(),
                path: "~/.beta/{context}".to_string(),
            }));
        let error = build_exec_plan(&exec_request(), &config, inputs(home.path(), home.path()))
            .unwrap_err();
        assert!(matches!(
            error,
            LaunchError::ConfigDirectoryCollision(details)
                if details.environment == "ALPHA_HOME"
                    && details.first_harness.as_str() == "alpha"
                    && details.second_harness.as_str() == "beta"
        ));

        config.harness.get_mut("beta").unwrap().config_dir =
            Some(ConfigDirectory::Single(SingleDirectory {
                environment: "ALPHA_HOME".to_string(),
                path: "~/.alpha/{context}".to_string(),
            }));
        let (plan, skipped) =
            build_exec_plan(&exec_request(), &config, inputs(home.path(), home.path())).unwrap();
        assert!(skipped.is_empty());
        assert_eq!(
            exec_value(&plan, "ALPHA_HOME").map(|entry| entry.value.clone()),
            Some(home.path().join(".alpha/personal").into_os_string())
        );
    }

    #[test]
    fn exec_plan_adds_context_and_path_without_session_markers() {
        let home = exec_home();
        let inherited = std::env::join_paths([Path::new("/inherited/bin")]).unwrap();
        let mut exec_inputs = inputs(home.path(), home.path());
        exec_inputs.inherited_path = Some(&inherited);
        let (plan, skipped) =
            build_exec_plan(&exec_request(), &exec_config(), exec_inputs).unwrap();

        assert!(skipped.is_empty());
        assert_eq!(
            std::env::split_paths(&exec_value(&plan, "PATH").expect("PATH is present").value)
                .next(),
            Some(home.path().join(".local/clankers/bin"))
        );
        assert_eq!(
            exec_value(&plan, "CONTEXT").map(|entry| entry.value.clone()),
            Some(OsString::from("personal"))
        );
        assert_eq!(
            exec_value(&plan, "CLANKER_CONTEXT").map(|entry| entry.value.clone()),
            Some(OsString::from("personal"))
        );
        assert_eq!(
            exec_value(&plan, PLAYWRIGHT_MCP_BROWSER_ENVIRONMENT),
            Some(&EnvironmentValue {
                value: OsString::from("chromium"),
                provenance: Provenance::Clanker,
            })
        );
        assert_eq!(
            exec_value(&plan, PLAYWRIGHT_MCP_USER_DATA_DIR_ENVIRONMENT),
            Some(&EnvironmentValue {
                value: home
                    .path()
                    .join(".local/share/clanker/playwright/personal/chromium")
                    .into_os_string(),
                provenance: Provenance::Clanker,
            })
        );
        assert!(
            plan.environment
                .keys()
                .all(|key| !key.to_string_lossy().starts_with("CLANKER_SESSION"))
        );
    }

    #[test]
    fn expands_only_supported_tilde_forms() {
        let home = Path::new("/tmp/home");
        assert_eq!(expand_path("~", home).unwrap(), home);
        assert_eq!(
            expand_path("~/.config/tool", home).unwrap(),
            home.join(".config/tool")
        );
        assert_eq!(
            expand_path("relative/path", home).unwrap(),
            PathBuf::from("relative/path")
        );
        assert!(expand_path("~someone/tool", home).is_err());
    }

    #[test]
    fn symlink_plan_resolves_harness_and_session_markers() {
        let config = config();
        let plan = build_symlink_plan(
            "claude-launch",
            vec![OsString::from("--version")],
            &config,
            inputs(Path::new("/home"), Path::new("/work")),
        )
        .unwrap();

        assert_eq!(plan.harness.as_str(), "claude");
        assert_eq!(plan.executable, OsString::from("claude"));
        assert_eq!(plan.args, vec![OsString::from("--version")]);
        assert_eq!(
            value(&plan, "CLANKER_SESSION").map(|entry| entry.value.clone()),
            Some(OsString::from("1"))
        );
        assert_eq!(
            value(&plan, "CLANKER_SESSION_VERSION").map(|entry| entry.value.clone()),
            Some(OsString::from("5"))
        );
        assert_eq!(
            value(&plan, SESSION_ID_ENVIRONMENT).map(|entry| entry.value.clone()),
            Some(OsString::from(TEST_SESSION_ID))
        );
        assert_eq!(
            value(&plan, MNENE_AGENT_ENVIRONMENT).map(|entry| entry.value.clone()),
            Some(OsString::from("claude"))
        );
        assert_eq!(
            value(&plan, MNENE_CONTEXT_ENVIRONMENT).map(|entry| entry.value.clone()),
            Some(OsString::from("personal"))
        );
        assert_eq!(
            value(&plan, MNENE_SESSION_ENVIRONMENT).map(|entry| entry.value.clone()),
            Some(OsString::from(TEST_SESSION_ID))
        );
        assert_eq!(value(&plan, "MNENE_SCOPE"), None);
    }

    #[test]
    fn exec_plans_neither_mint_nor_keep_a_session_identifier() {
        let home = exec_home();
        let config = exec_config();
        let (plan, _) = build_exec_plan(
            &exec_request(),
            &config,
            inputs(home.path(), Path::new("/work")),
        )
        .unwrap();

        assert_eq!(exec_value(&plan, SESSION_ID_ENVIRONMENT), None);
        assert!(
            plan.removed_environment()
                .contains(&OsString::from(SESSION_ID_ENVIRONMENT))
        );
        assert_eq!(plan.removed_environment().len(), 18);
    }

    #[test]
    fn both_context_variables_are_exported_and_owned_by_clanker() {
        let config = config();
        let plan = build_symlink_plan(
            "claude-launch",
            Vec::new(),
            &config,
            inputs(Path::new("/home"), Path::new("/work")),
        )
        .unwrap();

        for key in ["CONTEXT", "CLANKER_CONTEXT"] {
            let entry = value(&plan, key).expect("context variable is exported");
            assert_eq!(entry.value, OsString::from("personal"));
            assert_eq!(entry.provenance, Provenance::Clanker);
        }
    }

    #[test]
    fn playwright_profile_is_fixed_beneath_home_and_scoped_by_context() {
        let config = config();
        let personal = build_symlink_plan(
            "claude-launch",
            Vec::new(),
            &config,
            inputs(Path::new("/home"), Path::new("/work")),
        )
        .unwrap();
        let mut work_inputs = inputs(Path::new("/home"), Path::new("/work"));
        work_inputs.context_environment = Some("work");
        let work = build_symlink_plan("claude-launch", Vec::new(), &config, work_inputs).unwrap();

        for plan in [&personal, &work] {
            let browser = value(plan, PLAYWRIGHT_MCP_BROWSER_ENVIRONMENT)
                .expect("Playwright browser is exported");
            assert_eq!(browser.value, OsString::from("chromium"));
            assert_eq!(browser.provenance, Provenance::Clanker);
        }
        assert_eq!(
            value(&personal, PLAYWRIGHT_MCP_USER_DATA_DIR_ENVIRONMENT),
            Some(&EnvironmentValue {
                value: OsString::from("/home/.local/share/clanker/playwright/personal/chromium"),
                provenance: Provenance::Clanker,
            })
        );
        assert_eq!(
            value(&work, PLAYWRIGHT_MCP_USER_DATA_DIR_ENVIRONMENT),
            Some(&EnvironmentValue {
                value: OsString::from("/home/.local/share/clanker/playwright/work/chromium"),
                provenance: Provenance::Clanker,
            })
        );
        assert_ne!(
            value(&personal, PLAYWRIGHT_MCP_USER_DATA_DIR_ENVIRONMENT),
            value(&work, PLAYWRIGHT_MCP_USER_DATA_DIR_ENVIRONMENT)
        );
    }

    #[test]
    fn symlink_plan_rejects_non_launch_invocation() {
        let config = config();
        let error = build_symlink_plan(
            "clanker",
            Vec::new(),
            &config,
            inputs(Path::new("/home"), Path::new("/work")),
        )
        .unwrap_err();
        assert!(matches!(error, LaunchError::InvalidInvocation(_)));
    }

    #[test]
    fn a_launch_without_any_context_tier_fails_closed() {
        let config = config();
        let mut inputs = inputs(Path::new("/home"), Path::new("/work"));
        inputs.context_environment = None;

        let error = build_symlink_plan("claude-launch", Vec::new(), &config, inputs).unwrap_err();

        assert!(matches!(
            error,
            LaunchError::Context(ContextError::Unresolved)
        ));
    }

    #[test]
    fn command_plan_wraps_executable_with_sandbox() {
        let config = config();
        let mut request = request("claude");
        request.sandbox = true;
        let plan = build_command_plan(
            "clanker claude",
            &request,
            &config,
            inputs(Path::new("/home"), Path::new("/work")),
            false,
        )
        .unwrap();

        assert_eq!(plan.executable, OsString::from("/home/run-sandboxed.sh"));
        assert_eq!(plan.args.first(), Some(&OsString::from("claude")));
    }

    #[test]
    fn command_plan_rejects_unknown_model() {
        let config = config();
        let mut request = request("claude");
        request.model = Some(ModelName::new("ghost").unwrap());
        let error = build_command_plan(
            "clanker claude",
            &request,
            &config,
            inputs(Path::new("/home"), Path::new("/work")),
            false,
        )
        .unwrap_err();
        assert!(matches!(error, LaunchError::UnknownModel(_)));
    }

    #[test]
    fn command_plan_rejects_unknown_domain() {
        let config = config();
        let mut request = request("claude");
        request.domains = vec![DomainName::new("ghost").unwrap()];
        let error = build_command_plan(
            "clanker claude",
            &request,
            &config,
            inputs(Path::new("/home"), Path::new("/work")),
            false,
        )
        .unwrap_err();
        assert!(matches!(error, LaunchError::UnknownDomain(_)));
    }

    #[test]
    fn command_plan_rejects_model_harness_mismatch() {
        let config = config();
        let mut request = request("codex");
        request.model = Some(ModelName::new("alt").unwrap());
        let error = build_command_plan(
            "clanker codex",
            &request,
            &config,
            inputs(Path::new("/home"), Path::new("/work")),
            false,
        )
        .unwrap_err();
        assert!(matches!(error, LaunchError::ModelHarnessMismatch { .. }));
    }

    #[test]
    fn model_environment_carries_model_provenance() {
        let config = config();
        let mut request = request("claude");
        request.model = Some(ModelName::new("alt").unwrap());
        let plan = build_command_plan(
            "clanker claude",
            &request,
            &config,
            inputs(Path::new("/home"), Path::new("/work")),
            false,
        )
        .unwrap();

        let entry = value(&plan, "LITERAL").expect("model env is applied");
        assert_eq!(entry.value, OsString::from("value"));
        assert_eq!(entry.provenance, Provenance::Model);
        assert_eq!(plan.model_source, Some(AxisSource::CommandLine));
    }

    #[test]
    fn every_axis_reports_the_tier_that_supplied_it() {
        let config = config();
        let mut inputs = inputs(Path::new("/home"), Path::new("/work"));
        inputs.domain_environment = Some("eng");
        let plan = build_command_plan("clanker claude", &request("claude"), &config, inputs, false)
            .unwrap();

        assert_eq!(plan.context.source, AxisSource::Environment);
        assert_eq!(plan.domain_source, Some(AxisSource::Environment));
        assert_eq!(plan.model_source, None);

        let output = LaunchPlanOutput::from(&plan);
        let serialized = serde_json::to_string(&output).unwrap();
        assert!(serialized.contains("\"context_source\":\"CLANKER_CONTEXT\""));
        assert!(serialized.contains("\"domain_source\":\"CLANKER_DOMAIN\""));
    }

    /// Build inputs whose ambient environment carries the given variable names.
    fn inputs_with_ambient<'a>(
        home: &'a Path,
        current_dir: &'a Path,
        names: &'a [OsString],
    ) -> LaunchInputs<'a> {
        LaunchInputs {
            inherited_names: names,
            ..inputs(home, current_dir)
        }
    }

    fn ambient(names: &[&str]) -> Vec<OsString> {
        names.iter().map(OsString::from).collect()
    }

    fn plan_with_ambient(config: &Config, names: &[OsString]) -> LaunchPlan {
        build_command_plan(
            "clanker claude",
            &request("claude"),
            config,
            inputs_with_ambient(Path::new("/home"), Path::new("/work"), names),
            false,
        )
        .unwrap()
    }

    #[test]
    fn a_domain_granting_no_credentials_removes_every_one_of_them() {
        // A representative spread of credential variable names: personal access
        // tokens, API tokens, registry tokens, and client secrets.
        let names = ambient(&[
            "ISSUE_TRACKER_PAT",
            "CARGO_REGISTRY_TOKEN",
            "DNS_API_TOKEN",
            "GITLAB_PAT",
            "SEARCH_API_TOKEN",
            "WAREHOUSE_TOKEN",
            "NPM_TOKEN",
            "SCANNER_CLIENT_SECRET",
            "HOME",
            "PATH",
            "TERM",
        ]);
        let plan = plan_with_ambient(&config(), &names);
        let removed: BTreeSet<&OsString> = plan.removed_environment.iter().collect();

        for credential in [
            "ISSUE_TRACKER_PAT",
            "CARGO_REGISTRY_TOKEN",
            "DNS_API_TOKEN",
            "GITLAB_PAT",
            "SEARCH_API_TOKEN",
            "WAREHOUSE_TOKEN",
            "NPM_TOKEN",
            "SCANNER_CLIENT_SECRET",
        ] {
            assert!(
                removed.contains(&OsString::from(credential)),
                "{credential} must not reach a session whose domain did not grant it"
            );
        }

        // What a session needs to function is untouched.
        for passthrough in ["HOME", "PATH", "TERM"] {
            assert!(
                !removed.contains(&OsString::from(passthrough)),
                "{passthrough} must pass through"
            );
        }
    }

    #[test]
    fn a_domain_granting_one_credential_sees_that_one_and_no_others() {
        let mut config = config();
        let domain = config.defaults.domain.clone();
        config
            .domain
            .get_mut(&domain)
            .unwrap()
            .credentials
            .push("ISSUE_TRACKER_PAT".to_owned());

        let names = ambient(&["ISSUE_TRACKER_PAT", "GITLAB_PAT", "NPM_TOKEN", "PATH"]);
        let plan = plan_with_ambient(&config, &names);
        let removed: BTreeSet<&OsString> = plan.removed_environment.iter().collect();

        assert!(
            !removed.contains(&OsString::from("ISSUE_TRACKER_PAT")),
            "the granted credential passes through"
        );
        for denied in ["GITLAB_PAT", "NPM_TOKEN"] {
            assert!(
                removed.contains(&OsString::from(denied)),
                "{denied} was not granted and must be removed"
            );
        }
    }

    #[test]
    fn the_default_passthrough_grants_no_authority() {
        // SSH_AUTH_SOCK confers the ability to authenticate and sign as the
        // operator. It must never be granted by default; a domain that needs it
        // has to name it.
        let names = ambient(&["SSH_AUTH_SOCK", "PATH"]);
        let plan = plan_with_ambient(&config(), &names);

        assert!(
            plan.removed_environment
                .contains(&OsString::from("SSH_AUTH_SOCK")),
            "SSH_AUTH_SOCK must not be inherited by default"
        );
    }

    #[test]
    fn symlink_mode_grants_no_credentials() {
        let names = ambient(&["ISSUE_TRACKER_PAT", "GITLAB_PAT", "PATH", "HOME"]);
        let plan = build_symlink_plan(
            "claude-launch",
            Vec::new(),
            &config(),
            inputs_with_ambient(Path::new("/home"), Path::new("/work"), &names),
        )
        .unwrap();
        let removed: BTreeSet<&OsString> = plan.removed_environment.iter().collect();

        assert!(plan.domains.is_empty(), "symlink mode resolves no domains");
        for denied in ["ISSUE_TRACKER_PAT", "GITLAB_PAT"] {
            assert!(
                removed.contains(&OsString::from(denied)),
                "{denied} must not reach a symlink-mode session"
            );
        }
        for passthrough in ["PATH", "HOME"] {
            assert!(!removed.contains(&OsString::from(passthrough)));
        }
    }

    #[test]
    fn an_empty_ambient_environment_removes_nothing() {
        let plan = plan_with_ambient(&config(), &[]);
        assert!(plan.removed_environment.is_empty());
    }

    #[test]
    fn a_configured_default_domain_reports_its_configured_source() {
        let config = config();
        let plan = build_command_plan(
            "clanker claude",
            &request("claude"),
            &config,
            inputs(Path::new("/home"), Path::new("/work")),
            false,
        )
        .unwrap();

        assert_eq!(plan.domain_source, Some(AxisSource::ConfiguredDefault));
        assert_eq!(
            value(&plan, "CLANKER_SESSION_DOMAIN_SOURCE").map(|entry| entry.value.clone()),
            Some(OsString::from("defaults.domain"))
        );
    }

    #[test]
    fn fallback_domain_rejects_invalid_environment_name() {
        let config = config();
        let mut inputs = inputs(Path::new("/home"), Path::new("/work"));
        inputs.domain_environment = Some("not a name");
        let error =
            build_command_plan("clanker claude", &request("claude"), &config, inputs, false)
                .unwrap_err();
        assert!(matches!(error, LaunchError::Axis(_)));
    }

    #[test]
    fn a_registered_path_resolves_the_project_and_exports_its_markers() {
        let config = config();
        let home = Path::new("/home");
        let work = Path::new("/work");
        let registry: tftio_lib::project::Registry = {
            let dir = TempDir::new().unwrap();
            fs::write(
                dir.path().join("projects.toml"),
                format!("[project.kb]\npaths = [\"{}\"]\n", work.display()),
            )
            .unwrap();
            tftio_lib::project::load_registry(dir.path(), None).unwrap()
        };
        let plan_inputs = LaunchInputs {
            project_registry: &registry,
            ..inputs(home, work)
        };
        let plan = build_symlink_plan("claude-launch", Vec::new(), &config, plan_inputs).unwrap();

        assert_eq!(
            plan.project.as_ref().map(ToString::to_string).as_deref(),
            Some("kb")
        );
        assert_eq!(plan.project_source, Some(tftio_lib::project::Source::Path));
        assert_eq!(plan.remote, None);
        assert_eq!(
            value(&plan, "CLANKER_SESSION_PROJECT").map(|entry| entry.value.clone()),
            Some(OsString::from("kb"))
        );
        assert_eq!(
            value(&plan, "CLANKER_SESSION_PROJECT_SOURCE").map(|entry| entry.value.clone()),
            Some(OsString::from("path"))
        );
        assert_eq!(
            value(&plan, "CLANKER_SESSION_REMOTE").map(|entry| entry.value.clone()),
            Some(OsString::new())
        );
    }

    #[test]
    fn an_unresolved_project_exports_empty_markers() {
        let config = config();
        let plan = build_symlink_plan(
            "claude-launch",
            Vec::new(),
            &config,
            inputs(Path::new("/home"), Path::new("/work")),
        )
        .unwrap();

        assert_eq!(plan.project, None);
        assert_eq!(plan.project_source, None);
        assert_eq!(plan.remote, None);
        for key in [
            "CLANKER_SESSION_PROJECT",
            "CLANKER_SESSION_PROJECT_SOURCE",
            "CLANKER_SESSION_REMOTE",
        ] {
            assert_eq!(
                value(&plan, key).map(|entry| entry.value.clone()),
                Some(OsString::new())
            );
        }
    }

    #[test]
    fn an_explicit_context_flag_outranks_the_environment() {
        let config = config();
        let mut request = request("claude");
        request.context = Some(ContextName::new("personal").unwrap());
        let plan = build_command_plan(
            "clanker claude",
            &request,
            &config,
            inputs(Path::new("/home"), Path::new("/work")),
            false,
        )
        .unwrap();

        assert_eq!(plan.context.source, AxisSource::CommandLine);
    }
}
