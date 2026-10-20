//! Test-only handoff seams (Plan 0061 D9), compiled only with the
//! `integration-test` feature.
//!
//! A real exec test needs two things no packaged daemon may ever do:
//!
//! * **A build-identity salt.** Forward preparation refuses a forward pair
//!   whose identity equals the running one ("no upgrade available"), and a
//!   candidate refuses a manifest whose target identity is not its own. A
//!   test therefore needs a second image of the same build whose identity
//!   differs. [`BUILD_SALT_MARKER`] is followed in the binary by
//!   [`BUILD_SALT_BYTES`] salt bytes, all zero as built (no salt, the
//!   unsalted identity); a test copies the binary and overwrites those bytes
//!   in its copy. The salt lives in a writable static of atomics, so the
//!   compiler cannot fold its value: the running image reads whatever bytes
//!   its file carried. Package provenance verification in these builds
//!   accepts a record naming the same salt (`integration_test_build_salt`)
//!   whose identity is [`salted_pair_build_identity`] of the canonical
//!   derivation; every other build has no salt field and refuses such a
//!   record (`package_provenance`).
//! * **A packaged-owner override.** Manifest validation requires every
//!   recorded executable source to be root-owned, which a test running as an
//!   ordinary user cannot create. [`PACKAGED_OWNER_UID_ENV`] names the uid
//!   that stands in for root.
//! * **Candidate fault points.** The rollback fault matrix (Plan 0061
//!   M3.11) needs a forward candidate to fail at each pre-commit step it
//!   cannot be made to fail at from outside. [`CANDIDATE_FAULT_ENV`] names
//!   the steps, each for one manifest kind, so the same environment, which
//!   crosses both execs, fails the forward candidate but not the rollback
//!   daemon (or, for the rollback-of-a-rollback case, the rollback daemon).
//! * **An old-side drill.** The old generation's full runtime stop and exec
//!   boundary (`stop_runtime_and_cross` in the binary) is reachable only
//!   through a trusted-UI `Execute`, which no test build can pass: live
//!   handoff is unsupported and the build is unpackaged. With
//!   [`OLD_SIDE_DRILL_ENV`] set, a real daemon process runs `Execute` with
//!   support and reasons forced and a test preparation that holds no forward
//!   daemon, so the boundary is crossed but never armed, and it sends itself
//!   a SIGTERM during the runtime stop (Plan 0061 M3.17).
//!
//! Packaging must never ship any of these seams. Every package recipe (the PKGBUILDs
//! and the Nix package) generates its provenance record with
//! `tools/package/generate-package-provenance.py`, which refuses a daemon or
//! client binary containing [`BUILD_SALT_MARKER`]: the marker is present
//! exactly when this module is compiled in, and a binary without a
//! provenance record cannot authorize a handoff at all.

use std::sync::atomic::{AtomicU8, Ordering};

use sha2::{Digest, Sha256};

use crate::handoff_compatibility::BuildIdentity;

/// The bytes that precede the salt in every `integration-test` build. The
/// packaging guard searches for exactly these bytes.
pub const BUILD_SALT_MARKER: &[u8; 32] = b"splinterm-integration-test-salt:";

/// The number of salt bytes after [`BUILD_SALT_MARKER`].
pub const BUILD_SALT_BYTES: usize = 16;

/// Names the uid that manifest validation accepts in place of root as the
/// owner of recorded executable sources. Read only by `integration-test`
/// builds.
pub const PACKAGED_OWNER_UID_ENV: &str = "SPLINTERD_INTEGRATION_TEST_PACKAGED_OWNER_UID";

/// The marker's length, as a constant, so no runtime code refers to the
/// marker bytes themselves.
const MARKER_BYTES: usize = BUILD_SALT_MARKER.len();
const SALT_CELLS: usize = MARKER_BYTES + BUILD_SALT_BYTES;

const fn salt_cells() -> [AtomicU8; SALT_CELLS] {
    let mut cells = [const { AtomicU8::new(0) }; SALT_CELLS];
    let mut index = 0;
    while index < MARKER_BYTES {
        cells[index] = AtomicU8::new(BUILD_SALT_MARKER[index]);
        index += 1;
    }
    cells
}

/// The marker followed by the salt, as one contiguous run of bytes in the
/// binary's writable data. Only the salt half is ever read at run time. A
/// compiler or debug-info copy of the marker constant may still appear
/// elsewhere in a binary, so locate this static with [`salt_slot_offset`],
/// never by the first raw marker match.
#[used]
static BUILD_SALT: [AtomicU8; SALT_CELLS] = salt_cells();

/// The salt this image carries; all zero unless a test patched its file.
fn build_salt() -> [u8; BUILD_SALT_BYTES] {
    let mut salt = [0; BUILD_SALT_BYTES];
    for (byte, cell) in salt.iter_mut().zip(&BUILD_SALT[MARKER_BYTES..]) {
        *byte = cell.load(Ordering::Relaxed);
    }
    salt
}

