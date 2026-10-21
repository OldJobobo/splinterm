//! The daemon-wide access inventory and its revocations (ADR 0014, Plan 0053
//! PR 4).
//!
//! The inventory lists every grant definition of every duration ("just this
//! once" for a Lair or one Splint, "while this workspace is open", and
//! remembered), every pending workspace request, and every policy-file rule
//! that matches a connected agent, with each entry's active use: the
//! automation connections it authorizes now and the controllers it
//! authorized. Remembered entries are named by their durable entry ID and
//! every other grant by its runtime grant ID, so the two never mix.
//!
//! A per-entry revoke ends one grant at once: new requests stop, its
//! subscriptions are cancelled, and its controllers released. A remembered
//! entry is then removed durably; if that write fails it stays revoked in
//! memory, the failure is reported, and the inventory lists it as returning
//! after a restart until a later write removes it. **Revoke all access** does
//! the same for every consent-derived grant and ends every pending request,
//! and never touches a policy rule or the trusted UI. Policy rules are listed
//! read-only, with the rule IDs under which an agent keeps its access.
//!
//! Who may call is decided by the authorization table: the trusted UI, or a
//! policy rule granting `authorization_inspect` (the inventory, on the
//! daemon) or `authorization_revoke` (a revoke, on the entry's workspace; all
//! of them, on the daemon). No consent level includes either scope.

use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use splinterm_core::{LairId, Topology};
use splinterm_policy::{durable_identity::DurableIdentity, policy::ResourceSelector};
use splinterm_protocol::{
    AccessEntry, AccessEntryKey, AccessEntryKind, AccessEntryState, AccessIdentity,
    AccessIdentityKind, AccessLevel, AccessPolicyResource, AccessRevocation,
    AccessRevocationCounts, AccessRevokeTarget, AccessScope, AccessWorkspace, DurableRemoval,
    ErrorCode, MAX_ACCESS_DISPLAY_NAME_BYTES, MAX_ACCESS_FAULT_BYTES, MAX_ACCESS_INVENTORY_PAGE,
    MAX_ACCESS_PATH_BYTES, MAX_ACCESS_POLICY_ITEMS, MAX_ACCESS_WORKSPACE_NAME_BYTES, ProtocolError,
    RememberedStoreStatus, Response,
};

use crate::{
    DaemonState, Revocation, agent_access, consent, internal, revoke_grant_controllers,
    revoke_remembered_grant, revoke_session_grant, splint_containment,
};

/// The reason audit records give for an explicit **Revoke all access**.
const REVOKE_ALL_REASON: &str = "revoke all access";

/// One automation connection, as the inventory matches it.
struct Connected {
    connection_id: u64,
    peer: consent::PeerIdentity,
}

/// Everything one inventory read saw.
struct Snapshot {
    authorization_revision: u64,
    policy_generation: u64,
    remembered_store: RememberedStoreStatus,
    /// In [`AccessEntryKey`] order.
    entries: Vec<AccessEntry>,
}

