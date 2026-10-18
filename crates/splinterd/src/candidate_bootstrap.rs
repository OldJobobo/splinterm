//! Pre-Tokio candidate acquisition and fallible runtime staging.
//!
//! This module is internal to the `splinterd` library so the body-free manifest
//! ABI and typed claim table remain crate-private. The binary sees only opaque
//! acquisition, bootstrap, and staging types. Its separate first-operation
//! module owns the sole unsafe invocation of the audited inherited-FD leaf
//! crate; nothing here borrows or converts a raw descriptor number.
//!
//! Staging is read-only until every check passes: an exhaustive per-role
//! kernel-type and aliasing pass, then manifest pairing, compiled generation,
//! executable snapshot, listener, checkpoint, terminal, and PTY evidence. Only
//! then are owners transferred out of the claim table.
//!
//! The exclusive single-threaded [`HandoffProcessGuard`] travels with the
//! claimed owners from acquisition through staging and is released only by
//! [`CandidatePreRuntimeStage::enter_tokio`], so no runtime thread can exist
//! while inherited authority is unverified.

use std::{
    collections::BTreeSet,
    ffi::CString,
    fs::File,
    io,
    os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd, RawFd},
    path::Path,
};

use nix::sys::socket::{UnixAddr, getsockname};
use rustix::{
    fs::{FileType, OFlags, SealFlags, Stat, fcntl_get_seals, fstat},
    io::{FdFlags, fcntl_getfd, pread},
    net::{AddressFamily, SocketType, sockopt},
};
use sha2::{Digest, Sha256};
use splinterm_filemap::REQUIRED_IMMUTABLE_SEALS;
use splinterm_pty::{
    AdoptableLinuxPtySession, InheritedChild, LinuxPtyIdentity, PtyError, PtySize,
};
use splinterm_terminal::{
    CheckpointError, MAX_TERMINAL_CHECKPOINT_BYTES, SharedImageBudget, SharedKittyUploadBudget,
    SnapshotRequest, Terminal,
};
use thiserror::Error;

use crate::{
    executable_snapshot::{MAX_EXECUTABLE_SNAPSHOT_BYTES, REQUIRED_EXECUTABLE_SEALS},
    handoff_compatibility::VersionRange,
    handoff_coordinator::{
        HandoffCoordinatorError, HandoffLifecycle, HandoffPhase, HandoffProcessGuard,
    },
    handoff_descriptors::{
        CandidateDescriptorClaims, DescriptorHandoffError, HandoffDescriptorSlot,
    },
    handoff_manifest::{
        BodyDisposition, HandoffManifestV1, MAX_HANDOFF_MANIFEST_BYTES, ManifestBuildVersion,
        ManifestError, ManifestExecutableImage, ManifestKind, ManifestPtySize, ManifestSplint,
        decode_manifest, validate_manifest_pair,
    },
    handoff_preflight::{PreflightError, compiled_handoff_generation},
};

const HASH_BUFFER_BYTES: usize = 64 * 1024;

/// Returns the closed manifest bound for the binary's pre-ownership read.
#[must_use]
#[doc(hidden)]
pub const fn handoff_manifest_max_bytes() -> usize {
    MAX_HANDOFF_MANIFEST_BYTES
}

/// Opaque canonical manifest plus its exact ordered raw descriptor set.
#[derive(Debug)]
#[doc(hidden)]
pub struct CandidateManifestAcquisition {
    manifest: HandoffManifestV1,
    expected_raw_descriptors: Vec<RawFd>,
}

impl CandidateManifestAcquisition {
    /// Returns the exact descriptor order proven by canonical manifest decoding.
    #[must_use]
    #[doc(hidden)]
    pub fn expected_raw_descriptors(&self) -> &[RawFd] {
        &self.expected_raw_descriptors
    }
}

/// Claimed candidate owners bound to one canonical manifest, not yet verified.
///
/// An acquired bootstrap holds the process guard. One recovered from a staging
/// failure does not: it is rollback-only, and staging it again fails closed.
#[derive(Debug)]
pub struct CandidateBootstrap {
    manifest: HandoffManifestV1,
    claims: CandidateDescriptorClaims,
    process_guard: Option<HandoffProcessGuard>,
    lifecycle: HandoffLifecycle,
}

/// Every owner and decoded record that staging verified.
#[derive(Debug)]
pub(crate) struct CandidateStagedAuthorities {
    pub(crate) manifest: HandoffManifestV1,
    pub(crate) counterpart_manifest: HandoffManifestV1,
    pub(crate) splints: Vec<CandidateSplintStage>,
    pub(crate) listener: OwnedFd,
    pub(crate) image_listener: OwnedFd,
    pub(crate) current_manifest_descriptor: OwnedFd,
    pub(crate) counterpart_manifest_descriptor: OwnedFd,
    pub(crate) forward_client: OwnedFd,
    pub(crate) rollback_daemon: OwnedFd,
    pub(crate) rollback_client: OwnedFd,
    pub(crate) closed_unexpected: usize,
}

/// Fully verified candidate authority while the process remains single-threaded.
#[derive(Debug)]
pub struct CandidatePreRuntimeStage {
    lifecycle: HandoffLifecycle,
    process_guard: HandoffProcessGuard,
    authorities: CandidateStagedAuthorities,
}

/// Verified candidate authority held before the first post-checkpoint PTY read,
/// released from the single-threaded boundary for runtime adoption.
#[derive(Debug)]
pub struct CandidateRuntimeStage {
    lifecycle: HandoffLifecycle,
    authorities: CandidateStagedAuthorities,
}

/// One staged Splint: its manifest evidence, restored terminal, inherited
/// PTY, and checkpoint body.
#[derive(Debug)]
pub(crate) struct CandidateSplintStage {
    pub(crate) evidence: ManifestSplint,
    pub(crate) terminal: Terminal,
    pub(crate) pty: AdoptableLinuxPtySession,
    pub(crate) checkpoint_descriptor: OwnedFd,
}

impl CandidatePreRuntimeStage {
    /// Number of unmanifested inherited descriptors the raw claim closed.
    #[must_use]
    pub const fn closed_unexpected(&self) -> usize {
        self.authorities.closed_unexpected
    }

    /// Releases the single-threaded descriptor-table boundary. Runtime
    /// adoption may create Tokio threads and descriptors only after this.
    ///
    /// # Errors
    /// Rejects an internal lifecycle mismatch rather than starting a runtime
    /// from authority that did not complete candidate staging.
    pub fn enter_tokio(self) -> Result<CandidateRuntimeStage, HandoffCoordinatorError> {
        if self.lifecycle.phase() != HandoffPhase::AdoptionStaged {
            return Err(HandoffCoordinatorError::UnexpectedPhase);
        }
        let Self {
            lifecycle,
            process_guard,
            authorities,
        } = self;
        drop(process_guard);
        Ok(CandidateRuntimeStage {
            lifecycle,
            authorities,
        })
    }
}

impl CandidateRuntimeStage {
    /// Number of unmanifested inherited descriptors the raw claim closed.
    #[must_use]
    pub const fn closed_unexpected(&self) -> usize {
        self.authorities.closed_unexpected
    }

    /// Moves the lifecycle and every staged owner out for runtime adoption
    /// (`handoff_adoption`).
    pub(crate) fn into_parts(self) -> (HandoffLifecycle, CandidateStagedAuthorities) {
        (self.lifecycle, self.authorities)
    }

