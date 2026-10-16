//! Canonical body-free handoff and rollback manifest schema v1.
//!
//! This internal daemon-generation ABI names authority and anonymous descriptor
//! slots only. Terminal, image, write-queue, and client-resume bodies remain in
//! separately sealed descriptors owned by later slices.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde::{Deserialize, Serialize};
use splinterm_core::{DojoId, LairId, SplintId};
use thiserror::Error;

use crate::{
    executable_snapshot::{
        HandoffExecutableSnapshots, MAX_EXECUTABLE_SNAPSHOT_BYTES, REQUIRED_EXECUTABLE_SEALS,
        SealedExecutablePair,
    },
    handoff_compatibility::{
        BuildIdentity, BuildVersion, CompatibilityError, ExecutablePairBinding,
        ExecutablePairPreflight, NegotiatedHandoff, RunningHandoffGeneration, negotiate_handoff,
    },
    handoff_descriptors::{HandoffDescriptorSlot, MAX_HANDOFF_DESCRIPTORS},
    handoff_preflight::SealedPreflightReports,
    package_provenance::ValidatedHandoffPackageProvenance,
};

pub const HANDOFF_MANIFEST_SCHEMA_V1: u16 = 1;
pub const MAX_HANDOFF_MANIFEST_BYTES: usize = 256 * 1024;
pub const MAX_MANIFEST_SPLINTS: usize = 256;
pub const MAX_MANIFEST_LAIRS: usize = 64;
pub const MAX_MANIFEST_DOJOS: usize = 64 * 64;
const MAX_INHERITED_FD: u32 = i32::MAX as u32;
const PACKAGED_OWNER_UID: u32 = 0;
const UNIX_FILE_TYPE_MASK: u32 = 0o170_000;
const UNIX_REGULAR_FILE: u32 = 0o100_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestKind {
    Forward,
    Rollback,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HandoffId([u8; 16]);

impl HandoffId {
    /// Creates one nonzero operation identity.
    ///
    /// # Errors
    /// Rejects the reserved all-zero value.
    pub fn new(bytes: [u8; 16]) -> Result<Self, ManifestError> {
        if bytes == [0; 16] {
            return Err(ManifestError::InvalidIdentity);
        }
        Ok(Self(bytes))
    }

    #[must_use]
    pub const fn bytes(self) -> [u8; 16] {
        self.0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffManifestV1 {
    pub schema: u16,
    pub kind: ManifestKind,
    pub handoff_id: HandoffId,
    pub source_generation: GenerationEvidence,
    pub target_generation: GenerationEvidence,
    pub topology_revision: u64,
    pub durable_state_generation: u64,
    pub private_protocol: u16,
    pub handoff_protocol: u16,
    pub terminal_checkpoint_schema: u16,
    pub descriptor_manifest_schema: u16,
    pub executable_pairs: ExecutablePairSetEvidence,
    pub counterpart_manifest_slot: HandoffDescriptorSlot,
    pub splints: Vec<ManifestSplint>,
    pub descriptors: Vec<ManifestDescriptor>,
}

impl HandoffManifestV1 {
    /// Validates decoded structure and rebinds all executable and version authority.
    ///
    /// # Errors
    /// Rejects structural invalidity, executable/report substitution, capability
    /// gaps, or any generation/version value other than the daemon-owned
    /// three-generation negotiation result.
    ///
    /// Manifest executable evidence is not package provenance. The opaque
    /// provenance argument proves that both package records were already
    /// validated against these exact sealed generations.
    pub fn validate_authority_against(
        &self,
        running: RunningHandoffGeneration,
        snapshots: &HandoffExecutableSnapshots,
        reports: &SealedPreflightReports,
        provenance: &ValidatedHandoffPackageProvenance,
    ) -> Result<(), ManifestError> {
        validate_manifest(self)?;
        let negotiated = negotiate_handoff(running, reports.forward, reports.rollback, snapshots)
            .map_err(|error| match error {
            CompatibilityError::ForwardSnapshotMismatch
            | CompatibilityError::RollbackSnapshotMismatch => ManifestError::InvalidExecutables,
            CompatibilityError::InvalidRange
            | CompatibilityError::InvalidBuildIdentity
            | CompatibilityError::RollbackGenerationMismatch
            | CompatibilityError::Incompatible(_) => ManifestError::CapabilityMismatch,
        })?;
        validate_negotiated_contract(self, negotiated)?;
        provenance
            .validate_against(snapshots, reports.forward, reports.rollback)
            .map_err(|_| ManifestError::InvalidExecutables)?;
        self.executable_pairs.validate_against(snapshots, reports)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationEvidence {
    pub build_identity: [u8; 32],
    pub build_version: ManifestBuildVersion,
}

impl GenerationEvidence {
    #[must_use]
    pub const fn from_preflight(preflight: ExecutablePairPreflight) -> Self {
        Self {
            build_identity: preflight.pair_build_identity.as_bytes(),
            build_version: ManifestBuildVersion::from_build_version(preflight.pair_build_version),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestBuildVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl ManifestBuildVersion {
    #[must_use]
    pub const fn from_build_version(version: BuildVersion) -> Self {
        Self {
            major: version.major,
            minor: version.minor,
            patch: version.patch,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutablePairSetEvidence {
    pub forward: ManifestExecutablePair,
    pub rollback: ManifestExecutablePair,
}

impl ExecutablePairSetEvidence {
    /// Captures manifest evidence from the exact authenticated sealed pairs.
    ///
    /// # Errors
    /// Rejects either preflight report unless it binds the corresponding sealed
    /// daemon/client pair.
    pub fn from_verified(
        snapshots: &HandoffExecutableSnapshots,
        reports: &SealedPreflightReports,
    ) -> Result<Self, ManifestError> {
        Ok(Self {
            forward: ManifestExecutablePair::from_verified(snapshots.forward(), reports.forward)?,
            rollback: ManifestExecutablePair::from_verified(
                snapshots.rollback(),
                reports.rollback,
            )?,
        })
    }

    /// Rebinds decoded evidence to parent-owned snapshots and authenticated reports.
    ///
    /// Structural manifest validation alone cannot prove that decoded identities
    /// name the descriptors still owned by the coordinator. Call this before the
    /// evidence is accepted as live handoff authority.
    ///
    /// # Errors
    /// Rejects snapshot/report mismatch or any decoded evidence substitution.
    pub fn validate_against(
        &self,
        snapshots: &HandoffExecutableSnapshots,
        reports: &SealedPreflightReports,
    ) -> Result<(), ManifestError> {
        let expected = Self::from_verified(snapshots, reports)?;
        if *self != expected {
            return Err(ManifestError::InvalidExecutables);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestExecutablePair {
    pub daemon: ManifestExecutableImage,
    pub client: ManifestExecutableImage,
    pub generation: GenerationEvidence,
}

impl ManifestExecutablePair {
    /// Captures one pair only when preflight names its exact sealed descriptors.
    ///
    /// # Errors
    /// Rejects a preflight report bound to any other daemon/client snapshots.
    pub fn from_verified(
        pair: &SealedExecutablePair,
        preflight: ExecutablePairPreflight,
    ) -> Result<Self, ManifestError> {
        if preflight.snapshots != ExecutablePairBinding::from_snapshots(pair) {
            return Err(ManifestError::InvalidExecutables);
        }
        let generation = GenerationEvidence::from_preflight(preflight);
        validate_generation(generation).map_err(|_| ManifestError::InvalidExecutables)?;
        Ok(Self {
            daemon: ManifestExecutableImage::from_snapshot(pair.daemon()),
            client: ManifestExecutableImage::from_snapshot(pair.client()),
            generation,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestExecutableImage {
    pub source_device: u64,
    pub source_inode: u64,
    pub source_owner_uid: u32,
    pub source_mode: u32,
    pub source_size: u64,
    pub source_sha256: [u8; 32],
    pub snapshot_device: u64,
    pub snapshot_inode: u64,
    pub snapshot_size: u64,
    pub snapshot_sha256: [u8; 32],
    pub snapshot_seals: u32,
}

impl ManifestExecutableImage {
    fn from_snapshot(snapshot: &crate::executable_snapshot::SealedExecutableSnapshot) -> Self {
        let source = snapshot.source();
        let sealed = snapshot.snapshot();
        Self {
            source_device: source.device,
            source_inode: source.inode,
            source_owner_uid: source.owner_uid,
            source_mode: source.mode,
            source_size: source.size,
            source_sha256: source.sha256,
            snapshot_device: sealed.device,
            snapshot_inode: sealed.inode,
            snapshot_size: sealed.size,
            snapshot_sha256: sealed.sha256,
            snapshot_seals: sealed.seals.bits(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestDescriptor {
    pub slot: HandoffDescriptorSlot,
    pub inherited_fd: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestSplint {
    pub lair_id: LairId,
    pub dojo_id: DojoId,
    pub splint_id: SplintId,
    pub incarnation: u64,
    pub pty: ManifestPtyIdentity,
    pub size: ManifestPtySize,
    pub foreground_process_group: Option<u32>,
    pub terminal_revision: u64,
    pub history_generation: u64,
    pub exit: ExitDisposition,
    pub terminal_checkpoint: BodyDisposition,
    pub image_state: BodyDisposition,
    pub controller: ControllerDisposition,
    pub subscriptions: SubscriptionDisposition,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestPtyIdentity {
    pub child_pid: u32,
    pub process_group: u32,
    pub session_id: u32,
    pub master_slot: u16,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestPtySize {
    pub columns: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitDisposition {
    Running,
    Exited,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "disposition", content = "slot", rename_all = "snake_case")]
pub enum BodyDisposition {
    Descriptor(u16),
    NotPresent,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerDisposition {
    Released,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionDisposition {
    Dropped,
}

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("handoff manifest exceeds {MAX_HANDOFF_MANIFEST_BYTES} bytes")]
    TooLarge,
    #[error("handoff manifest encoding is malformed: {0}")]
    Encoding(#[from] serde_json::Error),
    #[error("handoff manifest encoding is not canonical")]
    NonCanonical,
    #[error("handoff manifest schema or negotiated versions are invalid")]
    InvalidSchema,
    #[error("handoff manifest identity or generation is invalid")]
    InvalidIdentity,
    #[error("handoff manifest collection bounds are invalid")]
    InvalidBounds,
    #[error("handoff manifest topology is invalid or duplicated")]
    InvalidTopology,
    #[error("handoff manifest descriptor set is invalid or incomplete")]
    InvalidDescriptors,
    #[error("handoff manifest PTY identity, dimensions, or runtime state is invalid")]
    InvalidRuntime,
    #[error("handoff manifest negotiated versions do not match authenticated capabilities")]
    CapabilityMismatch,
    #[error("handoff manifest executable evidence is invalid")]
    InvalidExecutables,
    #[error("forward and rollback manifests do not form one exact operation pair")]
    PairMismatch,
}

/// Encodes one validated manifest into its unique canonical JSON byte form.
///
/// Vector order is canonicalized before validation. This is an internal sealed
/// runtime ABI; it is not the public automation JSON schema.
///
/// # Errors
/// Rejects invalid fields or an encoded aggregate larger than the hard bound.
pub fn encode_manifest(manifest: &HandoffManifestV1) -> Result<Vec<u8>, ManifestError> {
    let mut canonical = manifest.clone();
    canonical.splints.sort_by_key(splint_sort_key);
    canonical.descriptors.sort_by_key(|entry| entry.slot);
    validate_manifest(&canonical)?;
    let bytes = serde_json::to_vec(&canonical)?;
    if bytes.len() > MAX_HANDOFF_MANIFEST_BYTES {
        return Err(ManifestError::TooLarge);
    }
    Ok(bytes)
}

/// Decodes only the unique canonical representation and consumes every byte.
///
/// # Errors
/// Rejects oversized, malformed, noncanonical, unknown, or invalid content.
pub fn decode_manifest(bytes: &[u8]) -> Result<HandoffManifestV1, ManifestError> {
    if bytes.len() > MAX_HANDOFF_MANIFEST_BYTES {
        return Err(ManifestError::TooLarge);
    }
    let manifest: HandoffManifestV1 = serde_json::from_slice(bytes)?;
    if encode_manifest(&manifest)? != bytes {
        return Err(ManifestError::NonCanonical);
    }
    Ok(manifest)
}

/// Validates one complete body-free manifest without publishing partial state.
///
/// # Errors
/// Rejects any invalid schema, identity, topology, descriptor, runtime, or
/// executable evidence.
pub fn validate_manifest(manifest: &HandoffManifestV1) -> Result<(), ManifestError> {
    if manifest.schema != HANDOFF_MANIFEST_SCHEMA_V1
        || manifest.handoff_id.bytes() == [0; 16]
        || manifest.private_protocol == 0
        || manifest.handoff_protocol == 0
        || manifest.terminal_checkpoint_schema == 0
        || manifest.descriptor_manifest_schema != HANDOFF_MANIFEST_SCHEMA_V1
    {
        return Err(ManifestError::InvalidSchema);
    }
    validate_generation(manifest.source_generation)?;
    validate_generation(manifest.target_generation)?;
    if manifest.source_generation == manifest.target_generation {
        return Err(ManifestError::InvalidIdentity);
    }
    if manifest.splints.len() > MAX_MANIFEST_SPLINTS
        || manifest.descriptors.len() > MAX_HANDOFF_DESCRIPTORS
    {
        return Err(ManifestError::InvalidBounds);
    }
    validate_executable_pairs(manifest)?;
    validate_descriptors(manifest)?;
    validate_splints(manifest)?;
    match (manifest.kind, manifest.counterpart_manifest_slot) {
        (ManifestKind::Forward, HandoffDescriptorSlot::RollbackManifest)
        | (ManifestKind::Rollback, HandoffDescriptorSlot::CheckpointManifest) => Ok(()),
        _ => Err(ManifestError::InvalidDescriptors),
    }
}

/// Validates that forward and rollback manifests describe one exact reversible
/// pre-read operation.
///
/// # Errors
/// Rejects manifests that are individually invalid or differ in shared state,
/// descriptor assignment, executable evidence, or reversed generation binding.
pub fn validate_manifest_pair(
    forward: &HandoffManifestV1,
    rollback: &HandoffManifestV1,
) -> Result<(), ManifestError> {
    validate_manifest(forward)?;
    validate_manifest(rollback)?;
    let mut forward_splints = forward.splints.clone();
    let mut rollback_splints = rollback.splints.clone();
    forward_splints.sort_by_key(splint_sort_key);
    rollback_splints.sort_by_key(splint_sort_key);
    let mut forward_descriptors = forward.descriptors.clone();
    let mut rollback_descriptors = rollback.descriptors.clone();
    forward_descriptors.sort_by_key(|entry| entry.slot);
    rollback_descriptors.sort_by_key(|entry| entry.slot);
    if forward.kind != ManifestKind::Forward
        || rollback.kind != ManifestKind::Rollback
        || forward.handoff_id != rollback.handoff_id
        || forward.topology_revision != rollback.topology_revision
        || forward.durable_state_generation != rollback.durable_state_generation
        || forward.private_protocol != rollback.private_protocol
        || forward.handoff_protocol != rollback.handoff_protocol
        || forward.terminal_checkpoint_schema != rollback.terminal_checkpoint_schema
        || forward.descriptor_manifest_schema != rollback.descriptor_manifest_schema
        || forward.source_generation != rollback.target_generation
        || forward.target_generation != rollback.source_generation
        || forward.executable_pairs != rollback.executable_pairs
        || forward_splints != rollback_splints
        || forward_descriptors != rollback_descriptors
    {
        return Err(ManifestError::PairMismatch);
    }
    Ok(())
}

fn validate_negotiated_contract(
    manifest: &HandoffManifestV1,
    negotiated: NegotiatedHandoff,
) -> Result<(), ManifestError> {
    let forward = GenerationEvidence {
        build_identity: negotiated.forward_build_identity.as_bytes(),
        build_version: ManifestBuildVersion::from_build_version(negotiated.forward_build_version),
    };
    let rollback = GenerationEvidence {
        build_identity: negotiated.rollback_build_identity.as_bytes(),
        build_version: ManifestBuildVersion::from_build_version(negotiated.rollback_build_version),
    };
    let generations_match = match manifest.kind {
        ManifestKind::Forward => {
            manifest.source_generation == rollback && manifest.target_generation == forward
        }
        ManifestKind::Rollback => {
            manifest.source_generation == forward && manifest.target_generation == rollback
        }
    };
    if !generations_match
        || manifest.private_protocol != negotiated.private_protocol
        || manifest.handoff_protocol != negotiated.handoff_protocol
        || manifest.terminal_checkpoint_schema != negotiated.terminal_checkpoint
        || manifest.descriptor_manifest_schema != negotiated.descriptor_manifest
    {
        return Err(ManifestError::CapabilityMismatch);
    }
    Ok(())
}

fn validate_generation(generation: GenerationEvidence) -> Result<(), ManifestError> {
    BuildIdentity::new(generation.build_identity).map_err(|_| ManifestError::InvalidIdentity)?;
    if generation.build_version.major == 0
        && generation.build_version.minor == 0
        && generation.build_version.patch == 0
    {
        return Err(ManifestError::InvalidIdentity);
    }
    Ok(())
}

fn validate_executable_pairs(manifest: &HandoffManifestV1) -> Result<(), ManifestError> {
    let pairs = &manifest.executable_pairs;
    validate_executable_pair(&pairs.forward)?;
    validate_executable_pair(&pairs.rollback)?;
    let generations_match = match manifest.kind {
        ManifestKind::Forward => {
            pairs.forward.generation == manifest.target_generation
                && pairs.rollback.generation == manifest.source_generation
        }
        ManifestKind::Rollback => {
            pairs.forward.generation == manifest.source_generation
                && pairs.rollback.generation == manifest.target_generation
        }
    };
    if !generations_match {
        return Err(ManifestError::InvalidExecutables);
    }
    Ok(())
}

fn validate_executable_pair(pair: &ManifestExecutablePair) -> Result<(), ManifestError> {
    validate_generation(pair.generation)?;
    validate_executable_image(&pair.daemon)?;
    validate_executable_image(&pair.client)?;
    if pair.daemon.snapshot_device == pair.client.snapshot_device
        && pair.daemon.snapshot_inode == pair.client.snapshot_inode
    {
        return Err(ManifestError::InvalidExecutables);
    }
    Ok(())
}

fn validate_executable_image(image: &ManifestExecutableImage) -> Result<(), ManifestError> {
    let required_seals = REQUIRED_EXECUTABLE_SEALS.bits();
    let regular_file = image.source_mode & UNIX_FILE_TYPE_MASK == UNIX_REGULAR_FILE;
    if image.source_device == 0
        || image.source_inode == 0
        || image.snapshot_device == 0
        || image.snapshot_inode == 0
        || image.source_owner_uid != PACKAGED_OWNER_UID
        || !regular_file
        || image.source_mode & 0o111 == 0
        || image.source_mode & 0o022 != 0
        || image.source_size == 0
        || image.source_size > MAX_EXECUTABLE_SNAPSHOT_BYTES
        || image.source_size != image.snapshot_size
        || (image.source_device == image.snapshot_device
            && image.source_inode == image.snapshot_inode)
        || image.source_sha256 == [0; 32]
        || image.source_sha256 != image.snapshot_sha256
        || image.snapshot_seals & required_seals != required_seals
    {
        return Err(ManifestError::InvalidExecutables);
    }
    Ok(())
}

fn validate_descriptors(manifest: &HandoffManifestV1) -> Result<(), ManifestError> {
    let mut slots = BTreeSet::new();
    let mut descriptors = HashSet::new();
    for entry in &manifest.descriptors {
        if entry.inherited_fd < 3
            || entry.inherited_fd > MAX_INHERITED_FD
            || !slots.insert(entry.slot)
            || !descriptors.insert(entry.inherited_fd)
        {
            return Err(ManifestError::InvalidDescriptors);
        }
    }
    let mut expected = BTreeSet::from([
        HandoffDescriptorSlot::Listener,
        HandoffDescriptorSlot::ImageListener,
        HandoffDescriptorSlot::CheckpointManifest,
        HandoffDescriptorSlot::RollbackManifest,
        HandoffDescriptorSlot::ForwardClient,
        HandoffDescriptorSlot::RollbackDaemon,
        HandoffDescriptorSlot::RollbackClient,
    ]);
    for splint in &manifest.splints {
        expected.insert(HandoffDescriptorSlot::PtyMaster(splint.pty.master_slot));
        if let BodyDisposition::Descriptor(slot) = splint.terminal_checkpoint {
            expected.insert(HandoffDescriptorSlot::TerminalCheckpoint(slot));
        }
        if let BodyDisposition::Descriptor(slot) = splint.image_state {
            expected.insert(HandoffDescriptorSlot::ImageState(slot));
        }
    }
    if slots != expected {
        return Err(ManifestError::InvalidDescriptors);
    }
    Ok(())
}

fn validate_splints(manifest: &HandoffManifestV1) -> Result<(), ManifestError> {
    let descriptor_slots = manifest
        .descriptors
        .iter()
        .map(|entry| entry.slot)
        .collect::<BTreeSet<_>>();
    let mut splint_ids = HashSet::new();
    let mut dojo_lairs = BTreeMap::new();
    let mut lairs = HashSet::new();
    let mut dojos = HashSet::new();
    let mut pty_slots = BTreeSet::new();
    let mut checkpoint_slots = BTreeSet::new();
    let mut image_slots = BTreeSet::new();

    for splint in &manifest.splints {
        lairs.insert(splint.lair_id);
        dojos.insert(splint.dojo_id);
        if !splint_ids.insert(splint.splint_id)
            || dojo_lairs
                .insert(splint.dojo_id, splint.lair_id)
                .is_some_and(|lair| lair != splint.lair_id)
        {
            return Err(ManifestError::InvalidTopology);
        }
        let pty_slot = HandoffDescriptorSlot::PtyMaster(splint.pty.master_slot);
        if !pty_slots.insert(pty_slot) || !descriptor_slots.contains(&pty_slot) {
            return Err(ManifestError::InvalidDescriptors);
        }
        let BodyDisposition::Descriptor(checkpoint_slot) = splint.terminal_checkpoint else {
            return Err(ManifestError::InvalidRuntime);
        };
        let checkpoint_slot = HandoffDescriptorSlot::TerminalCheckpoint(checkpoint_slot);
        if !checkpoint_slots.insert(checkpoint_slot) || !descriptor_slots.contains(&checkpoint_slot)
        {
            return Err(ManifestError::InvalidDescriptors);
        }
        match splint.image_state {
            BodyDisposition::NotPresent => {}
            BodyDisposition::Descriptor(slot) => {
                let slot = HandoffDescriptorSlot::ImageState(slot);
                if !image_slots.insert(slot) || !descriptor_slots.contains(&slot) {
                    return Err(ManifestError::InvalidDescriptors);
                }
            }
            BodyDisposition::Unsupported => return Err(ManifestError::InvalidRuntime),
        }
        if splint.incarnation == 0
            || splint.pty.child_pid == 0
            || splint.pty.child_pid != splint.pty.process_group
            || splint.pty.child_pid != splint.pty.session_id
            || splint.size.columns == 0
            || splint.size.rows == 0
            || splint.history_generation == 0
            || splint.exit != ExitDisposition::Running
            || splint
                .foreground_process_group
                .is_some_and(|group| group == 0)
        {
            return Err(ManifestError::InvalidRuntime);
        }
    }
    if lairs.len() > MAX_MANIFEST_LAIRS || dojos.len() > MAX_MANIFEST_DOJOS {
        return Err(ManifestError::InvalidBounds);
    }
    Ok(())
}

fn splint_sort_key(splint: &ManifestSplint) -> (String, String, String) {
    (
        splint.lair_id.to_string(),
        splint.dojo_id.to_string(),
        splint.splint_id.to_string(),
    )
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::{
        executable_snapshot::{
            ExecutableSnapshotPolicy, ExecutableSourcePair, HandoffExecutableSnapshots,
            RetainedRollbackExecutables,
        },
        handoff_compatibility::{HandoffCapabilities, VersionRange},
    };

    use super::*;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct SnapshotFixture {
        directory: PathBuf,
        snapshots: HandoffExecutableSnapshots,
    }

    impl SnapshotFixture {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let directory = std::env::temp_dir().join(format!(
                "splinterd-handoff-manifest-{}-{unique}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&directory).unwrap();
            let forward = executable_pair(&directory, "forward");
            let rollback = executable_pair(&directory, "rollback");
            let policy = ExecutableSnapshotPolicy {
                expected_owner_uid: rustix::process::geteuid().as_raw(),
            };
            let rollback =
                RetainedRollbackExecutables::capture_declared_for_test(&rollback, policy).unwrap();
            let snapshots =
                HandoffExecutableSnapshots::materialize(&forward, rollback, policy).unwrap();
            Self {
                directory,
                snapshots,
            }
        }
    }

    impl Drop for SnapshotFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn executable_pair(root: &Path, generation: &str) -> ExecutableSourcePair {
        let directory = root.join(generation);
        fs::create_dir(&directory).unwrap();
        let daemon = directory.join("splinterd");
        let client = directory.join("splinterm");
        for (path, bytes) in [
            (&daemon, b"daemon".as_slice()),
            (&client, b"client".as_slice()),
        ] {
            fs::write(path, bytes).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        ExecutableSourcePair::new(daemon, client).unwrap()
    }

    fn range() -> VersionRange {
        version_range(1, 1)
    }

    fn version_range(minimum: u16, maximum: u16) -> VersionRange {
        VersionRange::new(minimum, maximum).unwrap()
    }

    fn preflight(pair: &SealedExecutablePair, byte: u8) -> ExecutablePairPreflight {
        ExecutablePairPreflight {
            pair_build_identity: BuildIdentity::new([byte; 32]).unwrap(),
            pair_build_version: BuildVersion::new(0, 2, u16::from(byte)),
            capabilities: HandoffCapabilities {
                private_protocol: range(),
                handoff_protocol: range(),
                terminal_checkpoint: range(),
                descriptor_manifest: range(),
            },
            snapshots: ExecutablePairBinding::from_snapshots(pair),
        }
    }

    fn generation(byte: u8, patch: u16) -> GenerationEvidence {
        GenerationEvidence {
            build_identity: [byte; 32],
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
            source_mode: UNIX_REGULAR_FILE | 0o755,
            source_size: 1024,
            source_sha256: [byte; 32],
            snapshot_device: u64::from(byte) + 20,
            snapshot_inode: u64::from(byte) + 30,
            snapshot_size: 1024,
            snapshot_sha256: [byte; 32],
            snapshot_seals: REQUIRED_EXECUTABLE_SEALS.bits(),
        }
    }

    fn manifest(kind: ManifestKind) -> HandoffManifestV1 {
        let source = generation(1, 1);
        let target = generation(2, 2);
        let splint = ManifestSplint {
            lair_id: LairId::new(),
            dojo_id: DojoId::new(),
            splint_id: SplintId::new(),
            incarnation: 1,
            pty: ManifestPtyIdentity {
                child_pid: 42,
                process_group: 42,
                session_id: 42,
                master_slot: 0,
            },
            size: ManifestPtySize {
                columns: 80,
                rows: 24,
                pixel_width: 800,
                pixel_height: 480,
            },
            foreground_process_group: Some(42),
            terminal_revision: 7,
            history_generation: 3,
            exit: ExitDisposition::Running,
            terminal_checkpoint: BodyDisposition::Descriptor(0),
            image_state: BodyDisposition::NotPresent,
            controller: ControllerDisposition::Released,
            subscriptions: SubscriptionDisposition::Dropped,
        };
        let descriptors = [
            HandoffDescriptorSlot::Listener,
            HandoffDescriptorSlot::ImageListener,
            HandoffDescriptorSlot::CheckpointManifest,
            HandoffDescriptorSlot::RollbackManifest,
            HandoffDescriptorSlot::ForwardClient,
            HandoffDescriptorSlot::RollbackDaemon,
            HandoffDescriptorSlot::RollbackClient,
            HandoffDescriptorSlot::PtyMaster(0),
            HandoffDescriptorSlot::TerminalCheckpoint(0),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, slot)| ManifestDescriptor {
            slot,
            inherited_fd: u32::try_from(index).unwrap() + 20,
        })
        .collect();
        HandoffManifestV1 {
            schema: 1,
            kind,
            handoff_id: HandoffId::new([9; 16]).unwrap(),
            source_generation: source,
            target_generation: target,
            topology_revision: 4,
            durable_state_generation: 4,
            private_protocol: 37,
            handoff_protocol: 1,
            terminal_checkpoint_schema: 1,
            descriptor_manifest_schema: 1,
            executable_pairs: ExecutablePairSetEvidence {
                forward: ManifestExecutablePair {
                    daemon: image(2),
                    client: image(3),
                    generation: target,
                },
                rollback: ManifestExecutablePair {
                    daemon: image(4),
                    client: image(5),
                    generation: source,
                },
            },
            counterpart_manifest_slot: match kind {
                ManifestKind::Forward => HandoffDescriptorSlot::RollbackManifest,
                ManifestKind::Rollback => HandoffDescriptorSlot::CheckpointManifest,
            },
            splints: vec![splint],
            descriptors,
        }
    }

    fn pair() -> (HandoffManifestV1, HandoffManifestV1) {
        let forward = manifest(ManifestKind::Forward);
        let mut rollback = forward.clone();
        rollback.kind = ManifestKind::Rollback;
        rollback.counterpart_manifest_slot = HandoffDescriptorSlot::CheckpointManifest;
        std::mem::swap(
            &mut rollback.source_generation,
            &mut rollback.target_generation,
        );
        (forward, rollback)
    }

    fn with_splint_count(count: usize) -> HandoffManifestV1 {
        let mut manifest = manifest(ManifestKind::Forward);
        let template = manifest.splints[0].clone();
        manifest.splints.clear();
        manifest.descriptors.retain(|entry| {
            !matches!(
                entry.slot,
                HandoffDescriptorSlot::PtyMaster(_) | HandoffDescriptorSlot::TerminalCheckpoint(_)
            )
        });
        for index in 0..count {
            let slot = u16::try_from(index).unwrap();
            let mut splint = template.clone();
            splint.splint_id = SplintId::new();
            splint.incarnation = u64::try_from(index).unwrap() + 1;
            splint.pty.child_pid = u32::try_from(index).unwrap() + 100;
            splint.pty.process_group = splint.pty.child_pid;
            splint.pty.session_id = splint.pty.child_pid;
            splint.pty.master_slot = slot;
            splint.terminal_checkpoint = BodyDisposition::Descriptor(slot);
            manifest.splints.push(splint);
            manifest.descriptors.push(ManifestDescriptor {
                slot: HandoffDescriptorSlot::PtyMaster(slot),
                inherited_fd: u32::try_from(index * 2).unwrap() + 1000,
            });
            manifest.descriptors.push(ManifestDescriptor {
                slot: HandoffDescriptorSlot::TerminalCheckpoint(slot),
                inherited_fd: u32::try_from(index * 2).unwrap() + 1001,
            });
        }
        manifest
    }

    #[test]
    fn canonical_round_trip_and_pair_validation() {
        let (mut forward, rollback) = pair();
        forward.descriptors.reverse();
        let bytes = encode_manifest(&forward).unwrap();
        let decoded = decode_manifest(&bytes).unwrap();
        validate_manifest_pair(&decoded, &rollback).unwrap();
        assert_eq!(encode_manifest(&decoded).unwrap(), bytes);
    }

    #[test]
    fn accepts_idle_daemon_and_fresh_zero_revisions() {
        let mut idle = manifest(ManifestKind::Forward);
        idle.topology_revision = 0;
        idle.durable_state_generation = 0;
        idle.splints.clear();
        idle.descriptors.retain(|entry| {
            !matches!(
                entry.slot,
                HandoffDescriptorSlot::PtyMaster(_) | HandoffDescriptorSlot::TerminalCheckpoint(_)
            )
        });
        validate_manifest(&idle).unwrap();
        assert!(decode_manifest(&encode_manifest(&idle).unwrap()).is_ok());

        let mut fresh_terminal = manifest(ManifestKind::Forward);
        fresh_terminal.splints[0].terminal_revision = 0;
        validate_manifest(&fresh_terminal).unwrap();
    }

    #[test]
    fn executable_evidence_binds_exact_snapshots_and_preflight_reports() {
        let fixture = SnapshotFixture::new();
        let reports = SealedPreflightReports {
            forward: preflight(fixture.snapshots.forward(), 1),
            rollback: preflight(fixture.snapshots.rollback(), 2),
        };
        let evidence =
            ExecutablePairSetEvidence::from_verified(&fixture.snapshots, &reports).unwrap();
        evidence
            .validate_against(&fixture.snapshots, &reports)
            .unwrap();

        let mut substituted_report = reports;
        substituted_report.forward.snapshots =
            ExecutablePairBinding::from_snapshots(fixture.snapshots.rollback());
        assert!(matches!(
            ExecutablePairSetEvidence::from_verified(&fixture.snapshots, &substituted_report),
            Err(ManifestError::InvalidExecutables)
        ));

        let mut substituted_evidence = evidence;
        substituted_evidence.forward.daemon.snapshot_sha256[0] ^= 1;
        assert!(matches!(
            substituted_evidence.validate_against(&fixture.snapshots, &reports),
            Err(ManifestError::InvalidExecutables)
        ));
    }

    #[test]
    fn authenticated_negotiation_binds_every_generation_and_version_without_downgrade() {
        let fixture = SnapshotFixture::new();
        let reports = SealedPreflightReports {
            forward: preflight(fixture.snapshots.forward(), 2),
            rollback: preflight(fixture.snapshots.rollback(), 1),
        };
        let running = RunningHandoffGeneration {
            build_identity: reports.rollback.pair_build_identity,
            build_version: reports.rollback.pair_build_version,
            capabilities: reports.rollback.capabilities,
        };
        let negotiated = negotiate_handoff(
            running,
            reports.forward,
            reports.rollback,
            &fixture.snapshots,
        )
        .unwrap();
        let provenance = ValidatedHandoffPackageProvenance::from_verified_for_test(
            &fixture.snapshots,
            reports.forward,
            reports.rollback,
        );
        let mut baseline = manifest(ManifestKind::Forward);
        baseline.private_protocol = 1;
        validate_negotiated_contract(&baseline, negotiated).unwrap();

        for mutate in [
            (|manifest: &mut HandoffManifestV1| manifest.private_protocol = 2)
                as fn(&mut HandoffManifestV1),
            |manifest| manifest.handoff_protocol = 2,
            |manifest| manifest.terminal_checkpoint_schema = 2,
            |manifest| manifest.descriptor_manifest_schema = 2,
        ] {
            let mut invalid = baseline.clone();
            mutate(&mut invalid);
            assert!(matches!(
                validate_negotiated_contract(&invalid, negotiated),
                Err(ManifestError::CapabilityMismatch)
            ));
        }

        let mut incompatible = reports;
        incompatible.forward.capabilities.private_protocol = version_range(2, 2);
        assert!(matches!(
            baseline.validate_authority_against(
                running,
                &fixture.snapshots,
                &incompatible,
                &provenance,
            ),
            Err(ManifestError::CapabilityMismatch)
        ));

        let mut downgrade = reports;
        downgrade.forward.capabilities.private_protocol = version_range(1, 2);
        downgrade.rollback.capabilities.private_protocol = version_range(1, 2);
        let mut broader_running = running;
        broader_running.capabilities.private_protocol = version_range(1, 2);
        assert!(matches!(
            baseline.validate_authority_against(
                broader_running,
                &fixture.snapshots,
                &downgrade,
                &provenance,
            ),
            Err(ManifestError::CapabilityMismatch)
        ));
    }

    #[test]
    fn rejects_noncanonical_truncated_trailing_unknown_and_oversized_input() {
        let bytes = encode_manifest(&manifest(ManifestKind::Forward)).unwrap();
        let mut whitespace = bytes.clone();
        whitespace.push(b'\n');
        assert!(matches!(
            decode_manifest(&whitespace),
            Err(ManifestError::NonCanonical)
        ));
        assert!(decode_manifest(&bytes[..bytes.len() - 1]).is_err());
        let mut trailing = bytes.clone();
        trailing.push(b'x');
        assert!(decode_manifest(&trailing).is_err());
        let unknown = String::from_utf8(bytes.clone())
            .unwrap()
            .replacen('{', "{\"unknown\":1,", 1);
        assert!(decode_manifest(unknown.as_bytes()).is_err());
        assert!(matches!(
            decode_manifest(&vec![b'x'; MAX_HANDOFF_MANIFEST_BYTES + 1]),
            Err(ManifestError::TooLarge)
        ));
    }

    #[test]
    fn exact_collection_bound_is_encodable_and_excess_fails_before_encoding() {
        let maximum = with_splint_count(MAX_MANIFEST_SPLINTS);
        let bytes = encode_manifest(&maximum).unwrap();
        assert!(bytes.len() < MAX_HANDOFF_MANIFEST_BYTES);
        assert_eq!(
            decode_manifest(&bytes).unwrap().splints.len(),
            MAX_MANIFEST_SPLINTS
        );

        let excessive = with_splint_count(MAX_MANIFEST_SPLINTS + 1);
        assert!(matches!(
            encode_manifest(&excessive),
            Err(ManifestError::InvalidBounds)
        ));

        let mut excessive_descriptors = manifest(ManifestKind::Forward);
        for index in 0..=MAX_HANDOFF_DESCRIPTORS {
            excessive_descriptors.descriptors.push(ManifestDescriptor {
                slot: HandoffDescriptorSlot::PtyMaster(u16::try_from(index).unwrap()),
                inherited_fd: u32::try_from(index).unwrap() + 10_000,
            });
        }
        assert!(matches!(
            validate_manifest(&excessive_descriptors),
            Err(ManifestError::InvalidBounds)
        ));

        let mut excessive_lairs = with_splint_count(MAX_MANIFEST_LAIRS + 1);
        for splint in &mut excessive_lairs.splints {
            splint.lair_id = LairId::new();
            splint.dojo_id = DojoId::new();
        }
        assert!(matches!(
            validate_manifest(&excessive_lairs),
            Err(ManifestError::InvalidBounds)
        ));
    }

    #[test]
    fn every_truncation_and_single_byte_mutation_fails_or_is_separately_canonical() {
        let bytes = encode_manifest(&manifest(ManifestKind::Forward)).unwrap();
        for length in 0..bytes.len() {
            assert!(
                decode_manifest(&bytes[..length]).is_err(),
                "length {length}"
            );
        }
        for index in 0..bytes.len() {
            let mut mutated = bytes.clone();
            mutated[index] ^= 1;
            if let Ok(decoded) = decode_manifest(&mutated) {
                assert_eq!(encode_manifest(&decoded).unwrap(), mutated, "index {index}");
            }
        }
        let unknown_kind =
            String::from_utf8(bytes)
                .unwrap()
                .replacen("\"forward\"", "\"sideways\"", 1);
        assert!(decode_manifest(unknown_kind.as_bytes()).is_err());
    }

    #[test]
    fn rejects_schema_identity_and_generation_mutations() {
        let baseline = manifest(ManifestKind::Forward);
        for mutate in [
            (|manifest: &mut HandoffManifestV1| manifest.schema = 2) as fn(&mut HandoffManifestV1),
            |manifest| manifest.handoff_id = HandoffId([0; 16]),
            |manifest| manifest.private_protocol = 0,
            |manifest| manifest.handoff_protocol = 0,
            |manifest| manifest.terminal_checkpoint_schema = 0,
            |manifest| manifest.descriptor_manifest_schema = 2,
            |manifest| manifest.source_generation.build_identity = [0; 32],
            |manifest| manifest.target_generation.build_identity = [0; 32],
            |manifest| {
                manifest.source_generation.build_version = ManifestBuildVersion {
                    major: 0,
                    minor: 0,
                    patch: 0,
                };
            },
            |manifest| manifest.source_generation = manifest.target_generation,
            |manifest| manifest.counterpart_manifest_slot = HandoffDescriptorSlot::Listener,
        ] {
            let mut invalid = baseline.clone();
            mutate(&mut invalid);
            assert!(validate_manifest(&invalid).is_err());
        }
    }

    #[test]
    fn rejects_every_executable_identity_authority_and_seal_corruption() {
        let baseline = manifest(ManifestKind::Forward);
        for mutate in [
            (|image: &mut ManifestExecutableImage| image.source_device = 0)
                as fn(&mut ManifestExecutableImage),
            |image| image.source_inode = 0,
            |image| image.source_owner_uid = 1000,
            |image| image.source_mode = 0o755,
            |image| image.source_mode &= !0o111,
            |image| image.source_mode |= 0o022,
            |image| image.source_size = 0,
            |image| image.source_size = MAX_EXECUTABLE_SNAPSHOT_BYTES + 1,
            |image| image.snapshot_device = 0,
            |image| image.snapshot_inode = 0,
            |image| image.snapshot_size += 1,
            |image| {
                image.snapshot_device = image.source_device;
                image.snapshot_inode = image.source_inode;
            },
            |image| image.source_sha256 = [0; 32],
            |image| image.snapshot_sha256[0] ^= 1,
            |image| image.snapshot_seals = 0,
        ] {
            let mut invalid = baseline.clone();
            mutate(&mut invalid.executable_pairs.forward.daemon);
            assert!(matches!(
                validate_manifest(&invalid),
                Err(ManifestError::InvalidExecutables)
            ));
        }

        let mut aliased_pair = baseline.clone();
        aliased_pair.executable_pairs.forward.client.snapshot_device =
            aliased_pair.executable_pairs.forward.daemon.snapshot_device;
        aliased_pair.executable_pairs.forward.client.snapshot_inode =
            aliased_pair.executable_pairs.forward.daemon.snapshot_inode;
        assert!(matches!(
            validate_manifest(&aliased_pair),
            Err(ManifestError::InvalidExecutables)
        ));

        let mut wrong_generation = baseline;
        wrong_generation.executable_pairs.forward.generation = wrong_generation.source_generation;
        assert!(matches!(
            validate_manifest(&wrong_generation),
            Err(ManifestError::InvalidExecutables)
        ));
    }

    #[test]
    fn rejects_every_shared_forward_rollback_pair_mutation() {
        let (forward, rollback) = pair();
        for mutate in [
            (|manifest: &mut HandoffManifestV1| {
                manifest.handoff_id = HandoffId::new([8; 16]).unwrap();
            }) as fn(&mut HandoffManifestV1),
            |manifest| manifest.topology_revision += 1,
            |manifest| manifest.durable_state_generation += 1,
            |manifest| manifest.private_protocol += 1,
            |manifest| manifest.handoff_protocol += 1,
            |manifest| manifest.terminal_checkpoint_schema += 1,
            |manifest| manifest.splints[0].terminal_revision += 1,
            |manifest| manifest.descriptors[0].inherited_fd += 100,
            |manifest| {
                manifest.executable_pairs.forward.daemon.source_sha256[0] ^= 1;
                manifest.executable_pairs.forward.daemon.snapshot_sha256[0] ^= 1;
            },
        ] {
            let mut mismatched = rollback.clone();
            mutate(&mut mismatched);
            assert!(matches!(
                validate_manifest_pair(&forward, &mismatched),
                Err(ManifestError::PairMismatch)
            ));
        }
    }

    #[test]
    fn rejects_duplicate_missing_and_standard_descriptors() {
        let baseline = manifest(ManifestKind::Forward);
        let mut duplicate_slot = baseline.clone();
        duplicate_slot.descriptors.push(ManifestDescriptor {
            slot: HandoffDescriptorSlot::Listener,
            inherited_fd: 100,
        });
        assert!(matches!(
            validate_manifest(&duplicate_slot),
            Err(ManifestError::InvalidDescriptors)
        ));
        let mut duplicate_fd = baseline.clone();
        duplicate_fd.descriptors[1].inherited_fd = duplicate_fd.descriptors[0].inherited_fd;
        assert!(matches!(
            validate_manifest(&duplicate_fd),
            Err(ManifestError::InvalidDescriptors)
        ));
        let mut missing = baseline.clone();
        missing
            .descriptors
            .retain(|entry| entry.slot != HandoffDescriptorSlot::RollbackClient);
        assert!(matches!(
            validate_manifest(&missing),
            Err(ManifestError::InvalidDescriptors)
        ));
        let mut standard = baseline.clone();
        standard.descriptors[0].inherited_fd = 2;
        assert!(matches!(
            validate_manifest(&standard),
            Err(ManifestError::InvalidDescriptors)
        ));
        let mut oversized = baseline.clone();
        oversized.descriptors[0].inherited_fd = MAX_INHERITED_FD + 1;
        assert!(matches!(
            validate_manifest(&oversized),
            Err(ManifestError::InvalidDescriptors)
        ));
        for slot in [
            HandoffDescriptorSlot::Listener,
            HandoffDescriptorSlot::ImageListener,
            HandoffDescriptorSlot::CheckpointManifest,
            HandoffDescriptorSlot::RollbackManifest,
            HandoffDescriptorSlot::ForwardClient,
            HandoffDescriptorSlot::RollbackDaemon,
            HandoffDescriptorSlot::RollbackClient,
        ] {
            let mut missing = baseline.clone();
            missing.descriptors.retain(|entry| entry.slot != slot);
            assert!(matches!(
                validate_manifest(&missing),
                Err(ManifestError::InvalidDescriptors)
            ));
        }
        for slot in [
            HandoffDescriptorSlot::PtyMaster(99),
            HandoffDescriptorSlot::TerminalCheckpoint(99),
            HandoffDescriptorSlot::ImageState(99),
            HandoffDescriptorSlot::ClientProcess(0),
            HandoffDescriptorSlot::Continuation(0),
        ] {
            let mut unexpected = baseline.clone();
            unexpected.descriptors.push(ManifestDescriptor {
                slot,
                inherited_fd: 200,
            });
            assert!(matches!(
                validate_manifest(&unexpected),
                Err(ManifestError::InvalidDescriptors)
            ));
        }
    }

    #[test]
    fn rejects_duplicate_topology_slots_and_unsupported_runtime_state() {
        let baseline = manifest(ManifestKind::Forward);
        let mut duplicate = baseline.clone();
        duplicate.splints.push(duplicate.splints[0].clone());
        assert!(matches!(
            validate_manifest(&duplicate),
            Err(ManifestError::InvalidTopology)
        ));
        let mut duplicate_pty = baseline.clone();
        let mut second = duplicate_pty.splints[0].clone();
        second.splint_id = SplintId::new();
        duplicate_pty.splints.push(second);
        assert!(matches!(
            validate_manifest(&duplicate_pty),
            Err(ManifestError::InvalidDescriptors)
        ));
        let mut conflicting_parent = with_splint_count(2);
        conflicting_parent.splints[1].dojo_id = conflicting_parent.splints[0].dojo_id;
        conflicting_parent.splints[1].lair_id = LairId::new();
        assert!(matches!(
            validate_manifest(&conflicting_parent),
            Err(ManifestError::InvalidTopology)
        ));
        let mut duplicate_image = with_splint_count(2);
        duplicate_image.descriptors.push(ManifestDescriptor {
            slot: HandoffDescriptorSlot::ImageState(0),
            inherited_fd: 3000,
        });
        for splint in &mut duplicate_image.splints {
            splint.image_state = BodyDisposition::Descriptor(0);
        }
        assert!(matches!(
            validate_manifest(&duplicate_image),
            Err(ManifestError::InvalidDescriptors)
        ));
        for mutate in [
            (|splint: &mut ManifestSplint| splint.incarnation = 0) as fn(&mut ManifestSplint),
            |splint| splint.pty.child_pid = 0,
            |splint| splint.pty.process_group += 1,
            |splint| splint.pty.session_id += 1,
            |splint| splint.size.columns = 0,
            |splint| splint.size.rows = 0,
            |splint| splint.history_generation = 0,
            |splint| splint.foreground_process_group = Some(0),
            |splint| splint.exit = ExitDisposition::Exited,
            |splint| splint.terminal_checkpoint = BodyDisposition::NotPresent,
            |splint| splint.terminal_checkpoint = BodyDisposition::Unsupported,
            |splint| splint.image_state = BodyDisposition::Unsupported,
        ] {
            let mut invalid = baseline.clone();
            mutate(&mut invalid.splints[0]);
            assert!(validate_manifest(&invalid).is_err());
        }
    }
}
