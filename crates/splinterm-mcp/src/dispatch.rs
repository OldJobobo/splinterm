use std::{collections::BTreeSet, env, time::Duration};

use anyhow::Error;
use serde_json::{Value, json};
use splinterm_automation_client::{Connection, protocol_error, request_cancellation};
use splinterm_core::{DojoId, LayoutNode, SplintId, WindowId};
use splinterm_protocol::{
    AccessGrant, AccessScope, AuditDecision, AuditOperation, AuditOutcome, ErrorCode, Request,
    Response, SplintLifecycle, SplintRuntimeSummary, TopologySnapshot,
};
use tokio_util::sync::CancellationToken;

const SCHEMA: &str = "splinterm.mcp.v1";
const DEFAULT_DEADLINE: Duration = Duration::from_secs(5);
const MINIMUM_DEADLINE_MS: u64 = 100;
const MAXIMUM_DEADLINE_MS: u64 = 30_000;
const DEFAULT_PAGE_SIZE: usize = 64;
const MAXIMUM_AUDIT_PAGE: usize = 128;

#[derive(Debug, Clone, Copy)]
pub(crate) struct DispatchFailure {
    pub(crate) code: &'static str,
    pub(crate) message: &'static str,
    pub(crate) retryable: bool,
}

impl DispatchFailure {
    const fn new(code: &'static str, message: &'static str, retryable: bool) -> Self {
        Self {
            code,
            message,
            retryable,
        }
    }

    pub(crate) const fn internal() -> Self {
        Self::new("internal", "the local automation request failed", false)
    }

    pub(crate) const fn resource_limit() -> Self {
        Self::new(
            "resource_limit",
            "the tool response exceeds the adapter limit",
            false,
        )
    }
}

fn deadline() -> Result<Duration, DispatchFailure> {
    let Some(value) = env::var_os("SPLINTERM_MCP_TIMEOUT_MS") else {
        return Ok(DEFAULT_DEADLINE);
    };
    let value = value
        .to_str()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| (MINIMUM_DEADLINE_MS..=MAXIMUM_DEADLINE_MS).contains(value))
        .ok_or_else(DispatchFailure::internal)?;
    Ok(Duration::from_millis(value))
}

async fn daemon_request(
    request: Request,
    cancellation: &CancellationToken,
) -> Result<Response, DispatchFailure> {
    let deadline = deadline()?;
    let started = tokio::time::Instant::now();
    let connection = tokio::select! {
        biased;
        () = cancellation.cancelled() => {
            return Err(DispatchFailure::new("cancelled", "the tool call was cancelled", true));
        }
        result = tokio::time::timeout(deadline, Connection::connect_automation()) => result,
    };
    let mut connection = match connection {
        Ok(Ok(connection)) => connection,
        Ok(Err(error)) => return Err(map_client_error(&error)),
        Err(_) => {
            return Err(DispatchFailure::new(
                "timeout",
                "the local automation deadline elapsed",
                true,
            ));
        }
    };
    let remaining = deadline.saturating_sub(started.elapsed());
    connection
        .request_with_cancellation(request, remaining, cancellation)
        .await
        .map_err(|error| map_client_error(&error))
}

