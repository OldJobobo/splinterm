#![deny(unsafe_op_in_unsafe_fn)]

//! Read-only mappings for immutable application assets and sealed descriptors.
//!
//! The unsafe operating-system mapping boundary stays in this leaf crate. Callers
//! receive only an immutable byte slice and cannot mutate or resize the mapping.

use std::{fs::File, io, ops::Deref, os::fd::OwnedFd, path::Path};

use memmap2::{Mmap, MmapOptions};
use rustix::fs::{SealFlags, fcntl_get_seals, fstat};

const REQUIRED_IMMUTABLE_SEALS: SealFlags = SealFlags::WRITE
    .union(SealFlags::GROW)
    .union(SealFlags::SHRINK)
    .union(SealFlags::SEAL);

/// An owned, read-only mapping of one non-empty regular file.
#[derive(Debug)]
pub struct ReadOnlyFileMap {
    mapping: Mmap,
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
        let file = File::open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "mapped asset must be a non-empty regular file",
            ));
        }
        // SAFETY: This leaf API is restricted to package-managed immutable assets.
        // Splinterm opens system font files read-only; Arch package upgrades replace
        // those paths with new inodes rather than mutating an active inode. `Mmap`
        // owns the mapping for as long as the returned slice can be borrowed.
        let mapping = unsafe { MmapOptions::new().map(&file)? };
        Ok(Self { mapping })
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
        // SAFETY: All mutations and size changes are prohibited by verified
        // kernel-enforced seals before mapping. The mapping owns its file-backed
        // pages and exposes only immutable bytes.
        let mapping = unsafe { MmapOptions::new().map(&file)? };
        Ok(Self { mapping })
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
        assert_eq!(&*mapping, b"mapped font bytes");
        fs::remove_file(path).unwrap();
        assert_eq!(&*mapping, b"mapped font bytes");
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
