//! Runtime configuration loading, validation, and overlay merging.

use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

const CONTEXT_PLACEHOLDER: &str = "{context}";

/// Environment variables clanker computes for every launch. A domain or model
/// may not name one, because doing so would silently displace a value the
/// launcher owns.
pub(crate) const PLAYWRIGHT_MCP_BROWSER_ENVIRONMENT: &str = "PLAYWRIGHT_MCP_BROWSER";
pub(crate) const PLAYWRIGHT_MCP_USER_DATA_DIR_ENVIRONMENT: &str = "PLAYWRIGHT_MCP_USER_DATA_DIR";

const CLANKER_OWNED_ENVIRONMENT: [&str; 6] = [
    "PATH",
    "CONTEXT",
    "CLANKER_CONTEXT",
    "CLANKER_PROVIDER",
    PLAYWRIGHT_MCP_BROWSER_ENVIRONMENT,
    PLAYWRIGHT_MCP_USER_DATA_DIR_ENVIRONMENT,
];

/// Prefix reserved for launch-time session markers.
const SESSION_ENVIRONMENT_PREFIX: &str = "CLANKER_SESSION";

/// Error returned when a configured name is empty or unsafe as one path-like
/// component.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {kind} name `{value}`; use letters, digits, '.', '_', or '-'")]
pub struct InvalidName {
    kind: &'static str,
    value: String,
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

macro_rules! name_type {
    ($name:ident, $kind:literal, $docs:literal) => {
        #[doc = $docs]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            #[doc = concat!("Create a validated ", $kind, " name.")]
            ///
            /// # Errors
            /// Returns [`InvalidName`] when the value is empty or contains an
            /// unsupported character.
            pub fn new(value: impl Into<String>) -> Result<Self, InvalidName> {
                let value = value.into();
                if valid_name(&value) {
                    Ok(Self(value))
                } else {
                    Err(InvalidName { kind: $kind, value })
                }
            }

            #[doc = concat!("Return the validated ", $kind, " name.")]
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(D::Error::custom)
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

name_type!(ContextName, "context", "A validated context name.");
name_type!(DomainName, "domain", "A validated domain name.");
name_type!(HarnessName, "harness", "A validated harness name.");
name_type!(ModelName, "model", "A validated model alias.");
name_type!(ProviderName, "provider", "A validated provider name.");

/// A validated prompter family name from runtime configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFamily(crate::promptlib::FamilyName);

impl ModelFamily {
    /// Return the prompter family value.
    #[must_use]
    pub const fn as_prompter_family(&self) -> &crate::promptlib::FamilyName {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ModelFamily {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        crate::promptlib::FamilyName::new(value)
            .map(Self)
            .map_err(D::Error::custom)
    }
}

/// Top-level runtime configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Runtime defaults and filesystem locations.
    pub defaults: DefaultsConfig,
    /// Harness definitions keyed by command name.
    pub harness: BTreeMap<HarnessName, HarnessConfig>,
    /// Model aliases and provider environment bundles.
    pub model: BTreeMap<ModelName, ModelConfig>,
    /// Domain prompt compositions.
    pub domain: BTreeMap<DomainName, DomainConfig>,
    /// Alternate inference endpoints a harness can be launched against.
    ///
    /// Absent from a configuration that defines none, so existing
    /// configurations parse unchanged.
    #[serde(default)]
    pub provider: BTreeMap<ProviderName, ProviderConfig>,
}

/// Runtime defaults and shared filesystem locations.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefaultsConfig {
    /// Default domain for subcommand launches.
    pub domain: DomainName,
    /// Registry of every launchable context.
    pub contexts: Vec<ContextName>,
    /// Directory prepended to every launched process's `PATH`.
    pub shim_path: String,
    /// Executable used to wrap sandboxed launches.
    pub sandbox_wrapper: String,
    /// Prompter bundle containing `config.toml` and `library/`.
    pub prompter_bundle: String,
    /// Directory holding rendered prompt files for file-based injection.
    pub prompt_cache: String,
    /// Age after which cached prompt files are removed opportunistically.
    pub prompt_cache_ttl_seconds: u64,
    /// Environment variables every launched session inherits, whatever its
    /// domain.
    ///
    /// A launched process receives exactly these, the names its domain's
    /// `credentials` list adds, and clanker's own overrides. Everything else in
    /// the ambient environment is removed before the harness starts, so a
    /// session cannot reach a credential its domain was not granted.
    ///
    /// The default carries what a shell session needs to function and nothing
    /// that confers authority. `SSH_AUTH_SOCK` is deliberately **absent**:
    /// passing it hands the session the ability to authenticate and sign as the
    /// operator for as long as the socket is live, which is the opposite of what
    /// this list exists to do. A domain that genuinely needs it must name it.
    #[serde(default = "default_passthrough_environment")]
    pub passthrough_environment: Vec<String>,
}

/// Environment variables a launched session inherits when the configuration
/// does not say otherwise.
///
/// Locale, terminal, and filesystem-location variables only. Nothing here
/// carries a credential or a capability.
fn default_passthrough_environment() -> Vec<String> {
    [
        // Locale, terminal, and identity.
        "COLORTERM",
        "COLUMNS",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "LINES",
        "LOGNAME",
        "SHELL",
        "SHLVL",
        "TERM",
        "TERMINFO",
        "TZ",
        "USER",
        // Filesystem locations.
        "HOME",
        "INFOPATH",
        "MANPATH",
        "PATH",
        "PWD",
        "TMPDIR",
        "XDG_CACHE_HOME",
        "XDG_CONFIG_DIRS",
        "XDG_CONFIG_HOME",
        "XDG_DATA_DIRS",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        // Toolchain locations. Without these a launched session cannot find
        // the toolchain the operator's shell resolves, which makes the whole
        // filter look broken rather than protective.
        "CARGO_HOME",
        "HOMEBREW_CELLAR",
        "HOMEBREW_PREFIX",
        "HOMEBREW_REPOSITORY",
        "MISE_SHELL",
        "RUSTUP_HOME",
        "RUSTUP_TOOLCHAIN",
        // Editor and pager preferences.
        "EDITOR",
        "GIT_EDITOR",
        "LESS",
        "PAGER",
        "VISUAL",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

/// One executable harness and its prompt/config integration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessConfig {
    /// Executable name or path.
    pub bin: String,
    /// Default model-family prompt variant.
    pub family: ModelFamily,
    /// Native arguments prepended to every command-mode launch.
    pub default_args: Vec<String>,
    /// Allowed contexts, or all global contexts when omitted.
    pub contexts: Option<Vec<ContextName>>,
    /// Prompt injection contract.
    pub injection: InjectionConfig,
    /// Optional context-derived config directory.
    pub config_dir: Option<ConfigDirectory>,
}

impl HarnessConfig {
    /// Whether this harness is enabled in a globally validated context.
    #[must_use]
    pub fn supports_context(&self, context: &ContextName) -> bool {
        self.contexts
            .as_ref()
            .is_none_or(|contexts| contexts.contains(context))
    }
}

/// Prompt injection contract for a harness.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum InjectionConfig {
    /// Inject prompt text as arguments, replacing `{text}`.
    ArgText {
        /// Argument template.
        args: Vec<String>,
    },
    /// Write prompt text to a file and inject its path as arguments.
    ArgFile {
        /// Argument template containing `{file}`.
        args: Vec<String>,
    },
    /// Write prompt text to a file and export its path through an environment variable.
    EnvFile {
        /// Target environment variable.
        environment: String,
    },
    /// Write prompt text to a file and expose it as an `OpenCode` instruction.
    OpencodeInstructions {
        /// Target environment variable for serialized inline `OpenCode` configuration.
        environment: String,
    },
}

/// One environment variable naming one context-derived directory.
#[derive(Debug, Clone)]
pub struct SingleDirectory {
    /// Environment variable exported for the context-derived directory.
    pub environment: String,
    /// Path template containing `{context}`.
    pub path: String,
}

/// Several environment variables, each naming one context-derived directory.
#[derive(Debug, Clone)]
pub struct MultipleDirectories {
    /// Path templates containing `{context}`, keyed by exported variable.
    pub directories: BTreeMap<String, String>,
}

/// Context-derived harness configuration directories.
///
/// A harness declares either the single-variable shape (`environment` plus
/// `path`) or the multi-variable shape (a `directories` table keyed by exported
/// variable). The wire shape is parsed into this closed sum type at the
/// boundary, so no consumer can observe a half-declared directory.
#[derive(Debug, Clone)]
pub enum ConfigDirectory {
    /// One exported variable and one path template.
    Single(SingleDirectory),
    /// One exported variable per entry, each with its own path template.
    Multiple(MultipleDirectories),
}

/// Wire shape for [`ConfigDirectory`], accepting either declared form.
///
/// The two forms are parsed through one struct rather than an untagged enum
/// because `toml`'s value deserializer does not resolve untagged variants, and
/// because a single struct with `deny_unknown_fields` reports a mistyped key by
/// name instead of silently matching the other form.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigDirectoryWire {
    environment: Option<String>,
    path: Option<String>,
    directories: Option<BTreeMap<String, String>>,
}

impl<'de> Deserialize<'de> for ConfigDirectory {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ConfigDirectoryWire::deserialize(deserializer)?;
        let single = wire.environment.is_some() || wire.path.is_some();
        match (wire.environment, wire.path, wire.directories) {
            (Some(environment), Some(path), None) => {
                Ok(Self::Single(SingleDirectory { environment, path }))
            }
            (None, None, Some(directories)) => {
                Ok(Self::Multiple(MultipleDirectories { directories }))
            }
            (_, _, Some(_)) if single => Err(D::Error::custom(
                "declare either `environment` and `path`, or `directories`, not both",
            )),
            (_, _, None) if single => Err(D::Error::custom(
                "`environment` and `path` must be declared together",
            )),
            _ => Err(D::Error::custom(
                "expected `environment` and `path`, or `directories`",
            )),
        }
    }
}

