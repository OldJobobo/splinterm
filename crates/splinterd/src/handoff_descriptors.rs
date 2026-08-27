//! Bounded pre-exec descriptor inheritance for daemon handoff.
//!
//! This module prepares descriptors in the old daemon generation. Candidate-side
//! manifest validation, descriptor claiming, and closure of unclaimed inherited
//! slots belong to the later handoff ABI boundary.

use std::{
    collections::BTreeSet,
    fs, io,
    os::fd::{AsRawFd, BorrowedFd, RawFd},
    path::Path,
};

use rustix::{
    fs::OFlags,
    io::{FdFlags, fcntl_getfd, fcntl_setfd},
};
use thiserror::Error;

/// Maximum number of descriptors one handoff preparation may expose.
pub const MAX_HANDOFF_DESCRIPTORS: usize = 1024;

/// Closed set of descriptor roles understood by the pre-exec boundary.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum HandoffDescriptorSlot {
    PtyMaster(u16),
    Listener,
    CheckpointManifest,
    TerminalCheckpoint(u16),
    ImageState(u16),
    ForwardClient,
    RollbackDaemon,
    RollbackClient,
    ClientProcess(u8),
    Continuation(u8),
}

/// One borrowed descriptor selected for inheritance.
#[derive(Clone, Copy, Debug)]
pub struct HandoffDescriptor<'fd> {
    slot: HandoffDescriptorSlot,
    descriptor: BorrowedFd<'fd>,
}

impl<'fd> HandoffDescriptor<'fd> {
    #[must_use]
    pub const fn new(slot: HandoffDescriptorSlot, descriptor: BorrowedFd<'fd>) -> Self {
        Self { slot, descriptor }
    }

    #[must_use]
    pub const fn slot(self) -> HandoffDescriptorSlot {
        self.slot
    }

    #[must_use]
    pub fn descriptor(self) -> BorrowedFd<'fd> {
        self.descriptor
    }

    #[must_use]
    pub fn raw_fd(self) -> RawFd {
        self.descriptor.as_raw_fd()
    }
}

/// Failure to establish or restore the bounded inheritance set.
#[derive(Debug, Error)]
pub enum DescriptorHandoffError {
    #[error("handoff descriptor count exceeds {MAX_HANDOFF_DESCRIPTORS}")]
    TooManyDescriptors,
    #[error("standard descriptor {0} cannot be inherited as a handoff slot")]
    StandardDescriptor(RawFd),
    #[error("handoff descriptor {0} is listed more than once")]
    DuplicateDescriptor(RawFd),
    #[error("handoff descriptor slot {0:?} is listed more than once")]
    DuplicateSlot(HandoffDescriptorSlot),
    #[error("handoff descriptor {0} was not close-on-exec before preparation")]
    AllowlistedDescriptorWasInheritable(RawFd),
    #[error("unexpected descriptor {0} is not close-on-exec")]
    UnexpectedInheritableDescriptor(RawFd),
    #[error("cannot inspect process descriptor table: {0}")]
    InspectProcessDescriptors(#[source] io::Error),
    #[error("cannot read flags for handoff descriptor {fd}: {source}")]
    ReadDescriptorFlags {
        fd: RawFd,
        #[source]
        source: io::Error,
    },
    #[error("cannot update flags for handoff descriptor {fd}: {source}")]
    UpdateDescriptorFlags {
        fd: RawFd,
        #[source]
        source: io::Error,
    },
}

/// Prepared old-generation inheritance state.
///
/// Successful `execve`/`execveat` never returns. If execution fails or the
/// operation is cancelled, [`Self::restore`] reports restoration errors and
/// `Drop` remains a best-effort fallback.
#[derive(Debug)]
pub struct PreparedDescriptorInheritance<'fd> {
    descriptors: Vec<HandoffDescriptor<'fd>>,
    original_flags: Vec<FdFlags>,
    restored: bool,
}