fn unix_seconds_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Cuts `text` to at most `limit` bytes on a character boundary.
fn bounded(mut text: String, limit: usize) -> String {
    if text.len() > limit {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

fn path_text(path: &Path) -> String {
    let text = path.to_string_lossy().into_owned();
    if text.is_empty() {
        return "?".to_owned();
    }
    bounded(text, MAX_ACCESS_PATH_BYTES)
}

fn display_name(name: Option<&str>) -> Option<String> {
    name.map(|name| {
        bounded(
            name.chars()
                .filter(|character| !character.is_control())
                .collect(),
            MAX_ACCESS_DISPLAY_NAME_BYTES,
        )
    })
    .filter(|name| !name.is_empty())
}

/// The path a process-bound entry shows: the verified executable when the
/// connection has one, else the requester label.
fn process_path(peer: &consent::PeerIdentity) -> String {
    peer.persistent_executable().map_or_else(
        || bounded(peer.requester_label(), MAX_ACCESS_PATH_BYTES),
        |executable| path_text(&executable.path),
    )
}

fn sorted_scopes(scopes: impl IntoIterator<Item = AccessScope>) -> Vec<AccessScope> {
    let scopes: BTreeSet<_> = scopes.into_iter().collect();
    scopes.into_iter().collect()
}

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn workspace(topology: &Topology, lair_id: LairId) -> AccessWorkspace {
    AccessWorkspace {
        lair_id,
        name: topology
            .find_lair(lair_id)
            .map(|lair| bounded(lair.name.clone(), MAX_ACCESS_WORKSPACE_NAME_BYTES)),
    }
}

/// The Lair each policy selector resolves into now, from live topology.
fn selector_lair(topology: &Topology, selector: &ResourceSelector) -> Option<LairId> {
    match selector {
        ResourceSelector::Daemon => None,
        ResourceSelector::Lair { lair_id } => Some(*lair_id),
        ResourceSelector::Dojo { dojo_id } => topology
            .lairs()
            .find(|lair| lair.dojos.iter().any(|dojo| dojo.id == *dojo_id))
            .map(|lair| lair.id),
        ResourceSelector::Splint { splint_id, .. } => {
            splint_containment(topology, *splint_id).map(|(lair_id, _, _)| lair_id)
        }
    }
}

struct PolicyView<'a> {
    rule: &'a splinterm_policy::policy::PolicyRule,
    /// Lairs the rule's selectors reach now.
    lairs: BTreeSet<LairId>,
}

impl PolicyView<'_> {
    fn matches_executable(&self, path: &Path, sha256: &str) -> bool {
        self.rule.executable.path == path && self.rule.executable.sha256 == sha256
    }

    fn matches_peer(&self, peer: &consent::PeerIdentity) -> bool {
        peer.persistent_executable()
            .is_some_and(|executable| self.matches_executable(&executable.path, &executable.sha256))
    }
}

