//! Root-trust package provenance for sealed daemon/client pairs.
//!
//! This proves local package coherence against owner-controlled metadata. It is
//! not a signature or an attestation against root compromise.
//!
//! A `nix-store` record is weaker still: any user the Nix daemon serves can
//! realise a root-owned store output with a self-consistent record claiming any
//! commit. The record binds the hashes of the binaries beside it, but the
//! commit is asserted by whoever built the output. Authority to hand off to a
//! store generation must come from how the forward path is chosen (the system
//! or user profile), never from store ownership alone.

use std::{
    ffi::OsStr,
    fs::{File, Metadata},
    io::{self, Read},
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Component, Path, PathBuf},
};

use rustix::fs::{CWD, Mode, OFlags, ResolveFlags, openat2};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    executable_snapshot::{HandoffExecutableSnapshots, SealedExecutablePair},
    handoff_compatibility::{
        BuildIdentity, BuildVersion, ExecutablePairBinding, ExecutablePairPreflight,
        HandoffCapabilities,
    },
};

/// Record location relative to the install prefix that holds `bin/splinterd`.
pub const PACKAGE_PROVENANCE_RELATIVE_PATH: &str = "share/splinterm/package-provenance-v1.json";
const DISTRIBUTION_PREFIX: &str = "/usr";
const NIX_STORE: &str = "/nix/store";
pub const MAX_PACKAGE_PROVENANCE_BYTES: usize = 4096;
const MAX_PACKAGE_PROVENANCE_BYTES_U64: u64 = 4096;
const PACKAGE_NAME: &str = "splinterm";
const PACKAGE_ARCHITECTURE: &str = "x86_64";
const ZERO_COMMIT: &str = "0000000000000000000000000000000000000000";
const MAX_PATH_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuildProvenanceKind {
    Unpackaged,
    LocalCommittedHead,
    ReleaseCandidate,
    NixStore,
}

