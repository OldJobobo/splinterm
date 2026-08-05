//! JSON and NDJSON machine client.

use std::{
    collections::{HashMap, HashSet},
    env,
    path::PathBuf,
};

use anyhow::{Context, Result, bail};
use splinterm::automation::{
    CliEnvelopeV2, CliErrorCodeV2, CliEventV2, Connection, MutationIdentityV2, PingEnvelopeV2,
    ReadResyncReasonV2, ResyncReasonV2, TerminalContinuationV2, TerminalReadProvenanceV2,
    audit_page_envelope, authorization_status_envelope, committed_mutation_envelope,
    created_mutation_envelope, decode_terminal_cursor, inspect_splint_envelope,
    inspect_topology_envelope, kill_envelope, list_lairs_envelope, process_started_envelope,
    protocol_error, public_error_code, read_resync_envelope, response_protocol_error,
    restore_many_envelope, revoke_envelope, scrollback_page_envelope, search_page_envelope,
    terminal_action_envelope, terminal_snapshot_envelope, write_json_document,
};
use splinterm::config::load_default;
use splinterm_core::{
    Axis, DojoId, LairId, LayoutNode, SplintId, SplitRatio, SplitSide, TopologyRevision,
};
use splinterm_protocol::{
    ControlMode, ErrorCode, HistoryTransition, LaunchParameters, Request, Response, ServerFrame,
    SubscriptionEvent,
};

use super::{
    commands::{AuthorizationCommand, Command, SubscribeCommand},
    session_catalog::launch_parameters,
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
pub(in crate::app) use output::machine_exit_code;
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

pub(in crate::app) use mutation::{require_expected_incarnation, require_incarnation};
pub(in crate::app) use subscription::run_machine_subscription;

pub(in crate::app) async fn run_machine_command(
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
