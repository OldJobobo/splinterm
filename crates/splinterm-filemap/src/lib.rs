#![deny(unsafe_op_in_unsafe_fn)]

//! Read-only mappings for immutable application assets and sealed descriptors.
//!
//! The unsafe operating-system mapping boundary stays in this leaf crate. Callers
//! receive only an immutable byte slice and cannot mutate or resize the mapping.

use std::{
    fs::File,
    io::{self, Read, Write},
    ops::Deref,
    os::{fd::OwnedFd, unix::fs::MetadataExt},
    path::Path,
};

use memmap2::{Mmap, MmapOptions};
use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, fcntl_get_seals, fstat, memfd_create};

const REQUIRED_IMMUTABLE_SEALS: SealFlags = SealFlags::WRITE
    .union(SealFlags::GROW)
    .union(SealFlags::SHRINK)
    .union(SealFlags::SEAL);

/// Identity captured from the exact opened file backing a mapping.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FileIdentity {
    pub device: u64,
    pub inode: u64,
    pub length: u64,
    pub modified_seconds: i64,
    pub modified_nanoseconds: i64,
}

impl FileIdentity {
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
        }
    }
}

/// Immutable snapshot plus the identity of the exact source file that was copied.
#[derive(Debug)]
pub struct ImmutableFileSnapshot {
    pub mapping: ReadOnlyFileMap,
    pub source_identity: FileIdentity,
}

/// An owned, read-only mapping of one non-empty regular file.
#[derive(Debug)]
pub struct ReadOnlyFileMap {
    mapping: Mmap,
    identity: FileIdentity,
}

impl ReadOnlyFileMap {
    /// Maps a package-managed immutable file without copying its bytes to heap.
    ///
    /// The mapped inode must not be modified in place during this value's
    /// lifetime. Splinterm uses this only for system font assets, whose package
    /// updates replace files atomically. The private file descriptor and mapping
    /// lifetime are owned by `Mmap`.
    ///
    /// # Errors
    /// Returns an I/O error for missing, empty, non-regular, or unmappable files.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::map_file(&File::open(path)?)
    }

    fn map_file(file: &File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "mapped asset must be a non-empty regular file",
            ));
        }
        let identity = FileIdentity::from_metadata(&metadata);
        // SAFETY: Callers provide either a read-only package-managed asset or a
        // descriptor whose immutable seals were verified before this helper. The
        // private mapping owns the opened file's pages and exposes only immutable bytes.
        let mapping = unsafe { MmapOptions::new().map(file)? };
        Ok(Self { mapping, identity })
    }

    /// Returns metadata captured from the exact file descriptor that was mapped.
    #[must_use]
    pub const fn identity(&self) -> FileIdentity {
        self.identity
    }

    /// Copies one regular file into a bounded sealed anonymous snapshot.
    ///
    /// Source identity is checked before and after copying. The returned mapping
    /// is backed by a sealed memfd, so later source replacement, mutation, or
    /// truncation cannot change or fault the staged bytes.
    ///
    /// # Errors
    /// Returns an I/O error for an invalid source, an exceeded bound, source
    /// identity drift during copying, sealing failure, or mapping failure.
    pub fn immutable_snapshot(
        path: impl AsRef<Path>,
        maximum_len: usize,
    ) -> io::Result<ImmutableFileSnapshot> {
        if maximum_len == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "immutable snapshot bound must be positive",
            ));
        }
        let mut source = File::open(path)?;
        let before = source.metadata()?;
        if !before.is_file() || before.len() == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "snapshot source must be a non-empty regular file",
            ));
        }
        let maximum_len_u64 = u64::try_from(maximum_len).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "snapshot bound does not fit u64",
            )
        })?;
        if before.len() > maximum_len_u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "snapshot source exceeds its byte bound",
            ));
        }
        let source_identity = FileIdentity::from_metadata(&before);
        let expected_len = usize::try_from(before.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "snapshot length does not fit usize",
            )
        })?;
        let mut bytes = Vec::with_capacity(expected_len);
        (&mut source)
            .take(maximum_len_u64.saturating_add(1))
            .read_to_end(&mut bytes)?;
        let after_identity = FileIdentity::from_metadata(&source.metadata()?);
        if after_identity != source_identity || bytes.len() != expected_len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "snapshot source changed while it was copied",
            ));
        }

        let descriptor = memfd_create(
            "splinterm-immutable-snapshot",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        )?;
        let mut sealed_file = File::from(descriptor);
        sealed_file.write_all(&bytes)?;
        let descriptor: OwnedFd = sealed_file.into();
        fcntl_add_seals(&descriptor, REQUIRED_IMMUTABLE_SEALS)?;
        let mapping = Self::from_sealed_fd(descriptor, bytes.len())?;
        Ok(ImmutableFileSnapshot {
            mapping,
            source_identity,
        })
    }

    /// Verifies and maps an exactly-sized sealed descriptor read-only.
    ///
    /// The descriptor must carry write, grow, shrink, and further-sealing seals,
    /// making its bytes and extent immutable for the mapping lifetime.
    ///
    /// # Errors
    /// Returns an I/O error for missing seals, mismatched/empty size, or mapping failure.
    pub fn from_sealed_fd(fd: OwnedFd, expected_len: usize) -> io::Result<Self> {
        let seals = fcntl_get_seals(&fd)?;
        let size = usize::try_from(fstat(&fd)?.st_size).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "sealed descriptor size is invalid",
            )
        })?;
        if expected_len == 0 || size != expected_len || !seals.contains(REQUIRED_IMMUTABLE_SEALS) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "sealed descriptor metadata is inconsistent",
            ));
        }
        let file = File::from(fd);
        Self::map_file(&file)
    }
}

