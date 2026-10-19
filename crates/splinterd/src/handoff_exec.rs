//! The old generation's exec boundary and its in-process resume (Plan 0061
//! slice M3.9; Plan 0037 "Descriptor discipline" and "Failure and rollback").
//!
//! The binary reaches this module only after serve has left with a fenced,
//! captured attempt, every captured actor has ended for handoff, and the Tokio
//! runtime and the systemd connection are gone. [`cross_exec_boundary`] then
//! runs the single-threaded critical section:
//!
//! 0. block SIGINT, SIGTERM, and SIGHUP on the exec thread
//!    ([`BlockedHandoffSignals`]);
//! 1. take the process guard ([`HandoffProcessGuard::acquire_until`]), which
//!    proves the process has exactly one thread;
//! 2. write exactly the encoded manifest bytes into both empty manifest memfds
//!    and seal them;
//! 3. clear close-on-exec on exactly the descriptors the sealed manifests
//!    record, at exactly the numbers they record
//!    ([`PreparedDescriptorInheritance::prepare`]); nothing is renumbered
//!    once a manifest is sealed;
//! 4. exec the forward daemon (`execveat(AT_EMPTY_PATH)` of its sealed
//!    snapshot), at the one call site in `under_guard`;
//! 5. when exec does not happen, restore close-on-exec, release the guard,
//!    and restore the signal mask.
//!
//! Step 4 is compiled only into `integration-test` builds (Plan 0061 M3.10).
//! The forward candidate adopts and serves in every build, and since Plan
//! 0061 M3.11a a candidate failure before its commit rolls back through
//! [`crate::handoff_rollback`]; enabling the forward exec in production
//! builds is M3.11b's decision. Every other build keeps the M3.9 behaviour:
//! the boundary always returns, and the old generation resumes in process
//! (Plan 0061 D5) with [`resume_handed_off`]: every Splint keeps running and
//! every client reconnects to the same daemon generation.
//!
//! # Signals across exec
//!
//! A successful exec resets every caught signal to its default action, which
//! for SIGINT, SIGTERM, and SIGHUP is to terminate. Until the candidate has
//! installed its own handlers, `systemctl stop` or `reload` would therefore
//! kill it mid-adoption and hang up every carried Splint. Both handoff execs
//! (this boundary's forward exec and the rollback exec,
//! [`exec_rollback_daemon`]) run with those three signals blocked: the
//! blocked mask and the pending set both survive `execve`, and the candidate
//! unblocks them right after installing its handlers
//! ([`unblock_handoff_signals`]), so a signal that arrived in the window is
//! delivered to the handler and acted on in order (a SIGTERM becomes an
//! ordered shutdown after adoption). When exec does not happen, the old
//! generation restores its mask and a pending signal reaches its still
//! installed handler before the resumed round serves.
//!
//! No child may inherit the blocked mask. `std::process::Command` passes the
//! spawning thread's signal mask on to the child (it has since Rust 1.66;
//! only SIGPIPE's disposition is reset), and threads inherit their creator's
//! mask, so the mask is confined to threads that never spawn:
//!
//! * The old generation blocks only on its one remaining thread, under the
//!   process guard, where nothing spawns, and restores the mask before a
//!   resumed round builds its runtime.
//! * A candidate installs its handlers and unblocks before it builds its
//!   serving runtime (`release_handoff_signals` in the binary), so no
//!   worker or blocking-pool thread, and nothing they spawn (Splints, the
//!   consent launcher, the sealed-preflight launcher), ever carries it.
//! * `splinterm-pty-child` also empties the mask before it executes a
//!   Splint's command, so a Splint starts with nothing blocked whatever the
//!   spawning thread had.
//!
//! # Exec failure past the point of no return
//!
//! `execveat` returns an error only while the kernel can still return to
//! the calling image. A failure after its point of no return (for example
//! an allocation failure while mapping the new image) kills the process
//! instead: there is no fall-through, every PTY master closes with the
//! process, and every carried Splint is hung up. That belongs to the
//! daemon-crash row of Plan 0037's lifetime matrix.
//!
//! # No kill
//!
//! Nothing here signals another process or stops a workload scope. A
//! resumed Splint whose scope cannot be adopted runs uncontained, and every
//! scope already adopted in its Dojo stays retained in the new manager, so
//! that manager never stops the Dojo slice under it (the M3.8a sibling-kill
//! rule). A Splint whose actor cannot be staged again keeps its PTY master
//! open for the life of the process: it stalls once its output fills the
//! kernel buffer, but it is never hung up.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    os::fd::{AsRawFd, BorrowedFd, IntoRawFd, RawFd},
    time::{Duration, Instant},
};

use nix::sys::signal::{SigSet, SigmaskHow, Signal, pthread_sigmask};
use rustix::{
    fs::{fcntl_add_seals, fcntl_get_seals, fstat},
    io::pwrite,
    process::{Pid, WaitId, WaitIdOptions, waitid},
};
use splinterm_core::{DojoId, SplintId};
use splinterm_filemap::REQUIRED_IMMUTABLE_SEALS;
use splinterm_terminal::{
    MAX_TERMINAL_CHECKPOINT_BYTES, SharedImageBudget, SharedKittyUploadBudget, Terminal,
};
use thiserror::Error;
use tokio::time;
use tracing::{error, warn};

use crate::{
    HandedOffSplint, LiveSplintConfig, LiveSplintRuntime, ProcessExit, ProcessIncarnation,
    ProcessPlacement,
    candidate_bootstrap::read_immutable_descriptor,
    cgroup::RetainedWorkloadScope,
    handoff_adoption::{AdoptedPlacement, RetainOnDrop, WorkloadScopeAdopter, child_has_exited},
    handoff_checkpoint::LinkedCheckpointBody,
    handoff_coordinator::{HandoffCoordinatorError, HandoffProcessGuard},
    handoff_descriptors::{
        DescriptorHandoffError, HandoffDescriptor, HandoffDescriptorSlot,
        PreparedDescriptorInheritance,
    },
    handoff_prepare::PreparedManifests,
};

