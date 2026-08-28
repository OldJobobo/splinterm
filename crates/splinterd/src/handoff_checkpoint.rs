//! Immutable anonymous storage and parent-owned linkage for terminal checkpoints.
//!
//! This module does not clear `FD_CLOEXEC`, inherit, claim, or adopt descriptors.
//! It retains one sealed schema-v2 terminal body and projects its semantic slot
//! into the body-free manifest while the parent-owned object remains authority.

use std::{
    io,
    os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
};

use rustix::{
    fs::{MemfdFlags, SealFlags, fcntl_add_seals, fcntl_get_seals, fstat, memfd_create},
    io::{FdFlags, fcntl_getfd, pread, write},
};
use sha2::{Digest, Sha256};
use splinterm_filemap::REQUIRED_IMMUTABLE_SEALS;
use splinterm_terminal::{
    CheckpointError, MAX_TERMINAL_CHECKPOINT_BYTES, TERMINAL_CHECKPOINT_SCHEMA_V2, Terminal,
};
use thiserror::Error;

use crate::{
    handoff_descriptors::{HandoffDescriptor, HandoffDescriptorSlot},
    handoff_manifest::{BodyDisposition, ManifestDescriptor},
};

const HASH_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CheckpointBodyIdentity {
    pub schema: u16,
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub sha256: [u8; 32],
    pub seals: SealFlags,
}

#[derive(Debug)]
pub struct SealedCheckpointBody {
    identity: CheckpointBodyIdentity,
    descriptor: OwnedFd,
}

impl SealedCheckpointBody {
    /// Copies one canonical image-aware terminal checkpoint into a sealed memfd.
    ///
    /// # Errors
    /// Rejects empty, oversized, noncanonical, digest-invalid, non-v2, writable,
    /// incorrectly sized, incorrectly hashed, unsealed, or inheritable storage.
    pub fn seal(bytes: &[u8]) -> Result<Self, CheckpointStorageError> {
        if bytes.is_empty() {
            return Err(CheckpointStorageError::Empty);
        }
        if bytes.len() > MAX_TERMINAL_CHECKPOINT_BYTES {
            return Err(CheckpointStorageError::TooLarge);
        }
        let schema = Terminal::validate_checkpoint_encoding(bytes)?;
        if schema != TERMINAL_CHECKPOINT_SCHEMA_V2 {
            return Err(CheckpointStorageError::UnsupportedSchema(schema));
        }

        let descriptor = memfd_create(
            "splinterd-terminal-checkpoint",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        )
        .map_err(errno_error)
        .map_err(CheckpointStorageError::Create)?;
        write_all(&descriptor, bytes).map_err(CheckpointStorageError::Write)?;
        let source_digest: [u8; 32] = Sha256::digest(bytes).into();
        let size = u64::try_from(bytes.len()).map_err(|_| CheckpointStorageError::TooLarge)?;
        let descriptor_digest = hash_descriptor(&descriptor, size)?;
        if descriptor_digest != source_digest {
            return Err(CheckpointStorageError::Mismatch);
        }

        fcntl_add_seals(&descriptor, REQUIRED_IMMUTABLE_SEALS)
            .map_err(errno_error)
            .map_err(CheckpointStorageError::Verify)?;
        let stat = fstat(&descriptor)
            .map_err(errno_error)
            .map_err(CheckpointStorageError::Verify)?;
        let seals = fcntl_get_seals(&descriptor)
            .map_err(errno_error)
            .map_err(CheckpointStorageError::Verify)?;
        let identity = CheckpointBodyIdentity {
            schema,
            device: stat.st_dev,
            inode: stat.st_ino,
            size,
            sha256: descriptor_digest,
            seals,
        };
        verify_descriptor(descriptor.as_fd(), identity)?;
        Ok(Self {
            identity,
            descriptor,
        })
    }

    #[must_use]
    pub const fn identity(&self) -> CheckpointBodyIdentity {
        self.identity
    }