    /// Keeps the newly verified owners alive until the next adoption slice.
    ///
    /// Forward exec is not wired in this slice, so reaching this temporary
    /// boundary is a fatal internal activation error rather than ordinary
    /// daemon startup. The explicit destructure ensures every staged authority
    /// remains represented while the later runtime-adoption API is developed.
    ///
    /// # Errors
    /// Always returns the deliberate bounded adoption-not-wired error after
    /// checking that the complete staged authority remains represented.
    pub fn reject_until_runtime_adoption(self) -> anyhow::Result<()> {
        if self.lifecycle.phase() != HandoffPhase::AdoptionStaged {
            anyhow::bail!("candidate runtime lifecycle invariant failed");
        }
        let CandidateStagedAuthorities {
            manifest,
            counterpart_manifest,
            splints,
            listener,
            image_listener,
            current_manifest_descriptor,
            counterpart_manifest_descriptor,
            forward_client,
            rollback_daemon,
            rollback_client,
            closed_unexpected: _,
        } = self.authorities;
        let staged_splints = splints.len() == manifest.splints.len()
            && splints.iter().all(|stage| {
                stage.evidence.incarnation > 0
                    && stage.terminal.revision().value() == stage.evidence.terminal_revision
                    && stage.pty.master_raw_fd() >= 3
                    && stage.checkpoint_descriptor.as_raw_fd() >= 3
            });
        let retained = [
            listener.as_raw_fd(),
            image_listener.as_raw_fd(),
            current_manifest_descriptor.as_raw_fd(),
            counterpart_manifest_descriptor.as_raw_fd(),
            forward_client.as_raw_fd(),
            rollback_daemon.as_raw_fd(),
            rollback_client.as_raw_fd(),
        ]
        .into_iter()
        .all(|fd| fd >= 3);
        let paired = manifest.handoff_id == counterpart_manifest.handoff_id;
        if !(staged_splints && retained && paired) {
            anyhow::bail!("candidate runtime staging invariant failed");
        }
        anyhow::bail!("candidate runtime adoption is not wired")
    }
}

/// The owners an exec would hand a candidate, for staging in process.
///
/// A full [`CandidateBootstrap::stage`] needs this process to run from the
/// manifest's sealed daemon snapshot, which a test cannot arrange, so tests
/// stage Splints through the same per-Splint verification and skip only the
/// whole-process role, executable, and listener-path passes.
#[cfg(test)]
pub(crate) struct RuntimeStageForTest {
    pub(crate) manifest: HandoffManifestV1,
    pub(crate) counterpart_manifest: HandoffManifestV1,
    /// Each Splint's PTY master and checkpoint body, in manifest order.
    pub(crate) splints: Vec<(OwnedFd, OwnedFd)>,
    pub(crate) listener: OwnedFd,
    pub(crate) image_listener: OwnedFd,
    pub(crate) current_manifest_descriptor: OwnedFd,
    pub(crate) counterpart_manifest_descriptor: OwnedFd,
    pub(crate) forward_client: OwnedFd,
    pub(crate) rollback_daemon: OwnedFd,
    pub(crate) rollback_client: OwnedFd,
}

#[cfg(test)]
impl CandidateRuntimeStage {
    pub(crate) fn stage_for_test(
        parts: RuntimeStageForTest,
        shared_image_budget: &SharedImageBudget,
        shared_kitty_upload_budget: &SharedKittyUploadBudget,
    ) -> Result<Self, CandidateBootstrapError> {
        let mut splints = Vec::with_capacity(parts.splints.len());
        for (evidence, (master, checkpoint)) in
            parts.manifest.splints.iter().cloned().zip(parts.splints)
        {
            let (terminal, identity) = stage_splint(
                &evidence,
                parts.manifest.terminal_checkpoint_schema,
                checkpoint.as_fd(),
                master.as_fd(),
                shared_image_budget,
                shared_kitty_upload_budget,
            )?;
            splints.push(CandidateSplintStage {
                evidence,
                terminal,
                pty: AdoptableLinuxPtySession::from_parts(identity, master),
                checkpoint_descriptor: checkpoint,
            });
        }
        let mut lifecycle = HandoffLifecycle::candidate_claimed();
        lifecycle.transition(HandoffPhase::AdoptionStaged)?;
        Ok(Self {
            lifecycle,
            authorities: CandidateStagedAuthorities {
                manifest: parts.manifest,
                counterpart_manifest: parts.counterpart_manifest,
                splints,
                listener: parts.listener,
                image_listener: parts.image_listener,
                current_manifest_descriptor: parts.current_manifest_descriptor,
                counterpart_manifest_descriptor: parts.counterpart_manifest_descriptor,
                forward_client: parts.forward_client,
                rollback_daemon: parts.rollback_daemon,
                rollback_client: parts.rollback_client,
                closed_unexpected: 0,
            },
        })
    }
}

/// A staging failure that still owns the complete claimed descriptor table.
///
/// It never holds the thread-affine process guard, so it remains an ordinary
/// sendable error.
#[derive(Debug, Error)]
#[error("candidate staging failed")]
pub struct CandidateStageFailure {
    #[source]
    error: CandidateBootstrapError,
    recovery: Box<CandidateRecovery>,
}

/// The claimed owner table and lifecycle a staging failure retains.
#[derive(Debug)]
struct CandidateRecovery {
    manifest: HandoffManifestV1,
    claims: CandidateDescriptorClaims,
    lifecycle: HandoffLifecycle,
}

impl CandidateStageFailure {
    /// Returns the untouched claimed owner table for cooperative rollback.
    ///
    /// The returned bootstrap holds no process guard: a rollback executor must
    /// acquire a fresh single-threaded guard before descriptor mutation or
    /// exec, and staging it again fails closed.
    #[must_use]
    pub fn into_recovery(self) -> CandidateBootstrap {
        let CandidateRecovery {
            manifest,
            claims,
            lifecycle,
        } = *self.recovery;
        CandidateBootstrap {
            manifest,
            claims,
            process_guard: None,
            lifecycle,
        }
    }
}

#[derive(Debug)]
struct VerifiedCandidate {
    counterpart_manifest: HandoffManifestV1,
    terminals: Vec<(Terminal, LinuxPtyIdentity, HandoffDescriptorSlot)>,
}