/// The policy rules under which an agent still reaches `lair_id`: rules for
/// the exact executable (`exact`, path and digest), rules naming the same
/// canonical path (`path`: a protected install, or the store file a Nix
/// profile entry resolves to now), or rules matching one of the agent's
/// connected processes.
fn continuing_rules(
    policies: &[PolicyView<'_>],
    lair_id: LairId,
    exact: Option<(&Path, &str)>,
    path: Option<&Path>,
    peers: &[&consent::PeerIdentity],
) -> Vec<String> {
    policies
        .iter()
        .filter(|policy| policy.lairs.contains(&lair_id))
        .filter(|policy| {
            exact.is_some_and(|(path, sha256)| policy.matches_executable(path, sha256))
                || path.is_some_and(|path| policy.rule.executable.path == path)
                || peers.iter().any(|peer| policy.matches_peer(peer))
        })
        .map(|policy| policy.rule.id.clone())
        .take(MAX_ACCESS_POLICY_ITEMS)
        .collect()
}

#[allow(
    clippy::too_many_lines,
    reason = "every authority source is projected in one auditable pass"
)]
async fn snapshot(state: &DaemonState) -> Snapshot {
    // Read first: any change while the sources are read advances it past
    // this value, so a client paging on sees that the inventory moved.
    let authorization_revision = state.grants.lock().await.authorization_revision();
    let topology = state.topology.read().await.clone();
    let connected: Vec<Connected> = state
        .automation_connections
        .lock()
        .await
        .iter()
        .map(|(connection_id, peer)| Connected {
            connection_id: *connection_id,
            peer: peer.clone(),
        })
        .collect();
    let leases: Vec<crate::ControllerLease> = state
        .controller
        .lock()
        .await
        .by_id
        .values()
        .copied()
        .collect();
    let once = state.grants.lock().await.inventory();
    let engine = state.agent_access.inventory();
    // The canonical file each remembered identity names now, so a policy
    // rule for the same program is named even with the agent disconnected:
    // a Nix profile entry is walked to its physical store path (blocking).
    let canonical_targets: HashMap<DurableIdentity, PathBuf> = {
        let context = Arc::clone(&state.identity_context);
        let identities: BTreeSet<DurableIdentity> = engine
            .remembered
            .iter()
            .map(|remembered| remembered.entry.identity.clone())
            .collect();
        tokio::task::spawn_blocking(move || {
            identities
                .into_iter()
                .filter_map(|identity| {
                    let target = context.canonical_target(&identity)?;
                    Some((identity, target))
                })
                .collect()
        })
        .await
        .unwrap_or_default()
    };
    let pending = state.pending_access.inventory();
    let generation = state.policy.lock().await.snapshot();
    let now = unix_seconds_now();

    let policies: Vec<PolicyView<'_>> = generation
        .document
        .rules()
        .iter()
        .filter(|rule| {
            rule.expires_at_unix_seconds
                .is_none_or(|expires| expires > now)
        })
        .map(|rule| PolicyView {
            rule,
            lairs: rule
                .resources
                .iter()
                .filter_map(|selector| selector_lair(&topology, selector))
                .collect(),
        })
        .collect();
    let controllers_of = |grant_id: u64| {
        count(
            leases
                .iter()
                .filter(|lease| lease.grant_id == Some(grant_id))
                .count(),
        )
    };
    let mut entries = Vec::new();

    for grant in once {
        let peers: Vec<_> = connected
            .iter()
            .filter(|connection| connection.peer == grant.peer)
            .map(|connection| &connection.peer)
            .collect();
        let (lair_id, splint) = match grant.target {
            consent::GrantResource::Lair { lair_id } => (Some(lair_id), None),
            consent::GrantResource::Splint {
                splint_id,
                incarnation,
            } => (
                splint_containment(&topology, splint_id).map(|(lair_id, _, _)| lair_id),
                Some((splint_id, incarnation)),
            ),
        };
        let exact = grant
            .peer
            .persistent_executable()
            .map(|executable| (executable.path.as_path(), executable.sha256.as_str()));
        let scopes = sorted_scopes(grant.scopes.iter().copied());
        entries.push(AccessEntry {
            key: AccessEntryKey::Grant {
                grant_id: grant.grant_id,
            },
            kind: AccessEntryKind::Once,
            state: AccessEntryState::Active,
            revocable: true,
            grant_id: Some(grant.grant_id),
            identity: AccessIdentity {
                kind: AccessIdentityKind::Process,
                executable_path: process_path(&grant.peer),
                pid: Some(grant.peer.pid),
                display_name: display_name(grant.display_name.as_deref()),
            },
            // A Splint grant whose Splint left topology is still listed, with
            // no workspace, so the list shows what Revoke all access counts.
            workspace: lair_id.map(|lair_id| workspace(&topology, lair_id)),
            splint_id: splint.map(|(splint_id, _)| splint_id),
            incarnation: splint.map(|(_, incarnation)| incarnation),
            level: AccessLevel::exactly(&scopes),
            scopes,
            policy_scopes: Vec::new(),
            policy_resources: Vec::new(),
            created_at_unix_seconds: Some(grant.created_unix_seconds),
            last_used_at_unix_seconds: None,
            expires_at_unix_seconds: Some(grant.expires_at_unix_seconds),
            connections: count(peers.len()),
            controllers: controllers_of(grant.grant_id),
            policy_rules: lair_id.map_or_else(Vec::new, |lair_id| {
                continuing_rules(&policies, lair_id, exact, None, &peers)
            }),
        });
    }

    for session in &engine.sessions {
        let peers: Vec<_> = connected
            .iter()
            .filter(|connection| {
                connection.peer.uid == session.uid
                    && connection
                        .peer
                        .persistent_executable()
                        .is_some_and(|executable| {
                            executable.path == session.path && executable.sha256 == session.sha256
                        })
            })
            .map(|connection| &connection.peer)
            .collect();
        let scopes = sorted_scopes(session.access.scopes());
        entries.push(AccessEntry {
            key: AccessEntryKey::Grant {
                grant_id: session.grant_id,
            },
            kind: AccessEntryKind::WorkspaceSession,
            state: AccessEntryState::Active,
            revocable: true,
            grant_id: Some(session.grant_id),
            identity: AccessIdentity {
                kind: AccessIdentityKind::ExactBuild,
                executable_path: path_text(&session.path),
                pid: None,
                display_name: display_name(session.display_name.as_deref()),
            },
            workspace: Some(workspace(&topology, session.lair_id)),
            splint_id: None,
            incarnation: None,
            level: AccessLevel::exactly(&scopes),
            scopes,
            policy_scopes: Vec::new(),
            policy_resources: Vec::new(),
            created_at_unix_seconds: Some(session.created_unix_seconds),
            last_used_at_unix_seconds: None,
            expires_at_unix_seconds: None,
            connections: count(peers.len()),
            controllers: controllers_of(session.grant_id),
            policy_rules: continuing_rules(
                &policies,
                session.lair_id,
                Some((session.path.as_path(), session.sha256.as_str())),
                None,
                &peers,
            ),
        });
    }

    for remembered in &engine.remembered {
        let entry = &remembered.entry;
        let state_now = match (remembered.grant_id, remembered.workspace_missing) {
            (None, _) => AccessEntryState::RemovalPending,
            (Some(_), true) => AccessEntryState::WorkspaceMissing,
            (Some(_), false) => AccessEntryState::Active,
        };
        let authorizing = state_now == AccessEntryState::Active
            && engine.store == agent_access::StoreStatus::Available;
        let peers: Vec<_> = connected
            .iter()
            .filter(|connection| {
                authorizing
                    && connection.peer.durable_evidence().is_some_and(|evidence| {
                        !evidence.installed_relay && evidence.matches(&entry.identity)
                    })
            })
            .map(|connection| &connection.peer)
            .collect();
        let scopes = sorted_scopes(entry.access.scopes());
        entries.push(AccessEntry {
            key: AccessEntryKey::Remembered {
                entry_id: entry.id.as_str().to_owned(),
            },
            kind: AccessEntryKind::Remembered,
            state: state_now,
            revocable: state_now != AccessEntryState::RemovalPending,
            grant_id: remembered.grant_id,
            identity: AccessIdentity {
                kind: match entry.identity {
                    DurableIdentity::ProtectedInstall(_) => AccessIdentityKind::ProtectedInstall,
                    DurableIdentity::NixProfile(_) => AccessIdentityKind::NixProfile,
                },
                executable_path: path_text(entry.identity.path()),
                pid: None,
                display_name: display_name(entry.display_name.as_deref()),
            },
            workspace: Some(workspace(&topology, entry.lair_id)),
            splint_id: None,
            incarnation: None,
            level: AccessLevel::exactly(&scopes),
            scopes,
            policy_scopes: Vec::new(),
            policy_resources: Vec::new(),
            created_at_unix_seconds: Some(entry.created_unix_seconds),
            last_used_at_unix_seconds: entry.last_used_unix_seconds,
            expires_at_unix_seconds: None,
            connections: count(peers.len()),
            controllers: remembered.grant_id.map_or(0, controllers_of),
            policy_rules: continuing_rules(
                &policies,
                entry.lair_id,
                None,
                canonical_targets.get(&entry.identity).map(PathBuf::as_path),
                &peers,
            ),
        });
    }

    for request in pending {
        let scopes = sorted_scopes(request.scopes.iter().copied());
        entries.push(AccessEntry {
            key: AccessEntryKey::Pending {
                request_id: request.request_id,
            },
            kind: AccessEntryKind::Pending,
            state: if request.prompting {
                AccessEntryState::Prompting
            } else {
                AccessEntryState::Queued
            },
            revocable: false,
            grant_id: None,
            identity: AccessIdentity {
                kind: AccessIdentityKind::Process,
                executable_path: process_path(&request.peer),
                pid: Some(request.peer.pid),
                display_name: display_name(request.agent_name.as_deref()),
            },
            workspace: Some(workspace(&topology, request.lair_id)),
            splint_id: None,
            incarnation: None,
            level: AccessLevel::exactly(&scopes),
            scopes,
            policy_scopes: Vec::new(),
            policy_resources: Vec::new(),
            created_at_unix_seconds: Some(request.opened_unix_seconds),
            last_used_at_unix_seconds: None,
            expires_at_unix_seconds: Some(request.expires_at_unix_seconds),
            connections: count(
                connected
                    .iter()
                    .filter(|connection| connection.peer == request.peer)
                    .count(),
            ),
            controllers: 0,
            policy_rules: Vec::new(),
        });
    }

    // ADR 0014: policy rules that match a connected agent, read-only here.
    for policy in &policies {
        let matched: Vec<_> = connected
            .iter()
            .filter(|connection| policy.matches_peer(&connection.peer))
            .map(|connection| connection.connection_id)
            .collect();
        if matched.is_empty() {
            continue;
        }
        let rule = policy.rule;
        let mut policy_scopes = rule.scopes.clone();
        policy_scopes.sort_unstable();
        policy_scopes.dedup();
        entries.push(AccessEntry {
            key: AccessEntryKey::Policy {
                rule_id: rule.id.clone(),
            },
            kind: AccessEntryKind::PersistentPolicy,
            state: AccessEntryState::Active,
            revocable: false,
            grant_id: None,
            identity: AccessIdentity {
                kind: AccessIdentityKind::PolicyDigest,
                executable_path: path_text(&rule.executable.path),
                pid: None,
                display_name: None,
            },
            workspace: None,
            splint_id: None,
            incarnation: None,
            level: None,
            scopes: Vec::new(),
            policy_scopes,
            policy_resources: rule
                .resources
                .iter()
                .take(MAX_ACCESS_POLICY_ITEMS)
                .map(|selector| match selector {
                    ResourceSelector::Daemon => AccessPolicyResource::Daemon,
                    ResourceSelector::Lair { lair_id } => {
                        AccessPolicyResource::Lair { lair_id: *lair_id }
                    }
                    ResourceSelector::Dojo { dojo_id } => {
                        AccessPolicyResource::Dojo { dojo_id: *dojo_id }
                    }
                    ResourceSelector::Splint { splint_id, .. } => AccessPolicyResource::Splint {
                        splint_id: *splint_id,
                    },
                })
                .collect(),
            created_at_unix_seconds: None,
            last_used_at_unix_seconds: None,
            expires_at_unix_seconds: rule.expires_at_unix_seconds,
            connections: count(matched.len()),
            // Policy controllers carry no grant; count those held by the
            // rule's connected processes.
            controllers: count(
                leases
                    .iter()
                    .filter(|lease| {
                        lease.grant_id.is_none() && matched.contains(&lease.connection_id)
                    })
                    .count(),
            ),
            policy_rules: Vec::new(),
        });
    }

    entries.sort_by(|left, right| left.key.cmp(&right.key));
    entries.dedup_by(|right, left| left.key == right.key);
    entries.retain(|entry| {
        let valid = entry.validate();
        if let Err(reason) = valid {
            tracing::error!(?entry.key, reason, "access inventory entry left out");
        }
        valid.is_ok()
    });
    Snapshot {
        authorization_revision,
        policy_generation: generation.id,
        remembered_store: match engine.store {
            agent_access::StoreStatus::Available => RememberedStoreStatus::Available,
            agent_access::StoreStatus::NotConfigured => RememberedStoreStatus::NotConfigured,
            agent_access::StoreStatus::Unavailable(reason) => RememberedStoreStatus::Unavailable {
                reason: bounded(reason, MAX_ACCESS_FAULT_BYTES),
            },
        },
        entries,
    }
}

