//! Runtime-configured multi-harness launcher.
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

pub mod axis;
pub mod cli;
pub mod config;
pub mod context;
pub mod doctor;
pub mod error;
pub mod launch;
pub mod prompt;
pub mod promptlib;
mod session;

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use cli::{
    Cli, Command, ProjectCommand, PromptCommand, parse_exec, parse_external_launch, parse_launch,
};
use config::{LoadedConfig, load_config};
use doctor::{Doctor, DoctorInputs};
use error::ClankerError;
use launch::{
    LaunchInputs, LaunchPlan, LaunchPlanOutput, build_command_plan, build_exec_plan,
    build_symlink_plan, execute_plan,
};
use serde::Serialize;
use session::CurrentSession;
use tftio_lib::{
    AgentCapability, AgentSurfaceSpec, CommandSelector, FatalCliError, FlagSelector, JsonOutput,
    LicenseType, ProcessEnv, StandardCommand, ToolSpec, map_standard_command, render_response,
    run_cli_from, run_doctor_with_output, workspace_tool,
};

const LIST_COMMAND: CommandSelector = CommandSelector::new(&["list"]);
const CURRENT_COMMAND: CommandSelector = CommandSelector::new(&["current"]);
const CURRENT_JSON_FLAG: FlagSelector = FlagSelector::new(&[], "json");
const LIST_CAPABILITY: AgentCapability =
    AgentCapability::minimal("list-configuration", &[LIST_COMMAND], &[])
        .with_output("configured harness, model, and domain names")
        .with_constraints("reads runtime configuration only")
        .with_when_to_use("the user wants to inspect available clanker launch choices")
        .with_when_not_to_use("the user wants to launch a harness or modify configuration");
const CURRENT_CAPABILITY: AgentCapability = AgentCapability::new(
    "current-session",
    "Determine how clanker launched the current harness session",
    &[CURRENT_COMMAND],
    &[CURRENT_JSON_FLAG],
)
.with_examples(&["clanker --json current"])
.with_output(
    "launch-time active state, invocation, harness, context, ordered domains, model, prompt family, and the precedence tier that supplied each axis",
)
.with_constraints(
    "run clanker --json current and report its inherited launch markers; active=false means this process was not launched by marker-capable clanker; never recompute the answer from the current directory or configuration",
)
.with_when_to_use(
    "the user asks how this agent session was launched, which clanker context or domains are active, or whether clanker launched it",
)
.with_when_not_to_use(
    "the user wants to predict a future launch or inspect available clanker configuration",
);
const PROJECT_RESOLVE_COMMAND: CommandSelector = CommandSelector::new(&["project", "resolve"]);
const PROJECT_DIR_FLAG: FlagSelector = FlagSelector::new(&[], "dir");
const PROJECT_JSON_FLAG: FlagSelector = FlagSelector::new(&[], "json");
const PROJECT_CAPABILITY: AgentCapability = AgentCapability::new(
    "resolve-project",
    "Resolve which project a directory belongs to",
    &[PROJECT_RESOLVE_COMMAND],
    &[PROJECT_DIR_FLAG, PROJECT_JSON_FLAG],
)
.with_examples(&["clanker project resolve --dir /path/to/repo"])
.with_output(
    "tab-separated slug, precedence tier, and normalized origin remote; empty fields when unresolved",
)
.with_constraints(
    "reads the project registry and repository metadata for --dir or the current directory; exits 0 even when unresolved",
)
.with_when_to_use("a hook or script needs the project a directory belongs to")
.with_when_not_to_use(
    "the user wants the current session's own project (use current-session) or wants to launch a harness",
);
const AGENT_SURFACE: AgentSurfaceSpec =
    AgentSurfaceSpec::new(&[LIST_CAPABILITY, CURRENT_CAPABILITY, PROJECT_CAPABILITY]);

/// Shared clanker CLI metadata and agent surface.
pub const TOOL_SPEC: ToolSpec = workspace_tool(
    "clanker",
    "clanker",
    env!("CARGO_PKG_VERSION"),
    LicenseType::MIT,
    true,
    true,
)
.with_agent_surface(&AGENT_SURFACE);

