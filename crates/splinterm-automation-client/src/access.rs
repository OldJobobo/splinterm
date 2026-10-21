//! Public v2 projections of the access inventory and its revokes (ADR 0014,
//! Plan 0053 PR 4): `access_list`, `access_revoke`, and `access_revoke_all`.
//!
//! Every entry has one public `id`, which is also its continuation key:
//!
//! * a runtime grant ("just this once" or "while this workspace is open"):
//!   its nonzero decimal grant ID, for example `17`;
//! * a remembered grant: its durable entry ID, `ra_` and 32 hex digits;
//! * a pending request: `pending:` and its request ID; and
//! * a policy rule: `policy:` and its rule ID.
//!
//! The forms never overlap, so a remembered entry's durable ID and a runtime
//! grant ID are never confused. Only the first two forms are revocable.

use anyhow::{Result, bail};
use serde::Serialize;
use splinterm_protocol::{
    AccessEntry, AccessEntryKey, AccessEntryKind, AccessEntryState, AccessIdentityKind,
    AccessLevel, AccessPolicyResource, AccessRevocation, AccessRevocationCounts,
    AccessRevokeTarget, AccessScope, DurableRemoval, MAX_ACCESS_INVENTORY_PAGE,
    RememberedStoreStatus, is_remembered_entry_id,
};

use super::{CliEnvelopeV2, automation_scope_name, decimal_id, valid_rule_id};

const PENDING_PREFIX: &str = "pending:";
const POLICY_PREFIX: &str = "policy:";

/// The public ID of an inventory entry.
#[must_use]
pub fn access_entry_id(key: &AccessEntryKey) -> String {
    match key {
        AccessEntryKey::Grant { grant_id } => grant_id.to_string(),
        AccessEntryKey::Remembered { entry_id } => entry_id.clone(),
        AccessEntryKey::Pending { request_id } => format!("{PENDING_PREFIX}{request_id}"),
        AccessEntryKey::Policy { rule_id } => format!("{POLICY_PREFIX}{rule_id}"),
    }
}

fn nonzero_decimal(text: &str) -> Option<u64> {
    (!text.is_empty() && !text.starts_with('0') && text.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

/// Parses a public entry ID back into its key.
#[must_use]
pub fn parse_access_entry_id(id: &str) -> Option<AccessEntryKey> {
    if let Some(request_id) = id.strip_prefix(PENDING_PREFIX) {
        return nonzero_decimal(request_id)
            .map(|request_id| AccessEntryKey::Pending { request_id });
    }
    if let Some(rule_id) = id.strip_prefix(POLICY_PREFIX) {
        return valid_rule_id(rule_id).then(|| AccessEntryKey::Policy {
            rule_id: rule_id.to_owned(),
        });
    }
    if is_remembered_entry_id(id) {
        return Some(AccessEntryKey::Remembered {
            entry_id: id.to_owned(),
        });
    }
    nonzero_decimal(id).map(|grant_id| AccessEntryKey::Grant { grant_id })
}

/// Why a public ID cannot be revoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessRevokeIdError {
    Malformed,
    Pending,
    Policy,
}

impl std::fmt::Display for AccessRevokeIdError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Malformed => {
                "an access ID is a grant ID, a remembered entry ID (ra_…), pending:N, or policy:RULE"
            }
            Self::Pending => {
                "a pending request is not a grant; it ends when it expires, is answered, or with --all"
            }
            Self::Policy => {
                "a policy rule cannot be revoked here; edit policy.json and reload the policy"
            }
        })
    }
}

impl std::error::Error for AccessRevokeIdError {}