fn map_client_error(error: &Error) -> DispatchFailure {
    if let Some(reason) = request_cancellation(error) {
        return match reason {
            splinterm_automation_client::RequestCancellation::DeadlineElapsed => {
                DispatchFailure::new("timeout", "the local automation deadline elapsed", true)
            }
            splinterm_automation_client::RequestCancellation::Cancelled => {
                DispatchFailure::new("cancelled", "the tool call was cancelled", true)
            }
        };
    }
    let Some(error) = protocol_error(error) else {
        return DispatchFailure::internal();
    };
    let (code, message, retryable) = match error.code {
        ErrorCode::AuthenticationFailed => (
            "authentication_failed",
            "daemon authentication failed",
            false,
        ),
        ErrorCode::HandshakeRequired => {
            ("handshake_required", "daemon handshake is required", false)
        }
        ErrorCode::IncompatibleVersion => (
            "incompatible_version",
            "daemon protocol version is incompatible",
            false,
        ),
        ErrorCode::ConsentUnavailable => (
            "consent_unavailable",
            "trusted graphical consent is unavailable",
            false,
        ),
        ErrorCode::ConsentDenied => ("consent_denied", "the access request was denied", false),
        ErrorCode::Unauthorized | ErrorCode::DevelopmentFeatureDisabled => (
            "unauthorized",
            "the exact policy scope or resource is not authorized",
            false,
        ),
        ErrorCode::ControllerUnavailable => (
            "controller_unavailable",
            "terminal control is unavailable",
            true,
        ),
        ErrorCode::ControlTransferUnavailable => (
            "control_transfer_unavailable",
            "control transfer is unavailable",
            true,
        ),
        ErrorCode::StaleTopology => ("stale_topology", "the topology revision is stale", true),
        ErrorCode::NotFound | ErrorCode::RequestNotFound => {
            ("not_found", "the requested resource was not found", false)
        }
        ErrorCode::StaleIncarnation => (
            "stale_incarnation",
            "the requested Splint incarnation is stale",
            true,
        ),
        ErrorCode::InvalidArgument
        | ErrorCode::InvalidFrame
        | ErrorCode::FrameTooLarge
        | ErrorCode::InvalidRequestId
        | ErrorCode::DuplicateRequestId
        | ErrorCode::UnsupportedOperation => {
            ("invalid_argument", "the daemon rejected the request", false)
        }
        ErrorCode::TooManyOutstandingRequests | ErrorCode::ResourceLimit => (
            "resource_limit",
            "the local automation resource limit was reached",
            true,
        ),
        ErrorCode::Cancelled => ("cancelled", "the daemon request was cancelled", true),
        ErrorCode::Internal => ("internal", "the local automation request failed", false),
    };
    DispatchFailure::new(code, message, retryable)
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "closed result values are moved conceptually into the serialized envelope"
)]
fn success(tool: &str, resource: Value, data: Value, truncated: bool, trust: &str) -> Value {
    json!({
        "schema": SCHEMA,
        "tool": tool,
        "ok": true,
        "resource": resource,
        "data": data,
        "truncated": truncated,
        "content_trust": trust,
    })
}

fn parse_splint(arguments: &Value) -> Result<SplintId, DispatchFailure> {
    arguments["splint_id"]
        .as_str()
        .and_then(|value| value.parse().ok())
        .ok_or_else(DispatchFailure::internal)
}

fn parse_incarnation(arguments: &Value) -> Result<u64, DispatchFailure> {
    arguments["incarnation"]
        .as_u64()
        .filter(|value| *value > 0)
        .ok_or_else(DispatchFailure::internal)
}

fn parse_grant_id(arguments: &Value) -> Result<u64, DispatchFailure> {
    arguments["grant_id"]
        .as_str()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .ok_or_else(DispatchFailure::internal)
}

fn scope_from_name(value: &str) -> Option<AccessScope> {
    match value {
        "terminal_visible_read" => Some(AccessScope::Observe),
        "scrollback_read" => Some(AccessScope::Scrollback),
        "input" => Some(AccessScope::Input),
        "resize" => Some(AccessScope::Resize),
        "process_terminate" => Some(AccessScope::Terminate),
        "controller_transfer" => Some(AccessScope::ControlTakeover),
        _ => None,
    }
}

fn access_scope_name(scope: AccessScope) -> Option<&'static str> {
    match scope {
        AccessScope::Observe => Some("terminal_visible_read"),
        AccessScope::Scrollback => Some("scrollback_read"),
        AccessScope::Input => Some("input"),
        AccessScope::Resize => Some("resize"),
        AccessScope::Terminate => Some("process_terminate"),
        AccessScope::ControlTakeover => Some("controller_transfer"),
        AccessScope::ClipboardRead | AccessScope::ClipboardWrite => None,
    }
}

