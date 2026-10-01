//! Error types for the prompter crate.

use super::profile::ResolveError;
use std::path::PathBuf;

/// All errors that can occur during prompter configuration loading, profile
/// resolution, rendering, and scaffolding.
///
/// Each variant names a distinct failure mode a caller might branch on.
/// Variants that carry a bare `String` do so only for human-facing context
/// (a library/serializer message, a clap usage string) that callers merely
/// display rather than match on.
#[derive(thiserror::Error, Debug)]
pub enum PrompterError {
    /// An I/O operation failed, annotated with the path it concerned.
    #[error("Failed to access {path}: {source}")]
    Io {
        /// The filesystem path the failing operation targeted.
        path: PathBuf,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// A write to an output sink failed.
    #[error("Write error: {0}")]
    Write(std::io::Error),

    /// The home directory could not be located (`$HOME` unset).
    #[error("$HOME not set")]
    HomeNotSet,

    /// The current working directory could not be resolved.
    #[error("Failed to resolve working directory: {0}")]
    WorkingDir(std::io::Error),

    /// A config file path lacked a parent directory.
    #[error("Config path {0} has no parent directory")]
    NoParentDir(PathBuf),

    /// The TOML in a config file was syntactically invalid.
    #[error("Invalid TOML: {0}")]
    InvalidToml(#[from] toml::de::Error),

    /// A reserved top-level config field had the wrong type or shape.
    #[error("{0}")]
    ConfigField(String),

    /// A profile name was defined more than once across the merged bundle.
    #[error("{0}")]
    DuplicateProfile(String),

    /// An `import` chain referred back to a config file already being loaded.
    #[error("Import cycle detected at {0}")]
    ImportCycle(PathBuf),

    /// A profile dependency could not be resolved.
    #[error(transparent)]
    Resolve(#[from] ResolveError),

    /// Validation found one or more problems; the message aggregates them.
    #[error("{0}")]
    Validation(String),

    /// A value could not be serialized to JSON.
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    /// Command-line arguments could not be parsed; the payload is clap's
    /// formatted usage/error text, shown to the user verbatim.
    #[error("{0}")]
    ArgParse(String),
}
