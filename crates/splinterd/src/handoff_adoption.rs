//! Candidate-side runtime adoption for one daemon handoff (Plan 0061 M3.8).
//!
//! [`CandidateRuntimeStage::adopt`] runs every fallible adoption step after
//! staging and before the first post-checkpoint PTY read (Plan 0037
//! quiescence item 7):
//!
//! 1. the install prefix, taken only from the authenticated sealed manifest,
//!    re-resolved, and re-bound to the forward pair's recorded sources (a
//!    rollback reports a changed pair instead of refusing it);
//! 2. the durable Topology at the manifest's revision, with every carried
//!    Splint present in its Lair and Dojo at its carried incarnation;
//! 3. both inherited listeners re-checked as listening Unix stream sockets
//!    and registered with the serving runtime;
//! 4. every carried incarnation adopted before this generation allocates one;
//! 5. a fresh persistent-policy generation against the adopted Topology;
//! 6. every carried workload scope adopted on the blocking pool, and any
//!    provably empty Dojo slice left by Splints that all exited reclaimed;
//! 7. every live actor staged with its read gate closed.
//!
//! [`StagedAdoption::commit`] then starts the actors and cannot fail. It is
//! the irreversible adoption commit of Plan 0037 quiescence item 8: the first
//! PTY read happens in an actor it started.
//!
//! Until commit, every failure returns every owned part in an
//! [`AdoptionFailure`]: the inherited descriptors, each restored terminal,
//! and each adopted scope retained again. Nothing is killed and no scope is
//! stopped, so the rollback executor (M3.11) can exec the rollback image with
//! the same descriptors.
//!
//! # Exited children
//!
//! A child that exits between capture and adoption is carried, not refused
//! (Plan 0061 M3.7b review): staging accepts it unreaped
//! ([`splinterm_pty::LinuxPtyIdentity::verify_inherited_allowing_exit`]),
//! [`crate::LiveSplintRuntime::adopt_staged`] accepts it without reaping, and
//! its committed actor drains its last output, reaps it, and publishes its
//! exit like any other. Its workload scope is adopted when systemd still
//! holds it; otherwise it runs uncontained, which is harmless for a process
//! that is gone.
//!
//! # Workload scopes
//!
//! With a workload unit manager, scopes are adopted before commit (Plan 0061
//! D7), whether or not cgroups are required: a failed adoption of a process
//! that is still running fails the whole adoption, because continuing with it
//! uncounted would let its Dojo slice stop under it once its adopted siblings
//! release (the M3.8a sibling-kill rule). The one exception: with cgroups
//! optional, a running child whose scope and Dojo slice are both absent ran
//! uncontained in the old generation, has no slice that could stop under
//! it, and runs uncontained here too. Without a manager (development, or
//! a manager that could not connect while cgroups are optional), Splints run
//! uncontained. Required cgroups without a manager fail closed.
//!
//! A Dojo whose carried Splints all exited has no adopted scope, so this
//! generation does not count its slice, and every later launch there would
//! fail with `UnitExists`. Adoption stops such a slice only when
//! `GetUnitProcesses` proves it holds no process
//! ([`WorkloadUnitManager::reclaim_empty_dojo_slice`]); otherwise it leaves
//! it running and reports it. A failed reclaim never fails the adoption.

use std::{
    collections::{BTreeSet, HashSet},
    fs, io,
    os::{
        fd::{AsFd, AsRawFd, OwnedFd},
        unix::{fs::MetadataExt, net::UnixListener},
    },
    path::{Path, PathBuf},
};

use rustix::{
    fs::fstat,
    process::{Pid, WaitId, WaitIdOptions, waitid},
};
use splinterm_core::{DojoId, LairId, LayoutNode, SplintId, SplintState, Topology};
use splinterm_policy::policy::{self, PolicyGeneration, PolicyStore};
use splinterm_pty::AdoptableLinuxPtySession;
use splinterm_terminal::Terminal;
use thiserror::Error;
use tokio::runtime::Handle;
use tracing::{info, warn};

use crate::{
    LiveError, LiveSplintConfig, LiveSplintRuntime, ProcessExit, ProcessIncarnation,
    ProcessPlacement, StagedLiveSplint,
    candidate_bootstrap::{
        CandidateRuntimeStage, CandidateSplintStage, CandidateStagedAuthorities,
        require_unix_stream_listener,
    },
    cgroup::{
        ActiveWorkloadScope, DojoSliceReclaim, RetainedWorkloadScope, WorkloadUnitError,
        WorkloadUnitManager,
    },
    handoff_coordinator::{HandoffLifecycle, HandoffPhase},
    handoff_descriptors::HandoffDescriptorSlot,
    handoff_manifest::{
        ExitDisposition, HandoffManifestV1, ManifestExcludedSplint, ManifestKind, ManifestSplint,
    },
    handoff_prepare::ListenerDescriptors,
    install_prefix::{CLIENT_EXECUTABLE, DAEMON_EXECUTABLE, InstallPrefix, InstallPrefixError},
};

/// The workload scope authority adoption needs, so tests can stand in for
/// systemd. [`WorkloadUnitManager`] is the daemon's implementation.
pub trait WorkloadScopeAdopter: Clone + Send + Sync + 'static {
    /// The counted scope a successful adoption yields.
    type Scope: ProcessPlacement;

    /// Takes ownership of a retained scope after checking systemd still
    /// holds its process there. Blocking: adoption calls it on the blocking
    /// pool.
    ///
    /// # Errors
    /// See [`WorkloadUnitManager::adopt_scope`].
    fn adopt_scope(
        &self,
        retained: &RetainedWorkloadScope,
    ) -> Result<Self::Scope, WorkloadUnitError>;

    /// Stops a leftover Dojo slice only when it provably holds no process.
    /// Blocking.
    ///
    /// # Errors
    /// See [`WorkloadUnitManager::reclaim_empty_dojo_slice`].
    fn reclaim_empty_dojo_slice(
        &self,
        dojo_id: DojoId,
    ) -> Result<DojoSliceReclaim, WorkloadUnitError>;

    /// Whether systemd has neither the retained scope nor its Dojo slice
    /// loaded. Blocking.
    ///
    /// # Errors
    /// See [`WorkloadUnitManager::retained_units_absent`].
    fn retained_units_absent(
        &self,
        retained: &RetainedWorkloadScope,
    ) -> Result<bool, WorkloadUnitError>;
}

impl WorkloadScopeAdopter for WorkloadUnitManager {
    type Scope = ActiveWorkloadScope;

    fn adopt_scope(
        &self,
        retained: &RetainedWorkloadScope,
    ) -> Result<ActiveWorkloadScope, WorkloadUnitError> {
        Self::adopt_scope(self, retained)
    }

    fn reclaim_empty_dojo_slice(
        &self,
        dojo_id: DojoId,
    ) -> Result<DojoSliceReclaim, WorkloadUnitError> {
        Self::reclaim_empty_dojo_slice(self, dojo_id)
    }

    fn retained_units_absent(
        &self,
        retained: &RetainedWorkloadScope,
    ) -> Result<bool, WorkloadUnitError> {
        Self::retained_units_absent(self, retained)
    }
}

/// Where an adopted Splint's process runs.
#[derive(Debug)]
pub enum AdoptedPlacement<S> {
    /// Its carried workload scope, now counted by this generation.
    Scope(S),
    /// No workload scope: no manager, or a child that exited before systemd
    /// could confirm its scope.
    Uncontained,
}

impl<S: ProcessPlacement> ProcessPlacement for AdoptedPlacement<S> {
    fn release(self) {
        if let Self::Scope(scope) = self {
            scope.release();
        }
    }

    fn retain_for_handoff(self) {
        if let Self::Scope(scope) = self {
            scope.retain_for_handoff();
        }
    }
}

/// What the binary supplies besides the staged authority.
#[derive(Debug)]
pub struct AdoptionInputs<A> {
    /// The durable Topology the binary loaded from its metadata store.
    pub topology: Topology,
    /// The one workload unit manager of this process, if it connected.
    pub workload_units: Option<A>,
    /// Whether the daemon runs with `--require-workload-cgroups`.
    pub workload_cgroups_required: bool,
    /// The persistent policy file, as [`policy::configured_path`] names it.
    pub policy_path: Option<PathBuf>,
    /// The runtime configuration every adopted actor starts from: grace
    /// periods, budgets, and the incarnation environment. Adoption sets each
    /// Splint's geometry from its manifest record and its scrollback from
    /// its Topology launch metadata.
    pub live_config: LiveSplintConfig,
}

/// Why runtime adoption refused the staged authority.
#[derive(Debug, Error)]
pub enum AdoptionError {
    #[error("candidate adoption lifecycle is not staged")]
    UnexpectedPhase,
    #[error("the manifest's install prefix is invalid: {0}")]
    InstallPrefix(#[source] InstallPrefixError),
    #[error("the install prefix does not hold the forward pair the manifest recorded")]
    InstallPrefixMismatch,
    #[error(
        "the durable Topology is at revision {durable}, but the manifest recorded revision {manifest}"
    )]
    TopologyRevision { manifest: u64, durable: u64 },
    #[error("carried Splint {0} is missing from the durable Topology")]
    MissingSplint(SplintId),
    #[error("carried Splint {0} is in another Lair or Dojo in the durable Topology")]
    SplintPlacement(SplintId),
    #[error("carried Splint {0}'s incarnation does not match the durable Topology")]
    IncarnationMismatch(SplintId),
    #[error("carried Splint {0} is not recorded as running")]
    NotRunning(SplintId),
    #[error("incarnation {0} cannot be adopted: this generation already allocated one")]
    IncarnationAllocationStarted(u64),
    #[error("inherited {0:?} is not a listening Unix stream socket")]
    InvalidListener(HandoffDescriptorSlot),
    #[error("inherited {slot:?} cannot be registered with the serving runtime: {error}")]
    ListenerRegistration {
        slot: HandoffDescriptorSlot,
        #[source]
        error: io::Error,
    },
    #[error("workload cgroups are required, but no workload unit manager is available")]
    WorkloadCgroupsUnavailable,
    #[error(
        "carried Splint {splint_id}'s running process could not be adopted into its workload scope: {error}"
    )]
    WorkloadScope {
        splint_id: SplintId,
        #[source]
        error: Box<WorkloadUnitError>,
    },
    #[error("carried Splint {splint_id}'s live runtime could not be staged: {error}")]
    LiveRuntime {
        splint_id: SplintId,
        #[source]
        error: Box<LiveError>,
    },
    #[error("an adoption task failed: {0}")]
    TaskFailed(#[source] tokio::task::JoinError),
    #[error("the staged adoption was abandoned before commit")]
    Aborted,
    /// The adopting daemon could not build an input adoption needs (its
    /// signal streams, metadata store, or durable Topology), so adoption
    /// never ran.
    #[error("the adopting daemon could not start: {0}")]
    DaemonStartup(String),
}

