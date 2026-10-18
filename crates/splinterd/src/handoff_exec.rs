//! The old generation's exec boundary and its in-process resume (Plan 0061
//! slice M3.9; Plan 0037 "Descriptor discipline" and "Failure and rollback").
//!
//! The binary reaches this module only after serve has left with a fenced,
//! captured attempt, every captured actor has ended for handoff, and the Tokio
//! runtime and the systemd connection are gone. [`cross_exec_boundary`] then
//! runs the single-threaded critical section:
//!
//! 1. take the process guard ([`HandoffProcessGuard::acquire_until`]), which
//!    proves the process has exactly one thread;
//! 2. write exactly the encoded manifest bytes into both empty manifest memfds
//!    and seal them;
//! 3. clear close-on-exec on exactly the descriptors the sealed manifests
//!    record, at exactly the numbers they record
//!    ([`PreparedDescriptorInheritance::prepare`]); nothing is renumbered
//!    once a manifest is sealed;
//! 4. exec the forward daemon, at the one call site M3.10 fills;
//! 5. when exec does not happen, restore close-on-exec and release the guard.
//!
//! The forward candidate still ends in `reject_until_runtime_adoption`, so
//! step 4 is deliberately not wired: execing now would end every Splint. The
//! boundary therefore always returns, and the old generation resumes in
//! process (Plan 0061 D5) with [`resume_handed_off`]: every Splint keeps
//! running and every client reconnects to the same daemon generation.
//!
//! # No kill
//!
//! Nothing here signals a process or stops a workload scope. A resumed Splint
//! whose scope cannot be adopted runs uncontained, and every scope already
//! adopted in its Dojo stays retained in the new manager, so that manager
//! never stops the Dojo slice under it (the M3.8a sibling-kill rule). A
//! Splint whose actor cannot be staged again keeps its PTY master open for
//! the life of the process: it stalls once its output fills the kernel
//! buffer, but it is never hung up.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    os::fd::{AsRawFd, BorrowedFd, IntoRawFd, RawFd},
    time::{Duration, Instant},
};

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
        })
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
    #[error("exec of the forward daemon is not wired yet (Plan 0061 M3.10)")]
    ExecNotWired,
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
/// See the module documentation for the steps. A successful exec (M3.10)
/// will never return; today the boundary always returns, at the latest with
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
    let guard = match stage(&mut hook, ExecBoundaryStage::Guard).and_then(|()| {
        HandoffProcessGuard::acquire_until(guard_deadline).map_err(ExecBoundaryError::Guard)
    }) {
        Ok(guard) => guard,
        Err(failure) => return failure,
    };
    let failure = under_guard(&guard, plan, hook);
    drop(guard);
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
    // M3.10 replaces the `Ok(())` arm with `execveat` of the sealed forward
    // daemon (`AT_EMPTY_PATH`, argv and `SPLINTERM_POLICY` preserved), which
    // returns only when exec fails; that failure takes this same
    // restore-and-resume path. Until the candidate adopts instead of
    // rejecting, execing would end every Splint, so nothing is executed.
    let failure = match stage(&mut hook, ExecBoundaryStage::Exec) {
        Err(failure) => failure,
        Ok(()) => ExecBoundaryError::ExecNotWired,
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

    use super::apply_captured_pixel_geometry;

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
