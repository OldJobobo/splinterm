//! First-operation candidate descriptor acquisition before Tokio.
//!
//! This binary-only module owns the candidate's audited unsafe invocation in
//! `splinterd`: it composes the two unsafe entrypoints of the
//! `splinterm-inherited-fd` leaf crate, which holds every raw borrow,
//! conversion, and closure. Private manifest decoding and typed binding remain
//! in the library behind opaque safe values. `tests/unsafe_boundary_audit.rs`
//! pins this block and the sealed-preflight launcher's descriptor closure in
//! `handoff_preflight.rs` as the package's only unsafe constructs.
//!
//! Activation is an argv flag rather than an environment variable (Plan 0061
//! D4): a spawned child inherits the daemon's environment but never its argv,
//! so no PTY helper, shell, consent client, or preflight launcher can observe
//! or re-trigger activation. This module owns the flag; the ordinary daemon
//! argument parser only ever sees what [`daemon_arguments`] leaves.

use std::{
    env,
    ffi::OsString,
    fmt,
    os::{fd::RawFd, unix::ffi::OsStrExt},
};

use anyhow::{Context, Result};

/// The candidate activation flag, spelled `ACTIVATION_ARGUMENT=<fd>`, where
/// `<fd>` is the canonical decimal number of the inherited forward manifest.
/// It must be the first argument; the ordinary daemon arguments follow it.
pub const ACTIVATION_ARGUMENT: &str = "--splinterm-internal-handoff-manifest-fd-v1";

/// Every argument starting with this is an activation attempt. Only the exact
/// [`ACTIVATION_ARGUMENT`] spelling is valid, so a near miss or a future
/// version fails closed as a candidate instead of reaching normal startup.
const ACTIVATION_FAMILY: &[u8] = b"--splinterm-internal-handoff-manifest-fd";

/// Why an activation attempt in argv is malformed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActivationError {
    Duplicated,
    NotFirst,
    Unrecognized,
    MissingDescriptor,
    NoncanonicalDescriptor,
    StandardDescriptor,
}

impl fmt::Display for ActivationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Duplicated => "candidate activation appears more than once",
            Self::NotFirst => "candidate activation is not the first argument",
            Self::Unrecognized => "candidate activation argument is not recognized",
            Self::MissingDescriptor => "candidate activation names no manifest descriptor",
            Self::NoncanonicalDescriptor => {
                "candidate manifest descriptor is not a canonical decimal number"
            }
            Self::StandardDescriptor => "candidate manifest descriptor is a standard stream",
        })
    }
}

impl std::error::Error for ActivationError {}

fn is_activation_attempt(argument: &OsString) -> bool {
    argument.as_bytes().starts_with(ACTIVATION_FAMILY)
}

/// Splits argv (without the program name) into the candidate manifest
/// descriptor and the ordinary daemon arguments after it.
///
/// `Ok(None)` means no argument belongs to the activation family: normal
/// startup. This only inspects bytes already in memory; it touches no
/// descriptor.
fn split_activation(
    arguments: &[OsString],
) -> Result<Option<(RawFd, &[OsString])>, ActivationError> {
    match arguments
        .iter()
        .filter(|argument| is_activation_attempt(argument))
        .count()
    {
        0 => return Ok(None),
        1 => {}
        _ => return Err(ActivationError::Duplicated),
    }
    let Some((first, rest)) = arguments
        .split_first()
        .filter(|(first, _)| is_activation_attempt(first))
    else {
        return Err(ActivationError::NotFirst);
    };
    let value = first
        .as_bytes()
        .strip_prefix(ACTIVATION_ARGUMENT.as_bytes())
        .ok_or(ActivationError::Unrecognized)?;
    let value = match value.strip_prefix(b"=") {
        Some(value) => value,
        None if value.is_empty() => return Err(ActivationError::MissingDescriptor),
        None => return Err(ActivationError::Unrecognized),
    };
    Ok(Some((parse_descriptor(value)?, rest)))
}

