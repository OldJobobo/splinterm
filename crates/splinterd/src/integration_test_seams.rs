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
//!   its file carried.
//! * **A packaged-owner override.** Manifest validation requires every
//!   recorded executable source to be root-owned, which a test running as an
//!   ordinary user cannot create. [`PACKAGED_OWNER_UID_ENV`] names the uid
//!   that stands in for root.
//!
//! Packaging must never ship either seam. Every package recipe (the PKGBUILDs
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

const SALT_CELLS: usize = BUILD_SALT_MARKER.len() + BUILD_SALT_BYTES;

const fn salt_cells() -> [AtomicU8; SALT_CELLS] {
    let mut cells = [const { AtomicU8::new(0) }; SALT_CELLS];
    let mut index = 0;
    while index < BUILD_SALT_MARKER.len() {
        cells[index] = AtomicU8::new(BUILD_SALT_MARKER[index]);
        index += 1;
    }
    cells
}

/// The marker followed by the salt, as one contiguous run of bytes in the
/// binary's writable data. Only the salt half is ever read, so the marker
/// appears in the binary exactly once.
#[used]
static BUILD_SALT: [AtomicU8; SALT_CELLS] = salt_cells();

/// The salt this image carries; all zero unless a test patched its file.
fn build_salt() -> [u8; BUILD_SALT_BYTES] {
    let mut salt = [0; BUILD_SALT_BYTES];
    for (byte, cell) in salt.iter_mut().zip(&BUILD_SALT[BUILD_SALT_MARKER.len()..]) {
        *byte = cell.load(Ordering::Relaxed);
    }
    salt
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unpatched_image_reports_the_unsalted_identity() {
        let identity = BuildIdentity::new([7; 32]).unwrap();
        assert_eq!(build_salt(), [0; BUILD_SALT_BYTES]);
        assert_eq!(salt_compiled_identity(identity), identity);
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