/// One page of the inventory.
///
/// # Errors
/// A page size outside `1..=`[`MAX_ACCESS_INVENTORY_PAGE`] or a malformed
/// continuation key.
pub(crate) async fn list(
    state: &DaemonState,
    after: Option<AccessEntryKey>,
    max_entries: u16,
) -> Result<Response, ProtocolError> {
    if max_entries == 0 || max_entries > MAX_ACCESS_INVENTORY_PAGE {
        return Err(ProtocolError::new(
            ErrorCode::InvalidArgument,
            "access inventory page size is out of bounds",
        ));
    }
    if after.as_ref().is_some_and(|key| !key.is_valid()) {
        return Err(ProtocolError::new(
            ErrorCode::InvalidArgument,
            "access inventory continuation key is malformed",
        ));
    }
    let snapshot = snapshot(state).await;
    let total_entries = count(snapshot.entries.len());
    let mut remaining = snapshot
        .entries
        .into_iter()
        .filter(|entry| after.as_ref().is_none_or(|after| entry.key > *after))
        .peekable();
    let entries: Vec<_> = remaining.by_ref().take(usize::from(max_entries)).collect();
    let next_after = remaining
        .peek()
        .is_some()
        .then(|| entries.last().map(|entry| entry.key.clone()))
        .flatten();
    Ok(Response::AccessInventory {
        authorization_revision: snapshot.authorization_revision,
        policy_generation: snapshot.policy_generation,
        remembered_store: snapshot.remembered_store,
        total_entries,
        entries,
        next_after,
    })
}