impl ConfigDirectory {
    /// Return every exported variable with its path template, in a stable
    /// order, so callers never dispatch over the declared shape.
    #[must_use]
    pub fn entries(&self) -> Vec<(&str, &str)> {
        match self {
            Self::Single(single) => {
                vec![(single.environment.as_str(), single.path.as_str())]
            }
            Self::Multiple(multiple) => multiple
                .directories
                .iter()
                .map(|(environment, path)| (environment.as_str(), path.as_str()))
                .collect(),
        }
    }

    /// Derive the directories a provider uses in place of these.
    ///
    /// Every template is rewritten by [`provider_template`], so the derived
    /// set declares the same variables and differs in every path.
    #[must_use]
    pub fn for_provider(&self, provider: &ProviderName) -> Self {
        match self {
            Self::Single(single) => Self::Single(SingleDirectory {
                environment: single.environment.clone(),
                path: provider_template(&single.path, provider),
            }),
            Self::Multiple(multiple) => Self::Multiple(MultipleDirectories {
                directories: multiple
                    .directories
                    .iter()
                    .map(|(environment, path)| {
                        (environment.clone(), provider_template(path, provider))
                    })
                    .collect(),
            }),
        }
    }

    /// Return the configuration key identifying one entry's path template.
    fn path_key(&self, prefix: &str, environment: &str) -> String {
        match self {
            Self::Single(_) => format!("{prefix}.path"),
            Self::Multiple(_) => format!("{prefix}.directories.{environment}"),
        }
    }

    /// Return the configuration key identifying one entry's variable name.
    fn environment_key(&self, prefix: &str, environment: &str) -> String {
        match self {
            Self::Single(_) => format!("{prefix}.environment"),
            Self::Multiple(_) => format!("{prefix}.directories.{environment}"),
        }
    }
}

/// One alternate inference endpoint.
///
/// A provider exists so a harness can run against a second endpoint, in any
/// context, without sharing the native endpoint's config directories: those
/// directories hold the harness's credentials and session history. A provider
/// covers every harness that declares a `config_dir` and derives each
/// directory from the harness's own template (see
/// [`ConfigDirectory::for_provider`]), so an empty table is a complete
/// provider.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    /// Optional per-harness additions, keyed by the harness they apply to.
    #[serde(default)]
    pub harness: BTreeMap<HarnessName, ProviderHarnessConfig>,
}

/// What one provider adds to one harness's launch beyond its directories.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderHarnessConfig {
    /// Literal environment values applied by the provider.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Native arguments appended for this provider.
    #[serde(default)]
    pub harness_args: Vec<String>,
}

/// One configured model alias.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
    /// Harness this model can launch through.
    pub harness: HarnessName,
    /// Optional family override; the harness family is the fallback.
    pub family: Option<ModelFamily>,
    /// Literal environment values applied by the model.
    pub env: BTreeMap<String, String>,
    /// Native arguments appended for this model.
    pub harness_args: Vec<String>,
}

/// One domain's prompt profiles and optional runtime defaults.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainConfig {
    /// Prompter profiles composed in order.
    pub profiles: Vec<String>,
    /// Environment defaults applied by the domain.
    pub env: BTreeMap<String, String>,
    /// Optional model selected when no higher-precedence source exists.
    pub default_model: Option<ModelName>,
    /// Ambient environment variables this domain's sessions may inherit, by
    /// name.
    ///
    /// This is how a credential reaches a session: it is named here, and the
    /// value passes through untouched from the ambient environment. clanker
    /// never reads the value, so a credential it does not grant never enters
    /// this process at all.
    ///
    /// The default is empty. A domain that names nothing launches sessions that
    /// carry no credentials, which is the correct default for a domain whose
    /// work needs none.
    #[serde(default)]
    pub credentials: Vec<String>,
}

/// One semantic configuration validation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigIssue {
    /// Dotted configuration key.
    pub key: String,
    /// Human-readable correction.
    pub message: String,
}

impl fmt::Display for ConfigIssue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.key, self.message)
    }
}

/// Loaded and validated configuration plus its source paths.
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    /// Required base configuration path.
    pub base_path: PathBuf,
    /// Applied overlay path, when present.
    pub overlay_path: Option<PathBuf>,
    /// Validated merged configuration.
    pub config: Config,
}

/// Fail-closed configuration loading errors.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Required runtime config does not exist.
    #[error(
        "clanker config not found at {path}; install config/config.toml from the clanker repository there, or set CLANKER_CONFIG"
    )]
    Missing {
        /// Missing path.
        path: PathBuf,
    },
    /// Config file could not be read.
    #[error("failed to read clanker config {path}: {source}")]
    Read {
        /// Failing path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// TOML syntax is invalid.
    #[error("failed to parse clanker config {path}: {source}")]
    Parse {
        /// Failing path.
        path: PathBuf,
        /// TOML parse error.
        source: toml::de::Error,
    },
    /// Merged config does not match the typed schema.
    #[error("invalid clanker config schema from {sources}: {source}")]
    Schema {
        /// Base and optional overlay paths whose merged document failed.
        sources: String,
        /// Deserialization error.
        source: toml::de::Error,
    },
    /// Typed config violates cross-field invariants.
    #[error("invalid clanker config from {sources}:\n{issues}")]
    Invalid {
        /// Base and optional overlay paths whose merged document failed.
        sources: String,
        /// Newline-separated per-key issues.
        issues: String,
    },
}

