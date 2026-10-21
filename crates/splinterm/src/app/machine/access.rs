//! Machine projection of `splinterm access` (ADR 0014, Plan 0053 PR 4).
//!
//! It uses the automation role, so the daemon admits it only under a policy
//! rule granting `authorization_inspect` (listing) or `authorization_revoke`
//! (revoking); no consent level grants either.

use super::{
    CliErrorCodeV2, DaemonIdentity, Request, Response, Result, bail, connect_machine,
    explain_machine_result, finish_machine_envelope, safe_machine_read, write_machine_read_failure,
};
use crate::app::access_cli::AccessRequest;
use splinterm::automation::{
    access_list_envelope, access_revoke_all_envelope, access_revoke_envelope,
};
use splinterm_protocol::MAX_ACCESS_INVENTORY_PAGE;

/// Runs one resolved access operation and emits one `splinterm.cli.v2`
/// document.
pub(in crate::app) async fn run_machine_access(
    request: AccessRequest,
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
    if request.confirmation_missing() {
        write_machine_read_failure(
            operation,
            CliErrorCodeV2::ConfirmationRequired,
            "revoking access requires --yes",
            false,
        )?;
        bail!("revoking access requires --yes");
    }
    let deadline = std::time::Duration::from_millis(timeout_ms);
    let (mut connection, started) = connect_machine(operation, deadline).await?;
    let before = DaemonIdentity::of(&connection);
    let result = match request {
        AccessRequest::List { after, max_entries } => {
            list_envelope(&mut connection, after, max_entries, deadline, started).await
        }
        AccessRequest::Revoke { target, .. } => {
            let result = match connection
                .request_with_deadline(
                    Request::RevokeAccessEntry { target },
                    deadline.saturating_sub(started.elapsed()),
                )
                .await
            {
                Ok(Response::AccessEntryRevoked {
                    authorization_revision,
                    revocation,
                }) => access_revoke_envelope(authorization_revision, &revocation),
                Ok(_) => Err(anyhow::anyhow!(
                    "splinterd returned an unexpected revoke response"
                )),
                Err(error) => Err(error),
            };
            explain_machine_result(result, before, false, deadline, started).await
        }
        AccessRequest::RevokeAll { dry_run, .. } => {
            let result = match connection
                .request_with_deadline(
                    Request::RevokeAllAccess { dry_run },
                    deadline.saturating_sub(started.elapsed()),
                )
                .await
            {
                Ok(Response::AllAccessRevoked {
                    authorization_revision,
                    dry_run: answered,
                    counts,
                    durable_removal,
                }) if answered == dry_run => access_revoke_all_envelope(
                    authorization_revision,
                    dry_run,
                    counts,
                    &durable_removal,
                ),
                Ok(_) => Err(anyhow::anyhow!(
                    "splinterd returned an unexpected revoke response"
                )),
                Err(error) => Err(error),
            };
            explain_machine_result(result, before, dry_run, deadline, started).await
        }
    };
    finish_machine_envelope(operation, result)
}

/// One inventory page: a trivially safe read, retried once on a lost
/// connection like the other machine reads.
async fn list_envelope(
    connection: &mut splinterm::automation::Connection,
    after: Option<splinterm_protocol::AccessEntryKey>,
    max_entries: Option<u16>,
    deadline: std::time::Duration,
    started: std::time::Instant,
) -> Result<splinterm::automation::CliEnvelopeV2> {
    let max_entries = max_entries.unwrap_or(MAX_ACCESS_INVENTORY_PAGE);
    let request = || Request::ListAccess {
        after: after.clone(),
        max_entries,
    };
    match safe_machine_read(connection, request, deadline, started).await? {
        Response::AccessInventory {
            authorization_revision,
            policy_generation,
            remembered_store,
            total_entries,
            entries,
            next_after,
        } => access_list_envelope(
            authorization_revision,
            policy_generation,
            &remembered_store,
            total_entries,
            &entries,
            next_after.as_ref(),
        ),
        _ => bail!("splinterd returned an unexpected access inventory response"),
    }
}