/// The workspace a revoke target covers, for policy authorization: `None`
/// when no such revocable entry exists, `Some(None)` for a remembered entry
/// whose workspace is gone (authorized on the daemon instead).
pub(crate) async fn target_lair(
    state: &DaemonState,
    target: &AccessRevokeTarget,
) -> Option<Option<LairId>> {
    let lair_id = match target {
        AccessRevokeTarget::Grant { grant_id } => {
            let once = state.grants.lock().await.grant_resource(*grant_id);
            match once {
                Some(consent::GrantResource::Lair { lair_id }) => Some(lair_id),
                Some(consent::GrantResource::Splint { splint_id, .. }) => {
                    splint_containment(&*state.topology.read().await, splint_id)
                        .map(|(lair_id, _, _)| lair_id)
                }
                None => Some(state.agent_access.session_lair(*grant_id)?),
            }
        }
        AccessRevokeTarget::Remembered { entry_id } => {
            let entry_id = agent_access::EntryId::parse(entry_id)?;
            Some(state.agent_access.remembered_lair(&entry_id)?)
        }
    };
    let topology = state.topology.read().await;
    Some(lair_id.filter(|lair_id| topology.find_lair(*lair_id).is_some()))
}

fn not_found(message: &'static str) -> ProtocolError {
    ProtocolError::new(ErrorCode::NotFound, message)
}

