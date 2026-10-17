//! Audited ownership boundary for descriptors inherited across daemon exec.
//!
//! Rust cannot safely construct `OwnedFd` from manifest-provided raw numbers:
//! the compiler cannot prove that no pre-exec Rust owner survived. This crate is
//! the only Plan 0037 boundary permitted to perform that conversion. Its unsafe
//! entrypoint is intentionally narrow and valid only in the new daemon image,
//! before threads or ordinary runtime objects are created, so its caller must
//! prove the temporal ownership contract. The entrypoint independently rejects
//! a multi-threaded process before it observes or mutates any descriptor.

#![deny(unsafe_code)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd},
    path::Path,
};

#[cfg(any(test, feature = "test-support"))]
use rustix::io::fcntl_getfd;
use rustix::{
    fs::OFlags,
    io::{FdFlags, fcntl_setfd},
};
use thiserror::Error;

/// Hard ceiling shared with the daemon descriptor manifest.
pub const MAX_INHERITED_DESCRIPTORS: usize = 1024;

/// Owned descriptors in the exact order supplied by the validated manifest.
///
/// Every descriptor is close-on-exec. Outside tests, the only constructor is
/// [`claim_post_exec_inherited`].
#[derive(Debug)]
pub struct ClaimedInheritedDescriptors {
    descriptors: Vec<OwnedFd>,
    closed_unexpected: usize,
}

impl ClaimedInheritedDescriptors {
    /// Wraps descriptors that already have unique Rust ownership and are
    /// protected from inheritance across another exec.
    ///
    /// This test seam bypasses the post-exec table audit, so it exists only
    /// with the `test-support` feature.
    ///
    /// # Errors
    /// Rejects, and closes, the complete owner set if it is too large or any
    /// descriptor is inheritable or its descriptor flags cannot be inspected.
    #[cfg(any(test, feature = "test-support"))]
    pub fn from_owned(descriptors: Vec<OwnedFd>) -> Result<Self, InheritedFdError> {
        if descriptors.len() > MAX_INHERITED_DESCRIPTORS {
            return Err(InheritedFdError::TooMany);
        }
        for descriptor in &descriptors {
            let flags = fcntl_getfd(descriptor).map_err(|source| {
                InheritedFdError::Inspect(io::Error::from_raw_os_error(source.raw_os_error()))
            })?;
            if !flags.contains(FdFlags::CLOEXEC) {
                return Err(InheritedFdError::OwnedDescriptorInheritable(
                    descriptor.as_raw_fd(),
                ));
            }
        }
        Ok(Self {
            descriptors,
            closed_unexpected: 0,
        })
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.descriptors.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }

    /// Returns whether the owners are exactly `expected`, in order.
    #[must_use]
    pub fn matches_raw_fds(&self, expected: &[RawFd]) -> bool {
        self.descriptors.len() == expected.len()
            && self
                .descriptors
                .iter()
                .zip(expected)
                .all(|(descriptor, expected)| descriptor.as_raw_fd() == *expected)
    }

    #[must_use]
    pub fn into_descriptors(self) -> Vec<OwnedFd> {
        self.descriptors
    }

    /// Number of unmanifested inherited descriptors closed by the claim.
    #[must_use]
    pub const fn closed_unexpected(&self) -> usize {
        self.closed_unexpected
    }
}

