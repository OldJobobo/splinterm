//! Clap command grammar shared by application dispatchers.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use splinterm_core::{Axis, DojoId, LairId, SplintId, SplitSide};

#[derive(Debug, Parser)]
#[command(version, about = "Splinterm terminal client")]
pub(in crate::app) struct Cli {
    /// Select human output or the supported JSON machine contract.
    #[arg(long, global = true, value_enum)]
    pub(in crate::app) output: Option<OutputMode>,
    /// Select the public machine schema major.
    #[arg(long, global = true, value_parser = clap::value_parser!(u16).range(1..))]
    pub(in crate::app) schema_major: Option<u16>,
    /// Bound a machine request deadline in milliseconds.
    #[arg(long, global = true, value_parser = clap::value_parser!(u64).range(1..=300_000))]
    pub(in crate::app) timeout_ms: Option<u64>,
    /// Bind this invocation to one configured remote daemon endpoint.
    #[arg(long, global = true, value_name = "PROFILE")]
    pub(in crate::app) remote: Option<String>,
    #[command(subcommand)]
    pub(in crate::app) command: Option<Command>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(in crate::app) enum OutputMode {
    Human,
    Json,
    Ndjson,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(in crate::app) enum SplitAxis {
    Horizontal,
    Vertical,
}

impl From<SplitAxis> for Axis {
    fn from(value: SplitAxis) -> Self {
        match value {
            SplitAxis::Horizontal => Self::Horizontal,
            SplitAxis::Vertical => Self::Vertical,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(in crate::app) enum NewSplintSide {
    First,
    Second,
}

impl From<NewSplintSide> for SplitSide {
    fn from(value: NewSplintSide) -> Self {
        match value {
            NewSplintSide::First => Self::First,
            NewSplintSide::Second => Self::Second,
        }
    }
}

#[derive(Debug, Subcommand)]
pub(in crate::app) enum Command {
    /// Choose a recent running logical Dojo or create a fresh terminal.
    Sessions,
    /// Reopen the most recently opened logical Dojo that is still running.
    Reopen,
    /// Render ordered snapshots of one explicitly selected terminal.
    Window {
        /// Select a Lair by stable identity (required with --dojo-id).
        #[arg(long, requires = "dojo_id")]
        lair_id: Option<LairId>,
        /// Select one daemon-owned Dojo (required with --lair-id).
        #[arg(long, requires = "lair_id")]
        dojo_id: Option<DojoId>,
    },
    Ping,
    /// Read only the keyboard-focused graphical Splint ID and current working directory.
    Focus,
    /// Stop the daemon, back up and clear every session, then restart cleanly.
    Reset {
        /// Confirm termination of every daemon-owned shell without prompting.
        #[arg(long)]
        yes: bool,
    },
    /// List active Lairs without flooding the terminal with exited history.
    List {
        /// Include exited-only sessions and their complete topology.
        #[arg(long)]
        all: bool,
    },
    /// Inspect effective authority or revoke ephemeral grants.
    Authorization {
        #[command(subcommand)]
        command: AuthorizationCommand,
    },
    /// Bridge private protocol bytes over non-terminal stdin/stdout.
    Relay {
        /// Use stdin and stdout as one byte-transparent automation transport.
        #[arg(
            long,
            conflicts_with = "graphical_stdio",
            required_unless_present = "graphical_stdio"
        )]
        stdio: bool,
        /// Carry bounded channels for one native remote graphical client.
        #[arg(long, conflicts_with = "stdio", required_unless_present = "stdio")]
        graphical_stdio: bool,
    },
    /// Inspect strictly parsed remote connection profiles without connecting.
    Remote {
        #[command(subcommand)]
        command: RemoteCommand,
    },
    /// Validate local client configuration without contacting the daemon.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Inspect built-in or effective client keymaps.
    Keymap {
        #[command(subcommand)]
        command: KeymapCommand,
    },
    /// Validate, inspect, or reload the local persistent policy.
    Policy {
        #[command(subcommand)]
        command: PolicyCommand,
    },
    /// Inspect bounded in-memory daemon audit records.
    Audit {
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        after: Option<u64>,
        #[arg(long, default_value_t = 128, value_parser = clap::value_parser!(u16).range(1..=128))]
        max_records: u16,
    },
    /// Stream bounded machine events as NDJSON.
    Subscribe {
        #[command(subcommand)]
        stream: SubscribeCommand,
    },
    /// Inspect all reviewed topology metadata.
    Topology,
    /// Inspect one Splint by stable identity.
    Inspect {
        splint_id: SplintId,
    },
    /// Create a persistent Lair with one Dojo and live Splint.
    New {
        name: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Execute argv directly instead of starting the configured shell.
        #[arg(last = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Create a fresh graphical Lair, or explicitly attach by Splint ID.
    Launch {
        #[arg(long = "working-directory", alias = "dir")]
        cwd: Option<PathBuf>,
        /// Give the fresh Lair an explicit unique name.
        #[arg(long)]
        name: Option<String>,
        /// Attach an existing Splint by stable identity.
        #[arg(long)]
        splint_id: Option<SplintId>,
        /// Compatibility flag; fresh creation is already the default.
        #[arg(long, conflicts_with = "splint_id")]
        new: bool,
        /// Executable and arguments passed directly, never through a shell.
        #[arg(last = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Split a target leaf and launch a new sibling.
    Split {
        target_splint_id: SplintId,
        #[arg(long, value_enum)]
        axis: SplitAxis,
        #[arg(long, value_enum)]
        side: NewSplintSide,
        /// Thousandths assigned to the first child (1..=999).
        #[arg(long, default_value_t = 500, value_parser = clap::value_parser!(u16).range(1..=999))]
        ratio: u16,
        /// Fail if the target no longer has this exact live incarnation.
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        expected_incarnation: Option<u64>,
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Executable and arguments passed directly, never through a shell.
        #[arg(last = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Close an exited Splint leaf and collapse its parent branch.
    Close {
        splint_id: SplintId,
        /// Confirm destructive topology mutation for machine output.
        #[arg(long)]
        yes: bool,
    },
    /// Change the ratio of the selected Splint's parent branch.
    Ratio {
        target_splint_id: SplintId,
        #[arg(value_parser = clap::value_parser!(u16).range(1..=999))]
        ratio: u16,
    },
    /// Create a persistent Dojo with one live Splint.
    NewDojo {
        lair_id: LairId,
        #[arg(long, default_value = "terminal")]
        name: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(last = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Close a Dojo whose Splints have all exited.
    CloseDojo {
        dojo_id: DojoId,
        /// Confirm destructive topology mutation for machine output.
        #[arg(long)]
        yes: bool,
    },
    RenameLair {
        lair_id: LairId,
        name: String,
    },
    RenameDojo {
        dojo_id: DojoId,
        name: String,
    },
    /// Set a persisted convenience hint without changing any client's actual focus.
    DojoFocusHint {
        dojo_id: DojoId,
        splint_id: SplintId,
    },
    RenameSplint {
        splint_id: SplintId,
        title: String,
    },
    /// Relaunch an exited Splint under a new process incarnation.
    Relaunch {
        splint_id: SplintId,
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Executable and arguments passed directly, never through a shell.
        #[arg(last = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Restore one exited Splint using its saved launch metadata.
    Restore {
        splint_id: SplintId,
    },
    /// Restore every exited Splint in a saved Dojo.
    RestoreDojo {
        dojo_id: DojoId,
    },
    /// Restore every exited Splint in a saved Lair.
    RestoreLair {
        lair_id: LairId,
    },
    /// Show one live terminal snapshot (development mode only).
    Snapshot {
        splint_id: SplintId,
        /// Fail if the target no longer has this exact live incarnation.
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        expected_incarnation: Option<u64>,
    },
    /// Read one bounded page of terminal history.
    Scrollback {
        splint_id: SplintId,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long, default_value_t = 16, value_parser = clap::value_parser!(u16).range(1..=16))]
        max_rows: u16,
    },
    /// Search terminal history without echoing the query in machine output.
    Search {
        splint_id: SplintId,
        query: String,
        #[arg(long)]
        case_sensitive: bool,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long, default_value_t = 64, value_parser = clap::value_parser!(u16).range(1..=64))]
        max_results: u16,
    },
    /// Send literal UTF-8 text to one live shell (development mode only).
    Send {
        splint_id: SplintId,
        text: String,
        /// Fail if the target no longer has this exact live incarnation.
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        expected_incarnation: Option<u64>,
    },
    /// Resize one live terminal (development mode only).
    Resize {
        splint_id: SplintId,
        columns: u16,
        rows: u16,
    },
    /// Kill one live process while retaining its Splint leaf.
    Kill {
        splint_id: SplintId,
        /// Confirm process termination without prompting.
        #[arg(long)]
        yes: bool,
    },
    /// Private daemon-launched trusted consent surface.
    #[command(hide = true)]
    Consent,
}

#[derive(Debug, Subcommand)]
pub(in crate::app) enum RemoteCommand {
    /// List configured named remotes.
    List,
    /// Show one resolved profile and its credential-free SSH process plan.
    Inspect { profile: String },
    /// Negotiate SSH, relay, and daemon read-only reachability without mapping a Window.
    Check { profile: String },
}

#[derive(Clone, Copy, Debug, Subcommand)]
pub(in crate::app) enum ConfigCommand {
    /// Parse and resolve config.ini plus its selected keymap.
    Check,
}

#[derive(Debug, Subcommand)]
pub(in crate::app) enum KeymapCommand {
    /// List packaged keymap profiles.
    List,
    /// Show the effective keymap, or one named built-in profile.
    Show { profile: Option<String> },
    /// Validate that the effective keymap has no overlapping chords.
    Conflicts,
}

#[derive(Debug, Subcommand)]
pub(in crate::app) enum PolicyCommand {
    /// Validate a policy using the daemon's secure loader without publishing it.
    Validate { path: PathBuf },
    /// Print the normalized, validated policy document.
    Inspect { path: PathBuf },
    /// Ask the canonical systemd user service to reload its configured policy.
    Reload,
}

#[derive(Debug, Subcommand)]
pub(in crate::app) enum AuthorizationCommand {
    Status {
        splint_id: SplintId,
    },
    Revoke {
        #[arg(value_parser = clap::value_parser!(u64).range(1..))]
        grant_id: u64,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(in crate::app) enum SubscribeCommand {
    Terminal {
        splint_id: SplintId,
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        expected_incarnation: Option<u64>,
    },
    Topology,
    Control {
        splint_id: SplintId,
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        expected_incarnation: Option<u64>,
    },
}
