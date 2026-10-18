//! Startup retention of the running packaged daemon/client rollback authority,
//! and per-attempt preparation of the installed forward pair beside it.
//!
//! This is deliberately preparation-only. It does not build a checkpoint or
//! descriptor manifest, quiesce live PTYs, execute a forward image, publish a
//! generation, or claim candidate descriptors. It also owns the fail-closed
//! handoff phase machine and the exclusive single-threaded process guard that
//! both sides of the exec boundary hold across descriptor-table work.

use std::{
    io,
    marker::PhantomData,
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use thiserror::Error;

use crate::{
    executable_snapshot::{
        ExecutableSnapshotError, ExecutableSnapshotPolicy, ExecutableSourcePair,
        HandoffExecutableSnapshots, PACKAGED_OWNER_UID, RetainedRollbackExecutables,
        SealedExecutablePair,
    },
    handoff_compatibility::{
        CompatibilityError, ExecutablePairPreflight, NegotiatedHandoff, RunningHandoffGeneration,
        negotiate_handoff,
    },
    handoff_preflight::{PreflightError, PreflightGeneration, preflight_sealed_pair},
    package_provenance::{
        PackageProvenanceError, PackageProvenanceRecord, ValidatedPackageProvenance,
        package_provenance_path,
    },
};

static HANDOFF_PROCESS_GUARD_ACTIVE: AtomicBool = AtomicBool::new(false);
const PROCESS_GUARD_RETRY_INITIAL: Duration = Duration::from_millis(1);
const PROCESS_GUARD_RETRY_MAX: Duration = Duration::from_millis(10);

/// One explicit handoff generation phase.
///
/// The old and candidate generations construct separate lifecycle values across
/// exec. Transitions encode the rollback boundary: once adoption commits, no
/// transition back to cooperative rollback is valid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    dead_code,
    reason = "the complete transition boundary lands before each coordinator call site"
)]
pub(crate) enum HandoffPhase {
    /// The old generation serves normally; no handoff is in progress.
    OldRunning,
    /// The old generation is building checkpoint and manifest authority.
    Preparing,
    /// The old generation fences lifecycle work. Before `RuntimeStopped`, the
    /// coordinator that drives this phase must also drain or fence retiring
    /// Splint processes and held unused-close claims, which leave the runtime
    /// registry before their processes are reaped.
    Quiescing,
    /// Tokio and every live runtime are stopped; the process is single-threaded.
    RuntimeStopped,
    /// The bounded descriptor allowlist is inheritable for exec.
    InheritancePrepared,
    /// The candidate owns the complete inherited descriptor set.
    CandidateClaimed,
    /// The candidate verified and staged every inherited authority.
    AdoptionStaged,
    /// The candidate adopted the staged authority; rollback is closed.
    Committed,
    /// The candidate published its generation.
    Published,
    /// The candidate is returning authority to the rollback generation.
    RollingBack,
    /// The handoff failed terminally.
    Failed,
}

/// Fail-closed state machine for one side of a daemon handoff exec boundary.
///
/// Deliberately neither `Clone` nor `Copy`: the lifecycle is uniquely owned,
/// so committing advances the only rollback-capable value and no stale copy
/// can later roll back.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct HandoffLifecycle {
    phase: HandoffPhase,
}

impl HandoffLifecycle {
    #[must_use]
    pub(crate) const fn old_running() -> Self {
        Self {
            phase: HandoffPhase::OldRunning,
        }
    }

    #[must_use]
    pub(crate) const fn candidate_claimed() -> Self {
        Self {
            phase: HandoffPhase::CandidateClaimed,
        }
    }

    #[must_use]
    pub(crate) const fn phase(&self) -> HandoffPhase {
        self.phase
    }

    /// Advances to `next` only along a defined handoff edge.
    pub(crate) fn transition(&mut self, next: HandoffPhase) -> Result<(), HandoffCoordinatorError> {
        let valid = matches!(
            (self.phase, next),
            (HandoffPhase::OldRunning, HandoffPhase::Preparing)
                | (
                    HandoffPhase::Preparing,
                    HandoffPhase::Quiescing | HandoffPhase::OldRunning
                )
                | (
                    HandoffPhase::Quiescing,
                    HandoffPhase::RuntimeStopped | HandoffPhase::OldRunning
                )
                | (
                    HandoffPhase::RuntimeStopped,
                    HandoffPhase::InheritancePrepared | HandoffPhase::OldRunning
                )
                | (HandoffPhase::InheritancePrepared, HandoffPhase::OldRunning)
                | (
                    HandoffPhase::CandidateClaimed,
                    HandoffPhase::AdoptionStaged | HandoffPhase::RollingBack
                )
                | (
                    HandoffPhase::AdoptionStaged,
                    HandoffPhase::RollingBack | HandoffPhase::Committed
                )
                | (
                    HandoffPhase::Committed,
                    HandoffPhase::Published | HandoffPhase::Failed
                )
                | (
                    HandoffPhase::Published | HandoffPhase::RollingBack,
                    HandoffPhase::Failed
                )
        );
        if !valid {
            return Err(HandoffCoordinatorError::InvalidPhaseTransition);
        }
        self.phase = next;
        Ok(())
    }
}

