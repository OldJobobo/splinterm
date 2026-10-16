//! Explicitly reported agent display status, held in daemon memory only.
//!
//! A status is display data bound to one live Splint incarnation. It never
//! grants authority, never bumps the topology revision, is never persisted or
//! carried through handoff, and its free-text detail never reaches audit,
//! diagnostics, or traces. One timer task drives TTL expiry, stale retention,
//! and coalesced publication for every Splint through an ordered deadline
//! index; no task is spawned per Splint.

use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    fs,
    sync::{Arc, Mutex as StdMutex, Weak},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use splinterm_core::{SplintId, SplintState};
use splinterm_protocol::{
    AccessScope, AgentState, AgentStatusReport, AgentStatusSummary, ErrorCode, ProtocolError,
    Request, Response, SubscriptionEvent, TopologySnapshot,
};
use tokio::{sync::Notify, task::JoinHandle, time};
use tracing::warn;

use crate::{
    DaemonState, MAX_LIVE_SPLINTS, PeerIdentity, RequestAuditDisposition,
    RequestAuthorizationContext, append_request_audit, audit_resource, cgroup, internal, not_found,
    request_policy_resources,
};

/// An expired active status stays visible as stale for this long, then drops.
pub(crate) const STALE_RETENTION: Duration = Duration::from_secs(15 * 60);
/// Minimum spacing of published status events for one Splint.
pub(crate) const PUBLISH_INTERVAL: Duration = Duration::from_millis(250);
/// Admission window and limit for reports targeting one Splint.
pub(crate) const ADMISSION_WINDOW: Duration = Duration::from_secs(1);
pub(crate) const ADMISSIONS_PER_WINDOW: usize = 10;
/// Upper bound on the `/proc` parent walk from the reporting peer.
pub(crate) const MAX_ANCESTRY_STEPS: usize = 64;
const IDLE_TIMER: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug)]
struct CurrentStatus {
    state: AgentState,
    detail: Option<String>,
    reported_at: Instant,
    /// Deadline for an expiring status that is not yet stale.
    expires_at: Option<Instant>,
    stale_since: Option<Instant>,
}

#[derive(Debug)]
struct Slot {
    key: u64,
    incarnation: u64,
    current: Option<CurrentStatus>,
    /// Sequence of the latest visible change for this incarnation, or zero.
    sequence: u64,
    admissions: VecDeque<Instant>,
    last_published: Option<Instant>,
    /// When the latest unpublished change was made, if any.
    pending_since: Option<Instant>,
    deadline: Option<Instant>,
    rate_refusal_audited: bool,
}

impl Slot {
    fn summary(&self, now: Instant) -> Option<AgentStatusSummary> {
        let current = self.current.as_ref()?;
        Some(AgentStatusSummary {
            state: current.state,
            detail: current.detail.clone(),
            sequence: self.sequence,
            stale: current.stale_since.is_some(),
            reported_age_ms: u64::try_from(
                now.saturating_duration_since(current.reported_at)
                    .as_millis(),
            )
            .unwrap_or(u64::MAX),
        })
    }