impl Config {
    /// Validate cross-field and template invariants.
    #[must_use]
    pub fn issues(&self) -> Vec<ConfigIssue> {
        let mut issues = Vec::new();
        self.validate_defaults(&mut issues);
        self.validate_harnesses(&mut issues);
        let reserved = self.reserved_environment();
        self.validate_models(&reserved, &mut issues);
        self.validate_domains(&reserved, &mut issues);
        self.validate_providers(&reserved, &mut issues);
        issues
    }

    /// Environment variable names clanker computes and therefore owns.
    ///
    /// A domain or model that named one of these would displace a value the
    /// launcher computed, silently, at a layer the operator cannot see. The set
    /// covers the launcher's own variables, every harness config directory, and
    /// every env-file injection target.
    fn reserved_environment(&self) -> BTreeSet<String> {
        let mut reserved: BTreeSet<String> = CLANKER_OWNED_ENVIRONMENT
            .iter()
            .map(|name| (*name).to_string())
            .collect();
        for harness in self.harness.values() {
            if let Some(config_dir) = &harness.config_dir {
                for (environment, _) in config_dir.entries() {
                    reserved.insert(environment.to_string());
                }
            }
            match &harness.injection {
                InjectionConfig::EnvFile { environment }
                | InjectionConfig::OpencodeInstructions { environment } => {
                    reserved.insert(environment.clone());
                }
                InjectionConfig::ArgText { .. } | InjectionConfig::ArgFile { .. } => {}
            }
        }
        reserved
    }

    fn validate_defaults(&self, issues: &mut Vec<ConfigIssue>) {
        if self.harness.is_empty() {
            issues.push(ConfigIssue {
                key: "harness".to_string(),
                message: "must contain at least one harness".to_string(),
            });
        }

        check_nonempty(issues, "defaults.shim_path", &self.defaults.shim_path);
        check_nonempty(
            issues,
            "defaults.sandbox_wrapper",
            &self.defaults.sandbox_wrapper,
        );
        check_nonempty(
            issues,
            "defaults.prompter_bundle",
            &self.defaults.prompter_bundle,
        );
        check_nonempty(issues, "defaults.prompt_cache", &self.defaults.prompt_cache);
        if !self.domain.contains_key(&self.defaults.domain) {
            issues.push(ConfigIssue {
                key: "defaults.domain".to_string(),
                message: format!("unknown domain `{}`", self.defaults.domain),
            });
        }
        if self.defaults.contexts.is_empty() {
            issues.push(ConfigIssue {
                key: "defaults.contexts".to_string(),
                message: "must contain at least one context".to_string(),
            });
        }
    }

    fn validate_harnesses(&self, issues: &mut Vec<ConfigIssue>) {
        for (name, harness) in &self.harness {
            check_nonempty(issues, &format!("harness.{name}.bin"), &harness.bin);
            if let Some(contexts) = &harness.contexts {
                let unique: BTreeSet<_> = contexts.iter().collect();
                if contexts.is_empty()
                    || unique.len() != contexts.len()
                    || contexts
                        .iter()
                        .any(|context| !self.defaults.contexts.contains(context))
                {
                    issues.push(ConfigIssue {
                        key: format!("harness.{name}.contexts"),
                        message: "must be a nonempty, unique subset of defaults.contexts"
                            .to_string(),
                    });
                }
            }
            validate_injection(name, &harness.injection, issues);
            if let Some(config_dir) = &harness.config_dir {
                let prefix = format!("harness.{name}.config_dir");
                let entries = config_dir.entries();
                if entries.is_empty() {
                    issues.push(ConfigIssue {
                        key: format!("{prefix}.directories"),
                        message: "must declare at least one directory".to_string(),
                    });
                }
                for (environment, path) in entries {
                    check_environment_name(
                        issues,
                        &config_dir.environment_key(&prefix, environment),
                        environment,
                    );
                    check_not_playwright_owned(
                        issues,
                        &config_dir.environment_key(&prefix, environment),
                        environment,
                    );
                    let key = config_dir.path_key(&prefix, environment);
                    check_directory_template(issues, &key, path, harness);
                }
            }
        }
    }

    fn validate_models(&self, reserved: &BTreeSet<String>, issues: &mut Vec<ConfigIssue>) {
        for (name, model) in &self.model {
            if !self.harness.contains_key(&model.harness) {
                issues.push(ConfigIssue {
                    key: format!("model.{name}.harness"),
                    message: format!("unknown harness `{}`", model.harness),
                });
            }
            for environment in model.env.keys() {
                let key = format!("model.{name}.env.{environment}");
                check_environment_name(issues, &key, environment);
                check_not_reserved(issues, &key, environment, reserved);
            }
        }
    }

    fn validate_domains(&self, reserved: &BTreeSet<String>, issues: &mut Vec<ConfigIssue>) {
        for (name, domain) in &self.domain {
            if domain.profiles.is_empty() {
                issues.push(ConfigIssue {
                    key: format!("domain.{name}.profiles"),
                    message: "must contain at least one profile".to_string(),
                });
            }
            for (index, profile) in domain.profiles.iter().enumerate() {
                check_nonempty(issues, &format!("domain.{name}.profiles[{index}]"), profile);
            }
            if let Some(model) = &domain.default_model
                && !self.model.contains_key(model)
            {
                issues.push(ConfigIssue {
                    key: format!("domain.{name}.default_model"),
                    message: format!("unknown model `{model}`"),
                });
            }
            for environment in domain.env.keys() {
                let key = format!("domain.{name}.env.{environment}");
                check_environment_name(issues, &key, environment);
                check_not_reserved(issues, &key, environment, reserved);
            }
        }
    }
}

impl Config {
    fn validate_providers(&self, reserved: &BTreeSet<String>, issues: &mut Vec<ConfigIssue>) {
        let native = self.native_directories();
        for (name, provider) in &self.provider {
            for (harness_name, entry) in &provider.harness {
                let prefix = format!("provider.{name}.harness.{harness_name}");
                self.validate_provider_harness(&prefix, harness_name, entry, reserved, issues);
            }
            self.validate_provider_directories(name, &native, issues);
        }
    }

    /// Every directory a harness uses natively, rendered for every context the
    /// harness supports, mapped to the harness and context that own it.
    fn native_directories(&self) -> BTreeMap<String, (&HarnessName, &ContextName)> {
        let mut native = BTreeMap::new();
        for (harness_name, harness) in &self.harness {
            let Some(config_dir) = &harness.config_dir else {
                continue;
            };
            for context in self.supported_contexts(harness) {
                for (_, path) in config_dir.entries() {
                    native.insert(render_directory(path, context), (harness_name, context));
                }
            }
        }
        native
    }