impl BuildProvenanceKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unpackaged => "unpackaged",
            Self::LocalCommittedHead => "local-committed-head",
            Self::ReleaseCandidate => "release-candidate",
            Self::NixStore => "nix-store",
        }
    }

    fn parse(value: &str) -> Result<Self, PackageProvenanceError> {
        match value {
            "local-committed-head" => Ok(Self::LocalCommittedHead),
            "release-candidate" => Ok(Self::ReleaseCandidate),
            "nix-store" => Ok(Self::NixStore),
            "unpackaged" => Ok(Self::Unpackaged),
            _ => Err(PackageProvenanceError::InvalidRecord),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompiledBuildProvenance {
    pub source_commit: &'static str,
    pub kind: BuildProvenanceKind,
    pub build_version: &'static str,
    pub pair_build_identity: BuildIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageProvenanceRecord {
    source_path: PathBuf,
    source_commit: String,
    kind: BuildProvenanceKind,
    build_version: String,
    pair_build_identity: BuildIdentity,
    daemon_sha256: [u8; 32],
    client_sha256: [u8; 32],
}

impl PackageProvenanceRecord {
    /// Opens and parses one bounded root-trust package record without following
    /// symlinks in any path component.
    ///
    /// Only Nix store locations are checked here. Callers must pass the path
    /// from [`package_provenance_path`]; any other root-owned file outside the
    /// store would otherwise be accepted as a distribution record.
    ///
    /// # Errors
    ///
    /// Rejects unsafe paths, missing or malformed records, incorrect ownership
    /// or mode, non-regular files, and oversized content.
    pub fn load(path: &Path, expected_owner_uid: u32) -> Result<Self, PackageProvenanceError> {
        validate_path(path)?;
        let descriptor = openat2(
            CWD,
            path,
            OFlags::RDONLY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::NO_SYMLINKS,
        )
        .map_err(errno_io)
        .map_err(|source| PackageProvenanceError::Open {
            path: path.to_path_buf(),
            source,
        })?;
        let mut file = File::from(descriptor);
        let metadata = file.metadata().map_err(PackageProvenanceError::Inspect)?;
        validate_metadata(&metadata, expected_owner_uid)?;
        let expected_size =
            usize::try_from(metadata.len()).map_err(|_| PackageProvenanceError::InvalidMetadata)?;
        let mut bytes = Vec::with_capacity(expected_size);
        file.by_ref()
            .take(MAX_PACKAGE_PROVENANCE_BYTES_U64 + 1)
            .read_to_end(&mut bytes)
            .map_err(PackageProvenanceError::Read)?;
        if bytes.len() != expected_size || bytes.len() > MAX_PACKAGE_PROVENANCE_BYTES {
            return Err(PackageProvenanceError::InvalidMetadata);
        }
        let wire: PackageProvenanceWire =
            serde_json::from_slice(&bytes).map_err(PackageProvenanceError::Parse)?;
        Self::from_wire(path, wire)
    }

    fn from_wire(
        source_path: &Path,
        wire: PackageProvenanceWire,
    ) -> Result<Self, PackageProvenanceError> {
        if wire.schema != 1
            || wire.package != PACKAGE_NAME
            || wire.architecture != PACKAGE_ARCHITECTURE
            || !valid_commit(&wire.source_commit)
            || !valid_version(&wire.build_version)
        {
            return Err(PackageProvenanceError::InvalidRecord);
        }
        parse_build_version(&wire.build_version)?;
        let kind = BuildProvenanceKind::parse(&wire.provenance)?;
        if kind == BuildProvenanceKind::Unpackaged || wire.source_commit == ZERO_COMMIT {
            return Err(PackageProvenanceError::Unpackaged);
        }
        // A Nix record is trusted only inside a store output, and only a Nix
        // record may come from one.
        let in_store = source_path.starts_with(NIX_STORE);
        if in_store != (kind == BuildProvenanceKind::NixStore)
            || (in_store && !is_store_output_record(source_path))
        {
            return Err(PackageProvenanceError::LocationMismatch);
        }
        Ok(Self {
            source_path: source_path.to_path_buf(),
            source_commit: wire.source_commit,
            kind,
            build_version: wire.build_version,
            pair_build_identity: BuildIdentity::new(parse_hex_32(&wire.pair_build_identity)?)
                .map_err(|_| PackageProvenanceError::InvalidRecord)?,
            daemon_sha256: parse_hex_32(&wire.daemon_sha256)?,
            client_sha256: parse_hex_32(&wire.client_sha256)?,
        })
    }

    /// Binds this package record to one exact materialized pair and its
    /// authenticated preflight report.
    ///
    /// # Errors
    ///
    /// Rejects executable hash, version, capability-derived build identity, or
    /// preflight identity mismatches.
    pub fn verify_pair(
        &self,
        pair: &SealedExecutablePair,
        preflight: ExecutablePairPreflight,
    ) -> Result<ValidatedPackageProvenance, PackageProvenanceError> {
        if preflight.snapshots != ExecutablePairBinding::from_snapshots(pair) {
            return Err(PackageProvenanceError::BuildMismatch);
        }
        if pair.daemon().source().sha256 != self.daemon_sha256
            || pair.client().source().sha256 != self.client_sha256
        {
            return Err(PackageProvenanceError::ExecutableMismatch);
        }
        let expected_version = parse_build_version(&self.build_version)?;
        if preflight.pair_build_version != expected_version {
            return Err(PackageProvenanceError::BuildMismatch);
        }
        let derived = derive_pair_build_identity(
            &self.source_commit,
            self.kind,
            &self.build_version,
            preflight.capabilities,
        )?;
        if derived != self.pair_build_identity
            || preflight.pair_build_identity != self.pair_build_identity
        {
            return Err(PackageProvenanceError::BuildMismatch);
        }
        Ok(ValidatedPackageProvenance {
            source_path: self.source_path.clone(),
            source_commit: self.source_commit.clone(),
            kind: self.kind,
            build_version: self.build_version.clone(),
            pair_build_identity: self.pair_build_identity,
            daemon_sha256: self.daemon_sha256,
            client_sha256: self.client_sha256,
        })
    }
}

/// Locates the package record installed with one daemon executable.
///
/// Distribution packages keep the record under `/usr`; Nix keeps each
/// generation's record inside that generation's own store output.
///
/// # Errors
///
/// Rejects a daemon outside `/usr/bin` or a direct `/nix/store/<output>/bin`.
pub fn package_provenance_path(daemon: &Path) -> Result<PathBuf, PackageProvenanceError> {
    validate_path(daemon)?;
    let prefix = daemon
        .parent()
        .filter(|bin| bin.file_name() == Some(OsStr::new("bin")))
        .and_then(Path::parent)
        .ok_or(PackageProvenanceError::InvalidPath)?;
    let packaged_prefix =
        prefix == Path::new(DISTRIBUTION_PREFIX) || prefix.parent() == Some(Path::new(NIX_STORE));
    if daemon.file_name() != Some(OsStr::new("splinterd")) || !packaged_prefix {
        return Err(PackageProvenanceError::InvalidPath);
    }
    Ok(prefix.join(PACKAGE_PROVENANCE_RELATIVE_PATH))
}

/// Whether a path is exactly the record of one direct store output, the only
/// location [`package_provenance_path`] yields inside the store.
fn is_store_output_record(path: &Path) -> bool {
    path.strip_prefix(NIX_STORE)
        .ok()
        .and_then(|rest| rest.components().next())
        .is_some_and(|output| {
            Path::new(NIX_STORE)
                .join(output)
                .join(PACKAGE_PROVENANCE_RELATIVE_PATH)
                == path
        })
}

/// Validates both package generations atomically against their exact sealed
/// sources and authenticated reports.
///
/// # Errors
///
/// Returns an error without a partial result when either generation fails.
pub fn validate_handoff_package_provenance(
    forward_record: &PackageProvenanceRecord,
    rollback_record: &PackageProvenanceRecord,
    snapshots: &HandoffExecutableSnapshots,
    forward_preflight: ExecutablePairPreflight,
    rollback_preflight: ExecutablePairPreflight,
) -> Result<ValidatedHandoffPackageProvenance, PackageProvenanceError> {
    let forward = forward_record.verify_pair(snapshots.forward(), forward_preflight)?;
    let rollback = rollback_record.verify_pair(snapshots.rollback(), rollback_preflight)?;
    Ok(ValidatedHandoffPackageProvenance { forward, rollback })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedHandoffPackageProvenance {
    pub forward: ValidatedPackageProvenance,
    pub rollback: ValidatedPackageProvenance,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedPackageProvenance {
    pub source_path: PathBuf,
    pub source_commit: String,
    pub kind: BuildProvenanceKind,
    pub build_version: String,
    pub pair_build_identity: BuildIdentity,
    pub daemon_sha256: [u8; 32],
    pub client_sha256: [u8; 32],
}

#[derive(Debug, Error)]
pub enum PackageProvenanceError {
    #[error("package provenance path is not a bounded canonical absolute path")]
    InvalidPath,
    #[error("cannot open package provenance {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot inspect package provenance: {0}")]
    Inspect(#[source] io::Error),
    #[error("package provenance ownership, mode, type, or size is invalid")]
    InvalidMetadata,
    #[error("cannot read package provenance: {0}")]
    Read(#[source] io::Error),
    #[error("cannot parse package provenance: {0}")]
    Parse(#[source] serde_json::Error),
    #[error("package provenance record is invalid")]
    InvalidRecord,
    #[error("unpackaged development builds cannot authorize handoff")]
    Unpackaged,
    #[error("package provenance kind does not match its install location")]
    LocationMismatch,
    #[error("package executable hashes do not match the sealed source pair")]
    ExecutableMismatch,
    #[error("package build identity does not match authenticated preflight")]
    BuildMismatch,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageProvenanceWire {
    schema: u16,
    package: String,
    architecture: String,
    provenance: String,
    source_commit: String,
    build_version: String,
    pair_build_identity: String,
    daemon_sha256: String,
    client_sha256: String,
}

/// Returns build provenance compiled into both daemon and client through their
/// shared daemon library.
///
/// # Errors
///
/// Returns an error only if compile-time metadata violates the closed format.
pub fn compiled_build_provenance(
    capabilities: HandoffCapabilities,
) -> Result<CompiledBuildProvenance, PackageProvenanceError> {
    let source_commit = env!("SPLINTERM_BUILD_COMMIT");
    let kind = BuildProvenanceKind::parse(env!("SPLINTERM_BUILD_PROVENANCE"))?;
    let build_version = env!("CARGO_PKG_VERSION");
    if !valid_commit(source_commit)
        || !valid_version(build_version)
        || (kind == BuildProvenanceKind::Unpackaged) != (source_commit == ZERO_COMMIT)
    {
        return Err(PackageProvenanceError::InvalidRecord);
    }
    Ok(CompiledBuildProvenance {
        source_commit,
        kind,
        build_version,
        pair_build_identity: derive_pair_build_identity(
            source_commit,
            kind,
            build_version,
            capabilities,
        )?,
    })
}

/// Derives the canonical pair identity shared by packaging and preflight.
///
/// # Errors
///
/// Rejects malformed commit/version metadata or invalid capability ranges.
pub fn derive_pair_build_identity(
    source_commit: &str,
    kind: BuildProvenanceKind,
    build_version: &str,
    capabilities: HandoffCapabilities,
) -> Result<BuildIdentity, PackageProvenanceError> {
    if !valid_commit(source_commit) || !valid_version(build_version) {
        return Err(PackageProvenanceError::InvalidRecord);
    }
    let mut hasher = Sha256::new();
    hasher.update(b"splinterm-package-pair-v1\0");
    hasher.update(source_commit.as_bytes());
    hasher.update([0]);
    hasher.update(kind.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(build_version.as_bytes());
    for range in [
        capabilities.private_protocol,
        capabilities.handoff_protocol,
        capabilities.terminal_checkpoint,
        capabilities.descriptor_manifest,
    ] {
        hasher.update(range.minimum().to_be_bytes());
        hasher.update(range.maximum().to_be_bytes());
    }
    BuildIdentity::new(hasher.finalize().into()).map_err(|_| PackageProvenanceError::InvalidRecord)
}

fn validate_path(path: &Path) -> Result<(), PackageProvenanceError> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_PATH_BYTES || !path.is_absolute() {
        return Err(PackageProvenanceError::InvalidPath);
    }
    if path
        .components()
        .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(PackageProvenanceError::InvalidPath);
    }
    Ok(())
}

fn validate_metadata(
    metadata: &Metadata,
    expected_owner_uid: u32,
) -> Result<(), PackageProvenanceError> {
    if !metadata.is_file()
        || metadata.uid() != expected_owner_uid
        || metadata.mode() & 0o022 != 0
        || metadata.len() == 0
        || metadata.len() > MAX_PACKAGE_PROVENANCE_BYTES_U64
    {
        return Err(PackageProvenanceError::InvalidMetadata);
    }
    Ok(())
}

fn valid_commit(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
}

fn parse_build_version(value: &str) -> Result<BuildVersion, PackageProvenanceError> {
    let release = value.split_once('-').map_or(value, |(release, _)| release);
    let mut parts = release.split('.');
    let major = parse_u16(parts.next())?;
    let minor = parse_u16(parts.next())?;
    let patch = parse_u16(parts.next())?;
    if parts.next().is_some() {
        return Err(PackageProvenanceError::InvalidRecord);
    }
    Ok(BuildVersion::new(major, minor, patch))
}

fn parse_u16(value: Option<&str>) -> Result<u16, PackageProvenanceError> {
    value
        .ok_or(PackageProvenanceError::InvalidRecord)?
        .parse()
        .map_err(|_| PackageProvenanceError::InvalidRecord)
}

fn parse_hex_32(value: &str) -> Result<[u8; 32], PackageProvenanceError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(PackageProvenanceError::InvalidRecord);
    }
    let mut bytes = [0_u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let offset = index * 2;
        *byte = u8::from_str_radix(&value[offset..offset + 2], 16)
            .map_err(|_| PackageProvenanceError::InvalidRecord)?;
    }
    Ok(bytes)
}

fn errno_io(error: rustix::io::Errno) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error())
}

#[cfg(test)]
mod tests {
    use std::{
        fmt::Write as _,
        fs,
        os::unix::fs::{PermissionsExt, symlink},
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::{
        executable_snapshot::{
            ExecutableSnapshotPolicy, ExecutableSourcePair, HandoffExecutableSnapshots,
            RetainedRollbackExecutables,
        },
        handoff_compatibility::{ExecutablePairBinding, VersionRange},
    };

    use super::*;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct Fixture {
        directory: PathBuf,
        snapshots: HandoffExecutableSnapshots,
        record: PathBuf,
        capabilities: HandoffCapabilities,
        identity: BuildIdentity,
    }

    impl Fixture {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let directory = std::env::temp_dir().join(format!(
                "splinterd-package-provenance-{}-{unique}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&directory).unwrap();
            let pair_directory = directory.join("pair");
            fs::create_dir(&pair_directory).unwrap();
            let daemon = pair_directory.join("splinterd");
            let client = pair_directory.join("splinterm");
            fs::write(&daemon, format!("daemon-package-bytes-{sequence}")).unwrap();
            fs::write(&client, format!("client-package-bytes-{sequence}")).unwrap();
            for path in [&daemon, &client] {
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }
            let pair = ExecutableSourcePair::new(daemon, client).unwrap();
            let policy = ExecutableSnapshotPolicy {
                expected_owner_uid: rustix::process::geteuid().as_raw(),
            };
            let rollback =
                RetainedRollbackExecutables::capture_declared_for_test(&pair, policy).unwrap();
            let snapshots =
                HandoffExecutableSnapshots::materialize(&pair, rollback, policy).unwrap();
            let capabilities = capabilities();
            let identity = derive_pair_build_identity(
                "1234567890abcdef1234567890abcdef12345678",
                BuildProvenanceKind::LocalCommittedHead,
                "0.2.3-beta1",
                capabilities,
            )
            .unwrap();
            let record = directory.join("package-provenance.json");
            write_record(&record, snapshots.forward(), identity);
            Self {
                directory,
                snapshots,
                record,
                capabilities,
                identity,
            }
        }

        fn preflight(&self, pair: &SealedExecutablePair) -> ExecutablePairPreflight {
            ExecutablePairPreflight {
                pair_build_identity: self.identity,
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
            private_protocol: exact(35),
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

    fn write_record(path: &Path, pair: &SealedExecutablePair, identity: BuildIdentity) {
        let value = serde_json::json!({
            "schema": 1,
            "package": "splinterm",
            "architecture": "x86_64",
            "provenance": "local-committed-head",
            "source_commit": "1234567890abcdef1234567890abcdef12345678",
            "build_version": "0.2.3-beta1",
            "pair_build_identity": hex(identity.as_bytes()),
            "daemon_sha256": hex(pair.daemon().source().sha256),
            "client_sha256": hex(pair.client().source().sha256),
        });
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
    }

    #[test]
    fn loads_and_binds_exact_package_record_to_snapshot_and_preflight() {
        let fixture = Fixture::new();
        let record =
            PackageProvenanceRecord::load(&fixture.record, rustix::process::geteuid().as_raw())
                .unwrap();
        let validated = record
            .verify_pair(
                fixture.snapshots.forward(),
                fixture.preflight(fixture.snapshots.forward()),
            )
            .unwrap();
        assert_eq!(
            validated.source_commit,
            "1234567890abcdef1234567890abcdef12345678"
        );
        assert_eq!(validated.pair_build_identity, fixture.identity);
    }

    #[test]
    fn validates_both_generations_without_partial_authority() {
        let fixture = Fixture::new();
        let record =
            PackageProvenanceRecord::load(&fixture.record, rustix::process::geteuid().as_raw())
                .unwrap();
        let validated = validate_handoff_package_provenance(
            &record,
            &record,
            &fixture.snapshots,
            fixture.preflight(fixture.snapshots.forward()),
            fixture.preflight(fixture.snapshots.rollback()),
        )
        .unwrap();
        assert_eq!(validated.forward, validated.rollback);

        let other = Fixture::new();
        let other_record =
            PackageProvenanceRecord::load(&other.record, rustix::process::geteuid().as_raw())
                .unwrap();
        assert!(matches!(
            validate_handoff_package_provenance(
                &record,
                &other_record,
                &fixture.snapshots,
                fixture.preflight(fixture.snapshots.forward()),
                fixture.preflight(fixture.snapshots.rollback()),
            ),
            Err(PackageProvenanceError::ExecutableMismatch)
        ));
    }

    #[test]
    fn rejects_manifest_symlinks_unsafe_modes_bounds_and_unpacked_records() {
        let fixture = Fixture::new();
        assert!(
            PackageProvenanceRecord::load(
                &fixture.record,
                rustix::process::geteuid().as_raw().wrapping_add(1),
            )
            .is_err()
        );
        let link = fixture.directory.join("link.json");
        symlink(&fixture.record, &link).unwrap();
        assert!(PackageProvenanceRecord::load(&link, rustix::process::geteuid().as_raw()).is_err());

        fs::set_permissions(&fixture.record, fs::Permissions::from_mode(0o666)).unwrap();
        assert!(
            PackageProvenanceRecord::load(&fixture.record, rustix::process::geteuid().as_raw())
                .is_err()
        );
        fs::set_permissions(&fixture.record, fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(
            &fixture.record,
            vec![b'x'; MAX_PACKAGE_PROVENANCE_BYTES + 1],
        )
        .unwrap();
        assert!(
            PackageProvenanceRecord::load(&fixture.record, rustix::process::geteuid().as_raw())
                .is_err()
        );

        let value = serde_json::json!({
            "schema": 1,
            "package": "splinterm",
            "architecture": "x86_64",
            "provenance": "unpackaged",
            "source_commit": ZERO_COMMIT,
            "build_version": "0.2.3",
            "pair_build_identity": "11".repeat(32),
            "daemon_sha256": "22".repeat(32),
            "client_sha256": "33".repeat(32),
        });
        fs::write(&fixture.record, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(matches!(
            PackageProvenanceRecord::load(&fixture.record, rustix::process::geteuid().as_raw()),
            Err(PackageProvenanceError::Unpackaged)
        ));
    }

    #[test]
    fn rejects_each_malformed_or_mislabelled_record_field() {
        let path = Path::new("/usr/share/splinterm/package-provenance-v1.json");
        let valid = serde_json::json!({
            "schema": 1,
            "package": "splinterm",
            "architecture": "x86_64",
            "provenance": "release-candidate",
            "source_commit": "a".repeat(40),
            "build_version": "0.2.0",
            "pair_build_identity": "11".repeat(32),
            "daemon_sha256": "22".repeat(32),
            "client_sha256": "33".repeat(32),
        });
        let parse = |value: &serde_json::Value| {
            serde_json::from_value::<PackageProvenanceWire>(value.clone())
                .map_err(PackageProvenanceError::Parse)
                .and_then(|wire| PackageProvenanceRecord::from_wire(path, wire))
        };
        parse(&valid).unwrap();

        let mut unknown = valid.clone();
        unknown["extra"] = serde_json::json!(true);
        assert!(matches!(
            parse(&unknown),
            Err(PackageProvenanceError::Parse(_))
        ));

        let mut zero_commit = valid.clone();
        zero_commit["source_commit"] = serde_json::json!(ZERO_COMMIT);
        assert!(matches!(
            parse(&zero_commit),
            Err(PackageProvenanceError::Unpackaged)
        ));

        for (field, value) in [
            ("schema", serde_json::json!(2)),
            ("package", serde_json::json!("splinterm-mcp")),
            ("architecture", serde_json::json!("aarch64")),
            ("provenance", serde_json::json!("custom")),
            ("source_commit", serde_json::json!("A".repeat(40))),
            ("build_version", serde_json::json!("0.2")),
            ("pair_build_identity", serde_json::json!("00".repeat(32))),
            ("daemon_sha256", serde_json::json!("AA".repeat(32))),
            ("client_sha256", serde_json::json!("33".repeat(31))),
        ] {
            let mut record = valid.clone();
            record[field] = value;
            assert!(
                matches!(parse(&record), Err(PackageProvenanceError::InvalidRecord)),
                "{field}"
            );
        }
    }

    #[test]
    fn rejects_each_package_preflight_and_executable_substitution() {
        let fixture = Fixture::new();
        let record =
            PackageProvenanceRecord::load(&fixture.record, rustix::process::geteuid().as_raw())
                .unwrap();
        let mut preflight = fixture.preflight(fixture.snapshots.forward());
        preflight.pair_build_identity = BuildIdentity::new([7; 32]).unwrap();
        assert!(matches!(
            record.verify_pair(fixture.snapshots.forward(), preflight),
            Err(PackageProvenanceError::BuildMismatch)
        ));
        let mut preflight = fixture.preflight(fixture.snapshots.forward());
        preflight.pair_build_version.patch += 1;
        assert!(matches!(
            record.verify_pair(fixture.snapshots.forward(), preflight),
            Err(PackageProvenanceError::BuildMismatch)
        ));

        let other = Fixture::new();
        assert!(matches!(
            record.verify_pair(
                other.snapshots.forward(),
                other.preflight(other.snapshots.forward()),
            ),
            Err(PackageProvenanceError::ExecutableMismatch)
        ));
    }

    #[test]
    fn locates_records_only_beside_packaged_daemons() {
        assert_eq!(
            package_provenance_path(Path::new("/usr/bin/splinterd")).unwrap(),
            Path::new("/usr/share/splinterm/package-provenance-v1.json")
        );
        assert_eq!(
            package_provenance_path(Path::new(
                "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-splinterm-0.2.0/bin/splinterd"
            ))
            .unwrap(),
            Path::new(
                "/nix/store/0123456789abcdfghijklmnpqrsvwxyz-splinterm-0.2.0/share/splinterm/package-provenance-v1.json"
            )
        );
        for daemon in [
            "usr/bin/splinterd",
            "/usr/bin/splinterm",
            "/usr/local/bin/splinterd",
            "/opt/splinterm/bin/splinterd",
            "/nix/store/bin/splinterd",
            "/nix/store/output/libexec/bin/splinterd",
            "/nix/store/output/sbin/splinterd",
            "/usr/bin/../bin/splinterd",
        ] {
            assert!(
                matches!(
                    package_provenance_path(Path::new(daemon)),
                    Err(PackageProvenanceError::InvalidPath)
                ),
                "{daemon}"
            );
        }
    }

    #[test]
    fn binds_nix_store_records_to_store_outputs() {
        let wire = |provenance: &str| -> PackageProvenanceWire {
            serde_json::from_value(serde_json::json!({
                "schema": 1,
                "package": "splinterm",
                "architecture": "x86_64",
                "provenance": provenance,
                "source_commit": "1234567890abcdef1234567890abcdef12345678",
                "build_version": "0.2.3",
                "pair_build_identity": "11".repeat(32),
                "daemon_sha256": "22".repeat(32),
                "client_sha256": "33".repeat(32),
            }))
            .unwrap()
        };
        let store = Path::new("/nix/store/output/share/splinterm/package-provenance-v1.json");
        let distribution = Path::new("/usr/share/splinterm/package-provenance-v1.json");

        let record = PackageProvenanceRecord::from_wire(store, wire("nix-store")).unwrap();
        assert_eq!(record.kind, BuildProvenanceKind::NixStore);
        for provenance in ["local-committed-head", "release-candidate"] {
            assert!(PackageProvenanceRecord::from_wire(distribution, wire(provenance)).is_ok());
            assert!(matches!(
                PackageProvenanceRecord::from_wire(store, wire(provenance)),
                Err(PackageProvenanceError::LocationMismatch)
            ));
        }
        for location in [
            distribution,
            Path::new("/nix/store/output/nested/share/splinterm/package-provenance-v1.json"),
            Path::new("/nix/store/output/share/splinterm/other.json"),
            Path::new("/nix/store/package-provenance-v1.json"),
        ] {
            assert!(
                matches!(
                    PackageProvenanceRecord::from_wire(location, wire("nix-store")),
                    Err(PackageProvenanceError::LocationMismatch)
                ),
                "{}",
                location.display()
            );
        }
    }

    #[test]
    fn canonical_identity_matches_the_packaging_generator_fixture() {
        let identity = derive_pair_build_identity(
            "1234567890abcdef1234567890abcdef12345678",
            BuildProvenanceKind::LocalCommittedHead,
            "0.2.3-beta1",
            capabilities(),
        )
        .unwrap();
        assert_eq!(
            hex(identity.as_bytes()),
            "a4a4cef437c25147d8b10c08fd3d5715b6fcbc53274e81f8d4efa700ad4f7cac"
        );
    }

    #[test]
    fn compile_time_identity_matches_the_injected_build_provenance() {
        let compiled = compiled_build_provenance(capabilities()).unwrap();
        assert_eq!(compiled.kind.as_str(), env!("SPLINTERM_BUILD_PROVENANCE"));
        assert_eq!(compiled.source_commit, env!("SPLINTERM_BUILD_COMMIT"));
        assert_eq!(
            compiled.pair_build_identity,
            derive_pair_build_identity(
                compiled.source_commit,
                compiled.kind,
                compiled.build_version,
                capabilities(),
            )
            .unwrap()
        );
        if compiled.kind == BuildProvenanceKind::Unpackaged {
            assert_eq!(compiled.source_commit, ZERO_COMMIT);
        } else {
            assert_ne!(compiled.source_commit, ZERO_COMMIT);
        }
    }
}
