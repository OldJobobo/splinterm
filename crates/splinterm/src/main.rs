use std::{
    collections::{HashMap, HashSet},
    env,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use splinterm::{
    AuthorityStatus, SessionPickerItem, WindowCommand, WindowDojoIdentity, WindowOptions,
    WindowPaneOptions, WindowTopologyCommand, WindowTopologyUpdate, WindowUpdate,
    automation::{
        CliEnvelopeV2, CliErrorCodeV2, CliEventV2, Connection, ImageContentLeaseSet,
        MAX_RENDERER_IMAGE_RESIDENT_BYTES, MutationIdentityV2, PingEnvelopeV2, ReadResyncReasonV2,
        ResyncReasonV2, SharedImageContentCache, TerminalContinuationV2, TerminalReadProvenanceV2,
        audit_page_envelope, authorization_status_envelope, committed_mutation_envelope,
        created_mutation_envelope, decode_terminal_cursor, inspect_splint_envelope,
        inspect_topology_envelope, kill_envelope, list_lairs_envelope, process_started_envelope,
        protocol_error, public_error_code, read_resync_envelope, response_protocol_error,
        restore_many_envelope, revoke_envelope, scrollback_page_envelope, search_page_envelope,
        terminal_action_envelope, terminal_snapshot_envelope, write_json_document,
    },
    config::{AppConfig, ConfigLoad, load_default},
    renderer::{self, RendererOptions},
    run_window,
    session_picker::{SessionEntry, collect_sessions},
    tab::{DojoTab, OpenTabOutcome, WindowTabSet},
};
use splinterm_core::{
    Axis, DojoId, LairId, LayoutNode, SplintId, SplintState, SplitRatio, SplitSide,
    TopologyRevision,
};
use splinterm_protocol::{
    AccessGrant, AccessScope, ControlMode, ControlTransferOutcome, ErrorCode, HistoryTransition,
    LaunchParameters, Request, Response, ServerFrame, SubscriptionEvent, TerminalSnapshot,
    TerminalUpdate,
    perf_trace::{PerfTraceEvent, emit_perf_trace, perf_trace_enabled},
};
use tokio::sync::mpsc;

mod app;

use app::{
    ControllerOutputs, EventAction, PaneTask, attach, classify_subscription_event,
    layout_splint_ids, lease_snapshot_images, lease_update_images, load_authority_status,
    pane_claims_initial_control, prepare_live_pane, resolve_image_contents, resolve_update_images,
    resynchronize, run_controller, update_advances_from, validate_attached_snapshot,
};
#[cfg(test)]
use app::{
    PendingPaneResize, handle_control_event, optional_pane_controller, queue_pane_resize,
    resolved_resize_request, terminal_action_matches, validate_scrollback_page_response,
};
use app::{
    ThemeUpdateSink, confirm_kill, create_request, launch, launch_parameters, load_startup_theme,
    print_lairs, print_response, recent_dojo_ids, remember_dojo, reopen_recent, run_consent_client,
    run_policy_command, run_relay_command, run_reset_command, run_sessions, select_dojo,
    select_dojo_from, session_picker_item, usage_error, watch_theme,
};
use app::{
    machine_exit_code, require_expected_incarnation, require_incarnation, run_machine_command,
    run_machine_subscription,
};

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
struct PendingTopologyFocus {
    splint_id: SplintId,
    revision: TopologyRevision,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TopologyCommandOutcome {
    Updated {
        pending_focus: Option<PendingTopologyFocus>,
    },
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
                return Ok(TopologyCommandOutcome::Updated {
                    pending_focus: None,
                });
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
                        return Ok(TopologyCommandOutcome::Updated {
                            pending_focus: None,
                        });
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
    if let WindowTopologyCommand::Close {
        dojo_id: target_dojo,
        target,
    } = command
    {
        anyhow::ensure!(
            target_dojo == dojo_id,
            "topology close targeted another Dojo"
        );
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
        WindowTopologyCommand::Split {
            dojo_id: target_dojo,
            target,
            axis,
        } => {
            anyhow::ensure!(
                target_dojo == dojo_id,
                "topology split targeted another Dojo"
            );
            Request::SplitSplint {
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
            }
        }
        WindowTopologyCommand::AdjustRatio {
            dojo_id: target_dojo,
            target,
            delta,
        } => {
            anyhow::ensure!(target_dojo == dojo_id, "ratio edit targeted another Dojo");
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
        | WindowTopologyCommand::OpenDojo { .. }
        | WindowTopologyCommand::NewLair
        | WindowTopologyCommand::NewDojo { .. }
        | WindowTopologyCommand::ActivateTab { .. }
        | WindowTopologyCommand::CloseTab { .. } => {
            unreachable!("session commands are handled by the topology manager")
        }
    };
    topology_command_outcome(connection.request(request).await?)
}

fn topology_command_outcome(response: Response) -> Result<TopologyCommandOutcome> {
    match response {
        Response::SplintStarted {
            splint_id,
            topology_revision,
            ..
        } => Ok(TopologyCommandOutcome::Updated {
            pending_focus: Some(PendingTopologyFocus {
                splint_id,
                revision: topology_revision,
            }),
        }),
        Response::TopologyCommitted { .. } => Ok(TopologyCommandOutcome::Updated {
            pending_focus: None,
        }),
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

fn pending_focus_for_observation(
    pending: Option<PendingTopologyFocus>,
    observed_revision: TopologyRevision,
    added: &[SplintId],
) -> (Option<SplintId>, bool) {
    let Some(pending) = pending else {
        return (None, false);
    };
    if observed_revision < pending.revision {
        return (None, false);
    }
    (
        added
            .contains(&pending.splint_id)
            .then_some(pending.splint_id),
        true,
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "one transactional reconciliation owns identity, layout, updates, and task cleanup"
)]
async fn reconcile_window_topology(
    config: &AppConfig,
    image_cache: &SharedImageContentCache,
    dojo_id: DojoId,
    root: &mut LayoutNode,
    next: LayoutNode,
    focused: Option<SplintId>,
    updates: &mpsc::Sender<WindowTopologyUpdate>,
    pane_tasks: &mut HashMap<SplintId, PaneTask>,
) -> Result<bool> {
    if *root == next {
        return Ok(true);
    }
    let (added_ids, removed) = topology_identity_diff(root, &next);
    let mut prepared = Vec::new();
    for splint_id in added_ids {
        match prepare_live_pane(config, splint_id, image_cache.clone(), false).await {
            Ok(pane) => prepared.push((splint_id, pane)),
            Err(error) => {
                let tasks = prepared
                    .into_iter()
                    .map(|(splint_id, pane)| (splint_id, pane.task))
                    .collect();
                cancel_pane_tasks(tasks).await;
                return Err(error);
            }
        }
    }
    let mut added = Vec::with_capacity(prepared.len());
    let mut new_tasks = HashMap::with_capacity(prepared.len());
    for (splint_id, pane) in prepared {
        added.push(pane.options);
        new_tasks.insert(splint_id, pane.task);
    }
    if updates
        .send(WindowTopologyUpdate::Apply {
            dojo_id,
            layout: next.clone(),
            added,
            removed: removed.clone(),
            focused,
        })
        .await
        .is_err()
    {
        cancel_pane_tasks(new_tasks).await;
        return Ok(false);
    }
    let mut removed_tasks = HashMap::new();
    for removed_id in &removed {
        if let Some(task) = pane_tasks.remove(removed_id) {
            removed_tasks.insert(*removed_id, task);
        }
    }
    cancel_pane_tasks(removed_tasks).await;
    pane_tasks.extend(new_tasks);
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

fn window_dojo_identity(
    lair: &splinterm_core::Lair,
    dojo: &splinterm_core::Dojo,
) -> WindowDojoIdentity {
    WindowDojoIdentity {
        lair_id: lair.id,
        dojo_id: dojo.id,
        lair_name: lair.name.clone(),
        dojo_name: dojo.name.clone(),
    }
}

async fn reopenable_dojo(
    connection: &mut Connection,
    lair_id: LairId,
    dojo_id: DojoId,
) -> Result<(WindowDojoIdentity, splinterm_core::Dojo)> {
    let Response::Lairs { lairs, .. } = connection.request(Request::ListLairs).await? else {
        bail!("splinterd did not return its session list");
    };
    let lair = lairs
        .iter()
        .find(|lair| lair.id == lair_id)
        .context("selected Lair is absent")?;
    let dojo = select_dojo_from(&lairs, (lair_id, dojo_id))?;
    anyhow::ensure!(
        collect_sessions(&lairs, &[])
            .into_iter()
            .any(|entry| entry.dojo_id == dojo_id && entry.reopenable()),
        "selected session no longer has a fully running pane layout"
    );
    Ok((window_dojo_identity(lair, &dojo), dojo))
}

async fn create_daily_dojo(
    connection: &mut Connection,
    config: &AppConfig,
) -> Result<(WindowDojoIdentity, splinterm_core::Dojo)> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let expected = connection.topology_revision().await?;
    let Response::LairCreated { lair, .. } = connection
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
    let dojo = lair
        .dojos
        .first()
        .cloned()
        .context("new Lair did not contain a Dojo")?;
    Ok((window_dojo_identity(&lair, &dojo), dojo))
}

async fn create_dojo_in_lair(
    connection: &mut Connection,
    config: &AppConfig,
    lair_id: LairId,
) -> Result<(WindowDojoIdentity, splinterm_core::Dojo)> {
    let expected_topology_revision = connection.topology_revision().await?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let Response::DojoStarted { dojo_id, .. } = connection
        .request(Request::NewDojo {
            expected_topology_revision,
            lair_id,
            name: format!("terminal-{stamp}"),
            launch: launch_parameters(
                env::current_dir().context("failed to read current directory")?,
                Vec::new(),
                config,
            ),
        })
        .await?
    else {
        bail!("splinterd did not create the requested Dojo");
    };
    reopenable_dojo(connection, lair_id, dojo_id).await
}

struct ManagedDojo {
    identity: WindowDojoIdentity,
    root: LayoutNode,
    pending_focus: Option<PendingTopologyFocus>,
    pane_tasks: HashMap<SplintId, PaneTask>,
}

async fn cancel_pane_tasks(pane_tasks: HashMap<SplintId, PaneTask>) {
    let tasks = pane_tasks.into_values().collect::<Vec<_>>();
    for task in &tasks {
        task.cancellation.cancel();
    }
    for task in tasks {
        let _ = task.task.await;
    }
}

struct PreparedManagedDojo {
    identity: WindowDojoIdentity,
    dojo: splinterm_core::Dojo,
    panes: Vec<WindowPaneOptions>,
    pane_tasks: HashMap<SplintId, PaneTask>,
}

async fn prepare_managed_dojo(
    config: &AppConfig,
    image_cache: &SharedImageContentCache,
    identity: WindowDojoIdentity,
    dojo: splinterm_core::Dojo,
) -> Result<PreparedManagedDojo> {
    anyhow::ensure!(
        dojo.root.find_splint(dojo.default_focus).is_some(),
        "target Dojo focus is absent from its layout"
    );
    let mut ids = Vec::new();
    layout_splint_ids(&dojo.root, &mut ids);
    let mut panes = Vec::with_capacity(ids.len());
    let mut pane_tasks = HashMap::with_capacity(ids.len());
    for splint_id in ids {
        match prepare_live_pane(config, splint_id, image_cache.clone(), false).await {
            Ok(pane) => {
                panes.push(pane.options);
                pane_tasks.insert(splint_id, pane.task);
            }
            Err(error) => {
                cancel_pane_tasks(pane_tasks).await;
                return Err(error);
            }
        }
    }
    Ok(PreparedManagedDojo {
        identity,
        dojo,
        panes,
        pane_tasks,
    })
}

enum TopologyManagerCommandOutcome {
    Continue,
    Stop,
    Edit(WindowTopologyCommand),
}

struct TopologyManagerState {
    tabs: WindowTabSet<ManagedDojo>,
}

const fn window_has_tab_capacity(tab_count: usize) -> bool {
    tab_count < splinterm::tab::MAX_WINDOW_TABS
}

async fn finish_managed_window_open(
    target: Result<(WindowDojoIdentity, splinterm_core::Dojo)>,
    state: &mut TopologyManagerState,
    config: &AppConfig,
    image_cache: &SharedImageContentCache,
    updates: &mpsc::Sender<WindowTopologyUpdate>,
) -> TopologyManagerCommandOutcome {
    let target_id = target.as_ref().ok().map(|(_, dojo)| dojo.id);
    let result = async {
        let (identity, dojo) = target?;
        if state.tabs.activate(dojo.id) {
            remember_dojo(dojo.id);
            updates
                .send(WindowTopologyUpdate::ActivateTab { dojo_id: dojo.id })
                .await
                .map_err(|_| anyhow::anyhow!("Wayland tab update channel closed"))?;
            return Ok(OpenTabOutcome::ActivatedExisting);
        }
        anyhow::ensure!(
            state.tabs.len() < splinterm::tab::MAX_WINDOW_TABS,
            "a Window may contain at most {} Dojo tabs",
            splinterm::tab::MAX_WINDOW_TABS
        );
        let prepared = prepare_managed_dojo(config, image_cache, identity, dojo).await?;
        let dojo_id = prepared.dojo.id;
        let lair_id = prepared.identity.lair_id;
        let (acknowledged, acknowledgement) = tokio::sync::oneshot::channel();
        if updates
            .send(WindowTopologyUpdate::OpenTab {
                identity: prepared.identity.clone(),
                layout: prepared.dojo.root.clone(),
                panes: prepared.panes,
                focused: prepared.dojo.default_focus,
                acknowledged,
            })
            .await
            .is_err()
        {
            cancel_pane_tasks(prepared.pane_tasks).await;
            bail!("Wayland tab update channel closed");
        }
        match acknowledgement.await {
            Ok(Ok(())) => {}
            Ok(Err(message)) => {
                cancel_pane_tasks(prepared.pane_tasks).await;
                bail!("Wayland rejected Dojo tab: {message}");
            }
            Err(_) => {
                cancel_pane_tasks(prepared.pane_tasks).await;
                bail!("Wayland dropped Dojo tab acknowledgement");
            }
        }
        state.tabs.open_or_activate(DojoTab::new(
            lair_id,
            dojo_id,
            ManagedDojo {
                identity: prepared.identity,
                root: prepared.dojo.root,
                pending_focus: None,
                pane_tasks: prepared.pane_tasks,
            },
        ))?;
        remember_dojo(dojo_id);
        Ok(OpenTabOutcome::Opened)
    }
    .await;
    match result {
        Ok(_) => TopologyManagerCommandOutcome::Continue,
        Err(error) => {
            let _ = updates
                .send(WindowTopologyUpdate::TabFailed {
                    dojo_id: target_id,
                    message: format!("{error:#}"),
                })
                .await;
            TopologyManagerCommandOutcome::Continue
        }
    }
}

async fn remove_frontend_tab(
    updates: &mpsc::Sender<WindowTopologyUpdate>,
    dojo_id: DojoId,
) -> bool {
    let (acknowledged, acknowledgement) = tokio::sync::oneshot::channel();
    if updates
        .send(WindowTopologyUpdate::RemoveTab {
            dojo_id,
            acknowledged,
        })
        .await
        .is_err()
    {
        return false;
    }
    acknowledgement.await.is_ok()
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
        WindowTopologyCommand::OpenDojo {
            lair_id,
            dojo_id: target_id,
        } => {
            let target = reopenable_dojo(connection, lair_id, target_id).await;
            finish_managed_window_open(target, state, config, image_cache, updates).await
        }
        WindowTopologyCommand::NewLair => {
            if !window_has_tab_capacity(state.tabs.len()) {
                let _ = updates
                    .send(WindowTopologyUpdate::TabFailed {
                        dojo_id: None,
                        message: format!(
                            "a Window may contain at most {} Dojo tabs",
                            splinterm::tab::MAX_WINDOW_TABS
                        ),
                    })
                    .await;
                return TopologyManagerCommandOutcome::Continue;
            }
            let target = create_daily_dojo(connection, config).await;
            finish_managed_window_open(target, state, config, image_cache, updates).await
        }
        WindowTopologyCommand::NewDojo { lair_id } => {
            if !window_has_tab_capacity(state.tabs.len()) {
                let _ = updates
                    .send(WindowTopologyUpdate::TabFailed {
                        dojo_id: None,
                        message: format!(
                            "a Window may contain at most {} Dojo tabs",
                            splinterm::tab::MAX_WINDOW_TABS
                        ),
                    })
                    .await;
                return TopologyManagerCommandOutcome::Continue;
            }
            let target = create_dojo_in_lair(connection, config, lair_id).await;
            finish_managed_window_open(target, state, config, image_cache, updates).await
        }
        WindowTopologyCommand::ActivateTab { dojo_id } => {
            if state.tabs.activate(dojo_id)
                && updates
                    .send(WindowTopologyUpdate::ActivateTab { dojo_id })
                    .await
                    .is_err()
            {
                return TopologyManagerCommandOutcome::Stop;
            }
            TopologyManagerCommandOutcome::Continue
        }
        WindowTopologyCommand::CloseTab { dojo_id } => {
            if let Some(removed) = state.tabs.close(dojo_id) {
                let acknowledged = remove_frontend_tab(updates, dojo_id).await;
                cancel_pane_tasks(removed.value.pane_tasks).await;
                if !acknowledged {
                    return TopologyManagerCommandOutcome::Stop;
                }
                if state.tabs.is_empty() {
                    let _ = updates.send(WindowTopologyUpdate::Closed).await;
                    return TopologyManagerCommandOutcome::Stop;
                }
            }
            TopologyManagerCommandOutcome::Continue
        }
        command => TopologyManagerCommandOutcome::Edit(command),
    }
}

async fn inspect_managed_topology(
    connection: &mut Connection,
) -> Result<splinterm_protocol::TopologySnapshot> {
    let Response::Topology { snapshot } = connection.request(Request::InspectTopology).await?
    else {
        bail!("splinterd did not return topology for Window reconciliation");
    };
    snapshot
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    Ok(snapshot)
}

async fn reconcile_managed_topology(
    config: &AppConfig,
    image_cache: &SharedImageContentCache,
    state: &mut TopologyManagerState,
    snapshot: &splinterm_protocol::TopologySnapshot,
    updates: &mpsc::Sender<WindowTopologyUpdate>,
) -> Result<bool> {
    let authoritative = snapshot
        .topology
        .lairs()
        .flat_map(|lair| {
            lair.dojos.iter().map(move |dojo| {
                (
                    dojo.id,
                    (window_dojo_identity(lair, dojo), dojo.root.clone()),
                )
            })
        })
        .collect::<HashMap<_, _>>();
    let dojo_ids = state.tabs.iter().map(|tab| tab.dojo_id).collect::<Vec<_>>();
    for dojo_id in dojo_ids {
        let Some((identity, root)) = authoritative.get(&dojo_id).cloned() else {
            if let Some(removed) = state.tabs.close(dojo_id) {
                let acknowledged = remove_frontend_tab(updates, dojo_id).await;
                cancel_pane_tasks(removed.value.pane_tasks).await;
                if !acknowledged {
                    return Ok(false);
                }
            }
            continue;
        };
        let managed = &mut state
            .tabs
            .get_mut(dojo_id)
            .context("managed Dojo disappeared during reconciliation")?
            .value;
        if managed.identity != identity {
            if updates
                .send(WindowTopologyUpdate::UpdateIdentity(identity.clone()))
                .await
                .is_err()
            {
                return Ok(false);
            }
            managed.identity = identity;
        }
        let (added, _) = topology_identity_diff(&managed.root, &root);
        let (focused, consumed) =
            pending_focus_for_observation(managed.pending_focus, snapshot.revision, &added);
        match reconcile_window_topology(
            config,
            image_cache,
            dojo_id,
            &mut managed.root,
            root,
            focused,
            updates,
            &mut managed.pane_tasks,
        )
        .await
        {
            Ok(true) => {}
            Ok(false) => return Ok(false),
            Err(error) => {
                let _ = updates
                    .send(WindowTopologyUpdate::TabFailed {
                        dojo_id: Some(dojo_id),
                        message: format!("{error:#}"),
                    })
                    .await;
            }
        }
        if consumed {
            managed.pending_focus = None;
        }
    }
    if state.tabs.is_empty() {
        let _ = updates.send(WindowTopologyUpdate::Closed).await;
        return Ok(false);
    }
    Ok(true)
}

#[allow(
    clippy::too_many_lines,
    reason = "poll reconciliation, stable command targeting, and owned task shutdown share one loop"
)]
async fn run_topology_manager(
    config: AppConfig,
    image_cache: SharedImageContentCache,
    initial_identity: WindowDojoIdentity,
    root: LayoutNode,
    mut commands: mpsc::Receiver<WindowTopologyCommand>,
    updates: mpsc::Sender<WindowTopologyUpdate>,
    pane_tasks: HashMap<SplintId, PaneTask>,
) -> Result<()> {
    let mut connection = Connection::connect().await?;
    let mut poll = tokio::time::interval(std::time::Duration::from_millis(250));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let initial_lair_id = initial_identity.lair_id;
    let initial_dojo_id = initial_identity.dojo_id;
    let mut state = TopologyManagerState {
        tabs: WindowTabSet::new(DojoTab::new(
            initial_lair_id,
            initial_dojo_id,
            ManagedDojo {
                identity: initial_identity,
                root,
                pending_focus: None,
                pane_tasks,
            },
        )),
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
        let snapshot = match inspect_managed_topology(&mut connection).await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let _ = updates
                    .send(WindowTopologyUpdate::Shutdown(format!("{error:#}")))
                    .await;
                return Err(error);
            }
        };
        if !reconcile_managed_topology(&config, &image_cache, &mut state, &snapshot, &updates)
            .await?
        {
            break;
        }
        let Some(command) = command else {
            if commands.is_closed() {
                break;
            }
            continue;
        };
        let (WindowTopologyCommand::Split { dojo_id, .. }
        | WindowTopologyCommand::Close { dojo_id, .. }
        | WindowTopologyCommand::AdjustRatio { dojo_id, .. }) = command
        else {
            unreachable!("session command escaped manager dispatch");
        };
        let managed = &mut state
            .tabs
            .get_mut(dojo_id)
            .context("topology command targeted a closed Dojo tab")?
            .value;
        match apply_topology_command(
            &mut connection,
            &config,
            dojo_id,
            &managed.root,
            snapshot.revision,
            command,
        )
        .await
        {
            Ok(TopologyCommandOutcome::Updated { pending_focus }) => {
                managed.pending_focus = pending_focus;
            }
            Ok(TopologyCommandOutcome::WindowClosed) => {
                let removed = state.tabs.close(dojo_id).expect("edited tab remains");
                let acknowledged = remove_frontend_tab(&updates, dojo_id).await;
                cancel_pane_tasks(removed.value.pane_tasks).await;
                if !acknowledged || state.tabs.is_empty() {
                    break;
                }
            }
            Err(error) => eprintln!("splinterm topology edit rejected: {error:#}"),
        }
    }
    let mut remaining_tasks = Vec::new();
    for tab in state.tabs.iter_mut() {
        remaining_tasks.extend(tab.value.pane_tasks.drain().map(|(_, task)| task));
    }
    for task in &remaining_tasks {
        task.cancellation.cancel();
    }
    for task in remaining_tasks {
        let _ = task.task.await;
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
    dojo_id: DojoId,
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
                dojo_id,
                target,
                axis: Axis::Horizontal,
            })
            .await
            .map_err(|_| anyhow::anyhow!("topology smoke split channel closed"))?;
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;
        commands
            .send(WindowTopologyCommand::AdjustRatio {
                dojo_id,
                target,
                delta: 100,
            })
            .await
            .map_err(|_| anyhow::anyhow!("topology smoke ratio channel closed"))
    })))
}