/// Process-edge values captured by the binary before domain execution.
///
/// Only `HOME`, `PATH`, and `CLANKER_`-prefixed variables are examined for
/// their *values*; nothing else in the ambient environment changes how a launch
/// resolves.
///
/// [`RuntimeContext::inherited_names`] additionally carries the *names* of every
/// ambient variable, which is what a launch needs in order to remove the ones a
/// domain was not granted. Names only: clanker never reads the value of a
/// credential it is merely passing through, so a secret it does not grant never
/// enters this process.
#[derive(Debug, Clone)]
pub struct RuntimeContext {
    /// Home directory, when set.
    pub home: Option<PathBuf>,
    /// Identifier minted once per process for this launch, exported as
    /// `CLANKER_SESSION_ID`.
    ///
    /// Minted at the binary edge rather than during plan construction so
    /// [`launch::build_command_plan`] and [`launch::build_symlink_plan`] stay
    /// pure functions of their inputs.
    pub session_id: String,
    /// Current working directory.
    pub current_dir: PathBuf,
    /// `CLANKER_CONFIG` override.
    pub config_environment: Option<PathBuf>,
    /// `CLANKER_CONTEXT` value.
    pub context_environment: Option<String>,
    /// `CLANKER_DOMAIN` value.
    pub domain_environment: Option<String>,
    /// `CLANKER_MODEL` value.
    pub model_environment: Option<String>,
    /// Inherited `PATH`.
    pub inherited_path: Option<OsString>,
    /// Inherited `CLANKER_`-prefixed environment, used to read session markers.
    pub clanker_environment: BTreeMap<OsString, OsString>,
    /// Names of every variable in the ambient environment, with no values.
    ///
    /// A launch removes those a domain's allowlist does not name. Holding names
    /// rather than values means a credential clanker does not grant is never
    /// read into this process.
    pub inherited_names: Vec<OsString>,
    /// Directory holding `projects.toml`, when computable from `HOME` and
    /// `XDG_CONFIG_HOME`.
    pub project_registry_dir: Option<PathBuf>,
    /// Loaded, merged project registry.
    ///
    /// A load failure never fails a launch (ENG-004): `main.rs` reports it
    /// on stderr and falls back to an empty registry, which resolves to no
    /// project rather than blocking the harness from starting.
    pub project_registry: tftio_lib::project::Registry,
    /// `.clanker` project declaration at this working directory's project
    /// root, read once at the binary edge.
    pub declared_project: Option<tftio_lib::project::Slug>,
    /// This working directory's normalized origin remote, when it is inside
    /// a repository with one, read once at the binary edge.
    pub origin_remote: Option<tftio_lib::project::NormalizedRemote>,
    /// Canonicalized working directory used to match registry `paths`
    /// entries.
    pub project_dir: PathBuf,
}

impl RuntimeContext {
    /// Resolve an explicit, environment, or default configuration path.
    ///
    /// Relative explicit/environment paths resolve against the captured working
    /// directory. The default requires `HOME`.
    ///
    /// # Errors
    /// Returns [`ClankerError::HomeNotSet`] when no override exists and `HOME`
    /// is unavailable.
    pub fn config_path(&self, explicit: Option<&Path>) -> Result<PathBuf, ClankerError> {
        if let Some(path) = explicit {
            return Ok(self.resolve_relative(path));
        }
        if let Some(path) = &self.config_environment {
            return Ok(self.resolve_relative(path));
        }
        self.home
            .as_ref()
            .map(|home| home.join(".config/clanker/config.toml"))
            .ok_or(ClankerError::HomeNotSet)
    }

    fn resolve_relative(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.current_dir.join(path)
        }
    }

    fn launch_inputs(&self) -> Result<LaunchInputs<'_>, ClankerError> {
        let home = self.home.as_deref().ok_or(ClankerError::HomeNotSet)?;
        Ok(LaunchInputs {
            home,
            session_id: &self.session_id,
            current_dir: &self.current_dir,
            context_environment: self.context_environment.as_deref(),
            domain_environment: self.domain_environment.as_deref(),
            model_environment: self.model_environment.as_deref(),
            inherited_path: self.inherited_path.as_deref(),
            inherited_names: &self.inherited_names,
            project_registry: &self.project_registry,
            declared_project: self.declared_project.as_ref(),
            origin_remote: self.origin_remote.as_ref(),
            project_dir: &self.project_dir,
        })
    }

    /// Build [`DoctorInputs`] from this runtime context.
    fn doctor_inputs(&self, config_path: Option<PathBuf>) -> DoctorInputs {
        DoctorInputs {
            config_path,
            registry_dir: self.project_registry_dir.clone(),
            home: self.home.clone(),
            current_dir: self.project_dir.clone(),
        }
    }
}