/// A refused adoption with every owned part, for cooperative rollback.
#[must_use = "a refused adoption owns every carried descriptor; recover it for rollback"]
pub struct AdoptionFailure {
    error: AdoptionError,
    recovery: Box<AdoptionRecovery>,
}

impl std::fmt::Debug for AdoptionFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdoptionFailure")
            .field("error", &self.error)
            .field("splints", &self.recovery.authorities.splints.len())
            .field("retained_scopes", &self.recovery.retained_scopes.len())
            .finish_non_exhaustive()
    }
}

impl std::fmt::Display for AdoptionFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "candidate runtime adoption failed: {}",
            self.error
        )
    }
}

impl std::error::Error for AdoptionFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

impl AdoptionFailure {
    #[must_use]
    pub const fn error(&self) -> &AdoptionError {
        &self.error
    }

    /// Returns every owned part for the rollback executor.
    #[must_use]
    pub fn into_recovery(self) -> AdoptionRecovery {
        *self.recovery
    }
}

/// Everything a refused adoption owned, unchanged where it matters: the same
/// inherited descriptors at the same numbers, each restored terminal, each
/// child unreaped, and every scope this attempt adopted retained again so
/// that dropping this value stops nothing.
///
/// The lifecycle is rolling back. Dropping this value closes every
/// descriptor, which hangs up the carried terminals.
pub struct AdoptionRecovery {
    lifecycle: HandoffLifecycle,
    authorities: CandidateStagedAuthorities,
    retained_scopes: Vec<RetainedWorkloadScope>,
}

impl std::fmt::Debug for AdoptionRecovery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdoptionRecovery")
            .field("phase", &self.lifecycle.phase())
            .field("splints", &self.authorities.splints.len())
            .field("retained_scopes", &self.retained_scopes.len())
            .finish_non_exhaustive()
    }
}

impl AdoptionRecovery {
    /// Number of carried Splints whose descriptors this recovery holds.
    #[must_use]
    pub fn splint_count(&self) -> usize {
        self.authorities.splints.len()
    }

    /// The scopes this attempt adopted and then retained again; their Dojo
    /// slices stay referenced in the manager that adopted them.
    #[must_use]
    pub fn retained_scopes(&self) -> &[RetainedWorkloadScope] {
        &self.retained_scopes
    }
}

/// One adopted live Splint and the Topology place it occupies.
#[derive(Debug)]
pub struct AdoptedSplint {
    pub lair_id: LairId,
    pub dojo_id: DojoId,
    pub splint_id: SplintId,
    pub incarnation: ProcessIncarnation,
    pub runtime: LiveSplintRuntime,
}

/// The committed adoption: everything the binary's daemon state needs.
///
/// Rollback is closed. The rollback snapshot pair and both manifest memfds
/// closed at commit. The forward client snapshot is handed out for the
/// trusted relaunch M3.13 and M3.14 build; until then the daemon closes it,
/// and no peer executing it is trusted UI (AGENTS.md §4.4).
#[derive(Debug)]
pub struct CommittedAdoption {
    pub handoff_id: [u8; 16],
    /// Whether this generation is the rollback image adopting a refused
    /// forward attempt's state.
    pub rollback: bool,
    pub source_daemon_generation: u64,
    /// The generation this daemon serves in `Hello`, from the authenticated
    /// manifest.
    pub daemon_generation: u64,
    pub install_prefix: InstallPrefix,
    /// The installed daemon file this image was snapshotted from, as the
    /// authenticated manifest records it: the adopted daemon's audit
    /// identity, since the running image is a memfd with no installed path.
    pub daemon_image: AdoptedDaemonImage,
    /// A rollback adoption found `<prefix>/splinterd` or `<prefix>/splinterm`
    /// no longer the forward pair recorded at preparation, and adopted
    /// anyway for continuity. Always `false` for a forward adoption, which
    /// refuses that case.
    pub install_pair_mismatch: bool,
    /// The durable Topology with every carried Splint running and every
    /// other Splint exited.
    pub topology: Topology,
    pub policy: PolicyStore,
    pub policy_generation: PolicyGeneration,
    /// Committed actors, in manifest order.
    pub splints: Vec<AdoptedSplint>,
    /// Both listeners, registered with the serving runtime before commit.
    /// Each owns a duplicate of its inherited descriptor; commit closed the
    /// inherited numbers themselves.
    pub listener: tokio::net::UnixListener,
    pub image_listener: tokio::net::UnixListener,
    /// The registered listeners' numbers, which the daemon holds for its
    /// life and a later handoff manifest names.
    pub handoff_listeners: ListenerDescriptors,
    /// The sealed forward client snapshot (see the type documentation).
    pub forward_client: OwnedFd,
    /// Carried Splints whose child had already exited when adoption staged
    /// them; each committed actor reaps it and publishes its exit.
    pub exited_before_adoption: Vec<SplintId>,
    /// Leftover Dojo slices reclaimed because they held no process.
    pub reclaimed_slices: Vec<DojoId>,
    /// Leftover Dojo slices left running because they still held a process,
    /// or because the reclaim request failed. Launches there fail until the
    /// slice empties.
    pub unreclaimed_slices: Vec<DojoId>,
    pub closed_unexpected: usize,
    /// The Splints the previous generation excluded from capture; see
    /// [`Self::finalize_excluded_splints`].
    excluded_splints: Vec<ManifestExcludedSplint>,
    lifecycle: HandoffLifecycle,
}

impl CommittedAdoption {
    /// Always [`HandoffPhase::Committed`]; kept for diagnostics.
    #[must_use]
    pub fn committed(&self) -> bool {
        self.lifecycle.phase() == HandoffPhase::Committed
    }
}

/// The recorded source evidence of the daemon image an adoption runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdoptedDaemonImage {
    pub device: u64,
    pub inode: u64,
    pub sha256: [u8; 32],
}

impl CommittedAdoption {
    /// Reaps the child of each Splint the previous generation excluded from
    /// capture, by the PID its manifest records, and writes that Splint's
    /// real exit into [`Self::topology`] before anything publishes it.
    /// Returns each excluded Splint with the exit recorded for it.
    ///
    /// Call it right after commit and before
    /// [`Self::reap_uncarried_children`], which would otherwise consume
    /// these statuses as unknown children's.
    #[must_use]
    pub fn finalize_excluded_splints(&mut self) -> Vec<(SplintId, Option<ProcessExit>)> {
        finalize_excluded(&mut self.topology, &self.excluded_splints)
    }
}

/// Records each excluded Splint's exit: its reaped child's status, else the
/// status the previous generation's actor observed (it reaped the child
/// first), else nothing, which leaves the `Exited(0)` adoption already wrote
/// (or a status the durable Topology recorded): the child was reaped before
/// its status could be recorded, or is not this process's child. A record
/// whose incarnation is no longer the Topology's is left alone.
fn finalize_excluded(
    topology: &mut Topology,
    excluded: &[ManifestExcludedSplint],
) -> Vec<(SplintId, Option<ProcessExit>)> {
    excluded
        .iter()
        .map(|record| {
            let current = topology
                .find_splint(record.splint_id)
                .is_some_and(|splint| splint.last_incarnation == Some(record.incarnation));
            if !current {
                return (record.splint_id, None);
            }
            let exit = crate::handoff_exec::reap_exited_child(record.child_pid).or_else(|| {
                record.exit.map(|exit| ProcessExit {
                    code: exit.code,
                    signal: exit.signal,
                })
            });
            if let Some(exit) = exit {
                topology.set_splint_state(record.splint_id, SplintState::Exited(exit_code(exit)));
            }
            (record.splint_id, exit)
        })
        .collect()
}

/// The code a Topology records for a process exit: the exit code, or 128
/// plus the terminating signal, as the daemon records every other exit.
const fn exit_code(exit: ProcessExit) -> i32 {
    match (exit.code, exit.signal) {
        (Some(code), _) => code,
        (None, Some(signal)) => 128 + signal,
        (None, None) => 1,
    }
}

impl CommittedAdoption {
    /// Reaps every exited child of this process that is not a carried
    /// Splint, and returns each reaped PID with its exit.
    ///
    /// The old generation leaves the children of Splints it excluded from
    /// capture (their process had exited) unreaped, and exec keeps them as
    /// this same process's children (Plan 0061 M3.7b and M3.9 reviews).
    /// Carried Splints are never touched: each committed actor reaps its own
    /// child and publishes its exit. Call it once, before serving, while
    /// this generation has spawned nothing. Children that are still running
    /// are left alone.
    #[must_use]
    pub fn reap_uncarried_children(&self) -> Vec<(u32, ProcessExit)> {
        let carried = self
            .splints
            .iter()
            .map(|splint| splint.runtime.handle().child_pid())
            .collect::<HashSet<_>>();
        match child_pids() {
            Ok(children) => reap_exited_among(children, &carried),
            Err(error) => {
                warn!(%error, "cannot list this process's children; exited children the previous generation left stay unreaped");
                Vec::new()
            }
        }
    }
}

/// This process's children, from every thread's procfs `children` list:
/// children of a thread that ended were reparented to a surviving one, and
/// after exec every pre-exec child belongs to the thread that called it.
fn child_pids() -> io::Result<Vec<u32>> {
    child_pids_in(Path::new("/proc/self/task"))
}