    fn next_deadline(&self) -> Option<Instant> {
        let status = self.current.as_ref().and_then(|current| {
            current
                .stale_since
                .map_or(current.expires_at, |stale_since| {
                    Some(stale_since + STALE_RETENTION)
                })
        });
        let publish = self.pending_since.map(|pending_since| {
            self.last_published
                .map_or(pending_since, |published| published + PUBLISH_INTERVAL)
        });
        match (status, publish) {
            (Some(status), Some(publish)) => Some(status.min(publish)),
            (status, publish) => status.or(publish),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Admission {
    Admitted,
    RateLimited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Accepted {
    pub(crate) sequence: u64,
    pub(crate) ttl: Option<Duration>,
    /// A visible change that must be published; identical refreshes are not.
    pub(crate) changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusEvent {
    pub(crate) splint_id: SplintId,
    pub(crate) incarnation: u64,
    pub(crate) status: Option<AgentStatusSummary>,
    pub(crate) sequence: u64,
}

impl From<StatusEvent> for SubscriptionEvent {
    fn from(event: StatusEvent) -> Self {
        Self::AgentStatusChanged {
            splint_id: event.splint_id,
            incarnation: event.incarnation,
            status: event.status,
            sequence: event.sequence,
        }
    }
}

/// At most one status per Splint, bound to one incarnation.
#[derive(Debug, Default)]
pub(crate) struct AgentStatusStore {
    slots: HashMap<SplintId, Slot>,
    keys: HashMap<u64, SplintId>,
    /// Ordered deadline index: one entry per scheduled slot.
    deadlines: BTreeSet<(Instant, u64)>,
    next_key: u64,
    sequence: u64,
}

impl AgentStatusStore {
    fn next_sequence(&mut self) -> u64 {
        self.sequence = self.sequence.saturating_add(1);
        self.sequence
    }

    fn reschedule(&mut self, splint_id: SplintId) {
        let Some(slot) = self.slots.get_mut(&splint_id) else {
            return;
        };
        if let Some(previous) = slot.deadline.take() {
            self.deadlines.remove(&(previous, slot.key));
        }
        slot.deadline = slot.next_deadline();
        if let Some(deadline) = slot.deadline {
            self.deadlines.insert((deadline, slot.key));
        }
    }

    fn remove(&mut self, splint_id: SplintId) {
        if let Some(slot) = self.slots.remove(&splint_id) {
            self.keys.remove(&slot.key);
            if let Some(deadline) = slot.deadline {
                self.deadlines.remove(&(deadline, slot.key));
            }
        }
    }

    /// Removes the status of `splint_id` only while it still belongs to
    /// `incarnation`.
    pub(crate) fn remove_incarnation(&mut self, splint_id: SplintId, incarnation: u64) {
        if self
            .slots
            .get(&splint_id)
            .is_some_and(|slot| slot.incarnation == incarnation)
        {
            self.remove(splint_id);
        }
    }

    /// Counts one report against the per-Splint admission window. A new
    /// incarnation starts from an empty slot.
    ///
    /// # Errors
    ///
    /// Returns `ResourceLimit` if the bounded slot map is full.
    pub(crate) fn admit(
        &mut self,
        splint_id: SplintId,
        incarnation: u64,
        now: Instant,
    ) -> Result<Admission, ProtocolError> {
        if self
            .slots
            .get(&splint_id)
            .is_some_and(|slot| slot.incarnation != incarnation)
        {
            self.remove(splint_id);
        }
        if !self.slots.contains_key(&splint_id) {
            if self.slots.len() >= MAX_LIVE_SPLINTS {
                return Err(ProtocolError::new(
                    ErrorCode::ResourceLimit,
                    "agent status capacity is exhausted",
                ));
            }
            self.next_key = self.next_key.wrapping_add(1);
            let key = self.next_key;
            self.keys.insert(key, splint_id);
            self.slots.insert(
                splint_id,
                Slot {
                    key,
                    incarnation,
                    current: None,
                    sequence: 0,
                    admissions: VecDeque::with_capacity(ADMISSIONS_PER_WINDOW),
                    last_published: None,
                    pending_since: None,
                    deadline: None,
                    rate_refusal_audited: false,
                },
            );
        }
        let slot = self.slots.get_mut(&splint_id).expect("slot was ensured");
        while slot
            .admissions
            .front()
            .is_some_and(|admitted| now.saturating_duration_since(*admitted) >= ADMISSION_WINDOW)
        {
            slot.admissions.pop_front();
        }
        if slot.admissions.len() >= ADMISSIONS_PER_WINDOW {
            return Ok(Admission::RateLimited);
        }
        slot.admissions.push_back(now);
        Ok(Admission::Admitted)
    }

    /// Returns true exactly once per incarnation, for the audit of a
    /// self-report rate refusal.
    pub(crate) fn claim_rate_refusal_audit(
        &mut self,
        splint_id: SplintId,
        incarnation: u64,
    ) -> bool {
        self.slots
            .get_mut(&splint_id)
            .filter(|slot| slot.incarnation == incarnation)
            .is_some_and(|slot| !std::mem::replace(&mut slot.rate_refusal_audited, true))
    }

    /// Applies an admitted report. `None` clears.
    pub(crate) fn apply(
        &mut self,
        splint_id: SplintId,
        incarnation: u64,
        report: Option<&AgentStatusReport>,
        now: Instant,
    ) -> Accepted {
        let identical = self.slots.get(&splint_id).is_some_and(|slot| {
            slot.incarnation == incarnation
                && slot
                    .current
                    .as_ref()
                    .zip(report)
                    .is_some_and(|(current, report)| {
                        current.stale_since.is_none()
                            && current.state == report.state
                            && current.detail == report.detail
                    })
        });
        let had_status = self
            .slots
            .get(&splint_id)
            .is_some_and(|slot| slot.incarnation == incarnation && slot.current.is_some());
        let ttl = report
            .and_then(AgentStatusReport::effective_ttl_seconds)
            .map(|seconds| Duration::from_secs(u64::from(seconds)));
        let changed = !identical && (report.is_some() || had_status);
        let sequence = changed.then(|| self.next_sequence());
        let Some(slot) = self
            .slots
            .get_mut(&splint_id)
            .filter(|slot| slot.incarnation == incarnation)
        else {
            return Accepted {
                sequence: 0,
                ttl,
                changed: false,
            };
        };
        match report {
            Some(_) if identical => {
                let current = slot.current.as_mut().expect("identical status exists");
                current.reported_at = now;
                current.expires_at = ttl.map(|ttl| now + ttl);
            }
            Some(report) => {
                slot.current = Some(CurrentStatus {
                    state: report.state,
                    detail: report.detail.clone(),
                    reported_at: now,
                    expires_at: ttl.map(|ttl| now + ttl),
                    stale_since: None,
                });
            }
            None => slot.current = None,
        }
        if let Some(sequence) = sequence {
            slot.sequence = sequence;
            slot.pending_since = Some(now);
        }
        let sequence = slot.sequence;
        self.reschedule(splint_id);
        Accepted {
            sequence,
            ttl,
            changed,
        }
    }

    /// Drops every slot whose Splint is not live at the recorded incarnation.
    pub(crate) fn retain_live(&mut self, live: &HashMap<SplintId, u64>) {
        let retired = self
            .slots
            .iter()
            .filter(|(splint_id, slot)| live.get(splint_id) != Some(&slot.incarnation))
            .map(|(splint_id, _)| *splint_id)
            .collect::<Vec<_>>();
        for splint_id in retired {
            self.remove(splint_id);
        }
    }

    pub(crate) fn summary(
        &self,
        splint_id: SplintId,
        incarnation: u64,
        now: Instant,
    ) -> Option<AgentStatusSummary> {
        self.slots
            .get(&splint_id)
            .filter(|slot| slot.incarnation == incarnation)
            .and_then(|slot| slot.summary(now))
    }

    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.deadlines.first().map(|(deadline, _)| *deadline)
    }

    /// Applies expiry and stale retention, then returns the coalesced events
    /// whose publication interval has elapsed.
    pub(crate) fn take_due(&mut self, now: Instant) -> Vec<StatusEvent> {
        let due = self
            .deadlines
            .range(..=(now, u64::MAX))
            .filter_map(|(_, key)| self.keys.get(key).copied())
            .collect::<Vec<_>>();
        let mut events = Vec::new();
        for splint_id in due {
            let transition = self.slots[&splint_id]
                .current
                .as_ref()
                .is_some_and(|current| {
                    let expired = current.stale_since.is_none()
                        && current.expires_at.is_some_and(|expires| expires <= now);
                    let dropped = current
                        .stale_since
                        .is_some_and(|stale_since| stale_since + STALE_RETENTION <= now);
                    expired || dropped
                });
            let sequence = transition.then(|| self.next_sequence());
            let slot = self.slots.get_mut(&splint_id).expect("indexed slot exists");
            if let Some(sequence) = sequence {
                let current = slot.current.as_mut().expect("status exists");
                if current.stale_since.is_some() {
                    slot.current = None;
                } else {
                    current.stale_since = Some(now);
                    current.expires_at = None;
                }
                slot.sequence = sequence;
                slot.pending_since = Some(now);
            }
            if slot.pending_since.is_some()
                && slot
                    .last_published
                    .is_none_or(|published| published + PUBLISH_INTERVAL <= now)
            {
                slot.pending_since = None;
                slot.last_published = Some(now);
                events.push(StatusEvent {
                    splint_id,
                    incarnation: slot.incarnation,
                    status: slot.summary(now),
                    sequence: slot.sequence,
                });
            }
            self.reschedule(splint_id);
        }
        events
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.slots.len()
    }
}

/// Daemon-owned status store plus the wakeup for its single timer task.
#[derive(Debug, Default)]
pub(crate) struct AgentStatusService {
    store: StdMutex<AgentStatusStore>,
    changed: Notify,
}

impl AgentStatusService {
    fn store(&self) -> std::sync::MutexGuard<'_, AgentStatusStore> {
        self.store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Prunes statuses of retired incarnations and fills runtime summaries.
    ///
    /// Every topology snapshot passes through here, so the transaction that
    /// publishes an exit, relaunch, restore, close, or termination also drops
    /// the retired incarnation's status.
    pub(crate) fn attach(&self, snapshot: &mut TopologySnapshot) {
        let live = snapshot
            .runtimes
            .iter()
            .filter_map(|runtime| Some((runtime.splint_id, runtime.live_incarnation?)))
            .collect::<HashMap<_, _>>();
        let now = Instant::now();
        let mut store = self.store();
        store.retain_live(&live);
        for runtime in &mut snapshot.runtimes {
            runtime.agent_status = runtime
                .live_incarnation
                .and_then(|incarnation| store.summary(runtime.splint_id, incarnation, now));
        }
    }
}

/// Starts the one daemon-wide status timer. It holds only a weak daemon
/// reference and is aborted when the returned handle is dropped.
pub(crate) fn spawn_timer(state: &Arc<DaemonState>) -> JoinHandle<()> {
    let service = Arc::clone(&state.agent_status);
    let state = Arc::downgrade(state);
    tokio::spawn(run_timer(service, state))
}

async fn run_timer(service: Arc<AgentStatusService>, state: Weak<DaemonState>) {
    loop {
        let deadline = service
            .store()
            .next_deadline()
            .unwrap_or_else(|| Instant::now() + IDLE_TIMER);
        tokio::select! {
            () = service.changed.notified() => {}
            () = time::sleep_until(time::Instant::from_std(deadline)) => {}
        }
        let Some(state) = state.upgrade() else {
            break;
        };
        publish_due(&state).await;
    }
}

/// Publishes due status events in topology-transaction order, so a status
/// event never precedes an older topology snapshot on any subscription.
async fn publish_due(state: &DaemonState) {
    // A change coalesced inside its publication interval wakes the timer
    // only to reschedule; skip the transaction and snapshot until due.
    if state
        .agent_status
        .store()
        .next_deadline()
        .is_none_or(|deadline| deadline > Instant::now())
    {
        return;
    }
    let Ok(_transaction) = state.topology_transactions.acquire().await else {
        return;
    };
    // Building the snapshot prunes statuses of retired incarnations first.
    let snapshot = crate::topology_snapshot(state).await;
    let events = state.agent_status.store().take_due(Instant::now());
    if events.is_empty() {
        return;
    }
    let mut hub = state.topology_hub.lock().await;
    for event in events {
        hub.publish_agent_status(event.into(), snapshot.revision);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelfReportPath {
    Session,
    Ancestry,
    WorkloadScope,
}

/// Parses `(ppid, session)` from `/proc/<pid>/stat`, tolerating any command
/// name by splitting after its final closing parenthesis.
fn parse_proc_stat(contents: &str) -> Option<(u32, u32)> {
    let rest = &contents[contents.rfind(')')? + 1..];
    let mut fields = rest.split_ascii_whitespace();
    let _state = fields.next()?;
    let parent = fields.next()?.parse().ok()?;
    let _process_group = fields.next()?;
    let session = fields.next()?.parse().ok()?;
    Some((parent, session))
}

fn read_proc_stat(pid: u32) -> Option<(u32, u32)> {
    parse_proc_stat(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

fn cgroup_contains_scope(contents: &str, scope: &str) -> bool {
    contents
        .lines()
        .filter_map(|line| line.strip_prefix("0::"))
        .any(|path| path.split('/').any(|component| component == scope))
}

/// Locates the peer inside the Splint whose live session leader is
/// `child_pid`. The validated PTY identity requires child, process group, and
/// session to be one PID, so the recorded session ID is `child_pid`. Checks:
/// same session, else a bounded parent walk reaching the child,
/// else (when workload units are active) the incarnation's cgroup scope.
pub(crate) fn locate_peer(
    peer_pid: u32,
    child_pid: u32,
    workload_scope: Option<&str>,
) -> Option<SelfReportPath> {
    if peer_pid <= 1 || child_pid <= 1 {
        return None;
    }
    let (mut parent, session) = read_proc_stat(peer_pid)?;
    if session == child_pid {
        return Some(SelfReportPath::Session);
    }
    for _ in 0..MAX_ANCESTRY_STEPS {
        if parent == child_pid {
            return Some(SelfReportPath::Ancestry);
        }
        if parent <= 1 {
            break;
        }
        let Some((next, _)) = read_proc_stat(parent) else {
            break;
        };
        parent = next;
    }
    let scope = workload_scope?;
    fs::read_to_string(format!("/proc/{peer_pid}/cgroup"))
        .ok()
        .filter(|contents| cgroup_contains_scope(contents, scope))
        .map(|_| SelfReportPath::WorkloadScope)
}

struct LiveTarget {
    incarnation: u64,
    handle: splinterd::LiveSplintHandle,
}

async fn live_target(
    state: &DaemonState,
    splint_id: SplintId,
) -> Result<LiveTarget, ProtocolError> {
    let exited = state
        .topology
        .read()
        .await
        .find_splint(splint_id)
        .map(|splint| matches!(splint.state, SplintState::Exited(_)))
        .ok_or_else(not_found)?;
    let handle = state
        .runtimes
        .lock()
        .await
        .handle(splint_id)
        .ok_or_else(not_found)?;
    if exited || !handle.root_process_is_live() {
        return Err(stale_incarnation());
    }
    Ok(LiveTarget {
        incarnation: handle.incarnation.value(),
        handle,
    })
}

fn stale_incarnation() -> ProtocolError {
    ProtocolError::new(
        ErrorCode::StaleIncarnation,
        "agent status incarnation is not live",
    )
}

fn rate_limited() -> ProtocolError {
    ProtocolError::new(
        ErrorCode::RateLimited,
        "agent status reports exceed 10 per second for this Splint; retry later",
    )
}

fn unix_millis_after(ttl: Duration) -> u64 {
    SystemTime::now()
        .checked_add(ttl)
        .and_then(|deadline| deadline.duration_since(UNIX_EPOCH).ok())
        .map_or(u64::MAX, |deadline| {
            u64::try_from(deadline.as_millis()).unwrap_or(u64::MAX)
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReportAuthority {
    SelfReport,
    Granted,
}

async fn accept_report(
    state: &DaemonState,
    peer: &PeerIdentity,
    request: &Request,
    target: &LiveTarget,
    authority: ReportAuthority,
) -> Result<Response, ProtocolError> {
    let Request::ReportAgentStatus {
        splint_id,
        incarnation,
        status,
    } = request
    else {
        return Err(internal());
    };
    let (splint_id, incarnation) = (*splint_id, *incarnation);
    if let Some(report) = status {
        report.validate()?;
    }
    if target.incarnation != incarnation {
        return Err(stale_incarnation());
    }
    let now = Instant::now();
    let admission = state
        .agent_status
        .store()
        .admit(splint_id, incarnation, now)?;
    if admission == Admission::RateLimited {
        let audit_once = authority == ReportAuthority::SelfReport
            && state
                .agent_status
                .store()
                .claim_rate_refusal_audit(splint_id, incarnation);
        if audit_once {
            let resource = request_policy_resources(request, state)
                .await
                .unwrap_or_default()
                .first()
                .copied()
                .and_then(audit_resource);
            append_request_audit(
                state,
                peer,
                request,
                None,
                RequestAuditDisposition {
                    resource,
                    decision: splinterm_protocol::AuditDecision::Rejected,
                    reason: "self_report_rate_limited",
                    outcome: splinterm_protocol::AuditOutcome::Failed,
                },
            )
            .await;
        }
        return Err(rate_limited());
    }
    let accepted = state
        .agent_status
        .store()
        .apply(splint_id, incarnation, status.as_ref(), now);
    // A concurrent exit may have published between the liveness check and the
    // insertion; never leave a status behind for a retired incarnation.
    if live_target(state, splint_id)
        .await
        .is_ok_and(|current| current.incarnation == incarnation)
    {
        if accepted.changed {
            state.agent_status.changed.notify_one();
        }
    } else {
        state
            .agent_status
            .store()
            .remove_incarnation(splint_id, incarnation);
        return Err(stale_incarnation());
    }
    Ok(Response::AgentStatusAccepted {
        sequence: accepted.sequence,
        expires_at_ms: accepted.ttl.map(unix_millis_after),
    })
}

/// Accepts a report from a process inside the target Splint without a grant
/// and without a per-update audit record. Returns `None` when the peer is not
/// inside the Splint, so ordinary authorization and audit apply.
pub(crate) async fn handle_self_report(
    state: &DaemonState,
    peer: &PeerIdentity,
    request: &Request,
) -> Option<Result<Response, ProtocolError>> {
    let Request::ReportAgentStatus { splint_id, .. } = request else {
        return None;
    };
    // Only pidfd-monitored connections: the numeric peer PID cannot be reused
    // while the connection lives, because peer exit closes it.
    peer.persistent_executable()?;
    let target = if let Ok(target) = live_target(state, *splint_id).await {
        target
    } else {
        // A Splint's first process can report before the transaction that
        // creates, relaunches, or restores it commits; wait that out once.
        drop(state.topology_transactions.acquire().await.ok()?);
        live_target(state, *splint_id).await.ok()?
    };
    let workload_scope = state
        .workload_units
        .is_some()
        .then(|| cgroup::splint_scope_unit_name(*splint_id, target.incarnation));
    let (peer_pid, child_pid) = (peer.pid, target.handle.child_pid());
    let located = tokio::task::spawn_blocking(move || {
        locate_peer(peer_pid, child_pid, workload_scope.as_deref())
    })
    .await
    .unwrap_or_else(|error| {
        warn!(%error, "agent status peer lookup task failed");
        None
    })?;
    // The session leader is still the live child, so its PID and session ID
    // could not have been reused during the walk.
    if !target.handle.root_process_is_live() {
        return None;
    }
    tracing::debug!(path = ?located, "agent status self-report");
    Some(accept_report(state, peer, request, &target, ReportAuthority::SelfReport).await)
}

/// Accepts a report that passed ordinary authorization: persistent policy,
/// a Lair grant, a Splint grant with `agent_status_report`, or the
/// development bypass. The trusted UI may only clear.
pub(crate) async fn handle_authorized_report(
    state: &DaemonState,
    peer: &PeerIdentity,
    authorization: &RequestAuthorizationContext,
    request: &Request,
) -> Result<Response, ProtocolError> {
    let Request::ReportAgentStatus {
        splint_id,
        incarnation,
        status,
    } = request
    else {
        return Err(internal());
    };
    let granted = authorization.policy_authorized()
        || authorization.authorized_grant_id().is_some()
        || state.development_terminal_access;
    if !granted {
        if authorization.interactive_bypass {
            if status.is_some() {
                return Err(ProtocolError::new(
                    ErrorCode::Unauthorized,
                    "the trusted UI may only clear agent status",
                ));
            }
        } else if state
            .grants
            .lock()
            .await
            .authorize(
                peer,
                *splint_id,
                *incarnation,
                &[AccessScope::AgentStatusReport],
            )
            .is_none()
        {
            return Err(ProtocolError::new(
                ErrorCode::Unauthorized,
                "agent status requires a process inside the Splint or an agent_status_report grant",
            ));
        }
    }
    let target = live_target(state, *splint_id).await?;
    accept_report(state, peer, request, &target, ReportAuthority::Granted).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(state: AgentState, detail: Option<&str>, ttl: Option<u32>) -> AgentStatusReport {
        AgentStatusReport {
            state,
            detail: detail.map(str::to_owned),
            ttl_seconds: ttl,
        }
    }

    fn admitted(store: &mut AgentStatusStore, splint_id: SplintId, now: Instant) {
        assert_eq!(store.admit(splint_id, 1, now).unwrap(), Admission::Admitted);
    }

    #[test]
    fn active_status_expires_to_stale_then_drops_after_retention() {
        let mut store = AgentStatusStore::default();
        let splint_id = SplintId::new();
        let start = Instant::now();
        admitted(&mut store, splint_id, start);
        let accepted = store.apply(
            splint_id,
            1,
            Some(&report(AgentState::Working, Some("compiling"), None)),
            start,
        );
        assert!(accepted.changed);
        assert_eq!(accepted.ttl, Some(Duration::from_secs(120)));
        let first = store.take_due(start);
        assert_eq!(first.len(), 1, "the first change publishes immediately");
        assert_eq!(first[0].status.as_ref().unwrap().state, AgentState::Working);
        assert_eq!(
            store.next_deadline(),
            Some(start + Duration::from_secs(120))
        );

        assert!(store.take_due(start + Duration::from_secs(119)).is_empty());
        let expiry = start + Duration::from_secs(120);
        let stale = store.take_due(expiry);
        assert_eq!(stale.len(), 1);
        let summary = stale[0].status.as_ref().unwrap();
        assert!(summary.stale);
        assert_eq!(summary.state, AgentState::Working);
        assert_eq!(summary.detail.as_deref(), Some("compiling"));
        assert_eq!(summary.reported_age_ms, 120_000);
        assert!(stale[0].sequence > first[0].sequence);
        assert!(store.summary(splint_id, 1, expiry).unwrap().stale);

        assert_eq!(store.next_deadline(), Some(expiry + STALE_RETENTION));
        assert!(
            store
                .take_due(expiry + (STALE_RETENTION - Duration::from_secs(1)))
                .is_empty()
        );
        let dropped = store.take_due(expiry + STALE_RETENTION);
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].status, None);
        assert!(dropped[0].sequence > stale[0].sequence);
        assert_eq!(store.summary(splint_id, 1, expiry + STALE_RETENTION), None);
        assert_eq!(store.next_deadline(), None);
    }

    #[test]
    fn resting_status_is_untimed_unless_a_ttl_is_reported() {
        let mut store = AgentStatusStore::default();
        let splint_id = SplintId::new();
        let start = Instant::now();
        admitted(&mut store, splint_id, start);
        let done = store.apply(
            splint_id,
            1,
            Some(&report(AgentState::Done, None, None)),
            start,
        );
        assert_eq!(done.ttl, None);
        store.take_due(start);
        assert_eq!(store.next_deadline(), None);
        assert!(
            !store
                .summary(splint_id, 1, start + Duration::from_secs(86_400))
                .unwrap()
                .stale
        );
        admitted(&mut store, splint_id, start);
        let timed = store.apply(
            splint_id,
            1,
            Some(&report(AgentState::Error, None, Some(30))),
            start,
        );
        assert_eq!(timed.ttl, Some(Duration::from_secs(30)));
        let events = store.take_due(start + Duration::from_secs(30));
        assert!(events[0].status.as_ref().unwrap().stale);
    }

    #[test]
    fn identical_report_refreshes_ttl_without_publishing() {
        let mut store = AgentStatusStore::default();
        let splint_id = SplintId::new();
        let start = Instant::now();
        let working = report(AgentState::Working, Some("tests"), Some(10));
        admitted(&mut store, splint_id, start);
        let first = store.apply(splint_id, 1, Some(&working), start);
        assert_eq!(store.take_due(start).len(), 1);

        let later = start + Duration::from_secs(9);
        admitted(&mut store, splint_id, later);
        let refreshed = store.apply(splint_id, 1, Some(&working), later);
        assert!(!refreshed.changed);
        assert_eq!(refreshed.sequence, first.sequence);
        assert_eq!(store.next_deadline(), Some(later + Duration::from_secs(10)));
        assert!(store.take_due(start + Duration::from_secs(10)).is_empty());
        assert_eq!(
            store
                .summary(splint_id, 1, later + Duration::from_secs(1))
                .unwrap()
                .reported_age_ms,
            1_000
        );

        // A refresh of a stale status is a visible change again.
        let stale_at = later + Duration::from_secs(10);
        assert!(store.take_due(stale_at)[0].status.as_ref().unwrap().stale);
        admitted(&mut store, splint_id, stale_at);
        let revived = store.apply(splint_id, 1, Some(&working), stale_at);
        assert!(revived.changed);
        assert!(revived.sequence > first.sequence);
        assert!(!store.summary(splint_id, 1, stale_at).unwrap().stale);
    }

    #[test]
    fn publication_coalesces_to_the_latest_value_every_quarter_second() {
        let mut store = AgentStatusStore::default();
        let splint_id = SplintId::new();
        let start = Instant::now();
        admitted(&mut store, splint_id, start);
        store.apply(
            splint_id,
            1,
            Some(&report(AgentState::Working, None, None)),
            start,
        );
        assert_eq!(store.take_due(start).len(), 1);
        for (offset, state) in [
            (10, AgentState::NeedsInput),
            (20, AgentState::Error),
            (30, AgentState::Done),
        ] {
            let now = start + Duration::from_millis(offset);
            admitted(&mut store, splint_id, now);
            assert!(
                store
                    .apply(splint_id, 1, Some(&report(state, None, None)), now)
                    .changed
            );
            assert!(store.take_due(now).is_empty(), "coalesced within 250 ms");
        }
        assert_eq!(
            store.next_deadline(),
            Some(start + PUBLISH_INTERVAL),
            "one deadline carries the pending publication"
        );
        let events = store.take_due(start + PUBLISH_INTERVAL);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].status.as_ref().unwrap().state, AgentState::Done);
        assert_eq!(store.next_deadline(), None);
    }

    #[test]
    fn clear_publishes_once_and_a_clear_without_status_is_silent() {
        let mut store = AgentStatusStore::default();
        let splint_id = SplintId::new();
        let start = Instant::now();
        admitted(&mut store, splint_id, start);
        let silent = store.apply(splint_id, 1, None, start);
        assert!(!silent.changed);
        assert_eq!(silent.sequence, 0);
        assert!(store.take_due(start).is_empty());
        admitted(&mut store, splint_id, start);
        store.apply(
            splint_id,
            1,
            Some(&report(AgentState::Idle, None, None)),
            start,
        );
        store.take_due(start);
        let later = start + Duration::from_secs(1);
        admitted(&mut store, splint_id, later);
        let cleared = store.apply(splint_id, 1, None, later);
        assert!(cleared.changed);
        let events = store.take_due(later);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].status, None);
        assert_eq!(events[0].sequence, cleared.sequence);
    }

    #[test]
    fn admission_is_limited_to_ten_reports_per_second_per_splint() {
        let mut store = AgentStatusStore::default();
        let splint_id = SplintId::new();
        let other = SplintId::new();
        let start = Instant::now();
        for index in 0..10 {
            admitted(
                &mut store,
                splint_id,
                start + Duration::from_millis(index * 10),
            );
        }
        let limited = start + Duration::from_millis(500);
        assert_eq!(
            store.admit(splint_id, 1, limited).unwrap(),
            Admission::RateLimited
        );
        assert!(store.claim_rate_refusal_audit(splint_id, 1));
        assert!(!store.claim_rate_refusal_audit(splint_id, 1));
        admitted(&mut store, other, limited);
        assert_eq!(
            store.admit(splint_id, 1, start + ADMISSION_WINDOW).unwrap(),
            Admission::Admitted,
            "the window slides past the first admission"
        );
        // A new incarnation starts with a fresh window and audit claim.
        assert_eq!(
            store.admit(splint_id, 2, limited).unwrap(),
            Admission::Admitted
        );
        assert!(store.claim_rate_refusal_audit(splint_id, 2));
    }

    #[test]
    fn retired_incarnations_are_dropped_and_never_summarized() {
        let mut store = AgentStatusStore::default();
        let splint_id = SplintId::new();
        let closed = SplintId::new();
        let start = Instant::now();
        for target in [splint_id, closed] {
            admitted(&mut store, target, start);
            store.apply(
                target,
                1,
                Some(&report(AgentState::Working, None, None)),
                start,
            );
        }
        assert_eq!(store.summary(splint_id, 2, start), None);
        store.retain_live(&HashMap::from([(splint_id, 1)]));
        assert_eq!(store.len(), 1);
        assert!(store.summary(splint_id, 1, start).is_some());
        // Relaunch: the same Splint is now live at incarnation 2.
        store.retain_live(&HashMap::from([(splint_id, 2)]));
        assert_eq!(store.len(), 0);
        assert_eq!(store.next_deadline(), None);
        assert!(store.take_due(start + Duration::from_secs(600)).is_empty());
    }

    #[test]
    fn a_new_report_replaces_the_previous_expiry() {
        let mut store = AgentStatusStore::default();
        let splint_id = SplintId::new();
        let start = Instant::now();
        admitted(&mut store, splint_id, start);
        store.apply(
            splint_id,
            1,
            Some(&report(AgentState::Working, None, Some(10))),
            start,
        );
        store.take_due(start);
        // A change just before expiry publishes, then expiry is spaced.
        let before = start + Duration::from_millis(9_900);
        admitted(&mut store, splint_id, before);
        store.apply(
            splint_id,
            1,
            Some(&report(AgentState::NeedsInput, None, Some(10))),
            before,
        );
        assert_eq!(store.take_due(before).len(), 1);
        assert!(
            store.take_due(start + Duration::from_secs(10)).is_empty(),
            "the replaced report's expiry no longer fires"
        );
        let expiry = before + Duration::from_secs(10);
        let stale = store.take_due(expiry);
        assert_eq!(stale.len(), 1);
        assert_eq!(
            stale[0].status.as_ref().unwrap().state,
            AgentState::NeedsInput
        );
    }

    #[test]
    fn proc_stat_parsing_tolerates_hostile_command_names() {
        assert_eq!(
            parse_proc_stat("4242 (a) b (c) R 17 4242 99 34816 4242 0"),
            Some((17, 99))
        );
        assert_eq!(parse_proc_stat("1 (init) S 0 1 1 0"), Some((0, 1)));
        assert_eq!(parse_proc_stat("garbage"), None);
        assert_eq!(parse_proc_stat("1 (x) S notanumber 1 1"), None);
    }

    #[test]
    fn cgroup_scope_match_requires_an_exact_path_component() {
        let scope = "splinterm-splintabc-i3.scope";
        assert!(cgroup_contains_scope(
            "0::/user.slice/app-splinterm-dojox.slice/splinterm-splintabc-i3.scope\n",
            scope
        ));
        assert!(!cgroup_contains_scope(
            "0::/user.slice/splinterm-splintabc-i30.scope\n",
            scope
        ));
        assert!(!cgroup_contains_scope(
            "1:name=systemd:/splinterm-splintabc-i3.scope\n",
            scope
        ));
    }

    #[test]
    fn peer_location_uses_session_then_bounded_ancestry() {
        let own = std::process::id();
        let session = rustix::process::getsid(None)
            .unwrap()
            .as_raw_nonzero()
            .get();
        let session = u32::try_from(session).unwrap();
        if session > 1 {
            assert_eq!(
                locate_peer(own, session, None),
                Some(SelfReportPath::Session)
            );
        }
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .expect("spawn sleep");
        let child_pid = child.id();
        // Treat the test process as a Splint leader: its child is inside
        // through the shared session when it leads one, else by ancestry.
        let expected = if session == own {
            SelfReportPath::Session
        } else {
            SelfReportPath::Ancestry
        };
        assert_eq!(locate_peer(child_pid, own, None), Some(expected));
        // A process is never inside a Splint led by its own descendant.
        assert_eq!(locate_peer(own, child_pid, None), None);
        assert_eq!(locate_peer(own, 1, None), None);
        let _ = child.kill();
        let _ = child.wait();
    }
}
