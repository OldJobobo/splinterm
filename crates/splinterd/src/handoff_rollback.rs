//! The forward candidate's cooperative rollback executor (Plan 0061 slice
//! M3.11a; Plan 0037 "Failure and rollback", item 5; ADR 0011 "Handoff and
//! rollback boundary").
//!
//! A forward candidate that fails before its adoption commit still owns
//! every descriptor the old generation handed it, at the numbers both sealed
//! manifests record, and every carried child is still its unreaped child.
//! [`execute_rollback`] re-executes the sealed rollback daemon snapshot by
//! descriptor with those unchanged descriptors and the rollback manifest, in
//! the same process. The rollback daemon claims, stages, and adopts them like
//! any candidate and serves the original daemon generation again; the
//! carried Splints keep their processes, PTYs, and incarnations.
//!
//! # What reaches it
//!
//! A [`PreCommitRecovery`] is built only from the values a pre-commit failure
//! produces: a staging failure ([`CandidateStageFailure`]), a refused
//! adoption ([`AdoptionFailure`], which a refused runtime entry or startup
//! also becomes through [`crate::CandidateRuntimeStage::refuse`]), and an
//! aborted staged adoption ([`AdoptionRecovery`] from
//! [`crate::handoff_adoption::StagedAdoption::abort`]).
//!
//! # No rollback after commit
//!
//! [`crate::handoff_adoption::StagedAdoption::commit`] consumes the staged
//! adoption, moves the lifecycle to `Committed`, and closes the rollback
//! daemon snapshot and both manifest descriptors. A committed adoption has
//! no conversion into any of the values above, so no code after commit can
//! build a [`PreCommitRecovery`]; and the executor itself refuses every
//! lifecycle that is not `RollingBack`, which no edge reaches from
//! `Committed` ([`HandoffLifecycle`]). Rollback after the first
//! post-checkpoint PTY read is therefore unrepresentable, not merely unused
//! (Plan 0037 quiescence item 8).
//!
//! # No rollback of a rollback
//!
//! The lifecycle knows the manifest kind. A rollback generation's pre-commit
//! failure takes the `Failed` edge, and the executor refuses it with
//! [`RollbackError::NoRollbackEdge`] before touching anything: the caller
//! exits loudly, and nothing is executed again.
//!
//! # The executor
//!
//! The caller must have stopped every thread it owns first: the Tokio
//! runtime, its blocking pool, and the workload unit manager (shut down
//! before the runtime, so no workload scope can be stopped). A candidate
//! that had released its signals blocks them on its main thread the moment
//! it decides to roll back, before that teardown, and captures what its
//! handlers took only after it, so no termination is lost between the
//! failure and the exec. Then, in order:
//!
//! 1. check the lifecycle is rolling back;
//! 2. take the rollback manifest: the counterpart staging already decoded
//!    and paired, or, when staging itself failed, decoded and paired here
//!    from its inherited sealed memfd;
//! 3. bind every owned descriptor to the slot and number the rollback
//!    manifest records; nothing is renumbered, so a descriptor at another
//!    number than recorded refuses rollback;
//! 4. verify the rollback daemon descriptor is exactly the sealed snapshot
//!    the rollback manifest records and no other role's object, whatever
//!    staging verified before it failed;
//! 5. rebuild argv from parsed state, never from the running argv:
//!    `[argv0, <activation>=<rollback manifest fd>]`, plus
//!    `--require-workload-cgroups` when it was given;
//! 6. hold SIGINT, SIGTERM, and SIGHUP blocked (the block the caller took
//!    when it decided to roll back, or a new one), and make a termination
//!    the failed candidate's handler already took pending again, so it
//!    crosses exec (`crate::handoff_exec` documents the signal discipline);
//! 7. take a fresh single-threaded process guard
//!    ([`HandoffProcessGuard::acquire_until`]);
//! 8. clear close-on-exec on exactly the recorded descriptors, under the
//!    guard. The process-wide audit only reports any other inheritable
//!    descriptor (`PreparedDescriptorInheritance::prepare_reporting`):
//!    the rollback daemon's audited claim closes it, and refusing would end
//!    every Splint over it;
//! 9. `execveat(rollback_daemon, "", argv, envp, AT_EMPTY_PATH)` with the
//!    environment unchanged.
//!
//! # When rollback fails
//!
//! Every refusal and a failed exec return a [`RollbackError`]. There is
//! nothing older to fall back to, so the caller logs it with the original
//! failure and exits non-zero. Exiting closes every carried PTY master,
//! which hangs up every carried Splint; workload scopes are never stopped
//! (adoption retained each one, and the manager was shut down first), and a
//! systemd service reports failed. An exec failure past the kernel's point
//! of no return kills the process with the same outcome and no fall-through.
//!
//! # Failures rollback cannot cure
//!
//! Rollback recovers a candidate whose own image or build failed. It does
//! not recover from a refusal caused by state that both generations read the
//! same way. The rollback daemon runs the same adoption
//! ([`crate::handoff_adoption`]) against the same files, descriptors, and
//! session. So when the forward candidate failed one of the checks below,
//! its rollback almost always fails the same check, takes the `Failed` edge,
//! and exits as described above:
//!
//! * the durable Topology: its revision against the manifest's
//!   (`AdoptionError::TopologyRevision`, for example when the state file was
//!   replaced or another writer saved it after the fence), and each carried
//!   Splint's presence, Lair and Dojo, incarnation, and running state there
//!   (`MissingSplint`, `SplintPlacement`, `IncarnationMismatch`,
//!   `NotRunning`);
//! * loading durable state at all: the metadata store or the Topology file
//!   cannot be found, read, or decoded (`DaemonStartup`);
//! * the install prefix recorded in the manifest no longer resolving to a
//!   valid prefix (`InstallPrefix`). Only a changed forward pair is
//!   tolerated on the rollback side;
//! * an inherited listener that is not a listening Unix stream socket
//!   (`InvalidListener`). The same descriptor crosses both execs;
//! * required workload cgroups without a reachable systemd user manager
//!   (`WorkloadCgroupsUnavailable`), and a running child whose retained
//!   scope systemd refuses to hand over (`WorkloadScope`), for as long as
//!   that session state persists.
//!
//! A transient condition (a single failed systemd call, a failed task, a
//! runtime that could not be built) can succeed on the second attempt. A
//! deterministic one cannot. The Plan 0061 M3.17 proofs and VM acceptance
//! must cover these rows before live handoff is enabled
//! (`WINDOW_CONTINUITY_READY` in the binary's `handoff_requests`).
//!
//! # Compatibility of what the rollback generation trusts
//!
//! The rollback daemon serves the original generation from its sealed
//! snapshot, but it trusts exactly `<prefix>/splinterm` by device and inode
//! for trusted UI (AGENTS.md §4.4), as every daemon does, and launches
//! `<prefix>/splinterm-pty-child`. After a package upgrade both may be the
//! newer installed files: a newer client talks to the older daemon over the
//! negotiated private protocol, which the forward pair's preflight already
//! required both generations to share, and a newer PTY helper must keep the
//! helper interface. Matching the pinned rollback client snapshot instead
//! needs an approved AGENTS.md §4.4 amendment first (Plan 0061 M3.10a
//! review), so rollback does not pin it. Like every adopted daemon, the
//! rollback daemon serves with handoff unavailable until it restarts (Plan
//! 0061 D8).