    #[must_use]
    pub fn descriptor(&self) -> BorrowedFd<'_> {
        self.descriptor.as_fd()
    }

    /// Revalidates the retained descriptor against its immutable identity.
    ///
    /// # Errors
    /// Rejects descriptor replacement, closure, flag mutation, size/hash drift,
    /// missing seals, or a writable shared mapping.
    pub fn verify(&self) -> Result<(), CheckpointStorageError> {
        verify_descriptor(self.descriptor(), self.identity)
    }
}

#[derive(Debug)]
pub struct LinkedCheckpointBody {
    slot: u16,
    inherited_fd: u32,
    body: SealedCheckpointBody,
}

impl LinkedCheckpointBody {
    /// Binds one parent-owned sealed body to its manifest semantic slot.
    ///
    /// The image-aware schema-v2 body uses no separate `ImageState` descriptor.
    ///
    /// # Errors
    /// Rejects a standard or otherwise invalid inherited descriptor number.
    pub fn new(slot: u16, body: SealedCheckpointBody) -> Result<Self, CheckpointStorageError> {
        let raw = body.descriptor().as_raw_fd();
        if raw < 3 {
            return Err(CheckpointStorageError::InvalidDescriptor(raw));
        }
        let inherited_fd =
            u32::try_from(raw).map_err(|_| CheckpointStorageError::InvalidDescriptor(raw))?;
        Ok(Self {
            slot,
            inherited_fd,
            body,
        })
    }

    #[must_use]
    pub const fn slot(&self) -> u16 {
        self.slot
    }

    #[must_use]
    pub const fn body(&self) -> &SealedCheckpointBody {
        &self.body
    }

    #[must_use]
    pub const fn terminal_disposition(&self) -> BodyDisposition {
        BodyDisposition::Descriptor(self.slot)
    }

    #[must_use]
    pub const fn image_disposition(&self) -> BodyDisposition {
        BodyDisposition::NotPresent
    }

    /// Projects the slot only when the manifest negotiated this exact body schema.
    ///
    /// # Errors
    /// Rejects schema substitution between parent-owned storage and manifest evidence.
    pub const fn manifest_descriptor(
        &self,
        terminal_checkpoint_schema: u16,
    ) -> Result<ManifestDescriptor, CheckpointStorageError> {
        if terminal_checkpoint_schema != self.body.identity.schema {
            return Err(CheckpointStorageError::ManifestSchemaMismatch);
        }
        Ok(ManifestDescriptor {
            slot: HandoffDescriptorSlot::TerminalCheckpoint(self.slot),
            inherited_fd: self.inherited_fd,
        })
    }

    #[must_use]
    pub fn handoff_descriptor(&self) -> HandoffDescriptor<'_> {
        HandoffDescriptor::new(
            HandoffDescriptorSlot::TerminalCheckpoint(self.slot),
            self.body.descriptor(),
        )
    }

    /// Revalidates parent-owned body authority before later inheritance work.
    ///
    /// # Errors
    /// Propagates any immutable descriptor identity failure.
    pub fn verify(&self) -> Result<(), CheckpointStorageError> {
        self.body.verify()
    }
}

