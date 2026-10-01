//! CLI argument types and mode resolution.
use super::profile::FamilyName;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tftio_lib::AgentSubcommand;
pub use tftio_lib::MetaCommand;

#[derive(Parser, Debug)]
#[command(name = "prompter")]
#[command(about = "Compose reusable prompt snippets from profile definitions")]
#[command(long_about = None)]
/// CLI argument struct for the prompter tool.
#[command(version)]
pub struct Cli {
    /// Subcommand to execute
    #[command(subcommand)]
    pub command: Commands,

    /// Override configuration file path
    #[arg(short = 'c', long, value_name = "FILE", global = true)]
    pub config: Option<PathBuf>,

    /// Output in JSON format
    #[arg(short = 'j', long, global = true)]
    pub json: bool,
}

/// Available subcommands for the prompter CLI.
///
/// Each variant represents a different operation mode of the tool.
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Shared metadata commands.
    Meta {
        /// Shared metadata command to execute.
        #[command(subcommand)]
        command: MetaCommand,
    },
    /// Initialize default config and library
    Init,
    /// List available profiles
    List,
    /// Show dependency tree for profiles
    Tree,
    /// Validate configuration and library references
    Validate,
    /// Render one or more profiles (concatenated file contents with deduplication)
    Run {
        /// Profile name(s) to render
        #[arg(required = true)]
        profiles: Vec<String>,
        /// Model family used to select fragment variants
        #[arg(long, value_name = "NAME")]
        family: Option<FamilyName>,
        /// Separator between files
        #[arg(short, long)]
        separator: Option<String>,
        /// Pre-prompt text to inject at the beginning
        #[arg(short = 'p', long)]
        pre_prompt: Option<String>,
        /// Post-prompt text to inject at the end
        #[arg(short = 'P', long)]
        post_prompt: Option<String>,
        /// Emit only the deduplicated fragments, suppressing the auto-injected
        /// default pre-prompt, the date/system context, and the post-prompt
        /// (an explicit --pre-prompt/--post-prompt still applies)
        #[arg(short = 'b', long)]
        bare: bool,
    },
    /// Render the always-on invariant base (the agent system prompt).
    ///
    /// Emits the `core.base` profile, optionally followed by additional
    /// profiles. Intended for harness/launcher startup: stamp the output into
    /// each agent's system-prompt site (Codex AGENTS.md, Claude
    /// --append-system-prompt, pi SYSTEM.md). Skills inject task context with
    /// `run` and do NOT re-emit this base.
    System {
        /// Additional profile(s) to append after the system base
        profiles: Vec<String>,
        /// Separator between files
        #[arg(short, long)]
        separator: Option<String>,
        /// Pre-prompt text to inject at the beginning
        #[arg(short = 'p', long)]
        pre_prompt: Option<String>,
        /// Post-prompt text to inject at the end
        #[arg(short = 'P', long)]
        post_prompt: Option<String>,
        /// Emit only the deduplicated fragments, suppressing the auto-injected
        /// default pre-prompt, the date/system context, and the post-prompt
        /// (an explicit --pre-prompt/--post-prompt still applies)
        #[arg(short = 'b', long)]
        bare: bool,
    },
}

/// Whether rendered output carries its auto-injected framing context.
///
/// `Full` wraps the deduplicated fragment bodies in the pre-prompt, the
/// date/system prefix, and the post-prompt. `Bare` suppresses everything
/// auto-injected — the default pre-prompt, the date/system stamp, and the
/// default/config post-prompt — leaving only the fragment bodies, for callers
/// that want just the composed prompt (e.g. a token-frugal local model that
/// caches the whole system prompt and is defeated by the per-call date line).
///
/// In both modes an explicit `--pre-prompt`/`--post-prompt` is honored: `Bare`
/// drops defaults, not text the user asked for. The date/system stamp is never
/// user-supplied, so `Bare` always omits it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// Surround fragments with the pre-prompt, system prefix, and post-prompt.
    Full,
    /// Emit only the deduplicated fragment bodies.
    Bare,
}

impl Framing {
    /// Convert a `--bare` flag into a framing mode.
    #[must_use]
    pub const fn from_bare_flag(bare: bool) -> Self {
        if bare { Self::Bare } else { Self::Full }
    }

    /// Whether this mode emits the surrounding framing context.
    #[must_use]
    pub const fn is_full(self) -> bool {
        matches!(self, Self::Full)
    }
}

/// Application execution modes after parsing command-line arguments.
///
/// This enum represents the resolved execution mode after processing
/// both subcommands and direct profile arguments.
#[derive(Debug)]
pub enum AppMode {
    /// Render one or more profiles with optional separator and pre-prompt
    Run {
        /// Profile name(s) to render
        profiles: Vec<String>,
        /// Optional model family used to select fragment variants
        family: Option<FamilyName>,
        /// Optional separator between concatenated files
        separator: Option<String>,
        /// Optional custom pre-prompt text
        pre_prompt: Option<String>,
        /// Optional custom post-prompt text
        post_prompt: Option<String>,
        /// Whether to wrap fragments in framing context or emit bare bodies
        framing: Framing,
        /// Optional configuration file override
        config: Option<PathBuf>,
        /// Output in JSON format
        json: bool,
    },
    /// List all available profiles using an optional config override
    List {
        /// Optional configuration file override
        config: Option<PathBuf>,
        /// Output in JSON format
        json: bool,
    },
    /// Show dependency tree for profiles
    Tree {
        /// Optional configuration file override
        config: Option<PathBuf>,
        /// Output in JSON format
        json: bool,
    },
    /// Validate configuration and library references with an optional config override
    Validate {
        /// Optional configuration file override
        config: Option<PathBuf>,
        /// Output in JSON format
        json: bool,
    },
    /// Initialize default configuration and library
    Init,
    /// Show version information
    Version {
        /// Output in JSON format
        json: bool,
    },
    /// Show license information
    License,
    /// Show help information
    Help,
    /// Generate shell completion scripts
    Completions {
        /// Shell to generate completions for
        shell: clap_complete::Shell,
    },
    /// Check health and configuration status
    Doctor {
        /// Output in JSON format
        json: bool,
    },
    /// Agent-skill artifact subcommand (routed through cli-common).
    Agent {
        /// Subcommand to dispatch.
        command: AgentSubcommand,
    },
}