/// [`child_pids`] over a task directory.
///
/// Threads come and go while the list is read: an idle Tokio blocking-pool
/// thread (adoption used several) exits after its keep-alive, and its
/// `children` file then fails with `ENOENT` or `ESRCH`. Its children were
/// already reparented to a surviving thread, so such a thread is skipped.
/// Only a task directory that cannot be read, or any other failure, gives up
/// the pass.
fn child_pids_in(tasks: &Path) -> io::Result<Vec<u32>> {
    let mut children = BTreeSet::new();
    for task in fs::read_dir(tasks)? {
        let listed = match fs::read_to_string(task?.path().join("children")) {
            Ok(listed) => listed,
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    || error.raw_os_error() == Some(libc::ESRCH) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        children.extend(
            listed
                .split_ascii_whitespace()
                .filter_map(|pid| pid.parse::<u32>().ok()),
        );
    }
    Ok(children.into_iter().collect())
}

/// Reaps each of `children` that is not `carried` and has exited.
fn reap_exited_among(
    children: impl IntoIterator<Item = u32>,
    carried: &HashSet<u32>,
) -> Vec<(u32, ProcessExit)> {
    children
        .into_iter()
        .filter(|pid| !carried.contains(pid))
        .filter_map(|pid| crate::handoff_exec::reap_exited_child(pid).map(|exit| (pid, exit)))
        .collect()
}

/// A Splint waiting for its live runtime stage. Its placement retains on drop.
struct PendingSplint<S: ProcessPlacement> {
    evidence: ManifestSplint,
    checkpoint_descriptor: OwnedFd,
    terminal: Terminal,
    pty: AdoptableLinuxPtySession,
    incarnation: Option<ProcessIncarnation>,
    scrollback_lines: Option<usize>,
    placement: RetainOnDrop<AdoptedPlacement<S>>,
}

impl<S: ProcessPlacement> PendingSplint<S> {
    fn new(stage: CandidateSplintStage) -> Self {
        Self {
            evidence: stage.evidence,
            checkpoint_descriptor: stage.checkpoint_descriptor,
            terminal: stage.terminal,
            pty: stage.pty,
            incarnation: None,
            scrollback_lines: None,
            placement: RetainOnDrop::new(AdoptedPlacement::Uncontained),
        }
    }

    /// Returns the staged owners, retaining any adopted scope.
    fn unwind(self) -> (CandidateSplintStage, bool) {
        let scoped = self.placement.release_retained();
        (
            CandidateSplintStage {
                evidence: self.evidence,
                terminal: self.terminal,
                pty: self.pty,
                checkpoint_descriptor: self.checkpoint_descriptor,
            },
            scoped,
        )
    }
}

/// A staged live Splint. Dropping it uncommitted aborts the stage and
/// retains its scope instead of letting the placement's `Drop` stop it.
struct StagedSplint<S: ProcessPlacement>(Option<StagedParts<S>>);

struct StagedParts<S: ProcessPlacement> {
    evidence: ManifestSplint,
    checkpoint_descriptor: OwnedFd,
    incarnation: ProcessIncarnation,
    staged: StagedLiveSplint<AdoptedPlacement<S>>,
}

impl<S: ProcessPlacement> StagedSplint<S> {
    fn unwind(mut self) -> Option<(CandidateSplintStage, bool)> {
        let parts = self.0.take()?;
        let inputs = parts.staged.abort();
        let scoped = matches!(inputs.placement, AdoptedPlacement::Scope(_));
        inputs.placement.retain_for_handoff();
        Some((
            CandidateSplintStage {
                evidence: parts.evidence,
                terminal: inputs.terminal,
                pty: inputs.session,
                checkpoint_descriptor: parts.checkpoint_descriptor,
            },
            scoped,
        ))
    }
}

impl<S: ProcessPlacement> Drop for StagedSplint<S> {
    fn drop(&mut self) {
        if let Some(parts) = self.0.take() {
            parts.staged.abort().placement.retain_for_handoff();
        }
    }
}

/// A placement that retains, rather than drops, when it goes out of scope
/// before adoption hands it to an actor.
pub(crate) struct RetainOnDrop<P: ProcessPlacement>(Option<P>);

impl<P: ProcessPlacement> RetainOnDrop<P> {
    pub(crate) const fn new(placement: P) -> Self {
        Self(Some(placement))
    }

    fn get(&self) -> Option<&P> {
        self.0.as_ref()
    }

    pub(crate) fn take(&mut self) -> Option<P> {
        self.0.take()
    }
}

impl<S: ProcessPlacement> RetainOnDrop<AdoptedPlacement<S>> {
    /// Hands the placement to its actor. Only `new` builds this value and
    /// only this consumes it, so the uncontained fallback is unreachable.
    fn into_inner(mut self) -> AdoptedPlacement<S> {
        self.take().unwrap_or(AdoptedPlacement::Uncontained)
    }

    /// Retains the placement now; returns whether it held a scope.
    fn release_retained(mut self) -> bool {
        let placement = self.take();
        let scoped = matches!(placement, Some(AdoptedPlacement::Scope(_)));
        if let Some(placement) = placement {
            placement.retain_for_handoff();
        }
        scoped
    }
}

impl<P: ProcessPlacement> Drop for RetainOnDrop<P> {
    fn drop(&mut self) {
        if let Some(placement) = self.0.take() {
            placement.retain_for_handoff();
        }
    }
}

/// Every carried Splint between staging and commit. Each element protects
/// its own placement, so no early return, panic, or cancellation stops a
/// scope.
struct AdoptionGuard<S: ProcessPlacement> {
    staged: Vec<StagedSplint<S>>,
    pending: Vec<PendingSplint<S>>,
}

impl<S: ProcessPlacement> AdoptionGuard<S> {
    /// Returns every Splint's owners, in manifest order, and the identities
    /// of the scopes retained again.
    fn unwind(self) -> (Vec<CandidateSplintStage>, Vec<RetainedWorkloadScope>) {
        let mut splints = Vec::with_capacity(self.staged.len() + self.pending.len());
        let mut retained = Vec::new();
        let unwound = self
            .staged
            .into_iter()
            .filter_map(StagedSplint::unwind)
            .chain(self.pending.into_iter().map(PendingSplint::unwind));
        for (stage, scoped) in unwound {
            if scoped {
                retained.push(retained_scope(&stage.evidence));
            }
            splints.push(stage);
        }
        (splints, retained)
    }
}

/// Every staged owner except the Splints, held across adoption.
struct AdoptionShell {
    lifecycle: HandoffLifecycle,
    manifest: HandoffManifestV1,
    counterpart_manifest: HandoffManifestV1,
    listener: OwnedFd,
    image_listener: OwnedFd,
    current_manifest_descriptor: OwnedFd,
    counterpart_manifest_descriptor: OwnedFd,
    forward_client: OwnedFd,
    rollback_daemon: OwnedFd,
    rollback_client: OwnedFd,
    closed_unexpected: usize,
}

impl AdoptionShell {
    /// Splits a staged candidate into the shell and its Splints.
    fn from_stage(stage: CandidateRuntimeStage) -> (Self, Vec<CandidateSplintStage>) {
        let (lifecycle, authorities) = stage.into_parts();
        let CandidateStagedAuthorities {
            manifest,
            counterpart_manifest,
            splints,
            listener,
            image_listener,
            current_manifest_descriptor,
            counterpart_manifest_descriptor,
            forward_client,
            rollback_daemon,
            rollback_client,
            closed_unexpected,
        } = authorities;
        (
            Self {
                lifecycle,
                manifest,
                counterpart_manifest,
                listener,
                image_listener,
                current_manifest_descriptor,
                counterpart_manifest_descriptor,
                forward_client,
                rollback_daemon,
                rollback_client,
                closed_unexpected,
            },
            splints,
        )
    }

    fn fail<S: ProcessPlacement>(
        self,
        error: AdoptionError,
        guard: AdoptionGuard<S>,
    ) -> AdoptionFailure {
        let (splints, retained_scopes) = guard.unwind();
        let mut lifecycle = self.lifecycle;
        if lifecycle.phase() == HandoffPhase::AdoptionStaged {
            // Always a valid edge from the only phase that adopts.
            let _ = lifecycle.transition(HandoffPhase::RollingBack);
        }
        AdoptionFailure {
            error,
            recovery: Box::new(AdoptionRecovery {
                lifecycle,
                authorities: CandidateStagedAuthorities {
                    manifest: self.manifest,
                    counterpart_manifest: self.counterpart_manifest,
                    splints,
                    listener: self.listener,
                    image_listener: self.image_listener,
                    current_manifest_descriptor: self.current_manifest_descriptor,
                    counterpart_manifest_descriptor: self.counterpart_manifest_descriptor,
                    forward_client: self.forward_client,
                    rollback_daemon: self.rollback_daemon,
                    rollback_client: self.rollback_client,
                    closed_unexpected: self.closed_unexpected,
                },
                retained_scopes,
            }),
        }
    }
}

/// What the fallible steps produced besides the staged Splints.
struct Prepared {
    install_prefix: InstallPrefix,
    install_pair_mismatch: bool,
    listener: tokio::net::UnixListener,
    image_listener: tokio::net::UnixListener,
    topology: Topology,
    policy: PolicyStore,
    policy_generation: PolicyGeneration,
    reclaimed_slices: Vec<DojoId>,
    unreclaimed_slices: Vec<DojoId>,
}

/// Every adoption step completed; only [`Self::commit`] remains.
///
/// [`Self::abort`] returns every owned part instead. Dropping it uncommitted
/// retains every adopted scope, so nothing is stopped, but closes the carried
/// descriptors, which hangs up their terminals.
#[must_use = "a staged adoption must be committed or aborted"]
pub struct StagedAdoption<S: ProcessPlacement> {
    runtime: Handle,
    shell: AdoptionShell,
    splints: Vec<StagedSplint<S>>,
    prepared: Prepared,
    exited_before_adoption: Vec<SplintId>,
}

impl<S: ProcessPlacement> std::fmt::Debug for StagedAdoption<S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StagedAdoption")
            .field("splints", &self.splints.len())
            .field(
                "daemon_generation",
                &self.shell.manifest.target_daemon_generation,
            )
            .finish_non_exhaustive()
    }
}

impl CandidateRuntimeStage {
    /// Refuses adoption before it starts, with every owned part, exactly as
    /// a refusal inside [`Self::adopt`] returns them: the daemon could not
    /// build an input adoption needs. The lifecycle moves to rolling back.
    pub fn refuse(self, error: AdoptionError) -> AdoptionFailure {
        let (shell, splints) = AdoptionShell::from_stage(self);
        // Nothing was adopted, so every placement is uncontained; the scope
        // type is only nominal.
        let guard = AdoptionGuard::<ActiveWorkloadScope> {
            staged: Vec::new(),
            pending: splints.into_iter().map(PendingSplint::new).collect(),
        };
        shell.fail(error, guard)
    }
}

impl CandidateRuntimeStage {
    /// Runs every fallible adoption step without reading any PTY, accepting
    /// a connection, or stopping anything; see the module documentation.
    ///
    /// Call it on the runtime that will serve: actors are staged with its
    /// I/O driver, and [`StagedAdoption::commit`] starts them on it.
    ///
    /// # Errors
    /// Returns the typed refusal with every owned part (see
    /// [`AdoptionRecovery`]) when any check, scope adoption, or actor stage
    /// fails.
    pub async fn adopt<A: WorkloadScopeAdopter>(
        self,
        inputs: AdoptionInputs<A>,
    ) -> Result<StagedAdoption<A::Scope>, AdoptionFailure> {
        let (shell, splints) = AdoptionShell::from_stage(self);
        let mut guard = AdoptionGuard {
            staged: Vec::with_capacity(splints.len()),
            pending: splints.into_iter().map(PendingSplint::new).collect(),
        };
        match prepare(&shell, &mut guard, inputs).await {
            Ok(prepared) => {
                let exited_before_adoption = guard
                    .staged
                    .iter()
                    .filter_map(|staged| staged.0.as_ref())
                    .filter(|parts| child_has_exited(parts.evidence.pty.child_pid))
                    .map(|parts| parts.evidence.splint_id)
                    .collect();
                Ok(StagedAdoption {
                    runtime: Handle::current(),
                    shell,
                    splints: std::mem::take(&mut guard.staged),
                    prepared,
                    exited_before_adoption,
                })
            }
            Err(error) => Err(shell.fail(error, guard)),
        }
    }
}

