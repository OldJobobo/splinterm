use std::{
    collections::{HashMap, HashSet},
    env,
    fmt::Write as _,
    io::{self, IsTerminal, Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc as std_mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use splinterm::{
    AuthorityStatus, SessionPickerDecision, SessionPickerItem, SessionPickerUi, ThemeUpdate,
    TrustedConsentUi, WindowCommand, WindowOptions, WindowPaneOptions, WindowTopologyCommand,
    WindowTopologyUpdate, WindowUpdate,
    automation::{
        CliEnvelopeV2, CliErrorCodeV2, CliEventV2, Connection, ImageContentLeaseSet,
        MAX_RENDERER_IMAGE_RESIDENT_BYTES, MutationIdentityV2, PingEnvelopeV2, ReadResyncReasonV2,
        ResyncReasonV2, SharedImageContentCache, TerminalContinuationV2, TerminalReadProvenanceV2,
        audit_page_envelope, authorization_status_envelope, committed_mutation_envelope,
        created_mutation_envelope, decode_terminal_cursor, inspect_splint_envelope,
        inspect_topology_envelope, kill_envelope, list_lairs_envelope, process_started_envelope,
        protocol_error, public_error_code, read_resync_envelope, response_protocol_error,
        restore_many_envelope, revoke_envelope, scrollback_page_envelope, search_page_envelope,
        socket_path as configured_socket_path, terminal_action_envelope,
        terminal_snapshot_envelope, write_json_document,
    },
    config::{
        AppConfig, ConfigLoad, ResolvedTheme, ThemeSource, load_default, load_live_theme_source,
        load_theme_source,
    },
    renderer::{self, RendererOptions},
    run_window,
    session_picker::{RecentDojos, SessionEntry, collect_sessions},
};
use splinterm_core::{
    Axis, DojoId, LairId, LayoutNode, SplintId, SplintState, SplitRatio, SplitSide,
    TopologyRevision,
};
use splinterm_protocol::{
    AccessGrant, AccessScope, ActiveScreen, CellAttributes, ColorSource, ConsentPrompt,
    ConsentReply, ControlMode, ControlTransferOutcome, ErrorCode, HistoryTransition,
    LaunchParameters, MAX_CONSENT_FRAME_BYTES, Request, Response, ServerFrame, SubscriptionEvent,
    TerminalCell, TerminalInputModes, TerminalRow, TerminalSnapshot, TerminalUpdate,
    UnderlineStyle,
    perf_trace::{PerfTraceEvent, emit_perf_trace, perf_trace_enabled},
};
use tokio::sync::mpsc;

const WINDOW_UPDATE_QUEUE: usize = 4;
const WINDOW_COMMAND_QUEUE: usize = 64;
static NEXT_THEME_UPDATE_GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Parser)]
#[command(version, about = "Splinterm terminal client")]
struct Cli {
    /// Select human output or the supported JSON machine contract.
    #[arg(long, global = true, value_enum)]
    output: Option<OutputMode>,
    /// Select the public machine schema major.
    #[arg(long, global = true, value_parser = clap::value_parser!(u16).range(1..))]
    schema_major: Option<u16>,
    /// Bound a machine request deadline in milliseconds.
    #[arg(long, global = true, value_parser = clap::value_parser!(u64).range(1..=300_000))]
    timeout_ms: Option<u64>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OutputMode {
    Human,
    Json,
    Ndjson,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SplitAxis {
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
enum NewSplintSide {
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
enum Command {
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
        /// Use stdin and stdout as one full-duplex SSH transport.
        #[arg(long, required = true)]
        stdio: bool,
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
enum PolicyCommand {
    /// Validate a policy using the daemon's secure loader without publishing it.
    Validate { path: PathBuf },
    /// Print the normalized, validated policy document.
    Inspect { path: PathBuf },
    /// Ask the canonical systemd user service to reload its configured policy.
    Reload,
}

#[derive(Debug, Subcommand)]
enum AuthorizationCommand {
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
enum SubscribeCommand {
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

fn confirm_kill(splint_id: SplintId) -> Result<bool> {
    if !io::stdin().is_terminal() {
        bail!(
            "refusing to kill {splint_id} without an interactive terminal; pass --yes to confirm"
        );
    }
    eprint!("Kill Splint {splint_id} and its live process? [y/N] ");
    io::stderr()
        .flush()
        .context("failed to display confirmation")?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .context("failed to read confirmation")?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn confirm_reset() -> Result<bool> {
    if !io::stdin().is_terminal() {
        bail!(
            "refusing to reset every session without an interactive terminal; pass --yes to confirm"
        );
    }
    eprint!("Stop every Splint, clear all sessions, and restart splinterd? [y/N] ");
    io::stderr()
        .flush()
        .context("failed to display confirmation")?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .context("failed to read confirmation")?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

// The client needs concurrent local IPC and theme watching, not one allocator
// arena per CPU. Wayland rendering already runs on a bounded blocking worker.
fn usage_error(message: &str) -> ! {
    Cli::command()
        .error(clap::error::ErrorKind::ArgumentConflict, message)
        .exit()
}

fn run_relay_command(stdio: bool) -> Result<()> {
    if !stdio {
        bail!("relay requires --stdio");
    }
    let current = env::current_exe().context("cannot resolve the splinterm executable")?;
    let relay = current.with_file_name("splinterm-relay");
    let error = ProcessCommand::new(&relay).arg("--stdio").exec();
    Err(error).with_context(|| format!("failed to execute {}", relay.display()))
}

fn run_policy_command(command: PolicyCommand) -> Result<()> {
    match command {
        PolicyCommand::Validate { path } => {
            let (rule_count, _) = splinterd::inspect_policy_file(&path)
                .with_context(|| format!("policy validation failed for {}", path.display()))?;
            println!("valid splinterm.policy.v2 policy ({rule_count} rules)");
        }
        PolicyCommand::Inspect { path } => {
            let (_, document) = splinterd::inspect_policy_file(&path)
                .with_context(|| format!("policy inspection failed for {}", path.display()))?;
            serde_json::to_writer_pretty(io::stdout().lock(), &document)
                .context("failed to write validated policy")?;
            println!();
        }
        PolicyCommand::Reload => {
            let status = ProcessCommand::new("systemctl")
                .args(["--user", "reload", "splinterd.service"])
                .status()
                .context("failed to invoke systemctl --user reload splinterd.service")?;
            if !status.success() {
                bail!("systemctl --user reload splinterd.service failed with {status}");
            }
            println!(
                "policy reload requested; inspect daemon logs or bounded audit metadata for acceptance"
            );
        }
    }
    Ok(())
}

fn splinterm_state_directory() -> Result<PathBuf> {
    let base = match env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(env::var_os("HOME").context("XDG_STATE_HOME and HOME are unset")?)
            .join(".local/state"),
    };
    if !base.is_absolute() {
        bail!("state directory base must be absolute");
    }
    Ok(base.join("splinterm"))
}

fn move_session_state_no_replace(state: &Path, backup: &Path) -> Result<bool> {
    match rustix::fs::renameat_with(
        rustix::fs::CWD,
        state,
        rustix::fs::CWD,
        backup,
        rustix::fs::RenameFlags::NOREPLACE,
    ) {
        Ok(()) => Ok(true),
        Err(rustix::io::Errno::EXIST) => Ok(false),
        Err(error) => Err(error).with_context(|| {
            format!(
                "failed to move session state {} to {}",
                state.display(),
                backup.display()
            )
        }),
    }
}

fn backup_session_state(state: &Path) -> Result<Option<PathBuf>> {
    let metadata = match std::fs::symlink_metadata(state) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_dir() {
        bail!("session state path is not a directory: {}", state.display());
    }
    let parent = state.parent().context("session state path has no parent")?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_millis();
    for suffix in 0_u8..100 {
        let name = if suffix == 0 {
            format!("splinterm.reset-{stamp}")
        } else {
            format!("splinterm.reset-{stamp}-{suffix}")
        };
        let backup = parent.join(name);
        if move_session_state_no_replace(state, &backup)? {
            return Ok(Some(backup));
        }
    }
    bail!("could not allocate a unique session backup path");
}

fn run_user_systemctl(action: &str) -> Result<()> {
    let status = ProcessCommand::new("systemctl")
        .args(["--user", action, "splinterd.service"])
        .status()
        .with_context(|| format!("failed to invoke systemctl --user {action} splinterd.service"))?;
    if !status.success() {
        bail!("systemctl --user {action} splinterd.service failed with {status}");
    }
    Ok(())
}

fn wait_for_splinterd_socket(socket: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if std::fs::symlink_metadata(socket).is_ok_and(|metadata| metadata.file_type().is_socket())
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
    bail!(
        "splinterd restarted but did not create {} within 5 seconds; inspect `systemctl --user status splinterd.service`",
        socket.display()
    );
}

fn reset_readiness_context(backup: Option<&Path>) -> String {
    match backup {
        Some(path) => format!(
            "sessions were reset and backed up to {}, but splinterd did not become ready",
            path.display()
        ),
        None => "session state was empty, but splinterd did not become ready".to_owned(),
    }
}

fn run_reset_command(yes: bool) -> Result<()> {
    if !yes && !confirm_reset()? {
        println!("Reset cancelled.");
        return Ok(());
    }

    let state = splinterm_state_directory()?;
    let socket = configured_socket_path()?;
    run_user_systemctl("stop")?;

    let backup = match backup_session_state(&state) {
        Ok(backup) => backup,
        Err(error) => {
            if let Err(restart) = run_user_systemctl("start") {
                return Err(error).context(format!(
                    "session reset failed, then splinterd restart also failed: {restart:#}"
                ));
            }
            if let Err(readiness) = wait_for_splinterd_socket(&socket) {
                return Err(error).context(format!(
                    "session reset failed; splinterd restart also did not become ready: {readiness:#}"
                ));
            }
            return Err(error).context("session reset failed; splinterd was restarted unchanged");
        }
    };

    run_user_systemctl("start").with_context(|| match &backup {
        Some(path) => format!(
            "sessions were backed up to {}, but splinterd failed to restart",
            path.display()
        ),
        None => "no session state existed, but splinterd failed to restart".to_owned(),
    })?;
    wait_for_splinterd_socket(&socket)
        .with_context(|| reset_readiness_context(backup.as_deref()))?;

    println!("Splinterd restarted with no sessions.");
    match backup {
        Some(path) => println!("Previous sessions: {}", path.display()),
        None => println!("No previous session database was present."),
    }
    Ok(())
}

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<()> {
    let Cli {
        output,
        schema_major,
        timeout_ms,
        command,
    } = Cli::parse();
    if matches!(
        &command,
        Command::Sessions
            | Command::Reopen
            | Command::Window { .. }
            | Command::Launch { .. }
            | Command::Consent
            | Command::Policy { .. }
            | Command::Relay { .. }
            | Command::Reset { .. }
    ) && (output.is_some() || schema_major.is_some() || timeout_ms.is_some())
    {
        usage_error(
            "automation output, schema, and timeout options are unavailable for graphical, policy, relay, and local service commands",
        );
    }
    if matches!(command, Command::Subscribe { .. }) && output != Some(OutputMode::Ndjson) {
        usage_error("subscriptions require --output ndjson");
    }
    if output == Some(OutputMode::Json) {
        match run_machine_command(
            command,
            schema_major.unwrap_or(2),
            timeout_ms.unwrap_or(5_000),
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(error) => {
                eprintln!("{error:#}");
                std::process::exit(machine_exit_code(&error));
            }
        }
    }
    if output == Some(OutputMode::Ndjson) {
        let Command::Subscribe { stream } = command else {
            usage_error("NDJSON output is reserved for subscription commands");
        };
        match run_machine_subscription(
            stream,
            schema_major.unwrap_or(2),
            timeout_ms.unwrap_or(5_000),
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(error) => {
                eprintln!("{error:#}");
                std::process::exit(machine_exit_code(&error));
            }
        }
    }
    if schema_major.is_some() || timeout_ms.is_some() {
        usage_error("--schema-major and --timeout-ms require --output json or ndjson");
    }
    if let Command::Policy { command } = command {
        return run_policy_command(command);
    }
    if let Command::Relay { stdio } = command {
        return run_relay_command(stdio);
    }
    if let Command::Reset { yes } = command {
        return run_reset_command(yes);
    }

    let ConfigLoad {
        config,
        diagnostics,
    } = load_default()?;
    for diagnostic in diagnostics {
        eprintln!("splinterm config: {diagnostic}");
    }
    run_configured_command(command, config).await
}

async fn run_configured_command(command: Command, config: AppConfig) -> Result<()> {
    match command {
        Command::Sessions => run_sessions(config).await,
        Command::Reopen => reopen_recent(config).await,
        Command::Window { lair_id, dojo_id } => {
            let dojo = select_dojo(lair_id.zip(dojo_id)).await?;
            remember_dojo(dojo.id);
            run_live_multipane_window(config, dojo).await
        }
        Command::Launch {
            cwd,
            name,
            splint_id,
            new,
            command,
        } => {
            let cwd =
                cwd.unwrap_or(env::current_dir().context("failed to read current directory")?);
            launch(name, cwd, splint_id, new, command, config).await
        }
        Command::Consent => tokio::task::spawn_blocking(run_consent_client)
            .await
            .context("trusted consent task failed")?,
        command => run_headless(command, &config).await,
    }
}

fn machine_exit_code(error: &anyhow::Error) -> i32 {
    if let Some(protocol) = protocol_error(error) {
        return match protocol.code {
            ErrorCode::ConsentUnavailable | ErrorCode::ConsentDenied | ErrorCode::Unauthorized => 3,
            ErrorCode::AuthenticationFailed
            | ErrorCode::HandshakeRequired
            | ErrorCode::IncompatibleVersion => 4,
            ErrorCode::Cancelled => 6,
            ErrorCode::Internal | ErrorCode::DevelopmentFeatureDisabled => 70,
            _ => 5,
        };
    }
    let message = error.to_string();
    if message.contains("requires --yes") {
        3
    } else if message.contains("timed out") || message.contains("deadline") {
        6
    } else if message.contains("unsupported schema")
        || message.contains("cannot connect")
        || message.contains("XDG_RUNTIME_DIR")
        || message.contains("handshake")
        || message.contains("protocol version")
    {
        4
    } else if message.contains("not found")
        || message.contains("invalid continuation cursor")
        || message.contains("does not match the selected")
        || message.contains("expected incarnation")
        || message.contains("does not have a live process")
        || message.contains("controller")
        || message.contains("resource limit")
    {
        5
    } else {
        70
    }
}

async fn run_machine_command(command: Command, schema_major: u16, timeout_ms: u64) -> Result<()> {
    let command = match extract_machine_mutation(command) {
        Ok(mutation) => return run_machine_mutation(mutation, schema_major, timeout_ms).await,
        Err(command) => command,
    };
    match command {
        Command::Ping => run_machine_ping(schema_major, timeout_ms).await,
        Command::List { .. } => run_machine_read(MachineRead::List, schema_major, timeout_ms).await,
        Command::Topology => {
            run_machine_read(MachineRead::Topology, schema_major, timeout_ms).await
        }
        Command::Inspect { splint_id } => {
            run_machine_read(MachineRead::Splint(splint_id), schema_major, timeout_ms).await
        }
        Command::Snapshot {
            splint_id,
            expected_incarnation,
        } => run_machine_snapshot(splint_id, expected_incarnation, schema_major, timeout_ms).await,
        Command::Authorization {
            command: AuthorizationCommand::Status { splint_id },
        } => run_machine_authorization_status(splint_id, schema_major, timeout_ms).await,
        Command::Audit { after, max_records } => {
            run_machine_audit(after, usize::from(max_records), schema_major, timeout_ms).await
        }
        Command::Send {
            splint_id,
            text,
            expected_incarnation,
        } => {
            run_machine_control(
                MachineControl::Input(text.into_bytes()),
                splint_id,
                expected_incarnation,
                schema_major,
                timeout_ms,
            )
            .await
        }
        Command::Resize {
            splint_id,
            columns,
            rows,
        } => {
            run_machine_control(
                MachineControl::Resize { columns, rows },
                splint_id,
                None,
                schema_major,
                timeout_ms,
            )
            .await
        }
        Command::Scrollback {
            splint_id,
            cursor,
            max_rows,
        } => {
            run_machine_history(
                MachineHistory::Scrollback {
                    cursor,
                    max_rows: usize::from(max_rows),
                },
                splint_id,
                schema_major,
                timeout_ms,
            )
            .await
        }
        Command::Search {
            splint_id,
            query,
            case_sensitive,
            cursor,
            max_results,
        } => {
            run_machine_history(
                MachineHistory::Search {
                    query,
                    case_sensitive,
                    cursor,
                    max_results: usize::from(max_results),
                },
                splint_id,
                schema_major,
                timeout_ms,
            )
            .await
        }
        _ => bail!("JSON output is not implemented for this command yet"),
    }
}

#[derive(Debug, Clone, Copy)]
enum MachineRead {
    List,
    Topology,
    Splint(SplintId),
}

impl MachineRead {
    const fn operation(self) -> &'static str {
        match self {
            Self::List => "list_lairs",
            Self::Topology => "inspect_topology",
            Self::Splint(_) => "inspect_splint",
        }
    }
}

fn write_machine_read_failure(
    operation: &'static str,
    code: CliErrorCodeV2,
    message: impl Into<String>,
    retryable: bool,
) -> Result<()> {
    write_json_document(&CliEnvelopeV2::failure(
        operation, code, message, retryable,
    )?)
}

fn write_machine_connection_failure(operation: &'static str, error: &anyhow::Error) -> Result<()> {
    if let Some(protocol) = protocol_error(error) {
        return write_json_document(&CliEnvelopeV2::protocol_failure(
            operation,
            protocol,
            bounded_public_message(error),
        )?);
    }
    write_machine_read_failure(
        operation,
        CliErrorCodeV2::Internal,
        bounded_public_message(error),
        true,
    )
}

async fn run_machine_read(command: MachineRead, schema_major: u16, timeout_ms: u64) -> Result<()> {
    let operation = command.operation();
    if schema_major != 2 {
        write_machine_read_failure(
            operation,
            CliErrorCodeV2::UnsupportedSchema,
            format!("unsupported schema major {schema_major}"),
            false,
        )?;
        bail!("unsupported schema major {schema_major}");
    }

    let deadline = std::time::Duration::from_millis(timeout_ms);
    let started = std::time::Instant::now();
    let mut connection =
        match tokio::time::timeout(deadline, Connection::connect_automation()).await {
            Ok(Ok(connection)) => connection,
            Ok(Err(error)) => {
                write_machine_connection_failure(operation, &error)?;
                return Err(error);
            }
            Err(_) => {
                write_machine_read_failure(
                    operation,
                    CliErrorCodeV2::Timeout,
                    "connection deadline elapsed",
                    true,
                )?;
                bail!("splinterd connection timed out");
            }
        };
    let remaining = deadline.saturating_sub(started.elapsed());
    let response = match connection
        .request_with_deadline(Request::InspectTopology, remaining)
        .await
    {
        Ok(response) => response,
        Err(error) => {
            let (code, retryable) = if error.to_string().contains("timed out") {
                (CliErrorCodeV2::Timeout, true)
            } else if let Some(protocol) = protocol_error(&error) {
                public_error_code(protocol.code)
            } else {
                (CliErrorCodeV2::Internal, true)
            };
            write_machine_read_failure(operation, code, bounded_public_message(&error), retryable)?;
            return Err(error);
        }
    };
    let Response::Topology { snapshot } = response else {
        write_machine_read_failure(
            operation,
            CliErrorCodeV2::Internal,
            "splinterd returned an unexpected topology response",
            false,
        )?;
        bail!("splinterd returned an unexpected topology response");
    };
    if let MachineRead::Splint(splint_id) = command
        && snapshot.topology.find_splint(splint_id).is_none()
    {
        write_machine_read_failure(
            operation,
            CliErrorCodeV2::NotFound,
            "requested Splint was not found",
            false,
        )?;
        bail!("requested Splint was not found");
    }
    let envelope = match command {
        MachineRead::List => list_lairs_envelope(&snapshot),
        MachineRead::Topology => inspect_topology_envelope(&snapshot),
        MachineRead::Splint(splint_id) => inspect_splint_envelope(&snapshot, splint_id),
    };
    match envelope {
        Ok(envelope) => write_json_document(&envelope),
        Err(error) => {
            write_machine_read_failure(
                operation,
                CliErrorCodeV2::Internal,
                bounded_public_message(&error),
                false,
            )?;
            Err(error)
        }
    }
}

enum MachineMutation {
    Create {
        name: String,
        cwd: Option<PathBuf>,
        command: Vec<String>,
    },
    Split {
        target_splint_id: SplintId,
        axis: Axis,
        side: SplitSide,
        ratio: SplitRatio,
        expected_incarnation: Option<u64>,
        cwd: Option<PathBuf>,
        command: Vec<String>,
    },
    CloseSplint {
        splint_id: SplintId,
        yes: bool,
    },
    Ratio {
        splint_id: SplintId,
        ratio: SplitRatio,
    },
    NewDojo {
        lair_id: LairId,
        name: String,
        cwd: Option<PathBuf>,
        command: Vec<String>,
    },
    CloseDojo {
        dojo_id: DojoId,
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
    Focus {
        dojo_id: DojoId,
        splint_id: SplintId,
    },
    RenameSplint {
        splint_id: SplintId,
        title: String,
    },
    Relaunch {
        splint_id: SplintId,
        cwd: Option<PathBuf>,
        command: Vec<String>,
    },
    RestoreSplint {
        splint_id: SplintId,
    },
    RestoreDojo {
        dojo_id: DojoId,
    },
    RestoreLair {
        lair_id: LairId,
    },
    Kill {
        splint_id: SplintId,
        yes: bool,
    },
    Revoke {
        grant_id: u64,
        yes: bool,
    },
}

fn extract_machine_mutation(command: Command) -> std::result::Result<MachineMutation, Command> {
    Ok(match command {
        Command::New { name, cwd, command } => MachineMutation::Create { name, cwd, command },
        Command::Split {
            target_splint_id,
            axis,
            side,
            ratio,
            expected_incarnation,
            cwd,
            command,
        } => MachineMutation::Split {
            target_splint_id,
            axis: axis.into(),
            side: side.into(),
            ratio: SplitRatio::new(ratio).unwrap_or_else(|_| unreachable!("Clap bounded ratio")),
            expected_incarnation,
            cwd,
            command,
        },
        Command::Close { splint_id, yes } => MachineMutation::CloseSplint { splint_id, yes },
        Command::Ratio {
            target_splint_id,
            ratio,
        } => MachineMutation::Ratio {
            splint_id: target_splint_id,
            ratio: SplitRatio::new(ratio).unwrap_or_else(|_| unreachable!("Clap bounded ratio")),
        },
        Command::NewDojo {
            lair_id,
            name,
            cwd,
            command,
        } => MachineMutation::NewDojo {
            lair_id,
            name,
            cwd,
            command,
        },
        Command::CloseDojo { dojo_id, yes } => MachineMutation::CloseDojo { dojo_id, yes },
        Command::RenameLair { lair_id, name } => MachineMutation::RenameLair { lair_id, name },
        Command::RenameDojo { dojo_id, name } => MachineMutation::RenameDojo { dojo_id, name },
        Command::DojoFocusHint { dojo_id, splint_id } => {
            MachineMutation::Focus { dojo_id, splint_id }
        }
        Command::RenameSplint { splint_id, title } => {
            MachineMutation::RenameSplint { splint_id, title }
        }
        Command::Relaunch {
            splint_id,
            cwd,
            command,
        } => MachineMutation::Relaunch {
            splint_id,
            cwd,
            command,
        },
        Command::Restore { splint_id } => MachineMutation::RestoreSplint { splint_id },
        Command::RestoreDojo { dojo_id } => MachineMutation::RestoreDojo { dojo_id },
        Command::RestoreLair { lair_id } => MachineMutation::RestoreLair { lair_id },
        Command::Kill { splint_id, yes } => MachineMutation::Kill { splint_id, yes },
        Command::Authorization {
            command: AuthorizationCommand::Revoke { grant_id, yes },
        } => MachineMutation::Revoke { grant_id, yes },
        other => return Err(other),
    })
}

impl MachineMutation {
    const fn operation(&self) -> &'static str {
        match self {
            Self::Create { .. } => "create_lair",
            Self::Split { .. } => "split_splint",
            Self::CloseSplint { .. } => "close_splint",
            Self::Ratio { .. } => "set_split_ratio",
            Self::NewDojo { .. } => "new_dojo",
            Self::CloseDojo { .. } => "close_dojo",
            Self::RenameLair { .. } => "rename_lair",
            Self::RenameDojo { .. } => "rename_dojo",
            Self::Focus { .. } => "set_dojo_default_focus",
            Self::RenameSplint { .. } => "rename_splint",
            Self::Relaunch { .. } => "relaunch_splint",
            Self::RestoreSplint { .. } => "restore_splint",
            Self::RestoreDojo { .. } => "restore_dojo",
            Self::RestoreLair { .. } => "restore_lair",
            Self::Kill { .. } => "kill_splint",
            Self::Revoke { .. } => "revoke_access",
        }
    }

    const fn confirmation_missing(&self) -> bool {
        matches!(
            self,
            Self::CloseSplint { yes: false, .. }
                | Self::CloseDojo { yes: false, .. }
                | Self::Kill { yes: false, .. }
                | Self::Revoke { yes: false, .. }
        )
    }
}

fn machine_launch(cwd: Option<PathBuf>, command: Vec<String>) -> Result<LaunchParameters> {
    let config = load_default()?.config;
    Ok(launch_parameters(
        cwd.unwrap_or(env::current_dir().context("failed to read current directory")?),
        command,
        &config,
    ))
}

fn topology_splint_location(
    topology: &splinterm_protocol::TopologySnapshot,
    splint_id: SplintId,
) -> Result<(LairId, DojoId)> {
    topology
        .topology
        .lairs()
        .find_map(|lair| {
            lair.dojos
                .iter()
                .find(|dojo| dojo.root.find_splint(splint_id).is_some())
                .map(|dojo| (lair.id, dojo.id))
        })
        .context("requested Splint was not found")
}

fn require_incarnation(actual: u64, expected: Option<u64>) -> Result<()> {
    if expected.is_some_and(|expected| actual != expected) {
        bail!("selected Splint does not match expected incarnation");
    }
    Ok(())
}

fn require_expected_incarnation(
    topology: &splinterm_protocol::TopologySnapshot,
    splint_id: SplintId,
    expected: Option<u64>,
) -> Result<()> {
    let actual = topology
        .runtimes
        .iter()
        .find(|runtime| runtime.splint_id == splint_id)
        .and_then(|runtime| runtime.live_incarnation)
        .context("selected Splint does not have a live process")?;
    require_incarnation(actual, expected)
}

fn topology_dojo_location(
    topology: &splinterm_protocol::TopologySnapshot,
    dojo_id: DojoId,
) -> Result<LairId> {
    topology
        .topology
        .lairs()
        .find(|lair| lair.dojos.iter().any(|dojo| dojo.id == dojo_id))
        .map(|lair| lair.id)
        .context("requested Dojo was not found")
}

fn require_lair(topology: &splinterm_protocol::TopologySnapshot, lair_id: LairId) -> Result<()> {
    if topology.topology.lairs().any(|lair| lair.id == lair_id) {
        Ok(())
    } else {
        bail!("requested Lair was not found")
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "closed machine mutation request construction stays adjacent for auditability"
)]
fn machine_mutation_request(
    mutation: &MachineMutation,
    topology: &splinterm_protocol::TopologySnapshot,
) -> Result<Request> {
    let expected_topology_revision = topology.revision;
    Ok(match mutation {
        MachineMutation::Create { name, cwd, command } => Request::CreateLair {
            expected_topology_revision,
            name: name.clone(),
            launch: machine_launch(cwd.clone(), command.clone())?,
        },
        MachineMutation::Split {
            target_splint_id,
            axis,
            side,
            ratio,
            expected_incarnation,
            cwd,
            command,
        } => {
            topology_splint_location(topology, *target_splint_id)?;
            require_expected_incarnation(topology, *target_splint_id, *expected_incarnation)?;
            Request::SplitSplint {
                expected_topology_revision,
                target_splint_id: *target_splint_id,
                axis: *axis,
                side: *side,
                ratio: *ratio,
                launch: machine_launch(cwd.clone(), command.clone())?,
            }
        }
        MachineMutation::CloseSplint { splint_id, .. } => {
            topology_splint_location(topology, *splint_id)?;
            Request::CloseSplint {
                expected_topology_revision,
                splint_id: *splint_id,
            }
        }
        MachineMutation::Ratio { splint_id, ratio } => {
            topology_splint_location(topology, *splint_id)?;
            Request::SetSplitRatio {
                expected_topology_revision,
                target_splint_id: *splint_id,
                ratio: *ratio,
            }
        }
        MachineMutation::NewDojo {
            lair_id,
            name,
            cwd,
            command,
        } => {
            require_lair(topology, *lair_id)?;
            Request::NewDojo {
                expected_topology_revision,
                lair_id: *lair_id,
                name: name.clone(),
                launch: machine_launch(cwd.clone(), command.clone())?,
            }
        }
        MachineMutation::CloseDojo { dojo_id, .. } => {
            topology_dojo_location(topology, *dojo_id)?;
            Request::CloseDojo {
                expected_topology_revision,
                dojo_id: *dojo_id,
            }
        }
        MachineMutation::RenameLair { lair_id, name } => {
            require_lair(topology, *lair_id)?;
            Request::RenameLair {
                expected_topology_revision,
                lair_id: *lair_id,
                name: name.clone(),
            }
        }
        MachineMutation::RenameDojo { dojo_id, name } => {
            topology_dojo_location(topology, *dojo_id)?;
            Request::RenameDojo {
                expected_topology_revision,
                dojo_id: *dojo_id,
                name: name.clone(),
            }
        }
        MachineMutation::Focus { dojo_id, splint_id } => {
            let (_, actual_dojo) = topology_splint_location(topology, *splint_id)?;
            if actual_dojo != *dojo_id {
                bail!("selected Splint does not belong to the selected Dojo");
            }
            Request::SetDojoDefaultFocus {
                expected_topology_revision,
                dojo_id: *dojo_id,
                splint_id: *splint_id,
            }
        }
        MachineMutation::RenameSplint { splint_id, title } => {
            topology_splint_location(topology, *splint_id)?;
            Request::RenameSplint {
                expected_topology_revision,
                splint_id: *splint_id,
                title: title.clone(),
            }
        }
        MachineMutation::Relaunch {
            splint_id,
            cwd,
            command,
        } => {
            topology_splint_location(topology, *splint_id)?;
            Request::RelaunchSplint {
                expected_topology_revision,
                splint_id: *splint_id,
                launch: machine_launch(cwd.clone(), command.clone())?,
            }
        }
        MachineMutation::RestoreSplint { splint_id } => {
            topology_splint_location(topology, *splint_id)?;
            Request::RestoreSplint {
                expected_topology_revision,
                splint_id: *splint_id,
            }
        }
        MachineMutation::RestoreDojo { dojo_id } => {
            topology_dojo_location(topology, *dojo_id)?;
            Request::RestoreDojo {
                expected_topology_revision,
                dojo_id: *dojo_id,
            }
        }
        MachineMutation::RestoreLair { lair_id } => {
            require_lair(topology, *lair_id)?;
            Request::RestoreLair {
                expected_topology_revision,
                lair_id: *lair_id,
            }
        }
        MachineMutation::Kill { splint_id, .. } => {
            let (_, _, incarnation) = live_terminal_location(topology, *splint_id)?;
            Request::KillSplint {
                splint_id: *splint_id,
                incarnation,
            }
        }
        MachineMutation::Revoke { grant_id, .. } => Request::RevokeAccess {
            grant_id: *grant_id,
        },
    })
}

async fn connect_machine(
    operation: &'static str,
    deadline: std::time::Duration,
) -> Result<(Connection, std::time::Instant)> {
    let started = std::time::Instant::now();
    match tokio::time::timeout(deadline, Connection::connect_automation()).await {
        Ok(Ok(connection)) => Ok((connection, started)),
        Ok(Err(error)) => {
            write_machine_connection_failure(operation, &error)?;
            Err(error)
        }
        Err(_) => {
            write_machine_read_failure(
                operation,
                CliErrorCodeV2::Timeout,
                "connection deadline elapsed",
                true,
            )?;
            bail!("splinterd connection timed out")
        }
    }
}

fn finish_machine_envelope(operation: &'static str, result: Result<CliEnvelopeV2>) -> Result<()> {
    match result {
        Ok(envelope) => write_json_document(&envelope),
        Err(error) => {
            if let Some(protocol) = protocol_error(&error) {
                write_json_document(&CliEnvelopeV2::protocol_failure(
                    operation,
                    protocol,
                    bounded_public_message(&error),
                )?)?;
                return Err(error);
            }
            let (code, retryable) = if error.to_string().contains("timed out") {
                (CliErrorCodeV2::Timeout, true)
            } else if error.to_string().contains("not found") {
                (CliErrorCodeV2::NotFound, false)
            } else if error.to_string().contains("expected incarnation")
                || error.to_string().contains("does not have a live process")
            {
                (CliErrorCodeV2::StaleIncarnation, false)
            } else {
                (CliErrorCodeV2::Internal, false)
            };
            write_machine_read_failure(operation, code, bounded_public_message(&error), retryable)?;
            Err(error)
        }
    }
}

fn committed_revision(before: TopologyRevision, committed: TopologyRevision) -> Result<u64> {
    let expected = before
        .get()
        .checked_add(1)
        .context("topology revision exhausted")?;
    if committed.get() != expected {
        bail!("splinterd returned an inconsistent committed topology revision");
    }
    Ok(committed.get())
}

fn topology_identity(
    topology: &splinterm_protocol::TopologySnapshot,
    splint_id: SplintId,
    revision: u64,
    incarnation: Option<u64>,
) -> Result<MutationIdentityV2> {
    let (lair_id, dojo_id) = topology_splint_location(topology, splint_id)?;
    Ok(MutationIdentityV2 {
        lair_id: Some(lair_id),
        dojo_id: Some(dojo_id),
        splint_id: Some(splint_id),
        topology_revision: Some(revision),
        incarnation,
    })
}

fn created_lair_envelope(
    before: &splinterm_protocol::TopologySnapshot,
    lair: &splinterm_core::Lair,
    incarnation: u64,
    topology_revision: TopologyRevision,
) -> Result<CliEnvelopeV2> {
    let revision = committed_revision(before.revision, topology_revision)?;
    if lair.dojos.len() != 1 || incarnation == 0 {
        bail!("splinterd returned inconsistent created Lair topology");
    }
    let dojo = &lair.dojos[0];
    let LayoutNode::Leaf(splint) = &dojo.root else {
        bail!("created Dojo did not contain one Splint leaf");
    };
    if before
        .topology
        .lairs()
        .any(|existing| existing.id == lair.id)
        || before
            .topology
            .lairs()
            .flat_map(|existing| &existing.dojos)
            .any(|existing| existing.id == dojo.id)
        || before.topology.find_splint(splint.id).is_some()
    {
        bail!("create response reused an existing stable identity");
    }
    created_mutation_envelope(
        "create_lair",
        MutationIdentityV2 {
            lair_id: Some(lair.id),
            dojo_id: Some(dojo.id),
            splint_id: Some(splint.id),
            topology_revision: Some(revision),
            incarnation: Some(incarnation),
        },
    )
}

fn topology_commit_envelope(
    mutation: &MachineMutation,
    topology: &splinterm_protocol::TopologySnapshot,
    revision: TopologyRevision,
) -> Result<CliEnvelopeV2> {
    let revision = committed_revision(topology.revision, revision)?;
    let (identity, confirmed) = match mutation {
        MachineMutation::CloseSplint { splint_id, .. }
        | MachineMutation::Ratio { splint_id, .. }
        | MachineMutation::RenameSplint { splint_id, .. } => (
            topology_identity(topology, *splint_id, revision, None)?,
            matches!(mutation, MachineMutation::CloseSplint { .. }),
        ),
        MachineMutation::Focus { dojo_id, splint_id } => {
            let identity = topology_identity(topology, *splint_id, revision, None)?;
            if identity.dojo_id != Some(*dojo_id) {
                bail!("committed focus hint identity is inconsistent");
            }
            (identity, false)
        }
        MachineMutation::CloseDojo { dojo_id, .. }
        | MachineMutation::RenameDojo { dojo_id, .. } => (
            MutationIdentityV2 {
                lair_id: Some(topology_dojo_location(topology, *dojo_id)?),
                dojo_id: Some(*dojo_id),
                splint_id: None,
                topology_revision: Some(revision),
                incarnation: None,
            },
            matches!(mutation, MachineMutation::CloseDojo { .. }),
        ),
        MachineMutation::RenameLair { lair_id, .. } => (
            MutationIdentityV2 {
                lair_id: Some(*lair_id),
                dojo_id: None,
                splint_id: None,
                topology_revision: Some(revision),
                incarnation: None,
            },
            false,
        ),
        _ => bail!("topology commit response does not match mutation"),
    };
    committed_mutation_envelope(mutation.operation(), identity, confirmed)
}

fn layout_ids(node: &LayoutNode, ids: &mut Vec<SplintId>) {
    match node {
        LayoutNode::Leaf(splint) => ids.push(splint.id),
        LayoutNode::Branch { first, second, .. } => {
            layout_ids(first, ids);
            layout_ids(second, ids);
        }
    }
}

fn validate_restore_results(
    topology: &splinterm_protocol::TopologySnapshot,
    mutation: &MachineMutation,
    topology_revision: TopologyRevision,
    results: &[splinterm_protocol::RestoreLeafResult],
) -> Result<()> {
    if topology_revision < topology.revision {
        bail!("restore response regressed topology revision");
    }
    let mut expected = Vec::new();
    match mutation {
        MachineMutation::RestoreDojo { dojo_id } => {
            let dojo = topology
                .topology
                .lairs()
                .flat_map(|dojo| &dojo.dojos)
                .find(|dojo| dojo.id == *dojo_id)
                .context("restore Dojo disappeared from reviewed topology")?;
            layout_ids(&dojo.root, &mut expected);
        }
        MachineMutation::RestoreLair { lair_id } => {
            let dojo = topology
                .topology
                .lairs()
                .find(|dojo| dojo.id == *lair_id)
                .context("restore Dojo disappeared from reviewed topology")?;
            for dojo in &dojo.dojos {
                layout_ids(&dojo.root, &mut expected);
            }
        }
        _ => bail!("restore result validation used for non-aggregate mutation"),
    }
    let expected = expected.into_iter().collect::<HashSet<_>>();
    let actual = results
        .iter()
        .map(|result| result.splint_id)
        .collect::<HashSet<_>>();
    if actual.len() != results.len() || actual != expected {
        bail!("restore response does not exactly cover the selected Splints");
    }
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "closed mutation response identity correlations stay adjacent for auditability"
)]
fn mutation_response_envelope(
    mutation: &MachineMutation,
    topology: &splinterm_protocol::TopologySnapshot,
    response: Response,
) -> Result<CliEnvelopeV2> {
    match (mutation, response) {
        (
            MachineMutation::Split {
                target_splint_id, ..
            },
            Response::SplintStarted {
                splint_id,
                incarnation,
                topology_revision,
            },
        ) => {
            let revision = committed_revision(topology.revision, topology_revision)?;
            if topology.topology.find_splint(splint_id).is_some() {
                bail!("split response reused an existing Splint identity");
            }
            let (lair_id, dojo_id) = topology_splint_location(topology, *target_splint_id)?;
            created_mutation_envelope(
                "split_splint",
                MutationIdentityV2 {
                    lair_id: Some(lair_id),
                    dojo_id: Some(dojo_id),
                    splint_id: Some(splint_id),
                    topology_revision: Some(revision),
                    incarnation: Some(incarnation),
                },
            )
        }
        (
            MachineMutation::NewDojo { lair_id, .. },
            Response::DojoStarted {
                dojo_id,
                splint_id,
                incarnation,
                topology_revision,
            },
        ) => {
            if topology.topology.find_splint(splint_id).is_some()
                || topology
                    .topology
                    .lairs()
                    .flat_map(|dojo| &dojo.dojos)
                    .any(|dojo| dojo.id == dojo_id)
            {
                bail!("new-Dojo response reused an existing stable identity");
            }
            created_mutation_envelope(
                "new_dojo",
                MutationIdentityV2 {
                    lair_id: Some(*lair_id),
                    dojo_id: Some(dojo_id),
                    splint_id: Some(splint_id),
                    topology_revision: Some(committed_revision(
                        topology.revision,
                        topology_revision,
                    )?),
                    incarnation: Some(incarnation),
                },
            )
        }
        (
            MachineMutation::Relaunch { splint_id, .. },
            Response::SplintStarted {
                splint_id: response_id,
                incarnation,
                topology_revision,
            },
        ) if *splint_id == response_id => process_started_envelope(
            mutation.operation(),
            topology_identity(
                topology,
                *splint_id,
                committed_revision(topology.revision, topology_revision)?,
                Some(incarnation),
            )?,
        ),
        (
            MachineMutation::RestoreSplint { splint_id },
            Response::RestoreCompleted {
                topology_revision,
                mut results,
            },
        ) if results.len() == 1 && results[0].splint_id == *splint_id => {
            let result = results.pop().expect("one checked restore result");
            if let Some(error) = result.error {
                return Err(response_protocol_error(error));
            }
            let incarnation = result
                .incarnation
                .context("successful restore omitted process incarnation")?;
            process_started_envelope(
                "restore_splint",
                topology_identity(
                    topology,
                    *splint_id,
                    committed_revision(topology.revision, topology_revision)?,
                    Some(incarnation),
                )?,
            )
        }
        (
            MachineMutation::RestoreDojo { dojo_id },
            Response::RestoreCompleted {
                topology_revision,
                results,
            },
        ) => {
            validate_restore_results(topology, mutation, topology_revision, &results)?;
            restore_many_envelope(
                "restore_dojo",
                MutationIdentityV2 {
                    lair_id: Some(topology_dojo_location(topology, *dojo_id)?),
                    dojo_id: Some(*dojo_id),
                    splint_id: None,
                    topology_revision: Some(topology_revision.get()),
                    incarnation: None,
                },
                &results,
            )
        }
        (
            MachineMutation::RestoreLair { lair_id },
            Response::RestoreCompleted {
                topology_revision,
                results,
            },
        ) => {
            validate_restore_results(topology, mutation, topology_revision, &results)?;
            restore_many_envelope(
                "restore_lair",
                MutationIdentityV2 {
                    lair_id: Some(*lair_id),
                    dojo_id: None,
                    splint_id: None,
                    topology_revision: Some(topology_revision.get()),
                    incarnation: None,
                },
                &results,
            )
        }
        (
            MachineMutation::Kill { splint_id, .. },
            Response::SplintKilled {
                splint_id: response_id,
                incarnation,
                ..
            },
        ) if *splint_id == response_id => {
            let (lair_id, dojo_id, expected_incarnation) =
                live_terminal_location(topology, *splint_id)?;
            if incarnation != expected_incarnation {
                bail!("splinterd returned an inconsistent killed incarnation");
            }
            kill_envelope(lair_id, dojo_id, *splint_id, incarnation)
        }
        (mutation, Response::TopologyCommitted { topology_revision }) => {
            topology_commit_envelope(mutation, topology, topology_revision)
        }
        _ => bail!("splinterd returned a mutation response with inconsistent identity"),
    }
}

async fn machine_mutation_envelope(
    connection: &mut Connection,
    mutation: &MachineMutation,
    deadline: std::time::Duration,
    started: std::time::Instant,
) -> Result<CliEnvelopeV2> {
    if let MachineMutation::Revoke { grant_id, .. } = mutation {
        let response = connection
            .request_with_deadline(
                Request::RevokeAccess {
                    grant_id: *grant_id,
                },
                deadline.saturating_sub(started.elapsed()),
            )
            .await?;
        let Response::AccessRevoked {
            lair_id,
            dojo_id,
            grant,
            ..
        } = response
        else {
            bail!("splinterd returned an inconsistent revoke response");
        };
        if grant.grant_id != *grant_id {
            bail!("splinterd returned an inconsistent revoked grant");
        }
        return revoke_envelope(lair_id, dojo_id, &grant);
    }

    let response = connection
        .request_with_deadline(
            Request::InspectTopology,
            deadline.saturating_sub(started.elapsed()),
        )
        .await?;
    let Response::Topology { snapshot: topology } = response else {
        bail!("splinterd returned an unexpected topology response");
    };
    topology
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    let request = machine_mutation_request(mutation, &topology)?;
    let response = connection
        .request_with_deadline(request, deadline.saturating_sub(started.elapsed()))
        .await?;
    if matches!(mutation, MachineMutation::Create { .. }) {
        let Response::LairCreated {
            lair: dojo,
            incarnation,
            topology_revision,
        } = response
        else {
            bail!("splinterd returned an inconsistent create response");
        };
        return created_lair_envelope(&topology, &dojo, incarnation, topology_revision);
    }
    mutation_response_envelope(mutation, &topology, response)
}

async fn run_machine_mutation(
    mutation: MachineMutation,
    schema_major: u16,
    timeout_ms: u64,
) -> Result<()> {
    let operation = mutation.operation();
    if schema_major != 2 {
        write_machine_read_failure(
            operation,
            CliErrorCodeV2::UnsupportedSchema,
            format!("unsupported schema major {schema_major}"),
            false,
        )?;
        bail!("unsupported schema major {schema_major}");
    }
    if mutation.confirmation_missing() {
        write_machine_read_failure(
            operation,
            CliErrorCodeV2::ConfirmationRequired,
            "destructive machine command requires --yes",
            false,
        )?;
        bail!("destructive machine command requires --yes");
    }
    let deadline = std::time::Duration::from_millis(timeout_ms);
    let (mut connection, started) = connect_machine(operation, deadline).await?;
    let result = machine_mutation_envelope(&mut connection, &mutation, deadline, started).await;
    finish_machine_envelope(operation, result)
}

async fn run_machine_authorization_status(
    splint_id: SplintId,
    schema_major: u16,
    timeout_ms: u64,
) -> Result<()> {
    const OPERATION: &str = "authorization_status";
    if schema_major != 2 {
        write_machine_read_failure(
            OPERATION,
            CliErrorCodeV2::UnsupportedSchema,
            format!("unsupported schema major {schema_major}"),
            false,
        )?;
        bail!("unsupported schema major {schema_major}");
    }
    let deadline = std::time::Duration::from_millis(timeout_ms);
    let (mut connection, started) = connect_machine(OPERATION, deadline).await?;
    let result = async {
        let response = connection
            .request_with_deadline(
                Request::AuthorizationStatus {
                    splint_id,
                    incarnation: None,
                },
                deadline.saturating_sub(started.elapsed()),
            )
            .await?;
        let Response::AuthorizationStatus {
            lair_id,
            dojo_id,
            incarnation,
            grants,
            persistent,
            development_bypass,
            ..
        } = response
        else {
            bail!("splinterd returned an unexpected authorization response");
        };
        authorization_status_envelope(
            lair_id,
            dojo_id,
            splint_id,
            incarnation,
            &grants,
            &persistent,
            development_bypass,
        )
    }
    .await;
    finish_machine_envelope(OPERATION, result)
}

async fn run_machine_audit(
    after_audit_id: Option<u64>,
    max_records: usize,
    schema_major: u16,
    timeout_ms: u64,
) -> Result<()> {
    const OPERATION: &str = "audit_inspect";
    if schema_major != 2 {
        write_machine_read_failure(
            OPERATION,
            CliErrorCodeV2::UnsupportedSchema,
            format!("unsupported schema major {schema_major}"),
            false,
        )?;
        bail!("unsupported schema major {schema_major}");
    }
    let deadline = std::time::Duration::from_millis(timeout_ms);
    let (mut connection, started) = connect_machine(OPERATION, deadline).await?;
    let result = async {
        let response = connection
            .request_with_deadline(
                Request::AuditInspect {
                    after_audit_id,
                    max_records,
                },
                deadline.saturating_sub(started.elapsed()),
            )
            .await?;
        let Response::AuditPage { page } = response else {
            bail!("splinterd returned an unexpected audit response");
        };
        audit_page_envelope(&page)
    }
    .await;
    finish_machine_envelope(OPERATION, result)
}

enum MachineControl {
    Input(Vec<u8>),
    Resize { columns: u16, rows: u16 },
}

impl MachineControl {
    const fn operation(&self) -> &'static str {
        match self {
            Self::Input(_) => "input",
            Self::Resize { .. } => "resize",
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "atomic acquire/action/release and cleanup remain adjacent for auditability"
)]
async fn machine_control_envelope(
    connection: &mut Connection,
    command: &MachineControl,
    splint_id: SplintId,
    expected_incarnation: Option<u64>,
    deadline: std::time::Duration,
    started: std::time::Instant,
) -> Result<CliEnvelopeV2> {
    if matches!(command, MachineControl::Input(bytes) if bytes.len() > connection.limits().maximum_input_bytes)
    {
        bail!("input exceeds negotiated resource limit");
    }
    let response = connection
        .request_with_deadline(
            Request::InspectTopology,
            deadline.saturating_sub(started.elapsed()),
        )
        .await?;
    let Response::Topology { snapshot: topology } = response else {
        bail!("splinterd returned an unexpected topology response");
    };
    require_expected_incarnation(&topology, splint_id, expected_incarnation)?;
    let (lair_id, dojo_id, incarnation) = live_terminal_location(&topology, splint_id)?;
    let response = connection
        .request_with_deadline(
            Request::AcquireControl {
                splint_id,
                incarnation,
                modes: vec![match command {
                    MachineControl::Input(_) => ControlMode::Input,
                    MachineControl::Resize { .. } => ControlMode::Resize,
                }],
            },
            deadline.saturating_sub(started.elapsed()),
        )
        .await?;
    let Response::ControlGranted { controller_id, .. } = response else {
        bail!("splinterd did not grant a controller lease");
    };
    if controller_id == 0 {
        bail!("splinterd returned an invalid controller lease");
    }
    let request = match command {
        MachineControl::Input(bytes) => Request::Input {
            controller_id,
            splint_id,
            incarnation,
            bytes: bytes.clone(),
        },
        MachineControl::Resize { columns, rows } => Request::Resize {
            controller_id,
            splint_id,
            incarnation,
            columns: *columns,
            rows: *rows,
            pixel_width: 0,
            pixel_height: 0,
        },
    };
    let action = connection
        .request_with_deadline(request, deadline.saturating_sub(started.elapsed()))
        .await;
    let release = connection
        .request_with_deadline(
            Request::ReleaseControl { controller_id },
            deadline.saturating_sub(started.elapsed()),
        )
        .await;
    let response = match action {
        Ok(response) => response,
        Err(error) => {
            let _ = release;
            return Err(error);
        }
    };
    if !matches!(release?, Response::Acknowledged) {
        bail!("splinterd did not release the controller lease");
    }
    let Response::TerminalActionAcknowledged {
        lair_id: response_lair,
        dojo_id: response_dojo,
        splint_id: response_splint,
        incarnation: response_incarnation,
        terminal_revision,
        history_generation,
    } = response
    else {
        bail!("splinterd returned an unexpected terminal action response");
    };
    if (
        response_lair,
        response_dojo,
        response_splint,
        response_incarnation,
    ) != (lair_id, dojo_id, splint_id, incarnation)
    {
        bail!("splinterd returned inconsistent terminal action identity");
    }
    terminal_action_envelope(
        command.operation(),
        TerminalReadProvenanceV2 {
            lair_id,
            dojo_id,
            splint_id,
            incarnation,
            terminal_revision,
            history_generation,
        },
        match command {
            MachineControl::Input(_) => None,
            MachineControl::Resize { columns, rows } => Some((*columns, *rows)),
        },
    )
}

async fn run_machine_control(
    command: MachineControl,
    splint_id: SplintId,
    expected_incarnation: Option<u64>,
    schema_major: u16,
    timeout_ms: u64,
) -> Result<()> {
    let operation = command.operation();
    if schema_major != 2 {
        write_machine_read_failure(
            operation,
            CliErrorCodeV2::UnsupportedSchema,
            format!("unsupported schema major {schema_major}"),
            false,
        )?;
        bail!("unsupported schema major {schema_major}");
    }
    let deadline = std::time::Duration::from_millis(timeout_ms);
    let (mut connection, started) = connect_machine(operation, deadline).await?;
    let result = machine_control_envelope(
        &mut connection,
        &command,
        splint_id,
        expected_incarnation,
        deadline,
        started,
    )
    .await;
    finish_machine_envelope(operation, result)
}

enum MachineHistory {
    Scrollback {
        cursor: Option<String>,
        max_rows: usize,
    },
    Search {
        query: String,
        case_sensitive: bool,
        cursor: Option<String>,
        max_results: usize,
    },
}

impl MachineHistory {
    const fn operation(&self) -> &'static str {
        match self {
            Self::Scrollback { .. } => "scrollback_page",
            Self::Search { .. } => "search_scrollback",
        }
    }

    fn cursor(&self) -> Option<&str> {
        match self {
            Self::Scrollback { cursor, .. } | Self::Search { cursor, .. } => cursor.as_deref(),
        }
    }
}

struct MachineHistoryContext {
    provenance: TerminalReadProvenanceV2,
    before_row_id: Option<u64>,
    daemon_cursor: Option<String>,
}

fn history_cursor_context(
    command: &MachineHistory,
    encoded: &str,
    lair_id: LairId,
    dojo_id: DojoId,
    splint_id: SplintId,
    incarnation: u64,
) -> Result<MachineHistoryContext> {
    let cursor = decode_terminal_cursor(encoded).context("invalid continuation cursor")?;
    let (cursor_splint, cursor_incarnation, revision, generation, before, daemon) = match cursor {
        TerminalContinuationV2::Scrollback {
            splint_id,
            incarnation,
            terminal_revision,
            history_generation,
            before_row_id,
        } if matches!(command, MachineHistory::Scrollback { .. }) => (
            splint_id,
            incarnation,
            terminal_revision,
            history_generation,
            Some(before_row_id),
            None,
        ),
        TerminalContinuationV2::Search {
            splint_id,
            incarnation,
            terminal_revision,
            history_generation,
            daemon_cursor,
        } if matches!(command, MachineHistory::Search { .. }) => (
            splint_id,
            incarnation,
            terminal_revision,
            history_generation,
            None,
            Some(daemon_cursor),
        ),
        _ => bail!("continuation cursor does not match the requested operation"),
    };
    if cursor_splint != splint_id || cursor_incarnation != incarnation {
        bail!("continuation cursor does not match the selected live Splint");
    }
    Ok(MachineHistoryContext {
        provenance: TerminalReadProvenanceV2 {
            lair_id,
            dojo_id,
            splint_id,
            incarnation,
            terminal_revision: revision,
            history_generation: generation,
        },
        before_row_id: before,
        daemon_cursor: daemon,
    })
}

fn live_terminal_location(
    topology: &splinterm_protocol::TopologySnapshot,
    splint_id: SplintId,
) -> Result<(LairId, DojoId, u64)> {
    topology
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    let (lair_id, dojo_id) = topology
        .topology
        .lairs()
        .find_map(|lair| {
            lair.dojos
                .iter()
                .find(|dojo| dojo.root.find_splint(splint_id).is_some())
                .map(|dojo| (lair.id, dojo.id))
        })
        .context("requested Splint was not found")?;
    let incarnation = topology
        .runtimes
        .iter()
        .find(|runtime| runtime.splint_id == splint_id)
        .context("validated topology omitted Splint runtime")?
        .live_incarnation
        .context("selected Splint does not have a live process")?;
    Ok((lair_id, dojo_id, incarnation))
}

async fn machine_history_context(
    connection: &mut Connection,
    command: &MachineHistory,
    splint_id: SplintId,
    deadline: std::time::Duration,
    started: std::time::Instant,
) -> Result<MachineHistoryContext> {
    let response = connection
        .request_with_deadline(
            Request::InspectTopology,
            deadline.saturating_sub(started.elapsed()),
        )
        .await?;
    let Response::Topology { snapshot: topology } = response else {
        bail!("splinterd returned an unexpected topology response");
    };
    let (lair_id, dojo_id, incarnation) = live_terminal_location(&topology, splint_id)?;
    if let Some(encoded) = command.cursor() {
        return history_cursor_context(command, encoded, lair_id, dojo_id, splint_id, incarnation);
    }
    let response = connection
        .request_with_deadline(
            Request::Attach {
                splint_id,
                incarnation: Some(incarnation),
                scrollback_rows: 0,
            },
            deadline.saturating_sub(started.elapsed()),
        )
        .await?;
    let Response::Attached {
        subscription_id,
        snapshot,
        ..
    } = response
    else {
        bail!("splinterd returned an unexpected attach response");
    };
    snapshot
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    if subscription_id == 0
        || snapshot.splint_id != splint_id
        || snapshot.incarnation != incarnation
    {
        bail!("splinterd returned inconsistent terminal identity");
    }
    let before_row_id = snapshot
        .newest_available_scrollback_row_id
        .unwrap_or(1)
        .checked_add(1)
        .context("scrollback row identity exhausted")?;
    let detached = connection
        .request_with_deadline(
            Request::Detach { subscription_id },
            deadline.saturating_sub(started.elapsed()),
        )
        .await?;
    if !matches!(detached, Response::Acknowledged) {
        bail!("splinterd did not detach the history bootstrap subscription");
    }
    Ok(MachineHistoryContext {
        provenance: TerminalReadProvenanceV2 {
            lair_id,
            dojo_id,
            splint_id,
            incarnation,
            terminal_revision: snapshot.revision,
            history_generation: snapshot.history_generation,
        },
        before_row_id: Some(before_row_id),
        daemon_cursor: None,
    })
}

#[allow(
    clippy::too_many_lines,
    reason = "the closed scrollback/search response matrix remains contiguous for protocol review"
)]
async fn machine_history_envelope(
    connection: &mut Connection,
    command: &MachineHistory,
    splint_id: SplintId,
    deadline: std::time::Duration,
    started: std::time::Instant,
) -> Result<CliEnvelopeV2> {
    let context =
        machine_history_context(connection, command, splint_id, deadline, started).await?;
    let request = match command {
        MachineHistory::Scrollback { max_rows, .. } => Request::ScrollbackPage {
            splint_id,
            incarnation: context.provenance.incarnation,
            terminal_revision: context.provenance.terminal_revision,
            history_generation: context.provenance.history_generation,
            before_row_id: context
                .before_row_id
                .context("scrollback cursor omitted row identity")?,
            max_rows: *max_rows,
        },
        MachineHistory::Search {
            query,
            case_sensitive,
            max_results,
            ..
        } => Request::SearchScrollback {
            splint_id,
            incarnation: context.provenance.incarnation,
            terminal_revision: context.provenance.terminal_revision,
            history_generation: context.provenance.history_generation,
            query: query.clone(),
            case_sensitive: *case_sensitive,
            cursor: context.daemon_cursor.clone(),
            max_results: *max_results,
        },
    };
    let response = connection
        .request_with_deadline(request, deadline.saturating_sub(started.elapsed()))
        .await?;
    match response {
        Response::ScrollbackPage { page, .. }
            if matches!(command, MachineHistory::Scrollback { .. }) =>
        {
            if page.splint_id != splint_id
                || page.incarnation != context.provenance.incarnation
                || page.terminal_revision != context.provenance.terminal_revision
                || page.history_generation != context.provenance.history_generation
            {
                bail!("splinterd returned inconsistent scrollback provenance");
            }
            scrollback_page_envelope(
                context.provenance.lair_id,
                context.provenance.dojo_id,
                &page,
            )
        }
        Response::SearchResults { page, .. }
            if matches!(command, MachineHistory::Search { .. }) =>
        {
            if page.splint_id != splint_id
                || page.incarnation != context.provenance.incarnation
                || page.terminal_revision != context.provenance.terminal_revision
                || page.history_generation != context.provenance.history_generation
            {
                bail!("splinterd returned inconsistent search provenance");
            }
            search_page_envelope(
                context.provenance.lair_id,
                context.provenance.dojo_id,
                &page,
            )
        }
        Response::ScrollbackResyncRequired {
            current_revision,
            history_generation,
            ..
        } if matches!(command, MachineHistory::Scrollback { .. }) => read_resync_envelope(
            command.operation(),
            TerminalReadProvenanceV2 {
                terminal_revision: current_revision,
                history_generation,
                ..context.provenance
            },
            if history_generation == context.provenance.history_generation {
                ReadResyncReasonV2::StaleRevision
            } else {
                ReadResyncReasonV2::HistoryReplaced
            },
        ),
        Response::SearchResyncRequired {
            current_revision,
            history_generation,
            ..
        } if matches!(command, MachineHistory::Search { .. }) => read_resync_envelope(
            command.operation(),
            TerminalReadProvenanceV2 {
                terminal_revision: current_revision,
                history_generation,
                ..context.provenance
            },
            if history_generation == context.provenance.history_generation {
                ReadResyncReasonV2::StaleRevision
            } else {
                ReadResyncReasonV2::HistoryReplaced
            },
        ),
        _ => bail!("splinterd returned an unexpected history response"),
    }
}

async fn run_machine_history(
    command: MachineHistory,
    splint_id: SplintId,
    schema_major: u16,
    timeout_ms: u64,
) -> Result<()> {
    let operation = command.operation();
    if schema_major != 2 {
        write_machine_read_failure(
            operation,
            CliErrorCodeV2::UnsupportedSchema,
            format!("unsupported schema major {schema_major}"),
            false,
        )?;
        bail!("unsupported schema major {schema_major}");
    }
    let deadline = std::time::Duration::from_millis(timeout_ms);
    let started = std::time::Instant::now();
    let mut connection =
        match tokio::time::timeout(deadline, Connection::connect_automation()).await {
            Ok(Ok(connection)) => connection,
            Ok(Err(error)) => {
                write_machine_connection_failure(operation, &error)?;
                return Err(error);
            }
            Err(_) => {
                write_machine_read_failure(
                    operation,
                    CliErrorCodeV2::Timeout,
                    "connection deadline elapsed",
                    true,
                )?;
                bail!("splinterd connection timed out");
            }
        };
    match machine_history_envelope(&mut connection, &command, splint_id, deadline, started).await {
        Ok(envelope) => write_json_document(&envelope),
        Err(error) => {
            let (code, retryable) = if error.to_string().contains("timed out") {
                (CliErrorCodeV2::Timeout, true)
            } else if let Some(protocol) = protocol_error(&error) {
                public_error_code(protocol.code)
            } else if error.to_string().contains("continuation cursor") {
                (CliErrorCodeV2::InvalidArgument, false)
            } else if error.to_string().contains("not found") {
                (CliErrorCodeV2::NotFound, false)
            } else {
                (CliErrorCodeV2::Internal, false)
            };
            write_machine_read_failure(operation, code, bounded_public_message(&error), retryable)?;
            Err(error)
        }
    }
}

async fn machine_snapshot_envelope(
    connection: &mut Connection,
    splint_id: SplintId,
    expected_incarnation: Option<u64>,
    deadline: std::time::Duration,
    started: std::time::Instant,
) -> Result<CliEnvelopeV2> {
    let topology = connection
        .request_with_deadline(
            Request::InspectTopology,
            deadline.saturating_sub(started.elapsed()),
        )
        .await?;
    let Response::Topology { snapshot: topology } = topology else {
        bail!("splinterd returned an unexpected topology response");
    };
    topology
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    require_expected_incarnation(&topology, splint_id, expected_incarnation)?;
    let mut identity = None;
    for lair in topology.topology.lairs() {
        for dojo in &lair.dojos {
            if dojo.root.find_splint(splint_id).is_some() {
                let runtime = topology
                    .runtimes
                    .iter()
                    .find(|runtime| runtime.splint_id == splint_id)
                    .context("validated topology omitted Splint runtime")?;
                identity = Some((
                    lair.id,
                    dojo.id,
                    runtime
                        .live_incarnation
                        .context("selected Splint does not have a live process")?,
                ));
            }
        }
    }
    let (lair_id, dojo_id, incarnation) = identity.context("requested Splint was not found")?;
    let attached = connection
        .request_with_deadline(
            Request::Attach {
                splint_id,
                incarnation: Some(incarnation),
                scrollback_rows: 0,
            },
            deadline.saturating_sub(started.elapsed()),
        )
        .await?;
    let Response::Attached {
        subscription_id,
        snapshot,
        ..
    } = attached
    else {
        bail!("splinterd returned an unexpected attach response");
    };
    if subscription_id == 0
        || snapshot.splint_id != splint_id
        || snapshot.incarnation != incarnation
    {
        bail!("splinterd returned inconsistent terminal identity");
    }
    let detached = connection
        .request_with_deadline(
            Request::Detach { subscription_id },
            deadline.saturating_sub(started.elapsed()),
        )
        .await?;
    if !matches!(detached, Response::Acknowledged) {
        bail!("splinterd did not detach the one-shot terminal subscription");
    }
    terminal_snapshot_envelope(lair_id, dojo_id, &snapshot)
}

async fn run_machine_snapshot(
    splint_id: SplintId,
    expected_incarnation: Option<u64>,
    schema_major: u16,
    timeout_ms: u64,
) -> Result<()> {
    const OPERATION: &str = "terminal_snapshot";
    if schema_major != 2 {
        write_machine_read_failure(
            OPERATION,
            CliErrorCodeV2::UnsupportedSchema,
            format!("unsupported schema major {schema_major}"),
            false,
        )?;
        bail!("unsupported schema major {schema_major}");
    }
    let deadline = std::time::Duration::from_millis(timeout_ms);
    let started = std::time::Instant::now();
    let mut connection =
        match tokio::time::timeout(deadline, Connection::connect_automation()).await {
            Ok(Ok(connection)) => connection,
            Ok(Err(error)) => {
                write_machine_connection_failure(OPERATION, &error)?;
                return Err(error);
            }
            Err(_) => {
                write_machine_read_failure(
                    OPERATION,
                    CliErrorCodeV2::Timeout,
                    "connection deadline elapsed",
                    true,
                )?;
                bail!("splinterd connection timed out");
            }
        };
    let result = machine_snapshot_envelope(
        &mut connection,
        splint_id,
        expected_incarnation,
        deadline,
        started,
    )
    .await;
    match result {
        Ok(envelope) => write_json_document(&envelope),
        Err(error) => {
            let (code, retryable) = if error.to_string().contains("timed out") {
                (CliErrorCodeV2::Timeout, true)
            } else if let Some(protocol) = protocol_error(&error) {
                public_error_code(protocol.code)
            } else if error.to_string().contains("not found") {
                (CliErrorCodeV2::NotFound, false)
            } else if error.to_string().contains("does not have a live process")
                || error.to_string().contains("expected incarnation")
            {
                (CliErrorCodeV2::StaleIncarnation, false)
            } else {
                (CliErrorCodeV2::Internal, false)
            };
            write_machine_read_failure(OPERATION, code, bounded_public_message(&error), retryable)?;
            Err(error)
        }
    }
}

async fn run_machine_ping(schema_major: u16, timeout_ms: u64) -> Result<()> {
    if schema_major != 2 {
        let envelope = PingEnvelopeV2::failure(
            1,
            CliErrorCodeV2::UnsupportedSchema,
            format!("unsupported schema major {schema_major}"),
            false,
        )?;
        write_json_document(&envelope)?;
        bail!("unsupported schema major {schema_major}");
    }

    let deadline = std::time::Duration::from_millis(timeout_ms);
    let started = std::time::Instant::now();
    let mut connection =
        match tokio::time::timeout(deadline, Connection::connect_automation()).await {
            Ok(Ok(connection)) => connection,
            Ok(Err(error)) => {
                let (code, retryable) = protocol_error(&error)
                    .map_or((CliErrorCodeV2::Internal, true), |protocol| {
                        public_error_code(protocol.code)
                    });
                write_json_document(&PingEnvelopeV2::failure(
                    1,
                    code,
                    bounded_public_message(&error),
                    retryable,
                )?)?;
                return Err(error);
            }
            Err(_) => {
                write_json_document(&PingEnvelopeV2::failure(
                    1,
                    CliErrorCodeV2::Timeout,
                    "connection deadline elapsed",
                    true,
                )?)?;
                bail!("splinterd connection timed out");
            }
        };
    let remaining = deadline.saturating_sub(started.elapsed());
    match connection
        .request_with_deadline(Request::Ping, remaining)
        .await
    {
        Ok(Response::Pong) => write_json_document(&PingEnvelopeV2::success(1)?),
        Ok(_) => {
            write_json_document(&PingEnvelopeV2::failure(
                1,
                CliErrorCodeV2::Internal,
                "splinterd returned an unexpected ping response",
                false,
            )?)?;
            bail!("splinterd returned an unexpected ping response")
        }
        Err(error) => {
            let timed_out = error.to_string().contains("timed out");
            let code = if timed_out {
                CliErrorCodeV2::Timeout
            } else {
                CliErrorCodeV2::Internal
            };
            write_json_document(&PingEnvelopeV2::failure(
                1,
                code,
                bounded_public_message(&error),
                true,
            )?)?;
            Err(error)
        }
    }
}

fn bounded_public_message(error: &anyhow::Error) -> String {
    let message = error.to_string();
    if message.chars().count() <= 1024 {
        return message;
    }
    message
        .chars()
        .take(1023)
        .chain(std::iter::once('…'))
        .collect()
}

async fn next_private_event(connection: &mut Connection) -> Result<(u64, u64, SubscriptionEvent)> {
    match connection.next_server_frame().await? {
        ServerFrame::Event {
            subscription_id,
            sequence,
            event,
        } => Ok((subscription_id, sequence, event)),
        ServerFrame::Error { error, .. } => Err(response_protocol_error(error)),
        _ => bail!("splinterd sent an unexpected subscription frame"),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "terminal sequence, revision, history, and termination state stay adjacent"
)]
async fn run_terminal_subscription(
    connection: &mut Connection,
    splint_id: SplintId,
    expected_incarnation: Option<u64>,
    setup_deadline: std::time::Duration,
) -> Result<()> {
    let runtime = connection
        .request_with_deadline(Request::InspectSplint { splint_id }, setup_deadline)
        .await?;
    let Response::Splint { runtime, .. } = runtime else {
        bail!("splinterd did not return the selected Splint identity");
    };
    let incarnation = runtime
        .live_incarnation
        .context("selected Splint does not have a live process")?;
    require_incarnation(incarnation, expected_incarnation)?;
    let response = connection
        .request_with_deadline(
            Request::Attach {
                splint_id,
                incarnation: Some(incarnation),
                scrollback_rows: 0,
            },
            setup_deadline,
        )
        .await?;
    let Response::Attached {
        subscription_id,
        snapshot,
        ..
    } = response
    else {
        bail!("splinterd returned an unexpected attach response");
    };
    if snapshot.splint_id != splint_id || snapshot.incarnation != incarnation {
        bail!("splinterd returned inconsistent terminal subscription identity");
    }
    write_json_document(&CliEventV2::terminal_snapshot(1, 1, &snapshot, false)?)?;
    let mut public_sequence = 1_u64;
    let mut private_sequence = 1_u64;
    let mut revision = snapshot.revision;
    let mut history_generation = snapshot.history_generation;
    let mut columns = snapshot.columns;
    let mut rows = snapshot.rows;
    loop {
        let (event_subscription, sequence, event) = tokio::select! {
            result = next_private_event(connection) => result?,
            _ = tokio::signal::ctrl_c() => return Ok(()),
        };
        public_sequence = public_sequence
            .checked_add(1)
            .context("public sequence exhausted")?;
        if event_subscription != subscription_id {
            bail!("splinterd sent an event for the wrong subscription");
        }
        if sequence != private_sequence {
            write_json_document(&CliEventV2::terminal_resync(
                1,
                public_sequence,
                splint_id,
                incarnation,
                revision,
                Some(history_generation),
                ResyncReasonV2::RevisionGap,
            )?)?;
            return Ok(());
        }
        private_sequence = private_sequence
            .checked_add(1)
            .context("private sequence exhausted")?;
        match event {
            SubscriptionEvent::Snapshot { snapshot } => {
                if snapshot.splint_id != splint_id || snapshot.incarnation != incarnation {
                    bail!("terminal subscription snapshot identity changed");
                }
                revision = snapshot.revision;
                history_generation = snapshot.history_generation;
                columns = snapshot.columns;
                rows = snapshot.rows;
                write_json_document(&CliEventV2::terminal_snapshot(
                    1,
                    public_sequence,
                    &snapshot,
                    false,
                )?)?;
            }
            SubscriptionEvent::Update { update } => {
                update
                    .validate_against(revision, history_generation, columns, rows)
                    .map_err(|error| anyhow::anyhow!(error.message))?;
                revision = update.revision;
                columns = update.columns.unwrap_or(columns);
                rows = update.row_count.unwrap_or(rows);
                if let Some(scrollback) = &update.scrollback {
                    history_generation = scrollback.history_generation;
                    if !matches!(scrollback.transition, HistoryTransition::Append { .. }) {
                        write_json_document(&CliEventV2::terminal_resync(
                            1,
                            public_sequence,
                            splint_id,
                            incarnation,
                            revision,
                            Some(history_generation),
                            ResyncReasonV2::HistoryReplaced,
                        )?)?;
                        return Ok(());
                    }
                }
                write_json_document(&CliEventV2::terminal_update(
                    1,
                    public_sequence,
                    splint_id,
                    incarnation,
                    revision,
                    history_generation,
                )?)?;
            }
            SubscriptionEvent::ResyncRequired { current_revision } => {
                write_json_document(&CliEventV2::terminal_resync(
                    1,
                    public_sequence,
                    splint_id,
                    incarnation,
                    current_revision,
                    Some(history_generation),
                    ResyncReasonV2::SubscriberStalled,
                )?)?;
                return Ok(());
            }
            SubscriptionEvent::AccessRevoked { grant_id } => {
                write_json_document(&CliEventV2::access_revoked(
                    1,
                    public_sequence,
                    splint_id,
                    incarnation,
                    grant_id,
                )?)?;
                return Ok(());
            }
            SubscriptionEvent::Exited { code, signal } => {
                write_json_document(&CliEventV2::exited(
                    1,
                    public_sequence,
                    splint_id,
                    incarnation,
                    code,
                    signal,
                )?)?;
                return Ok(());
            }
            _ => bail!("splinterd sent a non-terminal event on a terminal subscription"),
        }
    }
}

async fn run_topology_subscription(
    connection: &mut Connection,
    setup_deadline: std::time::Duration,
) -> Result<()> {
    let response = connection
        .request_with_deadline(Request::SubscribeTopology, setup_deadline)
        .await?;
    let Response::TopologySubscribed {
        subscription_id,
        snapshot,
    } = response
    else {
        bail!("splinterd returned an unexpected topology subscription response");
    };
    write_json_document(&CliEventV2::topology_snapshot(1, 1, &snapshot)?)?;
    let mut public_sequence = 1_u64;
    let mut private_sequence = 1_u64;
    let mut revision = snapshot.revision;
    loop {
        let (event_subscription, sequence, event) = tokio::select! {
            result = next_private_event(connection) => result?,
            _ = tokio::signal::ctrl_c() => return Ok(()),
        };
        public_sequence = public_sequence
            .checked_add(1)
            .context("public sequence exhausted")?;
        if event_subscription != subscription_id {
            bail!("splinterd sent an event for the wrong subscription");
        }
        let event_revision = match &event {
            SubscriptionEvent::TopologyChanged { change } => change.revision,
            SubscriptionEvent::TopologyResyncRequired { current_revision } => *current_revision,
            _ => bail!("splinterd sent a non-topology event on a topology subscription"),
        };
        if sequence != private_sequence {
            write_json_document(&CliEventV2::topology_resync(
                1,
                public_sequence,
                event_revision,
                ResyncReasonV2::RevisionGap,
            )?)?;
            return Ok(());
        }
        private_sequence = private_sequence
            .checked_add(1)
            .context("private sequence exhausted")?;
        match event {
            SubscriptionEvent::TopologyChanged { change } => {
                if change.revision <= revision {
                    bail!("topology subscription revision did not advance");
                }
                change
                    .validate()
                    .map_err(|error| anyhow::anyhow!(error.message))?;
                revision = change.revision;
                write_json_document(&CliEventV2::topology_changed(
                    1,
                    public_sequence,
                    change.kind,
                    &change.snapshot,
                )?)?;
            }
            SubscriptionEvent::TopologyResyncRequired { current_revision } => {
                write_json_document(&CliEventV2::topology_resync(
                    1,
                    public_sequence,
                    current_revision,
                    ResyncReasonV2::SubscriberStalled,
                )?)?;
                return Ok(());
            }
            _ => unreachable!("topology event checked above"),
        }
    }
}

async fn run_control_subscription(
    connection: &mut Connection,
    splint_id: SplintId,
    expected_incarnation: Option<u64>,
    setup_deadline: std::time::Duration,
) -> Result<()> {
    let runtime = connection
        .request_with_deadline(Request::InspectSplint { splint_id }, setup_deadline)
        .await?;
    let Response::Splint { runtime, .. } = runtime else {
        bail!("splinterd did not return the selected Splint identity");
    };
    let incarnation = runtime
        .live_incarnation
        .context("selected Splint does not have a live process")?;
    require_incarnation(incarnation, expected_incarnation)?;
    let response = connection
        .request_with_deadline(
            Request::SubscribeControl {
                splint_id,
                incarnation,
            },
            setup_deadline,
        )
        .await?;
    let Response::ControlSubscribed {
        subscription_id,
        status,
    } = response
    else {
        bail!("splinterd returned an unexpected control subscription response");
    };
    write_json_document(&CliEventV2::control_snapshot(1, 1, status)?)?;
    let mut public_sequence = 1_u64;
    let mut private_sequence = 1_u64;
    let mut transfer_ids = HashMap::<u64, u64>::new();
    let mut next_transfer_id = 1_u64;
    loop {
        let (event_subscription, sequence, event) = tokio::select! {
            result = next_private_event(connection) => result?,
            _ = tokio::signal::ctrl_c() => return Ok(()),
        };
        public_sequence = public_sequence
            .checked_add(1)
            .context("public sequence exhausted")?;
        if event_subscription != subscription_id {
            bail!("splinterd sent an event for the wrong subscription");
        }
        if sequence != private_sequence {
            write_json_document(&CliEventV2::control_resync(
                1,
                public_sequence,
                splint_id,
                incarnation,
                ResyncReasonV2::RevisionGap,
            )?)?;
            return Ok(());
        }
        private_sequence = private_sequence
            .checked_add(1)
            .context("private sequence exhausted")?;
        let record = match event {
            SubscriptionEvent::ControlStatusChanged { status } => {
                CliEventV2::control_status_changed(1, public_sequence, status)?
            }
            SubscriptionEvent::ControlTransferRequested { transfer_id } => {
                if transfer_ids.len() >= 64 || transfer_ids.contains_key(&transfer_id) {
                    bail!("control transfer map is full or contains a duplicate private ID");
                }
                let public_transfer_id = next_transfer_id;
                next_transfer_id = next_transfer_id
                    .checked_add(1)
                    .context("public transfer ID space exhausted")?;
                transfer_ids.insert(transfer_id, public_transfer_id);
                CliEventV2::control_transfer_requested(
                    1,
                    public_sequence,
                    splint_id,
                    incarnation,
                    public_transfer_id,
                )?
            }
            SubscriptionEvent::ControlTransferResolved {
                transfer_id,
                outcome,
                ..
            } => {
                let public_transfer_id = transfer_ids
                    .remove(&transfer_id)
                    .context("control transfer resolution has no public request mapping")?;
                CliEventV2::control_transfer_resolved(
                    1,
                    public_sequence,
                    splint_id,
                    incarnation,
                    public_transfer_id,
                    outcome,
                )?
            }
            _ => bail!("splinterd sent a non-control event on a control subscription"),
        };
        write_json_document(&record)?;
    }
}

async fn run_machine_subscription(
    stream: SubscribeCommand,
    schema_major: u16,
    timeout_ms: u64,
) -> Result<()> {
    if schema_major != 2 {
        bail!("unsupported schema major {schema_major}");
    }
    let setup_deadline = std::time::Duration::from_millis(timeout_ms);
    let mut connection = tokio::time::timeout(setup_deadline, Connection::connect_automation())
        .await
        .context("subscription connection deadline elapsed")??;
    match stream {
        SubscribeCommand::Terminal {
            splint_id,
            expected_incarnation,
        } => {
            run_terminal_subscription(
                &mut connection,
                splint_id,
                expected_incarnation,
                setup_deadline,
            )
            .await
        }
        SubscribeCommand::Topology => {
            run_topology_subscription(&mut connection, setup_deadline).await
        }
        SubscribeCommand::Control {
            splint_id,
            expected_incarnation,
        } => {
            run_control_subscription(
                &mut connection,
                splint_id,
                expected_incarnation,
                setup_deadline,
            )
            .await
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "explicit-ID lifecycle command construction stays adjacent for auditability"
)]
async fn run_headless(command: Command, config: &AppConfig) -> Result<()> {
    let mut connection = Connection::connect().await?;
    match command {
        Command::Sessions
        | Command::Reopen
        | Command::Window { .. }
        | Command::Launch { .. }
        | Command::Consent
        | Command::Policy { .. }
        | Command::Relay { .. }
        | Command::Reset { .. } => {
            unreachable!("graphical, policy, or relay command returned before daemon connection")
        }
        Command::Ping => print_response(connection.request(Request::Ping).await?),
        Command::List { all } => {
            let Response::Lairs { lairs, .. } = connection.request(Request::ListLairs).await?
            else {
                anyhow::bail!("splinterd returned an unexpected response to list")
            };
            print_lairs(&lairs, all);
            Ok(())
        }
        Command::Topology => print_response(connection.request(Request::InspectTopology).await?),
        Command::Inspect { splint_id } => print_response(
            connection
                .request(Request::InspectSplint { splint_id })
                .await?,
        ),
        Command::New { name, cwd, command } => {
            let expected = connection.topology_revision().await?;
            print_response(
                connection
                    .request(create_request(
                        expected,
                        name,
                        cwd.unwrap_or(
                            env::current_dir().context("failed to read current directory")?,
                        ),
                        command,
                        config,
                    ))
                    .await?,
            )
        }
        Command::Split {
            target_splint_id,
            axis,
            side,
            ratio,
            expected_incarnation,
            cwd,
            command,
        } => {
            let ratio = SplitRatio::new(ratio)
                .map_err(|_| anyhow::anyhow!("split ratio must be between 1 and 999"))?;
            let Response::Topology { snapshot } =
                connection.request(Request::InspectTopology).await?
            else {
                bail!("splinterd returned an unexpected topology response");
            };
            require_expected_incarnation(&snapshot, target_splint_id, expected_incarnation)?;
            let expected_topology_revision = snapshot.revision;
            print_response(
                connection
                    .request(Request::SplitSplint {
                        expected_topology_revision,
                        target_splint_id,
                        axis: axis.into(),
                        side: side.into(),
                        ratio,
                        launch: launch_parameters(
                            cwd.unwrap_or(
                                env::current_dir().context("failed to read current directory")?,
                            ),
                            command,
                            config,
                        ),
                    })
                    .await?,
            )
        }
        Command::Close { splint_id, .. } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::CloseSplint {
                        expected_topology_revision,
                        splint_id,
                    })
                    .await?,
            )
        }
        Command::Ratio {
            target_splint_id,
            ratio,
        } => {
            let expected_topology_revision = connection.topology_revision().await?;
            let ratio = SplitRatio::new(ratio)
                .map_err(|_| anyhow::anyhow!("split ratio must be between 1 and 999"))?;
            print_response(
                connection
                    .request(Request::SetSplitRatio {
                        expected_topology_revision,
                        target_splint_id,
                        ratio,
                    })
                    .await?,
            )
        }
        Command::NewDojo {
            lair_id,
            name,
            cwd,
            command,
        } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::NewDojo {
                        expected_topology_revision,
                        lair_id,
                        name,
                        launch: launch_parameters(
                            cwd.unwrap_or(
                                env::current_dir().context("failed to read current directory")?,
                            ),
                            command,
                            config,
                        ),
                    })
                    .await?,
            )
        }
        Command::CloseDojo { dojo_id, .. } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::CloseDojo {
                        expected_topology_revision,
                        dojo_id,
                    })
                    .await?,
            )
        }
        Command::RenameLair { lair_id, name } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::RenameLair {
                        expected_topology_revision,
                        lair_id,
                        name,
                    })
                    .await?,
            )
        }
        Command::RenameDojo { dojo_id, name } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::RenameDojo {
                        expected_topology_revision,
                        dojo_id,
                        name,
                    })
                    .await?,
            )
        }
        Command::DojoFocusHint { dojo_id, splint_id } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::SetDojoDefaultFocus {
                        expected_topology_revision,
                        dojo_id,
                        splint_id,
                    })
                    .await?,
            )
        }
        Command::RenameSplint { splint_id, title } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::RenameSplint {
                        expected_topology_revision,
                        splint_id,
                        title,
                    })
                    .await?,
            )
        }
        Command::Relaunch {
            splint_id,
            cwd,
            command,
        } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::RelaunchSplint {
                        expected_topology_revision,
                        splint_id,
                        launch: launch_parameters(
                            cwd.unwrap_or(
                                env::current_dir().context("failed to read current directory")?,
                            ),
                            command,
                            config,
                        ),
                    })
                    .await?,
            )
        }
        Command::Restore { splint_id } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::RestoreSplint {
                        expected_topology_revision,
                        splint_id,
                    })
                    .await?,
            )
        }
        Command::RestoreDojo { dojo_id } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::RestoreDojo {
                        expected_topology_revision,
                        dojo_id,
                    })
                    .await?,
            )
        }
        Command::RestoreLair { lair_id } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::RestoreLair {
                        expected_topology_revision,
                        lair_id,
                    })
                    .await?,
            )
        }
        Command::Authorization { .. } | Command::Audit { .. } | Command::Subscribe { .. } => {
            bail!("authorization, audit, and subscriptions require machine output")
        }
        Command::Snapshot {
            splint_id,
            expected_incarnation,
        } => {
            let incarnation = connection.live_incarnation(splint_id).await?;
            require_incarnation(incarnation, expected_incarnation)?;
            print_response(
                connection
                    .request(Request::Attach {
                        splint_id,
                        incarnation: Some(incarnation),
                        scrollback_rows: 16,
                    })
                    .await?,
            )
        }
        Command::Scrollback { .. } | Command::Search { .. } => {
            bail!("scrollback and search require --output json")
        }
        Command::Send {
            splint_id,
            text,
            expected_incarnation,
        } => {
            let incarnation = connection.live_incarnation(splint_id).await?;
            require_incarnation(incarnation, expected_incarnation)?;
            let controller_id = connection
                .acquire_control(splint_id, incarnation, vec![ControlMode::Input])
                .await?;
            let response = connection
                .request(Request::Input {
                    controller_id,
                    splint_id,
                    incarnation,
                    bytes: text.into_bytes(),
                })
                .await?;
            connection.release_control(controller_id).await?;
            print_response(response)
        }
        Command::Resize {
            splint_id,
            columns,
            rows,
        } => {
            let incarnation = connection.live_incarnation(splint_id).await?;
            let controller_id = connection
                .acquire_control(splint_id, incarnation, vec![ControlMode::Resize])
                .await?;
            let response = connection
                .request(Request::Resize {
                    controller_id,
                    splint_id,
                    incarnation,
                    columns,
                    rows,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .await?;
            connection.release_control(controller_id).await?;
            print_response(response)
        }
        Command::Kill { splint_id, yes } => {
            if !yes && !confirm_kill(splint_id)? {
                println!("Kill cancelled.");
                return Ok(());
            }
            let incarnation = connection.live_incarnation(splint_id).await?;
            print_response(
                connection
                    .request(Request::KillSplint {
                        splint_id,
                        incarnation,
                    })
                    .await?,
            )
        }
    }
}