fn durable_outcome(result: Result<(), agent_access::StoreWriteError>) -> DurableRemoval {
    match result {
        Ok(()) => DurableRemoval::Written,
        Err(error) => DurableRemoval::Failed {
            message: bounded(error.to_string(), MAX_ACCESS_FAULT_BYTES),
        },
    }
}

/// Revokes one grant definition.
///
/// # Errors
/// `invalid_argument` for a malformed entry ID or a remembered entry's
/// runtime grant ID (revoke it by its entry ID); `not_found` when nothing
/// revocable has that identifier.
pub(crate) async fn revoke_entry(
    state: &Arc<DaemonState>,
    target: &AccessRevokeTarget,
) -> Result<Response, ProtocolError> {
    let key = target.key();
    if !key.is_valid() {
        return Err(ProtocolError::new(
            ErrorCode::InvalidArgument,
            "the access entry identifier is malformed",
        ));
    }
    if let AccessRevokeTarget::Grant { grant_id } = target
        && state.agent_access.is_remembered_grant(*grant_id)
    {
        return Err(ProtocolError::new(
            ErrorCode::InvalidArgument,
            "that runtime grant belongs to a remembered entry; revoke it by its entry ID",
        ));
    }
    // What the entry looked like, for the policy rules that outlive it.
    let before = snapshot(state)
        .await
        .entries
        .into_iter()
        .find(|entry| entry.key == key);
    let policy_rules = before
        .as_ref()
        .map(|entry| entry.policy_rules.clone())
        .unwrap_or_default();
    let listed_lair = before
        .as_ref()
        .and_then(|entry| entry.workspace.as_ref())
        .map(|workspace| workspace.lair_id);
    let revocation = match target {
        AccessRevokeTarget::Grant { grant_id } => {
            let grant_id = *grant_id;
            let revoked_once = {
                let mut grants = state.grants.lock().await;
                let resource = grants.grant_resource(grant_id);
                grants.revoke_any(grant_id).map(|_| resource)
            };
            if let Some(resource) = revoked_once {
                revoke_grant_controllers(state, grant_id).await;
                let _ = state.revocations.send(Revocation { grant_id });
                let lair_id = match resource {
                    Some(consent::GrantResource::Lair { lair_id }) => Some(lair_id),
                    _ => listed_lair,
                };
                AccessRevocation {
                    key,
                    kind: AccessEntryKind::Once,
                    grant_id,
                    lair_id,
                    durable_removal: DurableRemoval::NotNeeded,
                    policy_rules,
                }
            } else {
                let lair_id = state.agent_access.session_lair(grant_id);
                if !revoke_session_grant(state, grant_id).await {
                    return Err(not_found("no revocable grant has that identifier"));
                }
                AccessRevocation {
                    key,
                    kind: AccessEntryKind::WorkspaceSession,
                    grant_id,
                    lair_id,
                    durable_removal: DurableRemoval::NotNeeded,
                    policy_rules,
                }
            }
        }
        AccessRevokeTarget::Remembered { entry_id } => {
            let entry_id = agent_access::EntryId::parse(entry_id).ok_or_else(|| {
                ProtocolError::new(
                    ErrorCode::InvalidArgument,
                    "the remembered entry ID is malformed",
                )
            })?;
            let lair_id = state.agent_access.remembered_lair(&entry_id);
            let revoked = revoke_remembered_grant(state, &entry_id)
                .await
                .ok_or_else(|| not_found("no remembered grant has that entry ID"))?;
            AccessRevocation {
                key,
                kind: AccessEntryKind::Remembered,
                grant_id: revoked.grant_id,
                lair_id,
                durable_removal: durable_outcome(revoked.durable),
                policy_rules,
            }
        }
    };
    tracing::info!(
        grant_id = revocation.grant_id,
        kind = ?revocation.kind,
        durable = ?revocation.durable_removal,
        "access entry revoked"
    );
    Ok(Response::AccessEntryRevoked {
        authorization_revision: state.grants.lock().await.authorization_revision(),
        revocation,
    })
}

