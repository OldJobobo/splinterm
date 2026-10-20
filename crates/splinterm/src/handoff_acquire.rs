//! First-operation descriptor acquisition for a Window relaunched across a
//! daemon handoff (Plan 0061 M3.15, D12).
//!
//! A Window that took part in a handoff replaces its own process image once
//! the continuation reports the outcome (`splinterm::window_continuation`):
//! with the sealed forward client snapshot after an adoption, or with the
//! installed `<prefix>/splinterm` after a rollback. The new image inherits
//! exactly two descriptors, the sealed resume record and the client end of
//! the continuation socket, named by two argv flags
//! ([`splinterm_protocol::window_resume`]).
//!
//! This binary-only module owns the graphical client's one audited unsafe
//! invocation: it calls the `splinterm-inherited-fd` leaf crate's post-exec
//! claim, which holds every raw conversion and closure, before Tokio or any
//! other thread exists. `crates/splinterd/tests/unsafe_boundary_audit.rs`
//! pins this block as the package's only unsafe construct and pins `main`'s
//! opening so nothing that could own a descriptor runs first.
//!
//! Activation is argv rather than the environment (Plan 0061 D4): a child
//! the client spawns inherits its environment, never its argv, so nothing it
//! starts can observe or re-trigger activation. The flags are the only
//! arguments; every other argument in their families fails closed instead of
//! reaching the ordinary command-line parser.

use std::{
    env,
    ffi::OsString,
    fmt,
    os::{
        fd::{AsFd, OwnedFd, RawFd},
        unix::ffi::OsStrExt,
    },
};

use anyhow::{Context, Result, bail};
use rustix::{
    fs::{FileType, fstat},
    net::{AddressFamily, SocketType, sockopt},
};
use splinterm_protocol::window_resume::{
    CONTINUATION_FD_ARGUMENT, CONTINUATION_FD_FAMILY, WINDOW_RESUME_FD_ARGUMENT,
    WINDOW_RESUME_FD_FAMILY, WindowResumeRecord, read_window_resume_record,
};

/// Everything a relaunched Window claimed and validated before Tokio.
pub(crate) struct AcquiredWindowResume {
    pub(crate) record: WindowResumeRecord,
    /// The client end of the continuation socket, close-on-exec. It stays
    /// open for the Window's life, so the adopted daemon's hold sees the
    /// Window alive; M3.16's `ClaimHandoffControl` takes it from here.
    pub(crate) continuation: OwnedFd,
    /// Inheritable descriptors other than the two the claim closed.
    pub(crate) closed_unexpected: usize,
}

/// Why an activation attempt in argv is malformed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActivationError {
    Duplicated,
    Position,
    Unrecognized,
    MissingDescriptor,
    NoncanonicalDescriptor,
    StandardDescriptor,
    SameDescriptor,
}

impl fmt::Display for ActivationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Duplicated => "window resume activation appears more than once",
            Self::Position => {
                "window resume activation must be exactly the record flag, then the continuation flag"
            }
            Self::Unrecognized => "window resume activation argument is not recognized",
            Self::MissingDescriptor => "window resume activation names no descriptor",
            Self::NoncanonicalDescriptor => {
                "window resume descriptor is not a canonical decimal number"
            }
            Self::StandardDescriptor => "window resume descriptor is a standard stream",
            Self::SameDescriptor => "window resume record and continuation share a descriptor",
        })
    }
}

impl std::error::Error for ActivationError {}

fn in_family(argument: &OsString, family: &str) -> bool {
    argument.as_bytes().starts_with(family.as_bytes())
}

fn is_activation_attempt(argument: &OsString) -> bool {
    in_family(argument, WINDOW_RESUME_FD_FAMILY) || in_family(argument, CONTINUATION_FD_FAMILY)
}

/// Splits argv (without the program name) into the resume-record and
/// continuation descriptors.
///
/// `Ok(None)` means no argument belongs to either family: ordinary startup.
/// This only inspects bytes already in memory; it touches no descriptor.
fn split_activation(arguments: &[OsString]) -> Result<Option<(RawFd, RawFd)>, ActivationError> {
    let attempts = arguments
        .iter()
        .filter(|argument| is_activation_attempt(argument))
        .count();
    if attempts == 0 {
        return Ok(None);
    }
    let resume_attempts = arguments
        .iter()
        .filter(|argument| in_family(argument, WINDOW_RESUME_FD_FAMILY))
        .count();
    if resume_attempts > 1 || attempts - resume_attempts > 1 {
        return Err(ActivationError::Duplicated);
    }
    let [resume, continuation] = arguments else {
        return Err(ActivationError::Position);
    };
    if !in_family(resume, WINDOW_RESUME_FD_FAMILY)
        || !in_family(continuation, CONTINUATION_FD_FAMILY)
    {
        return Err(ActivationError::Position);
    }
    let resume = flag_descriptor(resume, WINDOW_RESUME_FD_ARGUMENT)?;
    let continuation = flag_descriptor(continuation, CONTINUATION_FD_ARGUMENT)?;
    if resume == continuation {
        return Err(ActivationError::SameDescriptor);
    }
    Ok(Some((resume, continuation)))
}