/// Parse and execute clanker command mode through the shared CLI runner.
#[must_use]
pub fn command_exit_code<I>(argv: I, process_env: &ProcessEnv, runtime: &RuntimeContext) -> i32
where
    I: IntoIterator,
    I::Item: Into<OsString> + Clone,
{
    let doctor_path = runtime.config_path(None).ok();
    let doctor = Doctor::new(runtime.doctor_inputs(doctor_path));
    run_cli_from::<Cli, _, Doctor, _, _>(
        &TOOL_SPEC,
        process_env,
        argv,
        &doctor,
        metadata_command,
        |cli| run_command(cli, runtime),
    )
}

/// Execute symlink mode and replace the process with the configured harness.
///
/// # Errors
/// Returns [`ClankerError`] when configuration, context resolution, or exec
/// fails. A successful exec never returns.
pub fn run_symlink_mode(
    invocation: &str,
    args: Vec<OsString>,
    runtime: &RuntimeContext,
) -> Result<i32, ClankerError> {
    let config_path = runtime.config_path(None)?;
    let loaded = load_config(&config_path)?;
    let plan = build_symlink_plan(invocation, args, &loaded.config, runtime.launch_inputs()?)?;
    execute_plan(&plan).map_err(Into::into)
}

fn metadata_command(cli: &Cli) -> Option<StandardCommand> {
    match &cli.command {
        Command::Meta { command } => Some(map_standard_command(
            command,
            JsonOutput::from_flag(cli.json),
        )),
        Command::Doctor
        | Command::List
        | Command::Current
        | Command::Env { .. }
        | Command::Exec { .. }
        | Command::Project { .. }
        | Command::Prompt { .. }
        | Command::Harness(_) => None,
    }
}

fn run_command(cli: Cli, runtime: &RuntimeContext) -> Result<i32, FatalCliError> {
    let output = JsonOutput::from_flag(cli.json);
    let command_label = match &cli.command {
        Command::Meta { .. } => "meta",
        Command::Doctor => "doctor",
        Command::List => "list",
        Command::Current => "current",
        Command::Env { .. } => "env",
        Command::Exec { .. } => "exec",
        Command::Project { .. } => "project",
        Command::Prompt { .. } => "prompt",
        Command::Harness(_) => "launch",
    };
    run_command_inner(cli, runtime, output)
        .map_err(|error| FatalCliError::new(command_label, output, error.to_string()))
}

fn run_command_inner(
    cli: Cli,
    runtime: &RuntimeContext,
    output: JsonOutput,
) -> Result<i32, ClankerError> {
    match cli.command {
        Command::Meta { .. } => Err(ClankerError::UnroutedMetadataCommand),
        Command::Doctor => {
            let config_path = runtime.config_path(cli.config.as_deref()).ok();
            Ok(run_doctor_with_output(
                &Doctor::new(runtime.doctor_inputs(config_path)),
                output,
            ))
        }
        Command::List => {
            let loaded = load_runtime_config(runtime, cli.config.as_deref())?;
            emit_list(&loaded, output)?;
            Ok(0)
        }
        Command::Current => {
            emit_current(
                &CurrentSession::from_environment(&runtime.clanker_environment)?,
                output,
            )?;
            Ok(0)
        }
        Command::Env { harness, arguments } => {
            run_environment(harness, arguments, cli.config.as_deref(), runtime, output)
        }
        Command::Exec { context, argv } => run_exec(context, argv, cli.config.as_deref(), runtime),
        Command::Project { command } => run_project(command, runtime, output),
        Command::Prompt { command } => {
            run_prompt_command(command, cli.config.as_deref(), runtime, output)
        }
        Command::Harness(values) => run_harness(values, cli.config.as_deref(), runtime, output),
    }
}

fn load_runtime_config(
    runtime: &RuntimeContext,
    explicit: Option<&Path>,
) -> Result<LoadedConfig, ClankerError> {
    let path = runtime.config_path(explicit)?;
    load_config(&path).map_err(Into::into)
}