/// Accepts only the canonical decimal spelling of a descriptor above standard
/// error: ASCII digits, no sign, no leading zero, within `RawFd`.
fn parse_descriptor(value: &[u8]) -> Result<RawFd, ActivationError> {
    if value.is_empty() {
        return Err(ActivationError::MissingDescriptor);
    }
    if !value.iter().all(u8::is_ascii_digit) || (value.len() > 1 && value[0] == b'0') {
        return Err(ActivationError::NoncanonicalDescriptor);
    }
    let descriptor = std::str::from_utf8(value)
        .ok()
        .and_then(|value| value.parse::<RawFd>().ok())
        .ok_or(ActivationError::NoncanonicalDescriptor)?;
    if descriptor < 3 {
        return Err(ActivationError::StandardDescriptor);
    }
    Ok(descriptor)
}

/// Returns the ordinary daemon arguments: argv after the program name, less a
/// valid leading candidate activation, which only [`acquire_before_tokio`]
/// consumes. The daemon parser accepts no activation argument itself, so in a
/// normal start it can never pass.
///
/// # Errors
/// Rejects malformed activation exactly as acquisition does.
pub fn daemon_arguments() -> Result<Vec<OsString>> {
    let mut arguments = env::args_os().skip(1).collect::<Vec<_>>();
    if split_activation(&arguments)?.is_some() {
        arguments.remove(0);
    }
    Ok(arguments)
}