    fn supported_contexts<'a>(&'a self, harness: &HarnessConfig) -> Vec<&'a ContextName> {
        self.defaults
            .contexts
            .iter()
            .filter(|context| harness.supports_context(context))
            .collect()
    }

    /// Report a derived provider directory that coincides with any harness's
    /// own directory in any context, which would share state the provider
    /// exists to separate. Derivation always changes the template, so this
    /// catches only cross-context and cross-harness coincidences.
    fn validate_provider_directories(
        &self,
        name: &ProviderName,
        native: &BTreeMap<String, (&HarnessName, &ContextName)>,
        issues: &mut Vec<ConfigIssue>,
    ) {
        for (harness_name, harness) in &self.harness {
            let Some(config_dir) = &harness.config_dir else {
                continue;
            };
            let derived = config_dir.for_provider(name);
            for context in self.supported_contexts(harness) {
                for (_, path) in derived.entries() {
                    let rendered = render_directory(path, context);
                    if let Some((owner, owner_context)) = native.get(&rendered) {
                        issues.push(ConfigIssue {
                            key: format!("provider.{name}"),
                            message: format!(
                                "derives {rendered} for harness `{harness_name}` in context `{context}`, which is harness `{owner}`'s own directory in context `{owner_context}`"
                            ),
                        });
                    }
                }
            }
        }
    }

    fn validate_provider_harness(
        &self,
        prefix: &str,
        harness_name: &HarnessName,
        entry: &ProviderHarnessConfig,
        reserved: &BTreeSet<String>,
        issues: &mut Vec<ConfigIssue>,
    ) {
        for environment in entry.env.keys() {
            let key = format!("{prefix}.env.{environment}");
            check_environment_name(issues, &key, environment);
            check_not_reserved(issues, &key, environment, reserved);
            for (model_name, model) in &self.model {
                if &model.harness == harness_name && model.env.contains_key(environment) {
                    issues.push(ConfigIssue {
                        key: key.clone(),
                        message: format!(
                            "also set by model `{model_name}`; a provider and a model for the same harness may not set the same variable"
                        ),
                    });
                }
            }
        }

        match self.harness.get(harness_name) {
            None => issues.push(ConfigIssue {
                key: prefix.to_string(),
                message: format!("unknown harness `{harness_name}`"),
            }),
            Some(harness) if harness.config_dir.is_none() => issues.push(ConfigIssue {
                key: prefix.to_string(),
                message: format!(
                    "harness `{harness_name}` declares no config_dir, so a provider cannot give it separate directories"
                ),
            }),
            Some(_) => {}
        }
    }
}

/// Rewrite one directory template for a provider by appending `-<provider>`
/// to one path component.
///
/// - When `{context}` is a whole component, the component before it gains the
///   suffix: `~/.config/claude/{context}` becomes
///   `~/.config/claude-zai/{context}`.
/// - When `{context}` sits inside a component, or the component before it is
///   empty, `~`, `.`, or `..`, that `{context}` component gains the suffix at
///   its end: `~/.c/x-{context}` becomes `~/.c/x-{context}-zai`.
/// - A fixed path with no `{context}` gains the suffix on its last component:
///   `~/.pi/agent` becomes `~/.pi/agent-zai`.
///
/// A trailing separator is dropped first, so the suffix never lands on an empty
/// component. The result always differs from the input.
#[must_use]
pub fn provider_template(template: &str, provider: &ProviderName) -> String {
    let trimmed = template.trim_end_matches('/');
    let mut components: Vec<String> = trimmed.split('/').map(str::to_string).collect();
    let context_index = components
        .iter()
        .position(|component| component.contains(CONTEXT_PLACEHOLDER));
    let target = match context_index {
        Some(index)
            if index > 0
                && components.get(index).map(String::as_str) == Some(CONTEXT_PLACEHOLDER)
                && components
                    .get(index - 1)
                    .is_some_and(|parent| !matches!(parent.as_str(), "" | "~" | "." | "..")) =>
        {
            index - 1
        }
        Some(index) => index,
        None => components.len().saturating_sub(1),
    };
    if let Some(component) = components.get_mut(target) {
        component.push('-');
        component.push_str(provider.as_str());
    }
    components.join("/")
}

/// Render a directory template for one context, ignoring trailing separators,
/// so two templates that name the same directory compare equal.
fn render_directory(template: &str, context: &ContextName) -> String {
    template
        .replace(CONTEXT_PLACEHOLDER, context.as_str())
        .trim_end_matches('/')
        .to_string()
}

/// Check one config-directory path template against its harness's context
/// restriction: a harness restricted to exactly one context may use a fixed
/// path; every other harness needs `{context}` in the template.
fn check_directory_template(
    issues: &mut Vec<ConfigIssue>,
    key: &str,
    path: &str,
    harness: &HarnessConfig,
) {
    if harness
        .contexts
        .as_ref()
        .is_some_and(|contexts| contexts.len() == 1)
    {
        check_nonempty(issues, key, path);
    } else {
        require_placeholder(issues, key, path, CONTEXT_PLACEHOLDER);
    }
}

fn validate_injection(
    name: &HarnessName,
    injection: &InjectionConfig,
    issues: &mut Vec<ConfigIssue>,
) {
    match injection {
        InjectionConfig::ArgText { args } => require_argument_placeholder(
            issues,
            &format!("harness.{name}.injection.args"),
            args,
            "{text}",
        ),
        InjectionConfig::ArgFile { args } => require_argument_placeholder(
            issues,
            &format!("harness.{name}.injection.args"),
            args,
            "{file}",
        ),
        InjectionConfig::EnvFile { environment }
        | InjectionConfig::OpencodeInstructions { environment } => {
            let key = format!("harness.{name}.injection.environment");
            check_environment_name(issues, &key, environment);
            check_not_playwright_owned(issues, &key, environment);
        }
    }
}

fn require_argument_placeholder(
    issues: &mut Vec<ConfigIssue>,
    key: &str,
    args: &[String],
    placeholder: &str,
) {
    if !args.iter().any(|argument| argument.contains(placeholder)) {
        issues.push(ConfigIssue {
            key: key.to_string(),
            message: format!("must contain `{placeholder}`"),
        });
    }
}

fn require_placeholder(issues: &mut Vec<ConfigIssue>, key: &str, value: &str, placeholder: &str) {
    if !value.contains(placeholder) {
        issues.push(ConfigIssue {
            key: key.to_string(),
            message: format!("must contain `{placeholder}`"),
        });
    }
}

fn check_nonempty(issues: &mut Vec<ConfigIssue>, key: &str, value: &str) {
    if value.trim().is_empty() {
        issues.push(ConfigIssue {
            key: key.to_string(),
            message: "must not be empty".to_string(),
        });
    }
}

fn check_environment_name(issues: &mut Vec<ConfigIssue>, key: &str, value: &str) {
    let mut bytes = value.bytes();
    let starts_valid = bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_');
    if !starts_valid || !bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
        issues.push(ConfigIssue {
            key: key.to_string(),
            message: format!("invalid environment variable name `{value}`"),
        });
    }
}

fn check_not_reserved(
    issues: &mut Vec<ConfigIssue>,
    key: &str,
    value: &str,
    reserved: &BTreeSet<String>,
) {
    if reserved.contains(value) || value.starts_with(SESSION_ENVIRONMENT_PREFIX) {
        issues.push(ConfigIssue {
            key: key.to_string(),
            message: format!("`{value}` is computed by clanker and must not be overridden"),
        });
    }
}

fn check_not_playwright_owned(issues: &mut Vec<ConfigIssue>, key: &str, value: &str) {
    if [
        PLAYWRIGHT_MCP_BROWSER_ENVIRONMENT,
        PLAYWRIGHT_MCP_USER_DATA_DIR_ENVIRONMENT,
    ]
    .contains(&value)
    {
        issues.push(ConfigIssue {
            key: key.to_string(),
            message: format!("`{value}` is computed by clanker and must not be overridden"),
        });
    }
}

/// Return the standard machine overlay path for a base config path.
#[must_use]
pub fn local_overlay_path(base_path: &Path) -> PathBuf {
    base_path.with_file_name("local.toml")
}

/// Load, deep-merge, deserialize, and validate runtime configuration.
///
/// The optional `local.toml` sibling replaces scalar/array values and merges
/// tables recursively.
///
/// # Errors
/// Returns [`ConfigError`] for missing files, I/O failures, invalid TOML,
/// schema mismatches, or cross-field validation failures.
pub fn load_config(base_path: &Path) -> Result<LoadedConfig, ConfigError> {
    let overlay_path = local_overlay_path(base_path);
    load_config_with_overlay(base_path, Some(&overlay_path))
}

