use std::{
    collections::{HashMap, HashSet},
    env,
    io::{self, IsTerminal, Read, Write},
    os::unix::process::CommandExt,
    path::PathBuf,
    process::Command as ProcessCommand,
    sync::mpsc as std_mpsc,
};

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use splinterm::{
    AuthorityStatus, TrustedConsentUi, WindowCommand, WindowOptions, WindowPaneOptions,
    WindowTopologyCommand, WindowTopologyUpdate, WindowUpdate,
    automation::{
        CliEnvelopeV1, CliErrorCodeV1, CliEventV1, Connection, MutationIdentityV1, PingEnvelopeV1,
        ReadResyncReasonV1, ResyncReasonV1, TerminalContinuationV1, TerminalReadProvenanceV1,
        audit_page_envelope, authorization_status_envelope, committed_mutation_envelope,
        created_mutation_envelope, decode_terminal_cursor, inspect_splint_envelope,
        inspect_topology_envelope, kill_envelope, list_dojos_envelope, process_started_envelope,
        protocol_error, public_error_code, read_resync_envelope, response_protocol_error,
        restore_many_envelope, revoke_envelope, scrollback_page_envelope, search_page_envelope,
        terminal_action_envelope, terminal_snapshot_envelope, write_json_document,
    },
    config::{AppConfig, ConfigLoad, ResolvedTheme, load_default, load_theme},
    renderer::{self, RendererOptions},
    run_window,
};
use splinterm_core::{
    Axis, DojoId, LayoutNode, SplintId, SplitRatio, SplitSide, TopologyRevision, WindowId,
};
use splinterm_protocol::{
    AccessGrant, AccessScope, ActiveScreen, CellAttributes, ColorSource, ConsentPrompt,
    ConsentReply, ControlTransferOutcome, ErrorCode, HistoryTransition, LaunchParameters,
    MAX_CONSENT_FRAME_BYTES, Request, Response, ServerFrame, SubscriptionEvent, TerminalCell,
    TerminalInputModes, TerminalRow, TerminalSnapshot, TerminalUpdate, UnderlineStyle,
};
use tokio::sync::mpsc;