/// Exclusive single-threaded process authority for descriptor-table handoff.
///
/// Acquiring this guard proves that exactly one thread exists at the observation
/// point and prevents a competing Splinterm handoff critical section. The owner
/// must not create another thread while it is held. The guard is intentionally
/// neither `Send` nor `Sync`, keeping the pre/post-exec critical section on its
/// acquiring thread.
#[derive(Debug)]
#[doc(hidden)]
pub struct HandoffProcessGuard {
    _not_send_or_sync: PhantomData<Rc<()>>,
}

impl HandoffProcessGuard {
    /// Acquires exclusive handoff authority in an observed single-threaded
    /// process.
    ///
    /// The thread observation is one close-on-exec procfs directory read,
    /// closed before this returns.
    ///
    /// # Errors
    /// Rejects a competing handoff, an unreadable process thread table, or any
    /// process with other live threads.
    pub fn acquire() -> Result<Self, HandoffCoordinatorError> {
        Self::acquire_with(process_thread_count)
    }

    /// Acquires the guard like [`Self::acquire`], retrying until `deadline`
    /// while other threads are still exiting.
    ///
    /// The kernel reaps a thread shortly after `pthread_join` returns, so a
    /// joined thread can stay listed in `/proc/self/task` for a moment. Only
    /// an observed thread count above one is retried, with a short backoff.
    /// A competing guard or an unreadable thread table fails at once. The
    /// guard is never held between attempts, and at least one observation is
    /// made even when `deadline` has already passed.
    ///
    /// # Errors
    /// As [`Self::acquire`]; a timeout reports the last observed count.
    pub fn acquire_until(deadline: Instant) -> Result<Self, HandoffCoordinatorError> {
        Self::acquire_until_with(deadline, process_thread_count)
    }

    pub(crate) fn acquire_until_with(
        deadline: Instant,
        mut observe_threads: impl FnMut() -> io::Result<usize>,
    ) -> Result<Self, HandoffCoordinatorError> {
        let mut pause = PROCESS_GUARD_RETRY_INITIAL;
        loop {
            match Self::acquire_with(&mut observe_threads) {
                Err(HandoffCoordinatorError::ProcessNotSingleThreaded { observed }) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(HandoffCoordinatorError::ProcessNotSingleThreaded { observed });
                    }
                    thread::sleep(pause.min(remaining));
                    pause = pause.saturating_mul(2).min(PROCESS_GUARD_RETRY_MAX);
                }
                result => return result,
            }
        }
    }

    pub(crate) fn acquire_with(
        observe_threads: impl FnOnce() -> io::Result<usize>,
    ) -> Result<Self, HandoffCoordinatorError> {
        HANDOFF_PROCESS_GUARD_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| HandoffCoordinatorError::ProcessGuardActive)?;
        let observed = match observe_threads() {
            Ok(observed) => observed,
            Err(error) => {
                HANDOFF_PROCESS_GUARD_ACTIVE.store(false, Ordering::Release);
                return Err(HandoffCoordinatorError::InspectProcessThreads(error));
            }
        };
        if observed != 1 {
            HANDOFF_PROCESS_GUARD_ACTIVE.store(false, Ordering::Release);
            return Err(HandoffCoordinatorError::ProcessNotSingleThreaded { observed });
        }
        Ok(Self {
            _not_send_or_sync: PhantomData,
        })
    }
}

impl Drop for HandoffProcessGuard {
    fn drop(&mut self) {
        let was_active = HANDOFF_PROCESS_GUARD_ACTIVE.swap(false, Ordering::AcqRel);
        debug_assert!(was_active, "handoff process guard released exactly once");
    }
}

fn process_thread_count() -> io::Result<usize> {
    std::fs::read_dir("/proc/self/task")?.try_fold(0_usize, |count, entry| {
        entry.map(|_| count.saturating_add(1))
    })
}

/// Complete rollback authority retained before package paths can be replaced.
#[derive(Debug)]
pub struct RollbackHandoffAuthority {
    executables: RetainedRollbackExecutables,
    preflight: ExecutablePairPreflight,
    provenance: ValidatedPackageProvenance,
    lifecycle: HandoffLifecycle,
}