#[derive(Debug, Error)]
pub enum CheckpointStorageError {
    #[error("terminal checkpoint body is empty")]
    Empty,
    #[error("terminal checkpoint exceeds {MAX_TERMINAL_CHECKPOINT_BYTES} bytes")]
    TooLarge,
    #[error("terminal checkpoint encoding is invalid: {0}")]
    Encoding(#[from] CheckpointError),
    #[error("terminal checkpoint schema {0} cannot carry embedded image state")]
    UnsupportedSchema(u16),
    #[error("cannot create terminal checkpoint storage: {0}")]
    Create(#[source] io::Error),
    #[error("cannot write terminal checkpoint storage: {0}")]
    Write(#[source] io::Error),
    #[error("cannot verify terminal checkpoint storage: {0}")]
    Verify(#[source] io::Error),
    #[error("terminal checkpoint descriptor does not match its retained identity")]
    Mismatch,
    #[error("terminal checkpoint descriptor {0} cannot be linked into a manifest")]
    InvalidDescriptor(i32),
    #[error("terminal checkpoint schema does not match the body-free manifest")]
    ManifestSchemaMismatch,
}

fn verify_descriptor(
    descriptor: BorrowedFd<'_>,
    identity: CheckpointBodyIdentity,
) -> Result<(), CheckpointStorageError> {
    let stat = fstat(descriptor)
        .map_err(errno_error)
        .map_err(CheckpointStorageError::Verify)?;
    let size = u64::try_from(stat.st_size).map_err(|error| {
        CheckpointStorageError::Verify(io::Error::new(io::ErrorKind::InvalidData, error))
    })?;
    let seals = fcntl_get_seals(descriptor)
        .map_err(errno_error)
        .map_err(CheckpointStorageError::Verify)?;
    let flags = fcntl_getfd(descriptor)
        .map_err(errno_error)
        .map_err(CheckpointStorageError::Verify)?;
    if identity.schema != TERMINAL_CHECKPOINT_SCHEMA_V2
        || identity.device == 0
        || identity.inode == 0
        || identity.size == 0
        || identity.size > MAX_TERMINAL_CHECKPOINT_BYTES as u64
        || identity.sha256 == [0; 32]
        || stat.st_dev != identity.device
        || stat.st_ino != identity.inode
        || size != identity.size
        || seals != identity.seals
        || !seals.contains(REQUIRED_IMMUTABLE_SEALS)
        || !flags.contains(FdFlags::CLOEXEC)
        || hash_descriptor_borrowed(descriptor, identity.size)? != identity.sha256
    {
        return Err(CheckpointStorageError::Mismatch);
    }
    let expected_len = usize::try_from(identity.size).map_err(|error| {
        CheckpointStorageError::Verify(io::Error::new(io::ErrorKind::InvalidData, error))
    })?;
    splinterm_filemap::verify_writable_shared_mapping_rejected(descriptor, expected_len)
        .map_err(CheckpointStorageError::Verify)
}

fn write_all(fd: &OwnedFd, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        let count = write(fd, bytes).map_err(errno_error)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "checkpoint write returned zero",
            ));
        }
        bytes = &bytes[count..];
    }
    Ok(())
}

fn hash_descriptor(
    descriptor: &OwnedFd,
    expected_size: u64,
) -> Result<[u8; 32], CheckpointStorageError> {
    hash_descriptor_borrowed(descriptor.as_fd(), expected_size)
}

fn hash_descriptor_borrowed(
    descriptor: BorrowedFd<'_>,
    expected_size: u64,
) -> Result<[u8; 32], CheckpointStorageError> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES].into_boxed_slice();
    let mut total = 0_u64;
    loop {
        let count = pread(descriptor, &mut *buffer, total)
            .map_err(errno_error)
            .map_err(CheckpointStorageError::Verify)?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(u64::try_from(count).expect("bounded read count fits u64"))
            .ok_or(CheckpointStorageError::Mismatch)?;
        if total > MAX_TERMINAL_CHECKPOINT_BYTES as u64 {
            return Err(CheckpointStorageError::Mismatch);
        }
        hasher.update(&buffer[..count]);
    }
    if total != expected_size {
        return Err(CheckpointStorageError::Mismatch);
    }
    Ok(hasher.finalize().into())
}