#[derive(Debug, Error)]
pub enum InheritedFdError {
    #[error("inherited descriptor count exceeds {MAX_INHERITED_DESCRIPTORS}")]
    TooMany,
    #[error("standard descriptor {0} cannot be claimed")]
    StandardDescriptor(RawFd),
    #[error("inherited descriptor {0} is duplicated")]
    DuplicateDescriptor(RawFd),
    #[error("post-exec descriptor claim requires one thread; found {0}")]
    MultiThreaded(usize),
    #[error("manifest descriptor {0} was not inherited")]
    MissingDescriptor(RawFd),
    #[error("manifest descriptor {0} was already close-on-exec")]
    DescriptorWasNotInherited(RawFd),
    #[error("owned candidate descriptor {0} is inheritable")]
    OwnedDescriptorInheritable(RawFd),
    #[error("cannot inspect the post-exec descriptor table: {0}")]
    Inspect(#[source] io::Error),
    #[error("cannot restore close-on-exec on inherited descriptor {fd}: {source}")]
    RestoreCloseOnExec {
        fd: RawFd,
        #[source]
        source: io::Error,
    },
    #[error("fatal post-exec claim failure left descriptor flags indeterminate: {0}")]
    FatalRollbackCloseOnExec(#[source] io::Error),
}

/// Claims one completely validated post-exec descriptor set.
///
/// The input order is preserved so the daemon can zip the result back to its
/// already-validated typed manifest entries. The function first rejects a
/// process with more than one thread, then performs a mutation-free
/// `/proc/self/fd` pass. Every expected descriptor must be open, non-standard,
/// unique, and intentionally inherited with `FD_CLOEXEC` clear. Only after all
/// expected entries validate does it take ownership, restore `FD_CLOEXEC`, and
/// close every other observed inheritable non-standard descriptor, so none can
/// cross a later exec.
///
/// # Safety
///
/// The caller must be the single-threaded daemon bootstrap immediately after
/// exec. Every non-standard descriptor with `FD_CLOEXEC` clear must have crossed
/// that exec boundary and must have no live Rust owner or borrower; this covers
/// both expected descriptors and unexpected inherited descriptors closed during
/// the ownership commit. An absent expected number is permitted and returns
/// mutation-free [`InheritedFdError::MissingDescriptor`]. No descriptor may be
/// opened, closed, duplicated, or otherwise replaced between this call's
/// validation pass and ownership commit. The thread check is a fail-closed
/// backstop; it does not relieve the caller of this contract.
///
/// # Errors
/// Rejection before the ownership commit does not mutate any descriptor. A
/// close-on-exec update failure returns every descriptor to inheritable raw
/// state and closes nothing. [`InheritedFdError::FatalRollbackCloseOnExec`]
/// means that restoration itself failed: rollback through exec is forbidden and
/// the bootstrap must terminate without treating the descriptor set as
/// recoverable.
#[allow(
    unsafe_code,
    reason = "the audited post-exec API must make its ownership precondition explicit"
)]
pub unsafe fn claim_post_exec_inherited(
    expected: &[RawFd],
) -> Result<ClaimedInheritedDescriptors, InheritedFdError> {
    validate_expected(expected)?;
    let threads = process_thread_count()?;
    if threads != 1 {
        return Err(InheritedFdError::MultiThreaded(threads));
    }
    let observed = observe_process_descriptors()?;
    for fd in expected {
        match observed.get(fd) {
            None => return Err(InheritedFdError::MissingDescriptor(*fd)),
            Some(true) => return Err(InheritedFdError::DescriptorWasNotInherited(*fd)),
            Some(false) => {}
        }
    }

    let expected_set = expected.iter().copied().collect::<BTreeSet<_>>();
    let unexpected = observed
        .iter()
        .filter_map(|(fd, close_on_exec)| {
            (!*close_on_exec && !expected_set.contains(fd)).then_some(*fd)
        })
        .collect::<Vec<_>>();
    let mut claimed = claim_raw_descriptors(expected);
    for (updated, descriptor) in claimed.iter().enumerate() {
        if let Err(source) = fcntl_setfd(descriptor, FdFlags::CLOEXEC) {
            let fd = descriptor.as_raw_fd();
            if let Err(error) = rollback_claim(&mut claimed, updated) {
                return Err(InheritedFdError::FatalRollbackCloseOnExec(error));
            }
            return Err(InheritedFdError::RestoreCloseOnExec {
                fd,
                source: io::Error::from_raw_os_error(source.raw_os_error()),
            });
        }
    }
    close_raw_descriptors(&unexpected);

    Ok(ClaimedInheritedDescriptors {
        descriptors: claimed,
        closed_unexpected: unexpected.len(),
    })
}

fn validate_expected(expected: &[RawFd]) -> Result<(), InheritedFdError> {
    if expected.len() > MAX_INHERITED_DESCRIPTORS {
        return Err(InheritedFdError::TooMany);
    }
    let mut unique = BTreeSet::new();
    for fd in expected {
        if *fd < 3 {
            return Err(InheritedFdError::StandardDescriptor(*fd));
        }
        if !unique.insert(*fd) {
            return Err(InheritedFdError::DuplicateDescriptor(*fd));
        }
    }
    Ok(())
}

fn process_thread_count() -> Result<usize, InheritedFdError> {
    fs::read_dir("/proc/self/task")
        .map_err(InheritedFdError::Inspect)?
        .try_fold(0_usize, |count, entry| {
            entry
                .map(|_| count.saturating_add(1))
                .map_err(InheritedFdError::Inspect)
        })
}