const WINDOW_UPDATE_QUEUE: usize = 4;
const WINDOW_COMMAND_QUEUE: usize = 64;

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
    /// Render ordered snapshots of one explicitly selected terminal.
    Window {
        /// Select a Dojo by stable identity (required with --window-id).
        #[arg(long, requires = "window_id")]
        dojo_id: Option<DojoId>,
        /// Select one daemon-owned window (required with --dojo-id).
        #[arg(long, requires = "dojo_id")]
        window_id: Option<WindowId>,
    },
    Ping,
    List,
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
    New {
        name: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Execute argv directly instead of starting the configured shell.
        #[arg(last = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// xdg-terminal-exec-compatible create/attach and graphical launch.
    Launch {
        #[arg(long = "working-directory", alias = "dir")]
        cwd: Option<PathBuf>,
        #[arg(long, default_value = "main")]
        name: String,
        /// Attach an existing Splint by stable identity.
        #[arg(long)]
        splint_id: Option<SplintId>,
        /// Create a new Dojo even when saved sessions exist.
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
    /// Create a daemon-owned window with one live Splint.
    NewWindow {
        dojo_id: DojoId,
        #[arg(long, default_value = "terminal")]
        title: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(last = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Close a window whose Splints have all exited.
    CloseWindow {
        window_id: WindowId,
        /// Confirm destructive topology mutation for machine output.
        #[arg(long)]
        yes: bool,
    },
    RenameDojo {
        dojo_id: DojoId,
        name: String,
    },
    RenameWindow {
        window_id: WindowId,
        title: String,
    },
    /// Set a persisted convenience hint without changing any client's actual focus.
    WindowFocusHint {
        window_id: WindowId,
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
    /// Restore every exited Splint in a saved window.
    RestoreWindow {
        window_id: WindowId,
    },
    /// Restore every exited Splint in a saved Dojo.
    RestoreDojo {
        dojo_id: DojoId,
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
            println!("valid splinterm.policy.v1 policy ({rule_count} rules)");
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
        Command::Window { .. }
            | Command::Launch { .. }
            | Command::Consent
            | Command::Policy { .. }
            | Command::Relay { .. }
    ) && (output.is_some() || schema_major.is_some() || timeout_ms.is_some())
    {
        usage_error(
            "automation output, schema, and timeout options are unavailable for graphical, policy, and relay commands",
        );
    }
    if matches!(command, Command::Subscribe { .. }) && output != Some(OutputMode::Ndjson) {
        usage_error("subscriptions require --output ndjson");
    }
    if output == Some(OutputMode::Json) {
        match run_machine_command(
            command,
            schema_major.unwrap_or(1),
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
            schema_major.unwrap_or(1),
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

    let ConfigLoad {
        config,
        diagnostics,
    } = load_default()?;
    for diagnostic in diagnostics {
        eprintln!("splinterm config: {diagnostic}");
    }
    if let Command::Window { dojo_id, window_id } = command {
        let window = select_window(dojo_id.zip(window_id)).await?;
        return run_live_multipane_window(config, window).await;
    }
    if let Command::Launch {
        cwd,
        name,
        splint_id,
        new,
        command,
    } = &command
    {
        let cwd = cwd
            .clone()
            .unwrap_or(env::current_dir().context("failed to read current directory")?);
        launch(name.clone(), cwd, *splint_id, *new, command.clone(), config).await?;
        return Ok(());
    }
    if matches!(command, Command::Consent) {
        return tokio::task::spawn_blocking(run_consent_client)
            .await
            .context("trusted consent task failed")?;
    }

    run_headless(command, &config).await
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
        Command::List => run_machine_read(MachineRead::List, schema_major, timeout_ms).await,
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
            Self::List => "list_dojos",
            Self::Topology => "inspect_topology",
            Self::Splint(_) => "inspect_splint",
        }
    }
}

fn write_machine_read_failure(
    operation: &'static str,
    code: CliErrorCodeV1,
    message: impl Into<String>,
    retryable: bool,
) -> Result<()> {
    write_json_document(&CliEnvelopeV1::failure(
        operation, code, message, retryable,
    )?)
}

fn write_machine_connection_failure(operation: &'static str, error: &anyhow::Error) -> Result<()> {
    if let Some(protocol) = protocol_error(error) {
        return write_json_document(&CliEnvelopeV1::protocol_failure(
            operation,
            protocol,
            bounded_public_message(error),
        )?);
    }
    write_machine_read_failure(
        operation,
        CliErrorCodeV1::Internal,
        bounded_public_message(error),
        true,
    )
}

async fn run_machine_read(command: MachineRead, schema_major: u16, timeout_ms: u64) -> Result<()> {
    let operation = command.operation();
    if schema_major != 1 {
        write_machine_read_failure(
            operation,
            CliErrorCodeV1::UnsupportedSchema,
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
                    CliErrorCodeV1::Timeout,
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
                (CliErrorCodeV1::Timeout, true)
            } else if let Some(protocol) = protocol_error(&error) {
                public_error_code(protocol.code)
            } else {
                (CliErrorCodeV1::Internal, true)
            };
            write_machine_read_failure(operation, code, bounded_public_message(&error), retryable)?;
            return Err(error);
        }
    };
    let Response::Topology { snapshot } = response else {
        write_machine_read_failure(
            operation,
            CliErrorCodeV1::Internal,
            "splinterd returned an unexpected topology response",
            false,
        )?;
        bail!("splinterd returned an unexpected topology response");
    };
    if let MachineRead::Splint(splint_id) = command
        && snapshot.lair.find_splint(splint_id).is_none()
    {
        write_machine_read_failure(
            operation,
            CliErrorCodeV1::NotFound,
            "requested Splint was not found",
            false,
        )?;
        bail!("requested Splint was not found");
    }
    let envelope = match command {
        MachineRead::List => list_dojos_envelope(&snapshot),
        MachineRead::Topology => inspect_topology_envelope(&snapshot),
        MachineRead::Splint(splint_id) => inspect_splint_envelope(&snapshot, splint_id),
    };
    match envelope {
        Ok(envelope) => write_json_document(&envelope),
        Err(error) => {
            write_machine_read_failure(
                operation,
                CliErrorCodeV1::Internal,
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
    NewWindow {
        dojo_id: DojoId,
        title: String,
        cwd: Option<PathBuf>,
        command: Vec<String>,
    },
    CloseWindow {
        window_id: WindowId,
        yes: bool,
    },
    RenameDojo {
        dojo_id: DojoId,
        name: String,
    },
    RenameWindow {
        window_id: WindowId,
        title: String,
    },
    Focus {
        window_id: WindowId,
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
    RestoreWindow {
        window_id: WindowId,
    },
    RestoreDojo {
        dojo_id: DojoId,
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
        Command::NewWindow {
            dojo_id,
            title,
            cwd,
            command,
        } => MachineMutation::NewWindow {
            dojo_id,
            title,
            cwd,
            command,
        },
        Command::CloseWindow { window_id, yes } => MachineMutation::CloseWindow { window_id, yes },
        Command::RenameDojo { dojo_id, name } => MachineMutation::RenameDojo { dojo_id, name },
        Command::RenameWindow { window_id, title } => {
            MachineMutation::RenameWindow { window_id, title }
        }
        Command::WindowFocusHint {
            window_id,
            splint_id,
        } => MachineMutation::Focus {
            window_id,
            splint_id,
        },
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
        Command::RestoreWindow { window_id } => MachineMutation::RestoreWindow { window_id },
        Command::RestoreDojo { dojo_id } => MachineMutation::RestoreDojo { dojo_id },
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
            Self::Create { .. } => "create_dojo",
            Self::Split { .. } => "split_splint",
            Self::CloseSplint { .. } => "close_splint",
            Self::Ratio { .. } => "set_split_ratio",
            Self::NewWindow { .. } => "new_window",
            Self::CloseWindow { .. } => "close_window",
            Self::RenameDojo { .. } => "rename_dojo",
            Self::RenameWindow { .. } => "rename_window",
            Self::Focus { .. } => "set_window_default_focus",
            Self::RenameSplint { .. } => "rename_splint",
            Self::Relaunch { .. } => "relaunch_splint",
            Self::RestoreSplint { .. } => "restore_splint",
            Self::RestoreWindow { .. } => "restore_window",
            Self::RestoreDojo { .. } => "restore_dojo",
            Self::Kill { .. } => "kill_splint",
            Self::Revoke { .. } => "revoke_access",
        }
    }

    const fn confirmation_missing(&self) -> bool {
        matches!(
            self,
            Self::CloseSplint { yes: false, .. }
                | Self::CloseWindow { yes: false, .. }
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
) -> Result<(DojoId, WindowId)> {
    topology
        .lair
        .dojos()
        .find_map(|dojo| {
            dojo.windows
                .iter()
                .find(|window| window.root.find_splint(splint_id).is_some())
                .map(|window| (dojo.id, window.id))
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

fn topology_window_location(
    topology: &splinterm_protocol::TopologySnapshot,
    window_id: WindowId,
) -> Result<DojoId> {
    topology
        .lair
        .dojos()
        .find(|dojo| dojo.windows.iter().any(|window| window.id == window_id))
        .map(|dojo| dojo.id)
        .context("requested window was not found")
}

fn require_dojo(topology: &splinterm_protocol::TopologySnapshot, dojo_id: DojoId) -> Result<()> {
    if topology.lair.dojos().any(|dojo| dojo.id == dojo_id) {
        Ok(())
    } else {
        bail!("requested Dojo was not found")
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
        MachineMutation::Create { name, cwd, command } => Request::CreateDojo {
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
        MachineMutation::NewWindow {
            dojo_id,
            title,
            cwd,
            command,
        } => {
            require_dojo(topology, *dojo_id)?;
            Request::NewWindow {
                expected_topology_revision,
                dojo_id: *dojo_id,
                title: title.clone(),
                launch: machine_launch(cwd.clone(), command.clone())?,
            }
        }
        MachineMutation::CloseWindow { window_id, .. } => {
            topology_window_location(topology, *window_id)?;
            Request::CloseWindow {
                expected_topology_revision,
                window_id: *window_id,
            }
        }
        MachineMutation::RenameDojo { dojo_id, name } => {
            require_dojo(topology, *dojo_id)?;
            Request::RenameDojo {
                expected_topology_revision,
                dojo_id: *dojo_id,
                name: name.clone(),
            }
        }
        MachineMutation::RenameWindow { window_id, title } => {
            topology_window_location(topology, *window_id)?;
            Request::RenameWindow {
                expected_topology_revision,
                window_id: *window_id,
                title: title.clone(),
            }
        }
        MachineMutation::Focus {
            window_id,
            splint_id,
        } => {
            let (_, actual_window) = topology_splint_location(topology, *splint_id)?;
            if actual_window != *window_id {
                bail!("selected Splint does not belong to the selected window");
            }
            Request::SetWindowDefaultFocus {
                expected_topology_revision,
                window_id: *window_id,
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
        MachineMutation::RestoreWindow { window_id } => {
            topology_window_location(topology, *window_id)?;
            Request::RestoreWindow {
                expected_topology_revision,
                window_id: *window_id,
            }
        }
        MachineMutation::RestoreDojo { dojo_id } => {
            require_dojo(topology, *dojo_id)?;
            Request::RestoreDojo {
                expected_topology_revision,
                dojo_id: *dojo_id,
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
                CliErrorCodeV1::Timeout,
                "connection deadline elapsed",
                true,
            )?;
            bail!("splinterd connection timed out")
        }
    }
}

fn finish_machine_envelope(operation: &'static str, result: Result<CliEnvelopeV1>) -> Result<()> {
    match result {
        Ok(envelope) => write_json_document(&envelope),
        Err(error) => {
            if let Some(protocol) = protocol_error(&error) {
                write_json_document(&CliEnvelopeV1::protocol_failure(
                    operation,
                    protocol,
                    bounded_public_message(&error),
                )?)?;
                return Err(error);
            }
            let (code, retryable) = if error.to_string().contains("timed out") {
                (CliErrorCodeV1::Timeout, true)
            } else if error.to_string().contains("not found") {
                (CliErrorCodeV1::NotFound, false)
            } else if error.to_string().contains("expected incarnation")
                || error.to_string().contains("does not have a live process")
            {
                (CliErrorCodeV1::StaleIncarnation, false)
            } else {
                (CliErrorCodeV1::Internal, false)
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
) -> Result<MutationIdentityV1> {
    let (dojo_id, window_id) = topology_splint_location(topology, splint_id)?;
    Ok(MutationIdentityV1 {
        dojo_id: Some(dojo_id),
        window_id: Some(window_id),
        splint_id: Some(splint_id),
        topology_revision: Some(revision),
        incarnation,
    })
}

fn created_dojo_envelope(
    before: &splinterm_protocol::TopologySnapshot,
    dojo: &splinterm_core::Dojo,
    incarnation: u64,
    topology_revision: TopologyRevision,
) -> Result<CliEnvelopeV1> {
    let revision = committed_revision(before.revision, topology_revision)?;
    if dojo.windows.len() != 1 || incarnation == 0 {
        bail!("splinterd returned inconsistent created Dojo topology");
    }
    let window = &dojo.windows[0];
    let LayoutNode::Leaf(splint) = &window.root else {
        bail!("created Dojo did not contain one Splint leaf");
    };
    if before.lair.dojos().any(|existing| existing.id == dojo.id)
        || before
            .lair
            .dojos()
            .flat_map(|existing| &existing.windows)
            .any(|existing| existing.id == window.id)
        || before.lair.find_splint(splint.id).is_some()
    {
        bail!("create response reused an existing stable identity");
    }
    created_mutation_envelope(
        "create_dojo",
        MutationIdentityV1 {
            dojo_id: Some(dojo.id),
            window_id: Some(window.id),
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
) -> Result<CliEnvelopeV1> {
    let revision = committed_revision(topology.revision, revision)?;
    let (identity, confirmed) = match mutation {
        MachineMutation::CloseSplint { splint_id, .. }
        | MachineMutation::Ratio { splint_id, .. }
        | MachineMutation::RenameSplint { splint_id, .. } => (
            topology_identity(topology, *splint_id, revision, None)?,
            matches!(mutation, MachineMutation::CloseSplint { .. }),
        ),
        MachineMutation::Focus {
            window_id,
            splint_id,
        } => {
            let identity = topology_identity(topology, *splint_id, revision, None)?;
            if identity.window_id != Some(*window_id) {
                bail!("committed focus hint identity is inconsistent");
            }
            (identity, false)
        }
        MachineMutation::CloseWindow { window_id, .. }
        | MachineMutation::RenameWindow { window_id, .. } => (
            MutationIdentityV1 {
                dojo_id: Some(topology_window_location(topology, *window_id)?),
                window_id: Some(*window_id),
                splint_id: None,
                topology_revision: Some(revision),
                incarnation: None,
            },
            matches!(mutation, MachineMutation::CloseWindow { .. }),
        ),
        MachineMutation::RenameDojo { dojo_id, .. } => (
            MutationIdentityV1 {
                dojo_id: Some(*dojo_id),
                window_id: None,
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
        MachineMutation::RestoreWindow { window_id } => {
            let window = topology
                .lair
                .dojos()
                .flat_map(|dojo| &dojo.windows)
                .find(|window| window.id == *window_id)
                .context("restore window disappeared from reviewed topology")?;
            layout_ids(&window.root, &mut expected);
        }
        MachineMutation::RestoreDojo { dojo_id } => {
            let dojo = topology
                .lair
                .dojos()
                .find(|dojo| dojo.id == *dojo_id)
                .context("restore Dojo disappeared from reviewed topology")?;
            for window in &dojo.windows {
                layout_ids(&window.root, &mut expected);
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
) -> Result<CliEnvelopeV1> {
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
            if topology.lair.find_splint(splint_id).is_some() {
                bail!("split response reused an existing Splint identity");
            }
            let (dojo_id, window_id) = topology_splint_location(topology, *target_splint_id)?;
            created_mutation_envelope(
                "split_splint",
                MutationIdentityV1 {
                    dojo_id: Some(dojo_id),
                    window_id: Some(window_id),
                    splint_id: Some(splint_id),
                    topology_revision: Some(revision),
                    incarnation: Some(incarnation),
                },
            )
        }
        (
            MachineMutation::NewWindow { dojo_id, .. },
            Response::WindowStarted {
                window_id,
                splint_id,
                incarnation,
                topology_revision,
            },
        ) => {
            if topology.lair.find_splint(splint_id).is_some()
                || topology
                    .lair
                    .dojos()
                    .flat_map(|dojo| &dojo.windows)
                    .any(|window| window.id == window_id)
            {
                bail!("new-window response reused an existing stable identity");
            }
            created_mutation_envelope(
                "new_window",
                MutationIdentityV1 {
                    dojo_id: Some(*dojo_id),
                    window_id: Some(window_id),
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
            MachineMutation::RestoreWindow { window_id },
            Response::RestoreCompleted {
                topology_revision,
                results,
            },
        ) => {
            validate_restore_results(topology, mutation, topology_revision, &results)?;
            restore_many_envelope(
                "restore_window",
                MutationIdentityV1 {
                    dojo_id: Some(topology_window_location(topology, *window_id)?),
                    window_id: Some(*window_id),
                    splint_id: None,
                    topology_revision: Some(topology_revision.get()),
                    incarnation: None,
                },
                &results,
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
                MutationIdentityV1 {
                    dojo_id: Some(*dojo_id),
                    window_id: None,
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
            let (dojo_id, window_id, expected_incarnation) =
                live_terminal_location(topology, *splint_id)?;
            if incarnation != expected_incarnation {
                bail!("splinterd returned an inconsistent killed incarnation");
            }
            kill_envelope(dojo_id, window_id, *splint_id, incarnation)
        }
        (MachineMutation::Revoke { grant_id, .. }, Response::AccessRevoked { grant })
            if *grant_id == grant.grant_id =>
        {
            let (dojo_id, window_id, incarnation) =
                live_terminal_location(topology, grant.splint_id)?;
            if incarnation != grant.incarnation {
                bail!("revoked grant incarnation does not match reviewed topology");
            }
            revoke_envelope(dojo_id, window_id, &grant)
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
) -> Result<CliEnvelopeV1> {
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
        let Response::DojoCreated {
            dojo,
            incarnation,
            topology_revision,
        } = response
        else {
            bail!("splinterd returned an inconsistent create response");
        };
        return created_dojo_envelope(&topology, &dojo, incarnation, topology_revision);
    }
    mutation_response_envelope(mutation, &topology, response)
}

async fn run_machine_mutation(
    mutation: MachineMutation,
    schema_major: u16,
    timeout_ms: u64,
) -> Result<()> {
    let operation = mutation.operation();
    if schema_major != 1 {
        write_machine_read_failure(
            operation,
            CliErrorCodeV1::UnsupportedSchema,
            format!("unsupported schema major {schema_major}"),
            false,
        )?;
        bail!("unsupported schema major {schema_major}");
    }
    if mutation.confirmation_missing() {
        write_machine_read_failure(
            operation,
            CliErrorCodeV1::ConfirmationRequired,
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
    if schema_major != 1 {
        write_machine_read_failure(
            OPERATION,
            CliErrorCodeV1::UnsupportedSchema,
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
                Request::InspectTopology,
                deadline.saturating_sub(started.elapsed()),
            )
            .await?;
        let Response::Topology { snapshot: topology } = response else {
            bail!("splinterd returned an unexpected topology response");
        };
        let (dojo_id, window_id, incarnation) = live_terminal_location(&topology, splint_id)?;
        let response = connection
            .request_with_deadline(
                Request::AuthorizationStatus {
                    splint_id,
                    incarnation,
                },
                deadline.saturating_sub(started.elapsed()),
            )
            .await?;
        let Response::AuthorizationStatus {
            grants,
            persistent,
            development_bypass,
        } = response
        else {
            bail!("splinterd returned an unexpected authorization response");
        };
        authorization_status_envelope(
            dojo_id,
            window_id,
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
    if schema_major != 1 {
        write_machine_read_failure(
            OPERATION,
            CliErrorCodeV1::UnsupportedSchema,
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
) -> Result<CliEnvelopeV1> {
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
    let (dojo_id, window_id, incarnation) = live_terminal_location(&topology, splint_id)?;
    let response = connection
        .request_with_deadline(
            Request::AcquireControl {
                splint_id,
                incarnation,
            },
            deadline.saturating_sub(started.elapsed()),
        )
        .await?;
    let Response::ControlGranted { controller_id } = response else {
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
        dojo_id: response_dojo,
        window_id: response_window,
        splint_id: response_splint,
        incarnation: response_incarnation,
        terminal_revision,
        history_generation,
    } = response
    else {
        bail!("splinterd returned an unexpected terminal action response");
    };
    if (
        response_dojo,
        response_window,
        response_splint,
        response_incarnation,
    ) != (dojo_id, window_id, splint_id, incarnation)
    {
        bail!("splinterd returned inconsistent terminal action identity");
    }
    terminal_action_envelope(
        command.operation(),
        TerminalReadProvenanceV1 {
            dojo_id,
            window_id,
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
    if schema_major != 1 {
        write_machine_read_failure(
            operation,
            CliErrorCodeV1::UnsupportedSchema,
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
    provenance: TerminalReadProvenanceV1,
    before_row_id: Option<u64>,
    daemon_cursor: Option<String>,
}

fn history_cursor_context(
    command: &MachineHistory,
    encoded: &str,
    dojo_id: DojoId,
    window_id: WindowId,
    splint_id: SplintId,
    incarnation: u64,
) -> Result<MachineHistoryContext> {
    let cursor = decode_terminal_cursor(encoded).context("invalid continuation cursor")?;
    let (cursor_splint, cursor_incarnation, revision, generation, before, daemon) = match cursor {
        TerminalContinuationV1::Scrollback {
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
        TerminalContinuationV1::Search {
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
        provenance: TerminalReadProvenanceV1 {
            dojo_id,
            window_id,
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
) -> Result<(DojoId, WindowId, u64)> {
    topology
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    let (dojo_id, window_id) = topology
        .lair
        .dojos()
        .find_map(|dojo| {
            dojo.windows
                .iter()
                .find(|window| window.root.find_splint(splint_id).is_some())
                .map(|window| (dojo.id, window.id))
        })
        .context("requested Splint was not found")?;
    let incarnation = topology
        .runtimes
        .iter()
        .find(|runtime| runtime.splint_id == splint_id)
        .context("validated topology omitted Splint runtime")?
        .live_incarnation
        .context("selected Splint does not have a live process")?;
    Ok((dojo_id, window_id, incarnation))
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
    let (dojo_id, window_id, incarnation) = live_terminal_location(&topology, splint_id)?;
    if let Some(encoded) = command.cursor() {
        return history_cursor_context(
            command,
            encoded,
            dojo_id,
            window_id,
            splint_id,
            incarnation,
        );
    }
    let response = connection
        .request_with_deadline(
            Request::Attach {
                splint_id,
                incarnation,
                scrollback_rows: 0,
            },
            deadline.saturating_sub(started.elapsed()),
        )
        .await?;
    let Response::Attached {
        subscription_id,
        snapshot,
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
        provenance: TerminalReadProvenanceV1 {
            dojo_id,
            window_id,
            splint_id,
            incarnation,
            terminal_revision: snapshot.revision,
            history_generation: snapshot.history_generation,
        },
        before_row_id: Some(before_row_id),
        daemon_cursor: None,
    })
}

async fn machine_history_envelope(
    connection: &mut Connection,
    command: &MachineHistory,
    splint_id: SplintId,
    deadline: std::time::Duration,
    started: std::time::Instant,
) -> Result<CliEnvelopeV1> {
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
        Response::ScrollbackPage { page }
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
                context.provenance.dojo_id,
                context.provenance.window_id,
                &page,
            )
        }
        Response::SearchResults { page } if matches!(command, MachineHistory::Search { .. }) => {
            if page.splint_id != splint_id
                || page.incarnation != context.provenance.incarnation
                || page.terminal_revision != context.provenance.terminal_revision
                || page.history_generation != context.provenance.history_generation
            {
                bail!("splinterd returned inconsistent search provenance");
            }
            search_page_envelope(
                context.provenance.dojo_id,
                context.provenance.window_id,
                &page,
            )
        }
        Response::ScrollbackResyncRequired {
            current_revision,
            history_generation,
        } if matches!(command, MachineHistory::Scrollback { .. }) => read_resync_envelope(
            command.operation(),
            TerminalReadProvenanceV1 {
                terminal_revision: current_revision,
                history_generation,
                ..context.provenance
            },
            if history_generation == context.provenance.history_generation {
                ReadResyncReasonV1::StaleRevision
            } else {
                ReadResyncReasonV1::HistoryReplaced
            },
        ),
        Response::SearchResyncRequired {
            current_revision,
            history_generation,
        } if matches!(command, MachineHistory::Search { .. }) => read_resync_envelope(
            command.operation(),
            TerminalReadProvenanceV1 {
                terminal_revision: current_revision,
                history_generation,
                ..context.provenance
            },
            if history_generation == context.provenance.history_generation {
                ReadResyncReasonV1::StaleRevision
            } else {
                ReadResyncReasonV1::HistoryReplaced
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
    if schema_major != 1 {
        write_machine_read_failure(
            operation,
            CliErrorCodeV1::UnsupportedSchema,
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
                    CliErrorCodeV1::Timeout,
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
                (CliErrorCodeV1::Timeout, true)
            } else if let Some(protocol) = protocol_error(&error) {
                public_error_code(protocol.code)
            } else if error.to_string().contains("continuation cursor") {
                (CliErrorCodeV1::InvalidArgument, false)
            } else if error.to_string().contains("not found") {
                (CliErrorCodeV1::NotFound, false)
            } else {
                (CliErrorCodeV1::Internal, false)
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
) -> Result<CliEnvelopeV1> {
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
    for dojo in topology.lair.dojos() {
        for window in &dojo.windows {
            if window.root.find_splint(splint_id).is_some() {
                let runtime = topology
                    .runtimes
                    .iter()
                    .find(|runtime| runtime.splint_id == splint_id)
                    .context("validated topology omitted Splint runtime")?;
                identity = Some((
                    dojo.id,
                    window.id,
                    runtime
                        .live_incarnation
                        .context("selected Splint does not have a live process")?,
                ));
            }
        }
    }
    let (dojo_id, window_id, incarnation) = identity.context("requested Splint was not found")?;
    let attached = connection
        .request_with_deadline(
            Request::Attach {
                splint_id,
                incarnation,
                scrollback_rows: 0,
            },
            deadline.saturating_sub(started.elapsed()),
        )
        .await?;
    let Response::Attached {
        subscription_id,
        snapshot,
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
    terminal_snapshot_envelope(dojo_id, window_id, &snapshot)
}

async fn run_machine_snapshot(
    splint_id: SplintId,
    expected_incarnation: Option<u64>,
    schema_major: u16,
    timeout_ms: u64,
) -> Result<()> {
    const OPERATION: &str = "terminal_snapshot";
    if schema_major != 1 {
        write_machine_read_failure(
            OPERATION,
            CliErrorCodeV1::UnsupportedSchema,
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
                    CliErrorCodeV1::Timeout,
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
                (CliErrorCodeV1::Timeout, true)
            } else if let Some(protocol) = protocol_error(&error) {
                public_error_code(protocol.code)
            } else if error.to_string().contains("not found") {
                (CliErrorCodeV1::NotFound, false)
            } else if error.to_string().contains("does not have a live process")
                || error.to_string().contains("expected incarnation")
            {
                (CliErrorCodeV1::StaleIncarnation, false)
            } else {
                (CliErrorCodeV1::Internal, false)
            };
            write_machine_read_failure(OPERATION, code, bounded_public_message(&error), retryable)?;
            Err(error)
        }
    }
}

async fn run_machine_ping(schema_major: u16, timeout_ms: u64) -> Result<()> {
    if schema_major != 1 {
        let envelope = PingEnvelopeV1::failure(
            1,
            CliErrorCodeV1::UnsupportedSchema,
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
                    .map_or((CliErrorCodeV1::Internal, true), |protocol| {
                        public_error_code(protocol.code)
                    });
                write_json_document(&PingEnvelopeV1::failure(
                    1,
                    code,
                    bounded_public_message(&error),
                    retryable,
                )?)?;
                return Err(error);
            }
            Err(_) => {
                write_json_document(&PingEnvelopeV1::failure(
                    1,
                    CliErrorCodeV1::Timeout,
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
        Ok(Response::Pong) => write_json_document(&PingEnvelopeV1::success(1)?),
        Ok(_) => {
            write_json_document(&PingEnvelopeV1::failure(
                1,
                CliErrorCodeV1::Internal,
                "splinterd returned an unexpected ping response",
                false,
            )?)?;
            bail!("splinterd returned an unexpected ping response")
        }
        Err(error) => {
            let timed_out = error.to_string().contains("timed out");
            let code = if timed_out {
                CliErrorCodeV1::Timeout
            } else {
                CliErrorCodeV1::Internal
            };
            write_json_document(&PingEnvelopeV1::failure(
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
    let Response::Splint { runtime } = runtime else {
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
                incarnation,
                scrollback_rows: 0,
            },
            setup_deadline,
        )
        .await?;
    let Response::Attached {
        subscription_id,
        snapshot,
    } = response
    else {
        bail!("splinterd returned an unexpected attach response");
    };
    if snapshot.splint_id != splint_id || snapshot.incarnation != incarnation {
        bail!("splinterd returned inconsistent terminal subscription identity");
    }
    write_json_document(&CliEventV1::terminal_snapshot(1, 1, &snapshot, false)?)?;
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
            write_json_document(&CliEventV1::terminal_resync(
                1,
                public_sequence,
                splint_id,
                incarnation,
                revision,
                Some(history_generation),
                ResyncReasonV1::RevisionGap,
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
                write_json_document(&CliEventV1::terminal_snapshot(
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
                        write_json_document(&CliEventV1::terminal_resync(
                            1,
                            public_sequence,
                            splint_id,
                            incarnation,
                            revision,
                            Some(history_generation),
                            ResyncReasonV1::HistoryReplaced,
                        )?)?;
                        return Ok(());
                    }
                }
                write_json_document(&CliEventV1::terminal_update(
                    1,
                    public_sequence,
                    splint_id,
                    incarnation,
                    revision,
                    history_generation,
                )?)?;
            }
            SubscriptionEvent::ResyncRequired { current_revision } => {
                write_json_document(&CliEventV1::terminal_resync(
                    1,
                    public_sequence,
                    splint_id,
                    incarnation,
                    current_revision,
                    Some(history_generation),
                    ResyncReasonV1::SubscriberStalled,
                )?)?;
                return Ok(());
            }
            SubscriptionEvent::AccessRevoked { grant_id } => {
                write_json_document(&CliEventV1::access_revoked(
                    1,
                    public_sequence,
                    splint_id,
                    incarnation,
                    grant_id,
                )?)?;
                return Ok(());
            }
            SubscriptionEvent::Exited { code, signal } => {
                write_json_document(&CliEventV1::exited(
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
    write_json_document(&CliEventV1::topology_snapshot(1, 1, &snapshot)?)?;
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
            write_json_document(&CliEventV1::topology_resync(
                1,
                public_sequence,
                event_revision,
                ResyncReasonV1::RevisionGap,
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
                write_json_document(&CliEventV1::topology_changed(
                    1,
                    public_sequence,
                    change.kind,
                    &change.snapshot,
                )?)?;
            }
            SubscriptionEvent::TopologyResyncRequired { current_revision } => {
                write_json_document(&CliEventV1::topology_resync(
                    1,
                    public_sequence,
                    current_revision,
                    ResyncReasonV1::SubscriberStalled,
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
    let Response::Splint { runtime } = runtime else {
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
    write_json_document(&CliEventV1::control_snapshot(1, 1, status)?)?;
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
            write_json_document(&CliEventV1::control_resync(
                1,
                public_sequence,
                splint_id,
                incarnation,
                ResyncReasonV1::RevisionGap,
            )?)?;
            return Ok(());
        }
        private_sequence = private_sequence
            .checked_add(1)
            .context("private sequence exhausted")?;
        let record = match event {
            SubscriptionEvent::ControlStatusChanged { status } => {
                CliEventV1::control_status_changed(1, public_sequence, status)?
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
                CliEventV1::control_transfer_requested(
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
                CliEventV1::control_transfer_resolved(
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
    if schema_major != 1 {
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
        Command::Window { .. }
        | Command::Launch { .. }
        | Command::Consent
        | Command::Policy { .. }
        | Command::Relay { .. } => {
            unreachable!("graphical, policy, or relay command returned before daemon connection")
        }
        Command::Ping => print_response(connection.request(Request::Ping).await?),
        Command::List => print_response(connection.request(Request::ListDojos).await?),
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
        Command::NewWindow {
            dojo_id,
            title,
            cwd,
            command,
        } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::NewWindow {
                        expected_topology_revision,
                        dojo_id,
                        title,
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
        Command::CloseWindow { window_id, .. } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::CloseWindow {
                        expected_topology_revision,
                        window_id,
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
        Command::RenameWindow { window_id, title } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::RenameWindow {
                        expected_topology_revision,
                        window_id,
                        title,
                    })
                    .await?,
            )
        }
        Command::WindowFocusHint {
            window_id,
            splint_id,
        } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::SetWindowDefaultFocus {
                        expected_topology_revision,
                        window_id,
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
        Command::RestoreWindow { window_id } => {
            let expected_topology_revision = connection.topology_revision().await?;
            print_response(
                connection
                    .request(Request::RestoreWindow {
                        expected_topology_revision,
                        window_id,
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
                        incarnation,
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
            let controller_id = connection.acquire_control(splint_id, incarnation).await?;
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
            let controller_id = connection.acquire_control(splint_id, incarnation).await?;
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
    Request::CreateDojo {
        expected_topology_revision,
        name,
        launch: launch_parameters(cwd, command, config),
    }
}

fn collect_choices(
    node: &splinterm_core::LayoutNode,
    dojo: &str,
    window: &str,
    choices: &mut Vec<(SplintId, String)>,
) {
    match node {
        splinterm_core::LayoutNode::Leaf(splint) => choices.push((
            splint.id,
            format!("{dojo} / {window} / {} ({:?})", splint.title, splint.state),
        )),
        splinterm_core::LayoutNode::Branch { first, second, .. } => {
            collect_choices(first, dojo, window, choices);
            collect_choices(second, dojo, window, choices);
        }
    }
}

fn choose_session(dojos: &[splinterm_core::Dojo], allow_new: bool) -> Result<Option<SplintId>> {
    if !io::stdin().is_terminal() {
        let guidance = if allow_new {
            "pass --splint-id <UUID> to attach or --new to create"
        } else {
            "pass a Splint UUID explicitly"
        };
        bail!("session selection requires an interactive terminal; {guidance}");
    }
    let mut choices = Vec::new();
    for dojo in dojos {
        for window in &dojo.windows {
            collect_choices(&window.root, &dojo.name, &window.title, &mut choices);
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

fn select_window_from(
    dojos: &[splinterm_core::Dojo],
    selection: (DojoId, WindowId),
) -> Result<splinterm_core::Window> {
    let (dojo_id, window_id) = selection;
    let dojo = dojos
        .iter()
        .find(|dojo| dojo.id == dojo_id)
        .context("selected Dojo is not present in the current Lair")?;
    let window = dojo
        .windows
        .iter()
        .find(|window| window.id == window_id)
        .context("selected window does not belong to the selected Dojo")?;
    window
        .root
        .find_splint(window.default_focus)
        .context("selected window has an invalid default-focus hint")?;
    Ok(window.clone())
}

fn window_containing(
    dojos: &[splinterm_core::Dojo],
    splint_id: SplintId,
) -> Option<splinterm_core::Window> {
    dojos
        .iter()
        .flat_map(|dojo| &dojo.windows)
        .find(|window| window.root.find_splint(splint_id).is_some())
        .cloned()
}

async fn select_window(selection: Option<(DojoId, WindowId)>) -> Result<splinterm_core::Window> {
    let mut connection = Connection::connect().await?;
    let Response::Dojos { dojos } = connection.request(Request::ListDojos).await? else {
        bail!("splinterd did not return its session list");
    };
    if let Some(selection) = selection {
        select_window_from(&dojos, selection)
    } else {
        let splint_id = choose_session(&dojos, false)?.context("no Splint was selected")?;
        window_containing(&dojos, splint_id)
            .context("selected Splint is not present in a daemon window")
    }
}

async fn launch(
    name: String,
    cwd: PathBuf,
    splint_id: Option<SplintId>,
    create_new: bool,
    command: Vec<String>,
    config: AppConfig,
) -> Result<()> {
    let mut connection = Connection::connect()
        .await
        .context("splinterd is unavailable; start splinterd.service or run splinterd")?;
    let Response::Dojos { dojos } = connection.request(Request::ListDojos).await? else {
        bail!("splinterd did not return its session list");
    };
    let attach = if let Some(splint_id) = splint_id {
        Some(splint_id)
    } else if create_new || dojos.is_empty() || !command.is_empty() {
        None
    } else {
        choose_session(&dojos, true)?
    };
    let selected = if let Some(splint_id) = attach {
        if !command.is_empty() {
            bail!("cannot execute a new command while attaching an existing Splint");
        }
        splint_id
    } else {
        let expected = connection.topology_revision().await?;
        let Response::DojoCreated { dojo, .. } = connection
            .request(create_request(expected, name, cwd, command, &config))
            .await?
        else {
            bail!("splinterd did not create the requested terminal");
        };
        let window = dojo
            .windows
            .first()
            .context("new dojo did not contain a window")?;
        match &window.root {
            splinterm_core::LayoutNode::Leaf(splint) => splint.id,
            splinterm_core::LayoutNode::Branch { .. } => {
                bail!("new dojo did not contain exactly one Splint")
            }
        }
    };
    drop(connection);
    run_live_window(config, selected).await
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
    if last_sequence.checked_add(1) != Some(sequence) {
        return EventAction::Resynchronize;
    }
    match event {
        SubscriptionEvent::Snapshot { snapshot } => EventAction::Snapshot { sequence, snapshot },
        SubscriptionEvent::Update { update } => EventAction::Update { sequence, update },
        SubscriptionEvent::ResyncRequired { .. } => EventAction::Resynchronize,
        SubscriptionEvent::AccessRevoked { .. } | SubscriptionEvent::Exited { .. } => {
            EventAction::Shutdown
        }
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
    } = connection
        .request(Request::Attach {
            splint_id,
            incarnation,
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
            incarnation,
        })
        .await?
    {
        Response::AuthorizationStatus {
            grants,
            persistent: _,
            development_bypass,
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
            Response::ScrollbackPage { page } => page,
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

async fn ensure_pane_control(
    control: &mut Connection,
    active_controller: &mut Option<u64>,
    prepared_resize: &mut Option<PaneResize>,
    updates: &mpsc::Sender<WindowUpdate>,
    splint_id: SplintId,
    incarnation: u64,
    apply_prepared_resize: bool,
) -> Result<u64> {
    if let Some(controller_id) = *active_controller {
        return Ok(controller_id);
    }
    let controller_id = control.acquire_control(splint_id, incarnation).await?;
    *active_controller = Some(controller_id);
    let _ = updates.send(WindowUpdate::Control(true)).await;
    if apply_prepared_resize {
        if let Some((columns, rows, pixel_width, pixel_height)) = prepared_resize.take() {
            if !matches!(
                control
                    .request(Request::Resize {
                        controller_id,
                        splint_id,
                        incarnation,
                        columns,
                        rows,
                        pixel_width,
                        pixel_height,
                    })
                    .await?,
                Response::Acknowledged
            ) {
                bail!("splinterd did not acknowledge prepared pane resize");
            }
        }
    }
    Ok(controller_id)
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

async fn active_resize_request(
    control: &mut Connection,
    active_controller: &mut Option<u64>,
    prepared_resize: &mut Option<PaneResize>,
    updates: &mpsc::Sender<WindowUpdate>,
    identity: (SplintId, u64),
    resize: PaneResize,
    resize_delay_ms: u64,
) -> Result<Request> {
    if resize_delay_ms > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(resize_delay_ms)).await;
    }
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
    Ok(Request::Resize {
        controller_id,
        splint_id,
        incarnation,
        columns: resize.0,
        rows: resize.1,
        pixel_width: resize.2,
        pixel_height: resize.3,
    })
}

async fn handle_control_event(
    frame: ServerFrame,
    subscription_id: u64,
    active_controller: &mut Option<u64>,
    updates: &mpsc::Sender<WindowUpdate>,
) -> Result<()> {
    let ServerFrame::Event {
        subscription_id: event_subscription,
        event,
        ..
    } = frame
    else {
        bail!("splinterd sent an unexpected control-subscription frame");
    };
    if event_subscription != subscription_id {
        return Ok(());
    }
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
            }
            let _ = updates
                .send(WindowUpdate::ControlTransferResolved(outcome))
                .await;
        }
        _ => {}
    }
    Ok(())
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
            let command = tokio::select! {
                frame = control.next_server_frame() => {
                    handle_control_event(
                        frame?,
                        control_subscription,
                        &mut active_controller,
                        &outputs.updates,
                    ).await?;
                    continue;
                }
                command = commands.recv() => command,
            };
            let Some(command) = command else { break };
            let request = match command {
                WindowCommand::Input(bytes) => {
                    let controller_id = ensure_pane_control(
                        &mut control,
                        &mut active_controller,
                        &mut prepared_resize,
                        &outputs.updates,
                        splint_id,
                        incarnation,
                        true,
                    )
                    .await?;
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
                    active_resize_request(
                        &mut control,
                        &mut active_controller,
                        &mut prepared_resize,
                        &outputs.updates,
                        (splint_id, incarnation),
                        (columns, rows, pixel_width, pixel_height),
                        resize_delay_ms,
                    )
                    .await?
                }
                WindowCommand::PrepareResize {
                    columns,
                    rows,
                    pixel_width,
                    pixel_height,
                } => {
                    prepared_resize = Some((columns, rows, pixel_width, pixel_height));
                    continue;
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
                        Response::ControlGranted { controller_id } => Some(controller_id),
                        _ => bail!("splinterd did not grant forced control"),
                    };
                    let _ = outputs.updates.send(WindowUpdate::Control(true)).await;
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
                        Response::SearchResults { page } => {
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
            if !matches!(control.request(request).await?, Response::Acknowledged) {
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

async fn watch_theme(
    path: PathBuf,
    mut current: ResolvedTheme,
    updates: mpsc::Sender<WindowUpdate>,
) {
    let mut poll = tokio::time::interval(std::time::Duration::from_millis(500));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        poll.tick().await;
        match load_theme(&path) {
            Ok(next) if next != current => {
                current = next;
                if updates.send(WindowUpdate::Theme(next)).await.is_err() {
                    break;
                }
            }
            Ok(_) => {}
            Err(error) => eprintln!("splinterm theme reload rejected: {error:#}"),
        }
    }
}

struct PreparedPane {
    options: WindowPaneOptions,
    updates: mpsc::Sender<WindowUpdate>,
    task: tokio::task::JoinHandle<Result<()>>,
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

async fn prepare_live_pane(config: &AppConfig, splint_id: SplintId) -> Result<PreparedPane> {
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
    let mut control = Connection::connect().await?;
    if control.live_incarnation(splint_id).await? != incarnation {
        bail!("control connection observed a different process incarnation");
    }
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
        None,
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
    ));
    Ok(PreparedPane {
        options: WindowPaneOptions {
            snapshot,
            updates: receiver,
            commands: command_sender,
            authority,
            controlled: false,
        },
        updates,
        task,
    })
}

#[allow(
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
                if updates.send(WindowUpdate::Snapshot(attachment.snapshot.clone())).await.is_err() {
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
                        if updates.send(WindowUpdate::Snapshot(snapshot)).await.is_err() {
                            return controller.await.context("pane controller task failed")?;
                        }
                        last_sequence = sequence;
                    }
                    EventAction::Update { sequence, update }
                        if update_advances_from(&update, last_revision) => {
                        last_revision = update.revision;
                        if updates.send(WindowUpdate::Update(update)).await.is_err() {
                            return controller.await.context("pane controller task failed")?;
                        }
                        last_sequence = sequence;
                    }
                    EventAction::Update { .. } | EventAction::Resynchronize => {
                        attachment = resynchronize(
                            &mut connection, attachment.subscription_id, splint_id, incarnation,
                        ).await?;
                        last_revision = attachment.snapshot.revision;
                        last_sequence = 0;
                        if updates.send(WindowUpdate::Snapshot(attachment.snapshot.clone())).await.is_err() {
                            return controller.await.context("pane controller task failed")?;
                        }
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

fn window_root_from_topology(
    snapshot: &splinterm_protocol::TopologySnapshot,
    window_id: WindowId,
) -> Result<LayoutNode> {
    snapshot
        .lair
        .dojos()
        .flat_map(|dojo| &dojo.windows)
        .find(|window| window.id == window_id)
        .map(|window| window.root.clone())
        .context("edited window is absent from committed topology")
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

async fn inspect_window_state(
    connection: &mut Connection,
    window_id: WindowId,
) -> Result<(TopologyRevision, LayoutNode)> {
    let Response::Topology { snapshot } = connection.request(Request::InspectTopology).await?
    else {
        bail!("splinterd did not return topology after edit");
    };
    snapshot
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    let root = window_root_from_topology(&snapshot, window_id)?;
    Ok((snapshot.revision, root))
}

async fn apply_topology_command(
    connection: &mut Connection,
    config: &AppConfig,
    root: &LayoutNode,
    expected_topology_revision: TopologyRevision,
    command: WindowTopologyCommand,
) -> Result<()> {
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
        WindowTopologyCommand::Close { target } => Request::CloseSplint {
            expected_topology_revision,
            splint_id: target,
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
    };
    match connection.request(request).await? {
        Response::TopologyCommitted { .. } | Response::SplintStarted { .. } => Ok(()),
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
        let pane = prepare_live_pane(config, splint_id).await?;
        pane_tasks.push(pane.task);
        added.push(pane.options);
    }
    if updates
        .send(WindowTopologyUpdate::Apply {
            layout: next.clone(),
            added,
            removed,
        })
        .await
        .is_err()
    {
        return Ok(false);
    }
    *root = next;
    Ok(true)
}

async fn run_topology_manager(
    config: AppConfig,
    window_id: WindowId,
    mut root: LayoutNode,
    mut commands: mpsc::Receiver<WindowTopologyCommand>,
    updates: mpsc::Sender<WindowTopologyUpdate>,
    mut pane_tasks: Vec<tokio::task::JoinHandle<Result<()>>>,
) -> Result<()> {
    let mut connection = Connection::connect().await?;
    let mut poll = tokio::time::interval(std::time::Duration::from_millis(250));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let command = tokio::select! {
            command = commands.recv() => command,
            _ = poll.tick() => None,
        };
        let (revision, authoritative) = match inspect_window_state(&mut connection, window_id).await
        {
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
            &mut root,
            authoritative,
            &updates,
            &mut pane_tasks,
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
        if let Err(error) =
            apply_topology_command(&mut connection, &config, &root, revision, command).await
        {
            eprintln!("splinterm topology edit rejected: {error:#}");
        }
    }
    for task in pane_tasks {
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
    window_model: splinterm_core::Window,
) -> Result<()> {
    renderer::configure(RendererOptions {
        font: config.font.clone(),
        font_size: config.font_size,
        font_sizing_policy: config.font_sizing_policy,
        physical_dpi: 96.0,
        padding: config.padding,
        background_alpha: config.background_alpha,
    })?;
    let theme = load_theme(&config.theme_path).unwrap_or_default();
    let mut ids = Vec::new();
    layout_splint_ids(&window_model.root, &mut ids);
    let mut prepared = Vec::with_capacity(ids.len());
    for splint_id in ids {
        prepared.push(prepare_live_pane(&config, splint_id).await?);
    }
    let theme_senders = prepared
        .iter()
        .map(|pane| pane.updates.clone())
        .collect::<Vec<_>>();
    let theme_path = config.theme_path.clone();
    let theme_task = tokio::spawn(async move {
        let mut current = theme;
        let mut poll = tokio::time::interval(std::time::Duration::from_millis(500));
        loop {
            poll.tick().await;
            if let Ok(next) = load_theme(&theme_path) {
                if next != current {
                    current = next;
                    for sender in &theme_senders {
                        let _ = sender.send(WindowUpdate::Theme(next)).await;
                    }
                }
            }
        }
    });
    let mut panes = Vec::with_capacity(prepared.len());
    let mut tasks = Vec::with_capacity(prepared.len());
    for pane in prepared {
        panes.push(pane.options);
        tasks.push(pane.task);
    }
    let (topology_commands, topology_command_receiver) = mpsc::channel(8);
    let (topology_update_sender, topology_updates) = mpsc::channel(4);
    let topology_smoke =
        spawn_topology_smoke(topology_commands.clone(), window_model.default_focus)?;
    let window_config = config.clone();
    let root = window_model.root;
    let manager_root = root.clone();
    let active_splint = window_model.default_focus;
    let window_id = window_model.id;
    let topology_manager = tokio::spawn(run_topology_manager(
        config,
        window_id,
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
    renderer::configure(RendererOptions {
        font: config.font.clone(),
        font_size: config.font_size,
        font_sizing_policy: config.font_sizing_policy,
        physical_dpi: 96.0,
        padding: config.padding,
        background_alpha: config.background_alpha,
    })?;
    let theme = load_theme(&config.theme_path).unwrap_or_else(|error| {
        eprintln!("splinterm theme: {error:#}; using safe fallback palette");
        ResolvedTheme::default()
    });
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
    let mut control = Connection::connect().await?;
    let control_incarnation = control.live_incarnation(splint_id).await?;
    if control_incarnation != incarnation {
        bail!("control connection observed a different process incarnation");
    }
    let controller_id = control.acquire_control(splint_id, incarnation).await?;
    println!("Controller lease {controller_id} granted for live Splint");
    let (updates, receiver) = mpsc::channel(WINDOW_UPDATE_QUEUE);
    let _theme_watcher = tokio::spawn(watch_theme(
        config.theme_path.clone(),
        theme,
        updates.clone(),
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
                if updates
                    .send(WindowUpdate::Snapshot(attachment.snapshot.clone()))
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
                            if updates.send(WindowUpdate::Snapshot(snapshot)).await.is_err() {
                                let window_result = window.await.context("Wayland window task failed")?;
                                controller.await.context("window controller task failed")??;
                                return window_result;
                            }
                            last_sequence = sequence;
                        }
                        EventAction::Update { sequence, update }
                            if update_advances_from(&update, last_revision) => {
                            last_revision = update.revision;
                            if updates.send(WindowUpdate::Update(update)).await.is_err() {
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
                            if updates
                                .send(WindowUpdate::Snapshot(attachment.snapshot.clone()))
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
                            if updates
                                .send(WindowUpdate::Snapshot(attachment.snapshot.clone()))
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
                        EventAction::Shutdown => {
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

fn print_dojos(dojos: Vec<splinterm_core::Dojo>) {
    for dojo in dojos {
        let splints: usize = dojo
            .windows
            .iter()
            .map(|window| window.root.splint_count())
            .sum();
        println!(
            "{}  {}  {} window(s)  {splints} Splint(s)",
            dojo.id,
            dojo.name,
            dojo.windows.len()
        );
        for window in &dojo.windows {
            println!(
                "  window {}  {}  default-focus {}",
                window.id, window.title, window.default_focus
            );
            print_splint_ids(&window.root);
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the CLI keeps exhaustive human rendering for every private protocol response"
)]
fn print_response(response: Response) -> Result<()> {
    match response {
        Response::Pong => println!("splinterd is awake"),
        Response::Dojos { dojos } if dojos.is_empty() => println!("No dojos in the lair."),
        Response::Dojos { dojos } => print_dojos(dojos),
        Response::DojoCreated { dojo, .. } => println!("Created dojo '{}'.", dojo.name),
        Response::Topology { snapshot } => println!(
            "Topology revision {}: {} dojo(s), {} Splint(s)",
            snapshot.revision.get(),
            snapshot.lair.dojos().count(),
            snapshot.runtimes.len()
        ),
        Response::TopologySubscribed {
            subscription_id,
            snapshot,
        } => println!(
            "Topology subscription {subscription_id} started at revision {}.",
            snapshot.revision.get()
        ),
        Response::Splint { runtime } => println!(
            "Splint {:?}: {:?}, incarnation={:?}, exit={:?}",
            runtime.splint_id, runtime.lifecycle, runtime.live_incarnation, runtime.exit_status
        ),
        Response::Attached { snapshot, .. } => print_snapshot(&snapshot),
        Response::ScrollbackPage { page } => println!(
            "Scrollback page: {} row(s), has_older={}",
            page.rows.len(),
            page.has_older
        ),
        Response::ScrollbackResyncRequired {
            current_revision,
            history_generation,
        } => println!(
            "Scrollback resync required at revision {current_revision}, generation {history_generation}"
        ),
        Response::SearchResults { page } => println!(
            "Search page: {} match(es), continuation={}, timed_out={}",
            page.matches.len(),
            page.next_cursor.is_some(),
            page.timed_out,
        ),
        Response::SearchResyncRequired {
            current_revision,
            history_generation,
        } => println!(
            "Search resync required at revision {current_revision}, generation {history_generation}"
        ),
        Response::AccessGranted { grant } => {
            println!("Access grant {} issued.", grant.grant_id);
        }
        Response::AccessRevoked { grant } => {
            println!("Access grant {} revoked.", grant.grant_id);
        }
        Response::AuthorizationStatus {
            grants,
            persistent,
            development_bypass,
        } => {
            println!(
                "{} active grant(s), {} persistent rule(s); development bypass={development_bypass}",
                grants.len(),
                persistent.len()
            );
        }
        Response::ControlGranted { controller_id } => {
            println!("Controller lease {controller_id} granted.");
        }
        Response::ControlSubscribed {
            subscription_id,
            status,
        } => println!(
            "Control subscription {subscription_id}: controlled={}, locally_owned={}",
            status.controlled, status.locally_owned,
        ),
        Response::ControlTransferPending { transfer_id } => {
            println!("Control transfer {transfer_id} pending.");
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
        Response::WindowStarted {
            window_id,
            splint_id,
            incarnation,
            topology_revision,
        } => println!(
            "Window {window_id:?} started with Splint {splint_id} incarnation {incarnation} at revision {}.",
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

fn print_splint_ids(node: &splinterm_core::LayoutNode) {
    match node {
        splinterm_core::LayoutNode::Leaf(splint) => {
            println!("  {}  {}  {:?}", splint.id, splint.title, splint.state);
        }
        splinterm_core::LayoutNode::Branch { first, second, .. } => {
            print_splint_ids(first);
            print_splint_ids(second);
        }
    }
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
        let dojo_id = DojoId::new();
        let window_id = WindowId::new();
        let parsed = Cli::try_parse_from([
            "splinterm",
            "window",
            "--dojo-id",
            &dojo_id.to_string(),
            "--window-id",
            &window_id.to_string(),
        ])
        .unwrap();
        assert!(matches!(
            parsed.command,
            Command::Window {
                dojo_id: Some(parsed_dojo),
                window_id: Some(parsed_window),
            } if parsed_dojo == dojo_id && parsed_window == window_id
        ));
        assert!(
            Cli::try_parse_from(["splinterm", "window", "--window-id", &window_id.to_string(),])
                .is_err()
        );
    }

    #[test]
    fn window_selection_uses_only_its_local_hint() {
        let mut first = splinterm_core::Dojo::new("first", PathBuf::from("/tmp"));
        let first_dojo = first.id;
        let first_window = first.windows[0].id;
        let first_hint = first.windows[0].default_focus;
        let second = splinterm_core::Dojo::new("second", PathBuf::from("/tmp"));
        let second_window = second.windows[0].id;
        let second_hint = second.windows[0].default_focus;

        let selected =
            select_window_from(&[first.clone(), second.clone()], (first_dojo, first_window))
                .unwrap();
        assert_eq!(selected.default_focus, first_hint);
        assert_eq!(selected.root, first.windows[0].root);
        assert_ne!(first_hint, second_hint);
        assert!(select_window_from(&[first.clone(), second], (first_dojo, second_window)).is_err());

        first.windows[0].default_focus = SplintId::new();
        assert!(select_window_from(&[first], (first_dojo, first_window)).is_err());
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
        let Request::CreateDojo { launch, .. } = request else {
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
            EventAction::Shutdown
        );
    }
}