impl Deref for ReadOnlyFileMap {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.mapping
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, time::SystemTime};

    use super::*;

    fn temporary_path(label: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "splinterm-filemap-{label}-{}-{unique}",
            std::process::id()
        ))
    }

    #[test]
    fn maps_non_empty_regular_file_without_mutable_access() {
        let path = temporary_path("bytes");
        fs::write(&path, b"mapped font bytes").unwrap();
        let mapping = ReadOnlyFileMap::open(&path).unwrap();
        let first_identity = mapping.identity();
        assert_eq!(&*mapping, b"mapped font bytes");
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"replacement font bytes are different").unwrap();
        let replacement = ReadOnlyFileMap::open(&path).unwrap();
        assert_ne!(replacement.identity(), first_identity);
        assert_eq!(&*mapping, b"mapped font bytes");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn immutable_snapshot_is_bounded_and_detached_from_source_mutation() {
        let path = temporary_path("immutable-snapshot");
        fs::write(&path, b"original font bytes").unwrap();
        let snapshot = ReadOnlyFileMap::immutable_snapshot(&path, 64).unwrap();
        assert_eq!(snapshot.source_identity.length, 19);
        assert_eq!(&*snapshot.mapping, b"original font bytes");

        fs::write(&path, b"x").unwrap();
        assert_eq!(&*snapshot.mapping, b"original font bytes");
        assert_eq!(
            ReadOnlyFileMap::immutable_snapshot(&path, 0)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        fs::write(&path, b"too large").unwrap();
        assert_eq!(
            ReadOnlyFileMap::immutable_snapshot(&path, 4)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn maps_only_exactly_sized_immutable_sealed_descriptors() {
        use rustix::{
            fs::{MemfdFlags, fcntl_add_seals, memfd_create},
            io::write,
        };

        let fd = memfd_create(
            "splinterm-filemap-test",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        )
        .unwrap();
        write(&fd, b"sealed pixels").unwrap();
        fcntl_add_seals(&fd, REQUIRED_IMMUTABLE_SEALS).unwrap();
        let mapping = ReadOnlyFileMap::from_sealed_fd(fd, 13).unwrap();
        assert_eq!(&*mapping, b"sealed pixels");

        let unsealed = memfd_create(
            "splinterm-filemap-unsealed-test",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        )
        .unwrap();
        write(&unsealed, b"mutable").unwrap();
        assert_eq!(
            ReadOnlyFileMap::from_sealed_fd(unsealed, 7)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn rejects_empty_files_and_directories() {
        let path = temporary_path("empty");
        fs::write(&path, []).unwrap();
        assert_eq!(
            ReadOnlyFileMap::open(&path).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        fs::remove_file(&path).unwrap();
        assert_eq!(
            ReadOnlyFileMap::open(std::env::temp_dir())
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
}
