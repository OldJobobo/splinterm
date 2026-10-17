//! First-operation candidate descriptor acquisition before Tokio.
//!
//! This binary-only module owns the sole audited unsafe invocation in
//! `splinterd`: it composes the two unsafe entrypoints of the
//! `splinterm-inherited-fd` leaf crate, which holds every raw borrow,
//! conversion, and closure. Private manifest decoding and typed binding remain
//! in the library behind opaque safe values. `tests/unsafe_boundary_audit.rs`
//! pins this block as the package's only unsafe construct.

use std::{env, os::fd::RawFd};

use anyhow::{Context, Result};

const CANDIDATE_MANIFEST_FD_ENV: &str = "SPLINTERM_INTERNAL_HANDOFF_MANIFEST_FD";

/// Acquires the complete inherited descriptor set before Tokio exists.
///
/// Absence of the reserved environment variable selects normal daemon startup.
/// Once activation is present, every error is fatal to the candidate and must
/// later route to cooperative rollback rather than ordinary startup.
///
/// # Errors
/// Rejects malformed activation, multi-threaded entry, an absent, uninherited,
/// mutable, or noncanonical manifest, manifest self-link mismatch, raw claim
/// failure, or typed binding mismatch.
#[allow(
    unsafe_code,
    reason = "sole audited invocation of the post-exec inherited-FD leaf boundary"
)]
pub fn acquire_before_tokio() -> Result<Option<splinterd::CandidateBootstrap>> {
    let Some(raw) = env::var_os(CANDIDATE_MANIFEST_FD_ENV) else {
        return Ok(None);
    };
    let manifest_fd = raw
        .to_str()
        .and_then(|raw| raw.parse::<RawFd>().ok())
        .filter(|fd| *fd >= 3)
        .context("candidate manifest descriptor activation is invalid")?;

    // SAFETY: `main` calls this first, before preflight dispatch, Tokio,
    // tracing, listeners, or any other descriptor owner exists, so the process
    // is single-threaded and owns no inherited descriptor. The source
    // generation passed the manifest and every listed descriptor through exec
    // with FD_CLOEXEC clear. Nothing between the positioned read, canonical
    // decode, expected-set derivation, and claim opens, closes, or replaces a
    // descriptor, so every non-standard inheritable descriptor crossed exec
    // without a surviving Rust owner, including unexpected entries the claim
    // closes. Both leaf entrypoints also reject a second thread.
    let (acquisition, claimed) = unsafe {
        let bytes = splinterm_inherited_fd::read_post_exec_sealed_descriptor(
            manifest_fd,
            splinterd::handoff_manifest_max_bytes(),
        )?;
        let acquisition = splinterd::decode_candidate_manifest(&bytes, manifest_fd)?;
        let claimed = splinterm_inherited_fd::claim_post_exec_inherited(
            acquisition.expected_raw_descriptors(),
        )?;
        (acquisition, claimed)
    };
    splinterd::bind_candidate_manifest(acquisition, claimed).map(Some)
}
