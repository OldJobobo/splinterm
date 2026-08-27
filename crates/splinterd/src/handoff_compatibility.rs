//! Pure compatibility negotiation for sealed daemon/client handoff snapshots.
//!
//! Transport and authenticity of preflight reports remain coordinator concerns.

use crate::executable_snapshot::{
    ExecutableSnapshotIdentity, HandoffExecutableSnapshots, SealedExecutablePair,
};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VersionRange {
    minimum: u16,
    maximum: u16,
}

impl VersionRange {
    /// Creates one nonzero inclusive compatibility range.
    ///
    /// # Errors
    ///
    /// Rejects zero endpoints and reversed ranges.
    pub fn new(minimum: u16, maximum: u16) -> Result<Self, CompatibilityError> {
        if minimum == 0 || maximum == 0 || minimum > maximum {
            return Err(CompatibilityError::InvalidRange);
        }
        Ok(Self { minimum, maximum })
    }

    #[must_use]
    pub const fn minimum(self) -> u16 {
        self.minimum
    }

    #[must_use]
    pub const fn maximum(self) -> u16 {
        self.maximum
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BuildIdentity([u8; 32]);

impl BuildIdentity {
    /// Creates one nonzero immutable build identity.
    ///
    /// # Errors
    ///
    /// Rejects the reserved all-zero identity.
    pub fn new(bytes: [u8; 32]) -> Result<Self, CompatibilityError> {
        if bytes == [0; 32] {
            return Err(CompatibilityError::InvalidBuildIdentity);
        }
        Ok(Self(bytes))
    }

    #[must_use]
    pub const fn as_bytes(self) -> [u8; 32] {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct BuildVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl BuildVersion {
    #[must_use]
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HandoffCapabilities {
    pub private_protocol: VersionRange,
    pub handoff_protocol: VersionRange,
    pub terminal_checkpoint: VersionRange,
    pub descriptor_manifest: VersionRange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RunningHandoffGeneration {
    pub build_identity: BuildIdentity,
    pub build_version: BuildVersion,
    pub capabilities: HandoffCapabilities,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutablePairBinding {
    pub daemon: ExecutableSnapshotIdentity,
    pub client: ExecutableSnapshotIdentity,
}

impl ExecutablePairBinding {
    #[must_use]
    pub fn from_snapshots(pair: &SealedExecutablePair) -> Self {
        Self {
            daemon: pair.daemon().snapshot(),
            client: pair.client().snapshot(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutablePairPreflight {
    /// One shared identity for the daemon/client build pair. A report cannot
    /// independently select a newer client identity.
    pub pair_build_identity: BuildIdentity,
    pub pair_build_version: BuildVersion,
    pub capabilities: HandoffCapabilities,
    pub snapshots: ExecutablePairBinding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NegotiatedHandoff {
    pub forward_build_identity: BuildIdentity,
    pub forward_build_version: BuildVersion,
    pub rollback_build_identity: BuildIdentity,
    pub rollback_build_version: BuildVersion,
    pub private_protocol: u16,
    pub handoff_protocol: u16,
    pub terminal_checkpoint: u16,
    pub descriptor_manifest: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompatibilityDimension {
    PrivateProtocol,
    HandoffProtocol,
    TerminalCheckpoint,
    DescriptorManifest,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CompatibilityError {
    #[error("compatibility range is zero or reversed")]
    InvalidRange,
    #[error("build identity is reserved or invalid")]
    InvalidBuildIdentity,
    #[error("forward preflight does not bind the exact sealed snapshot pair")]
    ForwardSnapshotMismatch,
    #[error("rollback preflight does not bind the exact sealed snapshot pair")]
    RollbackSnapshotMismatch,
    #[error("rollback preflight does not identify the running generation")]
    RollbackGenerationMismatch,
    #[error("no common {0:?} version exists across running, forward, and rollback generations")]
    Incompatible(CompatibilityDimension),
}

/// Negotiates one complete four-dimensional handoff contract.
///
/// The preflight reports must have been obtained from the exact sealed snapshots
/// by a future authenticated offline-preflight boundary. This function validates
/// their snapshot binding and compatibility but does not authenticate transport.
///
/// # Errors
///
/// Rejects snapshot substitution, a rollback pair not matching the running
/// generation, or a missing common version in any required dimension.
pub fn negotiate_handoff(
    running: RunningHandoffGeneration,
    forward: ExecutablePairPreflight,
    rollback: ExecutablePairPreflight,
    snapshots: &HandoffExecutableSnapshots,
) -> Result<NegotiatedHandoff, CompatibilityError> {
    let expected_forward = ExecutablePairBinding::from_snapshots(snapshots.forward());
    if forward.snapshots != expected_forward {
        return Err(CompatibilityError::ForwardSnapshotMismatch);
    }
    let expected_rollback = ExecutablePairBinding::from_snapshots(snapshots.rollback());
    if rollback.snapshots != expected_rollback {
        return Err(CompatibilityError::RollbackSnapshotMismatch);
    }
    if rollback.pair_build_identity != running.build_identity
        || rollback.pair_build_version != running.build_version
    {
        return Err(CompatibilityError::RollbackGenerationMismatch);
    }

    let private_protocol = highest_common(
        running.capabilities.private_protocol,
        forward.capabilities.private_protocol,
        rollback.capabilities.private_protocol,
    )
    .ok_or(CompatibilityError::Incompatible(
        CompatibilityDimension::PrivateProtocol,
    ))?;
    let handoff_protocol = highest_common(
        running.capabilities.handoff_protocol,
        forward.capabilities.handoff_protocol,
        rollback.capabilities.handoff_protocol,
    )
    .ok_or(CompatibilityError::Incompatible(
        CompatibilityDimension::HandoffProtocol,
    ))?;
    let terminal_checkpoint = highest_common(
        running.capabilities.terminal_checkpoint,
        forward.capabilities.terminal_checkpoint,
        rollback.capabilities.terminal_checkpoint,
    )
    .ok_or(CompatibilityError::Incompatible(
        CompatibilityDimension::TerminalCheckpoint,
    ))?;
    let descriptor_manifest = highest_common(
        running.capabilities.descriptor_manifest,
        forward.capabilities.descriptor_manifest,
        rollback.capabilities.descriptor_manifest,
    )
    .ok_or(CompatibilityError::Incompatible(
        CompatibilityDimension::DescriptorManifest,
    ))?;

    Ok(NegotiatedHandoff {
        forward_build_identity: forward.pair_build_identity,
        forward_build_version: forward.pair_build_version,
        rollback_build_identity: rollback.pair_build_identity,
        rollback_build_version: rollback.pair_build_version,
        private_protocol,
        handoff_protocol,
        terminal_checkpoint,
        descriptor_manifest,
    })
}

fn highest_common(
    running: VersionRange,
    forward: VersionRange,
    rollback: VersionRange,
) -> Option<u16> {
    let minimum = running.minimum.max(forward.minimum).max(rollback.minimum);
    let maximum = running.maximum.min(forward.maximum).min(rollback.maximum);
    (minimum <= maximum).then_some(maximum)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::executable_snapshot::{
        ExecutableSnapshotPolicy, ExecutableSourcePair, HandoffExecutableSnapshots,
    };

    use super::*;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct Fixture {
        directory: PathBuf,
        snapshots: HandoffExecutableSnapshots,
    }

    impl Fixture {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let directory = std::env::temp_dir().join(format!(
                "splinterd-handoff-compatibility-{}-{unique}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&directory).unwrap();
            let forward = pair(&directory, "forward");
            let rollback = pair(&directory, "rollback");
            let snapshots = HandoffExecutableSnapshots::materialize(
                &forward,
                &rollback,
                ExecutableSnapshotPolicy {
                    expected_owner_uid: rustix::process::geteuid().as_raw(),
                },
            )
            .unwrap();
            Self {
                directory,
                snapshots,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn pair(root: &Path, generation: &str) -> ExecutableSourcePair {
        let directory = root.join(generation);
        fs::create_dir(&directory).unwrap();
        let daemon = directory.join("splinterd");
        let client = directory.join("splinterm");
        for (path, bytes) in [
            (&daemon, b"daemon".as_slice()),
            (&client, b"client".as_slice()),
        ] {
            fs::write(path, bytes).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        ExecutableSourcePair::new(daemon, client).unwrap()
    }

    fn range(minimum: u16, maximum: u16) -> VersionRange {
        VersionRange::new(minimum, maximum).unwrap()
    }

    fn capabilities(offset: u16) -> HandoffCapabilities {
        HandoffCapabilities {
            private_protocol: range(1 + offset, 6 + offset),
            handoff_protocol: range(2 + offset, 7 + offset),
            terminal_checkpoint: range(3 + offset, 8 + offset),
            descriptor_manifest: range(4 + offset, 9 + offset),
        }
    }

    fn identity(byte: u8) -> BuildIdentity {
        BuildIdentity::new([byte; 32]).unwrap()
    }

    fn reports(
        fixture: &Fixture,
    ) -> (
        RunningHandoffGeneration,
        ExecutablePairPreflight,
        ExecutablePairPreflight,
    ) {
        let running = RunningHandoffGeneration {
            build_identity: identity(1),
            build_version: BuildVersion::new(0, 2, 1),
            capabilities: capabilities(0),
        };
        let forward = ExecutablePairPreflight {
            pair_build_identity: identity(2),
            pair_build_version: BuildVersion::new(0, 2, 2),
            capabilities: capabilities(2),
            snapshots: ExecutablePairBinding::from_snapshots(fixture.snapshots.forward()),
        };
        let rollback = ExecutablePairPreflight {
            pair_build_identity: running.build_identity,
            pair_build_version: running.build_version,
            capabilities: capabilities(1),
            snapshots: ExecutablePairBinding::from_snapshots(fixture.snapshots.rollback()),
        };
        (running, forward, rollback)
    }

    fn mutate_snapshot_identity(identity: &mut ExecutableSnapshotIdentity, field: usize) {
        match field {
            0 => identity.device += 1,
            1 => identity.inode += 1,
            2 => identity.size += 1,
            3 => identity.sha256[0] ^= 1,
            4 => identity.seals.remove(rustix::fs::SealFlags::WRITE),
            _ => unreachable!(),
        }
    }

    #[test]
    fn selects_highest_common_version_in_every_dimension() {
        let fixture = Fixture::new();
        let (running, forward, rollback) = reports(&fixture);
        let negotiated = negotiate_handoff(running, forward, rollback, &fixture.snapshots).unwrap();
        assert_eq!(negotiated.private_protocol, 6);
        assert_eq!(negotiated.handoff_protocol, 7);
        assert_eq!(negotiated.terminal_checkpoint, 8);
        assert_eq!(negotiated.descriptor_manifest, 9);
        assert_eq!(negotiated.forward_build_identity, identity(2));
        assert_eq!(negotiated.rollback_build_identity, identity(1));
    }

    #[test]
    fn rejects_zero_reversed_ranges_and_zero_build_identity() {
        assert_eq!(
            VersionRange::new(0, 1),
            Err(CompatibilityError::InvalidRange)
        );
        assert_eq!(
            VersionRange::new(2, 1),
            Err(CompatibilityError::InvalidRange)
        );
        assert_eq!(
            BuildIdentity::new([0; 32]),
            Err(CompatibilityError::InvalidBuildIdentity)
        );
    }

    #[test]
    fn rejects_disjointness_in_each_required_dimension() {
        for dimension in [
            CompatibilityDimension::PrivateProtocol,
            CompatibilityDimension::HandoffProtocol,
            CompatibilityDimension::TerminalCheckpoint,
            CompatibilityDimension::DescriptorManifest,
        ] {
            let fixture = Fixture::new();
            let (running, mut forward, rollback) = reports(&fixture);
            let disjoint = range(20, 21);
            match dimension {
                CompatibilityDimension::PrivateProtocol => {
                    forward.capabilities.private_protocol = disjoint;
                }
                CompatibilityDimension::HandoffProtocol => {
                    forward.capabilities.handoff_protocol = disjoint;
                }
                CompatibilityDimension::TerminalCheckpoint => {
                    forward.capabilities.terminal_checkpoint = disjoint;
                }
                CompatibilityDimension::DescriptorManifest => {
                    forward.capabilities.descriptor_manifest = disjoint;
                }
            }
            assert_eq!(
                negotiate_handoff(running, forward, rollback, &fixture.snapshots),
                Err(CompatibilityError::Incompatible(dimension))
            );
        }
    }

    #[test]
    fn rejects_forward_rollback_and_role_snapshot_substitution() {
        let fixture = Fixture::new();
        let (running, mut forward, mut rollback) = reports(&fixture);
        forward.snapshots = ExecutablePairBinding::from_snapshots(fixture.snapshots.rollback());
        assert_eq!(
            negotiate_handoff(running, forward, rollback, &fixture.snapshots),
            Err(CompatibilityError::ForwardSnapshotMismatch)
        );

        let (_, forward, _) = reports(&fixture);
        rollback.snapshots = ExecutablePairBinding {
            daemon: rollback.snapshots.client,
            client: rollback.snapshots.daemon,
        };
        assert_eq!(
            negotiate_handoff(running, forward, rollback, &fixture.snapshots),
            Err(CompatibilityError::RollbackSnapshotMismatch)
        );

        for target in 0..4 {
            for field in 0..5 {
                let (_, mut forward, mut rollback) = reports(&fixture);
                let (identity, expected) = match target {
                    0 => (
                        &mut forward.snapshots.daemon,
                        CompatibilityError::ForwardSnapshotMismatch,
                    ),
                    1 => (
                        &mut forward.snapshots.client,
                        CompatibilityError::ForwardSnapshotMismatch,
                    ),
                    2 => (
                        &mut rollback.snapshots.daemon,
                        CompatibilityError::RollbackSnapshotMismatch,
                    ),
                    3 => (
                        &mut rollback.snapshots.client,
                        CompatibilityError::RollbackSnapshotMismatch,
                    ),
                    _ => unreachable!(),
                };
                mutate_snapshot_identity(identity, field);
                assert_eq!(
                    negotiate_handoff(running, forward, rollback, &fixture.snapshots),
                    Err(expected)
                );
            }
        }
    }

    #[test]
    fn rollback_pair_must_exactly_identify_running_generation() {
        let fixture = Fixture::new();
        let (running, forward, mut rollback) = reports(&fixture);
        rollback.pair_build_identity = identity(3);
        assert_eq!(
            negotiate_handoff(running, forward, rollback, &fixture.snapshots),
            Err(CompatibilityError::RollbackGenerationMismatch)
        );

        let (_, forward, mut rollback) = reports(&fixture);
        rollback.pair_build_version = BuildVersion::new(0, 2, 0);
        assert_eq!(
            negotiate_handoff(running, forward, rollback, &fixture.snapshots),
            Err(CompatibilityError::RollbackGenerationMismatch)
        );
    }
}