/// How long the boundary waits for the stopped runtime's and the systemd
/// connection's threads to leave `/proc/self/task`.
///
/// Both were joined before the boundary starts, so the kernel normally
/// finishes reaping them within milliseconds. A thread still alive after
/// this long was not joined (a wedged blocking task the runtime abandoned),
/// and the old generation resumes rather than wait for it.
pub const HANDOFF_GUARD_TIMEOUT: Duration = Duration::from_secs(2);

/// How long resume waits for the new workload unit manager to adopt every
/// retained scope.
///
/// Each adoption makes at most four 500 ms systemd requests, so a healthy
/// bus adopts a few Splints in well under a second. Past this bound every
/// Splint resumes uncontained; scopes the abandoned adoption still takes are
/// retained, never stopped.
pub const HANDOFF_RESUME_SCOPE_TIMEOUT: Duration = Duration::from_secs(5);

/// One manifest encoding and the empty memfd the manifest names for it.
#[derive(Clone, Copy, Debug)]
pub struct ManifestSeal<'fd> {
    storage: BorrowedFd<'fd>,
    bytes: &'fd [u8],
}

impl<'fd> ManifestSeal<'fd> {
    #[must_use]
    pub const fn new(storage: BorrowedFd<'fd>, bytes: &'fd [u8]) -> Self {
        Self { storage, bytes }
    }
}

/// Every descriptor exec inherits, bound to the slot and number the manifests
/// record, and both manifest encodings to seal.
///
/// Building it checks numbers only; it changes no descriptor.
#[derive(Debug)]
pub struct ExecInheritancePlan<'fd> {
    manifests: [ManifestSeal<'fd>; 2],
    descriptors: Vec<HandoffDescriptor<'fd>>,
    /// What the exec call site executes; without it the boundary returns
    /// [`ExecBoundaryError::ExecNotWired`].
    #[cfg(feature = "integration-test")]
    forward: Option<ForwardDaemonExec<'fd>>,
}

/// The sealed forward daemon image and the exact argument and environment
/// vectors the exec call site hands it.
#[cfg(feature = "integration-test")]
pub struct ForwardDaemonExec<'fd> {
    daemon: BorrowedFd<'fd>,
    arguments: Vec<std::ffi::CString>,
    environment: Vec<std::ffi::CString>,
}

#[cfg(feature = "integration-test")]
impl std::fmt::Debug for ForwardDaemonExec<'_> {
    // The environment may hold credentials; only its size is diagnostic.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ForwardDaemonExec")
            .field("daemon", &self.daemon)
            .field("arguments", &self.arguments)
            .field("environment_entries", &self.environment.len())
            .finish()
    }
}

/// A handoff daemon's argument vector: `program`, the candidate activation
/// flag naming the sealed manifest at `manifest` (the forward manifest for
/// the forward daemon, the rollback manifest for the rollback daemon), then
/// the ordinary daemon arguments re-serialized from the running daemon's
/// parsed state.
///
/// Never a copy of the running argv: a candidate's own argv still carries
/// its activation flag, and a second one fails closed as a candidate.
/// `None` when `program` holds a NUL byte, which no real argv entry can.
#[must_use]
pub fn candidate_daemon_arguments(
    program: &std::ffi::OsStr,
    manifest: RawFd,
    workload_cgroups_required: bool,
) -> Option<Vec<std::ffi::CString>> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let mut arguments = vec![
        CString::new(program.as_bytes()).ok()?,
        CString::new(format!(
            "{}={manifest}",
            crate::CANDIDATE_ACTIVATION_ARGUMENT
        ))
        .ok()?,
    ];
    if workload_cgroups_required {
        arguments.push(CString::new(crate::REQUIRE_WORKLOAD_CGROUPS_ARGUMENT).ok()?);
    }
    Some(arguments)
}

/// This process's environment, unchanged, as exec's environment vector:
/// `SPLINTERM_POLICY`, `SPLINTERM_SOCKET`, `XDG_*`, and the systemd variables
/// cross exec as they would a restart. Candidate activation is argv only
/// (Plan 0061 D4), so nothing in the environment activates a candidate.
#[must_use]
pub fn exec_environment() -> Vec<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;

    std::env::vars_os()
        .filter_map(|(key, value)| {
            let mut entry = key.as_bytes().to_vec();
            entry.push(b'=');
            entry.extend_from_slice(value.as_bytes());
            std::ffi::CString::new(entry).ok()
        })
        .collect()
}