impl<'fd> PreparedDescriptorInheritance<'fd> {
    /// Audits the process descriptor table and clears `FD_CLOEXEC` only on the
    /// supplied bounded allowlist.
    ///
    /// The caller must hold a process-wide handoff guard from before this call
    /// until successful exec or restoration. That guard must prevent descriptor
    /// creation, duplication, closure, and flag mutation, plus child creation
    /// and exec. `/proc/self/fd` cannot provide a stable audit otherwise, and an
    /// unrelated exec could inherit an allowlisted descriptor after preparation.
    ///
    /// # Errors
    ///
    /// Returns an error before mutation for malformed inputs, a violated
    /// close-on-exec invariant, or an unexpected inheritable descriptor. Flag
    /// update failures restore every descriptor changed so far before returning.
    pub fn prepare(
        descriptors: impl IntoIterator<Item = HandoffDescriptor<'fd>>,
    ) -> Result<Self, DescriptorHandoffError> {
        Self::prepare_with_audit(
            descriptors.into_iter().collect::<Vec<_>>(),
            audit_process_descriptors,
        )
    }

    fn prepare_with_audit(
        descriptors: Vec<HandoffDescriptor<'fd>>,
        audit: impl FnOnce(&[HandoffDescriptor<'_>]) -> Result<(), DescriptorHandoffError>,
    ) -> Result<Self, DescriptorHandoffError> {
        validate_allowlist(&descriptors)?;

        let mut original_flags = Vec::with_capacity(descriptors.len());
        for descriptor in &descriptors {
            let flags = descriptor_flags(*descriptor)?;
            if !flags.contains(FdFlags::CLOEXEC) {
                return Err(DescriptorHandoffError::AllowlistedDescriptorWasInheritable(
                    descriptor.raw_fd(),
                ));
            }
            original_flags.push(flags);
        }

        audit(&descriptors)?;

        let mut prepared = Self {
            descriptors,
            original_flags,
            restored: false,
        };
        for index in 0..prepared.descriptors.len() {
            let flags = prepared.original_flags[index] - FdFlags::CLOEXEC;
            if let Err(source) = fcntl_setfd(prepared.descriptors[index].descriptor(), flags) {
                let error = DescriptorHandoffError::UpdateDescriptorFlags {
                    fd: prepared.descriptors[index].raw_fd(),
                    source: io::Error::from_raw_os_error(source.raw_os_error()),
                };
                let _ = prepared.restore_all();
                return Err(error);
            }
        }
        Ok(prepared)
    }

    #[must_use]
    pub fn descriptors(&self) -> &[HandoffDescriptor<'fd>] {
        &self.descriptors
    }

    /// Restores the exact descriptor flags observed before preparation.
    ///
    /// # Errors
    ///
    /// Returns the first restoration error after attempting every descriptor.
    pub fn restore(mut self) -> Result<(), DescriptorHandoffError> {
        self.restore_all()
    }

    fn restore_all(&mut self) -> Result<(), DescriptorHandoffError> {
        let mut first_error = None;
        for (descriptor, flags) in self.descriptors.iter().zip(&self.original_flags) {
            if let Err(source) = fcntl_setfd(descriptor.descriptor(), *flags)
                && first_error.is_none()
            {
                first_error = Some(DescriptorHandoffError::UpdateDescriptorFlags {
                    fd: descriptor.raw_fd(),
                    source: io::Error::from_raw_os_error(source.raw_os_error()),
                });
            }
        }
        self.restored = first_error.is_none();
        first_error.map_or(Ok(()), Err)
    }
}

impl Drop for PreparedDescriptorInheritance<'_> {
    fn drop(&mut self) {
        if !self.restored
            && let Err(error) = self.restore_all()
        {
            tracing::error!(%error, "failed to restore handoff descriptor flags");
        }
    }
}

fn validate_allowlist(descriptors: &[HandoffDescriptor<'_>]) -> Result<(), DescriptorHandoffError> {
    if descriptors.len() > MAX_HANDOFF_DESCRIPTORS {
        return Err(DescriptorHandoffError::TooManyDescriptors);
    }

    let mut raw_descriptors = BTreeSet::new();
    let mut slots = BTreeSet::new();
    for descriptor in descriptors {
        let raw_fd = descriptor.raw_fd();
        if raw_fd < 3 {
            return Err(DescriptorHandoffError::StandardDescriptor(raw_fd));
        }
        if !raw_descriptors.insert(raw_fd) {
            return Err(DescriptorHandoffError::DuplicateDescriptor(raw_fd));
        }
        if !slots.insert(descriptor.slot()) {
            return Err(DescriptorHandoffError::DuplicateSlot(descriptor.slot()));
        }
    }
    Ok(())
}

fn descriptor_flags(descriptor: HandoffDescriptor<'_>) -> Result<FdFlags, DescriptorHandoffError> {
    fcntl_getfd(descriptor.descriptor()).map_err(|source| {
        DescriptorHandoffError::ReadDescriptorFlags {
            fd: descriptor.raw_fd(),
            source: io::Error::from_raw_os_error(source.raw_os_error()),
        }
    })
}

fn audit_process_descriptors(
    allowlist: &[HandoffDescriptor<'_>],
) -> Result<(), DescriptorHandoffError> {
    let allowed = allowlist
        .iter()
        .map(|descriptor| descriptor.raw_fd())
        .collect::<BTreeSet<_>>();
    let directory =
        fs::read_dir("/proc/self/fd").map_err(DescriptorHandoffError::InspectProcessDescriptors)?;
    let mut entries = Vec::new();
    for entry in directory {
        let entry = entry.map_err(DescriptorHandoffError::InspectProcessDescriptors)?;
        let file_name = entry.file_name();
        let raw_name = file_name.to_str().ok_or_else(|| {
            DescriptorHandoffError::InspectProcessDescriptors(io::Error::new(
                io::ErrorKind::InvalidData,
                "descriptor table contains a non-UTF-8 entry",
            ))
        })?;
        let fd = raw_name.parse::<RawFd>().map_err(|error| {
            DescriptorHandoffError::InspectProcessDescriptors(io::Error::new(
                io::ErrorKind::InvalidData,
                error,
            ))
        })?;
        if fd >= 3 && !allowed.contains(&fd) {
            entries.push(fd);
        }
    }

    for fd in entries {
        match descriptor_is_close_on_exec(fd) {
            Ok(true) => {}
            Ok(false) => {
                return Err(DescriptorHandoffError::UnexpectedInheritableDescriptor(fd));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(DescriptorHandoffError::InspectProcessDescriptors(error)),
        }
    }
    Ok(())
}

fn descriptor_is_close_on_exec(fd: RawFd) -> io::Result<bool> {
    let fdinfo = fs::read_to_string(Path::new("/proc/self/fdinfo").join(fd.to_string()))?;
    let raw_flags = fdinfo
        .lines()
        .find_map(|line| line.strip_prefix("flags:\t"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "fdinfo omitted flags"))?;
    let flags = u64::from_str_radix(raw_flags, 8)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(flags & u64::from(OFlags::CLOEXEC.bits()) != 0)
}

#[cfg(test)]
mod tests {
    use std::{fs::File, os::fd::AsFd};

    use super::*;

    fn null_descriptor() -> File {
        File::open("/dev/null").expect("open null descriptor")
    }

    fn prepare_without_process_audit<'fd>(
        descriptors: impl IntoIterator<Item = HandoffDescriptor<'fd>>,
    ) -> Result<PreparedDescriptorInheritance<'fd>, DescriptorHandoffError> {
        PreparedDescriptorInheritance::prepare_with_audit(descriptors.into_iter().collect(), |_| {
            Ok(())
        })
    }

    #[test]
    fn preparation_records_slots_and_explicit_restore_recovers_exact_flags() {
        let descriptor = null_descriptor();
        let original = fcntl_getfd(&descriptor).unwrap();
        assert!(original.contains(FdFlags::CLOEXEC));
        let prepared = prepare_without_process_audit([HandoffDescriptor::new(
            HandoffDescriptorSlot::Listener,
            descriptor.as_fd(),
        )])
        .unwrap();

        assert_eq!(
            prepared.descriptors()[0].slot(),
            HandoffDescriptorSlot::Listener
        );
        assert_eq!(prepared.descriptors()[0].raw_fd(), descriptor.as_raw_fd());
        assert!(!fcntl_getfd(&descriptor).unwrap().contains(FdFlags::CLOEXEC));

        prepared.restore().unwrap();
        assert_eq!(fcntl_getfd(&descriptor).unwrap(), original);
    }

    #[test]
    fn malformed_allowlists_fail_before_changing_flags() {
        let first = null_descriptor();
        let second = null_descriptor();
        let first_flags = fcntl_getfd(&first).unwrap();
        let second_flags = fcntl_getfd(&second).unwrap();

        let duplicate_descriptor = PreparedDescriptorInheritance::prepare([
            HandoffDescriptor::new(HandoffDescriptorSlot::Listener, first.as_fd()),
            HandoffDescriptor::new(HandoffDescriptorSlot::CheckpointManifest, first.as_fd()),
        ]);
        assert!(matches!(
            duplicate_descriptor,
            Err(DescriptorHandoffError::DuplicateDescriptor(_))
        ));

        let duplicate_slot = PreparedDescriptorInheritance::prepare([
            HandoffDescriptor::new(HandoffDescriptorSlot::Listener, first.as_fd()),
            HandoffDescriptor::new(HandoffDescriptorSlot::Listener, second.as_fd()),
        ]);
        assert!(matches!(
            duplicate_slot,
            Err(DescriptorHandoffError::DuplicateSlot(
                HandoffDescriptorSlot::Listener
            ))
        ));
        assert_eq!(fcntl_getfd(&first).unwrap(), first_flags);
        assert_eq!(fcntl_getfd(&second).unwrap(), second_flags);
    }

    #[test]
    fn unexpected_inheritable_descriptor_blocks_preparation() {
        let allowed = null_descriptor();
        let unexpected = null_descriptor();
        let unexpected_flags = fcntl_getfd(&unexpected).unwrap();
        fcntl_setfd(&unexpected, unexpected_flags - FdFlags::CLOEXEC).unwrap();

        let result = PreparedDescriptorInheritance::prepare([HandoffDescriptor::new(
            HandoffDescriptorSlot::Listener,
            allowed.as_fd(),
        )]);

        assert!(matches!(
            result,
            Err(DescriptorHandoffError::UnexpectedInheritableDescriptor(fd))
                if fd == unexpected.as_raw_fd()
        ));
        assert!(fcntl_getfd(&allowed).unwrap().contains(FdFlags::CLOEXEC));
        fcntl_setfd(&unexpected, unexpected_flags).unwrap();
    }

    #[test]
    fn allowlisted_descriptor_must_start_close_on_exec() {
        let descriptor = null_descriptor();
        let original = fcntl_getfd(&descriptor).unwrap();
        fcntl_setfd(&descriptor, original - FdFlags::CLOEXEC).unwrap();

        let result = PreparedDescriptorInheritance::prepare([HandoffDescriptor::new(
            HandoffDescriptorSlot::Listener,
            descriptor.as_fd(),
        )]);

        assert!(matches!(
            result,
            Err(DescriptorHandoffError::AllowlistedDescriptorWasInheritable(
                _
            ))
        ));
        fcntl_setfd(&descriptor, original).unwrap();
    }

    #[test]
    fn drop_restores_flags_after_cancelled_preparation() {
        let descriptor = null_descriptor();
        let original = fcntl_getfd(&descriptor).unwrap();
        {
            let _prepared = prepare_without_process_audit([HandoffDescriptor::new(
                HandoffDescriptorSlot::Listener,
                descriptor.as_fd(),
            )])
            .unwrap();
            assert!(!fcntl_getfd(&descriptor).unwrap().contains(FdFlags::CLOEXEC));
        }
        assert_eq!(fcntl_getfd(&descriptor).unwrap(), original);
    }

    #[test]
    fn standard_and_excess_descriptor_sets_are_rejected() {
        let stdin = std::io::stdin();
        let standard = PreparedDescriptorInheritance::prepare([HandoffDescriptor::new(
            HandoffDescriptorSlot::Listener,
            stdin.as_fd(),
        )]);
        assert!(matches!(
            standard,
            Err(DescriptorHandoffError::StandardDescriptor(0))
        ));

        let descriptor = null_descriptor();
        let entry = HandoffDescriptor::new(HandoffDescriptorSlot::Listener, descriptor.as_fd());
        let excess = PreparedDescriptorInheritance::prepare(std::iter::repeat_n(
            entry,
            MAX_HANDOFF_DESCRIPTORS + 1,
        ));
        assert!(matches!(
            excess,
            Err(DescriptorHandoffError::TooManyDescriptors)
        ));
    }
}