impl RollbackHandoffAuthority {
    /// Captures the running installed pair and binds it to packaged provenance.
    ///
    /// This runs synchronously during startup, before Tokio, listeners,
    /// connection tasks, or live PTY runtimes exist, so the running pair is
    /// sealed before a package upgrade can replace it, and nothing else can
    /// create or close descriptors meanwhile. The exclusive
    /// [`HandoffProcessGuard`] enforces one thread from the first executable
    /// capture through preflight and provenance binding. Sealed preflight no
    /// longer depends on that quiet (Plan 0061 D2): forward preparation runs it
    /// beside a serving daemon.
    ///
    /// # Errors
    ///
    /// Fails closed if the installed executable pair, sealed execution, or
    /// package record cannot be authenticated as one exact running generation,
    /// or if the process is not single-threaded once a package record exists.
    pub fn prepare_installed() -> Result<Self, HandoffCoordinatorError> {
        let daemon = std::env::current_exe().map_err(HandoffCoordinatorError::CurrentExecutable)?;
        let daemon =
            std::fs::canonicalize(daemon).map_err(HandoffCoordinatorError::CurrentExecutable)?;
        // Locating the record first rejects development and test daemons before
        // either executable is copied.
        let record_path =
            package_provenance_path(&daemon).map_err(|_| HandoffCoordinatorError::NotPackaged)?;
        // Dirty-tree builds install the pair without a record on purpose.
        let record = match PackageProvenanceRecord::load(&record_path, PACKAGED_OWNER_UID) {
            Err(PackageProvenanceError::Open { source, .. })
                if source.kind() == io::ErrorKind::NotFound =>
            {
                return Err(HandoffCoordinatorError::NotPackaged);
            }
            record => record?,
        };
        // Development daemons return above without the guard, so only a
        // packaged pair's capture and sealed preflight need one thread.
        let _process_guard = HandoffProcessGuard::acquire()?;
        let source = installed_source_pair(&daemon)?;
        let executables = RetainedRollbackExecutables::capture(
            &source,
            ExecutableSnapshotPolicy {
                expected_owner_uid: PACKAGED_OWNER_UID,
            },
        )?;
        prepare_with_preflight(executables, &record, |pair| {
            preflight_sealed_pair(pair, PreflightGeneration::Rollback)
        })
    }

    #[must_use]
    pub const fn pair(&self) -> &SealedExecutablePair {
        self.executables.pair()
    }

    #[must_use]
    pub const fn preflight(&self) -> ExecutablePairPreflight {
        self.preflight
    }

    #[must_use]
    pub const fn provenance(&self) -> &ValidatedPackageProvenance {
        &self.provenance
    }

    #[must_use]
    pub fn running_generation(&self) -> RunningHandoffGeneration {
        debug_assert_eq!(self.lifecycle.phase(), HandoffPhase::OldRunning);
        RunningHandoffGeneration {
            build_identity: self.preflight.pair_build_identity,
            build_version: self.preflight.pair_build_version,
            capabilities: self.preflight.capabilities,
        }
    }

    /// Prepares the installed forward pair for one handoff attempt, borrowing
    /// this rollback authority.
    ///
    /// The forward candidate is whatever is installed now at the paths this
    /// authority was captured from: `splinterd` and the adjacent `splinterm`,
    /// where a package upgrade replaces them. Both are opened without following
    /// symlinks and must be root-owned, regular, executable, writable by no
    /// group or other user, and unchanged across their copy, exactly as the
    /// rollback capture requires. The package record beside them is located
    /// and loaded as at startup, then bound to the sealed forward pair and its
    /// authenticated forward preflight. Finally the forward, running, and
    /// rollback contracts must negotiate one handoff.
    ///
    /// The rollback pair is shared, not consumed: every failure, and dropping
    /// the result, leaves this authority intact for the next attempt.
    ///
    /// Sealed preflight runs and waits for two children, so async callers run
    /// this on a blocking thread. It is safe while the daemon serves (Plan
    /// 0061 D2); see the descriptor discipline in [`crate::handoff_preflight`].
    ///
    /// # Errors
    ///
    /// [`HandoffCoordinatorError::NoInstalledUpgrade`] when the installed pair
    /// is the running build, and
    /// [`HandoffCoordinatorError::NoInstalledPackageRecord`] when no package
    /// record is installed beside it. Otherwise fails closed on a missing, replaced,
    /// unsafe, or mutating forward source, a copy, digest, or seal failure, a
    /// failed or mismatched preflight, a package record that does not describe
    /// the sealed pair, or an incompatible contract.
    pub fn prepare_forward_installed(
        &self,
    ) -> Result<ForwardHandoffAuthority, HandoffCoordinatorError> {
        let daemon = &self.pair().daemon().source().path;
        let source = installed_source_pair(daemon)?;
        let record = load_installed_record(&package_provenance_path(daemon)?, PACKAGED_OWNER_UID)?;
        self.prepare_forward(
            &source,
            &record,
            ExecutableSnapshotPolicy {
                expected_owner_uid: PACKAGED_OWNER_UID,
            },
            |pair| preflight_sealed_pair(pair, PreflightGeneration::Forward),
        )
    }