impl<'fd> ExecInheritancePlan<'fd> {
    /// Binds each recorded slot to the one owner the caller holds for it.
    ///
    /// `recorded` is the manifests' descriptor list in canonical order; the
    /// plan keeps that order. Both manifest memfds must be among the owners.
    ///
    /// # Errors
    /// Refuses a slot with two owners, a recorded slot without an owner, an
    /// owner at another number than the manifest records (a descriptor that
    /// was renumbered after the manifest was built), an owner the manifest
    /// does not record, or a manifest memfd that is not inherited.
    pub fn new(
        recorded: &[(HandoffDescriptorSlot, RawFd)],
        owners: &[HandoffDescriptor<'fd>],
        manifests: [ManifestSeal<'fd>; 2],
    ) -> Result<Self, ExecBoundaryError> {
        let descriptors = bind_recorded_descriptors(recorded, owners)?;
        for manifest in &manifests {
            let storage = manifest.storage.as_raw_fd();
            if !descriptors
                .iter()
                .any(|descriptor| descriptor.raw_fd() == storage)
            {
                return Err(ExecBoundaryError::ManifestNotInherited(storage));
            }
        }
        Ok(Self {
            manifests,
            descriptors,
            #[cfg(feature = "integration-test")]
            forward: None,
        })
    }

    /// The number the manifests record for the forward manifest
    /// (`CheckpointManifest`), which the forward daemon's activation flag
    /// names.
    #[cfg(feature = "integration-test")]
    #[must_use]
    pub fn forward_manifest_descriptor(&self) -> Option<RawFd> {
        self.descriptors
            .iter()
            .find(|descriptor| descriptor.slot() == HandoffDescriptorSlot::CheckpointManifest)
            .map(|descriptor| descriptor.raw_fd())
    }

    /// Arms the exec call site with the sealed forward daemon image and the
    /// exact vectors exec passes it; see [`candidate_daemon_arguments`] and
    /// [`exec_environment`]. The image is executed through its descriptor,
    /// which stays close-on-exec and is not among the inherited ones.
    #[cfg(feature = "integration-test")]
    #[must_use]
    pub fn with_forward_daemon(
        mut self,
        daemon: BorrowedFd<'fd>,
        arguments: Vec<std::ffi::CString>,
        environment: Vec<std::ffi::CString>,
    ) -> Self {
        self.forward = Some(ForwardDaemonExec {
            daemon,
            arguments,
            environment,
        });
        self
    }

    /// [`Self::new`] for a prepared manifest pair: its recorded descriptors,
    /// its two encodings and memfds, which this adds as the
    /// `CheckpointManifest` and `RollbackManifest` owners, and the caller's
    /// other owners.
    ///
    /// # Errors
    /// As [`Self::new`].
    pub fn for_prepared(
        manifests: &'fd PreparedManifests,
        owners: &[HandoffDescriptor<'fd>],
    ) -> Result<Self, ExecBoundaryError> {
        let mut all = owners.to_vec();
        all.push(HandoffDescriptor::new(
            HandoffDescriptorSlot::CheckpointManifest,
            manifests.forward_storage(),
        ));
        all.push(HandoffDescriptor::new(
            HandoffDescriptorSlot::RollbackManifest,
            manifests.rollback_storage(),
        ));
        Self::new(
            &manifests.recorded_descriptors(),
            &all,
            [
                ManifestSeal::new(manifests.forward_storage(), manifests.forward_bytes()),
                ManifestSeal::new(manifests.rollback_storage(), manifests.rollback_bytes()),
            ],
        )
    }

    /// The inherited descriptors, in manifest order.
    #[must_use]
    pub fn descriptors(&self) -> &[HandoffDescriptor<'fd>] {
        &self.descriptors
    }
}

/// Binds each recorded slot to the one owner held for it, in `recorded`
/// order, requiring every owner at exactly the number the manifest records.
///
/// # Errors
/// Refuses a slot with two owners, a recorded slot without an owner, an
/// owner at another number than recorded, or an owner nothing records.
pub(crate) fn bind_recorded_descriptors<'fd>(
    recorded: &[(HandoffDescriptorSlot, RawFd)],
    owners: &[HandoffDescriptor<'fd>],
) -> Result<Vec<HandoffDescriptor<'fd>>, ExecBoundaryError> {
    let mut by_slot = BTreeMap::new();
    for owner in owners {
        if by_slot.insert(owner.slot(), *owner).is_some() {
            return Err(ExecBoundaryError::DuplicateOwner(owner.slot()));
        }
    }
    let mut descriptors = Vec::with_capacity(recorded.len());
    for &(slot, recorded) in recorded {
        let owner = by_slot
            .remove(&slot)
            .ok_or(ExecBoundaryError::MissingOwner(slot))?;
        if owner.raw_fd() != recorded {
            return Err(ExecBoundaryError::Renumbered {
                slot,
                recorded,
                held: owner.raw_fd(),
            });
        }
        descriptors.push(owner);
    }
    if let Some(slot) = by_slot.into_keys().next() {
        return Err(ExecBoundaryError::UnrecordedOwner(slot));
    }
    Ok(descriptors)
}

/// The signals a handoff exec carries blocked: the ones systemd sends a
/// service (`stop`, `reload`) and an interactive interrupt. Their default
/// action terminates, and exec resets caught signals to their default.
const HANDOFF_EXEC_SIGNALS: [Signal; 3] = [Signal::SIGINT, Signal::SIGTERM, Signal::SIGHUP];

fn handoff_signal_set() -> SigSet {
    let mut set = SigSet::empty();
    for signal in HANDOFF_EXEC_SIGNALS {
        set.add(signal);
    }
    set
}

/// SIGINT, SIGTERM, and SIGHUP blocked on the calling thread for a handoff
/// exec; dropping it restores the thread's previous mask, so a signal that
/// arrived meanwhile is delivered then. See the module documentation.
///
/// The mask is per thread. Every handoff exec runs under the single-threaded
/// process guard, so the exec thread is the only thread and its mask is
/// what crosses exec.
#[must_use = "dropping it restores the signal mask at once"]
#[derive(Debug)]
pub struct BlockedHandoffSignals {
    previous: SigSet,
}

impl BlockedHandoffSignals {
    /// Blocks SIGINT, SIGTERM, and SIGHUP on the calling thread.
    ///
    /// # Errors
    /// Reports a failed `pthread_sigmask`, which changes nothing.
    pub fn block() -> io::Result<Self> {
        let mut previous = SigSet::empty();
        pthread_sigmask(
            SigmaskHow::SIG_BLOCK,
            Some(&handoff_signal_set()),
            Some(&mut previous),
        )?;
        Ok(Self { previous })
    }
}

impl Drop for BlockedHandoffSignals {
    fn drop(&mut self) {
        if let Err(error) = pthread_sigmask(SigmaskHow::SIG_SETMASK, Some(&self.previous), None) {
            error!(%error, "cannot restore the signal mask after a handoff exec boundary");
        }
    }
}

/// Unblocks SIGINT, SIGTERM, and SIGHUP on the calling thread: a candidate
/// calls it right after installing its handlers, on the thread that
/// inherited the blocked mask across exec. Any of them that arrived while
/// blocked is delivered to the handler now. Harmless when none is blocked.
///
/// # Errors
/// Reports a failed `pthread_sigmask`.
pub fn unblock_handoff_signals() -> io::Result<()> {
    Ok(pthread_sigmask(
        SigmaskHow::SIG_UNBLOCK,
        Some(&handoff_signal_set()),
        None,
    )?)
}

/// Makes `signal` pending again while [`BlockedHandoffSignals`] holds it
/// blocked, so it crosses the next exec instead of being lost. Used for a
/// termination a failed candidate's handler already consumed (see
/// [`crate::handoff_rollback`]).
///
/// # Errors
/// Reports a failed `raise`.
pub(crate) fn replay_blocked_signal(
    _blocked: &BlockedHandoffSignals,
    signal: Signal,
) -> io::Result<()> {
    Ok(nix::sys::signal::raise(signal)?)
}

/// One step of [`cross_exec_boundary`], in order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecBoundaryStage {
    Guard,
    SealForwardManifest,
    SealRollbackManifest,
    Inheritance,
    Exec,
    Restore,
}

/// Why the old generation is still running after the exec boundary.
#[derive(Debug, Error)]
pub enum ExecBoundaryError {
    #[error("handoff descriptor slot {0:?} has more than one owner")]
    DuplicateOwner(HandoffDescriptorSlot),
    #[error("the manifest records {0:?}, but nothing owns it")]
    MissingOwner(HandoffDescriptorSlot),
    #[error("{slot:?} is held at descriptor {held}, but the manifest records {recorded}")]
    Renumbered {
        slot: HandoffDescriptorSlot,
        recorded: RawFd,
        held: RawFd,
    },
    #[error("{0:?} is owned but the manifest does not record it")]
    UnrecordedOwner(HandoffDescriptorSlot),
    #[error("manifest memfd {0} is not among the inherited descriptors")]
    ManifestNotInherited(RawFd),
    #[error("the daemon's listeners are not at the numbers it recorded")]
    ListenerMismatch,
    #[error("a captured actor did not confirm its handoff end")]
    ActorsNotHandedOff,
    #[error("cannot record the excluded Splints' final exits in the manifests: {0}")]
    ManifestRefresh(#[source] crate::handoff_prepare::ManifestPreparationError),
    #[error("the process did not reach one thread: {0}")]
    Guard(#[source] HandoffCoordinatorError),
    #[error("cannot write and seal the {kind} manifest: {source}")]
    Seal {
        kind: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("cannot prepare descriptor inheritance: {0}")]
    Inheritance(#[source] DescriptorHandoffError),
    #[error("cannot block the handoff signals: {0}")]
    SignalMask(#[source] io::Error),
    /// This build does not exec (every build but `integration-test` until
    /// Plan 0061 M3.11b), or the plan names no forward daemon.
    #[error("exec of the forward daemon is not enabled (Plan 0061 M3.11b gates it)")]
    ExecNotWired,
    /// `execveat` of the forward daemon returned.
    #[cfg(feature = "integration-test")]
    #[error("exec of the forward daemon failed: {0}")]
    Exec(#[source] io::Error),
    #[error("injected exec boundary fault at {0:?}")]
    Injected(ExecBoundaryStage),
    #[error("{cause}; restoring close-on-exec then failed: {restore}")]
    Restore {
        cause: Box<Self>,
        restore: Box<Self>,
    },
}

/// Runs the exec boundary and returns why the old generation resumes.
///
/// See the module documentation for the steps. A successful exec never
/// returns; a build without the exec call site (every build but
/// `integration-test`, Plan 0061 M3.10) always returns, at the latest with
/// [`ExecBoundaryError::ExecNotWired`]. Every return has released the guard,
/// and every one after inheritance preparation has restored close-on-exec
/// (or reports in [`ExecBoundaryError::Restore`] that it could not, after
/// which the inheritance value's drop retried). Sealing is permanent: a
/// resumed generation discards both sealed memfds, and a later attempt
/// builds new ones.
///
/// The caller must have stopped every thread it owns: the Tokio runtime,
/// its blocking pool, and the systemd connection. Until the guard is taken
/// nothing here changes any descriptor.
#[must_use = "the boundary returns only to say why the old generation resumes"]
pub fn cross_exec_boundary(
    plan: &ExecInheritancePlan<'_>,
    guard_deadline: Instant,
) -> ExecBoundaryError {
    cross_with(plan, guard_deadline, &mut |_| Ok(()))
}

/// [`cross_exec_boundary`] with `hook` called before each stage. The hook may
/// inspect the process (descriptor flags at [`ExecBoundaryStage::Exec`]), and
/// an `Err` injects a failure at that stage, which then resumes as a real
/// failure there would.
#[cfg(feature = "integration-test")]
#[doc(hidden)]
#[must_use]
pub fn cross_exec_boundary_hooked_for_integration_test(
    plan: &ExecInheritancePlan<'_>,
    guard_deadline: Instant,
    hook: &mut dyn FnMut(ExecBoundaryStage) -> Result<(), ()>,
) -> ExecBoundaryError {
    cross_with(plan, guard_deadline, hook)
}

type StageHook<'h> = &'h mut dyn FnMut(ExecBoundaryStage) -> Result<(), ()>;

fn stage(hook: &mut StageHook<'_>, stage: ExecBoundaryStage) -> Result<(), ExecBoundaryError> {
    hook(stage).map_err(|()| ExecBoundaryError::Injected(stage))
}

fn cross_with(
    plan: &ExecInheritancePlan<'_>,
    guard_deadline: Instant,
    mut hook: StageHook<'_>,
) -> ExecBoundaryError {
    // Blocked for the whole boundary, in every build, so a signal that
    // arrives anywhere in it either crosses exec pending or is delivered to
    // this generation's handler once the mask is restored on return.
    let signals = match BlockedHandoffSignals::block() {
        Ok(signals) => signals,
        Err(error) => return ExecBoundaryError::SignalMask(error),
    };
    let guard = match stage(&mut hook, ExecBoundaryStage::Guard).and_then(|()| {
        HandoffProcessGuard::acquire_until(guard_deadline).map_err(ExecBoundaryError::Guard)
    }) {
        Ok(guard) => guard,
        Err(failure) => return failure,
    };
    let failure = under_guard(&guard, plan, hook);
    drop(guard);
    drop(signals);
    failure
}

/// Steps 2 to 5, single-threaded. Holding `_guard` is the proof that no other
/// thread can create, duplicate, close, or exec descriptors meanwhile, so
/// clearing close-on-exec here cannot leak a descriptor into a concurrent
/// child (Plan 0061 M3.7a review).
fn under_guard(
    _guard: &HandoffProcessGuard,
    plan: &ExecInheritancePlan<'_>,
    mut hook: StageHook<'_>,
) -> ExecBoundaryError {
    for (stage_name, kind, manifest) in [
        (
            ExecBoundaryStage::SealForwardManifest,
            "forward",
            plan.manifests[0],
        ),
        (
            ExecBoundaryStage::SealRollbackManifest,
            "rollback",
            plan.manifests[1],
        ),
    ] {
        if let Err(failure) = stage(&mut hook, stage_name) {
            return failure;
        }
        if let Err(source) = seal_manifest(manifest) {
            return ExecBoundaryError::Seal { kind, source };
        }
    }
    if let Err(failure) = stage(&mut hook, ExecBoundaryStage::Inheritance) {
        return failure;
    }
    let inheritance = match PreparedDescriptorInheritance::prepare(plan.descriptors.iter().copied())
    {
        Ok(inheritance) => inheritance,
        // Preparation restores whatever it changed before refusing.
        Err(error) => return ExecBoundaryError::Inheritance(error),
    };

    // ---- The exec call site (Plan 0061 M3.10) ----
    // `execveat(forward_daemon, "", argv, envp, AT_EMPTY_PATH)` of the sealed
    // forward daemon snapshot, compiled ONLY into `integration-test` builds
    // ([`exec_forward_daemon`]). It returns only when exec fails, and that
    // failure takes this same restore-and-resume path. SIGINT, SIGTERM, and
    // SIGHUP are blocked (`cross_with`), so they cross exec pending.
    //
    // HARD GATE (Plan 0061 M3.11b): the cooperative rollback executor exists
    // since M3.11a ([`crate::handoff_rollback`]), so a candidate that fails
    // before its adoption commit re-executes the sealed rollback daemon with
    // the unchanged PTYs instead of exiting. Enabling this arm outside
    // `integration-test` is M3.11b's decision, with its security review, not
    // a flag: there is no runtime switch, and a normal build has no forward
    // exec code at all.
    let failure = match stage(&mut hook, ExecBoundaryStage::Exec) {
        Err(failure) => failure,
        #[cfg(not(feature = "integration-test"))]
        Ok(()) => ExecBoundaryError::ExecNotWired,
        #[cfg(feature = "integration-test")]
        Ok(()) => exec_forward_daemon(plan),
    };

    let restored = stage(&mut hook, ExecBoundaryStage::Restore).and_then(|()| {
        inheritance
            .restore()
            .map_err(ExecBoundaryError::Inheritance)
    });
    match restored {
        Ok(()) => failure,
        // An injected restore fault drops the unrestored inheritance here,
        // which restores best-effort and logs, like a real failure's drop.
        Err(restore) => ExecBoundaryError::Restore {
            cause: Box::new(failure),
            restore: Box::new(restore),
        },
    }
}

/// Executes the forward daemon the plan names, from its sealed snapshot
/// descriptor, and returns only when exec fails.
///
/// Compiled only into `integration-test` builds: enabling exec in a
/// production build is Plan 0061 M3.11b's gate; see the call site in
/// `under_guard`. Called with the handoff signals blocked and the process
/// guard held, after both manifests are sealed and exactly the recorded
/// descriptors were made inheritable; the image descriptor itself stays
/// close-on-exec, which `execveat(AT_EMPTY_PATH)` of an ELF image permits.
#[cfg(feature = "integration-test")]
fn exec_forward_daemon(plan: &ExecInheritancePlan<'_>) -> ExecBoundaryError {
    let Some(forward) = &plan.forward else {
        return ExecBoundaryError::ExecNotWired;
    };
    tracing::info!(
        arguments = forward.arguments.len(),
        inherited = plan.descriptors.len(),
        "daemon handoff: executing the sealed forward daemon"
    );
    match nix::unistd::execveat(
        forward.daemon,
        c"",
        &forward.arguments,
        &forward.environment,
        nix::fcntl::AtFlags::AT_EMPTY_PATH,
    ) {
        Err(errno) => ExecBoundaryError::Exec(errno.into()),
        Ok(never) => match never {},
    }
}

/// ---- The rollback exec call site (Plan 0061 M3.11a) ----
///
/// `execveat(rollback_daemon, "", argv, envp, AT_EMPTY_PATH)` of the sealed
/// rollback daemon snapshot a forward candidate inherited. Compiled into
/// every build: it is the only way back for a forward candidate that failed
/// before its adoption commit, and a production build reaches it only once
/// a forward exec exists there (M3.11b).
///
/// The `_guard` and `_signals` parameters are the proof the call site
/// requires: the process is single-threaded, SIGINT, SIGTERM, and SIGHUP are
/// blocked, and the inheritance (`_inheritance`) cleared close-on-exec on
/// exactly the descriptors the rollback manifest records, at the numbers it
/// records. The image descriptor is among them (the rollback manifest
/// records it, so the rollback daemon can verify its own image), so it is
/// inheritable too; that changes nothing for an ELF image. Returns only
/// when exec fails before the kernel's point of no return; see the module
/// documentation for a failure after it.
#[must_use]
pub(crate) fn exec_rollback_daemon(
    _guard: &HandoffProcessGuard,
    _signals: &BlockedHandoffSignals,
    _inheritance: &PreparedDescriptorInheritance<'_>,
    daemon: BorrowedFd<'_>,
    arguments: &[std::ffi::CString],
    environment: &[std::ffi::CString],
) -> io::Error {
    match nix::unistd::execveat(
        daemon,
        c"",
        arguments,
        environment,
        nix::fcntl::AtFlags::AT_EMPTY_PATH,
    ) {
        Err(errno) => errno.into(),
        Ok(never) => match never {},
    }
}

/// Writes exactly `manifest.bytes` into its empty memfd from offset zero,
/// then adds the immutable seal set the candidate requires.
fn seal_manifest(manifest: ManifestSeal<'_>) -> io::Result<()> {
    let storage = manifest.storage;
    let length = manifest.bytes.len();
    if length == 0 || fstat(storage)?.st_size != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "manifest storage must start empty and the encoding must not be empty",
        ));
    }
    let mut written = 0_usize;
    while written < length {
        let offset = u64::try_from(written).map_err(io::Error::other)?;
        let count = pwrite(storage, &manifest.bytes[written..], offset)?;
        if count == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        written += count;
    }
    fcntl_add_seals(storage, REQUIRED_IMMUTABLE_SEALS)?;
    let sealed = fcntl_get_seals(storage)?;
    if u64::try_from(fstat(storage)?.st_size).ok() != u64::try_from(length).ok()
        || !sealed.contains(REQUIRED_IMMUTABLE_SEALS)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "sealed manifest storage does not hold exactly the encoding",
        ));
    }
    Ok(())
}