fn automation_scope_name(scope: splinterm_protocol::AutomationScope) -> &'static str {
    match scope {
        splinterm_protocol::AutomationScope::TopologyMetadataRead => "topology_metadata_read",
        splinterm_protocol::AutomationScope::TopologySubscribe => "topology_subscribe",
        splinterm_protocol::AutomationScope::TerminalVisibleRead => "terminal_visible_read",
        splinterm_protocol::AutomationScope::TerminalSubscribe => "terminal_subscribe",
        splinterm_protocol::AutomationScope::ScrollbackRead => "scrollback_read",
        splinterm_protocol::AutomationScope::ScrollbackSearch => "scrollback_search",
        splinterm_protocol::AutomationScope::ControllerAcquire => "controller_acquire",
        splinterm_protocol::AutomationScope::ControllerTransfer => "controller_transfer",
        splinterm_protocol::AutomationScope::Input => "input",
        splinterm_protocol::AutomationScope::Resize => "resize",
        splinterm_protocol::AutomationScope::ProcessSpawn => "process_spawn",
        splinterm_protocol::AutomationScope::ProcessRestore => "process_restore",
        splinterm_protocol::AutomationScope::ProcessTerminate => "process_terminate",
        splinterm_protocol::AutomationScope::TopologyLayoutMutate => "topology_layout_mutate",
        splinterm_protocol::AutomationScope::TopologyNameMutate => "topology_name_mutate",
        splinterm_protocol::AutomationScope::AuthorizationInspect => "authorization_inspect",
        splinterm_protocol::AutomationScope::AuthorizationRevoke => "authorization_revoke",
        splinterm_protocol::AutomationScope::AuditInspect => "audit_inspect",
    }
}

fn authorization_resource(
    dojo_id: DojoId,
    window_id: WindowId,
    grant: &AccessGrant,
    revision: u64,
) -> Result<Value, DispatchFailure> {
    if grant.grant_id == 0 || grant.incarnation == 0 {
        return Err(DispatchFailure::internal());
    }
    Ok(json!({
        "kind": "authorization",
        "dojo_id": dojo_id.to_string(),
        "window_id": window_id.to_string(),
        "splint_id": grant.splint_id.to_string(),
        "incarnation": grant.incarnation,
        "grant_id": grant.grant_id.to_string(),
        "authorization_revision": revision,
    }))
}

fn state_name(runtime: &SplintRuntimeSummary) -> &'static str {
    match runtime.lifecycle {
        SplintLifecycle::Starting | SplintLifecycle::Running => "running",
        SplintLifecycle::Exited if runtime.restorable => "restorable",
        SplintLifecycle::Exited => "exited",
    }
}

fn runtime_for(
    snapshot: &TopologySnapshot,
    splint_id: SplintId,
) -> Result<&SplintRuntimeSummary, DispatchFailure> {
    snapshot
        .runtimes
        .iter()
        .find(|runtime| runtime.splint_id == splint_id)
        .ok_or_else(DispatchFailure::internal)
}

fn topology_splints(
    node: &LayoutNode,
    snapshot: &TopologySnapshot,
    output: &mut Vec<Value>,
) -> Result<(), DispatchFailure> {
    match node {
        LayoutNode::Leaf(splint) => {
            let runtime = runtime_for(snapshot, splint.id)?;
            output.push(json!({
                "splint_id": splint.id.to_string(),
                "current_incarnation": runtime.live_incarnation,
                "last_incarnation": runtime.last_incarnation,
                "title": splint.title,
                "state": state_name(runtime),
            }));
            Ok(())
        }
        LayoutNode::Branch { first, second, .. } => {
            topology_splints(first, snapshot, output)?;
            topology_splints(second, snapshot, output)
        }
    }
}