async fn prepare<A: WorkloadScopeAdopter>(
    shell: &AdoptionShell,
    guard: &mut AdoptionGuard<A::Scope>,
    inputs: AdoptionInputs<A>,
) -> Result<Prepared, AdoptionError> {
    let AdoptionInputs {
        mut topology,
        workload_units,
        workload_cgroups_required,
        policy_path,
        live_config,
    } = inputs;
    if shell.lifecycle.phase() != HandoffPhase::AdoptionStaged {
        return Err(AdoptionError::UnexpectedPhase);
    }
    let manifest = &shell.manifest;
    let (install_prefix, install_pair_mismatch) = verify_install_prefix(manifest)?;
    info!(
        prefix = %install_prefix.directory().display(),
        install_pair_mismatch,
        "handoff adoption: install prefix verified"
    );
    verify_topology(manifest, &topology)?;
    info!(
        revision = manifest.topology_revision,
        carried = manifest.splints.len(),
        "handoff adoption: durable Topology verified"
    );
    for (slot, listener) in [
        (HandoffDescriptorSlot::Listener, &shell.listener),
        (HandoffDescriptorSlot::ImageListener, &shell.image_listener),
    ] {
        verify_listener(slot, listener)?;
    }
    // Re-registration is fallible, so it happens here, before commit
    // (Plan 0037 quiescence items 7 and 9).
    let listener = register_listener(HandoffDescriptorSlot::Listener, &shell.listener)?;
    let image_listener =
        register_listener(HandoffDescriptorSlot::ImageListener, &shell.image_listener)?;
    info!("handoff adoption: inherited listeners registered");
    if workload_cgroups_required && workload_units.is_none() {
        return Err(AdoptionError::WorkloadCgroupsUnavailable);
    }
    let carried = manifest
        .splints
        .iter()
        .map(|splint| splint.splint_id)
        .collect::<HashSet<_>>();
    mark_adopted_states(&mut topology, &carried);
    reserve_recorded_incarnations(&topology);
    for pending in &mut guard.pending {
        let value = pending.evidence.incarnation;
        pending.incarnation = Some(
            adopt_carried_incarnation(value)
                .ok_or(AdoptionError::IncarnationAllocationStarted(value))?,
        );
        pending.scrollback_lines = topology
            .find_splint(pending.evidence.splint_id)
            .map(|splint| splint.launch.scrollback_lines);
    }
    info!(
        carried = guard.pending.len(),
        "handoff adoption: carried incarnations adopted"
    );
    let (policy, policy_generation) = reload_policy(policy_path, &topology).await?;
    info!(
        generation = policy_generation.id,
        rejected = policy_generation.diagnostic.is_some(),
        "handoff adoption: persistent policy reloaded"
    );
    let (reclaimed_slices, unreclaimed_slices) = match workload_units {
        Some(adopter) => {
            adopt_scopes(&adopter, guard, workload_cgroups_required).await?;
            reclaim_leftover_slices(&adopter, guard, &manifest.excluded_splints).await?
        }
        None => (Vec::new(), Vec::new()),
    };
    info!(
        contained = guard
            .pending
            .iter()
            .filter(|pending| matches!(pending.placement.get(), Some(AdoptedPlacement::Scope(_))))
            .count(),
        reclaimed_slices = reclaimed_slices.len(),
        unreclaimed_slices = unreclaimed_slices.len(),
        "handoff adoption: workload scopes adopted"
    );
    stage_actors(guard, &live_config)?;
    info!(
        staged = guard.staged.len(),
        "handoff adoption: live actors staged with closed read gates"
    );
    Ok(Prepared {
        install_prefix,
        install_pair_mismatch,
        listener,
        image_listener,
        topology,
        policy,
        policy_generation,
        reclaimed_slices,
        unreclaimed_slices,
    })
}

/// Takes the prefix only from the authenticated manifest (never the running
/// image, a memfd), re-resolves it, and re-binds the forward pair's recorded
/// sources to `<prefix>/splinterd` and `<prefix>/splinterm` as this process
/// sees them now (Plan 0061 M3.5b review). Both manifest kinds check the
/// forward pair: the package upgrade installed it at the prefix, and a
/// rollback restores the old process, not the old package.
///
/// A forward adoption refuses a mismatch. A rollback adoption only reports
/// it (the returned flag) and warns: the package may have changed again
/// since preparation, and refusing would end in a failed service that
/// destroys the Splints rollback exists to save. Live continuity outranks
/// launch authority from the prefix.
fn verify_install_prefix(
    manifest: &HandoffManifestV1,
) -> Result<(InstallPrefix, bool), AdoptionError> {
    let prefix = InstallPrefix::from_directory(&manifest.install_prefix)
        .map_err(AdoptionError::InstallPrefix)?;
    if forward_pair_installed(&prefix, manifest) {
        return Ok((prefix, false));
    }
    match manifest.kind {
        ManifestKind::Forward => Err(AdoptionError::InstallPrefixMismatch),
        ManifestKind::Rollback => {
            warn!(
                prefix = %prefix.directory().display(),
                "installed pair changed since handoff preparation; rollback adopts anyway"
            );
            Ok((prefix, true))
        }
    }
}

/// Whether `<prefix>/splinterd` and `<prefix>/splinterm` are exactly the
/// forward pair's recorded sources.
fn forward_pair_installed(prefix: &InstallPrefix, manifest: &HandoffManifestV1) -> bool {
    let pair = &manifest.executable_pairs.forward;
    [
        (DAEMON_EXECUTABLE, &pair.daemon),
        (CLIENT_EXECUTABLE, &pair.client),
    ]
    .into_iter()
    .all(|(name, image)| {
        fs::symlink_metadata(prefix.directory().join(name)).is_ok_and(|metadata| {
            metadata.is_file()
                && metadata.dev() == image.source_device
                && metadata.ino() == image.source_inode
                && metadata.uid() == image.source_owner_uid
                && metadata.mode() == image.source_mode
                && metadata.len() == image.source_size
        })
    })
}

/// Registers one inherited listener with the serving runtime's I/O driver.
///
/// Registration owns what it registers and closes it on failure, so it
/// registers a close-on-exec duplicate: the inherited descriptor stays at
/// its recorded number, recoverable for rollback, until commit.
fn register_listener(
    slot: HandoffDescriptorSlot,
    inherited: &OwnedFd,
) -> Result<tokio::net::UnixListener, AdoptionError> {
    let registration = |error| AdoptionError::ListenerRegistration { slot, error };
    let duplicate = inherited.try_clone().map_err(registration)?;
    tokio::net::UnixListener::from_std(UnixListener::from(duplicate)).map_err(registration)
}

/// Requires the durable Topology at the revision the old generation persisted
/// at its fence, and each carried Splint in it at its Lair, Dojo, and
/// incarnation.
fn verify_topology(manifest: &HandoffManifestV1, topology: &Topology) -> Result<(), AdoptionError> {
    let durable = topology.revision().get();
    // The fence persists exactly one revision, which the manifest records as
    // both values; the metadata store keeps no other generation.
    if manifest.topology_revision != durable || manifest.durable_state_generation != durable {
        return Err(AdoptionError::TopologyRevision {
            manifest: manifest.topology_revision,
            durable,
        });
    }
    for splint in &manifest.splints {
        if splint.exit != ExitDisposition::Running {
            return Err(AdoptionError::NotRunning(splint.splint_id));
        }
        let record = topology
            .find_splint(splint.splint_id)
            .ok_or(AdoptionError::MissingSplint(splint.splint_id))?;
        let placed = topology
            .find_lair(splint.lair_id)
            .and_then(|lair| lair.dojos.iter().find(|dojo| dojo.id == splint.dojo_id))
            .is_some_and(|dojo| dojo.root.find_splint(splint.splint_id).is_some());
        if !placed {
            return Err(AdoptionError::SplintPlacement(splint.splint_id));
        }
        if record.last_incarnation != Some(splint.incarnation) {
            return Err(AdoptionError::IncarnationMismatch(splint.splint_id));
        }
    }
    Ok(())
}

fn verify_listener(slot: HandoffDescriptorSlot, listener: &OwnedFd) -> Result<(), AdoptionError> {
    let stat = fstat(listener).map_err(|_| AdoptionError::InvalidListener(slot))?;
    require_unix_stream_listener(listener.as_fd(), &stat, slot)
        .map_err(|_| AdoptionError::InvalidListener(slot))
}

/// Marks every carried Splint running. Any other Splint the Topology records
/// as running or starting has no live process in this generation, so it is
/// marked exited (Plan 0061 M3.7b review). The durable projection already
/// stores every Splint as exited; this keeps the rule independent of that.
fn mark_adopted_states(topology: &mut Topology, carried: &HashSet<SplintId>) {
    let mut recorded = Vec::new();
    for lair in topology.lairs() {
        for dojo in &lair.dojos {
            collect_splint_states(&dojo.root, &mut recorded);
        }
    }
    for (splint_id, state) in recorded {
        if carried.contains(&splint_id) {
            topology.set_splint_state(splint_id, SplintState::Running);
        } else if !matches!(state, SplintState::Exited(_)) {
            topology.set_splint_state(splint_id, SplintState::Exited(0));
        }
    }
}

fn collect_splint_states(node: &LayoutNode, states: &mut Vec<(SplintId, SplintState)>) {
    match node {
        LayoutNode::Leaf(splint) => states.push((splint.id, splint.state)),
        LayoutNode::Branch { first, second, .. } => {
            collect_splint_states(first, states);
            collect_splint_states(second, states);
        }
    }
}

/// Reserves every incarnation the Topology records, so this generation never
/// reissues one, as a fresh start does.
fn reserve_recorded_incarnations(topology: &Topology) {
    let mut recorded = Vec::new();
    for lair in topology.lairs() {
        for dojo in &lair.dojos {
            collect_splint_states(&dojo.root, &mut recorded);
        }
    }
    for (splint_id, _) in recorded {
        if let Some(incarnation) = topology
            .find_splint(splint_id)
            .and_then(|splint| splint.last_incarnation)
        {
            ProcessIncarnation::reserve_after(incarnation);
        }
    }
}