use std::{
    convert::Infallible,
    ffi::{CString, OsString},
    os::fd::{AsFd, RawFd},
    time::Instant,
};

use nix::sys::signal::Signal;
use rustix::fs::fstat;
use thiserror::Error;
use tracing::{info, warn};

use crate::{
    candidate_bootstrap::{
        CandidateBootstrap, CandidateStageFailure, CandidateStagedAuthorities,
        current_manifest_slot, read_immutable_descriptor, terminal_checkpoint_slot,
        verify_executable,
    },
    handoff_adoption::{AdoptionFailure, AdoptionRecovery},
    handoff_coordinator::{
        HandoffCoordinatorError, HandoffLifecycle, HandoffPhase, HandoffProcessGuard,
    },
    handoff_descriptors::{
        CandidateDescriptorClaims, DescriptorHandoffError, HandoffDescriptor,
        HandoffDescriptorSlot, PreparedDescriptorInheritance,
    },
    handoff_exec::{
        BlockedHandoffSignals, ExecBoundaryError, bind_recorded_descriptors,
        candidate_daemon_arguments, exec_rollback_daemon, replay_blocked_signal,
    },
    handoff_manifest::{
        HandoffManifestV1, MAX_HANDOFF_MANIFEST_BYTES, ManifestKind, decode_manifest,
        validate_manifest_pair,
    },
};