/// One handed-off Splint waiting to be served again by the generation that
/// handed it off (Plan 0061 D5).
#[derive(Debug)]
pub struct ResumableSplint {
    pub dojo_id: DojoId,
    pub handed_off: HandedOffSplint,
    pub checkpoint: LinkedCheckpointBody,
    /// The daemon's configuration for this Splint. Resume sets its geometry
    /// from the capture evidence; scrollback and budgets are the caller's.
    pub config: LiveSplintConfig,
}

/// One Splint served again with its original incarnation.
#[derive(Debug)]
pub struct ResumedSplint {
    pub dojo_id: DojoId,
    pub splint_id: SplintId,
    pub incarnation: ProcessIncarnation,
    pub runtime: LiveSplintRuntime,
}

/// What [`resume_handed_off`] did with each Splint. It never fails as a
/// whole; every degradation is listed here.
#[derive(Debug, Default)]
pub struct ResumedFleet {
    /// Committed actors, in input order.
    pub resumed: Vec<ResumedSplint>,
    /// Splints whose checkpoint could not be restored; each resumed with a
    /// blank terminal of its size.
    pub blank_terminals: Vec<SplintId>,
    /// Splints whose actor could not be staged again; see the module
    /// documentation.
    pub stranded: Vec<SplintId>,
    /// Dojos where some scope could not be adopted: every Splint there runs
    /// uncontained, and scopes already adopted there stay retained.
    pub uncontained_dojos: Vec<DojoId>,
}

