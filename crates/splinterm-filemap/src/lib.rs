#![deny(unsafe_op_in_unsafe_fn)]

//! Read-only mappings for package-managed, immutable application assets.
//!
//! The unsafe operating-system mapping boundary stays in this leaf crate. Callers
//! receive only an immutable byte slice and cannot mutate or resize the mapping.

use std::{fs::File, io, ops::Deref, path::Path};

use memmap2::{Mmap, MmapOptions};

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