fn errno_error(error: rustix::io::Errno) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error())
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsRawFd;

    use rustix::{
        fs::{SealFlags, SeekFrom, fcntl_add_seals, ftruncate, seek},
        io::{FdFlags, fcntl_getfd, write},
    };
    use splinterm_terminal::{TERMINAL_CHECKPOINT_SCHEMA_V2, TerminalConfig};

    use super::*;

    fn checkpoint_bytes() -> Vec<u8> {
        let mut terminal = Terminal::new(8, 3, TerminalConfig::default());
        terminal.advance(b"checkpoint body");
        terminal.encode_checkpoint().unwrap()
    }

    #[test]
    fn sealed_checkpoint_has_exact_identity_and_kernel_immutability() {
        let bytes = checkpoint_bytes();
        let body = SealedCheckpointBody::seal(&bytes).unwrap();
        let identity = body.identity();
        assert_eq!(identity.schema, TERMINAL_CHECKPOINT_SCHEMA_V2);
        assert_eq!(identity.size, bytes.len() as u64);
        assert_eq!(identity.sha256, <[u8; 32]>::from(Sha256::digest(&bytes)));
        assert!(identity.seals.contains(REQUIRED_IMMUTABLE_SEALS));
        assert!(
            fcntl_getfd(body.descriptor())
                .unwrap()
                .contains(FdFlags::CLOEXEC)
        );
        assert!(write(body.descriptor(), b"x").is_err());
        assert!(ftruncate(body.descriptor(), 1).is_err());
        assert!(ftruncate(body.descriptor(), bytes.len() as u64 + 1).is_err());
        assert!(fcntl_add_seals(body.descriptor(), SealFlags::FUTURE_WRITE).is_err());
        let offset_before = seek(body.descriptor(), SeekFrom::Start(1)).unwrap();
        assert_eq!(offset_before, 1);
        body.verify().unwrap();
        assert_eq!(
            seek(body.descriptor(), SeekFrom::Current(0)).unwrap(),
            offset_before
        );

        let duplicate = rustix::io::fcntl_dupfd_cloexec(body.descriptor(), 3).unwrap();
        let mapped =
            splinterm_filemap::ReadOnlyFileMap::from_sealed_fd(duplicate, bytes.len()).unwrap();
        assert_eq!(&*mapped, bytes.as_slice());
    }

    #[test]
    fn parent_owned_link_projects_one_terminal_slot_and_embedded_image_disposition() {
        let body = SealedCheckpointBody::seal(&checkpoint_bytes()).unwrap();
        let raw = body.descriptor().as_raw_fd();
        let linked = LinkedCheckpointBody::new(9, body).unwrap();
        assert_eq!(linked.slot(), 9);
        assert_eq!(
            linked.terminal_disposition(),
            BodyDisposition::Descriptor(9)
        );
        assert_eq!(linked.image_disposition(), BodyDisposition::NotPresent);
        assert_eq!(
            linked
                .manifest_descriptor(TERMINAL_CHECKPOINT_SCHEMA_V2)
                .unwrap(),
            ManifestDescriptor {
                slot: HandoffDescriptorSlot::TerminalCheckpoint(9),
                inherited_fd: u32::try_from(raw).unwrap(),
            }
        );
        assert_eq!(
            linked.handoff_descriptor().slot(),
            HandoffDescriptorSlot::TerminalCheckpoint(9)
        );
        assert_eq!(linked.handoff_descriptor().raw_fd(), raw);
        assert!(matches!(
            linked.manifest_descriptor(TERMINAL_CHECKPOINT_SCHEMA_V2 - 1),
            Err(CheckpointStorageError::ManifestSchemaMismatch)
        ));
        linked.verify().unwrap();
    }

    #[test]
    fn invalid_encoding_bounds_and_descriptor_substitution_fail_closed() {
        assert!(matches!(
            SealedCheckpointBody::seal(&[]),
            Err(CheckpointStorageError::Empty)
        ));
        assert!(matches!(
            SealedCheckpointBody::seal(&vec![0; MAX_TERMINAL_CHECKPOINT_BYTES + 1]),
            Err(CheckpointStorageError::TooLarge)
        ));
        let mut corrupt = checkpoint_bytes();
        corrupt[0] ^= 1;
        assert!(matches!(
            SealedCheckpointBody::seal(&corrupt),
            Err(CheckpointStorageError::Encoding(_))
        ));

        let first = SealedCheckpointBody::seal(&checkpoint_bytes()).unwrap();
        let mut second_bytes = checkpoint_bytes();
        second_bytes.pop();
        second_bytes.push(b' ');
        assert!(SealedCheckpointBody::seal(&second_bytes).is_err());
        let second = SealedCheckpointBody::seal(&checkpoint_bytes()).unwrap();
        assert!(matches!(
            verify_descriptor(second.descriptor(), first.identity()),
            Err(CheckpointStorageError::Mismatch)
        ));
    }
}
