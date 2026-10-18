//! Old-side fenced capture and manifest construction for one daemon handoff
//! (Plan 0061 slice M3.7).
//!
//! Once the handoff gate holds the fence, [`capture_live_splints`] captures
//! every live Splint concurrently through its actor's linearization point
//! ([`LiveSplintHandle::prepare_pty_handoff_within`]), each attempt under
//! [`HANDOFF_CAPTURE_ACTOR_TIMEOUT`] and all checkpoints together under
//! [`MAX_HANDOFF_CHECKPOINT_AGGREGATE_BYTES`]. [`build_handoff_manifests`]
//! then binds those captures, the fenced Topology, and the attempt's
//! executable authority into one validated forward/rollback manifest pair.
//!
//! Nothing here clears close-on-exec, prepares inheritance, seals a manifest,
//! or stops the runtime; [`crate::handoff_exec`] does that (M3.9). A capture
//! is a lease: dropping [`HandoffCaptures`], on any path, only resumes each
//! actor. It never ends an actor, signals a child, or releases a placement.
//! Only [`HandoffCaptures::hand_off`] ends the actors, and it ends them for
//! handoff: no child is signalled and every placement is retained.
//!
//! Agent status is connection-owned and dropped across a handoff (Plan 0061
//! D6), so no manifest carries it.

use std::{
    future::Future,
    io,
    os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd, RawFd},
    sync::Arc,
    time::Duration,
};

use rustix::{
    fs::{MemfdFlags, memfd_create},
    rand::{GetRandomFlags, getrandom},
};
use splinterm_core::{DojoId, LairId, SplintId, Topology};
use splinterm_protocol::HandoffCancelReason;
use splinterm_terminal::MAX_TERMINAL_CHECKPOINT_BYTES;
use thiserror::Error;
use tokio::{
    task::{JoinError, JoinSet},
    time,
};
use tracing::warn;

use crate::{
    ActorHandoffEvidence, HandedOffSplint, HandoffCheckpointBudget, LiveError, LiveSplintHandle,
    MAX_LIVE_SPLINTS, PreparedPtyHandoff,
    executable_snapshot::HandoffExecutableSnapshots,
    handoff_checkpoint::{CheckpointStorageError, LinkedCheckpointBody, SealedCheckpointBody},
    handoff_compatibility::{NegotiatedHandoff, RunningHandoffGeneration},
    handoff_coordinator::{ForwardHandoffAuthority, RollbackHandoffAuthority},
    handoff_descriptors::{HandoffDescriptorSlot, MAX_HANDOFF_DESCRIPTORS},
    handoff_manifest::{
        ControllerDisposition, ExecutablePairSetEvidence, ExitDisposition, GenerationEvidence,
        HANDOFF_MANIFEST_SCHEMA_V1, HandoffId, HandoffManifestV1, ManifestDescriptor,
        ManifestError, ManifestKind, ManifestPtyIdentity, ManifestPtySize, ManifestSplint,
        SubscriptionDisposition, decode_manifest, encode_manifest, validate_manifest_pair,
    },
    handoff_preflight::SealedPreflightReports,
    install_prefix::InstallPrefix,
    live::SYNCHRONIZED_FRAME_INTERVAL,
    package_provenance::ValidatedHandoffPackageProvenance,
};

/// How long one actor may take to reach its handoff boundary, per attempt.
///
/// An actor stops reading its PTY the moment it accepts the capture command,
/// and a captured actor stays unread until exec or cancellation, so this
/// bounds Plan 0037's pause with no PTY reader during capture. Meanwhile the
/// kernel's PTY buffers hold the child's output and then block its writes:
/// nothing is lost, but an interactive child visibly stalls.
///
/// Reaching the boundary means writing accepted input and terminal replies
/// to the PTY, then encoding, validating, sealing, and hashing the
/// checkpoint. Every actor captures at once on the daemon's two runtime
/// workers, so this window must also cover that work for all checkpoints
/// together, up to [`MAX_HANDOFF_CHECKPOINT_AGGREGATE_BYTES`]. An actor still
/// short of its boundary after two seconds is wedged, usually on a child that
/// stopped reading its input, and the handoff yields rather than stretch the
/// pause. With the one retry and [`HANDOFF_CAPTURE_RETRY_DELAY`] before it,
/// capture adds at most about four seconds to the fenced pause.
pub const HANDOFF_CAPTURE_ACTOR_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a refused capture waits before its one retry: one and a half
/// synchronized-output frame intervals.
///
/// A refusal returns the actor to normal reading, so the wait adds no time
/// without a PTY reader. An actor refuses while a completed synchronized
/// frame is held back for publication, and the live runtime publishes that
/// frame within one frame interval. Busy synchronized-output programs (full
/// screen TUIs) are often between such frames, and an immediate retry would
/// find the same held frame.
pub const HANDOFF_CAPTURE_RETRY_DELAY: Duration =
    match SYNCHRONIZED_FRAME_INTERVAL.saturating_mul(3).checked_div(2) {
        Some(delay) => delay,
        None => SYNCHRONIZED_FRAME_INTERVAL,
    };
// The retry must outwait one held frame.
const _: () =
    assert!(HANDOFF_CAPTURE_RETRY_DELAY.as_nanos() > SYNCHRONIZED_FRAME_INTERVAL.as_nanos());

/// Upper bound on the sealed terminal checkpoint bytes one handoff's captures
/// hold at any moment (Plan 0061 operator decision D14).
///
/// Each checkpoint may reach [`MAX_TERMINAL_CHECKPOINT_BYTES`] (about 112 MiB)
/// and the registry holds up to [`MAX_LIVE_SPLINTS`] Splints, so without an
/// aggregate bound one handoff could seal about 28 GiB. Sealed checkpoints are
/// memfd pages charged to the daemon's memory cgroup and the user's memory;
/// they stay resident beside the live terminals they copy until the forward
/// daemon adopts them, and that daemon then restores terminals beside them.
/// 256 MiB matches the daemon-wide terminal publication budget: the handoff
/// never adds more sealed memory than the daemon already budgets for
/// publishing. It carries two maximal Splints, or every Splint of a full
/// registry at 1 MiB each.
///
/// What it bounds exactly: every actor reserves its encoded checkpoint's size
/// against one shared [`HandoffCheckpointBudget`] after encoding and before
/// sealing, and refuses once a reservation would pass the cap, which cancels
/// the attempt as `Failed`. A reservation lasts as long as the sealed body's
/// lease, so the sealed memfds of all captures together never pass the cap,
/// however many actors capture at once. What it does not bound: each actor's
/// transient encode buffer, up to [`MAX_TERMINAL_CHECKPOINT_BYTES`], which
/// exists before the reservation. An actor encodes, seals, and frees that
/// buffer within one poll of its task, so at most one buffer exists per
/// runtime worker thread (two in the daemon).
pub const MAX_HANDOFF_CHECKPOINT_AGGREGATE_BYTES: u64 = 256 * 1024 * 1024;

// One maximal Splint can always hand off on its own.
const _: () =
    assert!(MAX_HANDOFF_CHECKPOINT_AGGREGATE_BYTES >= MAX_TERMINAL_CHECKPOINT_BYTES as u64);

/// Descriptor slots every manifest names besides those of its Splints.
const GLOBAL_DESCRIPTOR_SLOTS: usize = 7;
// Each captured Splint adds its PTY master and its checkpoint body.
const _: () = assert!(GLOBAL_DESCRIPTOR_SLOTS + 2 * MAX_LIVE_SPLINTS <= MAX_HANDOFF_DESCRIPTORS);

/// Bound on the panic message a failed capture task reports.
const MAX_TASK_FAILURE_CHARS: usize = 256;

/// The capture bounds. Daemons use [`Self::DEFAULT`]; tests narrow them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaptureLimits {
    pub actor_timeout: Duration,
    pub aggregate_checkpoint_bytes: u64,
}

impl CaptureLimits {
    pub const DEFAULT: Self = Self {
        actor_timeout: HANDOFF_CAPTURE_ACTOR_TIMEOUT,
        aggregate_checkpoint_bytes: MAX_HANDOFF_CHECKPOINT_AGGREGATE_BYTES,
    };
}

