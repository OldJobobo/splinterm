//! Startup retention of the running packaged daemon/client rollback authority.
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
};

use thiserror::Error;

use crate::{
    executable_snapshot::{
        ExecutableSnapshotError, ExecutableSnapshotPolicy, ExecutableSourcePair,
        PACKAGED_OWNER_UID, RetainedRollbackExecutables, SealedExecutablePair,
    },
    handoff_compatibility::{ExecutablePairPreflight, RunningHandoffGeneration},
    handoff_preflight::{PreflightError, PreflightGeneration, preflight_sealed_pair},
    package_provenance::{
        PackageProvenanceError, PackageProvenanceRecord, ValidatedPackageProvenance,
        package_provenance_path,
    },
};

static HANDOFF_PROCESS_GUARD_ACTIVE: AtomicBool = AtomicBool::new(false);

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
    /// This must run synchronously during startup before Tokio, listeners,
    /// connection tasks, or live PTY runtimes can create or close descriptors
    /// concurrently with sealed preflight's descriptor audit. The exclusive
    /// [`HandoffProcessGuard`] enforces that requirement from the first
    /// executable capture through preflight and provenance binding.
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
    #[error(transparent)]
    Snapshot(#[from] ExecutableSnapshotError),
    #[error(transparent)]
    Preflight(#[from] PreflightError),
    #[error(transparent)]
    Provenance(#[from] PackageProvenanceError),
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
            let source = ExecutableSourcePair::new(daemon.clone(), client.clone()).unwrap();
            let capabilities = capabilities();
            let identity = derive_pair_build_identity(
                COMMIT,
                BuildProvenanceKind::LocalCommittedHead,
                VERSION,
                capabilities,
            )
            .unwrap();
            let record_path = directory.join("package-provenance.json");
            let record_json = serde_json::json!({
                "schema": 1,
                "package": "splinterm",
                "architecture": "x86_64",
                "provenance": "local-committed-head",
                "source_commit": COMMIT,
                "build_version": VERSION,
                "pair_build_identity": hex(identity.as_bytes()),
                "daemon_sha256": hex(Sha256::digest(fs::read(daemon).unwrap()).into()),
                "client_sha256": hex(Sha256::digest(fs::read(client).unwrap()).into()),
            });
            fs::write(&record_path, serde_json::to_vec(&record_json).unwrap()).unwrap();
            fs::set_permissions(&record_path, fs::Permissions::from_mode(0o644)).unwrap();
            let record =
                PackageProvenanceRecord::load(&record_path, rustix::process::geteuid().as_raw())
                    .unwrap();
            Self {
                directory,
                source,
                record,
                capabilities,
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
    fn development_daemon_has_no_packaged_authority() {
        // Test binaries run from the build tree, outside every packaged prefix.
        assert!(matches!(
            RollbackHandoffAuthority::prepare_installed(),
            Err(HandoffCoordinatorError::NotPackaged)
        ));
    }
}