/// Parses the target of a per-entry revoke.
///
/// # Errors
/// Names why the ID is not a revocable grant.
pub fn parse_access_revoke_target(id: &str) -> Result<AccessRevokeTarget, AccessRevokeIdError> {
    match parse_access_entry_id(id).ok_or(AccessRevokeIdError::Malformed)? {
        AccessEntryKey::Grant { grant_id } => Ok(AccessRevokeTarget::Grant { grant_id }),
        AccessEntryKey::Remembered { entry_id } => Ok(AccessRevokeTarget::Remembered { entry_id }),
        AccessEntryKey::Pending { .. } => Err(AccessRevokeIdError::Pending),
        AccessEntryKey::Policy { .. } => Err(AccessRevokeIdError::Policy),
    }
}

/// Display data an agent or user supplied, without control or bidirectional
/// formatting characters, so it can neither move a terminal cursor nor
/// reorder the text around it.
#[must_use]
pub fn public_display_text(text: &str) -> String {
    text.chars()
        .filter(|character| {
            !character.is_control()
                && !matches!(
                    character,
                    '\u{061c}'
                        | '\u{200e}'
                        | '\u{200f}'
                        | '\u{202a}'..='\u{202e}'
                        | '\u{2066}'..='\u{2069}'
                )
        })
        .collect()
}

/// The consent scope vocabulary `request_lair_access` uses (ADR 0014).
#[must_use]
pub fn consent_scope_name(scope: AccessScope) -> Option<&'static str> {
    match scope {
        AccessScope::TopologyObserve => Some("topology_observe"),
        AccessScope::Observe => Some("observe"),
        AccessScope::Scrollback => Some("scrollback"),
        AccessScope::AgentStatusReport => Some("agent_status_report"),
        AccessScope::Input => Some("input"),
        AccessScope::Resize => Some("resize"),
        AccessScope::ControlTakeover => Some("control_takeover"),
        AccessScope::ProcessSpawn => Some("process_spawn"),
        AccessScope::TopologyLayout => Some("topology_layout"),
        AccessScope::TopologyName => Some("topology_name"),
        AccessScope::Terminate => Some("terminate"),
        AccessScope::ProcessRestore => Some("process_restore"),
        AccessScope::ClipboardRead | AccessScope::ClipboardWrite => None,
    }
}

#[must_use]
pub const fn access_level_name(level: AccessLevel) -> &'static str {
    match level {
        AccessLevel::Look => "look",
        AccessLevel::LookAndType => "look_and_type",
        AccessLevel::Manage => "manage",
    }
}

#[must_use]
pub const fn access_kind_name(kind: AccessEntryKind) -> &'static str {
    match kind {
        AccessEntryKind::Once => "once",
        AccessEntryKind::WorkspaceSession => "workspace_session",
        AccessEntryKind::Remembered => "remembered",
        AccessEntryKind::Pending => "pending",
        AccessEntryKind::PersistentPolicy => "policy",
    }
}

const fn state_name(state: AccessEntryState) -> &'static str {
    match state {
        AccessEntryState::Active => "active",
        AccessEntryState::WorkspaceMissing => "workspace_missing",
        AccessEntryState::RemovalPending => "removal_pending",
        AccessEntryState::Prompting => "prompting",
        AccessEntryState::Queued => "queued",
    }
}