/// Everything a candidate still owns when it fails before its adoption
/// commit; see the module documentation for what builds it.
#[must_use = "a pre-commit recovery owns every carried descriptor; hand it to the rollback executor"]
pub struct PreCommitRecovery {
    recovered: Recovered,
}

enum Recovered {
    /// Staging refused: the complete claimed descriptor table.
    Unstaged(Box<CandidateBootstrap>),
    /// Adoption refused or was aborted: every staged part, each adopted
    /// scope retained.
    Adoption(Box<AdoptionRecovery>),
}

impl std::fmt::Debug for PreCommitRecovery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreCommitRecovery")
            .field("rollback_edge", &self.has_rollback_edge())
            .field("carried_splints", &self.carried_splints())
            .field("retained_scopes", &self.retained_scopes())
            .finish()
    }
}

impl From<CandidateStageFailure> for PreCommitRecovery {
    fn from(failure: CandidateStageFailure) -> Self {
        Self {
            recovered: Recovered::Unstaged(Box::new(failure.into_recovery())),
        }
    }
}

impl From<AdoptionFailure> for PreCommitRecovery {
    fn from(failure: AdoptionFailure) -> Self {
        failure.into_recovery().into()
    }
}

impl From<AdoptionRecovery> for PreCommitRecovery {
    fn from(recovery: AdoptionRecovery) -> Self {
        Self {
            recovered: Recovered::Adoption(Box::new(recovery)),
        }
    }
}

impl PreCommitRecovery {
    /// Carried Splints whose descriptors this recovery holds; `None` when
    /// staging failed before any was verified.
    #[must_use]
    pub fn carried_splints(&self) -> Option<usize> {
        match &self.recovered {
            Recovered::Unstaged(_) => None,
            Recovered::Adoption(recovery) => Some(recovery.splint_count()),
        }
    }

    /// Scopes the failed attempt adopted and retained again.
    #[must_use]
    pub fn retained_scopes(&self) -> usize {
        match &self.recovered {
            Recovered::Unstaged(_) => 0,
            Recovered::Adoption(recovery) => recovery.retained_scopes().len(),
        }
    }

    /// Whether this failure may roll back: false for a rollback generation,
    /// which has no rollback edge.
    #[must_use]
    pub fn has_rollback_edge(&self) -> bool {
        let phase = match &self.recovered {
            Recovered::Unstaged(bootstrap) => bootstrap.lifecycle_phase(),
            Recovered::Adoption(recovery) => recovery.lifecycle_phase(),
        };
        phase == HandoffPhase::RollingBack
    }
}

/// Termination requests the failed candidate's own handlers already took
/// from the kernel, captured once no runtime thread is left and the signals
/// are blocked, which the executor makes pending again under that block so
/// the rollback daemon acts on them after it adopts.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PendingTermination {
    /// A SIGTERM arrived.
    pub terminate: bool,
    /// A SIGINT arrived.
    pub interrupt: bool,
}

/// What the rollback exec needs besides the recovered descriptors.
pub struct RollbackExec {
    /// `argv[0]` for the rollback daemon: the running process's own.
    pub program: OsString,
    /// Whether the running daemon was started with
    /// `--require-workload-cgroups`, from its parsed arguments.
    pub workload_cgroups_required: bool,
    /// The environment, unchanged ([`crate::handoff_exec::exec_environment`]).
    pub environment: Vec<CString>,
    /// When to give up waiting for stopped threads to leave the process.
    pub guard_deadline: Instant,
    /// Terminations to carry across exec.
    pub pending: PendingTermination,
}