impl Default for CaptureLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Why a fenced capture cancelled the attempt. Every acquired lease was
/// released before this is returned.
#[derive(Debug, Error)]
pub enum CaptureFailure {
    #[error("Splint {splint_id} did not reach its handoff boundary within {timeout:?}")]
    TimedOut {
        splint_id: SplintId,
        timeout: Duration,
    },
    #[error("Splint {splint_id} cannot be captured for handoff: {error}")]
    Failed {
        splint_id: SplintId,
        #[source]
        error: Box<LiveError>,
    },
    #[error(
        "Splint {splint_id}'s checkpoint would pass the {cap}-byte aggregate handoff checkpoint cap"
    )]
    AggregateCapExceeded { splint_id: SplintId, cap: u64 },
    #[error("{0} live Splints exceed the handoff capture bound")]
    TooManySplints(usize),
    #[error("a handoff capture task failed: {0}")]
    TaskFailed(String),
}

impl CaptureFailure {
    /// The reason the gate cancels with: `TimedOut` for a missed deadline and
    /// `Failed` for everything else.
    #[must_use]
    pub const fn cancel_reason(&self) -> HandoffCancelReason {
        match self {
            Self::TimedOut { .. } => HandoffCancelReason::TimedOut,
            Self::Failed { .. }
            | Self::AggregateCapExceeded { .. }
            | Self::TooManySplints(_)
            | Self::TaskFailed(_) => HandoffCancelReason::Failed,
        }
    }

    fn failed(splint_id: SplintId, error: LiveError) -> Self {
        Self::Failed {
            splint_id,
            error: Box::new(error),
        }
    }

    /// A capture task that panicked or was cancelled, with at most
    /// [`MAX_TASK_FAILURE_CHARS`] characters of its panic message.
    fn task_failed(error: JoinError) -> Self {
        let message = match error.try_into_panic() {
            Ok(payload) => payload
                .downcast_ref::<&str>()
                .map(|message| (*message).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panicked with a non-string payload".to_owned()),
            Err(error) => error.to_string(),
        };
        Self::TaskFailed(message.chars().take(MAX_TASK_FAILURE_CHARS).collect())
    }
}

/// One captured Splint: its actor's lease and its sealed checkpoint, linked
/// to the slot index its PTY master and checkpoint share in the manifest.
#[derive(Debug)]
pub struct CapturedSplint {
    lease: PreparedPtyHandoff,
    checkpoint: LinkedCheckpointBody,
}

impl CapturedSplint {
    #[must_use]
    pub const fn evidence(&self) -> ActorHandoffEvidence {
        self.lease.evidence()
    }

    #[must_use]
    pub const fn slot(&self) -> u16 {
        self.checkpoint.slot()
    }

    #[must_use]
    pub const fn lease(&self) -> &PreparedPtyHandoff {
        &self.lease
    }

    #[must_use]
    pub const fn checkpoint(&self) -> &LinkedCheckpointBody {
        &self.checkpoint
    }
}

/// Every live Splint captured at the fence, in ascending slot order.
///
/// Dropping it resumes every actor without waiting; [`Self::release`] waits
/// until each actor reads again.
#[derive(Debug)]
pub struct HandoffCaptures {
    captured: Vec<CapturedSplint>,
    excluded: Vec<SplintId>,
    checkpoint_bytes: u64,
    release_timeout: Duration,
}

impl HandoffCaptures {
    #[must_use]
    pub fn captured(&self) -> &[CapturedSplint] {
        &self.captured
    }

    /// Registered Splints whose process had exited, so the handoff carries
    /// them only as their persisted Topology state.
    #[must_use]
    pub fn excluded(&self) -> &[SplintId] {
        &self.excluded
    }

    #[must_use]
    pub const fn checkpoint_bytes(&self) -> u64 {
        self.checkpoint_bytes
    }

    /// Ends every captured actor for handoff and returns each Splint's
    /// canonical PTY master, capture evidence and linked checkpoint, in slot
    /// order, with the excluded Splints.
    ///
    /// Each actor leaves through its handoff path
    /// ([`PreparedPtyHandoff::into_handed_off`]): it neither signals nor
    /// reaps its child and retains its placement, so no workload scope stops.
    /// The caller awaits each runtime's [`LiveError::HandedOff`] before it
    /// stops the Tokio runtime. Nothing here can fail.
    #[must_use]
    pub fn hand_off(self) -> (Vec<HandedOffCapture>, Vec<SplintId>) {
        let handed_off = self
            .captured
            .into_iter()
            .map(|CapturedSplint { lease, checkpoint }| HandedOffCapture {
                handed_off: lease.into_handed_off(),
                checkpoint,
            })
            .collect();
        (handed_off, self.excluded)
    }

    /// Resumes every captured actor and waits, within the capture's per-actor
    /// deadline, until each has its reader back. An actor that does not
    /// acknowledge in time still resumes: its lease sent the resume, or sends
    /// it when the abandoned wait drops the lease.
    pub async fn release(self) {
        let mut resumes = JoinSet::new();
        for CapturedSplint { lease, checkpoint } in self.captured {
            drop(checkpoint);
            let splint_id = lease.evidence().splint_id;
            resumes.spawn(async move { (splint_id, lease.resume().await) });
        }
        let acknowledged = time::timeout(self.release_timeout, async {
            while let Some(joined) = resumes.join_next().await {
                match joined {
                    Ok((_, Ok(()))) => {}
                    Ok((splint_id, Err(error))) => {
                        warn!(%splint_id, %error, "captured Splint did not resume after handoff cancellation");
                    }
                    Err(error) => warn!(%error, "handoff resume task failed"),
                }
            }
        })
        .await;
        if acknowledged.is_err() {
            warn!("captured Splints did not acknowledge their resume in time");
        }
    }