/// Adopts one carried incarnation, which must happen before this
/// generation's first allocation (Plan 0061 M3.3 review).
fn adopt_carried_incarnation(value: u64) -> Option<ProcessIncarnation> {
    #[cfg(not(test))]
    {
        ProcessIncarnation::adopted(value)
    }
    // The unit-test binary allocates incarnations long before any adoption.
    #[cfg(test)]
    {
        ProcessIncarnation::carried_for_test(value)
    }
}

async fn reload_policy(
    path: Option<PathBuf>,
    topology: &Topology,
) -> Result<(PolicyStore, PolicyGeneration), AdoptionError> {
    let candidate = tokio::task::spawn_blocking(move || policy::prepare(path))
        .await
        .map_err(AdoptionError::TaskFailed)?;
    let mut store = PolicyStore::default();
    let generation = store.publish(candidate, topology);
    if let Some(diagnostic) = &generation.diagnostic {
        warn!(generation = generation.id, %diagnostic, "persistent policy rejected at handoff adoption; installed deny-all generation");
    }
    Ok((store, generation))
}

/// How one carried scope's adoption ended on the blocking pool.
enum ScopeOutcome<S: ProcessPlacement> {
    Adopted(RetainOnDrop<S>),
    /// No scope to adopt and none to stop under the child: it has exited,
    /// or, with cgroups optional, systemd has neither its scope nor its
    /// Dojo slice because the old generation ran it uncontained.
    Uncontained,
}

/// Adopts every carried scope, in manifest order, on the blocking pool:
/// [`WorkloadUnitManager::adopt_scope`] holds a lock across D-Bus calls.
///
/// A scope that fails for a child that has exited is tolerated, and that
/// Splint runs uncontained. So does a running child when cgroups are
/// optional and systemd has neither its scope nor its Dojo slice loaded: the
/// old generation ran it uncontained (its manager never connected), and no
/// slice exists that could stop under it. Any other failure for a running
/// child fails the adoption (the sibling-kill rule): with its slice loaded,
/// continuing uncounted would let that slice stop under it. Every adopted
/// scope is wrapped so that a panic, an early return, or a dropped adoption
/// future retains it rather than stopping it.
async fn adopt_scopes<A: WorkloadScopeAdopter>(
    adopter: &A,
    guard: &mut AdoptionGuard<A::Scope>,
    cgroups_required: bool,
) -> Result<(), AdoptionError> {
    let identities = guard
        .pending
        .iter()
        .map(|pending| retained_scope(&pending.evidence))
        .collect::<Vec<_>>();
    let adopter = adopter.clone();
    let (outcomes, failure) = tokio::task::spawn_blocking(move || {
        let mut outcomes = Vec::with_capacity(identities.len());
        for retained in &identities {
            match adopter.adopt_scope(retained) {
                Ok(scope) => outcomes.push(ScopeOutcome::Adopted(RetainOnDrop::new(scope))),
                // Checked after the failure: a child cannot come back, so an
                // exit seen now explains the failure.
                Err(_) if child_has_exited(retained.child_pid()) => {
                    outcomes.push(ScopeOutcome::Uncontained);
                }
                Err(_)
                    if !cgroups_required
                        && adopter
                            .retained_units_absent(retained)
                            .is_ok_and(|absent| absent) =>
                {
                    outcomes.push(ScopeOutcome::Uncontained);
                }
                Err(error) => return (outcomes, Some(error)),
            }
        }
        (outcomes, None)
    })
    .await
    .map_err(AdoptionError::TaskFailed)?;
    let failed_index = outcomes.len();
    for (pending, outcome) in guard.pending.iter_mut().zip(outcomes) {
        if let ScopeOutcome::Adopted(mut scope) = outcome
            && let Some(scope) = scope.take()
        {
            pending.placement = RetainOnDrop::new(AdoptedPlacement::Scope(scope));
        }
    }
    match failure {
        Some(error) => Err(AdoptionError::WorkloadScope {
            splint_id: guard.pending[failed_index].evidence.splint_id,
            error: Box::new(error),
        }),
        None => Ok(()),
    }
}

/// Reclaims the slice of each Dojo the handoff names (a carried Splint's,
/// or an excluded Splint's) when no adopted scope counts it, only when
/// systemd proves it empty. A Dojo whose Splints all exited before capture
/// still has its slice loaded, uncounted by this generation, so a later
/// launch there would otherwise fail with `UnitExists`. A failed request is
/// reported in the committed adoption, never as a refusal.
async fn reclaim_leftover_slices<A: WorkloadScopeAdopter>(
    adopter: &A,
    guard: &AdoptionGuard<A::Scope>,
    excluded: &[ManifestExcludedSplint],
) -> Result<(Vec<DojoId>, Vec<DojoId>), AdoptionError> {
    let carried = guard
        .pending
        .iter()
        .map(|pending| pending.evidence.dojo_id)
        .chain(excluded.iter().map(|splint| splint.dojo_id))
        .collect::<BTreeSet<_>>();
    let counted = guard
        .pending
        .iter()
        .filter(|pending| matches!(pending.placement.get(), Some(AdoptedPlacement::Scope(_))))
        .map(|pending| pending.evidence.dojo_id)
        .collect::<BTreeSet<_>>();
    let leftovers = carried.difference(&counted).copied().collect::<Vec<_>>();
    if leftovers.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let adopter = adopter.clone();
    tokio::task::spawn_blocking(move || {
        let mut reclaimed = Vec::new();
        let mut unreclaimed = Vec::new();
        for dojo_id in leftovers {
            match adopter.reclaim_empty_dojo_slice(dojo_id) {
                Ok(DojoSliceReclaim::Stopped) => reclaimed.push(dojo_id),
                Ok(DojoSliceReclaim::Absent | DojoSliceReclaim::Referenced) => {}
                Ok(DojoSliceReclaim::Occupied) => {
                    warn!(%dojo_id, "leftover Dojo slice still holds processes; launches there fail until it empties");
                    unreclaimed.push(dojo_id);
                }
                Err(error) => {
                    warn!(%dojo_id, %error, "cannot reclaim leftover Dojo slice");
                    unreclaimed.push(dojo_id);
                }
            }
        }
        (reclaimed, unreclaimed)
    })
    .await
    .map_err(AdoptionError::TaskFailed)
}

/// Stages each actor with its read gate closed, in manifest order. A failed
/// stage puts that Splint's untouched inputs back, and the caller unwinds
/// every staged one.
fn stage_actors<S: ProcessPlacement>(
    guard: &mut AdoptionGuard<S>,
    base: &LiveSplintConfig,
) -> Result<(), AdoptionError> {
    let mut pending = std::mem::take(&mut guard.pending).into_iter();
    while let Some(splint) = pending.next() {
        let PendingSplint {
            evidence,
            checkpoint_descriptor,
            terminal,
            pty,
            incarnation,
            scrollback_lines,
            placement,
        } = splint;
        let Some(incarnation) = incarnation else {
            guard.pending.push(PendingSplint {
                evidence,
                checkpoint_descriptor,
                terminal,
                pty,
                incarnation,
                scrollback_lines,
                placement,
            });
            guard.pending.extend(pending);
            return Err(AdoptionError::UnexpectedPhase);
        };
        let mut config = base.clone();
        config.columns = evidence.size.columns;
        config.rows = evidence.size.rows;
        config.pixel_width = evidence.size.pixel_width;
        config.pixel_height = evidence.size.pixel_height;
        if let Some(lines) = scrollback_lines {
            config.terminal.scrollback_lines = lines;
        }
        match LiveSplintRuntime::adopt_staged(
            evidence.splint_id,
            incarnation,
            pty,
            terminal,
            config,
            placement.into_inner(),
        ) {
            Ok(staged) => guard.staged.push(StagedSplint(Some(StagedParts {
                evidence,
                checkpoint_descriptor,
                incarnation,
                staged,
            }))),
            Err(failure) => {
                let splint_id = evidence.splint_id;
                let inputs = failure.inputs;
                guard.pending.push(PendingSplint {
                    evidence,
                    checkpoint_descriptor,
                    terminal: inputs.terminal,
                    pty: inputs.session,
                    incarnation: Some(incarnation),
                    scrollback_lines,
                    placement: RetainOnDrop::new(inputs.placement),
                });
                guard.pending.extend(pending);
                return Err(AdoptionError::LiveRuntime {
                    splint_id,
                    error: Box::new(failure.error),
                });
            }
        }
    }
    Ok(())
}

impl<S: ProcessPlacement> StagedAdoption<S> {
    /// The generation this daemon will serve, from the authenticated manifest.
    #[must_use]
    pub const fn daemon_generation(&self) -> u64 {
        self.shell.manifest.target_daemon_generation
    }

    /// Abandons the staged adoption before commit, for a cooperative
    /// rollback, and returns every owned part as a refused adoption does.
    #[must_use]
    pub fn abort(self) -> AdoptionRecovery {
        let guard = AdoptionGuard {
            staged: self.splints,
            pending: Vec::new(),
        };
        self.shell
            .fail(AdoptionError::Aborted, guard)
            .into_recovery()
    }

    /// Commits the adoption: starts every staged actor, which opens its read
    /// gate, and releases the rollback-only descriptors.
    ///
    /// Nothing here can fail or panic: each actor starts on the runtime that
    /// staged it, entered explicitly, and the lifecycle edge from the staged
    /// phase to committed is always valid. This is the last step before the
    /// first post-checkpoint PTY read; after it, no cooperative rollback is
    /// permitted.
    #[must_use]
    pub fn commit(self) -> CommittedAdoption {
        let Self {
            runtime,
            shell,
            splints,
            prepared,
            exited_before_adoption,
        } = self;
        let AdoptionShell {
            mut lifecycle,
            manifest,
            counterpart_manifest,
            listener,
            image_listener,
            current_manifest_descriptor,
            counterpart_manifest_descriptor,
            forward_client,
            rollback_daemon,
            rollback_client,
            closed_unexpected,
        } = shell;
        // Valid by construction: only an adoption staged from
        // `AdoptionStaged` reaches here.
        let _ = lifecycle.transition(HandoffPhase::Committed);
        let _entered = runtime.enter();
        let splints = splints
            .into_iter()
            .filter_map(|mut staged| staged.0.take())
            .map(|parts| AdoptedSplint {
                lair_id: parts.evidence.lair_id,
                dojo_id: parts.evidence.dojo_id,
                splint_id: parts.evidence.splint_id,
                incarnation: parts.incarnation,
                runtime: parts.staged.commit(),
            })
            .collect();
        drop((
            counterpart_manifest,
            current_manifest_descriptor,
            counterpart_manifest_descriptor,
            rollback_daemon,
            rollback_client,
            listener,
            image_listener,
        ));
        let handoff_listeners = ListenerDescriptors {
            listener: prepared.listener.as_raw_fd(),
            image_listener: prepared.image_listener.as_raw_fd(),
        };
        let image = match manifest.kind {
            ManifestKind::Forward => &manifest.executable_pairs.forward.daemon,
            ManifestKind::Rollback => &manifest.executable_pairs.rollback.daemon,
        };
        let daemon_image = AdoptedDaemonImage {
            device: image.source_device,
            inode: image.source_inode,
            sha256: image.source_sha256,
        };
        CommittedAdoption {
            handoff_id: manifest.handoff_id.bytes(),
            rollback: manifest.kind == ManifestKind::Rollback,
            source_daemon_generation: manifest.source_daemon_generation,
            daemon_generation: manifest.target_daemon_generation,
            daemon_image,
            install_prefix: prepared.install_prefix,
            install_pair_mismatch: prepared.install_pair_mismatch,
            topology: prepared.topology,
            policy: prepared.policy,
            policy_generation: prepared.policy_generation,
            splints,
            listener: prepared.listener,
            image_listener: prepared.image_listener,
            handoff_listeners,
            forward_client,
            exited_before_adoption,
            reclaimed_slices: prepared.reclaimed_slices,
            unreclaimed_slices: prepared.unreclaimed_slices,
            closed_unexpected,
            excluded_splints: manifest.excluded_splints,
            lifecycle,
        }
    }
}