    fn prepare_forward(
        &self,
        source: &ExecutableSourcePair,
        record: &PackageProvenanceRecord,
        policy: ExecutableSnapshotPolicy,
        preflight: impl FnOnce(&SealedExecutablePair) -> Result<ExecutablePairPreflight, PreflightError>,
    ) -> Result<ForwardHandoffAuthority, HandoffCoordinatorError> {
        if self.lifecycle.phase() != HandoffPhase::OldRunning {
            return Err(HandoffCoordinatorError::UnexpectedPhase);
        }
        let snapshots = HandoffExecutableSnapshots::materialize(source, &self.executables, policy)?;
        let (forward, running) = (snapshots.forward(), self.pair());
        // Exactly the running images: nothing to hand off to, so spawn nothing.
        if forward.daemon().source().sha256 == running.daemon().source().sha256
            && forward.client().source().sha256 == running.client().source().sha256
        {
            return Err(HandoffCoordinatorError::NoInstalledUpgrade);
        }
        let forward_preflight = preflight(forward)?;
        let provenance = record.verify_pair(forward, forward_preflight)?;
        // A different image of the same authenticated build, such as a local
        // rebuild of the running commit, is the running generation too.
        if forward_preflight.pair_build_identity == self.preflight.pair_build_identity {
            return Err(HandoffCoordinatorError::NoInstalledUpgrade);
        }
        let negotiated = negotiate_handoff(
            self.running_generation(),
            forward_preflight,
            self.preflight,
            &snapshots,
        )?;
        Ok(ForwardHandoffAuthority {
            snapshots,
            preflight: forward_preflight,
            provenance,
            negotiated,
        })
    }
}

/// One attempt's complete forward authority: the sealed installed forward
/// pair beside its own handle on the retained rollback pair, the forward pair's
/// authenticated preflight and package provenance, and the contract all three
/// generations negotiated.
///
/// Dropping it closes only this attempt's descriptors; the retained rollback
/// authority keeps its own.
#[derive(Debug)]
pub struct ForwardHandoffAuthority {
    snapshots: HandoffExecutableSnapshots,
    preflight: ExecutablePairPreflight,
    provenance: ValidatedPackageProvenance,
    negotiated: NegotiatedHandoff,
}

impl ForwardHandoffAuthority {
    #[must_use]
    pub const fn snapshots(&self) -> &HandoffExecutableSnapshots {
        &self.snapshots
    }

    #[must_use]
    pub const fn preflight(&self) -> ExecutablePairPreflight {
        self.preflight
    }

    #[must_use]
    pub const fn provenance(&self) -> &ValidatedPackageProvenance {
        &self.provenance
    }

    #[must_use]
    pub const fn negotiated(&self) -> NegotiatedHandoff {
        self.negotiated
    }
}

