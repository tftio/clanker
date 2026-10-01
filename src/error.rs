//! Clanker application error types.

use crate::cli::LaunchArgumentError;
use crate::config::ConfigError;
use crate::launch::LaunchError;
use crate::promptlib::PrompterError;
use crate::session::SessionError;

/// Failures returned by clanker command and symlink modes.
#[derive(Debug, thiserror::Error)]
pub enum ClankerError {
    /// Required home directory is unavailable.
    #[error("$HOME is not set; set HOME or pass --config with an absolute path")]
    HomeNotSet,
    /// Current working directory could not be read.
    #[error("failed to resolve current working directory: {0}")]
    CurrentDirectory(std::io::Error),
    /// A process environment value is not UTF-8.
    #[error("environment variable {0} is not valid UTF-8")]
    NonUtf8Environment(&'static str),
    /// Launch-selection argument parsing failed.
    #[error(transparent)]
    LaunchArguments(#[from] LaunchArgumentError),
    /// Runtime config loading failed.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// Launch planning or exec failed.
    #[error(transparent)]
    Launch(#[from] LaunchError),
    /// Inherited current-session markers are malformed.
    #[error(transparent)]
    Session(#[from] SessionError),
    /// JSON output serialization failed.
    #[error("failed to serialize output: {0}")]
    Json(#[from] serde_json::Error),
    /// A resolved environment value cannot be represented as shell text.
    #[error("environment value for {0} is not valid UTF-8")]
    NonUtf8EnvironmentOutput(String),
    /// A metadata command reached the domain-command handler unexpectedly.
    #[error("metadata command was not routed by the shared CLI runner")]
    UnroutedMetadataCommand,
    /// Prompt composition, listing, tree, or validation failed.
    #[error(transparent)]
    Prompt(#[from] PrompterError),
    /// Discovering a directory's project declaration or origin remote failed.
    #[error(transparent)]
    ProjectDiscovery(#[from] tftio_lib::project::DiscoveryError),
}