/// **Revoke all access**, or with `dry_run` its exact counts.
///
/// # Errors
/// Only when the durable removal task cannot run at all.
pub(crate) async fn revoke_all(
    state: &Arc<DaemonState>,
    dry_run: bool,
) -> Result<Response, ProtocolError> {
    if dry_run {
        let once = state.grants.lock().await.live_count();
        let (sessions, remembered) = state.agent_access.revocable_counts();
        return Ok(Response::AllAccessRevoked {
            authorization_revision: state.grants.lock().await.authorization_revision(),
            dry_run: true,
            counts: AccessRevocationCounts {
                once: count(once),
                workspace_session: count(sessions),
                remembered: count(remembered),
                pending_requests: count(state.pending_access.live_count()),
            },
            durable_removal: DurableRemoval::NotNeeded,
        });
    }
    // Pending requests end first, so an answer being committed now is
    // revoked by its committer rather than surviving this revoke.
    let pending_requests = state.pending_access.cancel_all();
    let once = state.grants.lock().await.revoke_all(REVOKE_ALL_REASON);
    let engine = state.agent_access.revoke_all(REVOKE_ALL_REASON);
    let revoked: Vec<u64> = once
        .iter()
        .chain(&engine.session_grant_ids)
        .chain(&engine.remembered_grant_ids)
        .copied()
        .collect();
    for grant_id in &revoked {
        revoke_grant_controllers(state, *grant_id).await;
        let _ = state.revocations.send(Revocation {
            grant_id: *grant_id,
        });
    }
    let durable_removal = if state.agent_access.flush_needed() {
        let access = Arc::clone(&state.agent_access);
        durable_outcome(
            tokio::task::spawn_blocking(move || access.flush())
                .await
                .map_err(|_| internal())?,
        )
    } else if !engine.remembered_grant_ids.is_empty() {
        // Remembered grants were removed and nothing is owed: a concurrent
        // write already took them out of the store.
        DurableRemoval::Written
    } else {
        DurableRemoval::NotNeeded
    };
    let counts = AccessRevocationCounts {
        once: count(once.len()),
        workspace_session: count(engine.session_grant_ids.len()),
        remembered: count(engine.remembered_grant_ids.len()),
        pending_requests: count(pending_requests),
    };
    tracing::info!(?counts, durable = ?durable_removal, "all access revoked");
    Ok(Response::AllAccessRevoked {
        authorization_revision: state.grants.lock().await.authorization_revision(),
        dry_run: false,
        counts,
        durable_removal,
    })
}

