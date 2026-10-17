//! Machine projection of `splinterm agent-status`.

use super::{
    CliErrorCodeV2, Request, Response, Result, bail, bounded_public_message, connect_machine,
    finish_machine_envelope, write_json_document, write_machine_read_failure,
};
use crate::app::agent_status_cli::AgentStatusRequest;
use splinterm::automation::{agent_status_get_envelope, agent_status_report_envelope};

/// Runs one resolved agent status operation and emits one `splinterm.cli.v2`
/// document. Reports never preflight topology: a process inside the Splint
/// self-reports without any read grant.
pub(in crate::app) async fn run_machine_agent_status(
    request: AgentStatusRequest,
    schema_major: u16,
    timeout_ms: u64,
) -> Result<()> {
    let operation = request.operation();
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
    let remaining = deadline.saturating_sub(started.elapsed());
    match request {
        AgentStatusRequest::Report {
            splint_id,
            incarnation,
            status,
        } => {
            let state = status.as_ref().map(|status| status.state);
            let result = match connection
                .request_with_deadline(
                    Request::ReportAgentStatus {
                        splint_id,
                        incarnation,
                        status,
                    },
                    remaining,
                )
                .await
            {
                Ok(Response::AgentStatusAccepted {
                    sequence,
                    expires_at_ms,
                }) => agent_status_report_envelope(
                    splint_id,
                    incarnation,
                    state,
                    sequence,
                    expires_at_ms,
                ),
                Ok(_) => Err(anyhow::anyhow!(
                    "splinterd returned an unexpected agent status response"
                )),
                Err(error) => Err(error),
            };
            finish_machine_envelope(operation, result)
        }
        AgentStatusRequest::Get { splint_id } => {
            let snapshot = match connection
                .request_with_deadline(Request::InspectTopology, remaining)
                .await
            {
                Ok(Response::Topology { snapshot }) => snapshot,
                Ok(_) => {
                    return finish_machine_envelope(
                        operation,
                        Err(anyhow::anyhow!(
                            "splinterd returned an unexpected read response"
                        )),
                    );
                }
                Err(error) => return finish_machine_envelope(operation, Err(error)),
            };
            if snapshot.topology.find_splint(splint_id).is_none() {
                write_machine_read_failure(
                    operation,
                    CliErrorCodeV2::NotFound,
                    "requested Splint was not found",
                    false,
                )?;
                bail!("requested Splint was not found");
            }
            match agent_status_get_envelope(&snapshot, splint_id) {
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
    }
}