fn retained_scope(evidence: &ManifestSplint) -> RetainedWorkloadScope {
    RetainedWorkloadScope::new(
        evidence.dojo_id,
        evidence.splint_id,
        evidence.incarnation,
        evidence.pty.child_pid,
    )
}

/// Whether `pid` is this process's child and has exited, without consuming
/// its status. Anything else, including a query failure, reads as running,
/// so a doubtful scope failure fails closed.
pub(crate) fn child_has_exited(pid: u32) -> bool {
    let Some(pid) = i32::try_from(pid).ok().and_then(Pid::from_raw) else {
        return false;
    };
    matches!(
        waitid(
            WaitId::Pid(pid),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
        ),
        Ok(Some(_))
    )
}

#[cfg(test)]
mod tests {
    use std::{
        os::unix::net::UnixStream,
        path::Path,
        sync::{
            Arc, Mutex,
            atomic::{AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };

    use splinterm_core::TopologyDocument;
    use splinterm_terminal::{SharedImageBudget, SharedKittyUploadBudget};
    use tokio::time;

    use super::*;
    use crate::{
        HandedOffSplint, LiveSplintHandle,
        candidate_bootstrap::RuntimeStageForTest,
        handoff_prepare::tests::{
            ADOPTION_BUDGET_BYTES, CAT, OldSideHandoff, config, hand_off_fleet, snapshot_text,
        },
    };

    /// Records what adoption did with each scope. A scope dropped without a
    /// release or retain would have stopped its systemd unit.
    #[derive(Clone, Default)]
    struct FakeAdopter(Arc<FakeState>);

    #[derive(Default)]
    struct FakeState {
        events: Mutex<Vec<&'static str>>,
        refused: Mutex<HashSet<SplintId>>,
        /// Splints whose scope and Dojo slice systemd does not have.
        unplaced: Mutex<HashSet<SplintId>>,
        reclaims: Mutex<Vec<DojoId>>,
    }

    impl FakeAdopter {
        fn record(&self, event: &'static str) {
            self.0.events.lock().unwrap().push(event);
        }

        fn count(&self, event: &str) -> usize {
            let events = self.0.events.lock().unwrap();
            events.iter().filter(|name| **name == event).count()
        }

        fn refuse(&self, splint_id: SplintId) {
            self.0.refused.lock().unwrap().insert(splint_id);
        }
    }

    struct FakeScope {
        adopter: FakeAdopter,
        settled: bool,
    }

    impl ProcessPlacement for FakeScope {
        fn release(mut self) {
            self.settled = true;
            self.adopter.record("release");
        }

        fn retain_for_handoff(mut self) {
            self.settled = true;
            self.adopter.record("retain");
        }
    }

    impl Drop for FakeScope {
        fn drop(&mut self) {
            if !self.settled {
                self.adopter.record("stop");
            }
        }
    }

    impl WorkloadScopeAdopter for FakeAdopter {
        type Scope = FakeScope;

        fn adopt_scope(
            &self,
            retained: &RetainedWorkloadScope,
        ) -> Result<FakeScope, WorkloadUnitError> {
            if self
                .0
                .refused
                .lock()
                .unwrap()
                .contains(&retained.splint_id())
            {
                return Err(WorkloadUnitError::RetainedScopeMismatch);
            }
            self.record("adopt");
            Ok(FakeScope {
                adopter: self.clone(),
                settled: false,
            })
        }

        fn reclaim_empty_dojo_slice(
            &self,
            dojo_id: DojoId,
        ) -> Result<DojoSliceReclaim, WorkloadUnitError> {
            self.0.reclaims.lock().unwrap().push(dojo_id);
            Ok(DojoSliceReclaim::Stopped)
        }

        fn retained_units_absent(
            &self,
            retained: &RetainedWorkloadScope,
        ) -> Result<bool, WorkloadUnitError> {
            Ok(self
                .0
                .unplaced
                .lock()
                .unwrap()
                .contains(&retained.splint_id()))
        }
    }

    /// One old generation handed off in process, and the candidate stage an
    /// exec would have produced from the same descriptors.
    struct Harness {
        stage: Option<CandidateRuntimeStage>,
        topology: Topology,
        manifest: HandoffManifestV1,
        directory: PathBuf,
        _old: OldSideHandoff,
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn listener(path: &Path) -> OwnedFd {
        let listener = UnixListener::bind(path).unwrap();
        listener.set_nonblocking(true).unwrap();
        listener.into()
    }

    /// Removes and returns the entry for `slot`.
    fn take_slot<T>(entries: &mut Vec<(u16, T)>, slot: u16) -> T {
        let index = entries.iter().position(|(s, _)| *s == slot).unwrap();
        entries.remove(index).1
    }

    impl Harness {
        /// Hands off one Splint per script. `before_staging` sees each
        /// handed-off Splint and may edit the manifest before staging.
        async fn new(
            scripts: &[&str],
            before_staging: impl FnOnce(&[(u16, HandedOffSplint)], &mut HandoffManifestV1),
        ) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(1);
            let directory = std::env::temp_dir().join(format!(
                "sd-adopt-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&directory).unwrap();
            let listener_fd = listener(&directory.join("daemon.sock"));
            let image_listener_fd = listener(&directory.join("image.sock"));
            let mut old = hand_off_fleet(
                scripts,
                ListenerDescriptors {
                    listener: listener_fd.as_raw_fd(),
                    image_listener: image_listener_fd.as_raw_fd(),
                },
            )
            .await;
            let mut manifest = old.forward.clone();
            let mut handed_off = Vec::new();
            let mut checkpoints = Vec::new();
            for (slot, splint, checkpoint) in std::mem::take(&mut old.handed_off) {
                // The candidate inherits the checkpoint memfd itself.
                let body = checkpoint.body().descriptor().try_clone_to_owned().unwrap();
                checkpoints.push((slot, body));
                handed_off.push((slot, splint));
            }
            before_staging(&handed_off, &mut manifest);
            let mut masters = handed_off
                .into_iter()
                .map(|(slot, splint)| (slot, splint.session.into_parts().1))
                .collect::<Vec<_>>();
            let splints = manifest
                .splints
                .iter()
                .map(|record| {
                    let slot = record.pty.master_slot;
                    (
                        take_slot(&mut masters, slot),
                        take_slot(&mut checkpoints, slot),
                    )
                })
                .collect();
            let duplicate = |descriptor: &OwnedFd| descriptor.try_clone().unwrap();
            let stage = CandidateRuntimeStage::stage_for_test(
                RuntimeStageForTest {
                    manifest: manifest.clone(),
                    counterpart_manifest: old.rollback.clone(),
                    splints,
                    listener: listener_fd,
                    image_listener: image_listener_fd,
                    current_manifest_descriptor: duplicate(&old.forward_storage),
                    counterpart_manifest_descriptor: duplicate(&old.rollback_storage),
                    forward_client: duplicate(&old.forward_client),
                    rollback_daemon: duplicate(&old.rollback_daemon),
                    rollback_client: duplicate(&old.rollback_client),
                },
                &SharedImageBudget::new(ADOPTION_BUDGET_BYTES),
                &SharedKittyUploadBudget::new(ADOPTION_BUDGET_BYTES),
            )
            .unwrap();
            // The candidate loads the fenced Topology's durable form.
            let topology = TopologyDocument::from_topology(&old.topology)
                .unwrap()
                .into_topology()
                .unwrap();
            Self {
                stage: Some(stage),
                topology,
                manifest,
                directory,
                _old: old,
            }
        }

        fn inputs(&self, adopter: Option<FakeAdopter>) -> AdoptionInputs<FakeAdopter> {
            AdoptionInputs {
                topology: self.topology.clone(),
                workload_units: adopter,
                workload_cgroups_required: true,
                policy_path: None,
                live_config: config(),
            }
        }

        async fn adopt(
            &mut self,
            inputs: AdoptionInputs<FakeAdopter>,
        ) -> Result<StagedAdoption<FakeScope>, AdoptionFailure> {
            Box::pin(self.stage.take().unwrap().adopt(inputs)).await
        }

        fn splint_id(&self, index: usize) -> SplintId {
            self.manifest.splints[index].splint_id
        }

        /// Checks a refused adoption kept every part at its recorded
        /// number and killed nothing, then ends the carried children.
        fn assert_recovered(&self, recovery: AdoptionRecovery) -> Vec<RetainedWorkloadScope> {
            assert_eq!(recovery.lifecycle.phase(), HandoffPhase::RollingBack);
            assert_eq!(recovery.splint_count(), self.manifest.splints.len());
            let retained = recovery.retained_scopes().to_vec();
            for (stage, record) in recovery
                .authorities
                .splints
                .into_iter()
                .zip(&self.manifest.splints)
            {
                assert_eq!(stage.evidence, *record);
                let recorded = self
                    .manifest
                    .descriptors
                    .iter()
                    .find(|entry| {
                        entry.slot == HandoffDescriptorSlot::PtyMaster(record.pty.master_slot)
                    })
                    .unwrap();
                assert_eq!(
                    u32::try_from(stage.pty.master_raw_fd()).unwrap(),
                    recorded.inherited_fd
                );
                assert!(child_is_running(record.pty.child_pid), "nothing is killed");
                stage.pty.kill_child_and_wait().unwrap();
            }
            retained
        }
    }

    fn no_change(_: &[(u16, HandedOffSplint)], _: &mut HandoffManifestV1) {}

    fn child_is_running(pid: u32) -> bool {
        let pid = Pid::from_raw(i32::try_from(pid).unwrap()).unwrap();
        matches!(
            waitid(
                WaitId::Pid(pid),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            ),
            Ok(None)
        )
    }

    async fn assert_echoes(handle: &LiveSplintHandle, token: &str) {
        time::timeout(Duration::from_secs(5), async {
            handle
                .input(format!("{token}\n").into_bytes())
                .await
                .unwrap();
            while !snapshot_text(&handle.snapshot().await.unwrap()).contains(token) {
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("an adopted Splint reads its PTY and takes input");
    }

    async fn accept_one(listener: &tokio::net::UnixListener) {
        time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .expect("the registered listener accepts")
            .unwrap();
    }

    /// Replaces `<prefix>/splinterd` with a new inode, as a later package
    /// upgrade would between preparation and adoption.
    fn reinstall_daemon(prefix: &Path) {
        let staged = prefix.join(".splinterd.reinstalled");
        fs::write(&staged, "reinstalled-daemon").unwrap();
        fs::set_permissions(&staged, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        fs::rename(&staged, prefix.join(DAEMON_EXECUTABLE)).unwrap();
    }

    async fn shut_down(committed: CommittedAdoption) {
        for adopted in committed.splints {
            adopted.runtime.shutdown().await.unwrap();
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_in_process_round_trip_adopts_live_splints_and_commits() {
        let mut harness = Harness::new(&[CAT, CAT], no_change).await;
        let adopter = FakeAdopter::default();
        let carried_max = harness
            .manifest
            .splints
            .iter()
            .map(|splint| splint.incarnation)
            .max()
            .unwrap();
        let staged = harness
            .adopt(harness.inputs(Some(adopter.clone())))
            .await
            .unwrap();
        assert_eq!(staged.daemon_generation(), 2);
        assert_eq!(adopter.count("adopt"), 2);
        let committed = staged.commit();

        assert!(committed.committed());
        assert!(!committed.rollback);
        assert_eq!(committed.source_daemon_generation, 1);
        assert_eq!(committed.daemon_generation, 2);
        assert_eq!(
            committed.install_prefix.directory(),
            harness.manifest.install_prefix
        );
        assert!(committed.exited_before_adoption.is_empty());
        assert!(committed.reclaimed_slices.is_empty());
        assert!(committed.unreclaimed_slices.is_empty());
        assert!(committed.policy_generation.diagnostic.is_none());
        // Every carried incarnation is reserved before this generation's
        // first allocation.
        assert!(ProcessIncarnation::allocate().value() > carried_max);
        assert_eq!(committed.splints.len(), 2);
        for (adopted, record) in committed.splints.iter().zip(&harness.manifest.splints) {
            assert_eq!(adopted.splint_id, record.splint_id);
            assert_eq!(
                (adopted.lair_id, adopted.dojo_id),
                (record.lair_id, record.dojo_id)
            );
            assert_eq!(adopted.incarnation.value(), record.incarnation);
            assert_eq!(adopted.runtime.handle().child_pid(), record.pty.child_pid);
            assert_eq!(
                committed
                    .topology
                    .find_splint(record.splint_id)
                    .unwrap()
                    .state,
                SplintState::Running
            );
            assert_echoes(&adopted.runtime.handle(), "after-adoption").await;
        }
        // Both listeners were registered before commit and accept at their
        // bound paths; the record names the registered descriptors.
        assert!(!committed.install_pair_mismatch);
        assert_eq!(
            committed.handoff_listeners,
            ListenerDescriptors {
                listener: committed.listener.as_raw_fd(),
                image_listener: committed.image_listener.as_raw_fd(),
            }
        );
        for (name, listener) in [
            ("daemon.sock", &committed.listener),
            ("image.sock", &committed.image_listener),
        ] {
            let _client = UnixStream::connect(harness.directory.join(name)).unwrap();
            accept_one(listener).await;
        }
        assert_eq!(adopter.count("stop"), 0);
        for adopted in committed.splints {
            adopted.runtime.shutdown().await.unwrap();
        }
        // Each exit releases its adopted scope through the normal path.
        assert_eq!(adopter.count("release"), 2);
        assert_eq!(adopter.count("stop"), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_running_splint_whose_scope_fails_rolls_back_the_whole_adoption() {
        let mut harness = Harness::new(&[CAT, CAT, CAT], no_change).await;
        let adopter = FakeAdopter::default();
        let refused = harness.splint_id(1);
        adopter.refuse(refused);
        let failure = harness
            .adopt(harness.inputs(Some(adopter.clone())))
            .await
            .unwrap_err();
        assert!(matches!(
            failure.error(),
            AdoptionError::WorkloadScope { splint_id, .. } if *splint_id == refused
        ));
        // The first scope was adopted and retained again, the third never
        // adopted, and nothing stopped.
        assert_eq!(adopter.count("adopt"), 1);
        assert_eq!(adopter.count("retain"), 1);
        assert_eq!(adopter.count("stop"), 0);
        let retained = harness.assert_recovered(failure.into_recovery());
        assert_eq!(retained.len(), 1);
        assert_eq!(retained[0].splint_id(), harness.splint_id(0));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failure_mid_list_retains_every_scope_and_kills_nothing() {
        let mut harness = Harness::new(&[CAT, CAT, CAT], no_change).await;
        let adopter = FakeAdopter::default();
        let failing = harness.splint_id(1);
        crate::live::inject_adoption_reader_fault(failing);
        let failure = harness
            .adopt(harness.inputs(Some(adopter.clone())))
            .await
            .unwrap_err();
        assert!(matches!(
            failure.error(),
            AdoptionError::LiveRuntime { splint_id, .. } if *splint_id == failing
        ));
        // All three scopes were adopted; the staged first actor was aborted,
        // and every scope was retained again rather than stopped.
        assert_eq!(adopter.count("adopt"), 3);
        assert_eq!(adopter.count("retain"), 3);
        assert_eq!(adopter.count("stop") + adopter.count("release"), 0);
        assert_eq!(harness.assert_recovered(failure.into_recovery()).len(), 3);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_child_that_exits_after_capture_is_adopted_and_reaped() {
        let exiting = Mutex::new(None);
        let mut harness = Harness::new(&[CAT, "printf ready; read value; exit 3"], |splints, _| {
            for (_, splint) in splints {
                rustix::io::write(splint.session.master(), b"go\n").unwrap();
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while exiting.lock().unwrap().is_none() {
                assert!(Instant::now() < deadline, "the reading child exits");
                *exiting.lock().unwrap() = splints
                    .iter()
                    .find(|(_, splint)| !child_is_running(splint.evidence.pty_identity.child_pid()))
                    .map(|(_, splint)| splint.evidence.splint_id);
                std::thread::sleep(Duration::from_millis(5));
            }
        })
        .await;
        let exited = exiting.into_inner().unwrap().unwrap();
        let exited_dojo = harness
            .manifest
            .splints
            .iter()
            .find(|splint| splint.splint_id == exited)
            .unwrap()
            .dojo_id;
        // Systemd no longer confirms an exited child's scope.
        let adopter = FakeAdopter::default();
        adopter.refuse(exited);
        let committed = harness
            .adopt(harness.inputs(Some(adopter.clone())))
            .await
            .unwrap()
            .commit();
        assert_eq!(committed.exited_before_adoption, [exited]);
        assert_eq!(*adopter.0.reclaims.lock().unwrap(), [exited_dojo]);
        assert_eq!(committed.reclaimed_slices, [exited_dojo]);
        assert_eq!(adopter.count("adopt"), 1);
        for adopted in committed.splints {
            if adopted.splint_id == exited {
                // The committed actor reaps the child and publishes its exit.
                assert_eq!(adopted.runtime.wait().await.unwrap().code, Some(3));
            } else {
                assert_echoes(&adopted.runtime.handle(), "survivor").await;
                adopted.runtime.shutdown().await.unwrap();
            }
        }
        assert_eq!(adopter.count("stop"), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mismatched_authority_fails_closed_before_any_scope_is_adopted() {
        // The prefix holds no forward pair.
        let mut harness = Harness::new(&[CAT], |_, manifest| {
            manifest.install_prefix = manifest.install_prefix.parent().unwrap().to_path_buf();
        })
        .await;
        let adopter = FakeAdopter::default();
        let failure = harness
            .adopt(harness.inputs(Some(adopter.clone())))
            .await
            .unwrap_err();
        assert!(matches!(
            failure.error(),
            AdoptionError::InstallPrefixMismatch
        ));
        harness.assert_recovered(failure.into_recovery());

        // A durable Topology one revision ahead of the manifest.
        let mut harness = Harness::new(&[CAT], no_change).await;
        let mut inputs = harness.inputs(Some(adopter.clone()));
        inputs
            .topology
            .create_lair("later", PathBuf::from("/tmp"))
            .unwrap();
        let failure = harness.adopt(inputs).await.unwrap_err();
        assert!(matches!(
            failure.error(),
            AdoptionError::TopologyRevision { .. }
        ));
        harness.assert_recovered(failure.into_recovery());

        // A durable Topology at the same revision without the carried Splint.
        let mut harness = Harness::new(&[CAT], no_change).await;
        let mut inputs = harness.inputs(Some(adopter.clone()));
        inputs.topology = Topology::new();
        inputs
            .topology
            .create_lair("other", PathBuf::from("/tmp"))
            .unwrap();
        assert_eq!(inputs.topology.revision(), harness.topology.revision());
        let failure = harness.adopt(inputs).await.unwrap_err();
        assert!(matches!(
            failure.error(),
            AdoptionError::MissingSplint(splint_id) if *splint_id == harness.splint_id(0)
        ));
        harness.assert_recovered(failure.into_recovery());

        // Required workload cgroups without a manager.
        let mut harness = Harness::new(&[CAT], no_change).await;
        let failure = harness.adopt(harness.inputs(None)).await.unwrap_err();
        assert!(matches!(
            failure.error(),
            AdoptionError::WorkloadCgroupsUnavailable
        ));
        harness.assert_recovered(failure.into_recovery());
        assert_eq!(adopter.count("adopt"), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_aborted_staged_adoption_returns_every_part() {
        let mut harness = Harness::new(&[CAT, CAT], no_change).await;
        let adopter = FakeAdopter::default();
        let staged = harness
            .adopt(harness.inputs(Some(adopter.clone())))
            .await
            .unwrap();
        let recovery = staged.abort();
        assert_eq!(adopter.count("retain"), 2);
        assert_eq!(adopter.count("stop") + adopter.count("release"), 0);
        assert_eq!(harness.assert_recovered(recovery).len(), 2);
    }

    /// A daemon that cannot build adoption's inputs refuses before adopting
    /// and still returns every part at its recorded number, killing nothing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_refusal_before_adoption_returns_every_part() {
        let mut harness = Harness::new(&[CAT, CAT], no_change).await;
        let failure = harness
            .stage
            .take()
            .unwrap()
            .refuse(AdoptionError::DaemonStartup("no metadata store".to_owned()));
        assert!(matches!(failure.error(), AdoptionError::DaemonStartup(_)));
        assert!(failure.to_string().contains("no metadata store"));
        assert!(harness.assert_recovered(failure.into_recovery()).is_empty());
    }

    /// The committed adoption names the installed daemon its image came from
    /// as the manifest recorded it, never the running memfd.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_committed_adoption_carries_the_recorded_daemon_image() {
        let mut harness = Harness::new(&[CAT], no_change).await;
        let recorded = harness.manifest.executable_pairs.forward.daemon.clone();
        let staged = harness
            .adopt(harness.inputs(Some(FakeAdopter::default())))
            .await
            .unwrap();
        let committed = staged.commit();
        assert_eq!(
            committed.daemon_image,
            AdoptedDaemonImage {
                device: recorded.source_device,
                inode: recorded.source_inode,
                sha256: recorded.source_sha256,
            }
        );
        shut_down(committed).await;
    }

    /// A Dojo whose only Splint was excluded before capture holds no adopted
    /// scope, so its slice is reclaimed when systemd proves it empty, and
    /// a Dojo with an adopted scope is not.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_excluded_only_dojo_has_its_empty_slice_reclaimed() {
        let excluded_dojo = DojoId::new();
        let mut harness = Harness::new(&[CAT], |_, manifest| {
            manifest.excluded_splints.push(ManifestExcludedSplint {
                lair_id: LairId::new(),
                dojo_id: excluded_dojo,
                splint_id: SplintId::new(),
                incarnation: 7,
                child_pid: 999_999,
                exit: None,
            });
        })
        .await;
        let adopter = FakeAdopter::default();
        let staged = harness
            .adopt(harness.inputs(Some(adopter.clone())))
            .await
            .unwrap();
        assert_eq!(*adopter.0.reclaims.lock().unwrap(), [excluded_dojo]);
        let committed = staged.commit();
        assert_eq!(committed.reclaimed_slices, [excluded_dojo]);
        shut_down(committed).await;
    }

    /// Each excluded Splint gets its real exit: its reaped child's status,
    /// else the one the old actor recorded, else the adoption default; a
    /// record of an older incarnation changes nothing.
    #[test]
    fn excluded_splints_are_finalized_with_their_real_exit() {
        let mut topology = Topology::new();
        let mut splint_ids = Vec::new();
        for name in ["reaped", "recorded", "unknown", "stale"] {
            let lair = topology.create_lair(name, PathBuf::from("/tmp")).unwrap();
            let LayoutNode::Leaf(splint) = &lair.dojos[0].root else {
                panic!("a new Lair has one Splint");
            };
            splint_ids.push(splint.id);
        }
        for splint_id in &splint_ids {
            topology.set_splint_last_incarnation(*splint_id, 5);
            topology.set_splint_state(*splint_id, SplintState::Exited(0));
        }
        let child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 3"])
            .spawn()
            .unwrap()
            .id();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !child_has_exited(child) {
            assert!(Instant::now() < deadline, "the child did not exit");
            std::thread::sleep(Duration::from_millis(5));
        }
        // PID 1 is never this process's child: its status cannot be reaped.
        let record = |index: usize, child_pid, incarnation, exit| ManifestExcludedSplint {
            lair_id: LairId::new(),
            dojo_id: DojoId::new(),
            splint_id: splint_ids[index],
            incarnation,
            child_pid,
            exit,
        };
        let killed = Some(crate::handoff_manifest::ManifestProcessExit {
            code: None,
            signal: Some(9),
        });
        let records = [
            record(0, child, 5, None),
            record(1, 1, 5, killed),
            record(2, 1, 5, None),
            record(3, 1, 4, killed),
        ];
        let finalized = finalize_excluded(&mut topology, &records);
        let state = |index: usize| topology.find_splint(splint_ids[index]).unwrap().state;
        assert_eq!(state(0), SplintState::Exited(3));
        assert_eq!(state(1), SplintState::Exited(137));
        assert_eq!(state(2), SplintState::Exited(0));
        assert_eq!(state(3), SplintState::Exited(0));
        assert_eq!(finalized[0].1.and_then(|exit| exit.code), Some(3));
        assert_eq!(finalized[1].1.and_then(|exit| exit.signal), Some(9));
        assert!(finalized[2].1.is_none() && finalized[3].1.is_none());
        assert!(!child_has_exited(child), "the child was reaped");
    }

    /// A thread that exits between listing the tasks and reading its
    /// `children` file is skipped, not the whole pass; a task directory that
    /// cannot be read still fails.
    #[test]
    fn a_vanished_thread_does_not_abandon_the_child_list() {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let tasks = std::env::temp_dir().join(format!(
            "sd-tasks-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        for (task, children) in [
            ("100", Some("12 34 ")),
            ("101", None),
            ("102", Some("34 56")),
        ] {
            fs::create_dir_all(tasks.join(task)).unwrap();
            if let Some(children) = children {
                fs::write(tasks.join(task).join("children"), children).unwrap();
            }
        }
        assert_eq!(child_pids_in(&tasks).unwrap(), [12, 34, 56]);
        // The real list reads even while threads come and go.
        assert!(child_pids().is_ok());
        fs::remove_dir_all(&tasks).unwrap();
        assert!(child_pids_in(&tasks).is_err());
    }

    /// Exited children that are not carried are reaped; a carried child and
    /// a running one are left alone.
    #[test]
    fn only_exited_uncarried_children_are_reaped() {
        let spawn = |script: &str| {
            std::process::Command::new("/bin/sh")
                .args(["-c", script])
                .spawn()
                .unwrap()
                .id()
        };
        let exited = spawn("exit 4");
        let carried = spawn("exit 5");
        let mut running = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 30"])
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !(child_has_exited(exited) && child_has_exited(carried)) {
            assert!(Instant::now() < deadline, "children did not exit");
            std::thread::sleep(Duration::from_millis(5));
        }
        let reaped = reap_exited_among([exited, carried, running.id()], &HashSet::from([carried]));
        assert_eq!(reaped.len(), 1);
        assert_eq!(reaped[0].0, exited);
        assert_eq!(reaped[0].1.code, Some(4));
        assert!(
            child_has_exited(carried),
            "the carried child stays unreaped"
        );
        assert!(crate::handoff_exec::reap_exited_child(carried).is_some());
        running.kill().unwrap();
        running.wait().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_changed_install_pair_refuses_forward_but_not_rollback() {
        let mut harness = Harness::new(&[CAT], |_, manifest| {
            reinstall_daemon(&manifest.install_prefix);
        })
        .await;
        let failure = harness
            .adopt(harness.inputs(Some(FakeAdopter::default())))
            .await
            .unwrap_err();
        assert!(matches!(
            failure.error(),
            AdoptionError::InstallPrefixMismatch
        ));
        harness.assert_recovered(failure.into_recovery());

        // Rollback keeps the Splints and reports the changed pair.
        let mut harness = Harness::new(&[CAT], |_, manifest| {
            manifest.kind = ManifestKind::Rollback;
            reinstall_daemon(&manifest.install_prefix);
        })
        .await;
        let committed = harness
            .adopt(harness.inputs(Some(FakeAdopter::default())))
            .await
            .unwrap()
            .commit();
        assert!(committed.rollback);
        assert!(committed.install_pair_mismatch);
        assert_echoes(&committed.splints[0].runtime.handle(), "after-rollback").await;
        shut_down(committed).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_old_uncontained_child_is_adopted_uncontained_only_when_cgroups_are_optional() {
        // Optional cgroups, and systemd has neither the scope nor the slice:
        // the child runs uncontained, its sibling keeps its scope.
        let mut harness = Harness::new(&[CAT, CAT], no_change).await;
        let unplaced = harness.splint_id(0);
        let adopter = FakeAdopter::default();
        adopter.refuse(unplaced);
        adopter.0.unplaced.lock().unwrap().insert(unplaced);
        let mut inputs = harness.inputs(Some(adopter.clone()));
        inputs.workload_cgroups_required = false;
        let committed = harness.adopt(inputs).await.unwrap().commit();
        assert_eq!(adopter.count("adopt"), 1);
        assert!(committed.exited_before_adoption.is_empty());
        for adopted in &committed.splints {
            assert_echoes(&adopted.runtime.handle(), "uncontained").await;
        }
        shut_down(committed).await;
        assert_eq!(adopter.count("release"), 1);
        assert_eq!(adopter.count("stop"), 0);

        // Required cgroups keep the sibling-kill rule for the same child.
        let mut harness = Harness::new(&[CAT, CAT], no_change).await;
        let unplaced = harness.splint_id(0);
        let adopter = FakeAdopter::default();
        adopter.refuse(unplaced);
        adopter.0.unplaced.lock().unwrap().insert(unplaced);
        let failure = harness
            .adopt(harness.inputs(Some(adopter.clone())))
            .await
            .unwrap_err();
        assert!(matches!(
            failure.error(),
            AdoptionError::WorkloadScope { splint_id, .. } if *splint_id == unplaced
        ));
        harness.assert_recovered(failure.into_recovery());

        // So does a loaded slice with optional cgroups: it could stop under
        // an uncounted child.
        let mut harness = Harness::new(&[CAT, CAT], no_change).await;
        let refused = harness.splint_id(0);
        let adopter = FakeAdopter::default();
        adopter.refuse(refused);
        let mut inputs = harness.inputs(Some(adopter.clone()));
        inputs.workload_cgroups_required = false;
        let failure = harness.adopt(inputs).await.unwrap_err();
        assert!(matches!(
            failure.error(),
            AdoptionError::WorkloadScope { splint_id, .. } if *splint_id == refused
        ));
        harness.assert_recovered(failure.into_recovery());
        assert_eq!(adopter.count("stop"), 0);
    }

    #[test]
    fn only_carried_splints_are_running_after_adoption() {
        let mut topology = Topology::new();
        let carried = topology
            .create_lair("carried", PathBuf::from("/tmp"))
            .unwrap()
            .dojos[0]
            .default_focus;
        let stale = topology
            .create_lair("stale", PathBuf::from("/tmp"))
            .unwrap()
            .dojos[0]
            .default_focus;
        assert!(topology.set_splint_state(stale, SplintState::Running));
        mark_adopted_states(&mut topology, &HashSet::from([carried]));
        assert_eq!(
            topology.find_splint(carried).unwrap().state,
            SplintState::Running
        );
        assert_eq!(
            topology.find_splint(stale).unwrap().state,
            SplintState::Exited(0)
        );
    }
}