#[derive(Debug, Error)]
pub enum HandoffCoordinatorError {
    #[error("cannot resolve the running daemon executable: {0}")]
    CurrentExecutable(#[source] io::Error),
    #[error("the running daemon is not installed from a package")]
    NotPackaged,
    #[error("the running daemon is not an installed splinterd executable")]
    InvalidInstalledPair,
    #[error("another daemon handoff process guard is already active")]
    ProcessGuardActive,
    #[error("cannot inspect daemon process threads: {0}")]
    InspectProcessThreads(#[source] io::Error),
    #[error("daemon handoff requires exactly one thread, observed {observed}")]
    ProcessNotSingleThreaded { observed: usize },
    #[error("invalid daemon handoff phase transition")]
    InvalidPhaseTransition,
    #[error("daemon handoff authority is in an unexpected phase")]
    UnexpectedPhase,
    #[error("the installed daemon is the running build; there is nothing to hand off to")]
    NoInstalledUpgrade,
    #[error("the installed daemon has no package record, as after a dirty-tree install")]
    NoInstalledPackageRecord,
    #[error(transparent)]
    Compatibility(#[from] CompatibilityError),
    #[error(transparent)]
    Snapshot(#[from] ExecutableSnapshotError),
    #[error(transparent)]
    Preflight(#[from] PreflightError),
    #[error(transparent)]
    Provenance(#[from] PackageProvenanceError),
}

/// Loads the package record now installed beside the forward pair. An absent
/// record, which a dirty-tree install over a packaged daemon leaves, is named
/// as such rather than as a generic load failure.
fn load_installed_record(
    path: &Path,
    owner_uid: u32,
) -> Result<PackageProvenanceRecord, HandoffCoordinatorError> {
    match PackageProvenanceRecord::load(path, owner_uid) {
        Err(PackageProvenanceError::Open { source, .. })
            if source.kind() == io::ErrorKind::NotFound =>
        {
            Err(HandoffCoordinatorError::NoInstalledPackageRecord)
        }
        record => Ok(record?),
    }
}

fn installed_source_pair(daemon: &Path) -> Result<ExecutableSourcePair, HandoffCoordinatorError> {
    if daemon.file_name().and_then(|name| name.to_str()) != Some("splinterd") {
        return Err(HandoffCoordinatorError::InvalidInstalledPair);
    }
    let client = daemon
        .parent()
        .ok_or(HandoffCoordinatorError::InvalidInstalledPair)?
        .join("splinterm");
    ExecutableSourcePair::new(PathBuf::from(daemon), client).map_err(Into::into)
}

fn prepare_with_preflight(
    executables: RetainedRollbackExecutables,
    record: &PackageProvenanceRecord,
    preflight: impl FnOnce(&SealedExecutablePair) -> Result<ExecutablePairPreflight, PreflightError>,
) -> Result<RollbackHandoffAuthority, HandoffCoordinatorError> {
    let preflight = preflight(executables.pair())?;
    let provenance = record.verify_pair(executables.pair(), preflight)?;
    Ok(RollbackHandoffAuthority {
        executables,
        preflight,
        provenance,
        lifecycle: HandoffLifecycle::old_running(),
    })
}

/// Serializes unit tests that hold the process-global handoff guard.
#[cfg(test)]
pub(crate) static PROCESS_GUARD_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use std::{
        fmt::Write as _,
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use sha2::{Digest, Sha256};

    use crate::{
        handoff_compatibility::{
            BuildVersion, ExecutablePairBinding, HandoffCapabilities, VersionRange,
        },
        package_provenance::{BuildProvenanceKind, derive_pair_build_identity},
    };

    use super::*;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);
    const COMMIT: &str = "1234567890abcdef1234567890abcdef12345678";
    const VERSION: &str = "0.2.3-beta1";
    const FORWARD_COMMIT: &str = "abcdef1234567890abcdef1234567890abcdef12";
    const FORWARD_VERSION: &str = "0.2.4";

    struct Fixture {
        directory: PathBuf,
        source: ExecutableSourcePair,
        record: PackageProvenanceRecord,
        capabilities: HandoffCapabilities,
    }

    impl Fixture {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let directory = std::env::temp_dir().join(format!(
                "splinterd-rollback-authority-{}-{unique}-{sequence}",
                std::process::id()
            ));
            let pair_directory = directory.join("pair");
            fs::create_dir_all(&pair_directory).unwrap();
            let daemon = pair_directory.join("splinterd");
            let client = pair_directory.join("splinterm");
            fs::write(&daemon, format!("daemon-{sequence}")).unwrap();
            fs::write(&client, format!("client-{sequence}")).unwrap();
            for path in [&daemon, &client] {
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }
            let source = ExecutableSourcePair::new(daemon, client).unwrap();
            let capabilities = capabilities();
            let record = write_record(
                &directory.join("package-provenance.json"),
                &source,
                COMMIT,
                VERSION,
                capabilities,
            );
            Self {
                directory,
                source,
                record,
                capabilities,
            }
        }

        fn authority(&self) -> RollbackHandoffAuthority {
            prepare_with_preflight(self.retained(), &self.record, |pair| Ok(self.report(pair)))
                .unwrap()
        }

        /// Replaces the running pair in place, as a package upgrade does, and
        /// returns the new pair's record.
        fn install(
            &self,
            images: (&[u8], &[u8]),
            commit: &str,
            version: &str,
        ) -> PackageProvenanceRecord {
            fs::write(self.source.daemon(), images.0).unwrap();
            fs::write(self.source.client(), images.1).unwrap();
            let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            write_record(
                &self
                    .directory
                    .join(format!("forward-provenance-{sequence}.json")),
                &self.source,
                commit,
                version,
                self.capabilities,
            )
        }

        fn forward_report(&self, pair: &SealedExecutablePair) -> ExecutablePairPreflight {
            ExecutablePairPreflight {
                pair_build_identity: derive_pair_build_identity(
                    FORWARD_COMMIT,
                    BuildProvenanceKind::LocalCommittedHead,
                    FORWARD_VERSION,
                    self.capabilities,
                )
                .unwrap(),
                pair_build_version: BuildVersion::new(0, 2, 4),
                capabilities: self.capabilities,
                snapshots: ExecutablePairBinding::from_snapshots(pair),
            }
        }

        fn retained(&self) -> RetainedRollbackExecutables {
            RetainedRollbackExecutables::capture_declared_for_test(
                &self.source,
                ExecutableSnapshotPolicy {
                    expected_owner_uid: rustix::process::geteuid().as_raw(),
                },
            )
            .unwrap()
        }

        fn report(&self, pair: &SealedExecutablePair) -> ExecutablePairPreflight {
            ExecutablePairPreflight {
                pair_build_identity: derive_pair_build_identity(
                    COMMIT,
                    BuildProvenanceKind::LocalCommittedHead,
                    VERSION,
                    self.capabilities,
                )
                .unwrap(),
                pair_build_version: BuildVersion::new(0, 2, 3),
                capabilities: self.capabilities,
                snapshots: ExecutablePairBinding::from_snapshots(pair),
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn capabilities() -> HandoffCapabilities {
        let exact = |value| VersionRange::new(value, value).unwrap();
        HandoffCapabilities {
            private_protocol: exact(37),
            handoff_protocol: exact(1),
            terminal_checkpoint: exact(1),
            descriptor_manifest: exact(1),
        }
    }

    fn policy() -> ExecutableSnapshotPolicy {
        ExecutableSnapshotPolicy {
            expected_owner_uid: rustix::process::geteuid().as_raw(),
        }
    }

    /// Writes and loads a package record for the pair's current images.
    fn write_record(
        path: &Path,
        source: &ExecutableSourcePair,
        commit: &str,
        version: &str,
        capabilities: HandoffCapabilities,
    ) -> PackageProvenanceRecord {
        let identity = derive_pair_build_identity(
            commit,
            BuildProvenanceKind::LocalCommittedHead,
            version,
            capabilities,
        )
        .unwrap();
        let digest = |path: &Path| hex(Sha256::digest(fs::read(path).unwrap()).into());
        let record_json = serde_json::json!({
            "schema": 1,
            "package": "splinterm",
            "architecture": "x86_64",
            "provenance": "local-committed-head",
            "source_commit": commit,
            "build_version": version,
            "pair_build_identity": hex(identity.as_bytes()),
            "daemon_sha256": digest(source.daemon()),
            "client_sha256": digest(source.client()),
        });
        fs::write(path, serde_json::to_vec(&record_json).unwrap()).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
        PackageProvenanceRecord::load(path, rustix::process::geteuid().as_raw()).unwrap()
    }

    fn hex(bytes: [u8; 32]) -> String {
        bytes
            .iter()
            .fold(String::with_capacity(64), |mut output, byte| {
                write!(output, "{byte:02x}").unwrap();
                output
            })
    }

    #[test]
    fn retains_one_exact_verified_running_generation() {
        let fixture = Fixture::new();
        let authority = prepare_with_preflight(fixture.retained(), &fixture.record, |pair| {
            Ok(fixture.report(pair))
        })
        .unwrap();

        assert_eq!(
            authority.preflight().snapshots,
            ExecutablePairBinding::from_snapshots(authority.pair())
        );
        assert_eq!(authority.provenance().source_commit, COMMIT);
        assert_eq!(
            authority.running_generation().build_identity,
            authority.preflight().pair_build_identity
        );
    }

    #[test]
    fn preflight_failure_releases_partial_authority() {
        let fixture = Fixture::new();
        assert!(matches!(
            prepare_with_preflight(fixture.retained(), &fixture.record, |_| {
                Err(PreflightError::InvalidFrame)
            }),
            Err(HandoffCoordinatorError::Preflight(
                PreflightError::InvalidFrame
            ))
        ));
    }

    #[test]
    fn provenance_mismatch_releases_partial_authority() {
        let fixture = Fixture::new();
        assert!(matches!(
            prepare_with_preflight(fixture.retained(), &fixture.record, |pair| {
                let mut report = fixture.report(pair);
                report.pair_build_version.patch += 1;
                Ok(report)
            }),
            Err(HandoffCoordinatorError::Provenance(
                PackageProvenanceError::BuildMismatch
            ))
        ));
    }

    #[test]
    fn forward_preparation_binds_the_installed_upgrade_beside_the_shared_rollback_pair() {
        let fixture = Fixture::new();
        let authority = fixture.authority();
        let record = fixture.install(
            (b"forward-daemon", b"forward-client"),
            FORWARD_COMMIT,
            FORWARD_VERSION,
        );
        let forward = authority
            .prepare_forward(&fixture.source, &record, policy(), |pair| {
                Ok(fixture.forward_report(pair))
            })
            .unwrap();

        assert_eq!(forward.provenance().source_commit, FORWARD_COMMIT);
        assert_eq!(
            forward.preflight().snapshots,
            ExecutablePairBinding::from_snapshots(forward.snapshots().forward())
        );
        assert_eq!(
            forward.snapshots().forward().daemon().source().sha256,
            <[u8; 32]>::from(Sha256::digest(b"forward-daemon"))
        );
        // The shared rollback pair is the exact pair startup preflighted.
        assert_eq!(
            ExecutablePairBinding::from_snapshots(forward.snapshots().rollback()),
            authority.preflight().snapshots
        );
        let negotiated = forward.negotiated();
        assert_eq!(
            negotiated.forward_build_identity,
            forward.preflight().pair_build_identity
        );
        assert_eq!(
            negotiated.rollback_build_identity,
            authority.running_generation().build_identity
        );
    }

    #[test]
    fn a_failed_forward_attempt_keeps_rollback_authority_for_the_next() {
        let fixture = Fixture::new();
        let authority = fixture.authority();
        let rollback = authority.preflight().snapshots;
        let forward_report =
            |pair: &SealedExecutablePair| Ok::<_, PreflightError>(fixture.forward_report(pair));

        // A record describing other images than the installed pair.
        let record = fixture.install(
            (b"forward-daemon", b"forward-client"),
            FORWARD_COMMIT,
            FORWARD_VERSION,
        );
        fs::write(fixture.source.daemon(), b"substituted-daemon").unwrap();
        assert!(matches!(
            authority.prepare_forward(&fixture.source, &record, policy(), forward_report),
            Err(HandoffCoordinatorError::Provenance(
                PackageProvenanceError::ExecutableMismatch
            ))
        ));
        // A sealed preflight failure.
        let record = fixture.install(
            (b"forward-daemon", b"forward-client"),
            FORWARD_COMMIT,
            FORWARD_VERSION,
        );
        assert!(matches!(
            authority.prepare_forward(&fixture.source, &record, policy(), |_| {
                Err(PreflightError::Timeout)
            }),
            Err(HandoffCoordinatorError::Preflight(PreflightError::Timeout))
        ));
        // A forward client missing mid-upgrade.
        let displaced = fixture.source.client().with_file_name("displaced");
        fs::rename(fixture.source.client(), &displaced).unwrap();
        assert!(matches!(
            authority.prepare_forward(&fixture.source, &record, policy(), forward_report),
            Err(HandoffCoordinatorError::Snapshot(
                ExecutableSnapshotError::OpenSource { .. }
            ))
        ));
        fs::rename(&displaced, fixture.source.client()).unwrap();

        // The next attempt succeeds with the same rollback authority, twice.
        for _ in 0..2 {
            let forward = authority
                .prepare_forward(&fixture.source, &record, policy(), forward_report)
                .unwrap();
            assert_eq!(
                ExecutablePairBinding::from_snapshots(forward.snapshots().rollback()),
                rollback
            );
        }
        assert_eq!(
            ExecutablePairBinding::from_snapshots(authority.pair()),
            rollback
        );
    }

    #[test]
    fn the_running_build_installed_again_is_no_upgrade() {
        let fixture = Fixture::new();
        let authority = fixture.authority();
        // The untouched pair is refused before any preflight child runs.
        assert!(matches!(
            authority.prepare_forward(&fixture.source, &fixture.record, policy(), |_| {
                panic!("no preflight runs for the running images")
            }),
            Err(HandoffCoordinatorError::NoInstalledUpgrade)
        ));
        // Different images of the same authenticated build are refused too.
        let record = fixture.install((b"rebuilt-daemon", b"rebuilt-client"), COMMIT, VERSION);
        assert!(matches!(
            authority.prepare_forward(&fixture.source, &record, policy(), |pair| {
                Ok(fixture.report(pair))
            }),
            Err(HandoffCoordinatorError::NoInstalledUpgrade)
        ));
    }

    #[test]
    fn a_missing_installed_record_is_named_as_such() {
        let fixture = Fixture::new();
        assert!(matches!(
            load_installed_record(
                &fixture.directory.join("absent-provenance.json"),
                rustix::process::geteuid().as_raw(),
            ),
            Err(HandoffCoordinatorError::NoInstalledPackageRecord)
        ));
        assert!(
            load_installed_record(
                &fixture.directory.join("package-provenance.json"),
                rustix::process::geteuid().as_raw(),
            )
            .is_ok()
        );
    }

    #[test]
    fn installed_pair_requires_exact_daemon_name() {
        assert!(matches!(
            installed_source_pair(Path::new("/usr/bin/not-splinterd")),
            Err(HandoffCoordinatorError::InvalidInstalledPair)
        ));
        let pair = installed_source_pair(Path::new("/usr/bin/splinterd")).unwrap();
        assert_eq!(pair.daemon(), Path::new("/usr/bin/splinterd"));
        assert_eq!(pair.client(), Path::new("/usr/bin/splinterm"));
    }

    #[test]
    fn lifecycle_rejects_skips_and_rollback_after_commit() {
        let mut old = HandoffLifecycle::old_running();
        assert!(matches!(
            old.transition(HandoffPhase::RuntimeStopped),
            Err(HandoffCoordinatorError::InvalidPhaseTransition)
        ));
        for phase in [
            HandoffPhase::Preparing,
            HandoffPhase::Quiescing,
            HandoffPhase::RuntimeStopped,
            HandoffPhase::InheritancePrepared,
        ] {
            old.transition(phase).unwrap();
        }
        for skipped in [
            HandoffPhase::CandidateClaimed,
            HandoffPhase::AdoptionStaged,
            HandoffPhase::Committed,
        ] {
            assert!(matches!(
                old.transition(skipped),
                Err(HandoffCoordinatorError::InvalidPhaseTransition)
            ));
        }
        assert_eq!(old.phase(), HandoffPhase::InheritancePrepared);
        old.transition(HandoffPhase::OldRunning).unwrap();

        let mut candidate = HandoffLifecycle::candidate_claimed();
        assert!(matches!(
            candidate.transition(HandoffPhase::Committed),
            Err(HandoffCoordinatorError::InvalidPhaseTransition)
        ));
        candidate.transition(HandoffPhase::AdoptionStaged).unwrap();
        candidate.transition(HandoffPhase::Committed).unwrap();
        assert!(matches!(
            candidate.transition(HandoffPhase::RollingBack),
            Err(HandoffCoordinatorError::InvalidPhaseTransition)
        ));
        candidate.transition(HandoffPhase::Published).unwrap();
        candidate.transition(HandoffPhase::Failed).unwrap();
        assert!(matches!(
            candidate.transition(HandoffPhase::OldRunning),
            Err(HandoffCoordinatorError::InvalidPhaseTransition)
        ));

        let mut rolling_back = HandoffLifecycle::candidate_claimed();
        rolling_back.transition(HandoffPhase::RollingBack).unwrap();
        assert!(matches!(
            rolling_back.transition(HandoffPhase::AdoptionStaged),
            Err(HandoffCoordinatorError::InvalidPhaseTransition)
        ));
    }

    #[test]
    fn process_guard_requires_one_thread_and_one_owner() {
        let _serial = PROCESS_GUARD_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(matches!(
            HandoffProcessGuard::acquire_with(|| Ok(2)),
            Err(HandoffCoordinatorError::ProcessNotSingleThreaded { observed: 2 })
        ));
        assert!(matches!(
            HandoffProcessGuard::acquire_with(|| Err(io::Error::other("procfs"))),
            Err(HandoffCoordinatorError::InspectProcessThreads(_))
        ));
        let guard = HandoffProcessGuard::acquire_with(|| Ok(1)).unwrap();
        assert!(matches!(
            HandoffProcessGuard::acquire_with(|| Ok(1)),
            Err(HandoffCoordinatorError::ProcessGuardActive)
        ));
        drop(guard);

        let reacquired = HandoffProcessGuard::acquire_with(|| Ok(1)).unwrap();
        drop(reacquired);
    }

    #[test]
    fn process_guard_retries_until_exiting_threads_are_gone() {
        let _serial = PROCESS_GUARD_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut observations = vec![1, 2, 3];
        let guard = HandoffProcessGuard::acquire_until_with(
            Instant::now() + Duration::from_secs(5),
            || Ok(observations.pop().expect("no observation after success")),
        )
        .unwrap();
        assert_eq!(observations, Vec::<usize>::new());
        assert!(matches!(
            HandoffProcessGuard::acquire_with(|| Ok(1)),
            Err(HandoffCoordinatorError::ProcessGuardActive)
        ));
        drop(guard);
    }

    #[test]
    fn process_guard_retry_times_out_with_the_last_observation() {
        let _serial = PROCESS_GUARD_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let started = Instant::now();
        let mut calls = 0_usize;
        let result =
            HandoffProcessGuard::acquire_until_with(started + Duration::from_millis(30), || {
                calls += 1;
                Ok(1 + calls)
            });
        let Err(HandoffCoordinatorError::ProcessNotSingleThreaded { observed }) = result else {
            panic!("expected a retry timeout, got {result:?}");
        };
        assert!(calls >= 2, "retried only {calls} times");
        assert_eq!(observed, 1 + calls);
        assert!(started.elapsed() >= Duration::from_millis(30));
        // A timeout leaves the guard free.
        drop(HandoffProcessGuard::acquire_with(|| Ok(1)).unwrap());

        // A passed deadline still observes exactly once.
        let mut calls = 0_usize;
        let passed = Instant::now();
        assert!(matches!(
            HandoffProcessGuard::acquire_until_with(passed, || {
                calls += 1;
                Ok(2)
            }),
            Err(HandoffCoordinatorError::ProcessNotSingleThreaded { observed: 2 })
        ));
        assert_eq!(calls, 1);
        drop(HandoffProcessGuard::acquire_until_with(passed, || Ok(1)).unwrap());
    }

    #[test]
    fn process_guard_retry_does_not_wait_out_a_guard_or_procfs_failure() {
        let _serial = PROCESS_GUARD_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let deadline = Instant::now() + Duration::from_secs(5);
        let guard = HandoffProcessGuard::acquire_with(|| Ok(1)).unwrap();
        let mut calls = 0_usize;
        assert!(matches!(
            HandoffProcessGuard::acquire_until_with(deadline, || {
                calls += 1;
                Ok(1)
            }),
            Err(HandoffCoordinatorError::ProcessGuardActive)
        ));
        assert_eq!(calls, 0);
        drop(guard);

        assert!(matches!(
            HandoffProcessGuard::acquire_until_with(deadline, || {
                calls += 1;
                Err(io::Error::other("procfs"))
            }),
            Err(HandoffCoordinatorError::InspectProcessThreads(_))
        ));
        assert_eq!(calls, 1);
        assert!(Instant::now() < deadline);
        drop(HandoffProcessGuard::acquire_with(|| Ok(1)).unwrap());
    }

    #[test]
    fn development_daemon_has_no_packaged_authority() {
        // Test binaries run from the build tree, outside every packaged prefix.
        assert!(matches!(
            RollbackHandoffAuthority::prepare_installed(),
            Err(HandoffCoordinatorError::NotPackaged)
        ));
    }
}