/// Resolve the prompter bundle's `config.toml`.
///
/// An explicit `--config` value is used verbatim, exactly as the retired
/// `prompter -c` flag treated its override: a path to the config file itself,
/// not a bundle directory. Absent an override, the bundle named by clanker's
/// own `defaults.prompter_bundle` is used, matching launch-time composition.
fn resolve_prompt_config(
    explicit_config: Option<&Path>,
    runtime: &RuntimeContext,
) -> Result<PathBuf, ClankerError> {
    if let Some(path) = explicit_config {
        return Ok(path.to_path_buf());
    }
    let loaded = load_runtime_config(runtime, None)?;
    let home = runtime.home.as_deref().ok_or(ClankerError::HomeNotSet)?;
    let bundle = launch::expand_path(&loaded.config.defaults.prompter_bundle, home)?;
    Ok(bundle.join("config.toml"))
}

fn run_prompt_command(
    command: PromptCommand,
    explicit_config: Option<&Path>,
    runtime: &RuntimeContext,
    output: JsonOutput,
) -> Result<i32, ClankerError> {
    let bundle_config = resolve_prompt_config(explicit_config, runtime)?;
    match command {
        PromptCommand::List => {
            promptlib::run_list_stdout(Some(&bundle_config), output)?;
            Ok(0)
        }
        PromptCommand::Tree => {
            promptlib::run_tree_stdout(Some(&bundle_config), output)?;
            Ok(0)
        }
        PromptCommand::Validate => {
            promptlib::run_validate_stdout(Some(&bundle_config), output)?;
            if !output.is_json() {
                println!("All profiles valid");
            }
            Ok(0)
        }
        PromptCommand::Run {
            profiles,
            family,
            separator,
            pre_prompt,
            post_prompt,
            bare,
        } => {
            let separator = separator.as_deref().map(promptlib::unescape);
            let pre_prompt = pre_prompt.as_deref().map(promptlib::unescape);
            let post_prompt = post_prompt.as_deref().map(promptlib::unescape);
            promptlib::run_render_stdout(
                &profiles,
                family.as_ref(),
                separator.as_deref(),
                pre_prompt.as_deref(),
                post_prompt.as_deref(),
                promptlib::Framing::from_bare_flag(bare),
                Some(&bundle_config),
                output,
            )?;
            Ok(0)
        }
        PromptCommand::System {
            profiles,
            separator,
            pre_prompt,
            post_prompt,
            bare,
        } => {
            let separator = separator.as_deref().map(promptlib::unescape);
            let pre_prompt = pre_prompt.as_deref().map(promptlib::unescape);
            let post_prompt = post_prompt.as_deref().map(promptlib::unescape);
            let mut all_profiles = Vec::with_capacity(profiles.len() + 1);
            all_profiles.push(promptlib::SYSTEM_BASE_PROFILE.to_string());
            all_profiles.extend(profiles);
            promptlib::run_render_stdout(
                &all_profiles,
                None,
                separator.as_deref(),
                pre_prompt.as_deref(),
                post_prompt.as_deref(),
                promptlib::Framing::from_bare_flag(bare),
                Some(&bundle_config),
                output,
            )?;
            Ok(0)
        }
    }
}

fn run_harness(
    values: Vec<OsString>,
    explicit_config: Option<&Path>,
    runtime: &RuntimeContext,
    output: JsonOutput,
) -> Result<i32, ClankerError> {
    let request = parse_external_launch(values)?;
    let loaded = load_runtime_config(runtime, explicit_config)?;
    let invocation = format!("clanker {}", request.harness);
    let plan = build_command_plan(
        &invocation,
        &request,
        &loaded.config,
        runtime.launch_inputs()?,
        !request.dry_run,
    )?;
    if request.dry_run {
        emit_dry_run(&plan, output)?;
        Ok(0)
    } else {
        execute_plan(&plan).map_err(Into::into)
    }
}