/// The file offset of the patchable salt slot in an `integration-test`
/// binary's bytes: just after the one occurrence of [`BUILD_SALT_MARKER`]
/// followed by [`BUILD_SALT_BYTES`] zero bytes, as an unpatched image
/// carries it. `None` unless exactly one occurrence has that shape.
///
/// A test that salts a copy of the binary writes its salt at this offset;
/// the packaging guard only needs the marker present at all.
#[must_use]
pub fn salt_slot_offset(binary: &[u8]) -> Option<usize> {
    let mut slots = binary
        .windows(SALT_CELLS)
        .enumerate()
        .filter(|(_, window)| {
            window[..MARKER_BYTES] == BUILD_SALT_MARKER[..]
                && window[MARKER_BYTES..].iter().all(|byte| *byte == 0)
        })
        .map(|(offset, _)| offset + MARKER_BYTES);
    let slot = slots.next()?;
    slots.next().is_none().then_some(slot)
}

/// The pair identity an image with `salt` reports for an unsalted
/// `identity`. A zero salt leaves the identity unchanged.
#[must_use]
pub fn salted_pair_build_identity(
    identity: BuildIdentity,
    salt: [u8; BUILD_SALT_BYTES],
) -> BuildIdentity {
    if salt == [0; BUILD_SALT_BYTES] {
        return identity;
    }
    let mut hasher = Sha256::new();
    hasher.update(b"splinterm-integration-test-salted-pair-v1\0");
    hasher.update(identity.as_bytes());
    hasher.update(salt);
    // A SHA-256 output is all zero with negligible probability; keep the
    // unsalted identity rather than panic if it ever were.
    BuildIdentity::new(hasher.finalize().into()).unwrap_or(identity)
}

/// The compiled pair identity, salted with this image's own salt.
pub(crate) fn salt_compiled_identity(identity: BuildIdentity) -> BuildIdentity {
    salted_pair_build_identity(identity, build_salt())
}

/// The uid [`PACKAGED_OWNER_UID_ENV`] names, if it names one.
pub(crate) fn packaged_owner_uid_override() -> Option<u32> {
    std::env::var(PACKAGED_OWNER_UID_ENV).ok()?.parse().ok()
}

/// Arms candidate fault points: a comma-separated list of
/// `<point>@<forward|rollback>`, each naming a [`CandidateFaultPoint`] by
/// [`CandidateFaultPoint::name`] and the manifest kind whose candidate fails
/// there. Read only by `integration-test` builds.
pub const CANDIDATE_FAULT_ENV: &str = "SPLINTERD_INTEGRATION_TEST_CANDIDATE_FAULT";

/// One pre-commit step a test can make a candidate fail at, in the order a
/// candidate reaches them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateFaultPoint {
    /// After staging verified every descriptor, before it transfers them.
    Staging,
    /// Leaving the single-threaded boundary, before any runtime exists.
    RuntimeEntry,
    /// On the serving runtime, after the signal streams and durable state.
    Startup,
    /// Right after both inherited listeners registered with the runtime.
    ListenerRegistration,
    /// Where workload scopes are adopted, whether or not a manager exists.
    ScopeAdoption,
    /// After the first carried actor staged, before the next one.
    ActorStaging,
    /// Not a failure: at startup, leaves one inheritable descriptor open
    /// that no manifest names, as a leaked descriptor would
    /// ([`leak_inheritable_descriptor`]).
    StrayDescriptor,
    /// Not a failure: once a pre-commit failure has been decided and the
    /// handoff signals blocked on the main thread, sends this process a
    /// SIGTERM ([`terminate_this_process`]) while the runtime's workers still
    /// run with them unblocked, as `systemctl stop` during rollback teardown.
    TerminateDuringTeardown,
    /// At startup, starts a thread that outlives the runtime
    /// ([`outlive_the_runtime`]), then fails as at [`Self::Startup`]. The
    /// rollback executor's wait for one thread then times out, as it would
    /// for a blocking task the runtime abandoned, and the candidate exits
    /// loudly with no rollback exec (Plan 0061 M3.17).
    GuardTimeout,
}

impl CandidateFaultPoint {
    /// The point's spelling in [`CANDIDATE_FAULT_ENV`].
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Staging => "staging",
            Self::RuntimeEntry => "runtime-entry",
            Self::Startup => "startup",
            Self::ListenerRegistration => "listener-registration",
            Self::ScopeAdoption => "scope-adoption",
            Self::ActorStaging => "actor-staging",
            Self::StrayDescriptor => "stray-descriptor",
            Self::TerminateDuringTeardown => "terminate-during-teardown",
            Self::GuardTimeout => "guard-timeout",
        }
    }
}