#[derive(Debug, Error)]
pub(crate) enum CandidateBootstrapError {
    #[error("candidate manifest descriptor is invalid")]
    InvalidActivation,
    #[error("candidate manifest is invalid: {0}")]
    Manifest(#[from] ManifestError),
    #[error("candidate descriptor claim is invalid: {0}")]
    Claim(#[from] DescriptorHandoffError),
    #[error("candidate terminal checkpoint is invalid: {0}")]
    Checkpoint(#[from] CheckpointError),
    #[error("candidate compiled handoff identity is invalid: {0}")]
    Preflight(#[from] PreflightError),
    #[error("candidate handoff lifecycle is invalid: {0}")]
    Coordinator(#[from] HandoffCoordinatorError),
    #[error("candidate inherited PTY is invalid: {0}")]
    Pty(#[from] PtyError),
    #[error("cannot inspect candidate descriptor: {0}")]
    Descriptor(#[source] io::Error),
    #[error("candidate descriptor for {0:?} is missing")]
    MissingSlot(HandoffDescriptorSlot),
    #[error("candidate descriptor for {0:?} has the wrong kernel object type")]
    DescriptorRole(HandoffDescriptorSlot),
    #[error("candidate descriptor role {0:?} is not supported by staging")]
    UnsupportedRole(HandoffDescriptorSlot),
    #[error("candidate descriptor for {0:?} aliases another role")]
    DescriptorAlias(HandoffDescriptorSlot),
    #[error("candidate immutable descriptor does not match manifest authority")]
    DescriptorIdentity,
    #[error("candidate manifest does not name the executing generation")]
    GenerationMismatch,
    #[error("candidate listener descriptors are invalid")]
    InvalidListeners,
    #[error("candidate terminal state does not match manifest evidence")]
    TerminalMismatch,
}

/// Canonical-decodes one bounded manifest body and derives its exact raw set.
///
/// This safe half keeps private manifest types inside `splinterd`; the binary's
/// first-operation module performs the raw read and ownership claim around it.
///
/// # Errors
/// Rejects malformed/noncanonical manifest bytes, an invalid self-link, or a raw
/// descriptor value that cannot be represented by the platform.
#[doc(hidden)]
pub fn decode_candidate_manifest(
    bytes: &[u8],
    manifest_fd: RawFd,
) -> anyhow::Result<CandidateManifestAcquisition> {
    let manifest = decode_manifest(bytes).map_err(CandidateBootstrapError::from)?;
    validate_manifest_self_link(&manifest, manifest_fd)?;
    let expected_raw_descriptors = expected_raw_descriptors(&manifest)?;
    Ok(CandidateManifestAcquisition {
        manifest,
        expected_raw_descriptors,
    })
}

/// Safely binds already-owned descriptors to one decoded candidate manifest.
///
/// The process guard acquired before the raw read moves into the bootstrap and
/// stays held through staging.
///
/// # Errors
/// Rejects any owner count, number, or order mismatch against the validated
/// manifest; the rejected owners are closed and the guard is released.
#[doc(hidden)]
pub fn bind_candidate_manifest(
    acquisition: CandidateManifestAcquisition,
    claimed: splinterm_inherited_fd::ClaimedInheritedDescriptors,
    process_guard: HandoffProcessGuard,
) -> anyhow::Result<CandidateBootstrap> {
    if !claimed.matches_raw_fds(&acquisition.expected_raw_descriptors) {
        return Err(
            CandidateBootstrapError::Claim(DescriptorHandoffError::CandidateClaimMismatch).into(),
        );
    }
    let claims = CandidateDescriptorClaims::bind(&acquisition.manifest, claimed)
        .map_err(CandidateBootstrapError::from)?;
    Ok(CandidateBootstrap {
        manifest: acquisition.manifest,
        claims,
        process_guard: Some(process_guard),
        lifecycle: HandoffLifecycle::candidate_claimed(),
    })
}

impl CandidateBootstrap {
    /// Completes every fallible descriptor and terminal check without reading a
    /// PTY or accepting a connection, then transfers owners into a staging set.
    /// A validation failure returns the complete claimed owner table for future
    /// cooperative rollback. Success keeps the process guard in the returned
    /// pre-runtime stage; failure releases it.
    ///
    /// # Errors
    /// Returns typed validation failure together with the complete recovery
    /// owner table when any manifest, descriptor, PTY, or terminal check fails,
    /// or when the bootstrap no longer holds the process guard because it was
    /// recovered from an earlier failure.
    pub fn stage(
        mut self,
        shared_image_budget: &SharedImageBudget,
        shared_kitty_upload_budget: &SharedKittyUploadBudget,
        listener_path: &Path,
        image_listener_path: &Path,
    ) -> Result<CandidatePreRuntimeStage, CandidateStageFailure> {
        // Only a freshly claimed table stages; a recovered one is rolling back.
        if self.lifecycle.phase() != HandoffPhase::CandidateClaimed {
            return Err(self.into_stage_failure(HandoffCoordinatorError::UnexpectedPhase.into()));
        }
        let Some(process_guard) = self.process_guard.take() else {
            return Err(self.into_stage_failure(HandoffCoordinatorError::UnexpectedPhase.into()));
        };
        let verified = match self.verify(
            shared_image_budget,
            shared_kitty_upload_budget,
            listener_path,
            image_listener_path,
        ) {
            Ok(verified) => verified,
            Err(error) => return Err(self.into_stage_failure(error)),
        };
        if let Err(error) = self.lifecycle.transition(HandoffPhase::AdoptionStaged) {
            return Err(self.into_stage_failure(error.into()));
        }
        Ok(self.transfer_verified(verified, process_guard))
    }

    /// Retains the complete owner table without the thread-affine guard. The
    /// lifecycle moves to rolling back, so the phase machine itself, not only
    /// the missing guard, refuses to stage the recovered table again.
    fn into_stage_failure(self, error: CandidateBootstrapError) -> CandidateStageFailure {
        let Self {
            manifest,
            claims,
            process_guard,
            mut lifecycle,
        } = self;
        drop(process_guard);
        if lifecycle.phase() == HandoffPhase::CandidateClaimed {
            // Always a valid edge from the only phase that stages.
            let _ = lifecycle.transition(HandoffPhase::RollingBack);
        }
        CandidateStageFailure {
            error,
            recovery: Box::new(CandidateRecovery {
                manifest,
                claims,
                lifecycle,
            }),
        }
    }

    fn verify(
        &self,
        shared_image_budget: &SharedImageBudget,
        shared_kitty_upload_budget: &SharedKittyUploadBudget,
        listener_path: &Path,
        image_listener_path: &Path,
    ) -> Result<VerifiedCandidate, CandidateBootstrapError> {
        self.verify_descriptor_roles()?;
        let counterpart_manifest = self.verify_manifests()?;
        verify_listener_path(
            self.required(HandoffDescriptorSlot::Listener)?,
            listener_path,
        )?;
        verify_listener_path(
            self.required(HandoffDescriptorSlot::ImageListener)?,
            image_listener_path,
        )?;
        self.verify_compiled_generation()?;
        self.verify_executables()?;
        let terminals = self.verify_splints(shared_image_budget, shared_kitty_upload_budget)?;
        Ok(VerifiedCandidate {
            counterpart_manifest,
            terminals,
        })
    }

    /// Requires every claimed descriptor to be the kernel object its role
    /// names, and no two roles to share one object.
    ///
    /// The match is exhaustive so a new role cannot be staged without a type
    /// check. Roles with no staging consumer fail closed.
    fn verify_descriptor_roles(&self) -> Result<(), CandidateBootstrapError> {
        #[derive(Eq, Ord, PartialEq, PartialOrd)]
        enum ObjectKey {
            Inode(u64, u64),
            Pty(CString),
        }

        if self.claims.len() != self.manifest.descriptors.len() {
            return Err(CandidateBootstrapError::Claim(
                DescriptorHandoffError::CandidateClaimMismatch,
            ));
        }
        let mut objects = BTreeSet::new();
        for entry in &self.manifest.descriptors {
            let slot = entry.slot;
            let descriptor = self.required(slot)?;
            let flags = fcntl_getfd(descriptor).map_err(errno_descriptor)?;
            if RawFd::try_from(entry.inherited_fd).ok() != Some(descriptor.as_raw_fd())
                || !flags.contains(FdFlags::CLOEXEC)
            {
                return Err(CandidateBootstrapError::DescriptorRole(slot));
            }
            let stat = fstat(descriptor).map_err(errno_descriptor)?;
            let object = match slot {
                HandoffDescriptorSlot::CheckpointManifest
                | HandoffDescriptorSlot::RollbackManifest
                | HandoffDescriptorSlot::TerminalCheckpoint(_) => {
                    require_sealed_regular_file(descriptor, &stat, REQUIRED_IMMUTABLE_SEALS, slot)?;
                    ObjectKey::Inode(stat.st_dev, stat.st_ino)
                }
                HandoffDescriptorSlot::ForwardClient
                | HandoffDescriptorSlot::RollbackDaemon
                | HandoffDescriptorSlot::RollbackClient => {
                    require_sealed_regular_file(
                        descriptor,
                        &stat,
                        REQUIRED_EXECUTABLE_SEALS,
                        slot,
                    )?;
                    ObjectKey::Inode(stat.st_dev, stat.st_ino)
                }
                HandoffDescriptorSlot::Listener | HandoffDescriptorSlot::ImageListener => {
                    require_unix_stream_listener(descriptor, &stat, slot)?;
                    ObjectKey::Inode(stat.st_dev, stat.st_ino)
                }
                // Every PTY master shares the devpts `ptmx` inode, so its
                // object identity is the PTY index instead.
                HandoffDescriptorSlot::PtyMaster(_) => {
                    ObjectKey::Pty(require_pty_master(descriptor, &stat, slot)?)
                }
                // Schema-v2 checkpoints embed image state, and client
                // continuation records are not defined by manifest schema v1.
                HandoffDescriptorSlot::ImageState(_)
                | HandoffDescriptorSlot::ClientProcess(_)
                | HandoffDescriptorSlot::Continuation(_) => {
                    return Err(CandidateBootstrapError::UnsupportedRole(slot));
                }
            };
            if !objects.insert(object) {
                return Err(CandidateBootstrapError::DescriptorAlias(slot));
            }
        }
        Ok(())
    }

    /// Rereads the current manifest from its claimed owner and pairs it with
    /// the counterpart manifest.
    fn verify_manifests(&self) -> Result<HandoffManifestV1, CandidateBootstrapError> {
        let current_manifest_bytes = read_immutable_descriptor(
            self.required(current_manifest_slot(self.manifest.kind))?,
            MAX_HANDOFF_MANIFEST_BYTES,
        )?;
        if decode_manifest(&current_manifest_bytes)? != self.manifest {
            return Err(CandidateBootstrapError::DescriptorIdentity);
        }
        let counterpart_bytes = read_immutable_descriptor(
            self.required(self.manifest.counterpart_manifest_slot)?,
            MAX_HANDOFF_MANIFEST_BYTES,
        )?;
        let counterpart_manifest = decode_manifest(&counterpart_bytes)?;
        match self.manifest.kind {
            ManifestKind::Forward => validate_manifest_pair(&self.manifest, &counterpart_manifest)?,
            ManifestKind::Rollback => {
                validate_manifest_pair(&counterpart_manifest, &self.manifest)?;
            }
        }
        Ok(counterpart_manifest)
    }

    /// Requires the manifest's target generation and every negotiated version
    /// to describe the exact code now executing.
    fn verify_compiled_generation(&self) -> Result<(), CandidateBootstrapError> {
        let compiled = compiled_handoff_generation()?;
        let capabilities = compiled.capabilities;
        let supports =
            |range: VersionRange, value| range.minimum() <= value && value <= range.maximum();
        if self.manifest.target_generation.build_identity != compiled.build_identity.as_bytes()
            || self.manifest.target_generation.build_version
                != ManifestBuildVersion::from_build_version(compiled.build_version)
            || !supports(
                capabilities.private_protocol,
                self.manifest.private_protocol,
            )
            || !supports(
                capabilities.handoff_protocol,
                self.manifest.handoff_protocol,
            )
            || !supports(
                capabilities.terminal_checkpoint,
                self.manifest.terminal_checkpoint_schema,
            )
            || !supports(
                capabilities.descriptor_manifest,
                self.manifest.descriptor_manifest_schema,
            )
        {
            return Err(CandidateBootstrapError::GenerationMismatch);
        }
        Ok(())
    }

    /// Rebinds the executing image and every retained executable snapshot.
    fn verify_executables(&self) -> Result<(), CandidateBootstrapError> {
        let pairs = &self.manifest.executable_pairs;
        let current_image = match self.manifest.kind {
            ManifestKind::Forward => &pairs.forward.daemon,
            ManifestKind::Rollback => &pairs.rollback.daemon,
        };
        let current_executable =
            File::open("/proc/self/exe").map_err(CandidateBootstrapError::Descriptor)?;
        verify_executable(current_executable.as_fd(), current_image)?;
        verify_executable(
            self.required(HandoffDescriptorSlot::ForwardClient)?,
            &pairs.forward.client,
        )?;
        verify_executable(
            self.required(HandoffDescriptorSlot::RollbackDaemon)?,
            &pairs.rollback.daemon,
        )?;
        verify_executable(
            self.required(HandoffDescriptorSlot::RollbackClient)?,
            &pairs.rollback.client,
        )
    }

    /// Restores every checkpoint and checks it and its PTY against the
    /// body-free manifest evidence, without reading any PTY.
    fn verify_splints(
        &self,
        shared_image_budget: &SharedImageBudget,
        shared_kitty_upload_budget: &SharedKittyUploadBudget,
    ) -> Result<Vec<(Terminal, LinuxPtyIdentity, HandoffDescriptorSlot)>, CandidateBootstrapError>
    {
        let mut terminals = Vec::with_capacity(self.manifest.splints.len());
        for splint in &self.manifest.splints {
            let checkpoint_slot = terminal_checkpoint_slot(splint)?;
            let (terminal, identity) = stage_splint(
                splint,
                self.manifest.terminal_checkpoint_schema,
                self.required(checkpoint_slot)?,
                self.required(HandoffDescriptorSlot::PtyMaster(splint.pty.master_slot))?,
                shared_image_budget,
                shared_kitty_upload_budget,
            )?;
            terminals.push((terminal, identity, checkpoint_slot));
        }
        Ok(terminals)
    }

    fn transfer_verified(
        mut self,
        verified: VerifiedCandidate,
        process_guard: HandoffProcessGuard,
    ) -> CandidatePreRuntimeStage {
        let current_manifest_slot = current_manifest_slot(self.manifest.kind);
        let counterpart_manifest_slot = self.manifest.counterpart_manifest_slot;
        let evidence = self.manifest.splints.clone();
        let mut splints = Vec::with_capacity(evidence.len());
        for (evidence, (terminal, identity, checkpoint_slot)) in
            evidence.into_iter().zip(verified.terminals)
        {
            let checkpoint_descriptor = self.take_verified(checkpoint_slot);
            let master =
                self.take_verified(HandoffDescriptorSlot::PtyMaster(evidence.pty.master_slot));
            splints.push(CandidateSplintStage {
                evidence,
                terminal,
                pty: AdoptableLinuxPtySession::from_parts(identity, master),
                checkpoint_descriptor,
            });
        }
        let authorities = CandidateStagedAuthorities {
            manifest: self.manifest.clone(),
            counterpart_manifest: verified.counterpart_manifest,
            splints,
            listener: self.take_verified(HandoffDescriptorSlot::Listener),
            image_listener: self.take_verified(HandoffDescriptorSlot::ImageListener),
            current_manifest_descriptor: self.take_verified(current_manifest_slot),
            counterpart_manifest_descriptor: self.take_verified(counterpart_manifest_slot),
            forward_client: self.take_verified(HandoffDescriptorSlot::ForwardClient),
            rollback_daemon: self.take_verified(HandoffDescriptorSlot::RollbackDaemon),
            rollback_client: self.take_verified(HandoffDescriptorSlot::RollbackClient),
            closed_unexpected: self.claims.closed_unexpected(),
        };
        // The role pass rejected every role without a staged consumer.
        debug_assert!(self.claims.is_empty());
        CandidatePreRuntimeStage {
            lifecycle: self.lifecycle,
            process_guard,
            authorities,
        }
    }

    fn required(
        &self,
        slot: HandoffDescriptorSlot,
    ) -> Result<BorrowedFd<'_>, CandidateBootstrapError> {
        self.claims
            .descriptor(slot)
            .ok_or(CandidateBootstrapError::MissingSlot(slot))
    }

    fn take_verified(&mut self, slot: HandoffDescriptorSlot) -> OwnedFd {
        self.claims
            .take(slot)
            .expect("read-only verification proved the complete closed slot set")
    }
}

/// Restores one checkpoint and checks it and its inherited PTY against the
/// body-free manifest evidence, without reading the PTY.
///
/// A child that exited after capture and is still unreaped is accepted
/// (Plan 0061 M3.7b review): the adopting generation reaps it and publishes
/// its exit like any other, so a shell that exits in the exec window does not
/// roll back the whole upgrade. Everything else about such a Splint is still
/// verified: the checkpoint, the master, the kernel size, and the unreaped
/// direct child's process group and session.
fn stage_splint(
    splint: &ManifestSplint,
    checkpoint_schema: u16,
    checkpoint: BorrowedFd<'_>,
    master: BorrowedFd<'_>,
    shared_image_budget: &SharedImageBudget,
    shared_kitty_upload_budget: &SharedKittyUploadBudget,
) -> Result<(Terminal, LinuxPtyIdentity), CandidateBootstrapError> {
    let checkpoint_bytes = read_immutable_descriptor(checkpoint, MAX_TERMINAL_CHECKPOINT_BYTES)?;
    if Terminal::validate_checkpoint_encoding(&checkpoint_bytes)? != checkpoint_schema {
        return Err(CandidateBootstrapError::TerminalMismatch);
    }
    let terminal = Terminal::restore_handoff_checkpoint(
        &checkpoint_bytes,
        shared_image_budget.clone(),
        shared_kitty_upload_budget.clone(),
    )?;
    if !terminal_matches_manifest_state(
        &terminal,
        splint.size,
        splint.terminal_revision,
        splint.history_generation,
    ) {
        return Err(CandidateBootstrapError::TerminalMismatch);
    }
    let identity = LinuxPtyIdentity::from_raw(
        splint.pty.child_pid,
        splint.pty.process_group,
        splint.pty.session_id,
    )?;
    let foreground = splint
        .foreground_process_group
        .ok_or(CandidateBootstrapError::TerminalMismatch)?;
    // Running or exited-and-unreaped are both carried; see above.
    let _: InheritedChild = identity.verify_inherited_allowing_exit(
        master,
        PtySize {
            rows: splint.size.rows,
            columns: splint.size.columns,
            pixel_width: splint.size.pixel_width,
            pixel_height: splint.size.pixel_height,
        },
        foreground,
    )?;
    Ok((terminal, identity))
}

fn current_manifest_slot(kind: ManifestKind) -> HandoffDescriptorSlot {
    match kind {
        ManifestKind::Forward => HandoffDescriptorSlot::CheckpointManifest,
        ManifestKind::Rollback => HandoffDescriptorSlot::RollbackManifest,
    }
}

fn require_sealed_regular_file(
    descriptor: BorrowedFd<'_>,
    stat: &Stat,
    required: SealFlags,
    slot: HandoffDescriptorSlot,
) -> Result<(), CandidateBootstrapError> {
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile || stat.st_size <= 0 {
        return Err(CandidateBootstrapError::DescriptorRole(slot));
    }
    match fcntl_get_seals(descriptor) {
        Ok(seals) if seals.contains(required) => Ok(()),
        Ok(_) | Err(rustix::io::Errno::INVAL) => Err(CandidateBootstrapError::DescriptorRole(slot)),
        Err(error) => Err(errno_descriptor(error)),
    }
}

pub(crate) fn require_unix_stream_listener(
    descriptor: BorrowedFd<'_>,
    stat: &Stat,
    slot: HandoffDescriptorSlot,
) -> Result<(), CandidateBootstrapError> {
    if FileType::from_raw_mode(stat.st_mode) != FileType::Socket {
        return Err(CandidateBootstrapError::DescriptorRole(slot));
    }
    let domain = sockopt::socket_domain(descriptor).map_err(errno_descriptor)?;
    let socket_type = sockopt::socket_type(descriptor).map_err(errno_descriptor)?;
    let accepting = sockopt::socket_acceptconn(descriptor).map_err(errno_descriptor)?;
    let status_flags = rustix::fs::fcntl_getfl(descriptor).map_err(errno_descriptor)?;
    if domain != AddressFamily::UNIX
        || socket_type != SocketType::STREAM
        || !accepting
        || !status_flags.contains(OFlags::NONBLOCK)
    {
        return Err(CandidateBootstrapError::DescriptorRole(slot));
    }
    Ok(())
}

/// Returns the PTY's slave name, which identifies it uniquely, only for a
/// read-write Unix98 PTY master.
fn require_pty_master(
    descriptor: BorrowedFd<'_>,
    stat: &Stat,
    slot: HandoffDescriptorSlot,
) -> Result<CString, CandidateBootstrapError> {
    let status_flags = rustix::fs::fcntl_getfl(descriptor).map_err(errno_descriptor)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::CharacterDevice
        || status_flags & OFlags::ACCMODE != OFlags::RDWR
    {
        return Err(CandidateBootstrapError::DescriptorRole(slot));
    }
    match splinterm_pty::verify_pty_master(descriptor) {
        Ok(()) => {}
        Err(PtyError::IdentityMismatch) => {
            return Err(CandidateBootstrapError::DescriptorRole(slot));
        }
        Err(error) => return Err(error.into()),
    }
    rustix::pty::ptsname(descriptor, Vec::new())
        .map_err(|_| CandidateBootstrapError::DescriptorRole(slot))
}

fn terminal_matches_manifest_state(
    terminal: &Terminal,
    size: ManifestPtySize,
    terminal_revision: u64,
    history_generation: u64,
) -> bool {
    let snapshot = terminal.snapshot(SnapshotRequest {
        max_scrollback_rows: 0,
    });
    let dimensions = snapshot.dimensions();
    terminal.revision().value() == terminal_revision
        && snapshot.scrollback().history_generation == history_generation
        && dimensions.columns == usize::from(size.columns)
        && dimensions.rows == usize::from(size.rows)
        && terminal.cell_pixel_size() == manifest_cell_pixels(size)
}

/// Mirrors the actor's cell geometry: zero pixel extents mean unknown.
fn manifest_cell_pixels(size: ManifestPtySize) -> Option<(u32, u32)> {
    let width = u32::from(size.pixel_width).checked_div(u32::from(size.columns))?;
    let height = u32::from(size.pixel_height).checked_div(u32::from(size.rows))?;
    (width > 0 && height > 0).then_some((width, height))
}

fn terminal_checkpoint_slot(
    splint: &ManifestSplint,
) -> Result<HandoffDescriptorSlot, CandidateBootstrapError> {
    match splint.terminal_checkpoint {
        BodyDisposition::Descriptor(slot) => Ok(HandoffDescriptorSlot::TerminalCheckpoint(slot)),
        BodyDisposition::NotPresent | BodyDisposition::Unsupported => {
            Err(CandidateBootstrapError::TerminalMismatch)
        }
    }
}

fn validate_manifest_self_link(
    manifest: &HandoffManifestV1,
    manifest_fd: RawFd,
) -> Result<(), CandidateBootstrapError> {
    let slot = current_manifest_slot(manifest.kind);
    let expected =
        u32::try_from(manifest_fd).map_err(|_| CandidateBootstrapError::InvalidActivation)?;
    if manifest
        .descriptors
        .iter()
        .any(|entry| entry.slot == slot && entry.inherited_fd == expected)
    {
        Ok(())
    } else {
        Err(CandidateBootstrapError::InvalidActivation)
    }
}

fn expected_raw_descriptors(
    manifest: &HandoffManifestV1,
) -> Result<Vec<RawFd>, CandidateBootstrapError> {
    manifest
        .descriptors
        .iter()
        .map(|entry| {
            RawFd::try_from(entry.inherited_fd)
                .map_err(|_| CandidateBootstrapError::InvalidActivation)
        })
        .collect()
}

fn read_immutable_descriptor(
    descriptor: BorrowedFd<'_>,
    maximum_bytes: usize,
) -> Result<Vec<u8>, CandidateBootstrapError> {
    let stat = fstat(descriptor).map_err(errno_descriptor)?;
    let size = usize::try_from(stat.st_size)
        .ok()
        .filter(|size| *size > 0 && *size <= maximum_bytes)
        .ok_or(CandidateBootstrapError::DescriptorIdentity)?;
    let seals = fcntl_get_seals(descriptor).map_err(errno_descriptor)?;
    let flags = fcntl_getfd(descriptor).map_err(errno_descriptor)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || !seals.contains(REQUIRED_IMMUTABLE_SEALS)
        || !flags.contains(FdFlags::CLOEXEC)
    {
        return Err(CandidateBootstrapError::DescriptorIdentity);
    }
    splinterm_filemap::verify_writable_shared_mapping_rejected(descriptor, size)
        .map_err(CandidateBootstrapError::Descriptor)?;
    let mut bytes = vec![0_u8; size];
    let mut total = 0_usize;
    while total < size {
        let offset =
            u64::try_from(total).map_err(|_| CandidateBootstrapError::DescriptorIdentity)?;
        let count = pread(descriptor, &mut bytes[total..], offset).map_err(errno_descriptor)?;
        if count == 0 {
            return Err(CandidateBootstrapError::DescriptorIdentity);
        }
        // `pread` never returns more than the remaining slice length.
        total += count;
    }
    let trailing_offset =
        u64::try_from(size).map_err(|_| CandidateBootstrapError::DescriptorIdentity)?;
    if pread(descriptor, &mut [0_u8; 1], trailing_offset).map_err(errno_descriptor)? != 0 {
        return Err(CandidateBootstrapError::DescriptorIdentity);
    }
    Ok(bytes)
}

fn verify_executable(
    descriptor: BorrowedFd<'_>,
    expected: &ManifestExecutableImage,
) -> Result<(), CandidateBootstrapError> {
    let stat = fstat(descriptor).map_err(errno_descriptor)?;
    let size =
        u64::try_from(stat.st_size).map_err(|_| CandidateBootstrapError::DescriptorIdentity)?;
    let seals = fcntl_get_seals(descriptor).map_err(errno_descriptor)?;
    let flags = fcntl_getfd(descriptor).map_err(errno_descriptor)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || size == 0
        || size > MAX_EXECUTABLE_SNAPSHOT_BYTES
        || stat.st_dev != expected.snapshot_device
        || stat.st_ino != expected.snapshot_inode
        || size != expected.snapshot_size
        || !seals.contains(REQUIRED_EXECUTABLE_SEALS)
        || seals.bits() != expected.snapshot_seals
        || !flags.contains(FdFlags::CLOEXEC)
        || hash_descriptor(descriptor, size)? != expected.snapshot_sha256
    {
        return Err(CandidateBootstrapError::DescriptorIdentity);
    }
    let map_size =
        usize::try_from(size).map_err(|_| CandidateBootstrapError::DescriptorIdentity)?;
    splinterm_filemap::verify_writable_shared_mapping_rejected(descriptor, map_size)
        .map_err(CandidateBootstrapError::Descriptor)
}

fn hash_descriptor(
    descriptor: BorrowedFd<'_>,
    expected_size: u64,
) -> Result<[u8; 32], CandidateBootstrapError> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES].into_boxed_slice();
    let mut total = 0_u64;
    loop {
        let count = pread(descriptor, &mut *buffer, total).map_err(errno_descriptor)?;
        if count == 0 {
            break;
        }
        total = u64::try_from(count)
            .ok()
            .and_then(|count| total.checked_add(count))
            .filter(|total| *total <= expected_size)
            .ok_or(CandidateBootstrapError::DescriptorIdentity)?;
        hasher.update(&buffer[..count]);
    }
    if total != expected_size {
        return Err(CandidateBootstrapError::DescriptorIdentity);
    }
    Ok(hasher.finalize().into())
}

fn verify_listener_path(
    descriptor: BorrowedFd<'_>,
    expected_path: &Path,
) -> Result<(), CandidateBootstrapError> {
    let address = getsockname::<UnixAddr>(descriptor.as_raw_fd())
        .map_err(|error| CandidateBootstrapError::Descriptor(io::Error::from(error)))?;
    if address.path() != Some(expected_path) {
        return Err(CandidateBootstrapError::InvalidListeners);
    }
    Ok(())
}

fn errno_descriptor(error: rustix::io::Errno) -> CandidateBootstrapError {
    CandidateBootstrapError::Descriptor(io::Error::from_raw_os_error(error.raw_os_error()))
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::{UnixListener, UnixStream};

    use rustix::{
        fs::{MemfdFlags, fcntl_add_seals, memfd_create},
        io::write,
        pty::{OpenptFlags, grantpt, openpt, unlockpt},
    };
    use splinterm_inherited_fd::ClaimedInheritedDescriptors;
    use splinterm_terminal::TerminalConfig;

    use super::*;
    use crate::handoff_manifest::{
        ExecutablePairSetEvidence, GenerationEvidence, HandoffId, ManifestDescriptor,
        ManifestExecutablePair, validate_manifest,
    };

    #[test]
    fn restored_terminal_must_match_manifest_dimensions_and_pixel_geometry() {
        let mut terminal = Terminal::new(12, 4, TerminalConfig::default());
        terminal.set_cell_pixel_size(8, 16);
        let snapshot = terminal.snapshot(SnapshotRequest {
            max_scrollback_rows: 0,
        });
        let revision = terminal.revision().value();
        let history_generation = snapshot.scrollback().history_generation;
        let size = ManifestPtySize {
            columns: 12,
            rows: 4,
            pixel_width: 96,
            pixel_height: 64,
        };

        assert!(terminal_matches_manifest_state(
            &terminal,
            size,
            revision,
            history_generation,
        ));
        for mismatched in [
            ManifestPtySize {
                columns: 13,
                ..size
            },
            ManifestPtySize { rows: 5, ..size },
            ManifestPtySize {
                pixel_width: 108,
                ..size
            },
            ManifestPtySize {
                pixel_height: 0,
                ..size
            },
        ] {
            assert!(!terminal_matches_manifest_state(
                &terminal,
                mismatched,
                revision,
                history_generation,
            ));
        }
        assert!(!terminal_matches_manifest_state(
            &terminal,
            size,
            revision.saturating_add(1),
            history_generation,
        ));
        assert!(!terminal_matches_manifest_state(
            &terminal,
            size,
            revision,
            history_generation.saturating_add(1),
        ));
    }

    fn sealed_memfd(bytes: &[u8], seals: SealFlags) -> OwnedFd {
        let fd = memfd_create(
            "candidate-role-test",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        )
        .unwrap();
        let mut written = 0;
        while written < bytes.len() {
            written += write(&fd, &bytes[written..]).unwrap();
        }
        if !seals.is_empty() {
            fcntl_add_seals(&fd, seals).unwrap();
        }
        fd
    }

    fn body() -> OwnedFd {
        sealed_memfd(b"sealed body", REQUIRED_IMMUTABLE_SEALS)
    }

    fn listener(directory: &Path, name: &str) -> OwnedFd {
        let listener = UnixListener::bind(directory.join(name)).unwrap();
        listener.set_nonblocking(true).unwrap();
        listener.into()
    }

    fn pty_master() -> OwnedFd {
        let master =
            openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC).unwrap();
        grantpt(&master).unwrap();
        unlockpt(&master).unwrap();
        master
    }

    fn pty_slave(master: &OwnedFd) -> OwnedFd {
        let path = rustix::pty::ptsname(master, Vec::new()).unwrap();
        rustix::fs::open(
            path.as_c_str(),
            OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .unwrap()
    }

    fn generation(patch: u16) -> GenerationEvidence {
        GenerationEvidence {
            build_identity: [u8::try_from(patch).unwrap(); 32],
            build_version: ManifestBuildVersion {
                major: 0,
                minor: 2,
                patch,
            },
        }
    }

    fn image(byte: u8) -> ManifestExecutableImage {
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

    /// Correct owner for each idle role, in canonical manifest order.
    struct IdleOwners {
        directory: std::path::PathBuf,
        owners: Vec<(HandoffDescriptorSlot, OwnedFd)>,
    }

    impl Drop for IdleOwners {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    fn idle_owners() -> IdleOwners {
        // Parallel tests can read the same clock value; the sequence keeps
        // each fixture directory unique.
        static NEXT_DIRECTORY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let directory = std::env::temp_dir().join(format!(
            "sd-roles-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_DIRECTORY.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let owners = vec![
            (
                HandoffDescriptorSlot::Listener,
                listener(&directory, "daemon.sock"),
            ),
            (
                HandoffDescriptorSlot::ImageListener,
                listener(&directory, "image.sock"),
            ),
            (HandoffDescriptorSlot::CheckpointManifest, body()),
            (HandoffDescriptorSlot::RollbackManifest, body()),
            (HandoffDescriptorSlot::ForwardClient, body()),
            (HandoffDescriptorSlot::RollbackDaemon, body()),
            (HandoffDescriptorSlot::RollbackClient, body()),
        ];
        IdleOwners { directory, owners }
    }

    /// Binds `owners` to a valid manifest naming exactly their numbers.
    fn bootstrap(owners: Vec<(HandoffDescriptorSlot, OwnedFd)>) -> CandidateBootstrap {
        let mut manifest = HandoffManifestV1 {
            schema: 1,
            kind: ManifestKind::Forward,
            handoff_id: HandoffId::new([9; 16]).unwrap(),
            source_generation: generation(1),
            target_generation: generation(2),
            source_daemon_generation: 1,
            target_daemon_generation: 2,
            topology_revision: 0,
            durable_state_generation: 0,
            private_protocol: splinterm_protocol::PROTOCOL_VERSION,
            handoff_protocol: 1,
            terminal_checkpoint_schema: 2,
            descriptor_manifest_schema: 1,
            install_prefix: std::path::PathBuf::from("/usr/bin"),
            executable_pairs: ExecutablePairSetEvidence {
                forward: ManifestExecutablePair {
                    daemon: image(2),
                    client: image(3),
                    generation: generation(2),
                },
                rollback: ManifestExecutablePair {
                    daemon: image(4),
                    client: image(5),
                    generation: generation(1),
                },
            },
            counterpart_manifest_slot: HandoffDescriptorSlot::RollbackManifest,
            splints: Vec::new(),
            descriptors: owners
                .iter()
                .map(|(slot, owner)| ManifestDescriptor {
                    slot: *slot,
                    inherited_fd: u32::try_from(owner.as_raw_fd()).unwrap(),
                })
                .collect(),
        };
        manifest.descriptors.sort_by_key(|entry| entry.slot);
        validate_manifest(&manifest).unwrap();
        let mut owners = owners;
        owners.sort_by_key(|(slot, _)| *slot);
        let claimed = ClaimedInheritedDescriptors::from_owned(
            owners.into_iter().map(|(_, owner)| owner).collect(),
        )
        .unwrap();
        let claims = CandidateDescriptorClaims::bind(&manifest, claimed).unwrap();
        CandidateBootstrap {
            manifest,
            claims,
            process_guard: None,
            lifecycle: HandoffLifecycle::candidate_claimed(),
        }
    }

    fn replace_role(
        owners: &mut [(HandoffDescriptorSlot, OwnedFd)],
        slot: HandoffDescriptorSlot,
        replacement: OwnedFd,
    ) {
        owners
            .iter_mut()
            .find(|(candidate, _)| *candidate == slot)
            .unwrap()
            .1 = replacement;
    }

    /// A role paired with a constructor for a kernel object of the wrong type.
    type WrongKernelObject = (HandoffDescriptorSlot, fn(&Path) -> OwnedFd);

    #[test]
    fn every_idle_role_accepts_only_its_kernel_object_type() {
        let mut idle = idle_owners();
        bootstrap(std::mem::take(&mut idle.owners))
            .verify_descriptor_roles()
            .unwrap();

        let wrong_types: [WrongKernelObject; 7] = [
            (HandoffDescriptorSlot::Listener, |_| body()),
            (HandoffDescriptorSlot::ImageListener, |_| pty_master()),
            (HandoffDescriptorSlot::CheckpointManifest, |directory| {
                listener(directory, "wrong-manifest.sock")
            }),
            (HandoffDescriptorSlot::RollbackManifest, |_| {
                sealed_memfd(b"unsealed", SealFlags::empty())
            }),
            (HandoffDescriptorSlot::ForwardClient, |_| {
                sealed_memfd(b"", REQUIRED_EXECUTABLE_SEALS)
            }),
            (HandoffDescriptorSlot::RollbackDaemon, |_| {
                OwnedFd::from(File::open("/dev/null").unwrap())
            }),
            (HandoffDescriptorSlot::RollbackClient, |_| {
                OwnedFd::from(UnixStream::pair().unwrap().0)
            }),
        ];
        for (slot, wrong) in wrong_types {
            let mut idle = idle_owners();
            let replacement = wrong(&idle.directory);
            replace_role(&mut idle.owners, slot, replacement);
            let result = bootstrap(std::mem::take(&mut idle.owners)).verify_descriptor_roles();
            assert!(
                matches!(result, Err(CandidateBootstrapError::DescriptorRole(role)) if role == slot),
                "{slot:?} accepted a wrong object: {result:?}"
            );
        }
    }

    #[test]
    fn listener_role_rejects_blocking_and_non_listening_sockets() {
        let mut idle = idle_owners();
        let blocking = UnixListener::bind(idle.directory.join("blocking.sock")).unwrap();
        replace_role(
            &mut idle.owners,
            HandoffDescriptorSlot::Listener,
            blocking.into(),
        );
        assert!(matches!(
            bootstrap(std::mem::take(&mut idle.owners)).verify_descriptor_roles(),
            Err(CandidateBootstrapError::DescriptorRole(
                HandoffDescriptorSlot::Listener
            ))
        ));

        let mut idle = idle_owners();
        let (connected, _peer) = UnixStream::pair().unwrap();
        connected.set_nonblocking(true).unwrap();
        replace_role(
            &mut idle.owners,
            HandoffDescriptorSlot::ImageListener,
            connected.into(),
        );
        assert!(matches!(
            bootstrap(std::mem::take(&mut idle.owners)).verify_descriptor_roles(),
            Err(CandidateBootstrapError::DescriptorRole(
                HandoffDescriptorSlot::ImageListener
            ))
        ));
    }

    #[test]
    fn duplicated_objects_cannot_fill_two_roles() {
        let mut idle = idle_owners();
        let shared = body();
        let duplicate = shared.try_clone().unwrap();
        replace_role(
            &mut idle.owners,
            HandoffDescriptorSlot::ForwardClient,
            shared,
        );
        replace_role(
            &mut idle.owners,
            HandoffDescriptorSlot::RollbackClient,
            duplicate,
        );
        assert!(matches!(
            bootstrap(std::mem::take(&mut idle.owners)).verify_descriptor_roles(),
            Err(CandidateBootstrapError::DescriptorAlias(
                HandoffDescriptorSlot::RollbackClient
            ))
        ));
    }

    #[test]
    fn pty_master_role_rejects_slaves_and_aliases() {
        let master = pty_master();
        let stat = fstat(&master).unwrap();
        let name =
            require_pty_master(master.as_fd(), &stat, HandoffDescriptorSlot::PtyMaster(0)).unwrap();
        assert_eq!(name, rustix::pty::ptsname(&master, Vec::new()).unwrap());

        let slave = pty_slave(&master);
        let slave_stat = fstat(&slave).unwrap();
        assert!(matches!(
            require_pty_master(
                slave.as_fd(),
                &slave_stat,
                HandoffDescriptorSlot::PtyMaster(0)
            ),
            Err(CandidateBootstrapError::DescriptorRole(
                HandoffDescriptorSlot::PtyMaster(0)
            ))
        ));
        let null = File::open("/dev/null").unwrap();
        let null_stat = fstat(&null).unwrap();
        assert!(matches!(
            require_pty_master(
                null.as_fd(),
                &null_stat,
                HandoffDescriptorSlot::PtyMaster(0)
            ),
            Err(CandidateBootstrapError::DescriptorRole(
                HandoffDescriptorSlot::PtyMaster(0)
            ))
        ));
        let read_only = rustix::fs::open(
            "/dev/ptmx",
            OFlags::RDONLY | OFlags::NOCTTY | OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .unwrap();
        let read_only_stat = fstat(&read_only).unwrap();
        assert!(matches!(
            require_pty_master(
                read_only.as_fd(),
                &read_only_stat,
                HandoffDescriptorSlot::PtyMaster(0)
            ),
            Err(CandidateBootstrapError::DescriptorRole(
                HandoffDescriptorSlot::PtyMaster(0)
            ))
        ));
    }

    #[test]
    fn roles_without_a_staging_consumer_fail_closed() {
        // Manifest validation already rejects these roles. The table is
        // extended directly so staging is proven not to rely on that alone.
        for slot in [
            HandoffDescriptorSlot::ImageState(0),
            HandoffDescriptorSlot::ClientProcess(0),
            HandoffDescriptorSlot::Continuation(0),
        ] {
            let mut idle = idle_owners();
            let mut bootstrap = bootstrap(std::mem::take(&mut idle.owners));
            let extra = body();
            bootstrap.manifest.descriptors.push(ManifestDescriptor {
                slot,
                inherited_fd: u32::try_from(extra.as_raw_fd()).unwrap(),
            });
            bootstrap.claims.insert_for_test(slot, extra);
            assert!(matches!(
                bootstrap.verify_descriptor_roles(),
                Err(CandidateBootstrapError::UnsupportedRole(role)) if role == slot
            ));
        }
    }

    #[test]
    fn two_pty_roles_on_one_master_are_an_alias() {
        let mut idle = idle_owners();
        let mut bootstrap = bootstrap(std::mem::take(&mut idle.owners));
        let master = pty_master();
        let duplicate = master.try_clone().unwrap();
        for (slot, owner) in [
            (HandoffDescriptorSlot::PtyMaster(0), master),
            (HandoffDescriptorSlot::PtyMaster(1), duplicate),
        ] {
            bootstrap.manifest.descriptors.push(ManifestDescriptor {
                slot,
                inherited_fd: u32::try_from(owner.as_raw_fd()).unwrap(),
            });
            bootstrap.claims.insert_for_test(slot, owner);
        }
        assert!(matches!(
            bootstrap.verify_descriptor_roles(),
            Err(CandidateBootstrapError::DescriptorAlias(
                HandoffDescriptorSlot::PtyMaster(1)
            ))
        ));
    }

    #[test]
    fn claim_table_must_cover_exactly_the_manifest_roles() {
        let mut idle = idle_owners();
        let mut bootstrap = bootstrap(std::mem::take(&mut idle.owners));
        drop(bootstrap.claims.take(HandoffDescriptorSlot::RollbackClient));
        assert!(matches!(
            bootstrap.verify_descriptor_roles(),
            Err(CandidateBootstrapError::Claim(
                DescriptorHandoffError::CandidateClaimMismatch
            ))
        ));
    }

    fn stage_for_test(
        bootstrap: CandidateBootstrap,
        directory: &Path,
    ) -> Result<CandidatePreRuntimeStage, CandidateStageFailure> {
        bootstrap.stage(
            &SharedImageBudget::new(1 << 20),
            &SharedKittyUploadBudget::new(1 << 20),
            &directory.join("daemon.sock"),
            &directory.join("image.sock"),
        )
    }

    #[test]
    fn staging_failure_releases_guard_and_recovers_a_rollback_only_owner_table() {
        let _serial = crate::handoff_coordinator::PROCESS_GUARD_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut idle = idle_owners();
        replace_role(&mut idle.owners, HandoffDescriptorSlot::Listener, body());
        let directory = idle.directory.clone();
        let mut acquired = bootstrap(std::mem::take(&mut idle.owners));
        acquired.process_guard = Some(HandoffProcessGuard::acquire_with(|| Ok(1)).unwrap());

        let failure = stage_for_test(acquired, &directory).unwrap_err();
        assert!(matches!(
            failure.error,
            CandidateBootstrapError::DescriptorRole(HandoffDescriptorSlot::Listener)
        ));
        // The failure released the guard rather than carrying it as an error.
        drop(HandoffProcessGuard::acquire_with(|| Ok(1)).unwrap());

        let recovered = failure.into_recovery();
        assert!(recovered.process_guard.is_none());
        assert_eq!(recovered.lifecycle.phase(), HandoffPhase::RollingBack);
        assert_eq!(recovered.claims.len(), recovered.manifest.descriptors.len());

        // A recovered table is rollback-only: staging it again fails before
        // any verification and still returns every owner.
        let failure = stage_for_test(recovered, &directory).unwrap_err();
        assert!(matches!(
            failure.error,
            CandidateBootstrapError::Coordinator(HandoffCoordinatorError::UnexpectedPhase)
        ));
        let recovered = failure.into_recovery();
        assert_eq!(recovered.claims.len(), recovered.manifest.descriptors.len());
        assert_eq!(recovered.lifecycle.phase(), HandoffPhase::RollingBack);

        // Even with a fresh guard put back, the phase machine refuses.
        let mut recovered = recovered;
        recovered.process_guard = Some(HandoffProcessGuard::acquire_with(|| Ok(1)).unwrap());
        let failure = stage_for_test(recovered, &directory).unwrap_err();
        assert!(matches!(
            failure.error,
            CandidateBootstrapError::Coordinator(HandoffCoordinatorError::UnexpectedPhase)
        ));
        assert_eq!(
            failure.into_recovery().lifecycle.phase(),
            HandoffPhase::RollingBack
        );
    }

    #[test]
    fn stage_failure_is_a_sendable_error() {
        fn assert_send_sync<T: Send + Sync + 'static>() {}
        assert_send_sync::<CandidateStageFailure>();
    }
}