fn run_environment(
    harness: OsString,
    arguments: Vec<OsString>,
    explicit_config: Option<&Path>,
    runtime: &RuntimeContext,
    output: JsonOutput,
) -> Result<i32, ClankerError> {
    let mut request = parse_launch(harness, arguments)?;
    request.no_prompt = true;
    request.dry_run = true;
    request.sandbox = false;
    let loaded = load_runtime_config(runtime, explicit_config)?;
    let invocation = format!("clanker env {}", request.harness);
    let plan = build_command_plan(
        &invocation,
        &request,
        &loaded.config,
        runtime.launch_inputs()?,
        false,
    )?;
    emit_environment(&plan, output)?;
    Ok(0)
}

fn run_exec(
    context: Option<OsString>,
    argv: Vec<OsString>,
    explicit_config: Option<&Path>,
    runtime: &RuntimeContext,
) -> Result<i32, ClankerError> {
    let request = parse_exec(context, argv)?;
    let loaded = load_runtime_config(runtime, explicit_config)?;
    let (plan, skipped) = build_exec_plan(&request, &loaded.config, runtime.launch_inputs()?)?;
    for skip in skipped {
        eprintln!(
            "warning: skipping harness `{}`: config directory {} does not exist; no variables from this harness will be exported",
            skip.harness,
            skip.path.display()
        );
    }
    execute_plan(&plan).map_err(Into::into)
}

fn run_project(
    command: ProjectCommand,
    runtime: &RuntimeContext,
    output: JsonOutput,
) -> Result<i32, ClankerError> {
    match command {
        ProjectCommand::Resolve { dir } => run_project_resolve(dir, runtime, output),
        ProjectCommand::List => run_project_list(runtime, output),
    }
}

/// Resolve the project for a directory (default: the current directory).
///
/// Unlike a launch, this command reports a registry-load or discovery
/// failure as a real error: it is a diagnostic entry point with nothing
/// else to protect, unlike `runtime_context`'s own fallback at the binary
/// edge. An unresolved project is not an error; it exits `0` with empty
/// fields, so a shell hook can call it unconditionally.
fn run_project_resolve(
    dir: Option<PathBuf>,
    runtime: &RuntimeContext,
    output: JsonOutput,
) -> Result<i32, ClankerError> {
    let target = dir.map_or_else(
        || runtime.project_dir.clone(),
        |dir| runtime.resolve_relative(&dir),
    );
    let canonical = target.canonicalize().unwrap_or(target);
    let inputs = tftio_lib::project::discover_inputs(&canonical)?;
    let resolution = tftio_lib::project::resolve(
        inputs.declared.as_ref(),
        &canonical,
        inputs.remote.as_ref(),
        &runtime.project_registry,
    );
    emit_project_resolution(resolution.as_ref(), output)
}

#[derive(Debug, Serialize)]
struct ProjectResolutionOutput {
    project: Option<String>,
    source: Option<&'static str>,
    remote: Option<String>,
}

fn emit_project_resolution(
    resolution: Option<&tftio_lib::project::Resolution>,
    output: JsonOutput,
) -> Result<i32, ClankerError> {
    let project = resolution.map(|resolution| resolution.slug.to_string());
    let source = resolution.map(|resolution| resolution.source.label());
    let remote =
        resolution.and_then(|resolution| resolution.remote.as_ref().map(ToString::to_string));

    if output.is_json() {
        let data = serde_json::to_value(ProjectResolutionOutput {
            project,
            source,
            remote,
        })?;
        println!(
            "{}",
            render_response("project resolve", JsonOutput::Json, data, String::new())
        );
    } else {
        println!(
            "{}\t{}\t{}",
            project.unwrap_or_default(),
            source.unwrap_or_default(),
            remote.unwrap_or_default()
        );
    }
    Ok(0)
}

#[derive(Debug, Serialize)]
struct ProjectListEntry {
    slug: String,
    remotes: Vec<String>,
    paths: Vec<String>,
}

fn run_project_list(runtime: &RuntimeContext, output: JsonOutput) -> Result<i32, ClankerError> {
    let entries: Vec<ProjectListEntry> = runtime
        .project_registry
        .projects()
        .map(|(slug, project)| ProjectListEntry {
            slug: slug.to_string(),
            remotes: project.remotes.iter().cloned().collect(),
            paths: project
                .paths
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
        })
        .collect();

    if output.is_json() {
        let data = serde_json::to_value(&entries)?;
        println!(
            "{}",
            render_response("project list", JsonOutput::Json, data, String::new())
        );
    } else {
        for entry in &entries {
            println!(
                "{}\t{}\t{}",
                entry.slug,
                entry.remotes.join(","),
                entry.paths.join(",")
            );
        }
    }
    Ok(0)
}