fn launch_parameters(cwd: PathBuf, command: Vec<String>, config: &AppConfig) -> LaunchParameters {
    LaunchParameters {
        cwd,
        command,
        shell: config.shell.clone(),
        login_shell: config.login_shell,
        scrollback_lines: config.scrollback_lines,
    }
}

fn create_request(
    expected_topology_revision: TopologyRevision,
    name: String,
    cwd: PathBuf,
    command: Vec<String>,
    config: &AppConfig,
) -> Request {
    Request::CreateLair {
        expected_topology_revision,
        name,
        launch: launch_parameters(cwd, command, config),
    }
}

fn collect_choices(
    node: &splinterm_core::LayoutNode,
    lair: &str,
    dojo: &str,
    choices: &mut Vec<(SplintId, String)>,
) {
    match node {
        splinterm_core::LayoutNode::Leaf(splint) => choices.push((
            splint.id,
            format!("{lair} / {dojo} / {} ({:?})", splint.title, splint.state),
        )),
        splinterm_core::LayoutNode::Branch { first, second, .. } => {
            collect_choices(first, lair, dojo, choices);
            collect_choices(second, lair, dojo, choices);
        }
    }
}

fn parse_session_choice(
    choices: &[(SplintId, String)],
    allow_new: bool,
    answer: &str,
) -> Result<Option<SplintId>> {
    let answer = answer.trim();
    if allow_new && answer.eq_ignore_ascii_case("new") {
        return Ok(None);
    }
    let selected: SplintId = answer.parse().context("selection is not a Splint UUID")?;
    choices
        .iter()
        .any(|(id, _)| *id == selected)
        .then_some(Some(selected))
        .context("selected Splint is not present in the current Lair")
}