/// The shared budget authority restored terminals are charged to: the same
/// handles the daemon state owns.
#[derive(Clone, Debug)]
pub struct ResumeBudgets {
    pub image: SharedImageBudget,
    pub kitty_upload: SharedKittyUploadBudget,
}

/// Serves every handed-off Splint again in this process, after the exec
/// boundary returned.
///
/// For each Splint, in order: restore its terminal from its own sealed
/// checkpoint (a blank terminal of the same size when that fails), adopt its
/// retained scope into `adopter` (a fresh manager: each identity is adopted
/// exactly once, into exactly one manager), stage its actor at its original
/// incarnation with [`LiveSplintRuntime::adopt_staged`], and commit it. A
/// child that exited meanwhile is staged unreaped, and its actor reaps it and
/// publishes its exit.
///
/// Scope adoption runs on the blocking pool under `scope_timeout`. A scope
/// that cannot be adopted for a child that exited, or whose scope and slice
/// systemd no longer has, runs uncontained. Any other failure, or the
/// timeout, makes the whole Dojo (or every Dojo) uncontained, with each scope
/// already adopted there retained, so no slice can stop under a Splint.
///
/// Call it on the runtime that will serve: actors register with its I/O
/// driver and start on it.
pub async fn resume_handed_off<A: WorkloadScopeAdopter>(
    splints: Vec<ResumableSplint>,
    adopter: Option<A>,
    budgets: &ResumeBudgets,
    scope_timeout: Duration,
) -> ResumedFleet {
    let mut fleet = ResumedFleet::default();
    let mut pending = Vec::with_capacity(splints.len());
    for splint in splints {
        let ResumableSplint {
            dojo_id,
            handed_off,
            checkpoint,
            mut config,
        } = splint;
        let size = handed_off.evidence.size;
        config.columns = size.columns;
        config.rows = size.rows;
        config.pixel_width = size.pixel_width;
        config.pixel_height = size.pixel_height;
        let mut terminal = restore_terminal(&checkpoint, &config, budgets).unwrap_or_else(|| {
            warn!(splint_id = %handed_off.evidence.splint_id, "handoff checkpoint could not be restored; resuming with a blank terminal");
            fleet.blank_terminals.push(handed_off.evidence.splint_id);
            Terminal::new(
                usize::from(size.columns),
                usize::from(size.rows),
                config.terminal.clone(),
            )
        });
        apply_captured_pixel_geometry(&mut terminal, size);
        drop(checkpoint);
        pending.push((dojo_id, handed_off, terminal, config));
    }

    let identities = pending
        .iter()
        .map(|(dojo_id, handed_off, _, _)| {
            let evidence = handed_off.evidence;
            RetainedWorkloadScope::new(
                *dojo_id,
                evidence.splint_id,
                evidence.incarnation.value(),
                evidence.pty_identity.child_pid(),
            )
        })
        .collect::<Vec<_>>();
    let scopes = match adopter {
        Some(adopter) => adopt_scopes(adopter, identities, scope_timeout).await,
        None => identities
            .iter()
            .map(|_| ResumeScope::Uncontained)
            .collect(),
    };
    let failed_dojos = pending
        .iter()
        .zip(&scopes)
        .filter(|(_, scope)| matches!(scope, ResumeScope::Failed))
        .map(|((dojo_id, ..), _)| *dojo_id)
        .collect::<BTreeSet<_>>();
    fleet.uncontained_dojos = failed_dojos.iter().copied().collect();

    for ((dojo_id, handed_off, terminal, config), scope) in pending.into_iter().zip(scopes) {
        let placement = match scope {
            // Dropping the wrapper retains the scope: this manager keeps its
            // reference and never stops the slice under the Dojo's Splints.
            ResumeScope::Adopted(scope) if failed_dojos.contains(&dojo_id) => {
                drop(scope);
                AdoptedPlacement::Uncontained
            }
            ResumeScope::Adopted(mut scope) => scope
                .take()
                .map_or(AdoptedPlacement::Uncontained, AdoptedPlacement::Scope),
            ResumeScope::Uncontained | ResumeScope::Failed => AdoptedPlacement::Uncontained,
        };
        let evidence = handed_off.evidence;
        match LiveSplintRuntime::adopt_staged(
            evidence.splint_id,
            evidence.incarnation,
            handed_off.session,
            terminal,
            config,
            placement,
        ) {
            Ok(staged) => fleet.resumed.push(ResumedSplint {
                dojo_id,
                splint_id: evidence.splint_id,
                incarnation: evidence.incarnation,
                runtime: staged.commit(),
            }),
            Err(failure) => {
                error!(splint_id = %evidence.splint_id, error = %failure.error, "handed-off Splint could not be served again; its terminal stays open but unread");
                let inputs = failure.inputs;
                inputs.placement.retain_for_handoff();
                // Kept open for the life of the process: closing the master
                // would hang up the child.
                let (_, master) = inputs.session.into_parts();
                let _ = master.into_raw_fd();
                fleet.stranded.push(evidence.splint_id);
            }
        }
    }
    fleet
}

