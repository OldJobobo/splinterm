//! JSON and NDJSON machine client.

use crate::{
    AuthorizationCommand, Axis, CliEnvelopeV2, CliErrorCodeV2, CliEventV2, Command, Connection,
    Context, ControlMode, DojoId, ErrorCode, HashMap, HashSet, HistoryTransition, LairId,
    LaunchParameters, LayoutNode, MutationIdentityV2, PathBuf, PingEnvelopeV2, ReadResyncReasonV2,
    Request, Response, Result, ResyncReasonV2, ServerFrame, SplintId, SplitRatio, SplitSide,
    SubscribeCommand, SubscriptionEvent, TerminalContinuationV2, TerminalReadProvenanceV2,
    TopologyRevision, audit_page_envelope, authorization_status_envelope, bail,
    committed_mutation_envelope, created_mutation_envelope, decode_terminal_cursor, env,
    inspect_splint_envelope, inspect_topology_envelope, kill_envelope, launch_parameters,
    list_lairs_envelope, load_default, process_started_envelope, protocol_error, public_error_code,
    read_resync_envelope, response_protocol_error, restore_many_envelope, revoke_envelope,
    scrollback_page_envelope, search_page_envelope, terminal_action_envelope,
    terminal_snapshot_envelope, write_json_document,
};

mod control;
mod history;
mod mutation;
mod output;
mod read;
mod subscription;

use control::{MachineControl, run_machine_control};
use history::{
    MachineHistory, live_terminal_location, run_machine_history, run_machine_ping,
    run_machine_snapshot,
};
use mutation::{
    extract_machine_mutation, run_machine_audit, run_machine_authorization_status,
    run_machine_mutation,
};
pub(crate) use output::machine_exit_code;
use output::{
    bounded_public_message, finish_machine_envelope, write_machine_connection_failure,
    write_machine_read_failure,
};
use read::{MachineRead, run_machine_read};

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

pub(crate) use mutation::{require_expected_incarnation, require_incarnation};
pub(crate) use subscription::run_machine_subscription;

pub(crate) async fn run_machine_command(
    command: Command,
    schema_major: u16,
    timeout_ms: u64,
) -> Result<()> {
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