fn choose_session(lairs: &[splinterm_core::Lair], allow_new: bool) -> Result<Option<SplintId>> {
    if !io::stdin().is_terminal() {
        let guidance = if allow_new {
            "pass --splint-id <UUID> to attach or --new to create"
        } else {
            "pass a Splint UUID explicitly"
        };
        bail!("session selection requires an interactive terminal; {guidance}");
    }
    let mut choices = Vec::new();
    for lair in lairs {
        for dojo in &lair.dojos {
            collect_choices(&dojo.root, &lair.name, &dojo.name, &mut choices);
        }
    }
    eprintln!("Saved Splints:");
    for (id, label) in &choices {
        eprintln!("  {id}  {label}");
    }
    if allow_new {
        eprintln!("  new  create a new Dojo");
    }
    eprint!(
        "Enter an exact Splint UUID{}: ",
        if allow_new { " or 'new'" } else { "" }
    );
    io::stderr()
        .flush()
        .context("failed to display session chooser")?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .context("failed to read session selection")?;
    parse_session_choice(&choices, allow_new, &answer)
}

fn select_dojo_from(
    lairs: &[splinterm_core::Lair],
    selection: (LairId, DojoId),
) -> Result<splinterm_core::Dojo> {
    let (lair_id, dojo_id) = selection;
    let lair = lairs
        .iter()
        .find(|lair| lair.id == lair_id)
        .context("selected Lair is not present in the current topology")?;
    let dojo = lair
        .dojos
        .iter()
        .find(|dojo| dojo.id == dojo_id)
        .context("selected Dojo does not belong to the selected Dojo")?;
    dojo.root
        .find_splint(dojo.default_focus)
        .context("selected Dojo has an invalid default-focus hint")?;
    Ok(dojo.clone())
}