/// One Splint's scope after resume's adoption pass.
enum ResumeScope<S: ProcessPlacement> {
    Adopted(RetainOnDrop<S>),
    /// No scope to adopt and none that could stop under the child.
    Uncontained,
    Failed,
}

async fn adopt_scopes<A: WorkloadScopeAdopter>(
    adopter: A,
    identities: Vec<RetainedWorkloadScope>,
    timeout: Duration,
) -> Vec<ResumeScope<A::Scope>> {
    let count = identities.len();
    // Every adopted scope is wrapped at once, so a timeout that abandons this
    // task, or a panic in it, retains each scope instead of stopping it.
    let adoption = tokio::task::spawn_blocking(move || {
        identities
            .iter()
            .map(|retained| match adopter.adopt_scope(retained) {
                Ok(scope) => ResumeScope::Adopted(RetainOnDrop::new(scope)),
                // Checked after the failure: an exit seen now explains it.
                Err(_) if child_has_exited(retained.child_pid()) => ResumeScope::Uncontained,
                Err(_)
                    if adopter
                        .retained_units_absent(retained)
                        .is_ok_and(|absent| absent) =>
                {
                    ResumeScope::Uncontained
                }
                Err(error) => {
                    warn!(splint_id = %retained.splint_id(), %error, "retained workload scope could not be adopted on resume");
                    ResumeScope::Failed
                }
            })
            .collect::<Vec<_>>()
    });
    match time::timeout(timeout, adoption).await {
        Ok(Ok(scopes)) => scopes,
        Ok(Err(error)) => {
            error!(%error, "workload scope adoption task failed on resume; every Splint runs uncontained");
            (0..count).map(|_| ResumeScope::Failed).collect()
        }
        Err(_) => {
            error!(
                ?timeout,
                "workload scope adoption timed out on resume; every Splint runs uncontained"
            );
            (0..count).map(|_| ResumeScope::Failed).collect()
        }
    }
}