const fn identity_name(kind: AccessIdentityKind) -> &'static str {
    match kind {
        AccessIdentityKind::Process => "process",
        AccessIdentityKind::ExactBuild => "exact_build",
        AccessIdentityKind::ProtectedInstall => "protected_install",
        AccessIdentityKind::NixProfile => "nix_profile",
        AccessIdentityKind::PolicyDigest => "policy_digest",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AccessAgentV2 {
    identity: &'static str,
    executable_path: String,
    pid: Option<u32>,
    display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AccessWorkspaceV2 {
    lair_id: String,
    name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AccessSplintV2 {
    splint_id: String,
    incarnation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum AccessPolicyResourceV2 {
    Daemon,
    Lair { lair_id: String },
    Dojo { dojo_id: String },
    Splint { splint_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AccessEntryV2 {
    id: String,
    kind: &'static str,
    state: &'static str,
    revocable: bool,
    grant_id: Option<String>,
    agent: AccessAgentV2,
    workspace: Option<AccessWorkspaceV2>,
    splint: Option<AccessSplintV2>,
    level: Option<&'static str>,
    scopes: Vec<&'static str>,
    policy_scopes: Vec<&'static str>,
    policy_resources: Vec<AccessPolicyResourceV2>,
    created_unix_seconds: Option<u64>,
    last_used_unix_seconds: Option<u64>,
    expires_unix_seconds: Option<u64>,
    connections: u32,
    controllers: u32,
    policy_rules: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum RememberedStoreV2 {
    Available,
    Unavailable { reason: String },
    NotConfigured,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AccessListDataV2 {
    authorization_revision: u64,
    policy_generation: u64,
    remembered_store: RememberedStoreV2,
    total_entries: u32,
    entries: Vec<AccessEntryV2>,
    next_after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum DurableRemovalV2 {
    NotNeeded,
    Written,
    Failed {
        message: String,
        returns_after_restart: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AccessRevokeDataV2 {
    id: String,
    kind: &'static str,
    revoked_grant_id: String,
    lair_id: Option<String>,
    durable_removal: DurableRemovalV2,
    policy_rules: Vec<String>,
    authorization_revision: u64,
    confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AccessCountsV2 {
    once: u32,
    workspace_session: u32,
    remembered: u32,
    pending_requests: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AccessRevokeAllDataV2 {
    dry_run: bool,
    confirmed: bool,
    counts: AccessCountsV2,
    durable_removal: DurableRemovalV2,
    authorization_revision: u64,
}

fn bounded_text(text: &str, limit: usize, label: &str) -> Result<String> {
    let text = public_display_text(text);
    if text.chars().count() > limit {
        bail!("{label} exceeds public v2 bounds");
    }
    Ok(text)
}

fn rule_ids(rules: &[String]) -> Result<Vec<String>> {
    if rules.len() > 64 || rules.iter().any(|rule| !valid_rule_id(rule)) {
        bail!("access policy rule IDs exceed public v2 bounds");
    }
    Ok(rules.to_vec())
}

fn entry(entry: &AccessEntry) -> Result<AccessEntryV2> {
    if entry.validate().is_err() {
        bail!("access entry is inconsistent");
    }
    let mut scopes = entry
        .scopes
        .iter()
        .map(|scope| consent_scope_name(*scope))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| anyhow::anyhow!("access entry holds a clipboard scope"))?;
    scopes.sort_unstable();
    scopes.dedup();
    let mut policy_scopes: Vec<_> = entry
        .policy_scopes
        .iter()
        .copied()
        .map(automation_scope_name)
        .collect();
    policy_scopes.sort_unstable();
    policy_scopes.dedup();
    if let AccessEntryKey::Policy { rule_id } = &entry.key
        && !valid_rule_id(rule_id)
    {
        bail!("access policy rule ID exceeds public v2 bounds");
    }
    Ok(AccessEntryV2 {
        id: access_entry_id(&entry.key),
        kind: access_kind_name(entry.kind),
        state: state_name(entry.state),
        revocable: entry.revocable,
        grant_id: entry
            .grant_id
            .map(|grant_id| decimal_id(grant_id, "grant ID"))
            .transpose()?,
        agent: AccessAgentV2 {
            identity: identity_name(entry.identity.kind),
            executable_path: entry.identity.executable_path.clone(),
            pid: entry.identity.pid,
            display_name: entry
                .identity
                .display_name
                .as_deref()
                .map(|name| bounded_text(name, 256, "agent display name"))
                .transpose()?
                .filter(|name| !name.is_empty()),
        },
        workspace: entry.workspace.as_ref().map(|workspace| AccessWorkspaceV2 {
            lair_id: workspace.lair_id.to_string(),
            name: workspace.name.clone(),
        }),
        splint: entry
            .splint_id
            .zip(entry.incarnation)
            .map(|(splint_id, incarnation)| AccessSplintV2 {
                splint_id: splint_id.to_string(),
                incarnation,
            }),
        level: entry.level.map(access_level_name),
        scopes,
        policy_scopes,
        policy_resources: entry
            .policy_resources
            .iter()
            .map(|resource| match resource {
                AccessPolicyResource::Daemon => AccessPolicyResourceV2::Daemon,
                AccessPolicyResource::Lair { lair_id } => AccessPolicyResourceV2::Lair {
                    lair_id: lair_id.to_string(),
                },
                AccessPolicyResource::Dojo { dojo_id } => AccessPolicyResourceV2::Dojo {
                    dojo_id: dojo_id.to_string(),
                },
                AccessPolicyResource::Splint { splint_id } => AccessPolicyResourceV2::Splint {
                    splint_id: splint_id.to_string(),
                },
            })
            .collect(),
        created_unix_seconds: entry.created_at_unix_seconds,
        last_used_unix_seconds: entry.last_used_at_unix_seconds,
        expires_unix_seconds: entry.expires_at_unix_seconds,
        connections: entry.connections,
        controllers: entry.controllers,
        policy_rules: rule_ids(&entry.policy_rules)?,
    })
}

fn durable(removal: &DurableRemoval) -> Result<DurableRemovalV2> {
    Ok(match removal {
        DurableRemoval::NotNeeded => DurableRemovalV2::NotNeeded,
        DurableRemoval::Written => DurableRemovalV2::Written,
        DurableRemoval::Failed { message } => DurableRemovalV2::Failed {
            message: {
                let message = bounded_text(message, 256, "durable removal message")?;
                if message.is_empty() {
                    "the remembered-grant store could not be written".to_owned()
                } else {
                    message
                }
            },
            returns_after_restart: true,
        },
    })
}

/// One page of the access inventory, projected to public v2. `truncated`
/// means another page follows after `next_after`.
pub fn access_list_envelope(
    authorization_revision: u64,
    policy_generation: u64,
    remembered_store: &RememberedStoreStatus,
    total_entries: u32,
    entries: &[AccessEntry],
    next_after: Option<&AccessEntryKey>,
) -> Result<CliEnvelopeV2> {
    if entries.len() > usize::from(MAX_ACCESS_INVENTORY_PAGE)
        || u32::try_from(entries.len()).is_ok_and(|len| len > total_entries)
    {
        bail!("access inventory page exceeds public v2 bounds");
    }
    if entries.windows(2).any(|pair| pair[0].key >= pair[1].key) {
        bail!("access inventory page is not in key order");
    }
    if next_after.is_some_and(|after| entries.last().map(|entry| &entry.key) != Some(after)) {
        bail!("access inventory continuation does not follow the page");
    }
    let entries = entries.iter().map(entry).collect::<Result<Vec<_>>>()?;
    CliEnvelopeV2::success(
        "access_list",
        None,
        serde_json::to_value(AccessListDataV2 {
            authorization_revision,
            policy_generation,
            remembered_store: match remembered_store {
                RememberedStoreStatus::Available => RememberedStoreV2::Available,
                RememberedStoreStatus::NotConfigured => RememberedStoreV2::NotConfigured,
                RememberedStoreStatus::Unavailable { reason } => RememberedStoreV2::Unavailable {
                    reason: {
                        let reason = bounded_text(reason, 256, "remembered store reason")?;
                        if reason.is_empty() {
                            "unavailable".to_owned()
                        } else {
                            reason
                        }
                    },
                },
            },
            total_entries,
            entries,
            next_after: next_after.map(access_entry_id),
        })?,
        next_after.is_some(),
    )
}

/// A confirmed per-entry revoke, projected to public v2. A failed durable
/// removal is a successful revoke: the grant ended now, and the document
/// says it returns after a restart until a later write removes it.
pub fn access_revoke_envelope(
    authorization_revision: u64,
    revocation: &AccessRevocation,
) -> Result<CliEnvelopeV2> {
    let kind_matches = matches!(
        (&revocation.key, revocation.kind),
        (
            AccessEntryKey::Remembered { .. },
            AccessEntryKind::Remembered
        ) | (
            AccessEntryKey::Grant { .. },
            AccessEntryKind::Once | AccessEntryKind::WorkspaceSession
        )
    );
    if !kind_matches || !revocation.key.is_valid() {
        bail!("revoked access entry is inconsistent");
    }
    if revocation.kind != AccessEntryKind::Remembered
        && revocation.durable_removal != DurableRemoval::NotNeeded
    {
        bail!("only a remembered grant has a durable removal");
    }
    CliEnvelopeV2::success(
        "access_revoke",
        None,
        serde_json::to_value(AccessRevokeDataV2 {
            id: access_entry_id(&revocation.key),
            kind: access_kind_name(revocation.kind),
            revoked_grant_id: decimal_id(revocation.grant_id, "grant ID")?,
            lair_id: revocation.lair_id.map(|lair_id| lair_id.to_string()),
            durable_removal: durable(&revocation.durable_removal)?,
            policy_rules: rule_ids(&revocation.policy_rules)?,
            authorization_revision,
            confirmed: true,
        })?,
        false,
    )
}

/// **Revoke all access**, or its dry-run counts, projected to public v2.
pub fn access_revoke_all_envelope(
    authorization_revision: u64,
    dry_run: bool,
    counts: AccessRevocationCounts,
    durable_removal: &DurableRemoval,
) -> Result<CliEnvelopeV2> {
    if dry_run && *durable_removal != DurableRemoval::NotNeeded {
        bail!("a dry run removes nothing durably");
    }
    CliEnvelopeV2::success(
        "access_revoke_all",
        None,
        serde_json::to_value(AccessRevokeAllDataV2 {
            dry_run,
            confirmed: !dry_run,
            counts: AccessCountsV2 {
                once: counts.once,
                workspace_session: counts.workspace_session,
                remembered: counts.remembered,
                pending_requests: counts.pending_requests,
            },
            durable_removal: durable(durable_removal)?,
            authorization_revision,
        })?,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use splinterm_core::{LairId, SplintId};
    use splinterm_protocol::{AccessIdentity, AccessWorkspace, AutomationScope};

    const LAIR: &str = "018f4d8c-2a18-4b31-8c2f-9e7c5de77101";
    const OTHER_LAIR: &str = "018f4d8c-2a18-4b31-8c2f-9e7c5de77104";
    const SPLINT: &str = "018f4d8c-2a18-4b31-8c2f-9e7c5de77103";
    const ENTRY: &str = "ra_0123456789abcdef0123456789abcdef";

    fn fixture(raw: &str) -> serde_json::Value {
        serde_json::from_str::<serde_json::Value>(raw).unwrap()["document"].clone()
    }

    fn json(envelope: &CliEnvelopeV2) -> serde_json::Value {
        serde_json::to_value(envelope).unwrap()
    }

    fn lair(id: &str) -> LairId {
        id.parse().unwrap()
    }

    fn base(key: AccessEntryKey, kind: AccessEntryKind) -> AccessEntry {
        AccessEntry {
            key,
            kind,
            state: AccessEntryState::Active,
            revocable: true,
            grant_id: None,
            identity: AccessIdentity {
                kind: AccessIdentityKind::Process,
                executable_path: "/usr/bin/splinterm-mcp".to_owned(),
                pid: None,
                display_name: None,
            },
            workspace: Some(AccessWorkspace {
                lair_id: lair(LAIR),
                name: Some("work".to_owned()),
            }),
            splint_id: None,
            incarnation: None,
            level: None,
            scopes: Vec::new(),
            policy_scopes: Vec::new(),
            policy_resources: Vec::new(),
            created_at_unix_seconds: Some(1_760_918_400),
            last_used_at_unix_seconds: None,
            expires_at_unix_seconds: None,
            connections: 1,
            controllers: 0,
            policy_rules: Vec::new(),
        }
    }

    fn reviewed_entries() -> Vec<AccessEntry> {
        let mut remembered = base(
            AccessEntryKey::Remembered {
                entry_id: ENTRY.to_owned(),
            },
            AccessEntryKind::Remembered,
        );
        remembered.grant_id = Some(21);
        remembered.identity.kind = AccessIdentityKind::ProtectedInstall;
        remembered.identity.display_name = Some("claude-code".to_owned());
        remembered.level = Some(AccessLevel::Look);
        remembered.scopes = AccessLevel::Look.scopes().to_vec();
        remembered.last_used_at_unix_seconds = Some(1_760_922_000);
        remembered.policy_rules = vec!["mcp-reader".to_owned()];

        let mut once = base(
            AccessEntryKey::Grant { grant_id: 17 },
            AccessEntryKind::Once,
        );
        once.grant_id = Some(17);
        once.identity.pid = Some(4242);
        once.splint_id = Some(SPLINT.parse::<SplintId>().unwrap());
        once.incarnation = Some(2);
        once.scopes = vec![AccessScope::Input];
        once.expires_at_unix_seconds = Some(1_760_918_700);
        once.controllers = 1;

        let mut pending = base(
            AccessEntryKey::Pending { request_id: 3 },
            AccessEntryKind::Pending,
        );
        pending.state = AccessEntryState::Queued;
        pending.revocable = false;
        pending.identity.pid = Some(4343);
        pending.identity.display_name = Some("vs\u{202e}code\u{7}".to_owned());
        pending.workspace = Some(AccessWorkspace {
            lair_id: lair(OTHER_LAIR),
            name: None,
        });
        pending.level = Some(AccessLevel::Manage);
        pending.scopes = AccessLevel::Manage.scopes().to_vec();
        pending.expires_at_unix_seconds = Some(1_760_918_520);

        let mut policy = base(
            AccessEntryKey::Policy {
                rule_id: "mcp-reader".to_owned(),
            },
            AccessEntryKind::PersistentPolicy,
        );
        policy.revocable = false;
        policy.identity.kind = AccessIdentityKind::PolicyDigest;
        policy.workspace = None;
        policy.created_at_unix_seconds = None;
        policy.policy_scopes = vec![
            AutomationScope::TopologyMetadataRead,
            AutomationScope::TerminalVisibleRead,
        ];
        policy.policy_resources = vec![AccessPolicyResource::Lair {
            lair_id: lair(LAIR),
        }];
        vec![remembered, once, pending, policy]
    }

    #[test]
    fn the_inventory_projection_matches_the_reviewed_fixture() {
        let entries = reviewed_entries();
        let envelope = access_list_envelope(
            12,
            3,
            &RememberedStoreStatus::Available,
            5,
            &entries,
            Some(&entries[3].key),
        )
        .unwrap();
        assert_eq!(
            json(&envelope),
            fixture(include_str!(
                "../../../tests/automation/fixtures/valid/cli-access-list.json"
            ))
        );
        // A page must be in key order, and its continuation must follow it.
        let mut reversed = entries.clone();
        reversed.reverse();
        assert!(
            access_list_envelope(12, 3, &RememberedStoreStatus::Available, 5, &reversed, None)
                .is_err()
        );
        assert!(
            access_list_envelope(
                12,
                3,
                &RememberedStoreStatus::Available,
                5,
                &entries,
                Some(&entries[0].key)
            )
            .is_err()
        );
        assert!(
            access_list_envelope(12, 3, &RememberedStoreStatus::Available, 1, &entries, None)
                .is_err(),
            "a page is never larger than the inventory"
        );
        // An inconsistent entry is refused rather than projected.
        let mut revocable_policy = entries;
        revocable_policy[3].revocable = true;
        assert!(
            access_list_envelope(
                12,
                3,
                &RememberedStoreStatus::Available,
                5,
                &revocable_policy,
                None
            )
            .is_err()
        );
    }

    #[test]
    fn revoke_projections_match_the_reviewed_fixtures() {
        let revocation = AccessRevocation {
            key: AccessEntryKey::Remembered {
                entry_id: ENTRY.to_owned(),
            },
            kind: AccessEntryKind::Remembered,
            grant_id: 21,
            lair_id: Some(lair(LAIR)),
            durable_removal: DurableRemoval::Failed {
                message: "write temporary store: Read-only file system (os error 30)".to_owned(),
            },
            policy_rules: vec!["mcp-reader".to_owned()],
        };
        assert_eq!(
            json(&access_revoke_envelope(13, &revocation).unwrap()),
            fixture(include_str!(
                "../../../tests/automation/fixtures/valid/cli-access-revoke.json"
            ))
        );
        // Only a remembered grant has a durable side.
        let once = AccessRevocation {
            key: AccessEntryKey::Grant { grant_id: 17 },
            kind: AccessEntryKind::Once,
            grant_id: 17,
            ..revocation.clone()
        };
        assert!(access_revoke_envelope(13, &once).is_err());
        assert!(
            access_revoke_envelope(
                13,
                &AccessRevocation {
                    kind: AccessEntryKind::Remembered,
                    key: AccessEntryKey::Grant { grant_id: 17 },
                    ..revocation
                }
            )
            .is_err()
        );
        let counts = AccessRevocationCounts {
            once: 1,
            workspace_session: 0,
            remembered: 2,
            pending_requests: 1,
        };
        assert_eq!(
            json(
                &access_revoke_all_envelope(12, true, counts, &DurableRemoval::NotNeeded).unwrap()
            ),
            fixture(include_str!(
                "../../../tests/automation/fixtures/valid/cli-access-revoke-all.json"
            ))
        );
        assert!(access_revoke_all_envelope(12, true, counts, &DurableRemoval::Written).is_err());
        let done =
            json(&access_revoke_all_envelope(14, false, counts, &DurableRemoval::Written).unwrap());
        assert_eq!(done["data"]["confirmed"], true);
        assert_eq!(done["data"]["durable_removal"]["status"], "written");
    }

    #[test]
    fn public_ids_round_trip_and_never_overlap() {
        for key in [
            AccessEntryKey::Grant { grant_id: 17 },
            AccessEntryKey::Remembered {
                entry_id: ENTRY.to_owned(),
            },
            AccessEntryKey::Pending { request_id: 3 },
            AccessEntryKey::Policy {
                rule_id: "mcp-reader".to_owned(),
            },
        ] {
            assert_eq!(parse_access_entry_id(&access_entry_id(&key)), Some(key));
        }
        for refused in [
            "",
            "0",
            "017",
            "18446744073709551616",
            "ra_0123",
            "RA_0123456789abcdef0123456789abcdef",
            "pending:",
            "pending:0",
            "policy:",
            "policy:Upper",
            "policy:a:b",
            "grant:17",
        ] {
            assert_eq!(parse_access_entry_id(refused), None, "{refused}");
        }
        assert_eq!(
            parse_access_revoke_target("pending:3"),
            Err(AccessRevokeIdError::Pending)
        );
        assert_eq!(
            parse_access_revoke_target("policy:mcp-reader"),
            Err(AccessRevokeIdError::Policy)
        );
        assert_eq!(
            parse_access_revoke_target(ENTRY),
            Ok(AccessRevokeTarget::Remembered {
                entry_id: ENTRY.to_owned()
            })
        );
        assert_eq!(public_display_text("a\u{1b}[2J\u{202e}b\u{2066}"), "a[2Jb");
    }
}