fn dojo_containing(
    lairs: &[splinterm_core::Lair],
    splint_id: SplintId,
) -> Option<splinterm_core::Dojo> {
    lairs
        .iter()
        .flat_map(|dojo| &dojo.dojos)
        .find(|dojo| dojo.root.find_splint(splint_id).is_some())
        .cloned()
}

async fn select_dojo(selection: Option<(LairId, DojoId)>) -> Result<splinterm_core::Dojo> {
    let mut connection = Connection::connect().await?;
    let Response::Lairs { lairs, .. } = connection.request(Request::ListLairs).await? else {
        bail!("splinterd did not return its session list");
    };
    if let Some(selection) = selection {
        select_dojo_from(&lairs, selection)
    } else {
        let splint_id = choose_session(&lairs, false)?.context("no Splint was selected")?;
        dojo_containing(&lairs, splint_id)
            .context("selected Splint is not present in a daemon Dojo")
    }
}

fn recent_dojo_ids() -> Vec<DojoId> {
    RecentDojos::discover().map_or_else(
        |error| {
            eprintln!("splinterm recent sessions unavailable: {error:#}");
            Vec::new()
        },
        |store| store.load(),
    )
}

fn remember_dojo(dojo_id: DojoId) {
    match RecentDojos::discover().and_then(|store| store.record(dojo_id)) {
        Ok(()) => {}
        Err(error) => eprintln!("splinterm could not update recent sessions: {error:#}"),
    }
}

fn configure_picker_renderer(config: &AppConfig, theme: ResolvedTheme) -> Result<()> {
    renderer::configure(RendererOptions {
        font: config.font.clone(),
        font_size: config.font_size,
        font_sizing_policy: config.font_sizing_policy,
        physical_dpi: 96.0,
        padding: config.padding,
        background_alpha: theme.background_alpha,
    })
}

fn session_picker_item(entry: &SessionEntry) -> SessionPickerItem {
    SessionPickerItem {
        display_title: entry.display_title(),
        working_directory: entry.working_directory(),
        pane_count: entry.pane_count,
        running_pane_count: entry.running_panes,
    }
}

fn choose_recent_session(
    config: &AppConfig,
    entries: &[SessionEntry],
) -> Result<Option<SessionPickerDecision>> {
    let theme = load_startup_theme(config);
    configure_picker_renderer(config, theme)?;
    let items = entries.iter().map(session_picker_item).collect();
    let (decision, receiver) = std_mpsc::channel();
    let mut picker = SessionPickerUi::new(items, decision);
    let snapshot = picker.snapshot();
    run_window(WindowOptions {
        snapshot: Some(snapshot),
        session_picker: Some(picker),
        initial_columns: config.initial_columns,
        initial_rows: config.initial_rows,
        cursor_style: config.cursor_style,
        cursor_blink: false,
        theme,
        ..WindowOptions::default()
    })?;
    Ok(receiver.try_recv().ok())
}

async fn select_reopenable_dojo(lair_id: LairId, dojo_id: DojoId) -> Result<splinterm_core::Dojo> {
    let mut connection = Connection::connect().await?;
    let Response::Lairs { lairs, .. } = connection.request(Request::ListLairs).await? else {
        bail!("splinterd did not return its session list");
    };
    let dojo = select_dojo_from(&lairs, (lair_id, dojo_id))?;
    let reopenable = collect_sessions(&lairs, &[])
        .into_iter()
        .find(|entry| entry.dojo_id == dojo_id)
        .is_some_and(|entry| entry.reopenable());
    anyhow::ensure!(
        reopenable,
        "selected session no longer has a fully running pane layout"
    );
    Ok(dojo)
}

async fn run_sessions(config: AppConfig) -> Result<()> {
    let mut connection = Connection::connect()
        .await
        .context("splinterd is unavailable; start splinterd.service or run splinterd")?;
    let Response::Lairs { lairs, .. } = connection.request(Request::ListLairs).await? else {
        bail!("splinterd did not return its session list");
    };
    drop(connection);
    let entries = collect_sessions(&lairs, &recent_dojo_ids())
        .into_iter()
        .filter(SessionEntry::reopenable)
        .collect::<Vec<_>>();
    let picker_entries = entries.clone();
    let picker_config = config.clone();
    let decision =
        tokio::task::spawn_blocking(move || choose_recent_session(&picker_config, &picker_entries))
            .await
            .context("session picker task failed")??;
    match decision {
        None => Ok(()),
        Some(SessionPickerDecision::New) => {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            launch(
                Some(format!("terminal-{stamp}-{}", std::process::id())),
                env::current_dir().context("failed to read current directory")?,
                None,
                true,
                Vec::new(),
                config,
            )
            .await
        }
        Some(SessionPickerDecision::Open(index)) => {
            let selected = entries
                .get(index)
                .context("session picker returned an invalid selection")?;
            let dojo = select_reopenable_dojo(selected.lair_id, selected.dojo_id).await?;
            remember_dojo(dojo.id);
            run_live_multipane_window(config, dojo).await
        }
    }
}

async fn reopen_recent(config: AppConfig) -> Result<()> {
    let mut connection = Connection::connect()
        .await
        .context("splinterd is unavailable; start splinterd.service or run splinterd")?;
    let Response::Lairs { lairs, .. } = connection.request(Request::ListLairs).await? else {
        bail!("splinterd did not return its session list");
    };
    let recent = recent_dojo_ids();
    let entries = collect_sessions(&lairs, &recent);
    let selected = recent
        .iter()
        .find_map(|dojo_id| {
            entries
                .iter()
                .find(|entry| entry.dojo_id == *dojo_id && entry.reopenable())
        })
        .context("no recent running session; open the session picker with `splinterm sessions`")?;
    let dojo = select_dojo_from(&lairs, (selected.lair_id, selected.dojo_id))?;
    drop(connection);
    remember_dojo(dojo.id);
    run_live_multipane_window(config, dojo).await
}

fn fresh_dojo_name(now: SystemTime, process_id: u32) -> String {
    let stamp = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    format!("terminal-{stamp}-{process_id}")
}

async fn launch(
    name: Option<String>,
    cwd: PathBuf,
    splint_id: Option<SplintId>,
    _create_new: bool,
    command: Vec<String>,
    config: AppConfig,
) -> Result<()> {
    let mut connection = Connection::connect()
        .await
        .context("splinterd is unavailable; start splinterd.service or run splinterd")?;
    let Response::Lairs { lairs, .. } = connection.request(Request::ListLairs).await? else {
        bail!("splinterd did not return its session list");
    };
    if let Some(splint_id) = splint_id {
        if !command.is_empty() {
            bail!("cannot execute a new command while attaching an existing Splint");
        }
        let dojo = dojo_containing(&lairs, splint_id)
            .context("selected Splint is not present in a daemon Dojo")?;
        remember_dojo(dojo.id);
        drop(connection);
        return run_live_window(config, splint_id).await;
    }

    let name = name.unwrap_or_else(|| fresh_dojo_name(SystemTime::now(), std::process::id()));
    let expected = connection.topology_revision().await?;
    let Response::LairCreated { lair: dojo, .. } = connection
        .request(create_request(expected, name, cwd, command, &config))
        .await?
    else {
        bail!("splinterd did not create the requested terminal");
    };
    let dojo = dojo
        .dojos
        .first()
        .context("new Lair did not contain a Dojo")?;
    if !matches!(&dojo.root, splinterm_core::LayoutNode::Leaf(_)) {
        bail!("new dojo did not contain exactly one Splint");
    }
    let dojo = dojo.clone();
    remember_dojo(dojo.id);
    drop(connection);
    run_live_multipane_window(config, dojo).await
}

fn read_private_frame<T: serde::de::DeserializeOwned>(reader: &mut impl Read) -> Result<T> {
    let mut length = [0_u8; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_CONSENT_FRAME_BYTES {
        bail!("invalid private consent frame length");
    }
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).context("invalid private consent frame")
}

fn write_private_frame<T: serde::Serialize>(writer: &mut impl Write, value: &T) -> Result<()> {
    let body = serde_json::to_vec(value).context("encode private consent frame")?;
    if body.is_empty() || body.len() > MAX_CONSENT_FRAME_BYTES {
        bail!("private consent frame exceeds limit");
    }
    writer.write_all(&u32::try_from(body.len())?.to_be_bytes())?;
    writer.write_all(&body)?;
    writer.flush()?;
    Ok(())
}

fn consent_input_modes() -> TerminalInputModes {
    TerminalInputModes {
        application_cursor: false,
        application_keypad: false,
        focus_reporting: false,
        bracketed_paste: false,
        cursor_visible: false,
        cursor_blink: false,
        mouse_tracking: splinterm_protocol::MouseTracking::None,
        sgr_mouse: false,
    }
}

fn consent_snapshot(prompt: &ConsentPrompt) -> TerminalSnapshot {
    let mut lines = vec![
        "TRUSTED SPLINTERM ACCESS REQUEST".to_owned(),
        String::new(),
        format!("Requester: {}", prompt.requester),
        format!(
            "Process: PID {} · UID {}",
            prompt.requester_pid, prompt.requester_uid
        ),
        format!(
            "Splint: {:?} · incarnation {}",
            prompt.splint_id, prompt.incarnation
        ),
        String::new(),
        "Requested one-time capabilities:".to_owned(),
    ];
    lines.extend(
        prompt
            .scopes
            .iter()
            .map(|scope| format!("  • {}", scope.label())),
    );
    lines.extend([
        String::new(),
        "This grant expires automatically and is not persisted.".to_owned(),
        "D / Escape: DENY          G / Enter: GRANT ONCE".to_owned(),
        "Click the red left or green right action area below.".to_owned(),
    ]);
    let columns = lines
        .iter()
        .map(String::len)
        .max()
        .unwrap_or(1)
        .clamp(64, 120);
    let rows = lines.len().max(18);
    let blank_attributes = CellAttributes {
        bold: false,
        dim: false,
        italic: false,
        underline: UnderlineStyle::None,
        underline_color_source: ColorSource::Default,
        underline_color: 0,
        strikethrough: false,
        blink: false,
        conceal: false,
        reverse: false,
        foreground_source: ColorSource::Default,
        foreground: 0,
        background_source: ColorSource::Default,
        background: 0,
    };
    let mut visible_rows: Vec<_> = lines
        .into_iter()
        .map(|line| TerminalRow {
            row_id: None,
            linebreak: false,
            cells: line
                .chars()
                .take(columns)
                .map(|character| TerminalCell {
                    content: character.to_string(),
                    spacer_remaining: None,
                    attributes: blank_attributes,
                })
                .collect(),
        })
        .collect();
    visible_rows.resize_with(rows, || TerminalRow {
        row_id: None,
        linebreak: false,
        cells: Vec::new(),
    });
    TerminalSnapshot {
        splint_id: prompt.splint_id,
        incarnation: prompt.incarnation,
        revision: 1,
        columns,
        rows,
        cursor_column: -1,
        cursor_row: -1,
        cursor_deferred_wrap: false,
        active_screen: ActiveScreen::Normal,
        input_modes: consent_input_modes(),
        palette: vec![0; 256],
        default_colors: [0x00f4_f0e8, 0x0014_1820, 0x00e0_a030],
        title: "Trusted access request".to_owned(),
        visible_rows,
        history_generation: 1,
        oldest_available_scrollback_row_id: None,
        newest_available_scrollback_row_id: None,
        scrollback_rows: Vec::new(),
        available_scrollback_rows: 0,
        omitted_oldest_scrollback_rows: 0,
        images: None,
        exited_code: None,
        exited_signal: None,
    }
}

fn run_consent_client() -> Result<()> {
    let prompt: ConsentPrompt = read_private_frame(&mut io::stdin().lock())?;
    if prompt.capability.len() != splinterm_protocol::CONSENT_CAPABILITY_BYTES
        || prompt.scopes.is_empty()
        || prompt.scopes.len() > splinterm_protocol::MAX_ACCESS_SCOPES
        || prompt.requester.chars().count() > 1024
    {
        bail!("invalid trusted consent prompt");
    }
    let (decision, receiver) = std_mpsc::channel();
    run_window(WindowOptions {
        snapshot: Some(consent_snapshot(&prompt)),
        trusted_consent: Some(TrustedConsentUi { decision }),
        ..WindowOptions::default()
    })?;
    let granted = receiver.try_recv().unwrap_or(false);
    write_private_frame(
        &mut io::stdout().lock(),
        &ConsentReply {
            capability: prompt.capability,
            granted,
        },
    )
}

struct Attachment {
    subscription_id: u64,
    snapshot: TerminalSnapshot,
}

#[allow(
    clippy::large_enum_variant,
    reason = "subscription events already own bounded protocol snapshots"
)]
#[derive(Debug, PartialEq)]
enum EventAction {
    Ignore,
    Snapshot {
        sequence: u64,
        snapshot: TerminalSnapshot,
    },
    Update {
        sequence: u64,
        update: TerminalUpdate,
    },
    Resynchronize,
    Exited,
    Shutdown,
}

fn classify_subscription_event(
    expected_subscription: u64,
    last_sequence: u64,
    subscription_id: u64,
    sequence: u64,
    event: SubscriptionEvent,
) -> EventAction {
    if subscription_id != expected_subscription {
        return EventAction::Ignore;
    }
    match event {
        SubscriptionEvent::Exited { .. } => EventAction::Exited,
        SubscriptionEvent::AccessRevoked { .. } => EventAction::Shutdown,
        _ if last_sequence.checked_add(1) != Some(sequence) => EventAction::Resynchronize,
        SubscriptionEvent::Snapshot { snapshot } => EventAction::Snapshot { sequence, snapshot },
        SubscriptionEvent::Update { update } => EventAction::Update { sequence, update },
        SubscriptionEvent::ResyncRequired { .. } => EventAction::Resynchronize,
        SubscriptionEvent::TopologyChanged { .. }
        | SubscriptionEvent::TopologyResyncRequired { .. }
        | SubscriptionEvent::ControlStatusChanged { .. }
        | SubscriptionEvent::ControlTransferRequested { .. }
        | SubscriptionEvent::ControlTransferResolved { .. } => EventAction::Ignore,
    }
}

fn update_advances_from(update: &TerminalUpdate, current_revision: u64) -> bool {
    update.base_revision == current_revision && update.revision > current_revision
}