/// Whether [`CANDIDATE_FAULT_ENV`] arms `point` for a candidate activated by
/// a manifest of `kind`.
pub(crate) fn candidate_fault(
    point: CandidateFaultPoint,
    kind: crate::handoff_manifest::ManifestKind,
) -> bool {
    let kind = match kind {
        crate::handoff_manifest::ManifestKind::Forward => "forward",
        crate::handoff_manifest::ManifestKind::Rollback => "rollback",
    };
    std::env::var(CANDIDATE_FAULT_ENV).is_ok_and(|armed| {
        armed
            .split(',')
            .filter_map(|entry| entry.trim().split_once('@'))
            .any(|(armed_point, armed_kind)| armed_point == point.name() && armed_kind == kind)
    })
}

/// Opens `/dev/null` and leaves it open and inheritable across exec,
/// returning its number: a stand-in for a descriptor leaked without
/// close-on-exec (the zbus `SCM_RIGHTS` case). Only the
/// [`CandidateFaultPoint::StrayDescriptor`] seam calls it.
///
/// # Errors
/// Reports a failed open or flag update.
pub fn leak_inheritable_descriptor() -> std::io::Result<std::os::fd::RawFd> {
    use std::os::fd::IntoRawFd;

    let file = std::fs::File::open("/dev/null")?;
    let flags = rustix::io::fcntl_getfd(&file)?;
    rustix::io::fcntl_setfd(&file, flags - rustix::io::FdFlags::CLOEXEC)?;
    // Leaked on purpose: nothing owns it once this returns.
    Ok(file.into_raw_fd())
}

/// Starts a detached thread that sleeps for `duration`, a stand-in for a
/// blocking task that outlived the runtime's shutdown. Only the
/// [`CandidateFaultPoint::GuardTimeout`] seam calls it.
///
/// # Errors
/// Reports a failed thread spawn.
pub fn outlive_the_runtime(duration: std::time::Duration) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("test-seam-outlives-runtime".to_owned())
        .spawn(move || std::thread::sleep(duration))
        .map(drop)
}

/// Arms the old-side drill in a real daemon process (see the module
/// documentation). Read only by `integration-test` builds.
pub const OLD_SIDE_DRILL_ENV: &str = "SPLINTERD_INTEGRATION_TEST_OLD_SIDE_DRILL";

/// Whether [`OLD_SIDE_DRILL_ENV`] is set to `1`.
#[must_use]
pub fn old_side_drill() -> bool {
    std::env::var(OLD_SIDE_DRILL_ENV).is_ok_and(|value| value == "1")
}

/// Sends this process a process-directed SIGTERM, which the kernel delivers
/// to any thread that does not block it. Only the
/// [`CandidateFaultPoint::TerminateDuringTeardown`] seam and the old-side
/// drill ([`OLD_SIDE_DRILL_ENV`]) call it.
///
/// # Errors
/// Reports a failed `kill`.
pub fn terminate_this_process() -> std::io::Result<()> {
    Ok(rustix::process::kill_process(
        rustix::process::getpid(),
        rustix::process::Signal::TERM,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unpatched_image_reports_the_unsalted_identity() {
        let identity = BuildIdentity::new([7; 32]).unwrap();
        assert_eq!(build_salt(), [0; BUILD_SALT_BYTES]);
        assert_eq!(salt_compiled_identity(identity), identity);
    }

    /// The slot is the one marker followed by an all-zero salt; a separate
    /// copy of the marker constant elsewhere does not confuse it.
    #[test]
    fn the_salt_slot_is_the_marker_followed_by_a_zero_salt() {
        let mut binary = b"head".to_vec();
        binary.extend_from_slice(BUILD_SALT_MARKER);
        binary.extend_from_slice(b"other constant data");
        let slot = binary.len() + MARKER_BYTES;
        binary.extend_from_slice(BUILD_SALT_MARKER);
        binary.extend_from_slice(&[0; BUILD_SALT_BYTES]);
        binary.extend_from_slice(b"tail");
        assert_eq!(salt_slot_offset(&binary), Some(slot));
        let mut ambiguous = binary.clone();
        ambiguous.extend_from_slice(BUILD_SALT_MARKER);
        ambiguous.extend_from_slice(&[0; BUILD_SALT_BYTES]);
        assert_eq!(salt_slot_offset(&ambiguous), None);
        assert_eq!(salt_slot_offset(b"no marker"), None);
    }

    #[test]
    fn a_salt_changes_the_identity_deterministically() {
        let identity = BuildIdentity::new([7; 32]).unwrap();
        let salted = salted_pair_build_identity(identity, [1; BUILD_SALT_BYTES]);
        assert_ne!(salted, identity);
        assert_eq!(
            salted,
            salted_pair_build_identity(identity, [1; BUILD_SALT_BYTES])
        );
        assert_ne!(
            salted,
            salted_pair_build_identity(identity, [2; BUILD_SALT_BYTES])
        );
    }
}