    fn link(
        mut captured: Vec<(PreparedPtyHandoff, SealedCheckpointBody)>,
        excluded: Vec<SplintId>,
        checkpoint_bytes: u64,
        release_timeout: Duration,
    ) -> Result<Self, CaptureFailure> {
        let count = captured.len();
        captured.sort_by_cached_key(|(lease, _)| lease.evidence().splint_id.to_string());
        let mut linked = Vec::with_capacity(count);
        for (index, (lease, body)) in captured.into_iter().enumerate() {
            let slot = u16::try_from(index).map_err(|_| CaptureFailure::TooManySplints(count))?;
            let checkpoint = LinkedCheckpointBody::new(slot, body).map_err(|error| {
                CaptureFailure::failed(lease.evidence().splint_id, error.into())
            })?;
            linked.push(CapturedSplint { lease, checkpoint });
        }
        Ok(Self {
            captured: linked,
            excluded,
            checkpoint_bytes,
            release_timeout,
        })
    }
}

/// One captured Splint whose actor ended for handoff: the canonical PTY
/// master and evidence, and the sealed checkpoint linked to the slot the
/// manifest names for both.
///
/// Dropping it closes the master, which hangs up the child's terminal, so the
/// exec boundary keeps it until exec or resume.
#[derive(Debug)]
pub struct HandedOffCapture {
    pub handed_off: HandedOffSplint,
    pub checkpoint: LinkedCheckpointBody,
}

impl HandedOffCapture {
    /// The slot this Splint's PTY master and checkpoint share in the manifest.
    #[must_use]
    pub const fn slot(&self) -> u16 {
        self.checkpoint.slot()
    }
}

/// How one actor's capture ended.
enum ActorCapture {
    Captured(Box<PreparedPtyHandoff>),
    /// The Splint's process exited during capture; it is excluded.
    Exited(SplintId),
}

/// Captures every live Splint concurrently, each through `prepare`.
///
/// `prepare` receives the capture's shared checkpoint budget. Daemons pass
/// [`LiveSplintHandle::prepare_pty_handoff_within`] with it; the parameter
/// lets tests interpose on one actor's capture.
///
/// A Splint whose process has already exited is excluded without an attempt.
/// Each attempt runs under `limits.actor_timeout`. A refusal that an exit or
/// EOF during capture can produce (the actor's unrepresentable-state refusal,
/// which does not name the state, an observed exit, a failed kernel query on
/// a hung-up terminal, or an ended actor) is retried once, after
/// [`HANDOFF_CAPTURE_RETRY_DELAY`]. If the retry is refused too, a Splint
/// whose process has exited is excluded and any other cancels the capture.
/// Exited Splints are excluded, never carried as exited: manifest v1 carries
/// only running Splints, and the forward daemon would refuse to adopt an
/// exited child (Plan 0061 M3.3 review). The fenced Topology already records
/// an exit finalized before the fence; one during capture is finalized once
/// the fence lifts, or reads as exited when the forward daemon restores
/// durable state, as after any restart.
///
/// Every actor reserves its checkpoint against one budget of
/// `limits.aggregate_checkpoint_bytes` before sealing it; the first refused
/// reservation cancels at once and is never retried.
///
/// # Errors
/// Cancels on the first missed deadline, a non-exit refusal, a second
/// refusal while the process lives, the aggregate cap, or a failed task.
/// Before returning, every outstanding attempt has ended and every lease
/// already acquired has been dropped, which resumes its actor and returns
/// its budget reservation.
pub async fn capture_live_splints<F, Fut>(
    handles: Vec<LiveSplintHandle>,
    limits: CaptureLimits,
    prepare: F,
) -> Result<HandoffCaptures, CaptureFailure>
where
    F: Fn(LiveSplintHandle, HandoffCheckpointBudget) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<PreparedPtyHandoff, LiveError>> + Send + 'static,
{
    if handles.len() > MAX_LIVE_SPLINTS {
        return Err(CaptureFailure::TooManySplints(handles.len()));
    }
    let prepare = Arc::new(prepare);
    let budget = HandoffCheckpointBudget::new(limits.aggregate_checkpoint_bytes);
    let mut excluded = Vec::new();
    let mut attempts = JoinSet::new();
    for handle in handles {
        if handle.root_process_is_live() {
            attempts.spawn(capture_actor(
                handle,
                limits.actor_timeout,
                budget.clone(),
                Arc::clone(&prepare),
            ));
        } else {
            excluded.push(handle.splint_id);
        }
    }
    let mut captured = Vec::with_capacity(attempts.len());
    let collected = collect_captures(&mut attempts, limits, &mut captured, &mut excluded).await;
    match collected {
        Ok(checkpoint_bytes) => {
            HandoffCaptures::link(captured, excluded, checkpoint_bytes, limits.actor_timeout)
        }
        Err(failure) => {
            // Abort and wait out every outstanding attempt: one that already
            // holds a lease drops it, which resumes its actor.
            attempts.shutdown().await;
            drop(captured);
            Err(failure)
        }
    }
}

async fn collect_captures(
    attempts: &mut JoinSet<Result<ActorCapture, CaptureFailure>>,
    limits: CaptureLimits,
    captured: &mut Vec<(PreparedPtyHandoff, SealedCheckpointBody)>,
    excluded: &mut Vec<SplintId>,
) -> Result<u64, CaptureFailure> {
    let mut checkpoint_bytes = 0_u64;
    while let Some(joined) = attempts.join_next().await {
        match joined.map_err(CaptureFailure::task_failed)?? {
            ActorCapture::Exited(splint_id) => excluded.push(splint_id),
            ActorCapture::Captured(lease) => {
                let mut lease = *lease;
                let splint_id = lease.evidence().splint_id;
                let Some(body) = lease.take_checkpoint_body() else {
                    return Err(CaptureFailure::failed(
                        splint_id,
                        LiveError::HandoffStateUnsupported,
                    ));
                };
                checkpoint_bytes = checkpoint_bytes.saturating_add(body.identity().size);
                captured.push((lease, body));
                // The actors' reservations already hold this bound; this
                // catches a `prepare` that bypassed the budget.
                if checkpoint_bytes > limits.aggregate_checkpoint_bytes {
                    return Err(CaptureFailure::AggregateCapExceeded {
                        splint_id,
                        cap: limits.aggregate_checkpoint_bytes,
                    });
                }
            }
        }
    }
    Ok(checkpoint_bytes)
}

/// Captures one actor, retrying once after a refusal an exit can explain.
async fn capture_actor<F, Fut>(
    handle: LiveSplintHandle,
    timeout: Duration,
    budget: HandoffCheckpointBudget,
    prepare: Arc<F>,
) -> Result<ActorCapture, CaptureFailure>
where
    F: Fn(LiveSplintHandle, HandoffCheckpointBudget) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<PreparedPtyHandoff, LiveError>> + Send + 'static,
{
    let splint_id = handle.splint_id;
    let mut retried = false;
    loop {
        // A missed deadline drops the attempt: a pending command is withdrawn
        // and a lease already granted is dropped, so the actor resumes.
        let error = match time::timeout(timeout, prepare(handle.clone(), budget.clone())).await {
            Err(_) => return Err(CaptureFailure::TimedOut { splint_id, timeout }),
            Ok(Ok(lease)) => return Ok(ActorCapture::Captured(Box::new(lease))),
            Ok(Err(error)) => error,
        };
        if matches!(error, LiveError::CheckpointBudgetExceeded) {
            return Err(CaptureFailure::AggregateCapExceeded {
                splint_id,
                cap: budget.cap(),
            });
        }
        if !exit_can_explain(&error) {
            return Err(CaptureFailure::failed(splint_id, error));
        }
        if retried {
            return if handle.root_process_is_live() {
                Err(CaptureFailure::failed(splint_id, error))
            } else {
                Ok(ActorCapture::Exited(splint_id))
            };
        }
        retried = true;
        // The actor reads normally again meanwhile, so a held synchronized
        // frame publishes before the retry asks.
        time::sleep(HANDOFF_CAPTURE_RETRY_DELAY).await;
    }
}

/// Whether a child exit or PTY EOF during capture can explain a refusal.
const fn exit_can_explain(error: &LiveError) -> bool {
    matches!(
        error,
        LiveError::HandoffStateUnsupported
            | LiveError::ProcessExited
            | LiveError::Pty(_)
            | LiveError::Closed
    )
}

/// The raw numbers of the daemon's two listening sockets, which serve holds
/// open, at the same numbers, for the daemon's whole life.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ListenerDescriptors {
    pub listener: RawFd,
    pub image_listener: RawFd,
}

/// What the gate knows besides the captures and authority.
#[derive(Clone, Copy, Debug)]
pub struct ManifestInputs<'a> {
    /// The Topology as the fence persisted it.
    pub topology: &'a Topology,
    pub listeners: ListenerDescriptors,
    /// The generation this daemon serves.
    pub daemon_generation: u64,
    /// The daemon's installed-prefix authority, which both manifests carry
    /// and which must name both sealed pairs' recorded source paths.
    pub install_prefix: &'a InstallPrefix,
}

/// A validated forward/rollback manifest pair, its canonical encodings, and
/// the two empty manifest memfds the manifests name.
///
/// The exec boundary ([`crate::handoff_exec`]) writes and seals each encoding
/// into its memfd, then prepares inheritance. Dropping this closes both
/// memfds.
#[derive(Debug)]
pub struct PreparedManifests {
    forward: HandoffManifestV1,
    rollback: HandoffManifestV1,
    forward_bytes: Vec<u8>,
    rollback_bytes: Vec<u8>,
    forward_storage: OwnedFd,
    rollback_storage: OwnedFd,
}

impl PreparedManifests {
    #[must_use]
    pub fn forward_bytes(&self) -> &[u8] {
        &self.forward_bytes
    }

    #[must_use]
    pub fn rollback_bytes(&self) -> &[u8] {
        &self.rollback_bytes
    }

    /// The memfd the forward manifest names as its `CheckpointManifest` slot.
    #[must_use]
    pub fn forward_storage(&self) -> BorrowedFd<'_> {
        self.forward_storage.as_fd()
    }

    /// The memfd both manifests name as the `RollbackManifest` slot.
    #[must_use]
    pub fn rollback_storage(&self) -> BorrowedFd<'_> {
        self.rollback_storage.as_fd()
    }

    #[must_use]
    pub const fn handoff_id(&self) -> [u8; 16] {
        self.forward.handoff_id.bytes()
    }

    #[must_use]
    pub fn splint_count(&self) -> usize {
        self.forward.splints.len()
    }

    #[must_use]
    pub fn descriptor_count(&self) -> usize {
        self.forward.descriptors.len()
    }

    /// The served generations the forward manifest moves between.
    #[must_use]
    pub const fn daemon_generations(&self) -> (u64, u64) {
        (
            self.forward.source_daemon_generation,
            self.forward.target_daemon_generation,
        )
    }

    /// Every descriptor slot the manifests record, with the descriptor number
    /// recorded for it, in canonical manifest order. Both manifests record the
    /// same set, so exec inherits exactly these numbers.
    #[must_use]
    pub fn recorded_descriptors(&self) -> Vec<(HandoffDescriptorSlot, RawFd)> {
        self.forward
            .descriptors
            .iter()
            // Every recorded number came from a `RawFd` of at least 3; an
            // impossible one maps to -1, which matches no owner.
            .map(|entry| {
                (
                    entry.slot,
                    RawFd::try_from(entry.inherited_fd).unwrap_or(-1),
                )
            })
            .collect()
    }

    /// Whether the rollback manifest returns to the forward manifest's source.
    #[must_use]
    pub const fn rollback_reverses_forward(&self) -> bool {
        self.rollback.source_daemon_generation == self.forward.target_daemon_generation
            && self.rollback.target_daemon_generation == self.forward.source_daemon_generation
    }
}