fn validate_attached_snapshot(
    snapshot: &TerminalSnapshot,
    splint_id: SplintId,
    incarnation: u64,
) -> Result<()> {
    snapshot
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message.clone()))?;
    if snapshot.splint_id != splint_id || snapshot.incarnation != incarnation {
        bail!("splinterd returned a snapshot for a different live Splint identity");
    }
    Ok(())
}

async fn attach(
    connection: &mut Connection,
    splint_id: SplintId,
    incarnation: u64,
) -> Result<Attachment> {
    let Response::Attached {
        subscription_id,
        snapshot,
        ..
    } = connection
        .request(Request::Attach {
            splint_id,
            incarnation: Some(incarnation),
            scrollback_rows: splinterm_protocol::MAX_SNAPSHOT_SCROLLBACK_ROWS,
        })
        .await?
    else {
        bail!("splinterd did not return an attached terminal snapshot");
    };
    validate_attached_snapshot(&snapshot, splint_id, incarnation)?;
    Ok(Attachment {
        subscription_id,
        snapshot,
    })
}

async fn resynchronize(
    connection: &mut Connection,
    old_subscription: u64,
    splint_id: SplintId,
    incarnation: u64,
) -> Result<Attachment> {
    let _ = connection
        .request(Request::Detach {
            subscription_id: old_subscription,
        })
        .await?;
    attach(connection, splint_id, incarnation).await
}

fn authority_status(grants: Vec<AccessGrant>, development_bypass: bool) -> AuthorityStatus {
    AuthorityStatus {
        grants: grants
            .into_iter()
            .filter(|grant| grant.grant_id != 0)
            .map(|grant| {
                let scopes = grant
                    .scopes
                    .iter()
                    .map(|scope| scope.label())
                    .collect::<Vec<_>>()
                    .join(", ");
                (grant.grant_id, format!("{}: {scopes}", grant.requester))
            })
            .collect(),
        development_bypass,
    }
}

async fn load_authority_status(
    connection: &mut Connection,
    splint_id: SplintId,
    incarnation: u64,
) -> Result<AuthorityStatus> {
    match connection
        .request(Request::AuthorizationStatus {
            splint_id,
            incarnation: Some(incarnation),
        })
        .await?
    {
        Response::AuthorizationStatus {
            grants,
            persistent: _,
            development_bypass,
            ..
        } => Ok(authority_status(grants, development_bypass)),
        _ => bail!("splinterd did not return authorization status"),
    }
}

fn validate_scrollback_page_response(
    page: &splinterm_protocol::ScrollbackPage,
    splint_id: SplintId,
    incarnation: u64,
    terminal_revision: u64,
    history_generation: u64,
    before_row_id: u64,
) -> Result<()> {
    page.validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    if page.splint_id != splint_id
        || page.incarnation != incarnation
        || page.terminal_revision != terminal_revision
        || page.history_generation != history_generation
        || page
            .rows
            .iter()
            .filter_map(|row| row.row_id)
            .any(|row_id| row_id >= before_row_id)
    {
        bail!("splinterd returned a scrollback page outside the requested bounds");
    }
    Ok(())
}

async fn fetch_scrollback_pages(
    connection: &mut Connection,
    splint_id: SplintId,
    incarnation: u64,
    terminal_revision: u64,
    history_generation: u64,
    mut before_row_id: u64,
) -> Result<Option<Vec<splinterm_protocol::ScrollbackPage>>> {
    const PREFETCH_PAGE_COUNT: usize = 4;
    let started = std::time::Instant::now();
    let mut pages = Vec::with_capacity(PREFETCH_PAGE_COUNT);
    for _ in 0..PREFETCH_PAGE_COUNT {
        let response = connection
            .request(Request::ScrollbackPage {
                splint_id,
                incarnation,
                terminal_revision,
                history_generation,
                before_row_id,
                max_rows: splinterm_protocol::MAX_SCROLLBACK_PAGE_ROWS,
            })
            .await?;
        let page = match response {
            Response::ScrollbackPage { page, .. } => page,
            Response::ScrollbackResyncRequired { .. } => return Ok(None),
            _ => bail!("splinterd did not return a scrollback page"),
        };
        validate_scrollback_page_response(
            &page,
            splint_id,
            incarnation,
            terminal_revision,
            history_generation,
            before_row_id,
        )?;
        let next_before = page.rows.first().and_then(|row| row.row_id);
        let has_older = page.has_older;
        if page.rows.is_empty() {
            break;
        }
        pages.push(page);
        if !has_older {
            break;
        }
        let Some(next_before) = next_before else {
            break;
        };
        before_row_id = next_before;
    }
    if std::env::var_os("SPLINTERM_SCROLL_TRACE").is_some() {
        eprintln!(
            "scroll-trace page_batch_us={} pages={} rows={}",
            started.elapsed().as_micros(),
            pages.len(),
            pages.iter().map(|page| page.rows.len()).sum::<usize>(),
        );
    }
    Ok(Some(pages))
}

struct ControllerOutputs {
    updates: mpsc::Sender<WindowUpdate>,
    resyncs: mpsc::Sender<()>,
}

type PaneResize = (u16, u16, u16, u16);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingPaneResize {
    size: PaneResize,
    claim_control: bool,
}

impl PendingPaneResize {
    fn from_command(command: WindowCommand) -> std::result::Result<Self, WindowCommand> {
        match command {
            WindowCommand::Resize {
                columns,
                rows,
                pixel_width,
                pixel_height,
            } => Ok(Self {
                size: (columns, rows, pixel_width, pixel_height),
                claim_control: true,
            }),
            WindowCommand::PrepareResize {
                columns,
                rows,
                pixel_width,
                pixel_height,
            } => Ok(Self {
                size: (columns, rows, pixel_width, pixel_height),
                claim_control: false,
            }),
            command => Err(command),
        }
    }

    fn merge(&mut self, next: Self) {
        self.size = next.size;
        self.claim_control |= next.claim_control;
    }

    fn into_command(self) -> WindowCommand {
        let (columns, rows, pixel_width, pixel_height) = self.size;
        if self.claim_control {
            WindowCommand::Resize {
                columns,
                rows,
                pixel_width,
                pixel_height,
            }
        } else {
            WindowCommand::PrepareResize {
                columns,
                rows,
                pixel_width,
                pixel_height,
            }
        }
    }
}

fn queue_pane_resize(
    pending: &mut Option<PendingPaneResize>,
    deadline: &mut Option<tokio::time::Instant>,
    next: PendingPaneResize,
    delay: Duration,
    now: tokio::time::Instant,
) -> Option<WindowCommand> {
    if delay.is_zero() {
        return Some(next.into_command());
    }
    if let Some(pending) = pending {
        pending.merge(next);
    } else {
        *pending = Some(next);
    }
    *deadline = Some(now + delay);
    None
}

async fn wait_for_resize_deadline(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

fn terminal_action_matches(response: &Response, splint_id: SplintId, incarnation: u64) -> bool {
    matches!(
        response,
        Response::TerminalActionAcknowledged {
            splint_id: acknowledged_splint,
            incarnation: acknowledged_incarnation,
            ..
        } if *acknowledged_splint == splint_id && *acknowledged_incarnation == incarnation
    )
}

async fn apply_prepared_pane_resize(
    control: &mut Connection,
    controller_id: Option<u64>,
    prepared_resize: &mut Option<PaneResize>,
    splint_id: SplintId,
    incarnation: u64,
) -> Result<()> {
    let (Some(controller_id), Some((columns, rows, pixel_width, pixel_height))) =
        (controller_id, prepared_resize.take())
    else {
        return Ok(());
    };
    let response = control
        .request(Request::Resize {
            controller_id,
            splint_id,
            incarnation,
            columns,
            rows,
            pixel_width,
            pixel_height,
        })
        .await?;
    if !terminal_action_matches(&response, splint_id, incarnation) {
        bail!("splinterd did not acknowledge prepared pane resize");
    }
    Ok(())
}

async fn ensure_pane_control(
    control: &mut Connection,
    active_controller: &mut Option<u64>,
    prepared_resize: &mut Option<PaneResize>,
    updates: &mpsc::Sender<WindowUpdate>,
    splint_id: SplintId,
    incarnation: u64,
    apply_prepared_resize: bool,
) -> Result<Option<u64>> {
    if let Some(controller_id) = *active_controller {
        return Ok(Some(controller_id));
    }
    let Some(controller_id) = optional_pane_controller(
        control
            .acquire_control(
                splint_id,
                incarnation,
                vec![ControlMode::Input, ControlMode::Resize],
            )
            .await,
    )?
    else {
        let _ = updates.send(WindowUpdate::Control(false)).await;
        return Ok(None);
    };
    *active_controller = Some(controller_id);
    let _ = updates.send(WindowUpdate::Control(true)).await;
    if apply_prepared_resize {
        apply_prepared_pane_resize(
            control,
            *active_controller,
            prepared_resize,
            splint_id,
            incarnation,
        )
        .await?;
    }
    Ok(Some(controller_id))
}

async fn handle_scrollback_fetch(
    control: &mut Connection,
    outputs: &ControllerOutputs,
    splint_id: SplintId,
    incarnation: u64,
    terminal_revision: u64,
    history_generation: u64,
    before_row_id: u64,
) -> Result<bool> {
    match fetch_scrollback_pages(
        control,
        splint_id,
        incarnation,
        terminal_revision,
        history_generation,
        before_row_id,
    )
    .await?
    {
        Some(pages) if !pages.is_empty() => Ok(outputs
            .updates
            .send(WindowUpdate::ScrollbackPages(pages))
            .await
            .is_ok()),
        Some(_) => Ok(true),
        None => {
            let _ = outputs
                .updates
                .send(WindowUpdate::ScrollbackResyncRequired)
                .await;
            Ok(outputs.resyncs.send(()).await.is_ok())
        }
    }
}

fn resolved_resize_request(
    controller_id: Option<u64>,
    prepared_resize: &mut Option<PaneResize>,
    identity: (SplintId, u64),
    resize: PaneResize,
) -> Option<Request> {
    let Some(controller_id) = controller_id else {
        *prepared_resize = Some(resize);
        return None;
    };
    *prepared_resize = None;
    let (splint_id, incarnation) = identity;
    Some(Request::Resize {
        controller_id,
        splint_id,
        incarnation,
        columns: resize.0,
        rows: resize.1,
        pixel_width: resize.2,
        pixel_height: resize.3,
    })
}

async fn active_resize_request(
    control: &mut Connection,
    active_controller: &mut Option<u64>,
    prepared_resize: &mut Option<PaneResize>,
    updates: &mpsc::Sender<WindowUpdate>,
    identity: (SplintId, u64),
    resize: PaneResize,
) -> Result<Option<Request>> {
    let (splint_id, incarnation) = identity;
    let controller_id = ensure_pane_control(
        control,
        active_controller,
        prepared_resize,
        updates,
        splint_id,
        incarnation,
        false,
    )
    .await?;
    Ok(resolved_resize_request(
        controller_id,
        prepared_resize,
        (splint_id, incarnation),
        resize,
    ))
}

async fn handle_control_event(
    frame: ServerFrame,
    subscription_id: u64,
    active_controller: &mut Option<u64>,
    updates: &mpsc::Sender<WindowUpdate>,
) -> Result<bool> {
    let ServerFrame::Event {
        subscription_id: event_subscription,
        event,
        ..
    } = frame
    else {
        bail!("splinterd sent an unexpected control-subscription frame");
    };
    if event_subscription != subscription_id {
        return Ok(false);
    }
    let mut control_acquired = false;
    match event {
        SubscriptionEvent::ControlStatusChanged { status } => {
            if !status.locally_owned {
                *active_controller = None;
            }
            let _ = updates
                .send(WindowUpdate::Control(status.locally_owned))
                .await;
        }
        SubscriptionEvent::ControlTransferRequested { transfer_id } => {
            let _ = updates
                .send(WindowUpdate::ControlTransferRequested(transfer_id))
                .await;
        }
        SubscriptionEvent::ControlTransferResolved {
            outcome,
            controller_id,
            ..
        } => {
            if outcome == ControlTransferOutcome::Granted {
                *active_controller = controller_id;
                control_acquired = controller_id.is_some();
            }
            let _ = updates
                .send(WindowUpdate::ControlTransferResolved(outcome))
                .await;
        }
        _ => {}
    }
    Ok(control_acquired)
}

#[allow(
    clippy::too_many_lines,
    reason = "one task serializes control ownership, search, resize, and input for a pane"
)]
async fn run_controller(
    mut control: Connection,
    mut commands: mpsc::Receiver<WindowCommand>,
    outputs: ControllerOutputs,
    controller_id: Option<u64>,
    splint_id: SplintId,
    incarnation: u64,
    resize_delay_ms: u64,
) -> Result<()> {
    let mut active_controller = controller_id;
    let mut prepared_resize = None;
    let mut pending_resize = None;
    let mut resize_deadline = None;
    let mut deferred_command = None;
    let resize_delay = Duration::from_millis(resize_delay_ms);
    let Response::ControlSubscribed {
        subscription_id: control_subscription,
        status: initial_status,
    } = control
        .request(Request::SubscribeControl {
            splint_id,
            incarnation,
        })
        .await?
    else {
        bail!("splinterd did not establish a control subscription");
    };
    active_controller = active_controller.filter(|_| initial_status.locally_owned);
    let _ = outputs
        .updates
        .send(WindowUpdate::Control(initial_status.locally_owned))
        .await;
    let result = async {
        loop {
            let (command, debounce_incoming_resize) = if let Some(command) = deferred_command.take()
            {
                (Some(command), false)
            } else {
                tokio::select! {
                    biased;
                    frame = control.next_server_frame() => {
                        let control_acquired = handle_control_event(
                            frame?,
                            control_subscription,
                            &mut active_controller,
                            &outputs.updates,
                        ).await?;
                        if control_acquired && pending_resize.is_none() {
                            apply_prepared_pane_resize(
                                &mut control,
                                active_controller,
                                &mut prepared_resize,
                                splint_id,
                                incarnation,
                            ).await?;
                        }
                        continue;
                    }
                    command = commands.recv() => (command, true),
                    () = wait_for_resize_deadline(resize_deadline) => {
                        resize_deadline = None;
                        (
                            pending_resize.take().map(PendingPaneResize::into_command),
                            false,
                        )
                    }
                }
            };
            let (command, debounce_incoming_resize) = match command {
                Some(command) => (command, debounce_incoming_resize),
                None => {
                    resize_deadline = None;
                    let Some(pending) = pending_resize.take() else {
                        break;
                    };
                    (pending.into_command(), false)
                }
            };
            let command = if debounce_incoming_resize {
                match PendingPaneResize::from_command(command) {
                    Ok(next) => {
                        let Some(command) = queue_pane_resize(
                            &mut pending_resize,
                            &mut resize_deadline,
                            next,
                            resize_delay,
                            tokio::time::Instant::now(),
                        ) else {
                            continue;
                        };
                        command
                    }
                    Err(command) => {
                        if let Some(pending) = pending_resize.take() {
                            resize_deadline = None;
                            deferred_command = Some(command);
                            pending.into_command()
                        } else {
                            command
                        }
                    }
                }
            } else {
                command
            };
            let request = match command {
                WindowCommand::Input(bytes) => {
                    let Some(controller_id) = ensure_pane_control(
                        &mut control,
                        &mut active_controller,
                        &mut prepared_resize,
                        &outputs.updates,
                        splint_id,
                        incarnation,
                        true,
                    )
                    .await?
                    else {
                        continue;
                    };
                    Request::Input {
                        controller_id,
                        splint_id,
                        incarnation,
                        bytes,
                    }
                }
                WindowCommand::Resize {
                    columns,
                    rows,
                    pixel_width,
                    pixel_height,
                } => {
                    let Some(request) = active_resize_request(
                        &mut control,
                        &mut active_controller,
                        &mut prepared_resize,
                        &outputs.updates,
                        (splint_id, incarnation),
                        (columns, rows, pixel_width, pixel_height),
                    )
                    .await?
                    else {
                        continue;
                    };
                    request
                }
                WindowCommand::PrepareResize {
                    columns,
                    rows,
                    pixel_width,
                    pixel_height,
                } => {
                    let Some(request) = resolved_resize_request(
                        active_controller,
                        &mut prepared_resize,
                        (splint_id, incarnation),
                        (columns, rows, pixel_width, pixel_height),
                    ) else {
                        continue;
                    };
                    request
                }
                WindowCommand::FetchScrollback {
                    splint_id,
                    incarnation,
                    terminal_revision,
                    history_generation,
                    before_row_id,
                } => {
                    if !handle_scrollback_fetch(
                        &mut control,
                        &outputs,
                        splint_id,
                        incarnation,
                        terminal_revision,
                        history_generation,
                        before_row_id,
                    )
                    .await?
                    {
                        break;
                    }
                    continue;
                }
                WindowCommand::RevokeAccess(grant_id) => Request::RevokeAccess { grant_id },
                WindowCommand::RequestControlTransfer => {
                    if !matches!(
                        control
                            .request(Request::RequestControlTransfer {
                                splint_id,
                                incarnation,
                                modes: vec![ControlMode::Input, ControlMode::Resize],
                            })
                            .await?,
                        Response::ControlTransferPending { .. }
                    ) {
                        bail!("splinterd did not queue the control transfer");
                    }
                    continue;
                }
                WindowCommand::DecideControlTransfer {
                    transfer_id,
                    decision,
                } => Request::DecideControlTransfer {
                    transfer_id,
                    decision,
                },
                WindowCommand::ForceControlTransfer => {
                    active_controller = match control
                        .request(Request::ForceControlTransfer {
                            splint_id,
                            incarnation,
                        })
                        .await?
                    {
                        Response::ControlGranted { controller_id, .. } => Some(controller_id),
                        _ => bail!("splinterd did not grant forced control"),
                    };
                    let _ = outputs.updates.send(WindowUpdate::Control(true)).await;
                    apply_prepared_pane_resize(
                        &mut control,
                        active_controller,
                        &mut prepared_resize,
                        splint_id,
                        incarnation,
                    )
                    .await?;
                    continue;
                }
                WindowCommand::Search {
                    terminal_revision,
                    history_generation,
                    query,
                    case_sensitive,
                    cursor,
                } => {
                    match control
                        .request(Request::SearchScrollback {
                            splint_id,
                            incarnation,
                            terminal_revision,
                            history_generation,
                            query,
                            case_sensitive,
                            cursor,
                            max_results: splinterm_protocol::MAX_SEARCH_RESULTS,
                        })
                        .await?
                    {
                        Response::SearchResults { page, .. } => {
                            let _ = outputs
                                .updates
                                .send(WindowUpdate::SearchResults(page))
                                .await;
                        }
                        Response::SearchResyncRequired { .. } => {
                            let _ = outputs
                                .updates
                                .send(WindowUpdate::SearchResyncRequired)
                                .await;
                            let _ = outputs.resyncs.send(()).await;
                        }
                        _ => bail!("splinterd did not return search results"),
                    }
                    continue;
                }
                WindowCommand::ReleaseControl => {
                    let Some(controller_id) = active_controller.take() else {
                        continue;
                    };
                    let _ = outputs.updates.send(WindowUpdate::Control(false)).await;
                    Request::ReleaseControl { controller_id }
                }
            };
            let expects_terminal_action =
                matches!(&request, Request::Input { .. } | Request::Resize { .. });
            let expects_transfer_decision =
                matches!(&request, Request::DecideControlTransfer { .. });
            let response = control.request(request).await?;
            let acknowledged = if expects_terminal_action {
                terminal_action_matches(&response, splint_id, incarnation)
            } else if expects_transfer_decision {
                matches!(response, Response::ControlTransferDecided { .. })
            } else {
                matches!(response, Response::Acknowledged)
            };
            if !acknowledged {
                bail!("splinterd did not acknowledge a window control command");
            }
        }
        Ok(())
    }
    .await;
    if let Some(controller_id) = active_controller {
        let _ = control.release_control(controller_id).await;
    }
    result
}

type ThemeFileFingerprint = (u64, u64, u64, i64, i64);

fn theme_file_fingerprint(path: &std::path::Path) -> Option<ThemeFileFingerprint> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
    ))
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ThemeSourceFingerprint {
    Omarchy {
        directory: Option<ThemeFileFingerprint>,
        colors: Option<ThemeFileFingerprint>,
        foot: Option<ThemeFileFingerprint>,
    },
    Json(Option<ThemeFileFingerprint>),
}

fn theme_source_fingerprint(source: &ThemeSource) -> ThemeSourceFingerprint {
    match source {
        ThemeSource::Omarchy(theme_dir) => ThemeSourceFingerprint::Omarchy {
            directory: theme_file_fingerprint(theme_dir),
            colors: theme_file_fingerprint(&theme_dir.join("colors.toml")),
            foot: theme_file_fingerprint(&theme_dir.join("foot.ini")),
        },
        ThemeSource::Json(path) => ThemeSourceFingerprint::Json(theme_file_fingerprint(path)),
    }
}

fn resolve_configured_theme(
    source: &ThemeSource,
    alpha_override: Option<u16>,
    blur_override: Option<bool>,
) -> Result<ResolvedTheme> {
    load_theme_source(source).map(|theme| theme.with_color_overrides(alpha_override, blur_override))
}

struct StartupThemeLoad {
    theme: ResolvedTheme,
    diagnostic: Option<String>,
}

fn prepare_startup_theme(
    source: &ThemeSource,
    alpha_override: Option<u16>,
    blur_override: Option<bool>,
) -> StartupThemeLoad {
    match resolve_configured_theme(source, alpha_override, blur_override) {
        Ok(theme) => StartupThemeLoad {
            theme,
            diagnostic: None,
        },
        Err(error) => StartupThemeLoad {
            theme: ResolvedTheme::default().with_color_overrides(alpha_override, blur_override),
            diagnostic: Some(format!(
                "splinterm theme: {error:#}; using safe fallback palette"
            )),
        },
    }
}

fn load_startup_theme_with_reporter(
    config: &AppConfig,
    mut report: impl FnMut(&str),
) -> ResolvedTheme {
    let loaded = prepare_startup_theme(
        &config.theme_source(),
        config.background_alpha,
        config.background_blur,
    );
    if let Some(diagnostic) = loaded.diagnostic {
        report(&diagnostic);
    }
    loaded.theme
}

fn load_startup_theme(config: &AppConfig) -> ResolvedTheme {
    load_startup_theme_with_reporter(config, |diagnostic| eprintln!("{diagnostic}"))
}

fn resolve_live_theme_update(
    source: &ThemeSource,
    alpha_override: Option<u16>,
    blur_override: Option<bool>,
    current: ResolvedTheme,
) -> Result<Option<ResolvedTheme>> {
    let next = load_live_theme_source(source)?.with_color_overrides(alpha_override, blur_override);
    Ok((next != current).then_some(next))
}

#[derive(Default)]
struct ThemeReloadDiagnostics {
    rejection_reported: bool,
}

impl ThemeReloadDiagnostics {
    fn accepted(&mut self) {
        self.rejection_reported = false;
    }

    fn rejected(&mut self, error: &anyhow::Error) -> Option<String> {
        if self.rejection_reported {
            return None;
        }
        self.rejection_reported = true;
        Some(format!("splinterm theme reload rejected: {error:#}"))
    }
}

enum ThemeUpdateSink {
    Panes(Vec<mpsc::Sender<WindowUpdate>>),
    Topology(mpsc::Sender<WindowTopologyUpdate>),
}

async fn watch_theme(
    source: ThemeSource,
    alpha_override: Option<u16>,
    blur_override: Option<bool>,
    mut current: ResolvedTheme,
    sink: ThemeUpdateSink,
) {
    let mut observed = None;
    let mut diagnostics = ThemeReloadDiagnostics::default();
    let mut poll = tokio::time::interval(std::time::Duration::from_millis(500));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        poll.tick().await;
        let next_fingerprint = theme_source_fingerprint(&source);
        if observed.as_ref() == Some(&next_fingerprint) {
            continue;
        }
        observed = Some(next_fingerprint);
        match resolve_live_theme_update(&source, alpha_override, blur_override, current) {
            Ok(Some(next)) => {
                current = next;
                diagnostics.accepted();
                let update = ThemeUpdate {
                    generation: NEXT_THEME_UPDATE_GENERATION.fetch_add(1, Ordering::Relaxed),
                    theme: next,
                };
                let delivered = match &sink {
                    ThemeUpdateSink::Panes(updates) => {
                        let mut delivered = false;
                        for updates in updates {
                            delivered |= updates.send(WindowUpdate::Theme(update)).await.is_ok();
                        }
                        delivered
                    }
                    ThemeUpdateSink::Topology(updates) => updates
                        .send(WindowTopologyUpdate::Theme(update))
                        .await
                        .is_ok(),
                };
                if !delivered {
                    break;
                }
            }
            Ok(None) => diagnostics.accepted(),
            Err(error) => {
                if let Some(diagnostic) = diagnostics.rejected(&error) {
                    eprintln!("{diagnostic}");
                }
            }
        }
    }
}

struct PreparedPane {
    options: WindowPaneOptions,
    task: tokio::task::JoinHandle<Result<()>>,
}

fn pane_claims_initial_control(splint_id: SplintId, default_focus: SplintId) -> bool {
    splint_id == default_focus
}

fn layout_splint_ids(root: &LayoutNode, ids: &mut Vec<SplintId>) {
    match root {
        LayoutNode::Leaf(splint) => ids.push(splint.id),
        LayoutNode::Branch { first, second, .. } => {
            layout_splint_ids(first, ids);
            layout_splint_ids(second, ids);
        }
    }
}