fn topology_data(snapshot: &TopologySnapshot) -> Result<Value, DispatchFailure> {
    snapshot
        .validate()
        .map_err(|_| DispatchFailure::internal())?;
    let mut dojos = Vec::new();
    for dojo in snapshot.lair.dojos() {
        let mut windows = Vec::new();
        for window in &dojo.windows {
            let mut splints = Vec::new();
            topology_splints(&window.root, snapshot, &mut splints)?;
            windows.push(json!({
                "window_id": window.id.to_string(),
                "title": window.title,
                "default_focus_splint_id": window.default_focus.to_string(),
                "splints": splints,
            }));
        }
        dojos.push(json!({
            "dojo_id": dojo.id.to_string(),
            "name": dojo.name,
            "windows": windows,
        }));
    }
    Ok(json!({"dojos": dojos}))
}

fn audit_cursor(value: u64) -> Option<String> {
    (value > 0).then(|| format!("cur_{value:016x}"))
}

fn decode_audit_cursor(value: &str) -> Option<u64> {
    let suffix = value.strip_prefix("cur_")?;
    (suffix.len() == 16)
        .then(|| u64::from_str_radix(suffix, 16).ok())
        .flatten()
        .filter(|value| *value > 0)
}

fn audit_operation_name(operation: AuditOperation) -> &'static str {
    match operation {
        AuditOperation::Ping => "ping",
        AuditOperation::RequestAccess => "request_access",
        AuditOperation::AuthorizationStatus => "authorization_status",
        AuditOperation::RevokeAccess => "revoke_access",
        AuditOperation::ListDojos => "list_dojos",
        AuditOperation::InspectTopology => "inspect_topology",
        AuditOperation::SubscribeTopology => "subscribe_topology",
        AuditOperation::InspectSplint => "inspect_splint",
        AuditOperation::CreateDojo => "create_dojo",
        AuditOperation::SplitSplint => "split_splint",
        AuditOperation::RelaunchSplint => "relaunch_splint",
        AuditOperation::RestoreSplint => "restore_splint",
        AuditOperation::RestoreWindow => "restore_window",
        AuditOperation::RestoreDojo => "restore_dojo",
        AuditOperation::CloseSplint => "close_splint",
        AuditOperation::SetSplitRatio => "set_split_ratio",
        AuditOperation::NewWindow => "new_window",
        AuditOperation::CloseWindow => "close_window",
        AuditOperation::RenameDojo => "rename_dojo",
        AuditOperation::RenameWindow => "rename_window",
        AuditOperation::SetWindowDefaultFocus => "set_window_default_focus",
        AuditOperation::RenameSplint => "rename_splint",
        AuditOperation::Attach => "attach",
        AuditOperation::ScrollbackPage => "scrollback_page",
        AuditOperation::SearchScrollback => "search_scrollback",
        AuditOperation::AcquireControl => "acquire_control",
        AuditOperation::SubscribeControl => "subscribe_control",
        AuditOperation::RequestControlTransfer => "request_control_transfer",
        AuditOperation::DecideControlTransfer => "decide_control_transfer",
        AuditOperation::ForceControlTransfer => "force_control_transfer",
        AuditOperation::ReleaseControl => "release_control",
        AuditOperation::Input => "input",
        AuditOperation::Resize => "resize",
        AuditOperation::Detach => "detach",
        AuditOperation::KillSplint => "kill_splint",
        AuditOperation::ProcessExit => "process_exit",
        AuditOperation::AuditInspect => "audit_inspect",
        AuditOperation::PolicyReload => "policy_reload",
    }
}