/// A manifest the v1 validators refused.
#[derive(Debug, Error)]
#[error(transparent)]
pub struct ManifestRejection(ManifestError);

#[derive(Debug, Error)]
pub enum ManifestPreparationError {
    #[error("captured Splint {0} has no place in the fenced Topology")]
    MissingFromTopology(SplintId),
    #[error("captured Splint {0} reported runtime state a manifest cannot carry")]
    UnsupportedRuntime(SplintId),
    #[error("daemon generation {0} has no successor")]
    GenerationExhausted(u64),
    #[error("descriptor {0} cannot be named as a handoff slot")]
    InvalidDescriptor(RawFd),
    #[error("cannot create handoff manifest storage: {0}")]
    Storage(#[source] io::Error),
    #[error("cannot draw a handoff identity: {0}")]
    Identity(#[source] io::Error),
    #[error(transparent)]
    Checkpoint(#[from] CheckpointStorageError),
    #[error("handoff manifest is invalid: {0}")]
    Manifest(#[source] ManifestRejection),
}

const fn rejected(error: ManifestError) -> ManifestPreparationError {
    ManifestPreparationError::Manifest(ManifestRejection(error))
}

/// Everything the attempt authenticated that a manifest binds.
struct ManifestAuthority<'a> {
    running: RunningHandoffGeneration,
    snapshots: &'a HandoffExecutableSnapshots,
    reports: SealedPreflightReports,
    provenance: ValidatedHandoffPackageProvenance,
    negotiated: NegotiatedHandoff,
}

impl<'a> ManifestAuthority<'a> {
    fn from_attempt(
        rollback: &RollbackHandoffAuthority,
        forward: &'a ForwardHandoffAuthority,
    ) -> Self {
        Self {
            running: rollback.running_generation(),
            snapshots: forward.snapshots(),
            reports: SealedPreflightReports {
                forward: forward.preflight(),
                rollback: rollback.preflight(),
            },
            provenance: ValidatedHandoffPackageProvenance::from_generations(
                forward.provenance().clone(),
                rollback.provenance().clone(),
            ),
            negotiated: forward.negotiated(),
        }
    }
}

/// Builds and validates the forward and rollback manifests for one fenced
/// attempt.
///
/// The manifests carry IDs only for topology, each captured Splint's actor
/// evidence and slots, the three generations' executable evidence, and the
/// served generation moving to the next. Controllers are `Released` and
/// subscriptions `Dropped`: every client reconnects. Topology revision and
/// durable-state generation are the same value: the fence persisted exactly
/// this revision, and the metadata store keeps no other generation.
///
/// Every check the candidate will run passes first: canonical encoding within
/// the manifest byte bound, structural validation of each manifest, pair
/// validation, and authority validation of each against the attempt's sealed
/// snapshots, preflight reports, package provenance, and running contract.
///
/// # Errors
/// Fails closed when a captured Splint is missing from the Topology or
/// reported uncarriable state, the daemon generation cannot advance, manifest
/// storage or the handoff identity cannot be created, or any validation
/// refuses. Nothing is published on failure.
pub fn build_handoff_manifests(
    rollback: &RollbackHandoffAuthority,
    forward: &ForwardHandoffAuthority,
    captures: &HandoffCaptures,
    inputs: &ManifestInputs<'_>,
) -> Result<PreparedManifests, ManifestPreparationError> {
    build_manifests(
        &ManifestAuthority::from_attempt(rollback, forward),
        captures,
        inputs,
    )
}

fn build_manifests(
    authority: &ManifestAuthority<'_>,
    captures: &HandoffCaptures,
    inputs: &ManifestInputs<'_>,
) -> Result<PreparedManifests, ManifestPreparationError> {
    let target_daemon_generation = inputs.daemon_generation.checked_add(1).ok_or(
        ManifestPreparationError::GenerationExhausted(inputs.daemon_generation),
    )?;
    let forward_storage = manifest_storage("splinterd-handoff-manifest")?;
    let rollback_storage = manifest_storage("splinterd-rollback-manifest")?;
    let mut descriptors = global_descriptors(
        authority.snapshots,
        inputs.listeners,
        &forward_storage,
        &rollback_storage,
    )?;
    let mut splints = Vec::with_capacity(captures.captured.len());
    for captured in &captures.captured {
        splints.push(manifest_splint(captured, inputs.topology)?);
        descriptors.push(descriptor(
            HandoffDescriptorSlot::PtyMaster(captured.slot()),
            captured.lease.master_raw_fd(),
        )?);
        descriptors.push(
            captured
                .checkpoint
                .manifest_descriptor(authority.negotiated.terminal_checkpoint)?,
        );
    }
    let revision = inputs.topology.revision().get();
    let negotiated = authority.negotiated;
    let forward = HandoffManifestV1 {
        schema: HANDOFF_MANIFEST_SCHEMA_V1,
        kind: ManifestKind::Forward,
        handoff_id: draw_handoff_id()?,
        source_generation: GenerationEvidence::from_preflight(authority.reports.rollback),
        target_generation: GenerationEvidence::from_preflight(authority.reports.forward),
        source_daemon_generation: inputs.daemon_generation,
        target_daemon_generation,
        topology_revision: revision,
        durable_state_generation: revision,
        private_protocol: negotiated.private_protocol,
        handoff_protocol: negotiated.handoff_protocol,
        terminal_checkpoint_schema: negotiated.terminal_checkpoint,
        descriptor_manifest_schema: negotiated.descriptor_manifest,
        install_prefix: inputs.install_prefix.directory().to_path_buf(),
        executable_pairs: ExecutablePairSetEvidence::from_verified(
            authority.snapshots,
            &authority.reports,
        )
        .map_err(rejected)?,
        counterpart_manifest_slot: HandoffDescriptorSlot::RollbackManifest,
        splints,
        descriptors,
    };
    let rollback = HandoffManifestV1 {
        kind: ManifestKind::Rollback,
        source_generation: forward.target_generation,
        target_generation: forward.source_generation,
        source_daemon_generation: forward.target_daemon_generation,
        target_daemon_generation: forward.source_daemon_generation,
        counterpart_manifest_slot: HandoffDescriptorSlot::CheckpointManifest,
        ..forward.clone()
    };
    let forward_bytes = encode_manifest(&forward).map_err(rejected)?;
    let rollback_bytes = encode_manifest(&rollback).map_err(rejected)?;
    // Validate what the candidate will read: the canonical bytes decoded.
    let forward = decode_manifest(&forward_bytes).map_err(rejected)?;
    let rollback = decode_manifest(&rollback_bytes).map_err(rejected)?;
    validate_manifest_pair(&forward, &rollback).map_err(rejected)?;
    for manifest in [&forward, &rollback] {
        manifest
            .validate_authority_against(
                authority.running,
                authority.snapshots,
                &authority.reports,
                &authority.provenance,
            )
            .map_err(rejected)?;
    }
    Ok(PreparedManifests {
        forward,
        rollback,
        forward_bytes,
        rollback_bytes,
        forward_storage,
        rollback_storage,
    })
}

/// The slots every manifest names: both listeners, both manifest memfds, and
/// the sealed executables the forward daemon inherits. The forward daemon
/// image itself is executed, not inherited.
fn global_descriptors(
    snapshots: &HandoffExecutableSnapshots,
    listeners: ListenerDescriptors,
    forward_storage: &OwnedFd,
    rollback_storage: &OwnedFd,
) -> Result<Vec<ManifestDescriptor>, ManifestPreparationError> {
    let globals = [
        (HandoffDescriptorSlot::Listener, listeners.listener),
        (
            HandoffDescriptorSlot::ImageListener,
            listeners.image_listener,
        ),
        (
            HandoffDescriptorSlot::CheckpointManifest,
            forward_storage.as_raw_fd(),
        ),
        (
            HandoffDescriptorSlot::RollbackManifest,
            rollback_storage.as_raw_fd(),
        ),
        (
            HandoffDescriptorSlot::ForwardClient,
            snapshots.forward().client().descriptor().as_raw_fd(),
        ),
        (
            HandoffDescriptorSlot::RollbackDaemon,
            snapshots.rollback().daemon().descriptor().as_raw_fd(),
        ),
        (
            HandoffDescriptorSlot::RollbackClient,
            snapshots.rollback().client().descriptor().as_raw_fd(),
        ),
    ];
    let mut descriptors = Vec::with_capacity(GLOBAL_DESCRIPTOR_SLOTS + 2 * MAX_LIVE_SPLINTS);
    for (slot, raw) in globals {
        descriptors.push(descriptor(slot, raw)?);
    }
    Ok(descriptors)
}

fn descriptor(
    slot: HandoffDescriptorSlot,
    raw: RawFd,
) -> Result<ManifestDescriptor, ManifestPreparationError> {
    let inherited_fd = u32::try_from(raw)
        .ok()
        .filter(|fd| *fd >= 3)
        .ok_or(ManifestPreparationError::InvalidDescriptor(raw))?;
    Ok(ManifestDescriptor { slot, inherited_fd })
}

fn manifest_splint(
    captured: &CapturedSplint,
    topology: &Topology,
) -> Result<ManifestSplint, ManifestPreparationError> {
    let evidence = captured.evidence();
    let splint_id = evidence.splint_id;
    let (lair_id, dojo_id) = placement(topology, splint_id)
        .ok_or(ManifestPreparationError::MissingFromTopology(splint_id))?;
    // The actor never grants a lease past an exit or EOF, or with write
    // queues still holding bytes; recheck its evidence before relying on it.
    if evidence.child_exit.is_some()
        || evidence.eof_observed
        || evidence.user_write_bytes != 0
        || evidence.reply_write_bytes != 0
        || evidence.synchronized_publication_active
        || evidence.pty_identity != captured.lease.identity()
    {
        return Err(ManifestPreparationError::UnsupportedRuntime(splint_id));
    }
    Ok(ManifestSplint {
        lair_id,
        dojo_id,
        splint_id,
        incarnation: evidence.incarnation.value(),
        pty: ManifestPtyIdentity {
            child_pid: evidence.pty_identity.child_pid(),
            process_group: evidence.pty_identity.process_group(),
            session_id: evidence.pty_identity.session_id(),
            master_slot: captured.slot(),
        },
        size: ManifestPtySize {
            columns: evidence.size.columns,
            rows: evidence.size.rows,
            pixel_width: evidence.size.pixel_width,
            pixel_height: evidence.size.pixel_height,
        },
        foreground_process_group: Some(evidence.foreground_process_group),
        terminal_revision: evidence.terminal_revision.value(),
        history_generation: evidence.history_generation,
        exit: ExitDisposition::Running,
        terminal_checkpoint: captured.checkpoint.terminal_disposition(),
        image_state: captured.checkpoint.image_disposition(),
        controller: ControllerDisposition::Released,
        subscriptions: SubscriptionDisposition::Dropped,
    })
}

fn placement(topology: &Topology, splint_id: SplintId) -> Option<(LairId, DojoId)> {
    topology.lairs().find_map(|lair| {
        lair.dojos
            .iter()
            .find(|dojo| dojo.root.find_splint(splint_id).is_some())
            .map(|dojo| (lair.id, dojo.id))
    })
}

/// One empty, close-on-exec, sealable memfd that exec fills and seals.
fn manifest_storage(name: &str) -> Result<OwnedFd, ManifestPreparationError> {
    memfd_create(name, MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING)
        .map_err(|errno| ManifestPreparationError::Storage(errno.into()))
}

/// A fresh nonzero operation identity from the kernel's random source.
fn draw_handoff_id() -> Result<HandoffId, ManifestPreparationError> {
    let mut bytes = [0_u8; 16];
    let mut filled = 0;
    while filled < bytes.len() {
        filled += getrandom(&mut bytes[filled..], GetRandomFlags::empty())
            .map_err(|errno| ManifestPreparationError::Identity(errno.into()))?;
    }
    // A zero draw has probability 2^-128; refusing it beats retrying.
    HandoffId::new(bytes).map_err(rejected)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        fs::{self, File},
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use splinterm_pty::{LinuxPtyBackend, PtyCommand};
    use splinterm_terminal::TERMINAL_CHECKPOINT_SCHEMA_V2;
    use tokio::sync::Barrier;

    use super::*;
    use crate::{
        LiveSnapshot, LiveSplintConfig, LiveSplintRuntime,
        executable_snapshot::{
            ExecutableSnapshotPolicy, ExecutableSourcePair, RetainedRollbackExecutables,
        },
        handoff_compatibility::{
            BuildVersion, ExecutablePairBinding, ExecutablePairPreflight, HandoffCapabilities,
            VersionRange, negotiate_handoff,
        },
        handoff_manifest::{MAX_HANDOFF_MANIFEST_BYTES, PackagedOwnerOverride},
    };

    pub(crate) const CAT: &str = "printf ready; exec cat";

    fn backend() -> LinuxPtyBackend {
        let test_binary = std::env::current_exe().unwrap();
        let debug_directory = test_binary.parent().unwrap().parent().unwrap();
        let helper = debug_directory.join("splinterm-pty-child");
        assert!(helper.is_file(), "missing PTY helper: {}", helper.display());
        LinuxPtyBackend::new(helper)
    }

    pub(crate) fn config() -> LiveSplintConfig {
        LiveSplintConfig {
            columns: 60,
            rows: 24,
            hangup_grace: Duration::from_millis(30),
            terminate_grace: Duration::from_millis(30),
            poll_interval: Duration::from_millis(5),
            exit_drain_timeout: Duration::from_millis(50),
            ..LiveSplintConfig::default()
        }
    }

    pub(crate) fn snapshot_text(snapshot: &LiveSnapshot) -> String {
        snapshot
            .visible_rows
            .iter()
            .flat_map(|row| row.cells.iter())
            .filter(|cell| cell.spacer_remaining.is_none())
            .map(|cell| cell.content.as_str())
            .collect()
    }

    async fn prepare(
        handle: LiveSplintHandle,
        budget: HandoffCheckpointBudget,
    ) -> Result<PreparedPtyHandoff, LiveError> {
        handle.prepare_pty_handoff_within(&budget).await
    }

    /// Real PTY Splints, each the default focus of its own Lair.
    struct Fleet {
        topology: Topology,
        runtimes: Vec<LiveSplintRuntime>,
    }

    impl Fleet {
        async fn spawn(scripts: &[&str]) -> Self {
            Self::spawn_with(scripts, &config()).await
        }

        async fn spawn_with(scripts: &[&str], config: &LiveSplintConfig) -> Self {
            let mut topology = Topology::new();
            let mut runtimes = Vec::new();
            for (index, script) in scripts.iter().enumerate() {
                let splint_id = topology
                    .create_lair(format!("lair-{index}"), PathBuf::from("/tmp"))
                    .unwrap()
                    .dojos[0]
                    .default_focus;
                let command =
                    PtyCommand::new("/bin/sh", PathBuf::from("/tmp")).args(["-c", script]);
                runtimes.push(
                    LiveSplintRuntime::spawn(splint_id, backend(), command, config.clone())
                        .await
                        .unwrap(),
                );
            }
            Self { topology, runtimes }
        }

        fn handles(&self) -> Vec<LiveSplintHandle> {
            self.runtimes
                .iter()
                .map(LiveSplintRuntime::handle)
                .collect()
        }

        fn id(&self, index: usize) -> SplintId {
            self.runtimes[index].handle().splint_id
        }

        /// Every Splint still running takes input and reads its PTY again:
        /// the terminal echo of `token` reaches its screen.
        async fn assert_all_echo(&self, token: &str) {
            for handle in self.handles() {
                if !handle.root_process_is_live() {
                    continue;
                }
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
                .expect("a released actor reads its PTY again");
            }
        }

        async fn shutdown(self) {
            for runtime in self.runtimes {
                let _ = runtime.shutdown().await;
            }
        }
    }

    fn capabilities() -> HandoffCapabilities {
        let exact = |value| VersionRange::new(value, value).unwrap();
        HandoffCapabilities {
            private_protocol: exact(1),
            handoff_protocol: exact(1),
            terminal_checkpoint: exact(TERMINAL_CHECKPOINT_SCHEMA_V2),
            descriptor_manifest: exact(HANDOFF_MANIFEST_SCHEMA_V1),
        }
    }

    fn preflight(
        pair: &crate::executable_snapshot::SealedExecutablePair,
        patch: u16,
    ) -> ExecutablePairPreflight {
        let version = BuildVersion::new(0, 2, patch);
        ExecutablePairPreflight {
            pair_build_identity: crate::package_provenance::test_pair_build_identity(
                version,
                capabilities(),
            ),
            pair_build_version: version,
            capabilities: capabilities(),
            snapshots: ExecutablePairBinding::from_snapshots(pair),
        }
    }

    static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(1);

    /// Sealed forward and rollback pairs owned by the test user, with the
    /// reports, provenance, and contract an attempt would authenticate.
    struct AuthorityFixture {
        directory: PathBuf,
        install_prefix: InstallPrefix,
        snapshots: HandoffExecutableSnapshots,
        reports: SealedPreflightReports,
        provenance: ValidatedHandoffPackageProvenance,
        running: RunningHandoffGeneration,
        negotiated: NegotiatedHandoff,
    }

    impl AuthorityFixture {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory = std::env::temp_dir().join(format!(
                "splinterd-handoff-prepare-{}-{unique}-{}",
                std::process::id(),
                NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&directory).unwrap();
            let directory = fs::canonicalize(&directory).unwrap();
            let policy = ExecutableSnapshotPolicy {
                expected_owner_uid: rustix::process::geteuid().as_raw(),
            };
            // As in a package upgrade, the forward pair replaces the rollback
            // pair at the same installed paths after the rollback capture.
            let prefix = directory.join("bin");
            let rollback = install_pair(&prefix, "rollback");
            let retained =
                RetainedRollbackExecutables::capture_declared_for_test(&rollback, policy).unwrap();
            let forward = install_pair(&prefix, "forward");
            let snapshots =
                HandoffExecutableSnapshots::materialize(&forward, &retained, policy).unwrap();
            let reports = SealedPreflightReports {
                forward: preflight(snapshots.forward(), 2),
                rollback: preflight(snapshots.rollback(), 1),
            };
            let running = RunningHandoffGeneration {
                build_identity: reports.rollback.pair_build_identity,
                build_version: reports.rollback.pair_build_version,
                capabilities: reports.rollback.capabilities,
            };
            let negotiated =
                negotiate_handoff(running, reports.forward, reports.rollback, &snapshots).unwrap();
            let provenance = ValidatedHandoffPackageProvenance::from_verified_for_test(
                &snapshots,
                reports.forward,
                reports.rollback,
            );
            Self {
                directory,
                install_prefix: InstallPrefix::from_directory(&prefix).unwrap(),
                snapshots,
                reports,
                provenance,
                running,
                negotiated,
            }
        }

        fn authority(&self) -> ManifestAuthority<'_> {
            ManifestAuthority {
                running: self.running,
                snapshots: &self.snapshots,
                reports: self.reports,
                provenance: self.provenance.clone(),
                negotiated: self.negotiated,
            }
        }
    }

    impl Drop for AuthorityFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    /// Installs one generation's pair in `prefix` by rename, replacing any
    /// earlier generation with new inodes.
    fn install_pair(prefix: &Path, generation: &str) -> ExecutableSourcePair {
        fs::create_dir_all(prefix).unwrap();
        let daemon = prefix.join("splinterd");
        let client = prefix.join("splinterm");
        for (path, role) in [(&daemon, "daemon"), (&client, "client")] {
            let staged = prefix.join(format!(".{role}.staged"));
            fs::write(&staged, format!("{generation}-{role}")).unwrap();
            fs::set_permissions(&staged, fs::Permissions::from_mode(0o755)).unwrap();
            fs::rename(&staged, path).unwrap();
        }
        ExecutableSourcePair::new(daemon, client).unwrap()
    }

    /// Two open descriptors standing in for the daemon's listeners.
    fn listeners() -> (File, File, ListenerDescriptors) {
        let listener = File::open("/dev/null").unwrap();
        let image_listener = File::open("/dev/null").unwrap();
        let descriptors = ListenerDescriptors {
            listener: listener.as_raw_fd(),
            image_listener: image_listener.as_raw_fd(),
        };
        (listener, image_listener, descriptors)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn every_live_splint_is_captured_concurrently_and_held_until_release() {
        let fleet = Fleet::spawn(&[CAT, CAT, CAT]).await;
        // Each capture waits for the other two to start, so this completes
        // only when all three run at once.
        let barrier = Arc::new(Barrier::new(3));
        let captures = capture_live_splints(fleet.handles(), CaptureLimits::DEFAULT, {
            move |handle: LiveSplintHandle, budget: HandoffCheckpointBudget| {
                let barrier = Arc::clone(&barrier);
                async move {
                    barrier.wait().await;
                    handle.prepare_pty_handoff_within(&budget).await
                }
            }
        })
        .await
        .unwrap();

        assert_eq!(captures.captured().len(), 3);
        assert!(captures.excluded().is_empty());
        let slots = captures
            .captured()
            .iter()
            .map(CapturedSplint::slot)
            .collect::<Vec<_>>();
        assert_eq!(slots, [0, 1, 2]);
        assert_eq!(
            captures.checkpoint_bytes(),
            captures
                .captured()
                .iter()
                .map(|captured| captured.checkpoint().body().identity().size)
                .sum::<u64>()
        );
        // A held lease fences its actor: input waits for the release.
        let handle = fleet.runtimes[0].handle();
        assert!(
            time::timeout(Duration::from_millis(100), handle.input(b"held\n".to_vec()))
                .await
                .is_err()
        );
        captures.release().await;
        fleet.assert_all_echo("after-release").await;
        fleet.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_missed_actor_deadline_times_out_and_resumes_every_actor() {
        let fleet = Fleet::spawn(&[CAT, CAT, CAT]).await;
        let stalled = fleet.id(1);
        let limits = CaptureLimits {
            actor_timeout: Duration::from_millis(300),
            ..CaptureLimits::DEFAULT
        };
        let failure = capture_live_splints(
            fleet.handles(),
            limits,
            move |handle: LiveSplintHandle, budget: HandoffCheckpointBudget| async move {
                let lease = handle.prepare_pty_handoff_within(&budget).await?;
                if handle.splint_id == stalled {
                    // Holds its lease past the deadline.
                    time::sleep(Duration::from_secs(30)).await;
                }
                Ok(lease)
            },
        )
        .await
        .unwrap_err();

        assert!(matches!(
            failure,
            CaptureFailure::TimedOut { splint_id, .. } if splint_id == stalled
        ));
        assert_eq!(failure.cancel_reason(), HandoffCancelReason::TimedOut);
        fleet.assert_all_echo("after-timeout").await;
        fleet.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_splint_that_exits_during_capture_is_retried_once_then_excluded() {
        let fleet = Fleet::spawn(&[CAT, "read line; exit 0", CAT]).await;
        let exiting = fleet.id(1);
        let calls = Arc::new(AtomicUsize::new(0));
        let captures = capture_live_splints(fleet.handles(), CaptureLimits::DEFAULT, {
            let calls = Arc::clone(&calls);
            move |handle: LiveSplintHandle, budget: HandoffCheckpointBudget| {
                let calls = Arc::clone(&calls);
                async move {
                    if handle.splint_id == exiting {
                        calls.fetch_add(1, Ordering::SeqCst);
                        // Its child exits just before the actor is asked.
                        let _ = handle.input(b"bye\n".to_vec()).await;
                        let _ = handle.wait_for_exit().await;
                    }
                    handle.prepare_pty_handoff_within(&budget).await
                }
            }
        })
        .await
        .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 2, "one retry, then exclusion");
        assert_eq!(captures.excluded(), [exiting]);
        assert_eq!(captures.captured().len(), 2);
        assert!(
            captures
                .captured()
                .iter()
                .all(|captured| captured.evidence().splint_id != exiting)
        );
        captures.release().await;
        fleet.assert_all_echo("after-exit").await;
        fleet.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_already_exited_splint_is_excluded_without_an_attempt() {
        let fleet = Fleet::spawn(&[CAT, "exit 0"]).await;
        let exited = fleet.id(1);
        fleet.runtimes[1].handle().wait_for_exit().await.unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let captures = capture_live_splints(fleet.handles(), CaptureLimits::DEFAULT, {
            let calls = Arc::clone(&calls);
            move |handle: LiveSplintHandle, budget: HandoffCheckpointBudget| {
                if handle.splint_id == exited {
                    calls.fetch_add(1, Ordering::SeqCst);
                }
                prepare(handle, budget)
            }
        })
        .await
        .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(captures.excluded(), [exited]);
        assert_eq!(captures.captured().len(), 1);
        captures.release().await;
        fleet.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_refusal_the_retry_clears_is_captured() {
        let fleet = Fleet::spawn(&[CAT, CAT]).await;
        let refused_once = fleet.id(0);
        let calls = Arc::new(AtomicUsize::new(0));
        let captures = capture_live_splints(fleet.handles(), CaptureLimits::DEFAULT, {
            let calls = Arc::clone(&calls);
            move |handle: LiveSplintHandle, budget: HandoffCheckpointBudget| {
                let first =
                    handle.splint_id == refused_once && calls.fetch_add(1, Ordering::SeqCst) == 0;
                async move {
                    if first {
                        return Err(LiveError::HandoffStateUnsupported);
                    }
                    handle.prepare_pty_handoff_within(&budget).await
                }
            }
        })
        .await
        .unwrap();

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(captures.captured().len(), 2);
        captures.release().await;
        fleet.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_second_refusal_of_a_running_splint_fails_and_resumes_every_actor() {
        let fleet = Fleet::spawn(&[CAT, CAT, CAT]).await;
        let refusing = fleet.id(2);
        let calls = Arc::new(AtomicUsize::new(0));
        let failure = capture_live_splints(fleet.handles(), CaptureLimits::DEFAULT, {
            let calls = Arc::clone(&calls);
            move |handle: LiveSplintHandle, budget: HandoffCheckpointBudget| {
                let refuse = handle.splint_id == refusing;
                if refuse {
                    calls.fetch_add(1, Ordering::SeqCst);
                }
                async move {
                    let lease = handle.prepare_pty_handoff_within(&budget).await?;
                    if refuse {
                        drop(lease);
                        return Err(LiveError::HandoffStateUnsupported);
                    }
                    Ok(lease)
                }
            }
        })
        .await
        .unwrap_err();

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(matches!(
            &failure,
            CaptureFailure::Failed { splint_id, error }
                if *splint_id == refusing
                    && matches!(**error, LiveError::HandoffStateUnsupported)
        ));
        assert_eq!(failure.cancel_reason(), HandoffCancelReason::Failed);
        fleet.assert_all_echo("after-refusal").await;
        fleet.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_open_synchronized_update_fails_capture_after_one_retry() {
        let fleet = Fleet::spawn(&[CAT, "printf '\\033[?2026hactive'; sleep 5"]).await;
        let synchronized = fleet.runtimes[1].handle();
        time::timeout(Duration::from_secs(2), async {
            while !snapshot_text(&synchronized.snapshot().await.unwrap()).contains("active") {
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let failure = capture_live_splints(fleet.handles(), CaptureLimits::DEFAULT, prepare)
            .await
            .unwrap_err();

        assert!(matches!(
            failure,
            CaptureFailure::Failed { splint_id, .. } if splint_id == synchronized.splint_id
        ));
        fleet.assert_all_echo("after-synchronized").await;
        fleet.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_refusal_no_exit_explains_is_not_retried() {
        let fleet = Fleet::spawn(&[CAT, CAT]).await;
        let broken = fleet.id(0);
        let calls = Arc::new(AtomicUsize::new(0));
        let failure = capture_live_splints(fleet.handles(), CaptureLimits::DEFAULT, {
            let calls = Arc::clone(&calls);
            move |handle: LiveSplintHandle, budget: HandoffCheckpointBudget| {
                let fail = handle.splint_id == broken;
                if fail {
                    calls.fetch_add(1, Ordering::SeqCst);
                }
                async move {
                    if fail {
                        return Err(LiveError::Io(io::Error::other("injected")));
                    }
                    handle.prepare_pty_handoff_within(&budget).await
                }
            }
        })
        .await
        .unwrap_err();

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            failure,
            CaptureFailure::Failed { splint_id, .. } if splint_id == broken
        ));
        fleet.assert_all_echo("after-io").await;
        fleet.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn passing_the_aggregate_cap_fails_and_resumes_every_actor() {
        let fleet = Fleet::spawn(&[CAT, CAT, CAT]).await;
        let limits = CaptureLimits {
            aggregate_checkpoint_bytes: 1,
            ..CaptureLimits::DEFAULT
        };
        let failure = capture_live_splints(fleet.handles(), limits, prepare)
            .await
            .unwrap_err();

        assert!(matches!(
            failure,
            CaptureFailure::AggregateCapExceeded { cap: 1, .. }
        ));
        assert_eq!(failure.cancel_reason(), HandoffCancelReason::Failed);
        fleet.assert_all_echo("after-cap").await;
        fleet.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sealed_checkpoints_never_pass_the_budget_and_its_refusal_is_not_retried() {
        let fleet = Fleet::spawn(&[CAT, CAT, CAT, CAT]).await;
        fleet.assert_all_echo("settled").await;
        let measured = capture_live_splints(fleet.handles(), CaptureLimits::DEFAULT, prepare)
            .await
            .unwrap();
        let largest = measured
            .captured()
            .iter()
            .map(|captured| captured.checkpoint().body().identity().size)
            .max()
            .unwrap();
        measured.release().await;
        // Room for two checkpoints and a half, so all four together never fit.
        let cap = largest * 2 + largest / 2;
        let seen = Arc::new(std::sync::Mutex::new(Vec::<(
            SplintId,
            HandoffCheckpointBudget,
        )>::new()));
        let limits = CaptureLimits {
            aggregate_checkpoint_bytes: cap,
            ..CaptureLimits::DEFAULT
        };
        let failure = capture_live_splints(fleet.handles(), limits, {
            let seen = Arc::clone(&seen);
            move |handle: LiveSplintHandle, budget: HandoffCheckpointBudget| {
                seen.lock()
                    .unwrap()
                    .push((handle.splint_id, budget.clone()));
                async move { handle.prepare_pty_handoff_within(&budget).await }
            }
        })
        .await
        .unwrap_err();

        assert!(matches!(
            failure,
            CaptureFailure::AggregateCapExceeded { cap: refused, .. } if refused == cap
        ));
        let (budget, mut attempts) = {
            let seen = seen.lock().unwrap();
            let attempts = seen
                .iter()
                .map(|(splint_id, _)| *splint_id)
                .collect::<Vec<_>>();
            (seen[0].1.clone(), attempts)
        };
        assert!(budget.high_water() <= cap, "sealed bytes passed the cap");
        assert!(budget.high_water() > 0);
        let attempted = attempts.len();
        attempts.sort_by_key(ToString::to_string);
        attempts.dedup();
        assert_eq!(attempts.len(), attempted, "no capture was retried");
        // An actor still inside its capture when the attempt was aborted
        // drops its lease, and the reservation with it, as it replies.
        time::timeout(Duration::from_secs(5), async {
            while budget.reserved() != 0 {
                time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("every reservation is returned");
        fleet.assert_all_echo("after-budget").await;
        fleet.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_synchronized_frame_held_at_the_first_attempt_is_captured_on_the_retry() {
        let fleet = Fleet::spawn(&[
            CAT,
            "stty -echo; printf ready; while read l; do printf '\\033[?2026h1\\033[?2026l\\033[?2026h2\\033[?2026l\\033[?2026hframe-%s\\033[?2026l' \"$l\"; done",
        ])
        .await;
        let framed = fleet.id(1);
        let first = Arc::new(std::sync::Mutex::new(None::<bool>));
        let calls = Arc::new(AtomicUsize::new(0));
        let captures = capture_live_splints(fleet.handles(), CaptureLimits::DEFAULT, {
            let (first, calls) = (Arc::clone(&first), Arc::clone(&calls));
            move |handle: LiveSplintHandle, budget: HandoffCheckpointBudget| {
                let opening =
                    handle.splint_id == framed && calls.fetch_add(1, Ordering::SeqCst) == 0;
                let first = Arc::clone(&first);
                async move {
                    if !opening {
                        return handle.prepare_pty_handoff_within(&budget).await;
                    }
                    // Three frames in one burst: the first publishes and the
                    // others are held for up to one frame interval.
                    handle.input(b"x\n".to_vec()).await?;
                    while !snapshot_text(&handle.snapshot().await?).contains("frame-x") {
                        tokio::task::yield_now().await;
                    }
                    let attempt = handle.prepare_pty_handoff_within(&budget).await;
                    *first.lock().unwrap() =
                        Some(matches!(attempt, Err(LiveError::HandoffStateUnsupported)));
                    attempt
                }
            }
        })
        .await
        .unwrap();

        assert_eq!(
            *first.lock().unwrap(),
            Some(true),
            "the first attempt met the held frame"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(captures.captured().len(), 2);
        captures.release().await;
        fleet.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn captures_build_one_validated_manifest_pair() {
        let fleet = Fleet::spawn(&[CAT, CAT, CAT]).await;
        let captures = capture_live_splints(fleet.handles(), CaptureLimits::DEFAULT, prepare)
            .await
            .unwrap();
        let fixture = AuthorityFixture::new();
        let (_listener, _image_listener, listeners) = listeners();
        let inputs = ManifestInputs {
            topology: &fleet.topology,
            listeners,
            daemon_generation: 1,
            install_prefix: &fixture.install_prefix,
        };
        let owner = PackagedOwnerOverride::current_user();
        let manifests = build_manifests(&fixture.authority(), &captures, &inputs).unwrap();
        drop(owner);

        assert!(manifests.forward_bytes().len() <= MAX_HANDOFF_MANIFEST_BYTES);
        assert!(manifests.rollback_bytes().len() <= MAX_HANDOFF_MANIFEST_BYTES);
        {
            let _owner = PackagedOwnerOverride::current_user();
            assert_eq!(
                decode_manifest(manifests.forward_bytes()).unwrap(),
                manifests.forward
            );
            assert_eq!(
                decode_manifest(manifests.rollback_bytes()).unwrap(),
                manifests.rollback
            );
            validate_manifest_pair(&manifests.forward, &manifests.rollback).unwrap();
        }
        assert_eq!(manifests.splint_count(), 3);
        assert_eq!(
            manifests.forward.install_prefix,
            fixture.install_prefix.directory()
        );
        assert_eq!(
            manifests.rollback.install_prefix,
            manifests.forward.install_prefix
        );
        assert_eq!(manifests.descriptor_count(), GLOBAL_DESCRIPTOR_SLOTS + 6);
        assert_eq!(manifests.daemon_generations(), (1, 2));
        assert!(manifests.rollback_reverses_forward());
        assert_eq!(
            manifests.forward.topology_revision,
            fleet.topology.revision().get()
        );
        for captured in captures.captured() {
            let evidence = captured.evidence();
            let record = manifests
                .forward
                .splints
                .iter()
                .find(|record| record.splint_id == evidence.splint_id)
                .unwrap();
            assert_eq!(
                Some((record.lair_id, record.dojo_id)),
                placement(&fleet.topology, evidence.splint_id)
            );
            assert_eq!(record.incarnation, evidence.incarnation.value());
            assert_eq!(
                record.pty.child_pid,
                captured.lease().identity().child_pid()
            );
            assert_eq!(record.pty.master_slot, captured.slot());
            assert!(manifests.forward.descriptors.contains(&ManifestDescriptor {
                slot: HandoffDescriptorSlot::PtyMaster(captured.slot()),
                inherited_fd: u32::try_from(captured.lease().master_raw_fd()).unwrap(),
            }));
        }
        drop(manifests);
        captures.release().await;
        fleet.assert_all_echo("after-manifest").await;
        fleet.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_refused_manifest_publishes_nothing_and_releases_every_capture() {
        let fleet = Fleet::spawn(&[CAT, CAT]).await;
        let captures = capture_live_splints(fleet.handles(), CaptureLimits::DEFAULT, prepare)
            .await
            .unwrap();
        let fixture = AuthorityFixture::new();
        let (_listener, _image_listener, listeners) = listeners();
        let inputs = ManifestInputs {
            topology: &fleet.topology,
            listeners,
            daemon_generation: 1,
            install_prefix: &fixture.install_prefix,
        };
        // Without the test owner override, authority validation refuses the
        // test user's executables exactly as it refuses any non-root source.
        assert!(matches!(
            build_manifests(&fixture.authority(), &captures, &inputs),
            Err(ManifestPreparationError::Manifest(_))
        ));
        let _owner = PackagedOwnerOverride::current_user();
        // A prefix that does not hold both pairs' recorded sources.
        let elsewhere = InstallPrefix::from_directory(&fixture.directory).unwrap();
        assert!(matches!(
            build_manifests(
                &fixture.authority(),
                &captures,
                &ManifestInputs {
                    install_prefix: &elsewhere,
                    ..inputs
                },
            ),
            Err(ManifestPreparationError::Manifest(ManifestRejection(
                ManifestError::InvalidInstallPrefix
            )))
        ));
        let empty = Topology::new();
        assert!(matches!(
            build_manifests(
                &fixture.authority(),
                &captures,
                &ManifestInputs {
                    topology: &empty,
                    ..inputs
                },
            ),
            Err(ManifestPreparationError::MissingFromTopology(_))
        ));
        assert!(matches!(
            build_manifests(
                &fixture.authority(),
                &captures,
                &ManifestInputs {
                    daemon_generation: u64::MAX,
                    ..inputs
                },
            ),
            Err(ManifestPreparationError::GenerationExhausted(u64::MAX))
        ));
        captures.release().await;
        fleet.assert_all_echo("after-refused-manifest").await;
        fleet.shutdown().await;
    }

    /// The shared image and kitty-upload budget limits of [`hand_off_fleet`]
    /// terminals; a candidate restores their checkpoints with equal limits.
    pub(crate) const ADOPTION_BUDGET_BYTES: usize = 1 << 20;

    /// Everything one in-process old generation hands its candidate: the
    /// fenced Topology, the validated manifest pair and its storage, and
    /// each captured Splint ended for handoff with its linked checkpoint.
    /// The fixture keeps the sealed executable pairs and the install prefix
    /// alive.
    pub(crate) struct OldSideHandoff {
        pub(crate) forward_client: OwnedFd,
        pub(crate) rollback_daemon: OwnedFd,
        pub(crate) rollback_client: OwnedFd,
        pub(crate) topology: Topology,
        pub(crate) forward: HandoffManifestV1,
        pub(crate) rollback: HandoffManifestV1,
        pub(crate) forward_storage: OwnedFd,
        pub(crate) rollback_storage: OwnedFd,
        /// Keyed by the slot each Splint's master and checkpoint share.
        pub(crate) handed_off: Vec<(u16, crate::HandedOffSplint, LinkedCheckpointBody)>,
        _fixture: AuthorityFixture,
    }

    /// Spawns one real PTY Splint per script with the daemon's shared budgets,
    /// which a handoff checkpoint records ([`ADOPTION_BUDGET_BYTES`] each),
    /// waits until each printed `ready`, captures them all, builds the
    /// manifests as daemon generation 1 naming `listeners`, and ends every
    /// actor for handoff.
    pub(crate) async fn hand_off_fleet(
        scripts: &[&str],
        listeners: ListenerDescriptors,
    ) -> OldSideHandoff {
        let mut adoptable = config();
        adoptable.terminal.shared_image_budget = Some(splinterm_terminal::SharedImageBudget::new(
            ADOPTION_BUDGET_BYTES,
        ));
        adoptable.terminal.shared_kitty_upload_budget = Some(
            splinterm_terminal::SharedKittyUploadBudget::new(ADOPTION_BUDGET_BYTES),
        );
        let mut fleet = Fleet::spawn_with(scripts, &adoptable).await;
        for handle in fleet.handles() {
            // The daemon records each spawned incarnation in the Topology.
            assert!(
                fleet
                    .topology
                    .set_splint_last_incarnation(handle.splint_id, handle.incarnation.value())
            );
            time::timeout(Duration::from_secs(5), async {
                while !snapshot_text(&handle.snapshot().await.unwrap()).contains("ready") {
                    time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("every Splint prints ready");
        }
        let captures = capture_live_splints(fleet.handles(), CaptureLimits::DEFAULT, prepare)
            .await
            .unwrap();
        assert!(captures.excluded().is_empty());
        let fixture = AuthorityFixture::new();
        let manifests = {
            let _owner = PackagedOwnerOverride::current_user();
            build_manifests(
                &fixture.authority(),
                &captures,
                &ManifestInputs {
                    topology: &fleet.topology,
                    listeners,
                    daemon_generation: 1,
                    install_prefix: &fixture.install_prefix,
                },
            )
            .unwrap()
        };
        let handed_off = captures
            .captured
            .into_iter()
            .map(|CapturedSplint { lease, checkpoint }| {
                (checkpoint.slot(), lease.into_handed_off(), checkpoint)
            })
            .collect();
        // Each old actor ends on its handoff without killing its child.
        for runtime in fleet.runtimes {
            assert!(matches!(runtime.wait().await, Err(LiveError::HandedOff)));
        }
        let duplicate = |descriptor: BorrowedFd<'_>| descriptor.try_clone_to_owned().unwrap();
        OldSideHandoff {
            forward_client: duplicate(fixture.snapshots.forward().client().descriptor()),
            rollback_daemon: duplicate(fixture.snapshots.rollback().daemon().descriptor()),
            rollback_client: duplicate(fixture.snapshots.rollback().client().descriptor()),
            topology: fleet.topology,
            forward: manifests.forward,
            rollback: manifests.rollback,
            forward_storage: manifests.forward_storage,
            rollback_storage: manifests.rollback_storage,
            handed_off,
            _fixture: fixture,
        }
    }
}