async fn initial_window_dojo_identity(dojo_id: DojoId) -> Result<WindowDojoIdentity> {
    let mut connection = Connection::connect().await?;
    let Response::Lairs { lairs, .. } = connection.request(Request::ListLairs).await? else {
        bail!("splinterd did not return its Lairs for Window identity");
    };
    for lair in &lairs {
        if let Some(dojo) = lair.dojos.iter().find(|dojo| dojo.id == dojo_id) {
            return Ok(window_dojo_identity(lair, dojo));
        }
    }
    bail!("initial Dojo is absent from daemon topology")
}

async fn run_live_multipane_window(
    config: AppConfig,
    dojo_model: splinterm_core::Dojo,
) -> Result<()> {
    let initial_identity = initial_window_dojo_identity(dojo_model.id).await?;
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
    let mut tasks = HashMap::with_capacity(prepared.len());
    for pane in prepared {
        let splint_id = pane.options.snapshot.splint_id;
        panes.push(pane.options);
        tasks.insert(splint_id, pane.task);
    }
    let topology_smoke = spawn_topology_smoke(
        topology_commands.clone(),
        dojo_model.id,
        dojo_model.default_focus,
    )?;
    let window_config = config.clone();
    let root = dojo_model.root;
    let manager_root = root.clone();
    let active_splint = dojo_model.default_focus;
    let topology_manager = tokio::spawn(run_topology_manager(
        config,
        image_cache,
        initial_identity.clone(),
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
            initial_dojo: Some(initial_identity),
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
    let controller_cancellation = tokio_util::sync::CancellationToken::new();
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
        controller_cancellation,
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

#[cfg(test)]
mod tests {
    use super::*;
    use splinterm_protocol::{ActiveScreen, TerminalInputModes, TerminalRow};

    #[test]
    fn reset_requires_explicit_confirmation_for_unattended_use() {
        let guarded = Cli::try_parse_from(["splinterm", "reset"]).unwrap();
        assert!(matches!(guarded.command, Command::Reset { yes: false }));

        let confirmed = Cli::try_parse_from(["splinterm", "reset", "--yes"]).unwrap();
        assert!(matches!(confirmed.command, Command::Reset { yes: true }));
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
    fn tab_creation_capacity_is_rejected_before_daemon_creation() {
        assert!(window_has_tab_capacity(0));
        assert!(window_has_tab_capacity(splinterm::tab::MAX_WINDOW_TABS - 1));
        assert!(!window_has_tab_capacity(splinterm::tab::MAX_WINDOW_TABS));
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
    fn successful_split_focuses_the_new_local_splint() {
        let splint_id = SplintId::new();
        assert_eq!(
            topology_command_outcome(Response::SplintStarted {
                splint_id,
                incarnation: 1,
                topology_revision: TopologyRevision::new(2),
            })
            .unwrap(),
            TopologyCommandOutcome::Updated {
                pending_focus: Some(PendingTopologyFocus {
                    splint_id,
                    revision: TopologyRevision::new(2),
                })
            }
        );
        assert_eq!(
            topology_command_outcome(Response::TopologyCommitted {
                topology_revision: TopologyRevision::new(3),
            })
            .unwrap(),
            TopologyCommandOutcome::Updated {
                pending_focus: None
            }
        );
    }

    #[test]
    fn pending_split_focus_is_revision_bound_and_requires_the_added_splint() {
        let splint_id = SplintId::new();
        let unrelated = SplintId::new();
        let pending = Some(PendingTopologyFocus {
            splint_id,
            revision: TopologyRevision::new(4),
        });

        assert_eq!(
            pending_focus_for_observation(pending, TopologyRevision::new(3), &[splint_id]),
            (None, false)
        );
        assert_eq!(
            pending_focus_for_observation(pending, TopologyRevision::new(4), &[splint_id]),
            (Some(splint_id), true)
        );
        assert_eq!(
            pending_focus_for_observation(pending, TopologyRevision::new(4), &[unrelated]),
            (None, true)
        );
        assert_eq!(
            pending_focus_for_observation(pending, TopologyRevision::new(5), &[]),
            (None, true)
        );
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