/// Whether the runtime grant behind a controller or subscription is still in
/// force. Called under the controller lock (or after subscribing to
/// revocations), after authorization: every revoke removes the grant before
/// it takes that lock and before it broadcasts, so a request authorized just
/// before a revoke cannot register afterwards.
pub(crate) async fn grant_live(state: &DaemonState, grant_id: Option<u64>) -> bool {
    let Some(grant_id) = grant_id else {
        return true;
    };
    state.grants.lock().await.is_live(grant_id) || state.agent_access.is_live(grant_id)
}

/// The error a request gets when its grant was revoked after authorization.
pub(crate) fn revoked_grant() -> ProtocolError {
    ProtocolError::new(
        ErrorCode::Unauthorized,
        "the grant that authorized this request was revoked",
    )
}

/// Keeps the private map type in one place for the connection registry.
pub(crate) type AutomationPeers = HashMap<u64, consent::PeerIdentity>;

/// [`grant_live`] plus how long a terminal subscription may run, for a
/// subscription registering after authorization: a revoked grant is refused;
/// a once grant gives its remaining lifetime (it also ends by its expiry
/// broadcast); a while-open or remembered grant, policy, or trusted
/// authority gives no timer, so it ends only by revoke or workspace end.
///
/// # Errors
/// The grant was revoked (or expired) after authorization.
pub(crate) async fn checked_subscription_lifetime(
    state: &DaemonState,
    grant_id: Option<u64>,
) -> Result<Option<std::time::Duration>, ProtocolError> {
    let Some(grant_id) = grant_id else {
        return Ok(None);
    };
    if let Some(remaining) = state.grants.lock().await.once_remaining(grant_id) {
        return Ok(Some(remaining));
    }
    if state.agent_access.is_live(grant_id) {
        Ok(None)
    } else {
        Err(revoked_grant())
    }
}
