//! First-operation candidate descriptor acquisition before Tokio.
//!
//! This binary-only module owns the candidate's audited unsafe invocation in
//! `splinterd`: it composes the two unsafe entrypoints of the
//! `splinterm-inherited-fd` leaf crate, which holds every raw borrow,
//! conversion, and closure. Private manifest decoding and typed binding remain
//! in the library behind opaque safe values. `tests/unsafe_boundary_audit.rs`
//! pins this block and the sealed-preflight launcher's descriptor closure in
//! `handoff_preflight.rs` as the package's only unsafe constructs.

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
/// Rejects malformed activation, multi-threaded entry or a competing handoff
/// guard, an absent, uninherited, mutable, or noncanonical manifest, manifest
/// self-link mismatch, raw claim failure, or typed binding mismatch.
///
/// The returned bootstrap holds the exclusive single-threaded handoff process
/// guard until staging releases it at the runtime boundary.
#[allow(
    unsafe_code,
    reason = "audited candidate invocation of the post-exec inherited-FD leaf boundary"
)]
pub fn acquire_before_tokio() -> Result<Option<splinterd::CandidateBootstrap>> {
    let Some(raw) = env::var_os(CANDIDATE_MANIFEST_FD_ENV) else {
        return Ok(None);
    };
    // Every failure names the activation, so a stray value inherited from a
    // parent environment is diagnosable rather than an opaque startup failure.
    (|| {
    let manifest_fd = raw
        .to_str()
        .and_then(|raw| raw.parse::<RawFd>().ok())
        .filter(|fd| *fd >= 3)
        .context("candidate manifest descriptor activation is invalid")?;
    let process_guard = splinterd::handoff_coordinator::HandoffProcessGuard::acquire()?;

    // SAFETY: `main` calls this first, before preflight dispatch, Tokio,
    // tracing, listeners, or any other descriptor owner exists, so the process
    // is single-threaded and owns no inherited descriptor. The source
    // generation passed the manifest and every listed descriptor through exec
    // with FD_CLOEXEC clear. Nothing between the positioned read, canonical
    // decode, expected-set derivation, and claim opens, closes, or replaces an
    // inheritable descriptor; the leaf's procfs reads are close-on-exec and
    // closed before the claim scans, as is the process guard's procfs thread
    // observation just above. So every non-standard inheritable
    // descriptor crossed exec without a surviving Rust owner, including
    // unexpected entries the claim closes. Both leaf entrypoints also reject a second thread.
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
    splinterd::bind_candidate_manifest(acquisition, claimed, process_guard).map(Some)
    })()
    .with_context(|| {
        format!(
            "{CANDIDATE_MANIFEST_FD_ENV} is set, so splinterd started as an upgrade-handoff candidate and will not serve"
        )
    })
}