impl std::fmt::Debug for RollbackExec {
    // The environment may hold credentials; only its size is diagnostic.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RollbackExec")
            .field("program", &self.program)
            .field("workload_cgroups_required", &self.workload_cgroups_required)
            .field("environment_entries", &self.environment.len())
            .field("pending", &self.pending)
            .finish_non_exhaustive()
    }
}

/// Why the candidate could not roll back. Every one ends the candidate.
#[derive(Debug, Error)]
pub enum RollbackError {
    #[error(
        "this daemon is itself a rollback generation, so a failure before its commit has no rollback edge"
    )]
    NoRollbackEdge,
    #[error("the recovered handoff lifecycle is {0}, not rolling back")]
    UnexpectedPhase(String),
    #[error("the inherited rollback manifest is unusable: {0}")]
    RollbackManifest(String),
    #[error("the rollback manifest does not record {0:?}")]
    MissingRecord(HandoffDescriptorSlot),
    #[error("the recovered descriptors are not the rollback manifest's: {0}")]
    Descriptors(#[source] ExecBoundaryError),
    #[error(
        "the inherited rollback daemon image is not the one the rollback manifest records: {0}"
    )]
    RollbackImage(String),
    #[error("cannot build the rollback daemon's arguments")]
    Arguments,
    #[error("cannot block the handoff signals: {0}")]
    SignalMask(#[source] std::io::Error),
    #[error("the process did not reach one thread: {0}")]
    Guard(#[source] HandoffCoordinatorError),
    #[error("cannot prepare descriptor inheritance: {0}")]
    Inheritance(#[source] DescriptorHandoffError),
    #[error("exec of the rollback daemon failed: {0}")]
    Exec(#[source] std::io::Error),
}

/// Re-executes the sealed rollback daemon with the recovered descriptors;
/// see the module documentation. Returns only when rollback is impossible,
/// with why; the caller must then end the process loudly.
///
/// `blocked` is the block a caller took on this thread when it decided to
/// roll back, after its handlers had been released, and that it held
/// through teardown: the executor keeps it to the exec, so no termination
/// can be taken in between. Without one (the signals were never released,
/// so the blocked mask the forward exec carried still holds them), the
/// executor blocks itself before the replay.
#[must_use = "the executor returns only to say why rollback failed"]
pub fn execute_rollback(
    recovery: PreCommitRecovery,
    exec: &RollbackExec,
    blocked: Option<BlockedHandoffSignals>,
) -> RollbackError {
    match roll_back(recovery, exec, blocked) {
        Err(error) => error,
        Ok(never) => match never {},
    }
}

/// The owned parts the executor binds, out of their recovery wrappers.
enum RollbackParts {
    Unstaged {
        manifest: Box<HandoffManifestV1>,
        claims: CandidateDescriptorClaims,
    },
    Adoption(Box<CandidateStagedAuthorities>),
}

fn roll_back(
    recovery: PreCommitRecovery,
    exec: &RollbackExec,
    blocked: Option<BlockedHandoffSignals>,
) -> Result<Infallible, RollbackError> {
    let parts = RollbackParts::rolling_back(recovery)?;
    let rollback_manifest = parts.rollback_manifest()?;
    let recorded = rollback_manifest
        .descriptors
        .iter()
        .map(|entry| {
            RawFd::try_from(entry.inherited_fd)
                .map(|fd| (entry.slot, fd))
                .map_err(|_| {
                    RollbackError::RollbackManifest("descriptor number out of range".into())
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let owners = parts.owners()?;
    let descriptors =
        bind_recorded_descriptors(&recorded, &owners).map_err(RollbackError::Descriptors)?;
    let recorded_fd = |slot| {
        descriptors
            .iter()
            .find(|descriptor| descriptor.slot() == slot)
            .copied()
            .ok_or(RollbackError::MissingRecord(slot))
    };
    let manifest = recorded_fd(HandoffDescriptorSlot::RollbackManifest)?;
    let daemon = recorded_fd(HandoffDescriptorSlot::RollbackDaemon)?;
    verify_rollback_image(daemon, &descriptors, &rollback_manifest)?;
    let arguments = candidate_daemon_arguments(
        &exec.program,
        manifest.raw_fd(),
        exec.workload_cgroups_required,
    )
    .ok_or(RollbackError::Arguments)?;

    let signals = match blocked {
        Some(blocked) => blocked,
        None => BlockedHandoffSignals::block().map_err(RollbackError::SignalMask)?,
    };
    for (pending, signal) in [
        (exec.pending.terminate, Signal::SIGTERM),
        (exec.pending.interrupt, Signal::SIGINT),
    ] {
        if pending && let Err(error) = replay_blocked_signal(&signals, signal) {
            // Rollback still happens; only the request is lost.
            warn!(%error, ?signal, "cannot carry a termination request across the rollback exec");
        }
    }
    let guard =
        HandoffProcessGuard::acquire_until(exec.guard_deadline).map_err(RollbackError::Guard)?;
    let (inheritance, strays) =
        PreparedDescriptorInheritance::prepare_reporting(descriptors.iter().copied())
            .map_err(RollbackError::Inheritance)?;
    if !strays.is_empty() {
        // The rollback daemon's own claim closes each of these and reports
        // them as `closed_unexpected`; refusing here would end every Splint.
        warn!(
            strays = strays.len(),
            descriptors = ?strays,
            "daemon handoff: unexpected inheritable descriptors cross the rollback exec; the rollback daemon closes them"
        );
    }
    info!(
        handoff_id = ?rollback_manifest.handoff_id,
        target_daemon_generation = rollback_manifest.target_daemon_generation,
        inherited = descriptors.len(),
        carried = rollback_manifest.splints.len(),
        manifest = manifest.raw_fd(),
        "daemon handoff: executing the sealed rollback daemon"
    );
    let error = exec_rollback_daemon(
        &guard,
        &signals,
        &inheritance,
        daemon.descriptor(),
        &arguments,
        &exec.environment,
    );
    // Restore close-on-exec, then release the guard, then the mask; the
    // caller ends the process.
    drop(inheritance);
    drop(guard);
    drop(signals);
    Err(RollbackError::Exec(error))
}

impl RollbackParts {
    /// Unwraps a recovery whose lifecycle is rolling back; a rollback
    /// generation's (failed) has no rollback edge.
    fn rolling_back(recovery: PreCommitRecovery) -> Result<Self, RollbackError> {
        let (lifecycle, parts) = match recovery.recovered {
            Recovered::Unstaged(bootstrap) => {
                let (manifest, claims, lifecycle) = bootstrap.into_rollback_parts();
                (
                    lifecycle,
                    Self::Unstaged {
                        manifest: Box::new(manifest),
                        claims,
                    },
                )
            }
            Recovered::Adoption(recovery) => {
                let (lifecycle, authorities) = recovery.into_rollback_parts();
                (lifecycle, Self::Adoption(Box::new(authorities)))
            }
        };
        require_rolling_back(&lifecycle)?;
        if parts.current_manifest().kind != ManifestKind::Forward {
            return Err(RollbackError::NoRollbackEdge);
        }
        Ok(parts)
    }

    fn current_manifest(&self) -> &HandoffManifestV1 {
        match self {
            Self::Unstaged { manifest, .. } => manifest,
            Self::Adoption(authorities) => &authorities.manifest,
        }
    }

    /// The rollback manifest, paired with the forward manifest this
    /// candidate was activated with. Staging reread and paired it already
    /// (`verify_manifests`) unless staging is what failed; then it is read
    /// from its sealed memfd and paired here.
    fn rollback_manifest(&self) -> Result<HandoffManifestV1, RollbackError> {
        let unusable =
            |error: &dyn std::fmt::Display| RollbackError::RollbackManifest(error.to_string());
        match self {
            Self::Adoption(authorities) => Ok(authorities.counterpart_manifest.clone()),
            Self::Unstaged { manifest, claims } => {
                let slot = manifest.counterpart_manifest_slot;
                let descriptor = claims
                    .descriptor(slot)
                    .ok_or(RollbackError::MissingRecord(slot))?;
                let bytes = read_immutable_descriptor(descriptor, MAX_HANDOFF_MANIFEST_BYTES)
                    .map_err(|error| unusable(&error))?;
                let rollback = decode_manifest(&bytes).map_err(|error| unusable(&error))?;
                validate_manifest_pair(manifest, &rollback).map_err(|error| unusable(&error))?;
                Ok(rollback)
            }
        }
    }

    /// Every owned descriptor, bound to its slot.
    fn owners(&self) -> Result<Vec<HandoffDescriptor<'_>>, RollbackError> {
        match self {
            Self::Unstaged { claims, .. } => Ok(claims.handoff_descriptors().collect()),
            Self::Adoption(authorities) => staged_owners(authorities),
        }
    }
}

/// Requires the descriptor the executor is about to run to be exactly the
/// sealed rollback daemon snapshot the rollback manifest records, whatever
/// staging got to verify before it failed: a sealed regular file with the
/// recorded seal set, device, inode, size, and SHA-256, close-on-exec, that
/// refuses a writable shared mapping (`verify_executable`, the check staging
/// itself uses), and no other recorded role's object.
///
/// A staging failure can come before staging's executable pass (descriptor
/// roles, manifests, listener paths, compiled generation) or from that pass
/// itself, so in that case nothing else has authenticated this image.
fn verify_rollback_image(
    daemon: HandoffDescriptor<'_>,
    descriptors: &[HandoffDescriptor<'_>],
    rollback_manifest: &HandoffManifestV1,
) -> Result<(), RollbackError> {
    let refused = |error: &dyn std::fmt::Display| RollbackError::RollbackImage(error.to_string());
    verify_executable(
        daemon.descriptor(),
        &rollback_manifest.executable_pairs.rollback.daemon,
    )
    .map_err(|error| refused(&error))?;
    let image = fstat(daemon.descriptor()).map_err(|error| refused(&error))?;
    for other in descriptors {
        if other.slot() == daemon.slot() {
            continue;
        }
        let object = fstat(other.descriptor()).map_err(|error| refused(&error))?;
        if (object.st_dev, object.st_ino) == (image.st_dev, image.st_ino) {
            return Err(refused(&format_args!(
                "it is also inherited as {:?}",
                other.slot()
            )));
        }
    }
    Ok(())
}

fn require_rolling_back(lifecycle: &HandoffLifecycle) -> Result<(), RollbackError> {
    match lifecycle.phase() {
        HandoffPhase::RollingBack => Ok(()),
        HandoffPhase::Failed => Err(RollbackError::NoRollbackEdge),
        phase => Err(RollbackError::UnexpectedPhase(format!("{phase:?}"))),
    }
}

fn staged_owners(
    authorities: &CandidateStagedAuthorities,
) -> Result<Vec<HandoffDescriptor<'_>>, RollbackError> {
    let manifest = &authorities.manifest;
    let owner = HandoffDescriptor::new;
    let mut owners = vec![
        owner(
            HandoffDescriptorSlot::Listener,
            authorities.listener.as_fd(),
        ),
        owner(
            HandoffDescriptorSlot::ImageListener,
            authorities.image_listener.as_fd(),
        ),
        owner(
            current_manifest_slot(manifest.kind),
            authorities.current_manifest_descriptor.as_fd(),
        ),
        owner(
            manifest.counterpart_manifest_slot,
            authorities.counterpart_manifest_descriptor.as_fd(),
        ),
        owner(
            HandoffDescriptorSlot::ForwardClient,
            authorities.forward_client.as_fd(),
        ),
        owner(
            HandoffDescriptorSlot::RollbackDaemon,
            authorities.rollback_daemon.as_fd(),
        ),
        owner(
            HandoffDescriptorSlot::RollbackClient,
            authorities.rollback_client.as_fd(),
        ),
    ];
    for splint in &authorities.splints {
        owners.push(owner(
            HandoffDescriptorSlot::PtyMaster(splint.evidence.pty.master_slot),
            splint.pty.master(),
        ));
        let checkpoint = terminal_checkpoint_slot(&splint.evidence)
            .map_err(|error| RollbackError::RollbackManifest(error.to_string()))?;
        owners.push(owner(checkpoint, splint.checkpoint_descriptor.as_fd()));
    }
    Ok(owners)
}