/// Load runtime configuration with an explicit optional overlay.
///
/// # Errors
/// Returns [`ConfigError`] for missing files, I/O failures, invalid TOML,
/// schema mismatches, or cross-field validation failures.
pub fn load_config_with_overlay(
    base_path: &Path,
    overlay_path: Option<&Path>,
) -> Result<LoadedConfig, ConfigError> {
    let mut merged = read_toml(base_path, true)?.ok_or_else(|| ConfigError::Missing {
        path: base_path.to_path_buf(),
    })?;

    let applied_overlay = if let Some(path) = overlay_path {
        read_toml(path, false)?.map(|overlay| {
            deep_merge(&mut merged, overlay);
            path.to_path_buf()
        })
    } else {
        None
    };

    let sources = config_sources(base_path, applied_overlay.as_deref());
    let config: Config = merged.try_into().map_err(|source| ConfigError::Schema {
        sources: sources.clone(),
        source,
    })?;
    let issues = config.issues();
    if !issues.is_empty() {
        return Err(ConfigError::Invalid {
            sources,
            issues: issues
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        });
    }

    Ok(LoadedConfig {
        base_path: base_path.to_path_buf(),
        overlay_path: applied_overlay,
        config,
    })
}

fn config_sources(base_path: &Path, overlay_path: Option<&Path>) -> String {
    overlay_path.map_or_else(
        || base_path.display().to_string(),
        |overlay_path| {
            format!(
                "{} merged with {}",
                base_path.display(),
                overlay_path.display()
            )
        },
    )
}

fn read_toml(path: &Path, required: bool) -> Result<Option<toml::Value>, ConfigError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound && !required => {
            return Ok(None);
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(ConfigError::Missing {
                path: path.to_path_buf(),
            });
        }
        Err(source) => {
            return Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    toml::from_str(&text)
        .map(Some)
        .map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })
}

fn deep_merge(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base_table), toml::Value::Table(overlay_table)) => {
            for (key, overlay_value) in overlay_table {
                if let Some(base_value) = base_table.get_mut(&key) {
                    deep_merge(base_value, overlay_value);
                } else {
                    base_table.insert(key, overlay_value);
                }
            }
        }
        (base_value, overlay_value) => *base_value = overlay_value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const COMPLETE_CONFIG: &str = r#"
[defaults]
domain = "eng"
contexts = ["personal", "work"]
shim_path = "~/.local/clankers/bin"
sandbox_wrapper = "~/.config/sandbox-exec/run-sandboxed.sh"
prompter_bundle = "~/.local/prompter"
prompt_cache = "~/.cache/clanker/prompts"
prompt_cache_ttl_seconds = 86400

[harness.claude]
bin = "claude"
family = "claude"
default_args = []

[harness.claude.injection]
kind = "arg-text"
args = ["--append-system-prompt", "{text}"]

[model]

[domain.eng]
profiles = ["core.base", "domain.eng"]
env = {}
"#;

    #[test]
    fn harness_contexts_must_be_a_nonempty_unique_global_subset() {
        for contexts in ["[]", "[\"ghost\"]", "[\"personal\", \"personal\"]"] {
            let text = COMPLETE_CONFIG.replace(
                "[harness.claude]",
                &format!("[harness.claude]\ncontexts = {contexts}"),
            );
            let config: Config = toml::from_str(&text).unwrap();
            assert!(
                config
                    .issues()
                    .iter()
                    .any(|issue| issue.key == "harness.claude.contexts")
            );
        }
    }

    #[test]
    fn fixed_roots_require_exactly_one_explicit_context() {
        for (contexts, valid) in [
            ("", false),
            ("contexts = [\"personal\"]", true),
            ("contexts = [\"personal\", \"work\"]", false),
        ] {
            let text = format!(
                "{}\n[harness.claude.config_dir]\nenvironment = \"CLAUDE_CONFIG_DIR\"\npath = \"~/.claude\"\n",
                COMPLETE_CONFIG
                    .replace("[harness.claude]", &format!("[harness.claude]\n{contexts}"))
            );
            let config: Config = toml::from_str(&text).unwrap();
            assert_eq!(config.issues().is_empty(), valid);
        }
        let text = COMPLETE_CONFIG.replace(
            "[harness.claude]",
            "[harness.claude]\ncontexts = [\"personal\"]",
        ) + "\n[harness.claude.config_dir]\nenvironment = \"CLAUDE_CONFIG_DIR\"\npath = \" \"\n";
        let config: Config = toml::from_str(&text).unwrap();
        assert!(
            config
                .issues()
                .iter()
                .any(|issue| issue.key == "harness.claude.config_dir.path")
        );
    }

    #[test]
    fn issues_flag_every_semantic_violation() {
        let config: Config = toml::from_str(
            r#"
[defaults]
domain = "ghost"
contexts = []
shim_path = "  "
sandbox_wrapper = "ok"
prompter_bundle = "ok"
prompt_cache = "ok"
prompt_cache_ttl_seconds = 1

[harness.claude]
bin = ""
family = "claude"
default_args = []
[harness.claude.injection]
kind = "arg-text"
args = ["{text}"]
[harness.claude.config_dir]
environment = "1BAD"
path = "no-placeholder"

[model.m]
harness = "ghostharness"
env = { "1BADVAR" = "v" }
harness_args = []

[domain.eng]
profiles = ["", "ok"]
env = { "3BAD" = "v" }
default_model = "nomodel"
"#,
        )
        .unwrap();

        let keys: Vec<String> = config.issues().into_iter().map(|issue| issue.key).collect();
        for expected in [
            "defaults.shim_path",
            "defaults.domain",
            "defaults.contexts",
            "harness.claude.bin",
            "harness.claude.config_dir.environment",
            "harness.claude.config_dir.path",
            "model.m.harness",
            "model.m.env.1BADVAR",
            "domain.eng.profiles[0]",
            "domain.eng.default_model",
            "domain.eng.env.3BAD",
        ] {
            assert!(
                keys.iter().any(|key| key == expected),
                "missing issue key {expected}; got {keys:?}"
            );
        }
    }

    #[test]
    fn domain_env_may_not_name_a_clanker_owned_variable() {
        let config: Config =
            toml::from_str(&COMPLETE_CONFIG.replace("env = {}", "env = { PATH = \"/tmp\" }"))
                .unwrap();

        let issues = config.issues();
        assert!(issues.iter().any(|issue| {
            issue.key == "domain.eng.env.PATH" && issue.message.contains("computed by clanker")
        }));
    }

    #[test]
    fn domain_and_model_env_may_not_override_playwright_profiles() {
        let config: Config = toml::from_str(&format!(
            "{}\n[model.m]\nharness = \"claude\"\nenv = {{ PLAYWRIGHT_MCP_BROWSER = \"firefox\" }}\nharness_args = []\n",
            COMPLETE_CONFIG.replace(
                "env = {}",
                "env = { PLAYWRIGHT_MCP_USER_DATA_DIR = \"/tmp/human-profile\" }"
            )
            .replace("[model]\n", "")
        ))
        .unwrap();

        let issues = config.issues();
        for key in [
            "domain.eng.env.PLAYWRIGHT_MCP_USER_DATA_DIR",
            "model.m.env.PLAYWRIGHT_MCP_BROWSER",
        ] {
            assert!(issues.iter().any(|issue| {
                issue.key == key && issue.message.contains("computed by clanker")
            }));
        }
    }

    #[test]
    fn harness_boundaries_may_not_override_playwright_profiles() {
        let config: Config = toml::from_str(&format!(
            "{COMPLETE_CONFIG}\n[harness.claude.config_dir]\nenvironment = \"PLAYWRIGHT_MCP_USER_DATA_DIR\"\npath = \"~/.config/claude/{{context}}\"\n\n[harness.gemini]\nbin = \"gemini\"\nfamily = \"gemini\"\ndefault_args = []\n[harness.gemini.injection]\nkind = \"env-file\"\nenvironment = \"PLAYWRIGHT_MCP_BROWSER\"\n"
        ))
        .unwrap();

        let issues = config.issues();
        for key in [
            "harness.claude.config_dir.environment",
            "harness.gemini.injection.environment",
        ] {
            assert!(issues.iter().any(|issue| {
                issue.key == key && issue.message.contains("computed by clanker")
            }));
        }
    }

    /// `COMPLETE_CONFIG` with a Claude config directory, plus `extra`.
    fn provider_config(extra: &str) -> Config {
        toml::from_str(&format!(
            "{COMPLETE_CONFIG}\n[harness.claude.config_dir]\nenvironment = \"CLAUDE_CONFIG_DIR\"\npath = \"~/.config/claude/{{context}}\"\n{extra}"
        ))
        .unwrap()
    }

    fn issue_keys(config: &Config) -> Vec<String> {
        config.issues().into_iter().map(|issue| issue.key).collect()
    }

    fn zai() -> ProviderName {
        ProviderName::new("zai").unwrap()
    }

    const ZAI: &str = "