fn audit_outcome(decision: AuditDecision, outcome: Option<AuditOutcome>) -> &'static str {
    match (decision, outcome) {
        (AuditDecision::Denied | AuditDecision::Rejected | AuditDecision::Expired, _) => "denied",
        (_, Some(AuditOutcome::Failed | AuditOutcome::Cancelled)) => "failed",
        (AuditDecision::Revoked, Some(AuditOutcome::Succeeded)) => "committed",
        _ => "allowed",
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the exhaustive Slice 4 tool-to-private-request table stays contiguous for security review"
)]
pub(crate) async fn dispatch(
    tool: &str,
    arguments: &Value,
    cancellation: &CancellationToken,
) -> Result<Value, DispatchFailure> {
    match tool {
        "splinterm.ping" => match daemon_request(Request::Ping, cancellation).await? {
            Response::Pong => Ok(success(
                tool,
                json!({"kind": "daemon"}),
                json!({"protocol_version": "2025-11-25"}),
                false,
                "trusted_metadata",
            )),
            _ => Err(DispatchFailure::internal()),
        },
        "splinterm.list_dojos" => match daemon_request(Request::ListDojos, cancellation).await? {
            Response::Dojos {
                dojos,
                topology_revision,
            } => Ok(success(
                tool,
                json!({"kind": "topology", "topology_revision": topology_revision.get()}),
                json!({
                    "dojos": dojos.into_iter().map(|dojo| json!({
                        "dojo_id": dojo.id.to_string(),
                        "name": dojo.name,
                    })).collect::<Vec<_>>()
                }),
                false,
                "untrusted_terminal_data",
            )),
            _ => Err(DispatchFailure::internal()),
        },
        "splinterm.inspect_topology" => {
            match daemon_request(Request::InspectTopology, cancellation).await? {
                Response::Topology { snapshot } => Ok(success(
                    tool,
                    json!({"kind": "topology", "topology_revision": snapshot.revision.get()}),
                    topology_data(&snapshot)?,
                    false,
                    "untrusted_terminal_data",
                )),
                _ => Err(DispatchFailure::internal()),
            }
        }
        "splinterm.inspect_splint" => {
            let splint_id = parse_splint(arguments)?;
            match daemon_request(Request::InspectSplint { splint_id }, cancellation).await? {
                Response::Splint {
                    dojo_id,
                    window_id,
                    title,
                    topology_revision,
                    runtime,
                } if runtime.splint_id == splint_id => Ok(success(
                    tool,
                    json!({
                        "kind": "splint",
                        "dojo_id": dojo_id.to_string(),
                        "window_id": window_id.to_string(),
                        "splint_id": splint_id.to_string(),
                        "current_incarnation": runtime.live_incarnation,
                        "last_incarnation": runtime.last_incarnation,
                        "topology_revision": topology_revision.get(),
                    }),
                    json!({"title": title, "state": state_name(&runtime)}),
                    false,
                    "untrusted_terminal_data",
                )),
                _ => Err(DispatchFailure::internal()),
            }
        }
        "splinterm.request_access" => {
            let splint_id = parse_splint(arguments)?;
            let incarnation = parse_incarnation(arguments)?;
            let scopes = arguments["scopes"]
                .as_array()
                .ok_or_else(DispatchFailure::internal)?
                .iter()
                .map(|scope| scope.as_str().and_then(scope_from_name))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| {
                    DispatchFailure::new(
                        "invalid_argument",
                        "request_access supports only exact terminal access scopes",
                        false,
                    )
                })?;
            match daemon_request(
                Request::RequestAccess {
                    splint_id,
                    incarnation,
                    scopes,
                },
                cancellation,
            )
            .await?
            {
                Response::AccessGranted {
                    dojo_id,
                    window_id,
                    authorization_revision,
                    grant,
                } if grant.splint_id == splint_id && grant.incarnation == incarnation => {
                    let granted_scopes = grant
                        .scopes
                        .iter()
                        .copied()
                        .map(access_scope_name)
                        .collect::<Option<Vec<_>>>()
                        .ok_or_else(DispatchFailure::internal)?;
                    Ok(success(
                        tool,
                        authorization_resource(dojo_id, window_id, &grant, authorization_revision)?,
                        json!({
                            "committed": true,
                            "granted_scopes": granted_scopes,
                            "expires_at": grant.expires_at_unix_seconds.to_string(),
                        }),
                        false,
                        "trusted_metadata",
                    ))
                }
                _ => Err(DispatchFailure::internal()),
            }
        }
        "splinterm.authorization_status" => {
            let splint_id = parse_splint(arguments)?;
            match daemon_request(
                Request::AuthorizationStatus {
                    splint_id,
                    incarnation: None,
                },
                cancellation,
            )
            .await?
            {
                Response::AuthorizationStatus {
                    dojo_id,
                    window_id,
                    incarnation,
                    topology_revision,
                    policy_generation,
                    grants,
                    persistent,
                    development_bypass,
                } if incarnation > 0 => {
                    let mut scopes = BTreeSet::new();
                    for grant in grants {
                        if grant.splint_id != splint_id || grant.incarnation != incarnation {
                            return Err(DispatchFailure::internal());
                        }
                        scopes.extend(grant.scopes.into_iter().filter_map(access_scope_name));
                    }
                    for grant in persistent {
                        scopes.extend(grant.scopes.into_iter().map(automation_scope_name));
                    }
                    Ok(success(
                        tool,
                        json!({
                            "kind": "splint",
                            "dojo_id": dojo_id.to_string(),
                            "window_id": window_id.to_string(),
                            "splint_id": splint_id.to_string(),
                            "incarnation": incarnation,
                            "topology_revision": topology_revision.get(),
                        }),
                        json!({
                            "authorized": development_bypass || !scopes.is_empty(),
                            "scopes": scopes,
                            "policy_generation": policy_generation,
                        }),
                        false,
                        "trusted_metadata",
                    ))
                }
                _ => Err(DispatchFailure::internal()),
            }
        }
        "splinterm.revoke_access" => {
            let grant_id = parse_grant_id(arguments)?;
            match daemon_request(Request::RevokeAccess { grant_id }, cancellation).await? {
                Response::AccessRevoked {
                    dojo_id,
                    window_id,
                    authorization_revision,
                    grant,
                } if grant.grant_id == grant_id => Ok(success(
                    tool,
                    authorization_resource(dojo_id, window_id, &grant, authorization_revision)?,
                    json!({"committed": true}),
                    false,
                    "trusted_metadata",
                )),
                _ => Err(DispatchFailure::internal()),
            }
        }
        "splinterm.inspect_audit" => {
            let after_audit_id = arguments
                .get("cursor")
                .and_then(Value::as_str)
                .map(|cursor| {
                    decode_audit_cursor(cursor).ok_or_else(|| {
                        DispatchFailure::new(
                            "invalid_argument",
                            "the audit cursor is invalid",
                            false,
                        )
                    })
                })
                .transpose()?;
            let max_records = arguments
                .get("max_records")
                .and_then(Value::as_u64)
                .map_or(DEFAULT_PAGE_SIZE, |value| {
                    usize::try_from(value).unwrap_or(usize::MAX)
                });
            if max_records == 0 || max_records > MAXIMUM_AUDIT_PAGE {
                return Err(DispatchFailure::new(
                    "invalid_argument",
                    "the audit page limit exceeds the daemon limit",
                    false,
                ));
            }
            match daemon_request(
                Request::AuditInspect {
                    after_audit_id,
                    max_records,
                },
                cancellation,
            )
            .await?
            {
                Response::AuditPage { page } => {
                    let continuation_cursor = page.next_after_audit_id.and_then(audit_cursor);
                    let records = page
                        .records
                        .into_iter()
                        .map(|record| {
                            json!({
                                "audit_id": record.audit_id,
                                "operation": audit_operation_name(record.operation),
                                "outcome": audit_outcome(record.decision, record.outcome),
                            })
                        })
                        .collect::<Vec<_>>();
                    Ok(success(
                        tool,
                        json!({"kind": "audit"}),
                        json!({
                            "retention": "daemon_lifetime",
                            "retention_gap": page.retention_gap,
                            "records": records,
                            "continuation_cursor": continuation_cursor,
                        }),
                        continuation_cursor.is_some(),
                        "trusted_metadata",
                    ))
                }
                _ => Err(DispatchFailure::internal()),
            }
        }
        _ => Err(DispatchFailure::internal()),
    }
}