fn observe_process_descriptors() -> Result<BTreeMap<RawFd, bool>, InheritedFdError> {
    let directory = fs::read_dir("/proc/self/fd").map_err(InheritedFdError::Inspect)?;
    let mut entries = Vec::new();
    for entry in directory {
        let entry = entry.map_err(InheritedFdError::Inspect)?;
        let file_name = entry.file_name();
        let raw_name = file_name.to_str().ok_or_else(|| {
            InheritedFdError::Inspect(io::Error::new(
                io::ErrorKind::InvalidData,
                "descriptor table contains a non-UTF-8 entry",
            ))
        })?;
        let fd = raw_name.parse::<RawFd>().map_err(|error| {
            InheritedFdError::Inspect(io::Error::new(io::ErrorKind::InvalidData, error))
        })?;
        if fd >= 3 {
            entries.push(fd);
        }
    }

    // The scan's own close-on-exec descriptor is closed by now. Its number either
    // vanishes or names another close-on-exec procfs read; neither outcome can
    // satisfy an expected entry or add an unexpected inheritable one.
    let mut observed = BTreeMap::new();
    for fd in entries {
        match descriptor_is_close_on_exec(fd) {
            Ok(close_on_exec) => {
                observed.insert(fd, close_on_exec);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(InheritedFdError::Inspect(error)),
        }
    }
    Ok(observed)
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

#[allow(
    unsafe_code,
    reason = "single audited raw ownership boundary after complete validation"
)]
fn claim_raw_descriptors(expected: &[RawFd]) -> Vec<OwnedFd> {
    expected
        .iter()
        .map(|fd| {
            // SAFETY: the mutation-free pass proved the descriptor is open and
            // inherited. The post-exec single-thread invariant guarantees no
            // Rust owner exists, and `expected` was proven unique.
            unsafe { OwnedFd::from_raw_fd(*fd) }
        })
        .collect()
}

/// Returns the first `updated` owners to their observed inheritable state and
/// releases every owner back to raw form without closing it.
fn rollback_claim(claimed: &mut Vec<OwnedFd>, updated: usize) -> io::Result<()> {
    let mut first_error = None;
    for descriptor in claimed.iter().take(updated) {
        // `FD_CLOEXEC` is the only descriptor flag, and validation proved it
        // was clear, so an empty set restores the exact observed flags.
        if let Err(error) = fcntl_setfd(descriptor, FdFlags::empty())
            && first_error.is_none()
        {
            first_error = Some(io::Error::from_raw_os_error(error.raw_os_error()));
        }
    }
    for descriptor in claimed.drain(..) {
        let _ = descriptor.into_raw_fd();
    }
    first_error.map_or(Ok(()), Err)
}

#[allow(
    unsafe_code,
    reason = "single audited closure boundary for unmanifested inherited fds"
)]
fn close_raw_descriptors(descriptors: &[RawFd]) {
    for fd in descriptors {
        // SAFETY: the mutation-free pass observed this inherited descriptor,
        // the expected set excludes it, and startup is still single-threaded.
        drop(unsafe { OwnedFd::from_raw_fd(*fd) });
    }
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Write, os::fd::AsRawFd, sync::mpsc, thread};

    use super::*;

    #[test]
    fn safe_owned_constructor_rejects_and_closes_inheritable_descriptors() {
        let (reader, mut writer) = io::pipe().unwrap();
        let descriptor = OwnedFd::from(reader);
        let raw_fd = descriptor.as_raw_fd();
        fcntl_setfd(&descriptor, FdFlags::empty()).unwrap();

        assert!(matches!(
            ClaimedInheritedDescriptors::from_owned(vec![descriptor]),
            Err(InheritedFdError::OwnedDescriptorInheritable(fd)) if fd == raw_fd
        ));
        // Probe the pipe rather than the descriptor number, which a concurrent
        // test may already have reused.
        let error = writer.write_all(b"x").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    #[allow(unsafe_code, reason = "test exercises the explicit post-exec contract")]
    fn malformed_raw_sets_fail_before_process_table_claim() {
        // SAFETY (all calls): input validation rejects these sets before the
        // process table is inspected, so no descriptor is claimed or closed.
        assert!(matches!(
            unsafe { claim_post_exec_inherited(&[0]) },
            Err(InheritedFdError::StandardDescriptor(0))
        ));
        assert!(matches!(
            unsafe { claim_post_exec_inherited(&[-1]) },
            Err(InheritedFdError::StandardDescriptor(-1))
        ));
        assert!(matches!(
            unsafe { claim_post_exec_inherited(&[i32::MAX, i32::MAX]) },
            Err(InheritedFdError::DuplicateDescriptor(i32::MAX))
        ));
        assert!(matches!(
            unsafe { claim_post_exec_inherited(&vec![i32::MAX; MAX_INHERITED_DESCRIPTORS + 1]) },
            Err(InheritedFdError::TooMany)
        ));
    }

    #[test]
    #[allow(unsafe_code, reason = "test exercises the explicit post-exec contract")]
    fn multi_threaded_process_is_rejected_before_any_descriptor_mutation() {
        let inheritable = File::open("/dev/null").unwrap();
        fcntl_setfd(&inheritable, FdFlags::empty()).unwrap();
        let (release, parked) = mpsc::channel::<()>();
        let helper = thread::spawn(move || parked.recv());

        // SAFETY: the thread check rejects this multi-threaded process before
        // the process table is inspected, so no descriptor is claimed or closed.
        let result = unsafe { claim_post_exec_inherited(&[inheritable.as_raw_fd()]) };
        release.send(()).unwrap();
        helper.join().unwrap().unwrap();

        assert!(matches!(result, Err(InheritedFdError::MultiThreaded(threads)) if threads >= 2));
        assert!(fcntl_getfd(&inheritable).unwrap().is_empty());
    }
}