/// Gives a resumed terminal the cell pixel geometry its PTY was captured
/// with, exactly as a spawned actor derives it, so image placement before the
/// next resize uses real cells. A restored checkpoint already carries the
/// actor's geometry, which this re-derives unchanged; a blank fallback
/// terminal starts without one.
fn apply_captured_pixel_geometry(terminal: &mut Terminal, size: splinterm_pty::PtySize) {
    crate::live::set_terminal_pixel_geometry(
        terminal,
        size.columns,
        size.rows,
        size.pixel_width,
        size.pixel_height,
    );
}

/// Restores one terminal from its sealed checkpoint, or `None` when the body
/// is unreadable, invalid, or of another size than the capture recorded.
fn restore_terminal(
    checkpoint: &LinkedCheckpointBody,
    config: &LiveSplintConfig,
    budgets: &ResumeBudgets,
) -> Option<Terminal> {
    let bytes = read_immutable_descriptor(
        checkpoint.body().descriptor(),
        MAX_TERMINAL_CHECKPOINT_BYTES,
    )
    .ok()?;
    let terminal = Terminal::restore_handoff_checkpoint(
        &bytes,
        budgets.image.clone(),
        budgets.kitty_upload.clone(),
    )
    .ok()?;
    let dimensions = terminal
        .snapshot(splinterm_terminal::SnapshotRequest {
            max_scrollback_rows: 0,
        })
        .dimensions();
    (dimensions.columns == usize::from(config.columns)
        && dimensions.rows == usize::from(config.rows))
    .then_some(terminal)
}