#[derive(Debug, Serialize)]
struct ListOutput {
    harnesses: Vec<String>,
    models: Vec<String>,
    domains: Vec<String>,
}

fn emit_list(loaded: &LoadedConfig, output: JsonOutput) -> Result<(), ClankerError> {
    let listing = ListOutput {
        harnesses: loaded
            .config
            .harness
            .keys()
            .map(ToString::to_string)
            .collect(),
        models: loaded
            .config
            .model
            .keys()
            .map(ToString::to_string)
            .collect(),
        domains: loaded
            .config
            .domain
            .keys()
            .map(ToString::to_string)
            .collect(),
    };
    if output.is_json() {
        let data = serde_json::to_value(&listing)?;
        println!(
            "{}",
            render_response("list", JsonOutput::Json, data, String::new())
        );
    } else {
        println!("Harnesses:");
        for name in &listing.harnesses {
            println!("  {name}");
        }
        println!("Models:");
        for name in &listing.models {
            println!("  {name}");
        }
        println!("Domains:");
        for name in &listing.domains {
            println!("  {name}");
        }
    }
    Ok(())
}

fn emit_current(session: &CurrentSession, output: JsonOutput) -> Result<(), ClankerError> {
    if output.is_json() {
        let data = serde_json::to_value(session)?;
        println!(
            "{}",
            render_response("current", JsonOutput::Json, data, String::new())
        );
        return Ok(());
    }

    println!("active={}", session.active);
    for (name, value) in [
        ("version", session.version.as_deref()),
        ("id", session.id.as_deref()),
        ("invocation", session.invocation.as_deref()),
        ("harness", session.harness.as_deref()),
        ("context", session.context.as_deref()),
        ("context_source", session.context_source.as_deref()),
    ] {
        if let Some(value) = value {
            println!("{name}={value}");
        }
    }
    if let Some(domains) = &session.domains {
        for domain in domains {
            println!("domain={domain}");
        }
    }
    for (name, value) in [
        ("domain_source", session.domain_source.as_deref()),
        ("model", session.model.as_deref()),
        ("model_source", session.model_source.as_deref()),
        ("family", session.family.as_deref()),
        ("project", session.project.as_deref()),
        ("project_source", session.project_source.as_deref()),
        ("remote", session.remote.as_deref()),
    ] {
        if let Some(value) = value {
            println!("{name}={value}");
        }
    }
    Ok(())
}

fn emit_dry_run(plan: &LaunchPlan, output: JsonOutput) -> Result<(), ClankerError> {
    if output.is_json() {
        let data = serde_json::to_value(LaunchPlanOutput::from(plan))?;
        println!(
            "{}",
            render_response("launch", JsonOutput::Json, data, String::new())
        );
    } else {
        println!("invocation={}", plan.invocation);
        println!("harness={}", plan.harness);
        println!("context={}", plan.context.value);
        println!(
            "context_source={}",
            plan.context.source.label(axis::CONTEXT)
        );
        for domain in &plan.domains {
            println!("domain={domain}");
        }
        if let Some(source) = plan.domain_source {
            println!("domain_source={}", source.label(axis::DOMAIN));
        }
        if let Some(model) = &plan.model {
            println!("model={model}");
        }
        if let Some(source) = plan.model_source {
            println!("model_source={}", source.label(axis::MODEL));
        }
        if let Some(family) = &plan.family {
            println!("family={family}");
        }
        if let Some(project) = &plan.project {
            println!("project={project}");
        }
        if let Some(source) = plan.project_source {
            println!("project_source={}", source.label());
        }
        if let Some(remote) = &plan.remote {
            println!("remote={remote}");
        }
        println!("executable={}", plan.executable.to_string_lossy());
        for argument in &plan.args {
            println!("arg={}", argument.to_string_lossy());
        }
        for (key, entry) in &plan.environment {
            println!(
                "env.{}={} ({})",
                key.to_string_lossy(),
                entry.value.to_string_lossy(),
                entry.provenance.label()
            );
        }
        for name in &plan.removed_environment {
            println!("removed_env={}", name.to_string_lossy());
        }
        if let Some(prompt_file) = &plan.prompt_file {
            println!("prompt_file={}", prompt_file.display());
        }
        if let Some(prompt) = &plan.prompt {
            println!("prompt<<CLANKER_PROMPT");
            print!("{prompt}");
            if !prompt.ends_with('\n') {
                println!();
            }
            println!("CLANKER_PROMPT");
        }
    }
    Ok(())
}

