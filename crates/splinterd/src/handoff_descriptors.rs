//! Bounded pre-exec descriptor inheritance for daemon handoff.
//!
//! The old generation prepares one typed inheritance set. After exec, the
//! candidate validates the complete manifest before crossing the audited raw-FD
//! boundary in `splinterm-inherited-fd`, which restores close-on-exec and closes
//! unexpected inherited descriptors. `CandidateDescriptorClaims` then owns
//! every typed slot until an adopting subsystem claims it.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd, RawFd},
    path::Path,
};

use rustix::{
    fs::OFlags,
    io::{FdFlags, fcntl_getfd, fcntl_setfd},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::handoff_manifest::{HandoffManifestV1, validate_manifest};

/// Maximum number of descriptors one handoff preparation may expose.
pub const MAX_HANDOFF_DESCRIPTORS: usize = 1024;
const _: () = assert!(MAX_HANDOFF_DESCRIPTORS == splinterm_inherited_fd::MAX_INHERITED_DESCRIPTORS);

/// Closed set of descriptor roles understood by the pre-exec boundary.
///
/// The serde names and the derived order (declaration order) are wire format:
/// canonical handoff manifests sort descriptors by this order and are written by
/// one daemon generation and read by another. Append new variants; never
/// reorder or rename existing ones.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "role", content = "index", rename_all = "snake_case")]
pub enum HandoffDescriptorSlot {
    PtyMaster(u16),
    Listener,
    ImageListener,
    CheckpointManifest,
    RollbackManifest,
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
    #[error("candidate handoff manifest is invalid")]
    InvalidManifest,
    #[error("candidate descriptor ownership does not match the validated manifest")]
    CandidateClaimMismatch,
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

/// Candidate-owned descriptors rebound to exact validated manifest roles.
///
/// Every retained descriptor is close-on-exec. [`Self::take`] transfers one
/// role to its adopting subsystem; all entries left in the table close on drop.
#[derive(Debug)]
pub(crate) struct CandidateDescriptorClaims {
    descriptors: BTreeMap<HandoffDescriptorSlot, OwnedFd>,
    closed_unexpected: usize,
}

impl CandidateDescriptorClaims {
    /// Rebinds already-claimed owners to the complete validated manifest.
    ///
    /// Raw post-exec ownership is the caller's explicit unsafe bootstrap step in
    /// `splinterm-inherited-fd`. This safe layer requires the claimed owners to
    /// be exactly the manifest's descriptor numbers in manifest order, so every
    /// role binds to the descriptor the old generation assigned it.
    ///
    /// # Errors
    /// Rejects an invalid manifest or any count, number, or order mismatch. A
    /// rejected claim set is dropped, closing every owner.
    pub(crate) fn bind(
        manifest: &HandoffManifestV1,
        claimed: splinterm_inherited_fd::ClaimedInheritedDescriptors,
    ) -> Result<Self, DescriptorHandoffError> {
        validate_manifest(manifest).map_err(|_| DescriptorHandoffError::InvalidManifest)?;
        let expected = manifest
            .descriptors
            .iter()
            .map(|entry| {
                RawFd::try_from(entry.inherited_fd)
                    .map_err(|_| DescriptorHandoffError::InvalidManifest)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if !claimed.matches_raw_fds(&expected) {
            return Err(DescriptorHandoffError::CandidateClaimMismatch);
        }
        let closed_unexpected = claimed.closed_unexpected();
        let descriptors = manifest
            .descriptors
            .iter()
            .map(|entry| entry.slot)
            .zip(claimed.into_descriptors())
            .collect();
        Ok(Self {
            descriptors,
            closed_unexpected,
        })
    }

    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.descriptors.len()
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.descriptors.is_empty()
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn contains(&self, slot: HandoffDescriptorSlot) -> bool {
        self.descriptors.contains_key(&slot)
    }

    /// Adds one owner outside manifest validation, so staging tests can prove
    /// they do not depend on it.
    #[cfg(test)]
    pub(crate) fn insert_for_test(&mut self, slot: HandoffDescriptorSlot, descriptor: OwnedFd) {
        assert!(self.descriptors.insert(slot, descriptor).is_none());
    }

    #[must_use]
    pub(crate) fn descriptor(&self, slot: HandoffDescriptorSlot) -> Option<BorrowedFd<'_>> {
        self.descriptors.get(&slot).map(OwnedFd::as_fd)
    }

    /// Transfers unique ownership of one validated role.
    #[must_use]
    pub(crate) fn take(&mut self, slot: HandoffDescriptorSlot) -> Option<OwnedFd> {
        self.descriptors.remove(&slot)
    }

    /// Number of unmanifested inherited descriptors the raw claim closed.
    #[must_use]
    pub(crate) const fn closed_unexpected(&self) -> usize {
        self.closed_unexpected
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
    use std::{
        fs::File,
        io::{PipeWriter, Write},
        os::fd::AsFd,
    };

    use splinterm_inherited_fd::ClaimedInheritedDescriptors;

    use super::*;
    use crate::{
        executable_snapshot::REQUIRED_EXECUTABLE_SEALS,
        handoff_manifest::{
            ExecutablePairSetEvidence, GenerationEvidence, HandoffId, ManifestBuildVersion,
            ManifestDescriptor, ManifestExecutableImage, ManifestExecutablePair, ManifestKind,
        },
    };

    /// Descriptor roles of an idle daemon, in canonical manifest order.
    const CLAIM_ROLES: [HandoffDescriptorSlot; 7] = [
        HandoffDescriptorSlot::Listener,
        HandoffDescriptorSlot::ImageListener,
        HandoffDescriptorSlot::CheckpointManifest,
        HandoffDescriptorSlot::RollbackManifest,
        HandoffDescriptorSlot::ForwardClient,
        HandoffDescriptorSlot::RollbackDaemon,
        HandoffDescriptorSlot::RollbackClient,
    ];

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

    fn claim_generation(patch: u16) -> GenerationEvidence {
        GenerationEvidence {
            build_identity: [u8::try_from(patch).unwrap(); 32],
            build_version: ManifestBuildVersion {
                major: 0,
                minor: 2,
                patch,
            },
        }
    }

    fn claim_image(byte: u8) -> ManifestExecutableImage {
        ManifestExecutableImage {
            source_device: u64::from(byte),
            source_inode: u64::from(byte) + 10,
            source_owner_uid: 0,
            source_mode: 0o100_755,
            source_size: 1024,
            source_sha256: [byte; 32],
            snapshot_device: u64::from(byte) + 20,
            snapshot_inode: u64::from(byte) + 30,
            snapshot_size: 1024,
            snapshot_sha256: [byte; 32],
            snapshot_seals: REQUIRED_EXECUTABLE_SEALS.bits(),
        }
    }

    /// One valid idle-daemon forward manifest naming `raw_fds` in role order.
    fn claim_manifest(raw_fds: &[RawFd]) -> HandoffManifestV1 {
        assert_eq!(raw_fds.len(), CLAIM_ROLES.len());
        let source = claim_generation(1);
        let target = claim_generation(2);
        let manifest = HandoffManifestV1 {
            schema: 1,
            kind: ManifestKind::Forward,
            handoff_id: HandoffId::new([9; 16]).unwrap(),
            source_generation: source,
            target_generation: target,
            topology_revision: 0,
            durable_state_generation: 0,
            private_protocol: splinterm_protocol::PROTOCOL_VERSION,
            handoff_protocol: 1,
            terminal_checkpoint_schema: 2,
            descriptor_manifest_schema: 1,
            executable_pairs: ExecutablePairSetEvidence {
                forward: ManifestExecutablePair {
                    daemon: claim_image(2),
                    client: claim_image(3),
                    generation: target,
                },
                rollback: ManifestExecutablePair {
                    daemon: claim_image(4),
                    client: claim_image(5),
                    generation: source,
                },
            },
            counterpart_manifest_slot: HandoffDescriptorSlot::RollbackManifest,
            splints: Vec::new(),
            descriptors: CLAIM_ROLES
                .into_iter()
                .zip(raw_fds)
                .map(|(slot, fd)| ManifestDescriptor {
                    slot,
                    inherited_fd: u32::try_from(*fd).unwrap(),
                })
                .collect(),
        };
        validate_manifest(&manifest).unwrap();
        manifest
    }

    fn null_owners(count: usize) -> Vec<OwnedFd> {
        (0..count)
            .map(|_| OwnedFd::from(null_descriptor()))
            .collect()
    }

    /// Pipe read ends as owners, with write ends that observe their closure
    /// independently of descriptor-number reuse by concurrent tests.
    fn pipe_owners(count: usize) -> (Vec<OwnedFd>, Vec<PipeWriter>) {
        (0..count)
            .map(|_| {
                let (reader, writer) = std::io::pipe().unwrap();
                (OwnedFd::from(reader), writer)
            })
            .unzip()
    }

    fn reader_closed(writer: &mut PipeWriter) -> bool {
        match writer.write_all(b"x") {
            Ok(()) => false,
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => true,
            Err(error) => panic!("cannot probe pipe reader: {error}"),
        }
    }

    fn raw_fds(owners: &[OwnedFd]) -> Vec<RawFd> {
        owners.iter().map(AsRawFd::as_raw_fd).collect()
    }

    #[test]
    fn candidate_bind_rejects_reordered_short_unrelated_and_invalid_claims() {
        let (mut reordered_owners, mut reordered_writers) = pipe_owners(CLAIM_ROLES.len());
        let reordered_manifest = claim_manifest(&raw_fds(&reordered_owners));
        reordered_owners.swap(0, 1);
        let reordered = ClaimedInheritedDescriptors::from_owned(reordered_owners).unwrap();
        assert!(matches!(
            CandidateDescriptorClaims::bind(&reordered_manifest, reordered),
            Err(DescriptorHandoffError::CandidateClaimMismatch)
        ));
        assert!(reordered_writers.iter_mut().all(reader_closed));

        let (owners, mut writers) = pipe_owners(CLAIM_ROLES.len());
        let manifest = claim_manifest(&raw_fds(&owners));
        let short =
            ClaimedInheritedDescriptors::from_owned(null_owners(CLAIM_ROLES.len() - 1)).unwrap();
        assert!(matches!(
            CandidateDescriptorClaims::bind(&manifest, short),
            Err(DescriptorHandoffError::CandidateClaimMismatch)
        ));
        let unrelated =
            ClaimedInheritedDescriptors::from_owned(null_owners(CLAIM_ROLES.len())).unwrap();
        assert!(matches!(
            CandidateDescriptorClaims::bind(&manifest, unrelated),
            Err(DescriptorHandoffError::CandidateClaimMismatch)
        ));
        assert!(!writers.iter_mut().any(reader_closed));

        let mut invalid = manifest.clone();
        invalid.descriptors.pop();
        let claimed = ClaimedInheritedDescriptors::from_owned(owners).unwrap();
        assert!(matches!(
            CandidateDescriptorClaims::bind(&invalid, claimed),
            Err(DescriptorHandoffError::InvalidManifest)
        ));
        assert!(writers.iter_mut().all(reader_closed));
    }

    #[test]
    fn candidate_rebinds_every_typed_role_and_drop_closes_only_unclaimed_owners() {
        let (owners, mut writers) = pipe_owners(CLAIM_ROLES.len());
        let raw_fds = raw_fds(&owners);
        let manifest = claim_manifest(&raw_fds);

        let claimed = ClaimedInheritedDescriptors::from_owned(owners).unwrap();
        let mut claims = CandidateDescriptorClaims::bind(&manifest, claimed).unwrap();
        assert_eq!(claims.len(), CLAIM_ROLES.len());
        assert!(!claims.is_empty());
        assert_eq!(claims.closed_unexpected(), 0);
        for entry in &manifest.descriptors {
            let descriptor = claims.descriptor(entry.slot).unwrap();
            assert_eq!(
                descriptor.as_raw_fd(),
                RawFd::try_from(entry.inherited_fd).unwrap()
            );
            assert!(fcntl_getfd(descriptor).unwrap().contains(FdFlags::CLOEXEC));
        }

        let listener = claims.take(HandoffDescriptorSlot::Listener).unwrap();
        assert_eq!(listener.as_raw_fd(), raw_fds[0]);
        assert!(claims.take(HandoffDescriptorSlot::Listener).is_none());
        assert!(!claims.contains(HandoffDescriptorSlot::Listener));
        assert!(claims.descriptor(HandoffDescriptorSlot::Listener).is_none());
        drop(claims);

        let (listener_writer, unclaimed_writers) = writers.split_first_mut().unwrap();
        assert!(!reader_closed(listener_writer));
        assert!(unclaimed_writers.iter_mut().all(reader_closed));
        drop(listener);
        assert!(reader_closed(listener_writer));
    }
}