/// Reaps `pid` when it is an exited child of this process, without waiting,
/// and returns its exit.
///
/// The resumed generation reaps Splints excluded from capture this way: their
/// actors ended with the stopped runtime, perhaps before reaping. `None` when
/// `pid` is not a child (its actor already reaped it) or has not exited. A
/// zombie keeps its PID, so this cannot reap an unrelated process.
#[must_use]
pub fn reap_exited_child(pid: u32) -> Option<ProcessExit> {
    let pid = i32::try_from(pid).ok().and_then(Pid::from_raw)?;
    match waitid(
        WaitId::Pid(pid),
        WaitIdOptions::EXITED | WaitIdOptions::NOHANG,
    ) {
        Ok(Some(status)) => Some(ProcessExit {
            code: status.exit_status(),
            signal: status.terminating_signal(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use splinterm_pty::PtySize;
    use splinterm_terminal::{Terminal, TerminalConfig};

    use super::{
        BlockedHandoffSignals, apply_captured_pixel_geometry, candidate_daemon_arguments,
        unblock_handoff_signals,
    };

    /// The handoff signals block on this thread, restore exactly on drop,
    /// and unblock for a candidate that installed its handlers.
    #[test]
    fn handoff_signals_block_and_restore_on_the_calling_thread() {
        use nix::sys::signal::{SigSet, SigmaskHow, Signal, pthread_sigmask};

        let mask = || {
            let mut set = SigSet::empty();
            pthread_sigmask(SigmaskHow::SIG_BLOCK, None, Some(&mut set)).unwrap();
            set
        };
        let signals = [Signal::SIGINT, Signal::SIGTERM, Signal::SIGHUP];
        let before = mask();
        let blocked = BlockedHandoffSignals::block().unwrap();
        assert!(signals.iter().all(|signal| mask().contains(*signal)));
        drop(blocked);
        assert_eq!(mask(), before);

        let blocked = BlockedHandoffSignals::block().unwrap();
        unblock_handoff_signals().unwrap();
        assert!(signals.iter().all(|signal| !mask().contains(*signal)));
        drop(blocked);
        assert_eq!(mask(), before);
    }

    /// The forward argv is the activation flag naming the forward manifest,
    /// then only the arguments the old daemon's state implies: never a copy
    /// of its own argv, which may already carry an activation flag.
    #[test]
    fn the_forward_argv_is_rebuilt_from_state() {
        use std::ffi::{CString, OsStr};

        let plain = candidate_daemon_arguments(OsStr::new("splinterd"), 7, false).unwrap();
        assert_eq!(
            plain,
            [
                CString::new("splinterd").unwrap(),
                CString::new("--splinterm-internal-handoff-manifest-fd-v1=7").unwrap(),
            ]
        );
        let contained =
            candidate_daemon_arguments(OsStr::new("/usr/bin/splinterd"), 12, true).unwrap();
        assert_eq!(
            contained,
            [
                CString::new("/usr/bin/splinterd").unwrap(),
                CString::new("--splinterm-internal-handoff-manifest-fd-v1=12").unwrap(),
                CString::new("--require-workload-cgroups").unwrap(),
            ]
        );
        assert!(candidate_daemon_arguments(OsStr::new("bad\0name"), 7, false).is_none());
    }

    fn size(pixel_width: u16, pixel_height: u16) -> PtySize {
        PtySize {
            columns: 80,
            rows: 24,
            pixel_width,
            pixel_height,
        }
    }

    /// A blank fallback terminal for a capture with pixel extents gets the
    /// cell size a spawned actor would derive; zero extents stay unknown.
    #[test]
    fn a_blank_resumed_terminal_takes_the_captured_cell_pixel_size() {
        let mut terminal = Terminal::new(80, 24, TerminalConfig::default());
        assert_eq!(terminal.cell_pixel_size(), None);
        apply_captured_pixel_geometry(&mut terminal, size(800, 480));
        assert_eq!(terminal.cell_pixel_size(), Some((10, 20)));

        let mut unknown = Terminal::new(80, 24, TerminalConfig::default());
        apply_captured_pixel_geometry(&mut unknown, size(0, 0));
        assert_eq!(unknown.cell_pixel_size(), None);
    }

    /// A restored terminal ends with the captured geometry too, whatever its
    /// checkpoint carried.
    #[test]
    fn a_restored_terminal_keeps_the_geometry_its_capture_recorded() {
        let mut terminal = Terminal::new(80, 24, TerminalConfig::default());
        terminal.set_cell_pixel_size(7, 9);
        apply_captured_pixel_geometry(&mut terminal, size(800, 480));
        assert_eq!(terminal.cell_pixel_size(), Some((10, 20)));
    }
}