fn optional_pane_controller(result: Result<u64>) -> Result<Option<u64>> {
    match result {
        Ok(controller_id) => Ok(Some(controller_id)),
        Err(error)
            if protocol_error(&error)
                .is_some_and(|error| error.code == ErrorCode::ControllerUnavailable) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

async fn prepare_live_pane(
    config: &AppConfig,
    splint_id: SplintId,
    image_cache: SharedImageContentCache,
    claim_control: bool,
) -> Result<PreparedPane> {
    let mut connection = Connection::connect().await?;
    let incarnation = connection.live_incarnation(splint_id).await?;
    let scopes = vec![
        AccessScope::Observe,
        AccessScope::Scrollback,
        AccessScope::Input,
        AccessScope::Resize,
    ];
    if !matches!(
        connection
            .request(Request::RequestAccess {
                splint_id,
                incarnation,
                scopes,
            })
            .await?,
        Response::AccessGranted { .. }
    ) {
        bail!("splinterd did not grant requested terminal access");
    }
    let authority = load_authority_status(&mut connection, splint_id, incarnation).await?;
    let attachment = attach(&mut connection, splint_id, incarnation).await?;
    let snapshot = attachment.snapshot.clone();
    resolve_image_contents(&mut connection, &snapshot, &image_cache).await?;
    let image_sources = lease_snapshot_images(&image_cache, &snapshot)?;
    let mut control = Connection::connect().await?;
    if control.live_incarnation(splint_id).await? != incarnation {
        bail!("control connection observed a different process incarnation");
    }
    let controller_id = if claim_control {
        optional_pane_controller(
            control
                .acquire_control(
                    splint_id,
                    incarnation,
                    vec![ControlMode::Input, ControlMode::Resize],
                )
                .await,
        )?
    } else {
        None
    };
    let (updates, receiver) = mpsc::channel(WINDOW_UPDATE_QUEUE);
    let (command_sender, commands) = mpsc::channel(WINDOW_COMMAND_QUEUE);
    let (resync_sender, resyncs) = mpsc::channel(1);
    let controller_updates = updates.clone();
    let resize_delay_ms = config.resize_delay_ms;
    let controller = tokio::spawn(run_controller(
        control,
        commands,
        ControllerOutputs {
            updates: controller_updates,
            resyncs: resync_sender,
        },
        controller_id,
        splint_id,
        incarnation,
        resize_delay_ms,
    ));
    let task_updates = updates.clone();
    let task = tokio::spawn(run_pane_subscription(
        connection,
        attachment,
        controller,
        resyncs,
        task_updates,
        splint_id,
        incarnation,
        image_cache.clone(),
    ));
    Ok(PreparedPane {
        options: WindowPaneOptions {
            snapshot,
            updates: receiver,
            commands: command_sender,
            authority,
            controlled: controller_id.is_some(),
            image_sources,
        },
        task,
    })
}

fn lease_snapshot_images(
    cache: &SharedImageContentCache,
    snapshot: &TerminalSnapshot,
) -> Result<ImageContentLeaseSet> {
    snapshot.images.as_ref().map_or_else(
        || Ok(ImageContentLeaseSet::default()),
        |images| cache.lease(&images.contents),
    )
}

fn lease_update_images(
    cache: &SharedImageContentCache,
    update: &TerminalUpdate,
) -> Result<Option<ImageContentLeaseSet>> {
    update
        .images
        .as_ref()
        .map(|images| cache.lease(&images.contents))
        .transpose()
}

async fn resolve_image_contents(
    connection: &mut Connection,
    snapshot: &TerminalSnapshot,
    cache: &SharedImageContentCache,
) -> Result<()> {
    let Some(images) = &snapshot.images else {
        return Ok(());
    };
    let cancellation = tokio_util::sync::CancellationToken::new();
    for metadata in &images.contents {
        if !cache.contains(metadata)? {
            let source = connection
                .image_content_source(
                    snapshot.splint_id,
                    snapshot.incarnation,
                    metadata,
                    &cancellation,
                )
                .await?;
            cache.insert_source(metadata, source)?;
        }
    }
    Ok(())
}

async fn resolve_update_images(
    connection: &mut Connection,
    update: &TerminalUpdate,
    splint_id: SplintId,
    incarnation: u64,
    cache: &SharedImageContentCache,
) -> Result<()> {
    let Some(images) = &update.images else {
        return Ok(());
    };
    let cancellation = tokio_util::sync::CancellationToken::new();
    for metadata in &images.contents {
        if !cache.contains(metadata)? {
            let source = connection
                .image_content_source(splint_id, incarnation, metadata, &cancellation)
                .await?;
            cache.insert_source(metadata, source)?;
        }
    }
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "subscription ordering and controller resynchronization form one pane lifecycle"
)]
async fn run_pane_subscription(
    mut connection: Connection,
    mut attachment: Attachment,
    mut controller: tokio::task::JoinHandle<Result<()>>,
    mut resyncs: mpsc::Receiver<()>,
    updates: mpsc::Sender<WindowUpdate>,
    splint_id: SplintId,
    incarnation: u64,
    image_cache: SharedImageContentCache,
) -> Result<()> {
    let mut last_revision = attachment.snapshot.revision;
    let mut last_sequence = 0_u64;
    loop {
        tokio::select! {
            result = &mut controller => return result.context("pane controller task failed")?,
            Some(()) = resyncs.recv() => {
                attachment = resynchronize(
                    &mut connection, attachment.subscription_id, splint_id, incarnation,
                ).await?;
                last_revision = attachment.snapshot.revision;
                last_sequence = 0;
                resolve_image_contents(&mut connection, &attachment.snapshot, &image_cache).await?;
                let image_sources = lease_snapshot_images(&image_cache, &attachment.snapshot)?;
                if updates.send(WindowUpdate::Snapshot {
                    snapshot: attachment.snapshot.clone(),
                    image_sources,
                }).await.is_err() {
                    return controller.await.context("pane controller task failed")?;
                }
            }
            frame = connection.next_server_frame() => {
                let ServerFrame::Event { subscription_id, sequence, event } = frame? else {
                    bail!("splinterd sent an unexpected frame while subscribed");
                };
                match classify_subscription_event(
                    attachment.subscription_id, last_sequence, subscription_id, sequence, event,
                ) {
                    EventAction::Ignore => {}
                    EventAction::Snapshot { sequence, snapshot } => {
                        validate_attached_snapshot(&snapshot, splint_id, incarnation)?;
                        last_revision = snapshot.revision;
                        resolve_image_contents(&mut connection, &snapshot, &image_cache).await?;
                        let image_sources = lease_snapshot_images(&image_cache, &snapshot)?;
                        if updates.send(WindowUpdate::Snapshot { snapshot, image_sources }).await.is_err() {
                            return controller.await.context("pane controller task failed")?;
                        }
                        last_sequence = sequence;
                    }
                    EventAction::Update { sequence, update }
                        if update_advances_from(&update, last_revision) => {
                        if perf_trace_enabled() {
                            emit_perf_trace(
                                "splinterm",
                                "client_receive",
                                PerfTraceEvent {
                                    splint_id: Some(splint_id),
                                    incarnation: Some(incarnation),
                                    base_revision: Some(update.base_revision),
                                    revision: Some(update.revision),
                                    subscription_id: Some(attachment.subscription_id),
                                    transaction_sequence: Some(sequence),
                                    rows: Some(u64::try_from(update.rows.len()).unwrap_or(u64::MAX)),
                                    count: Some(1),
                                    ..PerfTraceEvent::default()
                                },
                            );
                        }
                        last_revision = update.revision;
                        resolve_update_images(
                            &mut connection,
                            &update,
                            splint_id,
                            incarnation,
                            &image_cache,
                        ).await?;
                        let image_sources = lease_update_images(&image_cache, &update)?;
                        let base_revision = update.base_revision;
                        let revision = update.revision;
                        let queue_depth = updates.max_capacity().saturating_sub(updates.capacity());
                        let enqueue_started = perf_trace_enabled().then(Instant::now);
                        if updates.send(WindowUpdate::Update { update, image_sources }).await.is_err() {
                            return controller.await.context("pane controller task failed")?;
                        }
                        if let Some(started) = enqueue_started {
                            emit_perf_trace(
                                "splinterm",
                                "client_enqueue",
                                PerfTraceEvent {
                                    splint_id: Some(splint_id),
                                    incarnation: Some(incarnation),
                                    base_revision: Some(base_revision),
                                    revision: Some(revision),
                                    subscription_id: Some(attachment.subscription_id),
                                    transaction_sequence: Some(sequence),
                                    duration_ns: Some(
                                        u64::try_from(started.elapsed().as_nanos())
                                            .unwrap_or(u64::MAX),
                                    ),
                                    queue_depth: Some(
                                        u64::try_from(queue_depth).unwrap_or(u64::MAX),
                                    ),
                                    ..PerfTraceEvent::default()
                                },
                            );
                        }
                        last_sequence = sequence;
                    }
                    EventAction::Update { .. } | EventAction::Resynchronize => {
                        attachment = resynchronize(
                            &mut connection, attachment.subscription_id, splint_id, incarnation,
                        ).await?;
                        last_revision = attachment.snapshot.revision;
                        last_sequence = 0;
                        resolve_image_contents(&mut connection, &attachment.snapshot, &image_cache).await?;
                        let image_sources = lease_snapshot_images(&image_cache, &attachment.snapshot)?;
                        if updates.send(WindowUpdate::Snapshot {
                            snapshot: attachment.snapshot.clone(),
                            image_sources,
                        }).await.is_err() {
                            return controller.await.context("pane controller task failed")?;
                        }
                    }
                    EventAction::Exited => {
                        let _ = updates.send(WindowUpdate::Exited { splint_id }).await;
                        return controller.await.context("pane controller task failed")?;
                    }
                    EventAction::Shutdown => {
                        let _ = updates.send(WindowUpdate::Shutdown).await;
                        return controller.await.context("pane controller task failed")?;
                    }
                }
            }
        }
    }
}

fn parent_ratio(root: &LayoutNode, target: SplintId) -> Option<SplitRatio> {
    match root {
        LayoutNode::Leaf(_) => None,
        LayoutNode::Branch {
            ratio,
            first,
            second,
            ..
        } => {
            let direct_child =
                |node: &LayoutNode| matches!(node, LayoutNode::Leaf(splint) if splint.id == target);
            if direct_child(first) || direct_child(second) {
                Some(*ratio)
            } else {
                parent_ratio(first, target).or_else(|| parent_ratio(second, target))
            }
        }
    }
}

async fn inspect_optional_dojo_state(
    connection: &mut Connection,
    dojo_id: DojoId,
) -> Result<(TopologyRevision, Option<LayoutNode>)> {
    let Response::Topology { snapshot } = connection.request(Request::InspectTopology).await?
    else {
        bail!("splinterd did not return topology after edit");
    };
    snapshot
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    let root = snapshot
        .topology
        .lairs()
        .flat_map(|dojo| &dojo.dojos)
        .find(|dojo| dojo.id == dojo_id)
        .map(|dojo| dojo.root.clone());
    Ok((snapshot.revision, root))
}

async fn inspect_dojo_state(
    connection: &mut Connection,
    dojo_id: DojoId,
) -> Result<(TopologyRevision, LayoutNode)> {
    let (revision, root) = inspect_optional_dojo_state(connection, dojo_id).await?;
    Ok((
        revision,
        root.context("edited Dojo is absent from committed topology")?,
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CloseAction {
    CloseExited,
    KillAndClose { incarnation: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TopologyCommandOutcome {
    Updated,
    WindowClosed,
}

const MAX_CLOSE_TOPOLOGY_RETRIES: usize = 64;

fn close_action(root: &LayoutNode, target: SplintId) -> Result<CloseAction> {
    let splint = root
        .find_splint(target)
        .context("focused pane is absent from committed topology")?;
    if matches!(splint.state, SplintState::Exited(_)) {
        return Ok(CloseAction::CloseExited);
    }
    Ok(CloseAction::KillAndClose {
        incarnation: splint
            .last_incarnation
            .context("live focused pane has no process incarnation")?,
    })
}

fn validate_exited_close_target(
    root: &LayoutNode,
    target: SplintId,
    expected_incarnation: Option<u64>,
) -> Result<bool> {
    let splint = root
        .find_splint(target)
        .context("focused pane is absent from committed topology")?;
    anyhow::ensure!(
        matches!(splint.state, SplintState::Exited(_)),
        "pane remained live before close"
    );
    if let Some(expected) = expected_incarnation {
        anyhow::ensure!(
            splint.last_incarnation == Some(expected),
            "pane incarnation changed before close"
        );
    }
    Ok(root.splint_count() == 1)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RefreshedCloseState {
    WindowClosed,
    TargetClosed,
    Retry,
}

fn refreshed_close_state(
    root: Option<&LayoutNode>,
    target: SplintId,
    expected_incarnation: Option<u64>,
) -> Result<RefreshedCloseState> {
    let Some(root) = root else {
        return Ok(RefreshedCloseState::WindowClosed);
    };
    if root.find_splint(target).is_none() {
        return Ok(RefreshedCloseState::TargetClosed);
    }
    validate_exited_close_target(root, target, expected_incarnation)?;
    Ok(RefreshedCloseState::Retry)
}

async fn close_focused_splint(
    connection: &mut Connection,
    dojo_id: DojoId,
    root: &LayoutNode,
    expected_topology_revision: TopologyRevision,
    target: SplintId,
) -> Result<TopologyCommandOutcome> {
    let (mut close_revision, mut close_root, expected_incarnation) =
        match close_action(root, target)? {
            CloseAction::CloseExited => (expected_topology_revision, root.clone(), None),
            CloseAction::KillAndClose { incarnation } => {
                match connection
                    .request(Request::KillSplint {
                        splint_id: target,
                        incarnation,
                    })
                    .await?
                {
                    Response::SplintKilled {
                        splint_id,
                        incarnation: killed_incarnation,
                        ..
                    } if splint_id == target && killed_incarnation == incarnation => {}
                    response => bail!("splinterd returned unexpected kill response: {response:?}"),
                }
                let (revision, refreshed_root) = inspect_dojo_state(connection, dojo_id).await?;
                (revision, refreshed_root, Some(incarnation))
            }
        };

    for attempt in 0..=MAX_CLOSE_TOPOLOGY_RETRIES {
        let final_leaf = validate_exited_close_target(&close_root, target, expected_incarnation)?;
        match connection
            .request(Request::CloseSplint {
                expected_topology_revision: close_revision,
                splint_id: target,
            })
            .await
        {
            Ok(Response::TopologyCommitted { .. }) if final_leaf => {
                return Ok(TopologyCommandOutcome::WindowClosed);
            }
            Ok(Response::TopologyCommitted { .. }) => {
                return Ok(TopologyCommandOutcome::Updated);
            }
            Ok(response) => {
                bail!("splinterd returned unexpected close response: {response:?}");
            }
            Err(error)
                if protocol_error(&error)
                    .is_some_and(|failure| failure.code == ErrorCode::StaleTopology)
                    && attempt < MAX_CLOSE_TOPOLOGY_RETRIES =>
            {
                let (revision, refreshed_root) =
                    inspect_optional_dojo_state(connection, dojo_id).await?;
                match refreshed_close_state(refreshed_root.as_ref(), target, expected_incarnation)?
                {
                    RefreshedCloseState::WindowClosed => {
                        return Ok(TopologyCommandOutcome::WindowClosed);
                    }
                    RefreshedCloseState::TargetClosed => {
                        return Ok(TopologyCommandOutcome::Updated);
                    }
                    RefreshedCloseState::Retry => {}
                }
                close_revision = revision;
                close_root = refreshed_root.context("close retry Dojo disappeared")?;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("bounded close retry loop returns on its final attempt")
}

async fn apply_topology_command(
    connection: &mut Connection,
    config: &AppConfig,
    dojo_id: DojoId,
    root: &LayoutNode,
    expected_topology_revision: TopologyRevision,
    command: WindowTopologyCommand,
) -> Result<TopologyCommandOutcome> {
    if let WindowTopologyCommand::Close { target } = command {
        return close_focused_splint(
            connection,
            dojo_id,
            root,
            expected_topology_revision,
            target,
        )
        .await;
    }
    let request = match command {
        WindowTopologyCommand::Split { target, axis } => Request::SplitSplint {
            expected_topology_revision,
            target_splint_id: target,
            axis,
            side: SplitSide::Second,
            ratio: SplitRatio::new(500).expect("fixed split ratio is valid"),
            launch: launch_parameters(
                env::current_dir().context("failed to read current directory")?,
                Vec::new(),
                config,
            ),
        },
        WindowTopologyCommand::AdjustRatio { target, delta } => {
            let current = i32::from(
                parent_ratio(root, target)
                    .context("focused pane has no adjustable parent ratio")?
                    .get(),
            );
            let next = u16::try_from((current + i32::from(delta)).clamp(1, 999))?;
            Request::SetSplitRatio {
                expected_topology_revision,
                target_splint_id: target,
                ratio: SplitRatio::new(next).map_err(|_| anyhow::anyhow!("invalid ratio"))?,
            }
        }
        WindowTopologyCommand::Close { .. } => unreachable!("close handled above"),
        WindowTopologyCommand::RequestSessionPicker
        | WindowTopologyCommand::SwitchDojo { .. }
        | WindowTopologyCommand::NewDojo => {
            unreachable!("session commands are handled by the topology manager")
        }
    };
    match connection.request(request).await? {
        Response::TopologyCommitted { .. } | Response::SplintStarted { .. } => {
            Ok(TopologyCommandOutcome::Updated)
        }
        response => bail!("splinterd returned unexpected topology response: {response:?}"),
    }
}

fn topology_identity_diff(
    previous: &LayoutNode,
    current: &LayoutNode,
) -> (Vec<SplintId>, Vec<SplintId>) {
    let mut previous_ids = Vec::new();
    let mut current_ids = Vec::new();
    layout_splint_ids(previous, &mut previous_ids);
    layout_splint_ids(current, &mut current_ids);
    let previous = previous_ids
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    let current = current_ids
        .into_iter()
        .collect::<std::collections::HashSet<_>>();
    (
        current.difference(&previous).copied().collect(),
        previous.difference(&current).copied().collect(),
    )
}

async fn reconcile_window_topology(
    config: &AppConfig,
    image_cache: &SharedImageContentCache,
    root: &mut LayoutNode,
    next: LayoutNode,
    updates: &mpsc::Sender<WindowTopologyUpdate>,
    pane_tasks: &mut Vec<tokio::task::JoinHandle<Result<()>>>,
) -> Result<bool> {
    if *root == next {
        return Ok(true);
    }
    let (added_ids, removed) = topology_identity_diff(root, &next);
    let mut added = Vec::new();
    for splint_id in added_ids {
        let pane = prepare_live_pane(config, splint_id, image_cache.clone(), false).await?;
        pane_tasks.push(pane.task);
        added.push(pane.options);
    }
    if updates
        .send(WindowTopologyUpdate::Apply {
            layout: next.clone(),
            added,
            removed,
            focused: None,
            switched: false,
        })
        .await
        .is_err()
    {
        return Ok(false);
    }
    *root = next;
    Ok(true)
}

async fn session_picker_catalog(
    connection: &mut Connection,
) -> Result<(Vec<SessionPickerItem>, Vec<(LairId, DojoId)>)> {
    let Response::Lairs { lairs, .. } = connection.request(Request::ListLairs).await? else {
        bail!("splinterd did not return its session list");
    };
    let entries = collect_sessions(&lairs, &recent_dojo_ids())
        .into_iter()
        .filter(SessionEntry::reopenable)
        .collect::<Vec<_>>();
    let items = entries.iter().map(session_picker_item).collect();
    let targets = entries
        .iter()
        .map(|entry| (entry.lair_id, entry.dojo_id))
        .collect();
    Ok((items, targets))
}

async fn reopenable_dojo(
    connection: &mut Connection,
    lair_id: LairId,
    dojo_id: DojoId,
) -> Result<splinterm_core::Dojo> {
    let Response::Lairs { lairs, .. } = connection.request(Request::ListLairs).await? else {
        bail!("splinterd did not return its session list");
    };
    let dojo = select_dojo_from(&lairs, (lair_id, dojo_id))?;
    anyhow::ensure!(
        collect_sessions(&lairs, &[])
            .into_iter()
            .any(|entry| entry.dojo_id == dojo_id && entry.reopenable()),
        "selected session no longer has a fully running pane layout"
    );
    Ok(dojo)
}

async fn create_daily_dojo(
    connection: &mut Connection,
    config: &AppConfig,
) -> Result<splinterm_core::Dojo> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let expected = connection.topology_revision().await?;
    let Response::LairCreated { lair: dojo, .. } = connection
        .request(create_request(
            expected,
            format!("terminal-{stamp}-{}", std::process::id()),
            env::current_dir().context("failed to read current directory")?,
            Vec::new(),
            config,
        ))
        .await?
    else {
        bail!("splinterd did not create the requested terminal");
    };
    dojo.dojos
        .first()
        .cloned()
        .context("new Lair did not contain a Dojo")
}

async fn replace_managed_window(
    config: &AppConfig,
    image_cache: &SharedImageContentCache,
    dojo_id: &mut DojoId,
    root: &mut LayoutNode,
    target: splinterm_core::Dojo,
    updates: &mpsc::Sender<WindowTopologyUpdate>,
    pane_tasks: &mut Vec<tokio::task::JoinHandle<Result<()>>>,
) -> Result<bool> {
    if target.id == *dojo_id {
        remember_dojo(target.id);
        return Ok(updates
            .send(WindowTopologyUpdate::SessionSwitchComplete)
            .await
            .is_ok());
    }
    anyhow::ensure!(
        target.root.find_splint(target.default_focus).is_some(),
        "target Dojo focus is absent from its layout"
    );
    let mut ids = Vec::new();
    layout_splint_ids(&target.root, &mut ids);
    let mut prepared = Vec::with_capacity(ids.len());
    for splint_id in ids {
        match prepare_live_pane(
            config,
            splint_id,
            image_cache.clone(),
            pane_claims_initial_control(splint_id, target.default_focus),
        )
        .await
        {
            Ok(pane) => prepared.push(pane),
            Err(error) => {
                for pane in prepared {
                    pane.task.abort();
                }
                return Err(error);
            }
        }
    }
    let mut removed = Vec::new();
    layout_splint_ids(root, &mut removed);
    let mut added = Vec::with_capacity(prepared.len());
    let mut tasks = Vec::with_capacity(prepared.len());
    for pane in prepared {
        added.push(pane.options);
        tasks.push(pane.task);
    }
    if updates
        .send(WindowTopologyUpdate::Apply {
            layout: target.root.clone(),
            added,
            removed,
            focused: Some(target.default_focus),
            switched: true,
        })
        .await
        .is_err()
    {
        for task in tasks {
            task.abort();
        }
        return Ok(false);
    }
    pane_tasks.extend(tasks);
    *dojo_id = target.id;
    *root = target.root;
    remember_dojo(target.id);
    Ok(true)
}

enum TopologyManagerCommandOutcome {
    Continue,
    Stop,
    Edit(WindowTopologyCommand),
}

struct TopologyManagerState {
    dojo_id: DojoId,
    root: LayoutNode,
    pane_tasks: Vec<tokio::task::JoinHandle<Result<()>>>,
}

async fn finish_managed_window_switch(
    target: Result<splinterm_core::Dojo>,
    state: &mut TopologyManagerState,
    config: &AppConfig,
    image_cache: &SharedImageContentCache,
    updates: &mpsc::Sender<WindowTopologyUpdate>,
) -> TopologyManagerCommandOutcome {
    let result = match target {
        Ok(target) => {
            replace_managed_window(
                config,
                image_cache,
                &mut state.dojo_id,
                &mut state.root,
                target,
                updates,
                &mut state.pane_tasks,
            )
            .await
        }
        Err(error) => Err(error),
    };
    match result {
        Ok(true) => TopologyManagerCommandOutcome::Continue,
        Ok(false) => TopologyManagerCommandOutcome::Stop,
        Err(error) => {
            let _ = updates
                .send(WindowTopologyUpdate::SessionPickerFailed(format!(
                    "{error:#}"
                )))
                .await;
            TopologyManagerCommandOutcome::Continue
        }
    }
}

async fn handle_session_manager_command(
    command: WindowTopologyCommand,
    connection: &mut Connection,
    config: &AppConfig,
    image_cache: &SharedImageContentCache,
    updates: &mpsc::Sender<WindowTopologyUpdate>,
    state: &mut TopologyManagerState,
) -> TopologyManagerCommandOutcome {
    match command {
        WindowTopologyCommand::RequestSessionPicker => {
            match session_picker_catalog(connection).await {
                Ok((items, targets)) => {
                    if updates
                        .send(WindowTopologyUpdate::ShowSessionPicker { items, targets })
                        .await
                        .is_err()
                    {
                        return TopologyManagerCommandOutcome::Stop;
                    }
                }
                Err(error) => {
                    let _ = updates
                        .send(WindowTopologyUpdate::SessionPickerFailed(format!(
                            "{error:#}"
                        )))
                        .await;
                }
            }
            TopologyManagerCommandOutcome::Continue
        }
        WindowTopologyCommand::SwitchDojo {
            lair_id,
            dojo_id: target_id,
        } => {
            let target = reopenable_dojo(connection, lair_id, target_id).await;
            finish_managed_window_switch(target, state, config, image_cache, updates).await
        }
        WindowTopologyCommand::NewDojo => {
            let target = create_daily_dojo(connection, config).await;
            finish_managed_window_switch(target, state, config, image_cache, updates).await
        }
        command => TopologyManagerCommandOutcome::Edit(command),
    }
}

async fn run_topology_manager(
    config: AppConfig,
    image_cache: SharedImageContentCache,
    dojo_id: DojoId,
    root: LayoutNode,
    mut commands: mpsc::Receiver<WindowTopologyCommand>,
    updates: mpsc::Sender<WindowTopologyUpdate>,
    pane_tasks: Vec<tokio::task::JoinHandle<Result<()>>>,
) -> Result<()> {
    let mut connection = Connection::connect().await?;
    let mut poll = tokio::time::interval(std::time::Duration::from_millis(250));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut state = TopologyManagerState {
        dojo_id,
        root,
        pane_tasks,
    };
    loop {
        let command = tokio::select! {
            command = commands.recv() => command,
            _ = poll.tick() => None,
        };
        let command = if let Some(command) = command {
            match handle_session_manager_command(
                command,
                &mut connection,
                &config,
                &image_cache,
                &updates,
                &mut state,
            )
            .await
            {
                TopologyManagerCommandOutcome::Continue => continue,
                TopologyManagerCommandOutcome::Stop => break,
                TopologyManagerCommandOutcome::Edit(command) => Some(command),
            }
        } else {
            None
        };
        let (revision, authoritative) =
            match inspect_dojo_state(&mut connection, state.dojo_id).await {
                Ok(state) => state,
                Err(error) => {
                    let _ = updates
                        .send(WindowTopologyUpdate::Shutdown(format!("{error:#}")))
                        .await;
                    return Err(error);
                }
            };
        let reconciled = match reconcile_window_topology(
            &config,
            &image_cache,
            &mut state.root,
            authoritative,
            &updates,
            &mut state.pane_tasks,
        )
        .await
        {
            Ok(reconciled) => reconciled,
            Err(error) => {
                let _ = updates
                    .send(WindowTopologyUpdate::Shutdown(format!("{error:#}")))
                    .await;
                return Err(error);
            }
        };
        if !reconciled {
            break;
        }
        let Some(command) = command else {
            if commands.is_closed() {
                break;
            }
            continue;
        };
        match apply_topology_command(
            &mut connection,
            &config,
            state.dojo_id,
            &state.root,
            revision,
            command,
        )
        .await
        {
            Ok(TopologyCommandOutcome::Updated) => {}
            Ok(TopologyCommandOutcome::WindowClosed) => {
                let _ = updates.send(WindowTopologyUpdate::Closed).await;
                break;
            }
            Err(error) => eprintln!("splinterm topology edit rejected: {error:#}"),
        }
    }
    for task in state.pane_tasks {
        task.await.context("pane subscription task failed")??;
    }
    Ok(())
}

fn pane_chrome_capture() -> Result<Option<PathBuf>> {
    let Some(path) = env::var_os("SPLINTERM_PANE_CHROME_CAPTURE") else {
        return Ok(None);
    };
    anyhow::ensure!(
        env::var_os("SPLINTERM_ENABLE_DEV_ATTACH").is_some(),
        "SPLINTERM_PANE_CHROME_CAPTURE requires development attach"
    );
    Ok(Some(PathBuf::from(path)))
}

fn spawn_topology_smoke(
    commands: mpsc::Sender<WindowTopologyCommand>,
    target: SplintId,
) -> Result<Option<tokio::task::JoinHandle<Result<()>>>> {
    if env::var_os("SPLINTERM_TOPOLOGY_SMOKE").is_none() {
        return Ok(None);
    }
    anyhow::ensure!(
        env::var_os("SPLINTERM_ENABLE_DEV_ATTACH").is_some(),
        "SPLINTERM_TOPOLOGY_SMOKE requires development attach"
    );
    Ok(Some(tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;
        commands
            .send(WindowTopologyCommand::Split {
                target,
                axis: Axis::Horizontal,
            })
            .await
            .map_err(|_| anyhow::anyhow!("topology smoke split channel closed"))?;
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;
        commands
            .send(WindowTopologyCommand::AdjustRatio { target, delta: 100 })
            .await
            .map_err(|_| anyhow::anyhow!("topology smoke ratio channel closed"))
    })))
}

async fn run_live_multipane_window(
    config: AppConfig,
    dojo_model: splinterm_core::Dojo,
) -> Result<()> {
    let theme = load_startup_theme(&config);
    renderer::configure(RendererOptions {
        font: config.font.clone(),
        font_size: config.font_size,
        font_sizing_policy: config.font_sizing_policy,
        physical_dpi: 96.0,
        padding: config.padding,
        background_alpha: theme.background_alpha,
    })?;
    let mut ids = Vec::new();
    layout_splint_ids(&dojo_model.root, &mut ids);
    let image_cache =
        SharedImageContentCache::with_maximum_bytes(MAX_RENDERER_IMAGE_RESIDENT_BYTES)?;
    let mut prepared = Vec::with_capacity(ids.len());
    for splint_id in ids {
        prepared.push(
            prepare_live_pane(
                &config,
                splint_id,
                image_cache.clone(),
                pane_claims_initial_control(splint_id, dojo_model.default_focus),
            )
            .await?,
        );
    }
    let (topology_commands, topology_command_receiver) = mpsc::channel(8);
    let (topology_update_sender, topology_updates) = mpsc::channel(4);
    let theme_task = tokio::spawn(watch_theme(
        config.theme_source(),
        config.background_alpha,
        config.background_blur,
        theme,
        ThemeUpdateSink::Topology(topology_update_sender.clone()),
    ));
    let mut panes = Vec::with_capacity(prepared.len());
    let mut tasks = Vec::with_capacity(prepared.len());
    for pane in prepared {
        panes.push(pane.options);
        tasks.push(pane.task);
    }
    let topology_smoke = spawn_topology_smoke(topology_commands.clone(), dojo_model.default_focus)?;
    let window_config = config.clone();
    let root = dojo_model.root;
    let manager_root = root.clone();
    let active_splint = dojo_model.default_focus;
    let dojo_id = dojo_model.id;
    let topology_manager = tokio::spawn(run_topology_manager(
        config,
        image_cache,
        dojo_id,
        manager_root,
        topology_command_receiver,
        topology_update_sender,
        tasks,
    ));
    let result = tokio::task::spawn_blocking(move || {
        run_window(WindowOptions {
            capture: pane_chrome_capture()?,
            panes,
            layout: Some(root),
            active_splint: Some(active_splint),
            topology_updates: Some(topology_updates),
            topology_commands: Some(topology_commands),
            initial_columns: window_config.initial_columns,
            initial_rows: window_config.initial_rows,
            cursor_style: window_config.cursor_style,
            cursor_blink: window_config.cursor_blink,
            title: window_config.title,
            theme,
            pane_divider_style: window_config.pane_divider_style,
            frame_title_mode: window_config.frame_title_mode,
            ..WindowOptions::default()
        })
    })
    .await
    .context("Wayland window task failed")?;
    theme_task.abort();
    if let Some(smoke) = topology_smoke {
        smoke.await.context("topology smoke task failed")??;
    }
    topology_manager
        .await
        .context("topology manager task failed")??;
    result
}

#[allow(
    clippy::too_many_lines,
    reason = "subscription resync, controller ownership, and window task shutdown are one lifecycle"
)]
async fn run_live_window(config: AppConfig, splint_id: SplintId) -> Result<()> {
    let theme = load_startup_theme(&config);
    renderer::configure(RendererOptions {
        font: config.font.clone(),
        font_size: config.font_size,
        font_sizing_policy: config.font_sizing_policy,
        physical_dpi: 96.0,
        padding: config.padding,
        background_alpha: theme.background_alpha,
    })?;
    let mut connection = Connection::connect().await?;
    let incarnation = connection.live_incarnation(splint_id).await?;
    let requested_scopes = vec![
        AccessScope::Observe,
        AccessScope::Scrollback,
        AccessScope::Input,
        AccessScope::Resize,
    ];
    if !matches!(
        connection
            .request(Request::RequestAccess {
                splint_id,
                incarnation,
                scopes: requested_scopes,
            })
            .await?,
        Response::AccessGranted { .. }
    ) {
        bail!("splinterd did not grant requested terminal access");
    }
    let authority = load_authority_status(&mut connection, splint_id, incarnation).await?;
    let mut attachment = attach(&mut connection, splint_id, incarnation).await?;
    let image_cache =
        SharedImageContentCache::with_maximum_bytes(MAX_RENDERER_IMAGE_RESIDENT_BYTES)?;
    resolve_image_contents(&mut connection, &attachment.snapshot, &image_cache).await?;
    let initial_image_sources = lease_snapshot_images(&image_cache, &attachment.snapshot)?;
    let mut control = Connection::connect().await?;
    let control_incarnation = control.live_incarnation(splint_id).await?;
    if control_incarnation != incarnation {
        bail!("control connection observed a different process incarnation");
    }
    let controller_id = control
        .acquire_control(
            splint_id,
            incarnation,
            vec![ControlMode::Input, ControlMode::Resize],
        )
        .await?;
    println!("Controller lease {controller_id} granted for live Splint");
    let (updates, receiver) = mpsc::channel(WINDOW_UPDATE_QUEUE);
    let _theme_watcher = tokio::spawn(watch_theme(
        config.theme_source(),
        config.background_alpha,
        config.background_blur,
        theme,
        ThemeUpdateSink::Panes(vec![updates.clone()]),
    ));
    let (command_sender, commands) = mpsc::channel(WINDOW_COMMAND_QUEUE);
    let (resync_sender, mut resyncs) = mpsc::channel(1);
    let mut controller = tokio::spawn(run_controller(
        control,
        commands,
        ControllerOutputs {
            updates: updates.clone(),
            resyncs: resync_sender,
        },
        Some(controller_id),
        splint_id,
        incarnation,
        config.resize_delay_ms,
    ));
    let mut last_revision = attachment.snapshot.revision;
    let initial_snapshot = attachment.snapshot;
    let window_config = config.clone();
    let mut window = tokio::task::spawn_blocking(move || {
        run_window(WindowOptions {
            snapshot: Some(initial_snapshot),
            image_sources: initial_image_sources,
            updates: Some(receiver),
            commands: Some(command_sender),
            authority,
            initial_columns: window_config.initial_columns,
            initial_rows: window_config.initial_rows,
            cursor_style: window_config.cursor_style,
            cursor_blink: window_config.cursor_blink,
            title: window_config.title,
            theme,
            pane_divider_style: window_config.pane_divider_style,
            frame_title_mode: window_config.frame_title_mode,
            ..WindowOptions::default()
        })
    });
    let mut last_sequence = 0_u64;

    loop {
        tokio::select! {
            biased;
            result = &mut window => {
                // Dropping the window closes its command sender. The controller task then
                // releases the lease explicitly; connection teardown is the fallback.
                let window_result = result.context("Wayland window task failed")?;
                controller
                    .await
                    .context("window controller task failed")??;
                return window_result;
            }
            result = &mut controller => {
                let controller_result = result.context("window controller task failed")?;
                let _ = updates.send(WindowUpdate::Shutdown).await;
                let window_result = window.await.context("Wayland window task failed")?;
                controller_result?;
                return window_result;
            }
            Some(()) = resyncs.recv() => {
                if std::env::var_os("SPLINTERM_SCROLL_TRACE").is_some() {
                    eprintln!("scroll-trace resync=controller_page");
                }
                attachment = resynchronize(
                    &mut connection,
                    attachment.subscription_id,
                    splint_id,
                    incarnation,
                ).await?;
                resolve_image_contents(&mut connection, &attachment.snapshot, &image_cache).await?;
                let image_sources = lease_snapshot_images(&image_cache, &attachment.snapshot)?;
                if updates
                    .send(WindowUpdate::Snapshot {
                        snapshot: attachment.snapshot.clone(),
                        image_sources,
                    })
                    .await
                    .is_err()
                {
                    let window_result = window.await.context("Wayland window task failed")?;
                    controller.await.context("window controller task failed")??;
                    return window_result;
                }
                last_revision = attachment.snapshot.revision;
                last_sequence = 0;
            }
            frame = connection.next_server_frame() => {
                match frame? {
                    ServerFrame::Event {
                        subscription_id,
                        sequence,
                        event,
                    } => match classify_subscription_event(
                        attachment.subscription_id,
                        last_sequence,
                        subscription_id,
                        sequence,
                        event,
                    ) {
                        EventAction::Ignore => {}
                        EventAction::Snapshot { sequence, snapshot } => {
                            validate_attached_snapshot(&snapshot, splint_id, incarnation)?;
                            last_revision = snapshot.revision;
                            resolve_image_contents(&mut connection, &snapshot, &image_cache).await?;
                            let image_sources = lease_snapshot_images(&image_cache, &snapshot)?;
                            if updates.send(WindowUpdate::Snapshot { snapshot, image_sources }).await.is_err() {
                                let window_result = window.await.context("Wayland window task failed")?;
                                controller.await.context("window controller task failed")??;
                                return window_result;
                            }
                            last_sequence = sequence;
                        }
                        EventAction::Update { sequence, update }
                            if update_advances_from(&update, last_revision) => {
                            last_revision = update.revision;
                            resolve_update_images(
                                &mut connection,
                                &update,
                                splint_id,
                                incarnation,
                                &image_cache,
                            ).await?;
                            let image_sources = lease_update_images(&image_cache, &update)?;
                            if updates.send(WindowUpdate::Update { update, image_sources }).await.is_err() {
                                let window_result = window.await.context("Wayland window task failed")?;
                                controller.await.context("window controller task failed")??;
                                return window_result;
                            }
                            last_sequence = sequence;
                        }
                        EventAction::Update { update, .. } => {
                            if std::env::var_os("SPLINTERM_SCROLL_TRACE").is_some() {
                                eprintln!(
                                    "scroll-trace resync=revision last={} base={} final={}",
                                    last_revision, update.base_revision, update.revision
                                );
                            }
                            attachment = resynchronize(
                                &mut connection,
                                attachment.subscription_id,
                                splint_id,
                                incarnation,
                            ).await?;
                            resolve_image_contents(&mut connection, &attachment.snapshot, &image_cache).await?;
                            let image_sources = lease_snapshot_images(&image_cache, &attachment.snapshot)?;
                            if updates
                                .send(WindowUpdate::Snapshot {
                                    snapshot: attachment.snapshot.clone(),
                                    image_sources,
                                })
                                .await
                                .is_err()
                            {
                                let window_result = window.await.context("Wayland window task failed")?;
                                controller.await.context("window controller task failed")??;
                                return window_result;
                            }
                            last_revision = attachment.snapshot.revision;
                            last_sequence = 0;
                        }
                        EventAction::Resynchronize => {
                            if std::env::var_os("SPLINTERM_SCROLL_TRACE").is_some() {
                                eprintln!(
                                    "scroll-trace resync=subscription_sequence last_sequence={last_sequence} received_sequence={sequence}"
                                );
                            }
                            attachment = resynchronize(
                                &mut connection,
                                attachment.subscription_id,
                                splint_id,
                                incarnation,
                            ).await?;
                            resolve_image_contents(&mut connection, &attachment.snapshot, &image_cache).await?;
                            let image_sources = lease_snapshot_images(&image_cache, &attachment.snapshot)?;
                            if updates
                                .send(WindowUpdate::Snapshot {
                                    snapshot: attachment.snapshot.clone(),
                                    image_sources,
                                })
                                .await
                                .is_err()
                            {
                                let window_result = window.await.context("Wayland window task failed")?;
                                controller.await.context("window controller task failed")??;
                                return window_result;
                            }
                            last_revision = attachment.snapshot.revision;
                            last_sequence = 0;
                        }
                        EventAction::Exited | EventAction::Shutdown => {
                            let _ = updates.send(WindowUpdate::Shutdown).await;
                            let window_result = window.await.context("Wayland window task failed")?;
                            controller
                                .await
                                .context("window controller task failed")??;
                            return window_result;
                        }
                    },
                    ServerFrame::Error { error, .. } => {
                        bail!("splinterd: {}", error.message);
                    }
                    _ => bail!("splinterd sent an unexpected frame while subscribed"),
                }
            }
        }
    }
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "response rendering retains a fallible CLI boundary for future output modes"
)]
fn print_restore_results(
    topology_revision: TopologyRevision,
    results: Vec<splinterm_protocol::RestoreLeafResult>,
) {
    println!(
        "Restore completed at topology revision {}.",
        topology_revision.get()
    );
    for result in results {
        match (result.incarnation, result.error) {
            (Some(incarnation), None) => {
                println!(
                    "  {}: started as incarnation {incarnation}",
                    result.splint_id
                );
            }
            (_, Some(error)) => {
                println!("  {}: failed: {}", result.splint_id, error.message);
            }
            _ => println!("  {}: failed without a result", result.splint_id),
        }
    }
}

fn node_has_active_splint(node: &splinterm_core::LayoutNode) -> bool {
    match node {
        splinterm_core::LayoutNode::Leaf(splint) => !matches!(splint.state, SplintState::Exited(_)),
        splinterm_core::LayoutNode::Branch { first, second, .. } => {
            node_has_active_splint(first) || node_has_active_splint(second)
        }
    }
}

fn lair_has_active_splint(dojo: &splinterm_core::Lair) -> bool {
    dojo.dojos
        .iter()
        .any(|dojo| node_has_active_splint(&dojo.root))
}

fn render_active_splint_ids(node: &splinterm_core::LayoutNode, output: &mut String) {
    match node {
        splinterm_core::LayoutNode::Leaf(splint)
            if !matches!(splint.state, SplintState::Exited(_)) =>
        {
            let state = match splint.state {
                SplintState::Starting => "starting",
                SplintState::Running => "running",
                SplintState::Exited(_) => unreachable!("exited Splints are filtered"),
            };
            writeln!(output, "    {state:<8} {}  {}", splint.title, splint.id)
                .expect("writing to String cannot fail");
        }
        splinterm_core::LayoutNode::Leaf(_) => {}
        splinterm_core::LayoutNode::Branch { first, second, .. } => {
            render_active_splint_ids(first, output);
            render_active_splint_ids(second, output);
        }
    }
}

fn render_all_splint_ids(node: &splinterm_core::LayoutNode, output: &mut String) {
    match node {
        splinterm_core::LayoutNode::Leaf(splint) => {
            writeln!(
                output,
                "  {}  {}  {:?}",
                splint.id, splint.title, splint.state
            )
            .expect("writing to String cannot fail");
        }
        splinterm_core::LayoutNode::Branch { first, second, .. } => {
            render_all_splint_ids(first, output);
            render_all_splint_ids(second, output);
        }
    }
}

fn render_lairs(lairs: &[splinterm_core::Lair], all: bool) -> String {
    let mut output = String::new();
    if all {
        if lairs.is_empty() {
            return "No Lairs.\n".to_owned();
        }
        for lair in lairs {
            let splints: usize = lair.dojos.iter().map(|dojo| dojo.root.splint_count()).sum();
            writeln!(
                output,
                "{}  {}  {} Dojo(s)  {splints} Splint(s)",
                lair.id,
                lair.name,
                lair.dojos.len()
            )
            .expect("writing to String cannot fail");
            for dojo in &lair.dojos {
                writeln!(
                    output,
                    "  Dojo {}  {}  default-focus {}",
                    dojo.id, dojo.name, dojo.default_focus
                )
                .expect("writing to String cannot fail");
                render_all_splint_ids(&dojo.root, &mut output);
            }
        }
        return output;
    }

    let active: Vec<_> = lairs
        .iter()
        .filter(|lair| lair_has_active_splint(lair))
        .collect();
    let hidden = lairs.len().saturating_sub(active.len());

    if active.is_empty() {
        writeln!(output, "No active Lairs.").expect("writing to String cannot fail");
    } else {
        writeln!(
            output,
            "Active Lair{} ({})",
            if active.len() == 1 { "" } else { "s" },
            active.len()
        )
        .expect("writing to String cannot fail");
        for lair in active {
            writeln!(output, "\n{}", lair.name).expect("writing to String cannot fail");
            writeln!(output, "  Lair    {}", lair.id).expect("writing to String cannot fail");
            for dojo in &lair.dojos {
                if !node_has_active_splint(&dojo.root) {
                    continue;
                }
                writeln!(output, "  Dojo    {}  {}", dojo.name, dojo.id)
                    .expect("writing to String cannot fail");
                render_active_splint_ids(&dojo.root, &mut output);
            }
        }
        writeln!(
            output,
            "\nStop one running Splint with: splinterm kill <SPLINT_ID>"
        )
        .expect("writing to String cannot fail");
    }

    if hidden > 0 {
        writeln!(
            output,
            "{hidden} inactive Lair{} hidden.",
            if hidden == 1 { "" } else { "s" },
        )
        .expect("writing to String cannot fail");
    }
    writeln!(output, "Show complete history with: splinterm list --all")
        .expect("writing to String cannot fail");
    output
}

fn print_lairs(lairs: &[splinterm_core::Lair], all: bool) {
    print!("{}", render_lairs(lairs, all));
}

#[allow(
    clippy::too_many_lines,
    reason = "the CLI keeps exhaustive human rendering for every private protocol response"
)]
fn print_response(response: Response) -> Result<()> {
    match response {
        Response::Pong => println!("splinterd is awake"),
        Response::MutationPrepared { .. } => {
            println!("Mutation preflight prepared.");
        }
        Response::Lairs { lairs, .. } => print_lairs(&lairs, true),
        Response::LairCreated { lair, .. } => println!("Created Lair '{}'.", lair.name),
        Response::Topology { snapshot } => println!(
            "Topology revision {}: {} Lair(s), {} Splint(s)",
            snapshot.revision.get(),
            snapshot.topology.lairs().count(),
            snapshot.runtimes.len()
        ),
        Response::TopologySubscribed {
            subscription_id,
            snapshot,
        } => println!(
            "Topology subscription {subscription_id} started at revision {}.",
            snapshot.revision.get()
        ),
        Response::Splint { runtime, .. } => println!(
            "Splint {:?}: {:?}, incarnation={:?}, exit={:?}",
            runtime.splint_id, runtime.lifecycle, runtime.live_incarnation, runtime.exit_status
        ),
        Response::Attached { snapshot, .. } => print_snapshot(&snapshot),
        Response::ImageContentReady { transfer } => println!(
            "Image content {} generation {} ready via {:?} ({} bytes).",
            transfer.content_id, transfer.generation, transfer.transfer, transfer.byte_length
        ),
        Response::ScrollbackPage { page, .. } => println!(
            "Scrollback page: {} row(s), has_older={}",
            page.rows.len(),
            page.has_older
        ),
        Response::ScrollbackResyncRequired {
            current_revision,
            history_generation,
            ..
        } => println!(
            "Scrollback resync required at revision {current_revision}, generation {history_generation}"
        ),
        Response::SearchResults { page, .. } => println!(
            "Search page: {} match(es), continuation={}, timed_out={}",
            page.matches.len(),
            page.next_cursor.is_some(),
            page.timed_out,
        ),
        Response::SearchResyncRequired {
            current_revision,
            history_generation,
            ..
        } => println!(
            "Search resync required at revision {current_revision}, generation {history_generation}"
        ),
        Response::AccessGranted { grant, .. } => {
            println!("Access grant {} issued.", grant.grant_id);
        }
        Response::AccessRevoked { grant, .. } => {
            println!("Access grant {} revoked.", grant.grant_id);
        }
        Response::AuthorizationStatus {
            grants,
            persistent,
            development_bypass,
            ..
        } => {
            println!(
                "{} active grant(s), {} persistent rule(s); development bypass={development_bypass}",
                grants.len(),
                persistent.len()
            );
        }
        Response::ControlGranted { controller_id, .. } => {
            println!("Controller lease {controller_id} granted.");
        }
        Response::ControlSubscribed {
            subscription_id,
            status,
        } => println!(
            "Control subscription {subscription_id}: controlled={}, locally_owned={}",
            status.controlled, status.locally_owned,
        ),
        Response::ControlTransferPending { transfer_id, .. } => {
            println!("Control transfer {transfer_id} pending.");
        }
        Response::ControlTransferDecided { outcome, .. } => {
            println!("Control transfer {outcome:?}.");
        }
        Response::AuditPage { page } => println!(
            "Audit page: {} record(s), retention_gap={}, newest={:?}.",
            page.records.len(),
            page.retention_gap,
            page.newest_available_audit_id
        ),
        Response::TerminalActionAcknowledged {
            splint_id,
            incarnation,
            terminal_revision,
            ..
        } => println!(
            "Splint {splint_id} incarnation {incarnation} acknowledged at terminal revision {terminal_revision}."
        ),
        Response::Acknowledged => println!("Acknowledged."),
        Response::SplintStarted {
            splint_id,
            incarnation,
            topology_revision,
        } => println!(
            "Splint {splint_id} started as incarnation {incarnation} at topology revision {}.",
            topology_revision.get()
        ),
        Response::DojoStarted {
            dojo_id,
            splint_id,
            incarnation,
            topology_revision,
        } => println!(
            "Dojo {dojo_id:?} started with Splint {splint_id} incarnation {incarnation} at revision {}.",
            topology_revision.get()
        ),
        Response::TopologyCommitted { topology_revision } => {
            println!("Topology revision {} committed.", topology_revision.get());
        }
        Response::RestoreCompleted {
            topology_revision,
            results,
        } => print_restore_results(topology_revision, results),
        Response::SplintKilled {
            splint_id,
            incarnation,
            exit_status,
        } => println!(
            "Splint {splint_id} incarnation {incarnation} exited (code={:?}, signal={:?}).",
            exit_status.code, exit_status.signal
        ),
    }
    io::stdout()
        .flush()
        .context("failed to flush command output")
}

fn print_snapshot(snapshot: &TerminalSnapshot) {
    println!(
        "Splint {:?} · incarnation {} · revision {} · {}x{}",
        snapshot.splint_id,
        snapshot.incarnation,
        snapshot.revision,
        snapshot.columns,
        snapshot.rows
    );
    for row in &snapshot.visible_rows {
        let line: String = row
            .cells
            .iter()
            .map(|cell| {
                if cell.content.is_empty() {
                    " "
                } else {
                    &cell.content
                }
            })
            .collect();
        println!("{}", line.trim_end());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use splinterm_protocol::{ActiveScreen, TerminalInputModes};

    #[test]
    fn reset_requires_explicit_confirmation_for_unattended_use() {
        let guarded = Cli::try_parse_from(["splinterm", "reset"]).unwrap();
        assert!(matches!(guarded.command, Command::Reset { yes: false }));

        let confirmed = Cli::try_parse_from(["splinterm", "reset", "--yes"]).unwrap();
        assert!(matches!(confirmed.command, Command::Reset { yes: true }));
    }

    #[test]
    fn session_reset_moves_state_to_a_reversible_backup() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "splinterm-reset-test-{}-{nonce}",
            std::process::id()
        ));
        let state = root.join("splinterm");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join("lair.json"), b"current").unwrap();
        std::fs::write(state.join("lair.json.previous"), b"previous").unwrap();

        let backup = backup_session_state(&state).unwrap().unwrap();
        assert!(!state.exists());
        assert_eq!(std::fs::read(backup.join("lair.json")).unwrap(), b"current");
        assert_eq!(
            std::fs::read(backup.join("lair.json.previous")).unwrap(),
            b"previous"
        );
        assert!(backup_session_state(&state).unwrap().is_none());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_backup_never_replaces_existing_or_dangling_destinations() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "splinterm-reset-collision-test-{}-{nonce}",
            std::process::id()
        ));
        let state = root.join("splinterm");
        let backup = root.join("splinterm.reset-collision");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join("lair.json"), b"sessions").unwrap();
        std::fs::write(&backup, b"existing").unwrap();

        assert!(!move_session_state_no_replace(&state, &backup).unwrap());
        assert!(state.exists());
        assert_eq!(std::fs::read(&backup).unwrap(), b"existing");

        std::fs::remove_file(&backup).unwrap();
        std::os::unix::fs::symlink(root.join("missing-target"), &backup).unwrap();
        assert!(!move_session_state_no_replace(&state, &backup).unwrap());
        assert!(state.exists());
        assert!(std::fs::symlink_metadata(&backup).unwrap().is_symlink());

        std::fs::remove_file(&backup).unwrap();
        assert!(move_session_state_no_replace(&state, &backup).unwrap());
        assert!(!state.exists());
        assert_eq!(
            std::fs::read(backup.join("lair.json")).unwrap(),
            b"sessions"
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn readiness_failure_context_preserves_partial_reset_backup() {
        let backup = Path::new("/tmp/splinterm.reset-example");
        let message = reset_readiness_context(Some(backup));
        assert!(message.contains("sessions were reset"));
        assert!(message.contains(backup.to_string_lossy().as_ref()));
        assert!(message.contains("did not become ready"));
    }

    #[test]
    fn graphical_session_commands_are_explicit() {
        let sessions = Cli::try_parse_from(["splinterm", "sessions"]).unwrap();
        assert!(matches!(sessions.command, Command::Sessions));
        let reopen = Cli::try_parse_from(["splinterm", "reopen"]).unwrap();
        assert!(matches!(reopen.command, Command::Reopen));
    }

    #[test]
    fn list_defaults_to_active_lairs_and_all_is_explicit() {
        let active = Cli::try_parse_from(["splinterm", "list"]).unwrap();
        assert!(matches!(active.command, Command::List { all: false }));

        let all = Cli::try_parse_from(["splinterm", "list", "--all"]).unwrap();
        assert!(matches!(all.command, Command::List { all: true }));
    }

    #[test]
    fn human_list_hides_inactive_lairs_and_all_retains_complete_detail() {
        let active = splinterm_core::Lair::new("active", PathBuf::from("/tmp"));
        let active_lair_id = active.id.to_string();
        let active_dojo_id = active.dojos[0].id.to_string();
        let active_splint_id = active.dojos[0].root.first_splint_id().to_string();
        assert!(lair_has_active_splint(&active));

        let mut inactive = splinterm_core::Lair::new("inactive", PathBuf::from("/tmp"));
        let inactive_id = inactive.id.to_string();
        let splinterm_core::LayoutNode::Leaf(splint) = &mut inactive.dojos[0].root else {
            panic!("new Dojo should contain one Splint")
        };
        splint.state = SplintState::Exited(0);
        assert!(!lair_has_active_splint(&inactive));

        let lairs = vec![active, inactive];
        let concise = render_lairs(&lairs, false);
        assert!(concise.contains("Active Lair (1)"));
        assert!(concise.contains("\nactive\n"));
        assert!(concise.contains(&active_lair_id));
        assert!(concise.contains(&active_dojo_id));
        assert!(concise.contains(&active_splint_id));
        assert!(!concise.contains("\ninactive\n"));
        assert!(concise.contains("1 inactive Lair hidden."));
        assert!(concise.contains("Stop one running Splint with:"));
        assert!(concise.contains("Show complete history with: splinterm list --all"));

        let all = render_lairs(&lairs, true);
        assert!(all.contains(" active "));
        assert!(all.contains(" inactive "));
        assert!(all.contains(&inactive_id));
        assert!(all.contains("default-focus"));
        assert!(all.contains("Exited(0)"));
    }

    #[test]
    fn empty_human_list_still_guides_complete_history() {
        let output = render_lairs(&[], false);
        assert_eq!(
            output,
            "No active Lairs.\nShow complete history with: splinterm list --all\n"
        );
        assert_eq!(render_lairs(&[], true), "No Lairs.\n");
    }

    #[test]
    fn native_theme_fingerprint_tracks_omarchy_atomic_directory_replacement() {
        let root = std::env::temp_dir().join(format!(
            "splinterm-omarchy-theme-fingerprint-{}",
            std::process::id()
        ));
        let theme = root.join("theme");
        let previous = root.join("previous");
        std::fs::create_dir_all(&theme).unwrap();
        std::fs::write(theme.join("colors.toml"), b"accent = \"#010203\"\n").unwrap();
        std::fs::write(theme.join("foot.ini"), b"[colors-dark]\n").unwrap();
        let source = ThemeSource::Omarchy(theme.clone());
        let first = theme_source_fingerprint(&source);

        std::fs::rename(&theme, &previous).unwrap();
        assert!(resolve_live_theme_update(&source, None, None, ResolvedTheme::default()).is_err());
        std::fs::create_dir(&theme).unwrap();
        std::fs::write(theme.join("colors.toml"), b"accent = \"#010203\"\n").unwrap();
        std::fs::write(theme.join("foot.ini"), b"[colors-dark]\n").unwrap();
        assert_ne!(theme_source_fingerprint(&source), first);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn theme_fingerprint_changes_without_parsing_unchanged_content() {
        let path = std::env::temp_dir().join(format!(
            "splinterm-theme-fingerprint-{}",
            std::process::id()
        ));
        std::fs::write(&path, b"one").unwrap();
        let first = theme_file_fingerprint(&path).unwrap();
        assert_eq!(theme_file_fingerprint(&path), Some(first));
        std::fs::write(&path, b"different-length").unwrap();
        assert_ne!(theme_file_fingerprint(&path), Some(first));
        std::fs::remove_file(path).unwrap();
    }

    fn valid_theme_json(alpha: f32, blur: bool) -> String {
        format!(
            r##"{{"background":"#000001","foreground":"#000003","cursor":"#000003","selection":"#000004","url":"#000005","ui_accent":"#000006","alpha":{alpha},"blur":{blur},"ansi":["#000000","#000001","#000002","#000003","#000004","#000005","#000006","#000007","#000008","#000009","#00000a","#00000b","#00000c","#00000d","#00000e","#00000f"]}}"##
        )
    }

    #[test]
    fn malformed_startup_and_live_themes_preserve_bounded_safe_state() {
        let path =
            std::env::temp_dir().join(format!("splinterm-theme-resolution-{}", std::process::id()));
        std::fs::write(&path, b"{}").unwrap();

        let mut config = AppConfig {
            theme_path: Some(path.clone()),
            background_alpha: Some(1234),
            background_blur: Some(true),
            ..AppConfig::default()
        };
        for _launch_path in ["single-pane", "multi-pane"] {
            let mut startup_diagnostics = Vec::new();
            let startup = load_startup_theme_with_reporter(&config, |diagnostic| {
                startup_diagnostics.push(diagnostic.to_owned());
            });
            assert_eq!(startup_diagnostics.len(), 1);
            assert_eq!(startup.background_alpha, 1234);
            assert!(startup.background_blur);
        }

        std::fs::write(&path, valid_theme_json(0.25, true)).unwrap();
        config.background_alpha = None;
        config.background_blur = None;
        let current = load_startup_theme_with_reporter(&config, |_| {});
        assert_eq!(current.background, 1);
        assert_eq!(current.foreground, 3);
        assert!(current.background_blur);
        let preserved = current;

        std::fs::write(&path, b"{}").unwrap();
        let source = ThemeSource::Json(path.clone());
        let error = resolve_live_theme_update(&source, None, None, current).unwrap_err();
        assert_eq!(current, preserved);

        let mut diagnostics = ThemeReloadDiagnostics::default();
        assert!(diagnostics.rejected(&error).is_some());
        assert!(diagnostics.rejected(&error).is_none());
        diagnostics.accepted();
        assert!(diagnostics.rejected(&error).is_some());

        std::fs::write(&path, valid_theme_json(0.5, true)).unwrap();
        let next = resolve_live_theme_update(&source, Some(2468), Some(false), current)
            .unwrap()
            .unwrap();
        assert_eq!(next.background_alpha, 2468);
        assert!(!next.background_blur);
        std::fs::remove_file(path).unwrap();
    }

    fn snapshot(revision: u64) -> TerminalSnapshot {
        TerminalSnapshot {
            splint_id: SplintId::new(),
            incarnation: 1,
            revision,
            columns: 1,
            rows: 1,
            cursor_column: 0,
            cursor_row: 0,
            cursor_deferred_wrap: false,
            active_screen: ActiveScreen::Normal,
            input_modes: TerminalInputModes {
                application_cursor: false,
                application_keypad: false,
                focus_reporting: false,
                bracketed_paste: false,
                cursor_visible: true,
                cursor_blink: true,
                mouse_tracking: splinterm_protocol::MouseTracking::None,
                sgr_mouse: false,
            },
            palette: vec![0; 256],
            default_colors: [0x00eb_ebeb, 0x000e_1216, 0x00eb_ebeb],
            title: String::new(),
            visible_rows: vec![TerminalRow {
                row_id: Some(1),
                linebreak: true,
                cells: Vec::new(),
            }],
            history_generation: 1,
            oldest_available_scrollback_row_id: None,
            newest_available_scrollback_row_id: None,
            scrollback_rows: Vec::new(),
            available_scrollback_rows: 0,
            omitted_oldest_scrollback_rows: 0,
            images: None,
            exited_code: None,
            exited_signal: None,
        }
    }

    #[test]
    fn lifecycle_commands_parse_explicit_ids_and_bounded_ratios() {
        let id = SplintId::new();
        let cli = Cli::try_parse_from([
            "splinterm",
            "split",
            &id.to_string(),
            "--axis",
            "vertical",
            "--side",
            "first",
            "--ratio",
            "400",
            "--",
            "printf",
            "ready",
        ])
        .unwrap();
        let Command::Split {
            target_splint_id,
            axis: SplitAxis::Vertical,
            side: NewSplintSide::First,
            ratio: 400,
            command,
            ..
        } = cli.command
        else {
            panic!("expected parsed split command");
        };
        assert_eq!(target_splint_id, id);
        assert_eq!(command, vec!["printf", "ready"]);

        assert!(
            Cli::try_parse_from([
                "splinterm",
                "split",
                &id.to_string(),
                "--axis",
                "horizontal",
                "--side",
                "second",
                "--ratio",
                "0",
            ])
            .is_err()
        );
        assert!(matches!(
            Cli::try_parse_from(["splinterm", "kill", &id.to_string(), "--yes"])
                .unwrap()
                .command,
            Command::Kill {
                splint_id,
                yes: true,
            } if splint_id == id
        ));
    }

    #[test]
    fn relay_requires_explicit_stdio_transport() {
        assert!(matches!(
            Cli::try_parse_from(["splinterm", "relay", "--stdio"])
                .unwrap()
                .command,
            Command::Relay { stdio: true }
        ));
        assert!(Cli::try_parse_from(["splinterm", "relay"]).is_err());
    }

    #[test]
    fn window_command_requires_exact_paired_resource_ids() {
        let lair_id = LairId::new();
        let dojo_id = DojoId::new();
        let parsed = Cli::try_parse_from([
            "splinterm",
            "window",
            "--lair-id",
            &lair_id.to_string(),
            "--dojo-id",
            &dojo_id.to_string(),
        ])
        .unwrap();
        assert!(matches!(
            parsed.command,
            Command::Window {
                lair_id: Some(parsed_lair),
                dojo_id: Some(parsed_dojo),
            } if parsed_lair == lair_id && parsed_dojo == dojo_id
        ));
        assert!(
            Cli::try_parse_from(["splinterm", "window", "--dojo-id", &dojo_id.to_string(),])
                .is_err()
        );
    }

    #[test]
    fn close_action_kills_live_panes_and_removes_exited_panes() {
        let mut live = splinterm_core::Splint::shell(PathBuf::from("/tmp"));
        let live_id = live.id;
        live.state = SplintState::Running;
        live.last_incarnation = Some(7);
        assert_eq!(
            close_action(&LayoutNode::Leaf(live), live_id).unwrap(),
            CloseAction::KillAndClose { incarnation: 7 }
        );

        let mut exited = splinterm_core::Splint::shell(PathBuf::from("/tmp"));
        let exited_id = exited.id;
        exited.state = SplintState::Exited(0);
        exited.last_incarnation = Some(7);
        let exited_root = LayoutNode::Leaf(exited.clone());
        assert_eq!(
            close_action(&exited_root, exited_id).unwrap(),
            CloseAction::CloseExited
        );
        assert!(validate_exited_close_target(&exited_root, exited_id, Some(7)).unwrap());
        assert!(validate_exited_close_target(&exited_root, exited_id, Some(8)).is_err());

        let sibling = splinterm_core::Splint::shell(PathBuf::from("/tmp"));
        let split_root = LayoutNode::Branch {
            axis: Axis::Horizontal,
            ratio: SplitRatio::new(500).unwrap(),
            first: Box::new(LayoutNode::Leaf(exited)),
            second: Box::new(LayoutNode::Leaf(sibling)),
        };
        assert!(!validate_exited_close_target(&split_root, exited_id, Some(7)).unwrap());
        assert_eq!(
            refreshed_close_state(Some(&split_root), exited_id, Some(7)).unwrap(),
            RefreshedCloseState::Retry
        );
        assert_eq!(
            refreshed_close_state(None, exited_id, Some(7)).unwrap(),
            RefreshedCloseState::WindowClosed
        );
        let unrelated = LayoutNode::Leaf(splinterm_core::Splint::shell(PathBuf::from("/tmp")));
        assert_eq!(
            refreshed_close_state(Some(&unrelated), exited_id, Some(7)).unwrap(),
            RefreshedCloseState::TargetClosed
        );

        let missing_incarnation = splinterm_core::Splint::shell(PathBuf::from("/tmp"));
        let missing_id = missing_incarnation.id;
        assert!(close_action(&LayoutNode::Leaf(missing_incarnation), missing_id).is_err());
    }

    #[test]
    fn dojo_selection_uses_only_its_local_hint() {
        let mut first = splinterm_core::Lair::new("first", PathBuf::from("/tmp"));
        let first_dojo = first.id;
        let first_dojo_id = first.dojos[0].id;
        let first_hint = first.dojos[0].default_focus;
        let second = splinterm_core::Lair::new("second", PathBuf::from("/tmp"));
        let second_dojo_id = second.dojos[0].id;
        let second_hint = second.dojos[0].default_focus;

        let selected = select_dojo_from(
            &[first.clone(), second.clone()],
            (first_dojo, first_dojo_id),
        )
        .unwrap();
        assert_eq!(selected.default_focus, first_hint);
        assert_eq!(selected.root, first.dojos[0].root);
        assert_ne!(first_hint, second_hint);
        assert!(select_dojo_from(&[first.clone(), second], (first_dojo, second_dojo_id)).is_err());

        first.dojos[0].default_focus = SplintId::new();
        assert!(select_dojo_from(&[first], (first_dojo, first_dojo_id)).is_err());
    }

    #[test]
    fn explicit_dojo_choice_requires_an_exact_saved_splint_id() {
        let saved = SplintId::new();
        let choices = vec![(saved, "saved".to_owned())];
        assert_eq!(parse_session_choice(&choices, true, "new\n").unwrap(), None);
        assert_eq!(
            parse_session_choice(&choices, true, &saved.to_string()).unwrap(),
            Some(saved)
        );
        assert!(parse_session_choice(&choices, false, "new").is_err());
    }

    #[test]
    fn launch_defaults_to_fresh_creation_with_a_collision_resistant_name() {
        let cli = Cli::try_parse_from(["splinterm", "launch"]).unwrap();
        let Command::Launch {
            name,
            splint_id,
            new,
            command,
            ..
        } = cli.command
        else {
            panic!("expected launch command");
        };
        assert!(name.is_none());
        assert!(splint_id.is_none());
        assert!(!new);
        assert!(command.is_empty());
        assert_eq!(
            fresh_dojo_name(UNIX_EPOCH + Duration::from_secs(1_234), 56),
            "terminal-1234-56"
        );
    }

    #[test]
    fn create_request_preserves_direct_argv_without_shell_interpolation() {
        let argv = vec![
            "/usr/bin/printf".to_owned(),
            "%s\\n".to_owned(),
            "$(touch /tmp/must-not-run); spaced argument".to_owned(),
        ];
        let request = create_request(
            TopologyRevision::default(),
            "argv".to_owned(),
            PathBuf::from("/tmp"),
            argv.clone(),
            &AppConfig::default(),
        );
        let Request::CreateLair { launch, .. } = request else {
            panic!("expected create request");
        };
        assert_eq!(launch.command, argv);
    }

    #[test]
    fn aggregate_update_interval_advances_from_published_revision() {
        let update = TerminalUpdate {
            base_revision: 4,
            revision: 29,
            rows: Vec::new(),
            scrolls: Vec::new(),
            cursor: None,
            title: None,
            input_modes: None,
            active_screen: None,
            palette: None,
            default_colors: None,
            columns: None,
            row_count: None,
            scrollback: None,
            images: None,
        };
        assert!(update_advances_from(&update, 4));
        assert!(!update_advances_from(&update, 3));
        let stale = TerminalUpdate {
            revision: 4,
            ..update
        };
        assert!(!update_advances_from(&stale, 4));
    }

    #[test]
    fn subscription_classifier_tracks_order_and_resyncs_gaps() {
        let action = classify_subscription_event(
            9,
            0,
            9,
            1,
            SubscriptionEvent::Snapshot {
                snapshot: snapshot(2),
            },
        );
        assert!(matches!(action, EventAction::Snapshot { sequence: 1, .. }));
        assert_eq!(
            classify_subscription_event(
                9,
                1,
                9,
                3,
                SubscriptionEvent::Snapshot {
                    snapshot: snapshot(3)
                },
            ),
            EventAction::Resynchronize
        );
        assert_eq!(
            classify_subscription_event(
                9,
                1,
                9,
                2,
                SubscriptionEvent::ResyncRequired {
                    current_revision: 4
                },
            ),
            EventAction::Resynchronize
        );
    }

    #[test]
    fn scrollback_page_response_validation_enforces_request_identity_and_cursor() {
        let page = splinterm_protocol::ScrollbackPage {
            splint_id: SplintId::new(),
            incarnation: 2,
            terminal_revision: 4,
            history_generation: 3,
            oldest_available_row_id: Some(1),
            newest_available_row_id: Some(12),
            rows: vec![TerminalRow {
                row_id: Some(8),
                linebreak: false,
                cells: Vec::new(),
            }],
            has_older: true,
        };
        assert!(
            validate_scrollback_page_response(
                &page,
                page.splint_id,
                page.incarnation,
                page.terminal_revision,
                page.history_generation,
                9,
            )
            .is_ok()
        );
        assert!(
            validate_scrollback_page_response(
                &page,
                SplintId::new(),
                page.incarnation,
                page.terminal_revision,
                page.history_generation,
                9,
            )
            .is_err()
        );
        assert!(
            validate_scrollback_page_response(
                &page,
                page.splint_id,
                page.incarnation,
                page.terminal_revision + 1,
                page.history_generation,
                9,
            )
            .is_err()
        );
        assert!(
            validate_scrollback_page_response(
                &page,
                page.splint_id,
                page.incarnation,
                page.terminal_revision,
                page.history_generation,
                8,
            )
            .is_err()
        );
    }

    #[test]
    fn initial_attachment_validation_rejects_malformed_and_wrong_identity_snapshots() {
        let valid = snapshot(1);
        assert!(validate_attached_snapshot(&valid, valid.splint_id, valid.incarnation).is_ok());

        let mut malformed = valid.clone();
        malformed.history_generation = 0;
        assert!(
            validate_attached_snapshot(&malformed, valid.splint_id, valid.incarnation).is_err()
        );
        assert!(validate_attached_snapshot(&valid, SplintId::new(), valid.incarnation).is_err());
        assert!(
            validate_attached_snapshot(&valid, valid.splint_id, valid.incarnation + 1).is_err()
        );
    }

    #[test]
    fn control_conflict_falls_back_to_observer_without_hiding_other_errors() {
        let unavailable = response_protocol_error(splinterm_protocol::ProtocolError::new(
            ErrorCode::ControllerUnavailable,
            "live Splint already has a controller",
        ));
        assert_eq!(optional_pane_controller(Err(unavailable)).unwrap(), None);
        assert_eq!(optional_pane_controller(Ok(42)).unwrap(), Some(42));

        let invalid = response_protocol_error(splinterm_protocol::ProtocolError::new(
            ErrorCode::InvalidArgument,
            "bad control request",
        ));
        assert!(optional_pane_controller(Err(invalid)).is_err());
    }

    #[test]
    fn topology_diff_and_parent_ratio_are_identity_local() {
        let first = splinterm_core::Splint::shell(PathBuf::from("/tmp"));
        let first_id = first.id;
        let second = splinterm_core::Splint::shell(PathBuf::from("/tmp"));
        let second_id = second.id;
        let third = splinterm_core::Splint::shell(PathBuf::from("/tmp"));
        let third_id = third.id;
        let initial = LayoutNode::Branch {
            axis: Axis::Horizontal,
            ratio: SplitRatio::new(400).unwrap(),
            first: Box::new(LayoutNode::Leaf(first.clone())),
            second: Box::new(LayoutNode::Leaf(second.clone())),
        };
        let nested = LayoutNode::Branch {
            axis: Axis::Horizontal,
            ratio: SplitRatio::new(400).unwrap(),
            first: Box::new(LayoutNode::Leaf(first)),
            second: Box::new(LayoutNode::Branch {
                axis: Axis::Vertical,
                ratio: SplitRatio::new(650).unwrap(),
                first: Box::new(LayoutNode::Leaf(second)),
                second: Box::new(LayoutNode::Leaf(third)),
            }),
        };
        let (added, removed) = topology_identity_diff(&initial, &nested);
        assert_eq!(added, vec![third_id]);
        assert!(removed.is_empty());
        assert_eq!(parent_ratio(&nested, first_id).unwrap().get(), 400);
        assert_eq!(parent_ratio(&nested, second_id).unwrap().get(), 650);
        assert!(pane_claims_initial_control(second_id, second_id));
        assert!(!pane_claims_initial_control(first_id, second_id));
        assert_eq!(parent_ratio(&nested, third_id).unwrap().get(), 650);
    }

    #[test]
    fn subscription_classifier_ignores_old_subscription_and_stops_on_exit() {
        assert_eq!(
            classify_subscription_event(
                9,
                0,
                8,
                1,
                SubscriptionEvent::Snapshot {
                    snapshot: snapshot(2)
                },
            ),
            EventAction::Ignore
        );
        assert_eq!(
            classify_subscription_event(
                9,
                0,
                9,
                1,
                SubscriptionEvent::Exited {
                    code: Some(0),
                    signal: None
                },
            ),
            EventAction::Exited
        );
        assert_eq!(
            classify_subscription_event(
                9,
                0,
                9,
                1,
                SubscriptionEvent::AccessRevoked { grant_id: 4 },
            ),
            EventAction::Shutdown
        );
        assert_eq!(
            classify_subscription_event(
                9,
                1,
                9,
                7,
                SubscriptionEvent::Exited {
                    code: Some(0),
                    signal: None
                },
            ),
            EventAction::Exited
        );
        assert_eq!(
            classify_subscription_event(
                9,
                1,
                9,
                7,
                SubscriptionEvent::AccessRevoked { grant_id: 5 },
            ),
            EventAction::Shutdown
        );
    }

    #[test]
    fn resize_is_retained_until_control_is_available_and_uses_existing_control() {
        let splint_id = SplintId::new();
        let resize = (80, 40, 800, 800);
        let mut prepared = None;

        assert!(resolved_resize_request(None, &mut prepared, (splint_id, 3), resize).is_none());
        assert_eq!(prepared, Some(resize));

        let request = resolved_resize_request(
            Some(9),
            &mut prepared,
            (splint_id, 3),
            (100, 50, 1_000, 1_000),
        )
        .unwrap();
        assert!(matches!(
            request,
            Request::Resize {
                controller_id: 9,
                splint_id: requested_splint,
                incarnation: 3,
                columns: 100,
                rows: 50,
                pixel_width: 1_000,
                pixel_height: 1_000,
            } if requested_splint == splint_id
        ));
        assert_eq!(prepared, None);
    }

    #[test]
    fn pane_resize_debounce_keeps_latest_size_any_control_claim_and_idle_deadline() {
        let mut pending = None;
        let mut deadline = None;
        let delay = Duration::from_millis(100);
        let started = tokio::time::Instant::now();
        assert!(
            queue_pane_resize(
                &mut pending,
                &mut deadline,
                PendingPaneResize {
                    size: (80, 24, 800, 480),
                    claim_control: false,
                },
                delay,
                started,
            )
            .is_none()
        );
        assert_eq!(deadline, Some(started + delay));

        let latest_at = started + Duration::from_millis(50);
        assert!(
            queue_pane_resize(
                &mut pending,
                &mut deadline,
                PendingPaneResize {
                    size: (100, 40, 1_000, 800),
                    claim_control: true,
                },
                delay,
                latest_at,
            )
            .is_none()
        );
        assert_eq!(deadline, Some(latest_at + delay));
        assert_eq!(
            pending,
            Some(PendingPaneResize {
                size: (100, 40, 1_000, 800),
                claim_control: true,
            })
        );
    }

    #[test]
    fn zero_resize_delay_returns_immediate_command_without_pending_state() {
        let mut pending = None;
        let mut deadline = None;
        let immediate = queue_pane_resize(
            &mut pending,
            &mut deadline,
            PendingPaneResize {
                size: (80, 24, 800, 480),
                claim_control: true,
            },
            Duration::ZERO,
            tokio::time::Instant::now(),
        );

        assert!(matches!(immediate, Some(WindowCommand::Resize { .. })));
        assert!(pending.is_none());
        assert!(deadline.is_none());
    }

    #[tokio::test]
    async fn granted_control_event_exposes_controller_for_prepared_resize() {
        let (updates, mut receiver) = mpsc::channel(1);
        let mut active_controller = None;
        let acquired = handle_control_event(
            ServerFrame::Event {
                subscription_id: 7,
                sequence: 1,
                event: SubscriptionEvent::ControlTransferResolved {
                    transfer_id: 11,
                    outcome: ControlTransferOutcome::Granted,
                    controller_id: Some(9),
                },
            },
            7,
            &mut active_controller,
            &updates,
        )
        .await
        .unwrap();

        assert!(acquired);
        assert_eq!(active_controller, Some(9));
        assert!(matches!(
            receiver.try_recv(),
            Ok(WindowUpdate::ControlTransferResolved(
                ControlTransferOutcome::Granted
            ))
        ));
        let mut prepared = Some((100, 40, 1_000, 800));
        assert!(matches!(
            resolved_resize_request(
                active_controller,
                &mut prepared,
                (SplintId::new(), 3),
                (100, 40, 1_000, 800),
            ),
            Some(Request::Resize {
                controller_id: 9,
                columns: 100,
                rows: 40,
                ..
            })
        ));
        assert!(prepared.is_none());
    }

    #[test]
    fn window_controller_accepts_only_its_exact_terminal_action_acknowledgement() {
        let splint_id = SplintId::new();
        let response = Response::TerminalActionAcknowledged {
            lair_id: splinterm_core::LairId::new(),
            dojo_id: splinterm_core::DojoId::new(),
            splint_id,
            incarnation: 3,
            terminal_revision: 7,
            history_generation: 2,
        };
        assert!(terminal_action_matches(&response, splint_id, 3));
        assert!(!terminal_action_matches(&response, splint_id, 4));
        assert!(!terminal_action_matches(&response, SplintId::new(), 3));
        assert!(!terminal_action_matches(
            &Response::Acknowledged,
            splint_id,
            3
        ));
    }
}