fn emit_environment(plan: &LaunchPlan, output: JsonOutput) -> Result<(), ClankerError> {
    if output.is_json() {
        let data = serde_json::to_value(LaunchPlanOutput::from(plan))?;
        println!(
            "{}",
            render_response("env", JsonOutput::Json, data, String::new())
        );
        return Ok(());
    }

    for (key, entry) in &plan.environment {
        let key = key.to_str().ok_or_else(|| {
            ClankerError::NonUtf8EnvironmentOutput(key.to_string_lossy().into_owned())
        })?;
        let value = entry
            .value
            .to_str()
            .ok_or_else(|| ClankerError::NonUtf8EnvironmentOutput(key.to_string()))?;
        println!("export {key}={}", shell_quote(value));
    }
    Ok(())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Extract the final path component from `argv[0]`.
#[must_use]
pub fn invocation_name(argv0: &OsStr) -> String {
    Path::new(argv0)
        .file_name()
        .unwrap_or(argv0)
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(home: Option<PathBuf>, current_dir: PathBuf) -> RuntimeContext {
        RuntimeContext {
            home,
            session_id: "01a06ebb-0000-7000-8000-000000000000".to_string(),
            project_dir: current_dir.clone(),
            current_dir,
            config_environment: None,
            context_environment: None,
            domain_environment: None,
            model_environment: None,
            inherited_path: None,
            clanker_environment: BTreeMap::new(),
            inherited_names: Vec::new(),
            project_registry_dir: None,
            project_registry: tftio_lib::project::Registry::default(),
            declared_project: None,
            origin_remote: None,
        }
    }

    #[test]
    fn config_path_resolves_relative_explicit_against_current_dir() {
        let runtime = runtime(None, PathBuf::from("/work/dir"));
        let path = runtime
            .config_path(Some(Path::new("rel/config.toml")))
            .unwrap();
        assert_eq!(path, PathBuf::from("/work/dir/rel/config.toml"));
    }

    #[test]
    fn config_path_keeps_absolute_explicit_path() {
        let runtime = runtime(None, PathBuf::from("/work/dir"));
        let path = runtime
            .config_path(Some(Path::new("/etc/clanker.toml")))
            .unwrap();
        assert_eq!(path, PathBuf::from("/etc/clanker.toml"));
    }

    #[test]
    fn config_path_uses_relative_config_environment_override() {
        let mut runtime = runtime(
            Some(PathBuf::from("/home/user")),
            PathBuf::from("/work/dir"),
        );
        runtime.config_environment = Some(PathBuf::from("env/clanker.toml"));
        let path = runtime.config_path(None).unwrap();
        assert_eq!(path, PathBuf::from("/work/dir/env/clanker.toml"));
    }

    #[test]
    fn config_path_defaults_under_home() {
        let runtime = runtime(
            Some(PathBuf::from("/home/user")),
            PathBuf::from("/work/dir"),
        );
        let path = runtime.config_path(None).unwrap();
        assert_eq!(
            path,
            PathBuf::from("/home/user/.config/clanker/config.toml")
        );
    }

    #[test]
    fn config_path_requires_home_without_overrides() {
        let runtime = runtime(None, PathBuf::from("/work/dir"));
        let error = runtime.config_path(None).unwrap_err();
        assert!(matches!(error, ClankerError::HomeNotSet));
    }

    #[test]
    fn launch_inputs_require_home() {
        let runtime = runtime(None, PathBuf::from("/work/dir"));
        let error = runtime.launch_inputs().unwrap_err();
        assert!(matches!(error, ClankerError::HomeNotSet));
    }

    #[test]
    fn invocation_name_extracts_final_component() {
        assert_eq!(
            invocation_name(OsStr::new("/usr/local/bin/claude")),
            "claude"
        );
    }

    #[test]
    fn shell_quote_escapes_embedded_single_quotes() {
        assert_eq!(shell_quote("it's"), r#"'it'"'"'s'"#);
    }
}