/// Acquires the complete inherited descriptor set before Tokio exists.
///
/// No argument in the activation family selects normal daemon startup. Once
/// activation is present, every error is fatal to the candidate and must
/// later route to cooperative rollback rather than ordinary startup.
///
/// # Errors
/// Rejects malformed or duplicated activation and invalid daemon arguments
/// before claiming anything, then multi-threaded entry or a competing handoff
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
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    let Some(activation) = split_activation(&arguments).transpose() else {
        return Ok(None);
    };
    // Every failure names the activation, so a candidate failure is never
    // mistaken for an ordinary startup failure.
    (|| {
    let (manifest_fd, daemon_arguments) = activation?;
    // The daemon arguments are validated before anything is claimed, by the
    // same exact parser normal startup applies.
    crate::parse_daemon_arguments(daemon_arguments.iter().cloned())?;
    let process_guard = splinterd::handoff_coordinator::HandoffProcessGuard::acquire()?;

    // SAFETY: `main` calls this first, before preflight dispatch, Tokio,
    // tracing, listeners, or any other descriptor owner exists, so the process
    // is single-threaded and owns no inherited descriptor; the argument
    // parsing above only reads memory. The source generation passed the
    // manifest and every listed descriptor through exec with FD_CLOEXEC
    // clear. Nothing between the positioned read, canonical decode,
    // expected-set derivation, and claim opens, closes, or replaces an
    // inheritable descriptor; the leaf's procfs reads are close-on-exec and
    // closed before the claim scans, as is the process guard's procfs thread
    // observation just above. So every non-standard inheritable descriptor
    // crossed exec without a surviving Rust owner, including unexpected
    // entries the claim closes. Both leaf entrypoints also reject a second
    // thread.
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
            "{ACTIVATION_ARGUMENT} is present, so splinterd started as an upgrade-handoff candidate and will not serve"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn split(values: &[&str]) -> Result<Option<(RawFd, Vec<OsString>)>, ActivationError> {
        let values = arguments(values);
        split_activation(&values).map(|split| split.map(|(fd, rest)| (fd, rest.to_vec())))
    }

    fn activation(value: &str) -> String {
        format!("{ACTIVATION_ARGUMENT}={value}")
    }

    #[test]
    fn absent_activation_selects_normal_startup() {
        assert_eq!(split(&[]), Ok(None));
        assert_eq!(split(&["--require-workload-cgroups"]), Ok(None));
        assert_eq!(split(&["--unknown", "extra"]), Ok(None));
    }

    #[test]
    fn valid_activation_yields_the_descriptor_and_the_daemon_arguments() {
        assert_eq!(
            split(&[activation("3").as_str()]),
            Ok(Some((3, Vec::new())))
        );
        assert_eq!(
            split(&[activation("1024").as_str(), "--require-workload-cgroups"]),
            Ok(Some((1024, arguments(&["--require-workload-cgroups"]))))
        );
        // The remainder is the daemon parser's to judge, before any claim.
        assert_eq!(
            split(&[activation("7").as_str(), "--unknown"]),
            Ok(Some((7, arguments(&["--unknown"]))))
        );
        assert_eq!(
            split(&[activation(&RawFd::MAX.to_string()).as_str()]),
            Ok(Some((RawFd::MAX, Vec::new())))
        );
    }

    #[test]
    fn malformed_descriptor_numbers_fail_closed() {
        for value in [
            "x",
            "3x",
            "-3",
            "+3",
            " 3",
            "3 ",
            "03",
            "00",
            "0x3",
            "2147483648",
            "99999999999",
        ] {
            assert_eq!(
                split(&[activation(value).as_str()]),
                Err(ActivationError::NoncanonicalDescriptor),
                "{value:?}"
            );
        }
    }

    #[test]
    fn standard_stream_descriptors_fail_closed() {
        for value in ["0", "1", "2"] {
            assert_eq!(
                split(&[activation(value).as_str()]),
                Err(ActivationError::StandardDescriptor),
                "{value:?}"
            );
        }
    }

    #[test]
    fn duplicated_activation_fails_closed() {
        assert_eq!(
            split(&[activation("3").as_str(), activation("3").as_str()]),
            Err(ActivationError::Duplicated)
        );
        assert_eq!(
            split(&[
                activation("3").as_str(),
                "--require-workload-cgroups",
                activation("4").as_str()
            ]),
            Err(ActivationError::Duplicated)
        );
        assert_eq!(
            split(&[activation("3").as_str(), ACTIVATION_ARGUMENT]),
            Err(ActivationError::Duplicated)
        );
    }

    #[test]
    fn missing_descriptor_value_fails_closed() {
        assert_eq!(
            split(&[ACTIVATION_ARGUMENT]),
            Err(ActivationError::MissingDescriptor)
        );
        assert_eq!(
            split(&[activation("").as_str()]),
            Err(ActivationError::MissingDescriptor)
        );
        // A separate value word is not the grammar.
        assert_eq!(
            split(&[ACTIVATION_ARGUMENT, "3"]),
            Err(ActivationError::MissingDescriptor)
        );
    }

    #[test]
    fn activation_after_another_argument_fails_closed() {
        assert_eq!(
            split(&["--require-workload-cgroups", activation("3").as_str()]),
            Err(ActivationError::NotFirst)
        );
    }

    #[test]
    fn near_miss_spellings_fail_closed_instead_of_starting_normally() {
        for argument in [
            "--splinterm-internal-handoff-manifest-fd=3",
            "--splinterm-internal-handoff-manifest-fd",
            "--splinterm-internal-handoff-manifest-fd-v2=3",
            "--splinterm-internal-handoff-manifest-fd-v1x=3",
            "--splinterm-internal-handoff-manifest-fd-v10=3",
        ] {
            assert_eq!(
                split(&[argument]),
                Err(ActivationError::Unrecognized),
                "{argument:?}"
            );
        }
    }

    #[test]
    fn non_utf8_activation_values_fail_closed() {
        use std::os::unix::ffi::OsStringExt;

        let mut bytes = activation("3").into_bytes();
        bytes.push(0xff);
        assert_eq!(
            split_activation(&[OsString::from_vec(bytes)]),
            Err(ActivationError::NoncanonicalDescriptor)
        );
    }

    #[test]
    fn the_daemon_parser_never_accepts_activation_in_a_normal_start() {
        for argument in [activation("3"), ACTIVATION_ARGUMENT.to_owned()] {
            assert!(crate::parse_daemon_arguments([OsString::from(argument)]).is_err());
        }
    }
}
