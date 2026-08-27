//! Immutable executable snapshot pairs for a future daemon handoff coordinator.

use std::{
    fs::{File, Metadata},
    io::{self, Read, Seek, SeekFrom},
    os::{
        fd::{AsFd, BorrowedFd, OwnedFd},
        unix::{
            ffi::OsStrExt,
            fs::{FileExt, MetadataExt},
        },
    },
    path::{Component, Path, PathBuf},
};

use rustix::{
    fs::{
        CWD, MemfdFlags, Mode, OFlags, ResolveFlags, SealFlags, fcntl_add_seals, fcntl_get_seals,
        fstat, memfd_create, openat2,
    },
    io::write,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const MAX_EXECUTABLE_SNAPSHOT_BYTES: u64 = 256 * 1024 * 1024;
const RUNNING_EXECUTABLE_PATH: &str = "/proc/self/exe";
const MAX_EXECUTABLE_PATH_BYTES: usize = 4096;
const HASH_BUFFER_BYTES: usize = 64 * 1024;
const REQUIRED_EXECUTABLE_SEALS: SealFlags = SealFlags::WRITE
    .union(SealFlags::GROW)
    .union(SealFlags::SHRINK)
    .union(SealFlags::SEAL);
type MetadataFingerprint = (u64, u64, u64, u32, u32, i64, i64, i64, i64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutableSourcePair {
    daemon: PathBuf,
    client: PathBuf,
}

impl ExecutableSourcePair {
    /// Records one adjacent `splinterd`/`splinterm` source pair.
    ///
    /// # Errors
    ///
    /// Rejects unbounded, non-canonical, incorrectly named, or non-adjacent paths.
    pub fn new(
        daemon: impl Into<PathBuf>,
        client: impl Into<PathBuf>,
    ) -> Result<Self, ExecutableSnapshotError> {
        let pair = Self {
            daemon: daemon.into(),
            client: client.into(),
        };
        validate_pair_paths(&pair)?;
        Ok(pair)
    }

    #[must_use]
    pub fn daemon(&self) -> &Path {
        &self.daemon
    }

    #[must_use]
    pub fn client(&self) -> &Path {
        &self.client
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutableSnapshotPolicy {
    pub expected_owner_uid: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutableSourceIdentity {
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
    pub owner_uid: u32,
    pub mode: u32,
    pub size: u64,
    pub sha256: [u8; 32],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutableSnapshotIdentity {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub sha256: [u8; 32],
    pub seals: SealFlags,
}

#[derive(Debug)]
pub struct SealedExecutableSnapshot {
    source: ExecutableSourceIdentity,
    snapshot: ExecutableSnapshotIdentity,
    descriptor: OwnedFd,
}

impl SealedExecutableSnapshot {
    #[must_use]
    pub fn source(&self) -> &ExecutableSourceIdentity {
        &self.source
    }

    #[must_use]
    pub const fn snapshot(&self) -> ExecutableSnapshotIdentity {
        self.snapshot
    }

    #[must_use]
    pub fn descriptor(&self) -> BorrowedFd<'_> {
        self.descriptor.as_fd()
    }

    #[must_use]
    pub fn into_descriptor(self) -> OwnedFd {
        self.descriptor
    }
}

#[derive(Debug)]
pub struct SealedExecutablePair {
    daemon: SealedExecutableSnapshot,
    client: SealedExecutableSnapshot,
}

impl SealedExecutablePair {
    #[must_use]
    pub const fn daemon(&self) -> &SealedExecutableSnapshot {
        &self.daemon
    }

    #[must_use]
    pub const fn client(&self) -> &SealedExecutableSnapshot {
        &self.client
    }
}

/// Open authority for the running daemon/client generation retained before
/// package replacement is allowed to begin.
#[derive(Debug)]
pub struct RetainedRollbackExecutables {
    pair: SealedExecutablePair,
}

impl RetainedRollbackExecutables {
    /// Captures the installed pair while the caller excludes package replacement.
    ///
    /// The daemon is opened through `/proc/self/exe` and must still identify the
    /// declared installed daemon. Both images are copied and sealed before capture
    /// returns, so later replacement or in-place writes cannot affect rollback.
    ///
    /// # Errors
    ///
    /// Rejects invalid paths, authority, source identity, or a daemon other than
    /// the executable running this process.
    pub fn capture(
        source: &ExecutableSourcePair,
        policy: ExecutableSnapshotPolicy,
    ) -> Result<Self, ExecutableSnapshotError> {
        let daemon = open_running_executable()?;
        capture_rollback_executables(source, &daemon, policy)
    }
}

#[derive(Debug)]
pub struct HandoffExecutableSnapshots {
    forward: SealedExecutablePair,
    rollback: SealedExecutablePair,
}

impl HandoffExecutableSnapshots {
    /// Materializes complete forward and rollback daemon/client snapshot pairs.
    ///
    /// Rollback authority must have been retained before package replacement was
    /// allowed to begin. Both rollback images are already sealed inside that
    /// opaque authority, so later replacement or writes cannot affect either image.
    /// No partial authority escapes when any of the four sources fails.
    /// Compatibility preflight remains a separate coordinator step.
    ///
    /// # Errors
    ///
    /// Rejects invalid source identity, mutation, copy, digest, descriptor, or seal state.
    pub fn materialize(
        forward: &ExecutableSourcePair,
        rollback: RetainedRollbackExecutables,
        policy: ExecutableSnapshotPolicy,
    ) -> Result<Self, ExecutableSnapshotError> {
        validate_pair_paths(forward)?;
        let forward = materialize_pair(forward, policy, &mut |_| Ok(()))?;
        Ok(Self {
            forward,
            rollback: rollback.pair,
        })
    }

    #[must_use]
    pub const fn forward(&self) -> &SealedExecutablePair {
        &self.forward
    }

    #[must_use]
    pub const fn rollback(&self) -> &SealedExecutablePair {
        &self.rollback
    }
}

#[derive(Debug, Error)]
pub enum ExecutableSnapshotError {
    #[error("executable source path is not a bounded canonical absolute path")]
    InvalidPath,
    #[error("executable source pair must be adjacent splinterd and splinterm paths")]
    InvalidPair,
    #[error("cannot open executable source {path}: {source}")]
    OpenSource {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot inspect executable source {path}: {source}")]
    InspectSource {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("executable source {0} is not a bounded regular executable")]
    InvalidSource(PathBuf),
    #[error("executable source {path} has invalid owner or writable mode")]
    InvalidAuthority { path: PathBuf },
    #[error("executable source {0} changed while being copied")]
    SourceChanged(PathBuf),
    #[error("declared rollback daemon does not identify the running executable")]
    RunningExecutableMismatch,
    #[error("cannot create executable snapshot: {0}")]
    CreateSnapshot(#[source] io::Error),
    #[error("cannot copy executable source {path}: {source}")]
    CopySource {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot verify executable snapshot: {0}")]
    VerifySnapshot(#[source] io::Error),
    #[error("executable snapshot identity does not match its source")]
    SnapshotMismatch,
}

#[cfg(test)]
fn materialize_with_hook(
    forward: &ExecutableSourcePair,
    rollback: &ExecutableSourcePair,
    running_daemon: BorrowedFd<'_>,
    running_client: BorrowedFd<'_>,
    policy: ExecutableSnapshotPolicy,
    mut after_copy: impl FnMut(&Path) -> io::Result<()>,
) -> Result<HandoffExecutableSnapshots, ExecutableSnapshotError> {
    validate_pair_paths(forward)?;
    validate_pair_paths(rollback)?;

    let forward = materialize_pair(forward, policy, &mut after_copy)?;
    let rollback = materialize_running_pair(
        rollback,
        running_daemon,
        running_client,
        policy,
        &mut after_copy,
    )?;
    Ok(HandoffExecutableSnapshots { forward, rollback })
}

fn materialize_pair(
    pair: &ExecutableSourcePair,
    policy: ExecutableSnapshotPolicy,
    after_copy: &mut impl FnMut(&Path) -> io::Result<()>,
) -> Result<SealedExecutablePair, ExecutableSnapshotError> {
    let daemon = materialize_one(&pair.daemon, "splinterd-daemon", policy, after_copy)?;
    let client = materialize_one(&pair.client, "splinterd-client", policy, after_copy)?;
    Ok(SealedExecutablePair { daemon, client })
}

fn materialize_running_pair(
    pair: &ExecutableSourcePair,
    running_daemon: BorrowedFd<'_>,
    running_client: BorrowedFd<'_>,
    policy: ExecutableSnapshotPolicy,
    after_copy: &mut impl FnMut(&Path) -> io::Result<()>,
) -> Result<SealedExecutablePair, ExecutableSnapshotError> {
    let descriptor = rustix::io::dup(running_daemon)
        .map_err(errno_error)
        .map_err(|source| ExecutableSnapshotError::InspectSource {
            path: pair.daemon.clone(),
            source,
        })?;
    let descriptor = File::from(descriptor);
    let daemon = materialize_open_source(
        &pair.daemon,
        "splinterd-daemon",
        policy,
        after_copy,
        &descriptor,
        false,
    )?;
    let descriptor = rustix::io::dup(running_client)
        .map_err(errno_error)
        .map_err(|source| ExecutableSnapshotError::InspectSource {
            path: pair.client.clone(),
            source,
        })?;
    let descriptor = File::from(descriptor);
    let client = materialize_open_source(
        &pair.client,
        "splinterd-client",
        policy,
        after_copy,
        &descriptor,
        false,
    )?;
    Ok(SealedExecutablePair { daemon, client })
}

fn materialize_one(
    path: &Path,
    snapshot_name: &str,
    policy: ExecutableSnapshotPolicy,
    after_copy: &mut impl FnMut(&Path) -> io::Result<()>,
) -> Result<SealedExecutableSnapshot, ExecutableSnapshotError> {
    let descriptor = File::from(open_source(path)?);
    materialize_open_source(path, snapshot_name, policy, after_copy, &descriptor, true)
}

fn materialize_open_source(
    path: &Path,
    snapshot_name: &str,
    policy: ExecutableSnapshotPolicy,
    after_copy: &mut impl FnMut(&Path) -> io::Result<()>,
    source: &File,
    revalidate_path: bool,
) -> Result<SealedExecutableSnapshot, ExecutableSnapshotError> {
    let metadata_before =
        source
            .metadata()
            .map_err(|source| ExecutableSnapshotError::InspectSource {
                path: path.to_path_buf(),
                source,
            })?;
    validate_source(path, &metadata_before, policy)?;

    let snapshot = memfd_create(
        snapshot_name,
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING | MemfdFlags::EXEC,
    )
    .map_err(errno_error)
    .map_err(ExecutableSnapshotError::CreateSnapshot)?;
    let source_digest = copy_and_hash(path, source, &snapshot, metadata_before.len())?;
    after_copy(path).map_err(|_| ExecutableSnapshotError::SourceChanged(path.to_path_buf()))?;

    let metadata_after =
        source
            .metadata()
            .map_err(|source| ExecutableSnapshotError::InspectSource {
                path: path.to_path_buf(),
                source,
            })?;
    if metadata_fingerprint(&metadata_before) != metadata_fingerprint(&metadata_after) {
        return Err(ExecutableSnapshotError::SourceChanged(path.to_path_buf()));
    }
    if revalidate_path {
        let reopened = open_source(path)
            .map_err(|_| ExecutableSnapshotError::SourceChanged(path.to_path_buf()))?;
        let reopened_metadata = File::from(reopened)
            .metadata()
            .map_err(|_| ExecutableSnapshotError::SourceChanged(path.to_path_buf()))?;
        if metadata_fingerprint(&metadata_after) != metadata_fingerprint(&reopened_metadata) {
            return Err(ExecutableSnapshotError::SourceChanged(path.to_path_buf()));
        }
    }

    let snapshot_digest = hash_descriptor(&snapshot, metadata_before.len())?;
    if snapshot_digest != source_digest {
        return Err(ExecutableSnapshotError::SnapshotMismatch);
    }
    fcntl_add_seals(&snapshot, REQUIRED_EXECUTABLE_SEALS)
        .map_err(errno_error)
        .map_err(ExecutableSnapshotError::VerifySnapshot)?;
    let seals = fcntl_get_seals(&snapshot)
        .map_err(errno_error)
        .map_err(ExecutableSnapshotError::VerifySnapshot)?;
    let snapshot_stat = fstat(&snapshot)
        .map_err(errno_error)
        .map_err(ExecutableSnapshotError::VerifySnapshot)?;
    let snapshot_size = u64::try_from(snapshot_stat.st_size).map_err(|error| {
        ExecutableSnapshotError::VerifySnapshot(io::Error::new(io::ErrorKind::InvalidData, error))
    })?;
    if snapshot_size != metadata_before.len()
        || snapshot_stat.st_mode & 0o111 == 0
        || !seals.contains(REQUIRED_EXECUTABLE_SEALS)
    {
        return Err(ExecutableSnapshotError::SnapshotMismatch);
    }
    let snapshot_len = usize::try_from(snapshot_size).map_err(|error| {
        ExecutableSnapshotError::VerifySnapshot(io::Error::new(io::ErrorKind::InvalidData, error))
    })?;
    splinterm_filemap::verify_writable_shared_mapping_rejected(snapshot.as_fd(), snapshot_len)
        .map_err(ExecutableSnapshotError::VerifySnapshot)?;

    Ok(SealedExecutableSnapshot {
        source: ExecutableSourceIdentity {
            path: path.to_path_buf(),
            device: metadata_after.dev(),
            inode: metadata_after.ino(),
            owner_uid: metadata_after.uid(),
            mode: metadata_after.mode(),
            size: metadata_after.len(),
            sha256: source_digest,
        },
        snapshot: ExecutableSnapshotIdentity {
            device: snapshot_stat.st_dev,
            inode: snapshot_stat.st_ino,
            size: snapshot_size,
            sha256: snapshot_digest,
            seals,
        },
        descriptor: snapshot,
    })
}

fn validate_pair_paths(pair: &ExecutableSourcePair) -> Result<(), ExecutableSnapshotError> {
    validate_path(&pair.daemon)?;
    validate_path(&pair.client)?;
    if pair.daemon.file_name().and_then(|name| name.to_str()) != Some("splinterd")
        || pair.client.file_name().and_then(|name| name.to_str()) != Some("splinterm")
        || pair.daemon.parent() != pair.client.parent()
    {
        return Err(ExecutableSnapshotError::InvalidPair);
    }
    Ok(())
}

fn validate_path(path: &Path) -> Result<(), ExecutableSnapshotError> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty()
        || bytes.len() > MAX_EXECUTABLE_PATH_BYTES
        || !path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(ExecutableSnapshotError::InvalidPath);
    }
    Ok(())
}

fn capture_rollback_executables(
    source: &ExecutableSourcePair,
    daemon: &OwnedFd,
    policy: ExecutableSnapshotPolicy,
) -> Result<RetainedRollbackExecutables, ExecutableSnapshotError> {
    validate_pair_paths(source)?;
    let installed_daemon = open_source(source.daemon())?;
    if descriptor_fingerprint(daemon, source.daemon())?
        != descriptor_fingerprint(&installed_daemon, source.daemon())?
    {
        return Err(ExecutableSnapshotError::RunningExecutableMismatch);
    }
    let client = open_source(source.client())?;
    validate_descriptor(source.daemon(), daemon, policy)?;
    validate_descriptor(source.client(), &client, policy)?;
    let pair =
        materialize_running_pair(source, daemon.as_fd(), client.as_fd(), policy, &mut |_| {
            Ok(())
        })?;
    Ok(RetainedRollbackExecutables { pair })
}

fn validate_descriptor(
    path: &Path,
    descriptor: &OwnedFd,
    policy: ExecutableSnapshotPolicy,
) -> Result<(), ExecutableSnapshotError> {
    let metadata = File::from(rustix::io::dup(descriptor).map_err(errno_error).map_err(
        |source| ExecutableSnapshotError::InspectSource {
            path: path.to_path_buf(),
            source,
        },
    )?)
    .metadata()
    .map_err(|source| ExecutableSnapshotError::InspectSource {
        path: path.to_path_buf(),
        source,
    })?;
    validate_source(path, &metadata, policy)
}

fn open_running_executable() -> Result<OwnedFd, ExecutableSnapshotError> {
    rustix::fs::open(
        RUNNING_EXECUTABLE_PATH,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOCTTY,
        Mode::empty(),
    )
    .map_err(errno_error)
    .map_err(|source| ExecutableSnapshotError::OpenSource {
        path: PathBuf::from(RUNNING_EXECUTABLE_PATH),
        source,
    })
}

fn open_source(path: &Path) -> Result<OwnedFd, ExecutableSnapshotError> {
    openat2(
        CWD,
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOCTTY,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS,
    )
    .map_err(errno_error)
    .map_err(|source| ExecutableSnapshotError::OpenSource {
        path: path.to_path_buf(),
        source,
    })
}

fn descriptor_fingerprint(
    descriptor: &OwnedFd,
    path: &Path,
) -> Result<MetadataFingerprint, ExecutableSnapshotError> {
    let metadata = File::from(rustix::io::dup(descriptor).map_err(errno_error).map_err(
        |source| ExecutableSnapshotError::InspectSource {
            path: path.to_path_buf(),
            source,
        },
    )?)
    .metadata()
    .map_err(|source| ExecutableSnapshotError::InspectSource {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(metadata_fingerprint(&metadata))
}

fn validate_source(
    path: &Path,
    metadata: &Metadata,
    policy: ExecutableSnapshotPolicy,
) -> Result<(), ExecutableSnapshotError> {
    if !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_EXECUTABLE_SNAPSHOT_BYTES
        || metadata.mode() & 0o111 == 0
    {
        return Err(ExecutableSnapshotError::InvalidSource(path.to_path_buf()));
    }
    if metadata.uid() != policy.expected_owner_uid || metadata.mode() & 0o022 != 0 {
        return Err(ExecutableSnapshotError::InvalidAuthority {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

fn copy_and_hash(
    path: &Path,
    source: &File,
    snapshot: &OwnedFd,
    expected_size: u64,
) -> Result<[u8; 32], ExecutableSnapshotError> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES].into_boxed_slice();
    let mut total = 0_u64;
    loop {
        let count = source.read_at(&mut buffer, total).map_err(|source| {
            ExecutableSnapshotError::CopySource {
                path: path.to_path_buf(),
                source,
            }
        })?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(u64::try_from(count).expect("buffer count fits u64"))
            .ok_or_else(|| ExecutableSnapshotError::CopySource {
                path: path.to_path_buf(),
                source: io::Error::new(io::ErrorKind::InvalidData, "source byte count overflow"),
            })?;
        if total > MAX_EXECUTABLE_SNAPSHOT_BYTES {
            return Err(ExecutableSnapshotError::SourceChanged(path.to_path_buf()));
        }
        write_all(snapshot, &buffer[..count]).map_err(|source| {
            ExecutableSnapshotError::CopySource {
                path: path.to_path_buf(),
                source,
            }
        })?;
        hasher.update(&buffer[..count]);
    }
    if total != expected_size {
        return Err(ExecutableSnapshotError::SourceChanged(path.to_path_buf()));
    }
    Ok(hasher.finalize().into())
}

fn write_all(fd: &OwnedFd, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        let count = write(fd, bytes).map_err(errno_error)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "snapshot write returned zero",
            ));
        }
        bytes = &bytes[count..];
    }
    Ok(())
}

fn hash_descriptor(fd: &OwnedFd, expected_size: u64) -> Result<[u8; 32], ExecutableSnapshotError> {
    let duplicate = rustix::io::dup(fd)
        .map_err(errno_error)
        .map_err(ExecutableSnapshotError::VerifySnapshot)?;
    let mut file = File::from(duplicate);
    file.seek(SeekFrom::Start(0))
        .map_err(ExecutableSnapshotError::VerifySnapshot)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES].into_boxed_slice();
    let mut total = 0_u64;
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(ExecutableSnapshotError::VerifySnapshot)?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(u64::try_from(count).expect("buffer count fits u64"))
            .ok_or_else(|| {
                ExecutableSnapshotError::VerifySnapshot(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "snapshot byte count overflow",
                ))
            })?;
        if total > MAX_EXECUTABLE_SNAPSHOT_BYTES {
            return Err(ExecutableSnapshotError::SnapshotMismatch);
        }
        hasher.update(&buffer[..count]);
    }
    if total != expected_size {
        return Err(ExecutableSnapshotError::SnapshotMismatch);
    }
    Ok(hasher.finalize().into())
}

fn metadata_fingerprint(metadata: &Metadata) -> MetadataFingerprint {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.uid(),
        metadata.mode(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

fn errno_error(error: rustix::io::Errno) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        fs::{self, OpenOptions},
        io::Write,
        os::{fd::AsRawFd, unix::fs::PermissionsExt},
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "splinterd-executable-snapshot-{}-{unique}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn pair(&self, generation: &str, daemon: &[u8], client: &[u8]) -> ExecutableSourcePair {
            let directory = self.0.join(generation);
            fs::create_dir(&directory).unwrap();
            let daemon_path = directory.join("splinterd");
            let client_path = directory.join("splinterm");
            write_executable(&daemon_path, daemon);
            write_executable(&client_path, client);
            ExecutableSourcePair::new(daemon_path, client_path).unwrap()
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_executable(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn policy() -> ExecutableSnapshotPolicy {
        ExecutableSnapshotPolicy {
            expected_owner_uid: rustix::process::geteuid().as_raw(),
        }
    }

    fn snapshot_bytes(snapshot: &SealedExecutableSnapshot) -> Vec<u8> {
        let duplicate = rustix::io::dup(snapshot.descriptor()).unwrap();
        let mut file = File::from(duplicate);
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        bytes
    }

    fn descriptors_under(path: &Path) -> usize {
        fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| fs::read_link(entry.path()).ok())
            .filter(|target| target.starts_with(path))
            .count()
    }

    fn retain(
        rollback: &ExecutableSourcePair,
        policy: ExecutableSnapshotPolicy,
    ) -> Result<RetainedRollbackExecutables, ExecutableSnapshotError> {
        let running_daemon = open_source(rollback.daemon())?;
        capture_rollback_executables(rollback, &running_daemon, policy)
    }

    fn materialize(
        forward: &ExecutableSourcePair,
        rollback: &ExecutableSourcePair,
        policy: ExecutableSnapshotPolicy,
    ) -> Result<HandoffExecutableSnapshots, ExecutableSnapshotError> {
        let rollback = retain(rollback, policy)?;
        HandoffExecutableSnapshots::materialize(forward, rollback, policy)
    }

    #[test]
    fn materializes_four_distinct_complete_snapshot_identities() {
        let directory = TestDirectory::new();
        let forward = directory.pair("forward", b"forward-daemon", b"forward-client");
        let rollback = directory.pair("rollback", b"rollback-daemon", b"rollback-client");
        let snapshots = materialize(&forward, &rollback, policy()).unwrap();
        let all = [
            snapshots.forward().daemon(),
            snapshots.forward().client(),
            snapshots.rollback().daemon(),
            snapshots.rollback().client(),
        ];
        let raw = all
            .iter()
            .map(|item| item.descriptor().as_raw_fd())
            .collect::<BTreeSet<_>>();
        assert_eq!(raw.len(), 4);
        let inodes = all
            .iter()
            .map(|item| item.snapshot().inode)
            .collect::<BTreeSet<_>>();
        assert_eq!(inodes.len(), 4);
        for item in all {
            assert_eq!(item.source().size, item.snapshot().size);
            assert_eq!(item.source().sha256, item.snapshot().sha256);
            assert!(item.snapshot().seals.contains(REQUIRED_EXECUTABLE_SEALS));
            assert!(write(item.descriptor(), b"x").is_err());
            assert!(rustix::fs::ftruncate(item.descriptor(), item.snapshot().size + 1).is_err());
            assert!(rustix::fs::ftruncate(item.descriptor(), item.snapshot().size - 1).is_err());
            assert!(fcntl_add_seals(item.descriptor(), SealFlags::empty()).is_err());
        }
    }

    #[test]
    fn source_changes_after_sealing_cannot_change_snapshot_bytes() {
        let directory = TestDirectory::new();
        let forward = directory.pair("forward", b"forward-daemon", b"forward-client");
        let rollback = directory.pair("rollback", b"rollback-daemon", b"rollback-client");
        let snapshots = materialize(&forward, &rollback, policy()).unwrap();
        fs::write(forward.daemon(), b"changed").unwrap();
        fs::remove_file(forward.client()).unwrap();
        assert_eq!(
            snapshot_bytes(snapshots.forward().daemon()),
            b"forward-daemon"
        );
        assert_eq!(
            snapshot_bytes(snapshots.forward().client()),
            b"forward-client"
        );
    }

    #[test]
    fn production_capture_rejects_a_declared_daemon_that_is_not_running() {
        let directory = TestDirectory::new();
        let rollback = directory.pair("rollback", b"not-running", b"rollback-client");

        assert!(matches!(
            RetainedRollbackExecutables::capture(&rollback, policy()),
            Err(ExecutableSnapshotError::RunningExecutableMismatch)
        ));
    }

    #[test]
    fn rollback_pair_is_bound_to_running_descriptors_not_replaced_paths() {
        let directory = TestDirectory::new();
        let forward = directory.pair("forward", b"forward-daemon", b"forward-client");
        let rollback = directory.pair("rollback", b"running-daemon", b"rollback-client");
        let retained = retain(&rollback, policy()).unwrap();
        let running_stat = retained.pair.daemon().source().inode;
        let client_stat = retained.pair.client().source().inode;

        let displaced_daemon = rollback.daemon().with_file_name("old-splinterd");
        fs::rename(rollback.daemon(), displaced_daemon).unwrap();
        write_executable(rollback.daemon(), b"incoming-daemon");
        let displaced_client = rollback.client().with_file_name("old-splinterm");
        fs::rename(rollback.client(), displaced_client).unwrap();
        write_executable(rollback.client(), b"incoming-client");

        let snapshots =
            HandoffExecutableSnapshots::materialize(&forward, retained, policy()).unwrap();
        assert_eq!(
            snapshot_bytes(snapshots.rollback().daemon()),
            b"running-daemon"
        );
        assert_eq!(
            snapshot_bytes(snapshots.rollback().client()),
            b"rollback-client"
        );
        assert_eq!(snapshots.rollback().daemon().source().inode, running_stat);
        assert_ne!(
            snapshots.rollback().daemon().source().inode,
            fs::metadata(rollback.daemon()).unwrap().ino()
        );
        assert_eq!(snapshots.rollback().client().source().inode, client_stat);
        assert_ne!(
            snapshots.rollback().client().source().inode,
            fs::metadata(rollback.client()).unwrap().ino()
        );
    }

    #[test]
    fn rollback_pair_is_sealed_before_in_place_package_writes() {
        let directory = TestDirectory::new();
        let forward = directory.pair("forward", b"forward-daemon", b"forward-client");
        let rollback = directory.pair("rollback", b"running-daemon", b"rollback-client");
        let retained = retain(&rollback, policy()).unwrap();

        fs::write(rollback.daemon(), b"incoming-daemon").unwrap();
        fs::write(rollback.client(), b"incoming-client").unwrap();

        let snapshots =
            HandoffExecutableSnapshots::materialize(&forward, retained, policy()).unwrap();
        assert_eq!(
            snapshot_bytes(snapshots.rollback().daemon()),
            b"running-daemon"
        );
        assert_eq!(
            snapshot_bytes(snapshots.rollback().client()),
            b"rollback-client"
        );
    }

    #[test]
    fn rejects_invalid_pair_paths_authority_and_bounds() {
        let directory = TestDirectory::new();
        let forward = directory.pair("forward", b"daemon", b"client");
        let rollback = directory.pair("rollback", b"daemon", b"client");
        assert!(ExecutableSourcePair::new(forward.client(), forward.daemon()).is_err());
        assert!(ExecutableSourcePair::new(forward.daemon(), rollback.client()).is_err());
        assert!(ExecutableSourcePair::new("splinterd", "splinterm").is_err());

        fs::set_permissions(forward.daemon(), fs::Permissions::from_mode(0o775)).unwrap();
        assert!(materialize(&forward, &rollback, policy()).is_err());
        fs::set_permissions(forward.daemon(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            materialize(
                &forward,
                &rollback,
                ExecutableSnapshotPolicy {
                    expected_owner_uid: policy().expected_owner_uid + 1
                },
            )
            .is_err()
        );

        fs::write(forward.daemon(), []).unwrap();
        assert!(materialize(&forward, &rollback, policy()).is_err());
        let file = OpenOptions::new()
            .write(true)
            .open(forward.daemon())
            .unwrap();
        file.set_len(MAX_EXECUTABLE_SNAPSHOT_BYTES + 1).unwrap();
        assert!(materialize(&forward, &rollback, policy()).is_err());
    }

    #[test]
    fn rejects_symlink_sources_and_parent_components() {
        use std::os::unix::fs::symlink;

        let directory = TestDirectory::new();
        let forward = directory.pair("forward", b"daemon", b"client");
        let rollback = directory.pair("rollback", b"daemon", b"client");
        let real = forward.daemon().with_file_name("real-daemon");
        fs::rename(forward.daemon(), &real).unwrap();
        symlink(&real, forward.daemon()).unwrap();
        assert!(materialize(&forward, &rollback, policy()).is_err());

        let link = directory.0.join("linked-parent");
        symlink(forward.daemon().parent().unwrap(), &link).unwrap();
        let linked =
            ExecutableSourcePair::new(link.join("splinterd"), link.join("splinterm")).unwrap();
        assert!(materialize(&linked, &rollback, policy()).is_err());
    }

    #[test]
    fn failed_fourth_source_returns_no_result_or_descriptor_leak() {
        let directory = TestDirectory::new();
        let forward = directory.pair("forward", b"daemon", b"client");
        let rollback = directory.pair("rollback", b"daemon", b"client");
        fs::set_permissions(rollback.client(), fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(descriptors_under(&directory.0), 0);
        assert!(materialize(&forward, &rollback, policy()).is_err());
        assert_eq!(descriptors_under(&directory.0), 0);
    }

    #[test]
    fn detects_replacement_deletion_and_in_place_mutation_during_copy() {
        for mutation in ["replace", "delete", "rewrite"] {
            let directory = TestDirectory::new();
            let forward = directory.pair("forward", b"daemon", b"client");
            let rollback = directory.pair("rollback", b"daemon", b"client");
            let target = forward.daemon().to_path_buf();
            let running_daemon = open_source(rollback.daemon()).unwrap();
            let running_client = open_source(rollback.client()).unwrap();
            let result = materialize_with_hook(
                &forward,
                &rollback,
                running_daemon.as_fd(),
                running_client.as_fd(),
                policy(),
                |path| {
                    if path != target {
                        return Ok(());
                    }
                    match mutation {
                        "replace" => {
                            let displaced = path.with_file_name("old-daemon");
                            fs::rename(path, displaced)?;
                            write_executable(path, b"replacement");
                        }
                        "delete" => fs::remove_file(path)?,
                        "rewrite" => {
                            let mut file =
                                OpenOptions::new().write(true).truncate(true).open(path)?;
                            file.write_all(b"change")?;
                        }
                        _ => unreachable!(),
                    }
                    Ok(())
                },
            );
            assert!(matches!(
                result,
                Err(ExecutableSnapshotError::SourceChanged(_))
            ));
        }
    }
}