[provider.zai.harness.claude]
env = { ANTHROPIC_BASE_URL = \"https://api.z.ai/api/anthropic\" }
harness_args = [\"--model\", \"opus\"]
";

    #[test]
    fn a_configuration_without_providers_parses_with_an_empty_provider_map() {
        let config: Config = toml::from_str(COMPLETE_CONFIG).unwrap();
        assert!(config.provider.is_empty());
        assert!(config.issues().is_empty());
    }

    #[test]
    fn an_empty_provider_table_is_a_complete_provider() {
        let config = provider_config("[provider.zai]\n");
        assert!(config.issues().is_empty(), "{:?}", config.issues());
        assert!(config.provider.get("zai").unwrap().harness.is_empty());
    }

    #[test]
    fn a_valid_provider_with_per_harness_additions_produces_no_issues() {
        let config = provider_config(ZAI);
        assert!(config.issues().is_empty(), "{:?}", config.issues());
        let entry = config
            .provider
            .get("zai")
            .and_then(|provider| provider.harness.get("claude"))
            .unwrap();
        assert_eq!(
            entry.env.get("ANTHROPIC_BASE_URL").map(String::as_str),
            Some("https://api.z.ai/api/anthropic")
        );
        assert_eq!(entry.harness_args, ["--model", "opus"]);
    }

    #[test]
    fn provider_env_and_args_default_to_empty() {
        let config = provider_config("[provider.zai.harness.claude]\n");
        assert!(config.issues().is_empty());
        let entry = config
            .provider
            .get("zai")
            .and_then(|provider| provider.harness.get("claude"))
            .unwrap();
        assert!(entry.env.is_empty());
        assert!(entry.harness_args.is_empty());
    }

    #[test]
    fn a_provider_rejects_unknown_fields_including_a_declared_directory() {
        for extra in [
            "[provider.zai]\nbase_url = \"x\"\n",
            "[provider.zai.harness.claude]\nbase_url = \"x\"\n",
            "[provider.zai.harness.claude.config_dir]\nenvironment = \"CLAUDE_CONFIG_DIR\"\npath = \"~/x/{context}\"\n",
        ] {
            assert!(
                toml::from_str::<Config>(&format!("{COMPLETE_CONFIG}\n{extra}")).is_err(),
                "{extra}"
            );
        }
    }

    #[test]
    fn a_provider_entry_for_an_unknown_harness_is_reported() {
        let config = provider_config("[provider.zai.harness.ghost]\n");
        let issues = config.issues();
        assert!(issues.iter().any(|issue| {
            issue.key == "provider.zai.harness.ghost" && issue.message.contains("unknown harness")
        }));
    }

    #[test]
    fn a_provider_entry_for_a_harness_without_a_config_dir_is_reported() {
        let config: Config = toml::from_str(&format!("{COMPLETE_CONFIG}{ZAI}")).unwrap();
        let issues = config.issues();
        assert!(issues.iter().any(|issue| {
            issue.key == "provider.zai.harness.claude"
                && issue.message.contains("declares no config_dir")
        }));
    }

    #[test]
    fn provider_templates_suffix_the_component_before_a_whole_context_component() {
        assert_eq!(
            provider_template("~/.config/claude/{context}", &zai()),
            "~/.config/claude-zai/{context}"
        );
        assert_eq!(
            provider_template("~/.config/claude/{context}/", &zai()),
            "~/.config/claude-zai/{context}"
        );
        assert_eq!(
            provider_template("/var/lib/agent/{context}/state", &zai()),
            "/var/lib/agent-zai/{context}/state"
        );
    }

    #[test]
    fn provider_templates_suffix_an_embedded_or_top_level_context_component() {
        assert_eq!(
            provider_template("~/.c/x-{context}", &zai()),
            "~/.c/x-{context}-zai"
        );
        assert_eq!(provider_template("~/{context}", &zai()), "~/{context}-zai");
        assert_eq!(provider_template("{context}", &zai()), "{context}-zai");
    }

    #[test]
    fn provider_templates_suffix_the_last_component_of_a_fixed_path() {
        assert_eq!(provider_template("~/.pi/agent", &zai()), "~/.pi/agent-zai");
        assert_eq!(provider_template("~/.pi/agent/", &zai()), "~/.pi/agent-zai");
    }

    #[test]
    fn both_directory_shapes_derive_every_entry_and_keep_their_variables() {
        let single = ConfigDirectory::Single(SingleDirectory {
            environment: "CLAUDE_CONFIG_DIR".to_string(),
            path: "~/.config/claude/{context}".to_string(),
        });
        assert_eq!(
            single.for_provider(&zai()).entries(),
            [("CLAUDE_CONFIG_DIR", "~/.config/claude-zai/{context}")]
        );

        let multiple = ConfigDirectory::Multiple(MultipleDirectories {
            directories: [
                ("A_CONFIG", "~/.config/a/{context}"),
                ("A_DATA", "~/.local/share/a/{context}"),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
        });
        assert_eq!(
            multiple.for_provider(&zai()).entries(),
            [
                ("A_CONFIG", "~/.config/a-zai/{context}"),
                ("A_DATA", "~/.local/share/a-zai/{context}"),
            ]
        );
    }

    #[test]
    fn a_derived_directory_that_is_another_harness_directory_is_reported() {
        let config = provider_config(
            "[provider.zai]\n[harness.claude-zai]\nbin = \"claude\"\nfamily = \"claude\"\ndefault_args = []\n[harness.claude-zai.injection]\nkind = \"arg-text\"\nargs = [\"{text}\"]\n[harness.claude-zai.config_dir]\nenvironment = \"OTHER_DIR\"\npath = \"~/.config/claude-zai/{context}\"\n",
        );
        assert!(config.issues().iter().any(|issue| {
            issue.key == "provider.zai" && issue.message.contains("harness `claude-zai`")
        }));
    }

    #[test]
    fn a_derived_directory_that_is_another_context_directory_is_reported() {
        let text = format!(
            "{}\n[harness.claude.config_dir]\nenvironment = \"CLAUDE_CONFIG_DIR\"\npath = \"~/.c/x-{{context}}\"\n[provider.zai]\n",
            COMPLETE_CONFIG.replace(
                "contexts = [\"personal\", \"work\"]",
                "contexts = [\"a\", \"a-zai\"]"
            )
        );
        let config: Config = toml::from_str(&text).unwrap();
        assert!(config.issues().iter().any(|issue| {
            issue.key == "provider.zai" && issue.message.contains("in context `a-zai`")
        }));
    }

    #[test]
    fn provider_env_may_not_name_an_invalid_or_reserved_variable() {
        let config = provider_config(
            "[provider.zai.harness.claude]\nenv = { \"1BAD\" = \"v\", CLAUDE_CONFIG_DIR = \"/tmp\", CLANKER_SESSION_X = \"v\", PATH = \"/bin\", CLANKER_PROVIDER = \"x\" }\n",
        );
        let issues = config.issues();
        for (variable, message) in [
            ("1BAD", "invalid environment variable name"),
            ("CLAUDE_CONFIG_DIR", "computed by clanker"),
            ("CLANKER_SESSION_X", "computed by clanker"),
            ("PATH", "computed by clanker"),
            ("CLANKER_PROVIDER", "computed by clanker"),
        ] {
            assert!(
                issues.iter().any(|issue| {
                    issue.key == format!("provider.zai.harness.claude.env.{variable}")
                        && issue.message.contains(message)
                }),
                "{variable}: {issues:?}"
            );
        }
    }

    #[test]
    fn a_provider_and_a_model_for_one_harness_may_not_set_the_same_variable() {
        let text = format!(
            "{}\n[harness.claude.config_dir]\nenvironment = \"CLAUDE_CONFIG_DIR\"\npath = \"~/.config/claude/{{context}}\"\n[model.glm]\nharness = \"claude\"\nenv = {{ ANTHROPIC_BASE_URL = \"https://elsewhere\" }}\nharness_args = []\n{ZAI}",
            COMPLETE_CONFIG.replace("[model]\n", "")
        );
        let config: Config = toml::from_str(&text).unwrap();
        let issues = config.issues();
        assert!(issues.iter().any(|issue| {
            issue.key == "provider.zai.harness.claude.env.ANTHROPIC_BASE_URL"
                && issue.message.contains("model `glm`")
        }));
        assert!(
            issue_keys(&config)
                .iter()
                .all(|key| key.starts_with("provider.zai"))
        );
    }

    #[test]
    fn model_env_may_not_name_a_session_marker_or_config_directory() {
        let config: Config = toml::from_str(&format!(
            "{}\n[harness.claude.config_dir]\nenvironment = \"CLAUDE_CONFIG_DIR\"\npath = \"~/.config/claude/{{context}}\"\n\n[model.m]\nharness = \"claude\"\nenv = {{ CLANKER_SESSION_CONTEXT = \"x\", CLAUDE_CONFIG_DIR = \"/tmp\" }}\nharness_args = []\n",
            COMPLETE_CONFIG.replace("[model]\n", "")
        ))
        .unwrap();

        let keys: Vec<String> = config.issues().into_iter().map(|issue| issue.key).collect();
        assert!(
            keys.iter()
                .any(|key| key == "model.m.env.CLANKER_SESSION_CONTEXT")
        );
        assert!(
            keys.iter()
                .any(|key| key == "model.m.env.CLAUDE_CONFIG_DIR")
        );
    }

    #[test]
    fn an_env_file_injection_target_is_reserved() {
        let config: Config = toml::from_str(&format!(
            "{}\n[harness.gemini]\nbin = \"gemini\"\nfamily = \"gemini\"\ndefault_args = []\n[harness.gemini.injection]\nkind = \"env-file\"\nenvironment = \"GEMINI_SYSTEM_MD\"\n",
            COMPLETE_CONFIG.replace("env = {}", "env = { GEMINI_SYSTEM_MD = \"/tmp/x\" }")
        ))
        .unwrap();

        assert!(
            config
                .issues()
                .iter()
                .any(|issue| issue.key == "domain.eng.env.GEMINI_SYSTEM_MD")
        );
    }

    #[test]
    fn an_opencode_instructions_target_is_validated_and_reserved() {
        let invalid_config = COMPLETE_CONFIG;
        let invalid: Config = toml::from_str(&format!(
            "{invalid_config}\n[harness.opencode]\nbin = \"opencode\"\nfamily = \"gpt\"\ndefault_args = []\n[harness.opencode.injection]\nkind = \"opencode-instructions\"\nenvironment = \"1BAD\"\n"
        ))
        .unwrap();
        assert!(invalid.issues().iter().any(|issue| {
            issue.key == "harness.opencode.injection.environment"
                && issue.message.contains("invalid environment variable")
        }));

        let reserved_config =
            COMPLETE_CONFIG.replace("env = {}", "env = { OPENCODE_CONFIG_CONTENT = \"{}\" }");
        let reserved: Config = toml::from_str(&format!(
            "{reserved_config}\n[harness.opencode]\nbin = \"opencode\"\nfamily = \"gpt\"\ndefault_args = []\n[harness.opencode.injection]\nkind = \"opencode-instructions\"\nenvironment = \"OPENCODE_CONFIG_CONTENT\"\n"
        ))
        .unwrap();
        assert!(reserved.issues().iter().any(|issue| {
            issue.key == "domain.eng.env.OPENCODE_CONFIG_CONTENT"
                && issue.message.contains("computed by clanker")
        }));
    }

    #[test]
    fn deep_merge_replaces_scalars_and_preserves_siblings() {
        let mut base: toml::Value = toml::from_str(
            r#"
[defaults]
domain = "eng"
shim_path = "a"
"#,
        )
        .unwrap();
        let overlay: toml::Value = toml::from_str(
            r#"
[defaults]
shim_path = "b"
"#,
        )
        .unwrap();

        deep_merge(&mut base, overlay);

        assert_eq!(base["defaults"]["domain"].as_str(), Some("eng"));
        assert_eq!(base["defaults"]["shim_path"].as_str(), Some("b"));
    }

    #[test]
    fn configured_names_reject_path_components() {
        assert!(HarnessName::new("claude").is_ok());
        assert!(HarnessName::new("../claude").is_err());
        assert!(ContextName::new("").is_err());
    }

    #[test]
    fn warnings_table_is_an_unknown_field_schema_error() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");
        fs::write(
            &path,
            format!("{COMPLETE_CONFIG}\n[warnings]\nunknown_profile = \"{{invocation}}\"\n"),
        )
        .unwrap();

        let error = load_config_with_overlay(&path, None).unwrap_err();

        assert!(error.to_string().contains("unknown field `warnings`"));
    }

    #[test]
    fn a_removed_field_is_now_an_unknown_field_schema_error() {
        let temp = TempDir::new().unwrap();
        for removed in [
            "context_fallback = \"personal\"",
            "context_by_hostname = {}",
            "skills_bundle = \"~/.local/clanker/skills\"",
        ] {
            let path = temp.path().join("config.toml");
            fs::write(
                &path,
                COMPLETE_CONFIG.replace("[defaults]", &format!("[defaults]\n{removed}")),
            )
            .unwrap();

            let error = load_config_with_overlay(&path, None).unwrap_err();
            assert!(
                error.to_string().contains("unknown field"),
                "expected a schema error for {removed}, got: {error}"
            );
        }
    }

    #[test]
    fn omitted_contexts_registry_is_a_schema_error() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");
        fs::write(
            &path,
            COMPLETE_CONFIG.replace("contexts = [\"personal\", \"work\"]\n", ""),
        )
        .unwrap();

        let error = load_config_with_overlay(&path, None).unwrap_err();

        assert!(error.to_string().contains("missing field `contexts`"));
    }

    #[test]
    fn omitted_harness_default_args_is_a_schema_error() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");
        fs::write(&path, COMPLETE_CONFIG.replace("default_args = []\n", "")).unwrap();

        let error = load_config_with_overlay(&path, None).unwrap_err();
        let message = error.to_string();

        assert!(message.contains(path.to_str().unwrap()));
        assert!(message.contains("missing field `default_args`"));
    }

    #[test]
    fn unknown_base_parameter_is_a_hard_schema_error() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");
        fs::write(
            &path,
            COMPLETE_CONFIG.replace(
                "prompter_bundle = \"~/.local/prompter\"",
                "prompter_bundle = \"~/.local/prompter\"\nunknown_parameter = true",
            ),
        )
        .unwrap();

        let error = load_config_with_overlay(&path, None).unwrap_err();
        let message = error.to_string();

        assert!(message.contains(path.to_str().unwrap()));
        assert!(message.contains("unknown field `unknown_parameter`"));
    }

    #[test]
    fn unknown_overlay_parameter_is_a_hard_schema_error() {
        let temp = TempDir::new().unwrap();
        let base_path = temp.path().join("config.toml");
        let overlay_path = temp.path().join("local.toml");
        fs::write(&base_path, COMPLETE_CONFIG).unwrap();
        fs::write(
            &overlay_path,
            "[harness.claude]\nunknown_parameter = true\n",
        )
        .unwrap();

        let error = load_config_with_overlay(&base_path, Some(&overlay_path)).unwrap_err();
        let message = error.to_string();

        assert!(message.contains(base_path.to_str().unwrap()));
        assert!(message.contains(overlay_path.to_str().unwrap()));
        assert!(message.contains("unknown field `unknown_parameter`"));
    }

    #[test]
    fn empty_harness_map_is_flagged() {
        let mut config: Config = toml::from_str(COMPLETE_CONFIG).unwrap();
        config.harness.clear();

        let keys: Vec<String> = config.issues().into_iter().map(|issue| issue.key).collect();
        assert!(keys.iter().any(|key| key == "harness"));
    }

    #[test]
    fn load_config_reports_read_error_for_directory_path() {
        let temp = TempDir::new().unwrap();
        let directory = temp.path().join("config-dir");
        fs::create_dir_all(&directory).unwrap();

        let error = load_config(&directory).unwrap_err();
        assert!(matches!(error, ConfigError::Read { .. }));
    }

    const MULTI_DIRECTORY_HARNESS: &str = r#"
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
POLYTOKEN_CACHE_PATH = "~/.cache/polytoken/{context}"
"#;

    fn harness_config_dir<'a>(config: &'a Config, harness: &str) -> &'a ConfigDirectory {
        let name = HarnessName::new(harness).unwrap();
        config.harness[&name].config_dir.as_ref().unwrap()
    }

    #[test]
    fn a_multi_directory_harness_validates_and_exposes_every_entry() {
        let config: Config =
            toml::from_str(&format!("{COMPLETE_CONFIG}{MULTI_DIRECTORY_HARNESS}")).unwrap();

        assert_eq!(config.issues(), Vec::new());
        assert_eq!(
            harness_config_dir(&config, "polytoken").entries(),
            vec![
                ("POLYTOKEN_CACHE_PATH", "~/.cache/polytoken/{context}"),
                ("POLYTOKEN_CONFIG_PATH", "~/.config/polytoken/{context}"),
                ("POLYTOKEN_DATA_PATH", "~/.local/share/polytoken/{context}"),
            ]
        );
    }

    #[test]
    fn a_single_directory_harness_still_exposes_one_entry() {
        let config: Config = toml::from_str(&format!(
            "{COMPLETE_CONFIG}
[harness.claude.config_dir]
environment = \"CLAUDE_CONFIG_DIR\"
path = \"~/.config/claude/{{context}}\"
"
        ))
        .unwrap();

        assert_eq!(config.issues(), Vec::new());
        assert_eq!(
            harness_config_dir(&config, "claude").entries(),
            vec![("CLAUDE_CONFIG_DIR", "~/.config/claude/{context}")]
        );
    }

    #[test]
    fn a_multi_directory_entry_is_validated_by_variable_name() {
        let config: Config = toml::from_str(&format!(
            "{COMPLETE_CONFIG}
[harness.polytoken]
bin = \"polytoken\"
family = \"gpt\"
default_args = []
[harness.polytoken.injection]
kind = \"env-file\"
environment = \"CLANKER_PROMPT_FILE\"
[harness.polytoken.config_dir.directories]
\"1BAD\" = \"~/.config/polytoken/{{context}}\"
POLYTOKEN_DATA_PATH = \"~/.local/share/polytoken\"
"
        ))
        .unwrap();

        let issues = config.issues();
        assert!(issues.iter().any(|issue| {
            issue.key == "harness.polytoken.config_dir.directories.1BAD"
                && issue.message.contains("invalid environment variable name")
        }));
        assert!(issues.iter().any(|issue| {
            issue.key == "harness.polytoken.config_dir.directories.POLYTOKEN_DATA_PATH"
                && issue.message.contains("{context}")
        }));
    }

    #[test]
    fn an_empty_directories_map_is_flagged() {
        let config: Config = toml::from_str(&format!(
            "{COMPLETE_CONFIG}
[harness.polytoken]
bin = \"polytoken\"
family = \"gpt\"
default_args = []
[harness.polytoken.injection]
kind = \"env-file\"
environment = \"CLANKER_PROMPT_FILE\"
[harness.polytoken.config_dir.directories]
"
        ))
        .unwrap();

        assert!(config.issues().iter().any(|issue| {
            issue.key == "harness.polytoken.config_dir.directories"
                && issue.message.contains("at least one directory")
        }));
    }

    #[test]
    fn model_env_may_not_name_a_multi_form_directory_variable() {
        let config: Config = toml::from_str(&format!(
            "{}{MULTI_DIRECTORY_HARNESS}
[model.m]
harness = \"polytoken\"
env = {{ POLYTOKEN_DATA_PATH = \"/tmp\" }}
harness_args = []
",
            COMPLETE_CONFIG.replace("[model]\n", "")
        ))
        .unwrap();

        assert!(config.issues().iter().any(|issue| {
            issue.key == "model.m.env.POLYTOKEN_DATA_PATH"
                && issue.message.contains("computed by clanker")
        }));
    }

    #[test]
    fn declaring_both_directory_shapes_is_rejected() {
        let error = toml::from_str::<Config>(&format!(
            "{COMPLETE_CONFIG}\n[harness.claude.config_dir]\nenvironment = \"CLAUDE_CONFIG_DIR\"\npath = \"~/.config/claude/{{context}}\"\n[harness.claude.config_dir.directories]\nOTHER = \"~/.config/other/{{context}}\"\n"
        ))
        .unwrap_err();

        assert!(
            error.to_string().contains("not both"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn a_half_declared_single_directory_is_rejected() {
        let error = toml::from_str::<Config>(&format!(
            "{COMPLETE_CONFIG}\n[harness.claude.config_dir]\nenvironment = \"CLAUDE_CONFIG_DIR\"\n"
        ))
        .unwrap_err();

        assert!(
            error.to_string().contains("must be declared together"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn an_empty_config_directory_table_is_rejected() {
        let error =
            toml::from_str::<Config>(&format!("{COMPLETE_CONFIG}\n[harness.claude.config_dir]\n"))
                .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("expected `environment` and `path`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn an_unrecognized_directory_key_fails_to_deserialize() {
        let error = toml::from_str::<Config>(&format!(
            "{COMPLETE_CONFIG}
[harness.claude.config_dir]
environment = \"CLAUDE_CONFIG_DIR\"
directory = \"~/.config/claude/{{context}}\"
"
        ))
        .unwrap_err();

        let message = error.to_string();
        assert!(
            message.contains("unknown field `directory`"),
            "unexpected error: {message}"
        );
        assert!(
            message.contains("`environment`, `path`, `directories`"),
            "unexpected error: {message}"
        );
    }
}