fn flag_descriptor(argument: &OsString, flag: &str) -> Result<RawFd, ActivationError> {
    let value = argument
        .as_bytes()
        .strip_prefix(flag.as_bytes())
        .ok_or(ActivationError::Unrecognized)?;
    let value = match value.strip_prefix(b"=") {
        Some(value) => value,
        None if value.is_empty() => return Err(ActivationError::MissingDescriptor),
        None => return Err(ActivationError::Unrecognized),
    };
    parse_descriptor(value)
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

/// The continuation must be the client end of a Unix sequenced-packet pair
/// whose peer runs as this user: the daemon that made it before exec, or the
/// daemon that took its process over. `SO_PASSCRED` is set on the daemon's
/// end only (it is the receiver of credentials), so it is not asked of this
/// end.
fn validate_continuation(continuation: &OwnedFd) -> Result<()> {
    let stat = fstat(continuation).context("cannot inspect the continuation descriptor")?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Socket
        || sockopt::socket_domain(continuation).ok() != Some(AddressFamily::UNIX)
        || sockopt::socket_type(continuation).ok() != Some(SocketType::SEQPACKET)
    {
        bail!("the continuation descriptor is not a Unix sequenced-packet socket");
    }
    let peer = sockopt::socket_peercred(continuation)
        .context("the continuation socket has no peer credentials")?;
    if peer.uid != rustix::process::geteuid() {
        bail!("the continuation socket's peer runs as another user");
    }
    Ok(())
}

/// Claims and validates a relaunched Window's descriptors before Tokio.
///
/// No argument in either activation family selects ordinary startup and
/// touches nothing. Once activation is present, every failure is fatal: the
/// resume record is the only source of the Window's tabs, so a client that
/// cannot read it exits with a clear message instead of opening something
/// unasked.
///
/// # Errors
/// Rejects malformed or duplicated activation before claiming anything, then
/// a multi-threaded process, an absent or uninherited descriptor, a record
/// that is not a bounded sealed memfd holding a valid record, and a
/// continuation that is not a Unix sequenced-packet socket of this user.
#[allow(
    unsafe_code,
    reason = "audited relaunched-Window invocation of the post-exec inherited-FD leaf boundary"
)]
pub(crate) fn acquire_before_tokio() -> Result<Option<AcquiredWindowResume>> {
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();
    let Some(activation) = split_activation(&arguments).transpose() else {
        return Ok(None);
    };
    (|| -> Result<AcquiredWindowResume> {
        let (resume_fd, continuation_fd) = activation?;
        // SAFETY: `main` calls this right after the askpass and sealed-preflight
        // dispatches, each of which, without its own activation, reads only
        // argv and the environment and owns no descriptor. So the process is
        // single-threaded (no Tokio runtime, no Wayland or font thread yet)
        // and holds no Rust owner of any inherited descriptor; the argument
        // parsing above only reads memory. Whoever exec'd this image (a
        // relaunching Window, its launcher, or anyone running the binary with
        // these flags), every non-standard descriptor that is inheritable now
        // crossed exec and has no Rust owner in this image: this image has
        // created no owner of any descriptor yet, and descriptors it opens are
        // close-on-exec. The claim takes ownership of exactly the two named
        // numbers, which are then validated below as a sealed resume record
        // and a continuation socket, and closes every other inheritable one,
        // whatever its origin. Nothing between the parse and the claim opens,
        // closes, or replaces a descriptor; the leaf's procfs reads are
        // close-on-exec and closed before its scan. The leaf also rejects a
        // second thread before it observes anything.
        let claimed = unsafe {
            splinterm_inherited_fd::claim_post_exec_inherited(&[resume_fd, continuation_fd])
        }
        .context("cannot claim the inherited resume descriptors")?;
        let closed_unexpected = claimed.closed_unexpected();
        let [resume, continuation] = <[OwnedFd; 2]>::try_from(claimed.into_descriptors())
            .map_err(|_| anyhow::anyhow!("the claim returned the wrong descriptor count"))?;
        let record = read_window_resume_record(resume.as_fd())
            .context("the inherited resume record is invalid")?;
        drop(resume);
        validate_continuation(&continuation)?;
        Ok(AcquiredWindowResume {
            record,
            continuation,
            closed_unexpected,
        })
    })()
    .with_context(|| {
        format!(
            "{WINDOW_RESUME_FD_ARGUMENT} is present, so splinterm started as a Window relaunched across a daemon upgrade and cannot restore it; reopen its Dojos from the launcher"
        )
    })
    .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(values: &[&str]) -> Result<Option<(RawFd, RawFd)>, ActivationError> {
        let values = values.iter().map(OsString::from).collect::<Vec<_>>();
        split_activation(&values)
    }

    fn resume(value: &str) -> String {
        format!("{WINDOW_RESUME_FD_ARGUMENT}={value}")
    }

    fn continuation(value: &str) -> String {
        format!("{CONTINUATION_FD_ARGUMENT}={value}")
    }

    #[test]
    fn absent_activation_selects_ordinary_startup() {
        assert_eq!(split(&[]), Ok(None));
        assert_eq!(split(&["dojos"]), Ok(None));
        assert_eq!(split(&["launch", "--name", "x"]), Ok(None));
    }

    #[test]
    fn exact_activation_yields_both_descriptors() {
        assert_eq!(split(&[&resume("3"), &continuation("4")]), Ok(Some((3, 4))));
        let max = RawFd::MAX.to_string();
        assert_eq!(
            split(&[&resume(&max), &continuation("1024")]),
            Ok(Some((RawFd::MAX, 1024)))
        );
    }

    #[test]
    fn malformed_activation_fails_closed() {
        assert_eq!(
            split(&[&continuation("4"), &resume("3")]),
            Err(ActivationError::Position)
        );
        assert_eq!(split(&[&resume("3")]), Err(ActivationError::Position));
        assert_eq!(split(&[&continuation("3")]), Err(ActivationError::Position));
        assert_eq!(
            split(&[&resume("3"), &continuation("4"), "dojos"]),
            Err(ActivationError::Position)
        );
        assert_eq!(
            split(&["dojos", &resume("3"), &continuation("4")]),
            Err(ActivationError::Position)
        );
        assert_eq!(
            split(&[&resume("3"), &resume("4")]),
            Err(ActivationError::Duplicated)
        );
        assert_eq!(
            split(&[&continuation("3"), &continuation("4")]),
            Err(ActivationError::Duplicated)
        );
        assert_eq!(
            split(&[&resume("3"), &continuation("3")]),
            Err(ActivationError::SameDescriptor)
        );
        for bad in ["", "-3", "+3", "03", "3x", "x", "99999999999999999999"] {
            let expected = if bad.is_empty() {
                ActivationError::MissingDescriptor
            } else {
                ActivationError::NoncanonicalDescriptor
            };
            assert_eq!(
                split(&[&resume(bad), &continuation("4")]),
                Err(expected),
                "{bad:?}"
            );
        }
        for standard in ["0", "1", "2"] {
            assert_eq!(
                split(&[&resume("5"), &continuation(standard)]),
                Err(ActivationError::StandardDescriptor)
            );
        }
        // A near miss or a future version is an activation attempt too.
        assert_eq!(
            split(&[
                "--splinterm-internal-window-resume-fd-v2=3",
                &continuation("4")
            ]),
            Err(ActivationError::Unrecognized)
        );
        assert_eq!(
            split(&[WINDOW_RESUME_FD_ARGUMENT, &continuation("4")]),
            Err(ActivationError::MissingDescriptor)
        );
    }

    #[test]
    fn only_a_unix_seqpacket_socket_of_this_user_is_a_continuation() {
        let (local, _peer) = rustix::net::socketpair(
            AddressFamily::UNIX,
            SocketType::SEQPACKET,
            rustix::net::SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        validate_continuation(&local).unwrap();
        let (stream, _stream_peer) = std::os::unix::net::UnixStream::pair().unwrap();
        assert!(validate_continuation(&OwnedFd::from(stream)).is_err());
        let datagram = rustix::net::socket(AddressFamily::UNIX, SocketType::DGRAM, None).unwrap();
        assert!(validate_continuation(&datagram).is_err());
        let file = std::fs::File::open("/dev/null").unwrap();
        assert!(validate_continuation(&OwnedFd::from(file)).is_err());
    }
}
