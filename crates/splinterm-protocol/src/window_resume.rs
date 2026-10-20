//! The sealed resume record a Window carries across its own relaunch during a
//! daemon handoff (Plan 0061 M3.14/M3.15; ADR 0011, "Smooth local client
//! recovery").
//!
//! Before exec, a Window that received [`crate::ServerFrame::HandoffPrepare`]
//! writes one [`WindowResumeRecord`] into an anonymous memfd and seals it
//! ([`seal_window_resume_record`]) before it acknowledges the frame. When the
//! continuation reports the outcome, the Window replaces its own process
//! image: with the sealed forward client snapshot after an adoption, or with
//! the installed `<prefix>/splinterm` after a rollback. The new image finds
//! the record and the continuation socket through two argv flags
//! ([`WINDOW_RESUME_FD_ARGUMENT`] and [`CONTINUATION_FD_ARGUMENT`]), claims
//! both descriptors before any thread exists, reads the record back
//! ([`read_window_resume_record`]), and rebuilds the Window from the daemon's
//! authoritative topology.
//!
//! The record names topology only: which Dojos the Window showed, in tab
//! order, which tab was active, which Splint each tab focused, the daemon
//! generation and prepare id it answered, and the Window's application id.
//! It never carries terminal contents, input, clipboard, search, credentials,
//! environment, or consent state (ADR 0011).
//!
//! The record is one fixed little-endian layout, never native struct layout
//! or a self-describing encoding:
//!
//! | bytes            | field                                                   |
//! |------------------|---------------------------------------------------------|
//! | 0..4             | magic, ASCII `SPWR`                                     |
//! | 4                | version, [`WINDOW_RESUME_RECORD_VERSION`]               |
//! | 5                | tab count `n`, `1..=`[`MAX_WINDOW_RESUME_TABS`]         |
//! | 6                | active tab index, below `n`                             |
//! | 7                | application id length `a`, `0..=`[`MAX_WINDOW_RESUME_APP_ID_BYTES`] |
//! | 8..16            | `daemon_generation`, `u64`, nonzero                     |
//! | 16..24           | `prepare_id`, `u64`, nonzero                            |
//! | 24..24+48n       | `n` tabs: Lair id, Dojo id, focused Splint id (16 bytes each; an all-zero focus means none) |
//! | 24+48n..+a       | application id, ASCII `[A-Za-z0-9._-]`                  |
//!
//! [`WindowResumeRecord::decode`] refuses any other length, magic, version,
//! count, index, a zero generation or prepare id, a nil Lair or Dojo id, a
//! Dojo named twice, or an application id outside its alphabet. A new layout
//! takes a new version and new argv flag spellings.

use std::{
    io,
    os::fd::{AsFd, BorrowedFd, OwnedFd, RawFd},
    str::FromStr,
};

use rustix::{
    fs::{FileType, MemfdFlags, SealFlags, fcntl_add_seals, fcntl_get_seals, fstat, memfd_create},
    io::{Errno, pread, pwrite},
};
use splinterm_core::{DojoId, LairId, SplintId};
use thiserror::Error;
use uuid::Uuid;

/// The first four bytes of every resume record.
pub const WINDOW_RESUME_RECORD_MAGIC: [u8; 4] = *b"SPWR";
/// The layout version this crate writes and reads.
pub const WINDOW_RESUME_RECORD_VERSION: u8 = 1;
/// Most tabs one record names: a Window's own tab limit, and the Dojos a
/// handoff decision prompt may name for one Window.
pub const MAX_WINDOW_RESUME_TABS: usize = crate::MAX_HANDOFF_WINDOW_DOJOS;
/// Longest application id a record carries.
pub const MAX_WINDOW_RESUME_APP_ID_BYTES: usize = 255;
const HEADER_BYTES: usize = 24;
const TAB_BYTES: usize = 48;
/// The largest record: every tab and the longest application id.
pub const MAX_WINDOW_RESUME_RECORD_BYTES: usize =
    HEADER_BYTES + MAX_WINDOW_RESUME_TABS * TAB_BYTES + MAX_WINDOW_RESUME_APP_ID_BYTES;
const _: () = assert!(MAX_WINDOW_RESUME_TABS <= u8::MAX as usize);
const _: () = assert!(MAX_WINDOW_RESUME_RECORD_BYTES <= 2048);

/// Seals a resume record must carry: its bytes, size, and seal set can never
/// change again.
pub const REQUIRED_WINDOW_RESUME_SEALS: SealFlags = SealFlags::WRITE
    .union(SealFlags::GROW)
    .union(SealFlags::SHRINK)
    .union(SealFlags::SEAL);

/// The relaunched client's resume-record flag, spelled
/// `WINDOW_RESUME_FD_ARGUMENT=<fd>` as the first argument, where `<fd>` is the
/// canonical decimal number of the inherited sealed record.
pub const WINDOW_RESUME_FD_ARGUMENT: &str = "--splinterm-internal-window-resume-fd-v1";
/// The relaunched client's continuation flag, spelled
/// `CONTINUATION_FD_ARGUMENT=<fd>` as the second and last argument, where
/// `<fd>` is the canonical decimal number of the inherited continuation
/// socket.
pub const CONTINUATION_FD_ARGUMENT: &str = "--splinterm-internal-continuation-fd-v1";
/// Every argument starting with this is a resume-record activation attempt;
/// only the exact versioned spelling is valid.
pub const WINDOW_RESUME_FD_FAMILY: &str = "--splinterm-internal-window-resume-fd";
/// Every argument starting with this is a continuation activation attempt;
/// only the exact versioned spelling is valid.
pub const CONTINUATION_FD_FAMILY: &str = "--splinterm-internal-continuation-fd";

/// The two activation arguments that follow the program name, in order.
#[must_use]
pub fn window_resume_arguments(resume: RawFd, continuation: RawFd) -> [String; 2] {
    [
        format!("{WINDOW_RESUME_FD_ARGUMENT}={resume}"),
        format!("{CONTINUATION_FD_ARGUMENT}={continuation}"),
    ]
}

/// One tab of the Window, in tab order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowResumeTab {
    pub lair_id: LairId,
    pub dojo_id: DojoId,
    /// The Splint the tab focused, when the Window knew it.
    pub focus: Option<SplintId>,
}

/// One Window's resume record; see the module documentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowResumeRecord {
    /// Generation of the daemon that sent the prepare frame.
    pub daemon_generation: u64,
    /// The prepare frame's id, as the acknowledgement echoed it.
    pub prepare_id: u64,
    /// Index of the active tab in `tabs`.
    pub active: usize,
    /// The Window's tabs in order, at least one.
    pub tabs: Vec<WindowResumeTab>,
    /// The Window's application id when it was not the default.
    pub app_id: Option<String>,
}

/// Why bytes or a descriptor are not a resume record.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum WindowResumeError {
    #[error("window resume record is {0} bytes, not a valid record length")]
    Length(usize),
    #[error("window resume record has the wrong magic")]
    Magic,
    #[error("window resume record version {0} is not supported")]
    Version(u8),
    #[error("window resume record names {0} tabs")]
    TabCount(usize),
    #[error("window resume record's active tab is out of range")]
    ActiveTab,
    #[error("window resume record names daemon generation or prepare id zero")]
    Zero,
    #[error("window resume record names a nil Lair or Dojo id")]
    NilIdentity,
    #[error("window resume record names one Dojo twice")]
    DuplicateDojo,
    #[error("window resume record's application id is invalid")]
    AppId,
    #[error("window resume descriptor is not a bounded sealed regular file")]
    Descriptor,
    #[error("window resume descriptor cannot be read: {0}")]
    Io(Errno),
}

fn id_bytes(id: impl std::fmt::Display) -> [u8; 16] {
    // The identifiers are UUIDs whose canonical text is their only public
    // spelling; this crate owns neither type's constructor.
    Uuid::parse_str(&id.to_string())
        .map(Uuid::into_bytes)
        .expect("Splinterm identifiers are UUIDs")
}

fn id_from_bytes<T: FromStr>(bytes: &[u8]) -> Option<T> {
    let bytes: [u8; 16] = bytes.try_into().ok()?;
    T::from_str(&Uuid::from_bytes(bytes).to_string()).ok()
}

/// Whether `value` may be carried as an application id.
#[must_use]
pub fn valid_app_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_WINDOW_RESUME_APP_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

impl WindowResumeRecord {
    /// Checks every bound [`Self::decode`] enforces.
    ///
    /// # Errors
    /// The first bound the record breaks.
    pub fn validate(&self) -> Result<(), WindowResumeError> {
        if !(1..=MAX_WINDOW_RESUME_TABS).contains(&self.tabs.len()) {
            return Err(WindowResumeError::TabCount(self.tabs.len()));
        }
        if self.active >= self.tabs.len() {
            return Err(WindowResumeError::ActiveTab);
        }
        if self.daemon_generation == 0 || self.prepare_id == 0 {
            return Err(WindowResumeError::Zero);
        }
        let mut seen = std::collections::HashSet::with_capacity(self.tabs.len());
        for tab in &self.tabs {
            if id_bytes(tab.lair_id) == [0; 16] || id_bytes(tab.dojo_id) == [0; 16] {
                return Err(WindowResumeError::NilIdentity);
            }
            if !seen.insert(tab.dojo_id) {
                return Err(WindowResumeError::DuplicateDojo);
            }
        }
        if self
            .app_id
            .as_deref()
            .is_some_and(|app_id| !valid_app_id(app_id))
        {
            return Err(WindowResumeError::AppId);
        }
        Ok(())
    }

    /// The record's bytes.
    ///
    /// # Errors
    /// Refuses a record [`Self::validate`] refuses.
    pub fn encode(&self) -> Result<Vec<u8>, WindowResumeError> {
        self.validate()?;
        let app_id = self.app_id.as_deref().unwrap_or_default().as_bytes();
        let mut bytes =
            Vec::with_capacity(HEADER_BYTES + self.tabs.len() * TAB_BYTES + app_id.len());
        bytes.extend_from_slice(&WINDOW_RESUME_RECORD_MAGIC);
        bytes.push(WINDOW_RESUME_RECORD_VERSION);
        // Each bound below was checked by `validate`.
        bytes.push(
            u8::try_from(self.tabs.len())
                .map_err(|_| WindowResumeError::TabCount(self.tabs.len()))?,
        );
        bytes.push(u8::try_from(self.active).map_err(|_| WindowResumeError::ActiveTab)?);
        bytes.push(u8::try_from(app_id.len()).map_err(|_| WindowResumeError::AppId)?);
        bytes.extend_from_slice(&self.daemon_generation.to_le_bytes());
        bytes.extend_from_slice(&self.prepare_id.to_le_bytes());
        for tab in &self.tabs {
            bytes.extend_from_slice(&id_bytes(tab.lair_id));
            bytes.extend_from_slice(&id_bytes(tab.dojo_id));
            bytes.extend_from_slice(&tab.focus.map_or([0; 16], id_bytes));
        }
        bytes.extend_from_slice(app_id);
        Ok(bytes)
    }

    /// Decodes exactly one record.
    ///
    /// # Errors
    /// Refuses everything the module documentation lists.
    pub fn decode(bytes: &[u8]) -> Result<Self, WindowResumeError> {
        if bytes.len() < HEADER_BYTES || bytes.len() > MAX_WINDOW_RESUME_RECORD_BYTES {
            return Err(WindowResumeError::Length(bytes.len()));
        }
        if bytes[..4] != WINDOW_RESUME_RECORD_MAGIC {
            return Err(WindowResumeError::Magic);
        }
        if bytes[4] != WINDOW_RESUME_RECORD_VERSION {
            return Err(WindowResumeError::Version(bytes[4]));
        }
        let count = usize::from(bytes[5]);
        let active = usize::from(bytes[6]);
        let app_id_length = usize::from(bytes[7]);
        if !(1..=MAX_WINDOW_RESUME_TABS).contains(&count) {
            return Err(WindowResumeError::TabCount(count));
        }
        if bytes.len() != HEADER_BYTES + count * TAB_BYTES + app_id_length {
            return Err(WindowResumeError::Length(bytes.len()));
        }
        let word = |offset: usize| {
            let mut value = [0_u8; 8];
            value.copy_from_slice(&bytes[offset..offset + 8]);
            u64::from_le_bytes(value)
        };
        let mut tabs = Vec::with_capacity(count);
        for entry in bytes[HEADER_BYTES..HEADER_BYTES + count * TAB_BYTES].chunks_exact(TAB_BYTES) {
            if entry[..16] == [0; 16] || entry[16..32] == [0; 16] {
                return Err(WindowResumeError::NilIdentity);
            }
            let invalid = || WindowResumeError::NilIdentity;
            tabs.push(WindowResumeTab {
                lair_id: id_from_bytes(&entry[..16]).ok_or_else(invalid)?,
                dojo_id: id_from_bytes(&entry[16..32]).ok_or_else(invalid)?,
                focus: if entry[32..] == [0; 16] {
                    None
                } else {
                    Some(id_from_bytes(&entry[32..]).ok_or_else(invalid)?)
                },
            });
        }
        let app_id = if app_id_length == 0 {
            None
        } else {
            let raw = &bytes[HEADER_BYTES + count * TAB_BYTES..];
            let text = std::str::from_utf8(raw).map_err(|_| WindowResumeError::AppId)?;
            Some(text.to_owned())
        };
        let record = Self {
            daemon_generation: word(8),
            prepare_id: word(16),
            active,
            tabs,
            app_id,
        };
        record.validate()?;
        Ok(record)
    }
}

/// Writes `record` into a new close-on-exec memfd and seals it immutable.
///
/// # Errors
/// Refuses an invalid record, and reports any memfd failure.
pub fn seal_window_resume_record(record: &WindowResumeRecord) -> io::Result<OwnedFd> {
    let bytes = record
        .encode()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let memfd = memfd_create(
        "splinterm-window-resume",
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )?;
    let mut written = 0_usize;
    while written < bytes.len() {
        let offset = u64::try_from(written).map_err(io::Error::other)?;
        let count = pwrite(&memfd, &bytes[written..], offset)?;
        if count == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        written += count;
    }
    fcntl_add_seals(&memfd, REQUIRED_WINDOW_RESUME_SEALS)?;
    let sealed = fcntl_get_seals(&memfd)?;
    if u64::try_from(fstat(&memfd)?.st_size).ok() != u64::try_from(bytes.len()).ok()
        || !sealed.contains(REQUIRED_WINDOW_RESUME_SEALS)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "sealed window resume record does not hold exactly its encoding",
        ));
    }
    Ok(memfd)
}

/// Reads one record back from a sealed descriptor, without moving its offset.
///
/// # Errors
/// Refuses a descriptor that is not a nonempty sealed regular file within
/// [`MAX_WINDOW_RESUME_RECORD_BYTES`], a read failure, and an invalid record.
pub fn read_window_resume_record(
    descriptor: BorrowedFd<'_>,
) -> Result<WindowResumeRecord, WindowResumeError> {
    let stat = fstat(descriptor).map_err(WindowResumeError::Io)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(WindowResumeError::Descriptor);
    }
    let size = usize::try_from(stat.st_size)
        .ok()
        .filter(|size| (1..=MAX_WINDOW_RESUME_RECORD_BYTES).contains(size))
        .ok_or(WindowResumeError::Descriptor)?;
    let seals = match fcntl_get_seals(descriptor) {
        Ok(seals) => seals,
        // Files without sealing support report EINVAL.
        Err(Errno::INVAL) => return Err(WindowResumeError::Descriptor),
        Err(error) => return Err(WindowResumeError::Io(error)),
    };
    if !seals.contains(REQUIRED_WINDOW_RESUME_SEALS) {
        return Err(WindowResumeError::Descriptor);
    }
    let mut bytes = vec![0_u8; size];
    let mut read = 0_usize;
    while read < size {
        let offset = u64::try_from(read).map_err(|_| WindowResumeError::Descriptor)?;
        let count = pread(descriptor, &mut bytes[read..], offset).map_err(WindowResumeError::Io)?;
        if count == 0 {
            return Err(WindowResumeError::Descriptor);
        }
        read += count;
    }
    let end = u64::try_from(size).map_err(|_| WindowResumeError::Descriptor)?;
    if pread(descriptor, &mut [0_u8; 1], end).map_err(WindowResumeError::Io)? != 0 {
        return Err(WindowResumeError::Descriptor);
    }
    WindowResumeRecord::decode(&bytes)
}

/// Whether `descriptor` already carries every required seal; for tests and
/// diagnostics.
#[must_use]
pub fn is_sealed_window_resume(descriptor: impl AsFd) -> bool {
    fcntl_get_seals(descriptor).is_ok_and(|seals| seals.contains(REQUIRED_WINDOW_RESUME_SEALS))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(focus: bool) -> WindowResumeTab {
        WindowResumeTab {
            lair_id: LairId::new(),
            dojo_id: DojoId::new(),
            focus: focus.then(SplintId::new),
        }
    }

    fn record(tabs: usize) -> WindowResumeRecord {
        WindowResumeRecord {
            daemon_generation: 4,
            prepare_id: 9,
            active: tabs - 1,
            tabs: (0..tabs).map(|index| tab(index % 2 == 0)).collect(),
            app_id: Some("org.omarchy.screensaver".to_owned()),
        }
    }

    #[test]
    fn records_round_trip_at_every_bound() {
        for tabs in [1, 2, MAX_WINDOW_RESUME_TABS] {
            let original = record(tabs);
            let bytes = original.encode().unwrap();
            assert_eq!(
                bytes.len(),
                HEADER_BYTES + tabs * TAB_BYTES + "org.omarchy.screensaver".len()
            );
            assert_eq!(WindowResumeRecord::decode(&bytes).unwrap(), original);
        }
        let mut longest = record(MAX_WINDOW_RESUME_TABS);
        longest.app_id = Some("a".repeat(MAX_WINDOW_RESUME_APP_ID_BYTES));
        let bytes = longest.encode().unwrap();
        assert_eq!(bytes.len(), MAX_WINDOW_RESUME_RECORD_BYTES);
        assert_eq!(WindowResumeRecord::decode(&bytes).unwrap(), longest);
        let mut plain = record(3);
        plain.app_id = None;
        assert_eq!(
            WindowResumeRecord::decode(&plain.encode().unwrap()).unwrap(),
            plain
        );
    }

    #[test]
    fn the_wire_layout_is_fixed() {
        let lair = LairId::from_str("00000000-0000-4000-8000-000000000001").unwrap();
        let dojo = DojoId::from_str("00000000-0000-4000-8000-000000000002").unwrap();
        let record = WindowResumeRecord {
            daemon_generation: 0x0102,
            prepare_id: 7,
            active: 0,
            tabs: vec![WindowResumeTab {
                lair_id: lair,
                dojo_id: dojo,
                focus: None,
            }],
            app_id: Some("x".to_owned()),
        };
        let bytes = record.encode().unwrap();
        assert_eq!(&bytes[..8], &[b'S', b'P', b'W', b'R', 1, 1, 0, 1]);
        assert_eq!(&bytes[8..16], &[2, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&bytes[16..24], &[7, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(bytes[24 + 15], 1);
        assert_eq!(bytes[40 + 15], 2);
        assert_eq!(&bytes[56..72], &[0; 16]);
        assert_eq!(&bytes[72..], b"x");
    }

    #[test]
    fn malformed_records_are_refused() {
        let valid = record(2).encode().unwrap();
        let mutate = |index: usize, value: u8| {
            let mut bytes = valid.clone();
            bytes[index] = value;
            WindowResumeRecord::decode(&bytes)
        };
        assert_eq!(mutate(0, b'X'), Err(WindowResumeError::Magic));
        assert_eq!(mutate(4, 2), Err(WindowResumeError::Version(2)));
        assert_eq!(mutate(5, 0), Err(WindowResumeError::TabCount(0)));
        assert_eq!(mutate(5, 33), Err(WindowResumeError::TabCount(33)));
        // One more tab than the bytes hold is a length mismatch.
        assert!(matches!(mutate(5, 3), Err(WindowResumeError::Length(_))));
        assert_eq!(mutate(6, 2), Err(WindowResumeError::ActiveTab));
        assert!(matches!(mutate(7, 0), Err(WindowResumeError::Length(_))));
        let mut truncated = valid.clone();
        truncated.pop();
        assert!(matches!(
            WindowResumeRecord::decode(&truncated),
            Err(WindowResumeError::Length(_))
        ));
        let mut trailing = valid.clone();
        trailing.push(b'x');
        assert!(matches!(
            WindowResumeRecord::decode(&trailing),
            Err(WindowResumeError::Length(_))
        ));
        assert!(matches!(
            WindowResumeRecord::decode(&[]),
            Err(WindowResumeError::Length(0))
        ));
        let mut zero_generation = valid.clone();
        zero_generation[8..16].fill(0);
        assert_eq!(
            WindowResumeRecord::decode(&zero_generation),
            Err(WindowResumeError::Zero)
        );
        let mut zero_prepare = valid.clone();
        zero_prepare[16..24].fill(0);
        assert_eq!(
            WindowResumeRecord::decode(&zero_prepare),
            Err(WindowResumeError::Zero)
        );
        let mut nil_dojo = valid.clone();
        nil_dojo[40..56].fill(0);
        assert_eq!(
            WindowResumeRecord::decode(&nil_dojo),
            Err(WindowResumeError::NilIdentity)
        );
        let mut duplicate = valid.clone();
        let first_dojo = duplicate[40..56].to_vec();
        duplicate[88..104].copy_from_slice(&first_dojo);
        assert_eq!(
            WindowResumeRecord::decode(&duplicate),
            Err(WindowResumeError::DuplicateDojo)
        );
        let mut bad_app_id = valid.clone();
        let last = bad_app_id.len() - 1;
        bad_app_id[last] = b'/';
        assert_eq!(
            WindowResumeRecord::decode(&bad_app_id),
            Err(WindowResumeError::AppId)
        );
    }

    #[test]
    fn invalid_records_are_never_encoded() {
        let mut empty = record(1);
        empty.tabs.clear();
        assert_eq!(empty.encode(), Err(WindowResumeError::TabCount(0)));
        let mut many = record(1);
        many.tabs = (0..=MAX_WINDOW_RESUME_TABS).map(|_| tab(false)).collect();
        assert_eq!(
            many.encode(),
            Err(WindowResumeError::TabCount(MAX_WINDOW_RESUME_TABS + 1))
        );
        let mut active = record(2);
        active.active = 2;
        assert_eq!(active.encode(), Err(WindowResumeError::ActiveTab));
        let mut twice = record(2);
        twice.tabs[1].dojo_id = twice.tabs[0].dojo_id;
        assert_eq!(twice.encode(), Err(WindowResumeError::DuplicateDojo));
        let mut generation = record(1);
        generation.daemon_generation = 0;
        assert_eq!(generation.encode(), Err(WindowResumeError::Zero));
        for app_id in ["", "with space", "slash/id", &"a".repeat(256)] {
            let mut bad = record(1);
            bad.app_id = Some(app_id.to_owned());
            assert_eq!(bad.encode(), Err(WindowResumeError::AppId), "{app_id:?}");
        }
    }

    #[test]
    fn sealed_records_read_back_and_unsealed_or_foreign_descriptors_are_refused() {
        let original = record(3);
        let sealed = seal_window_resume_record(&original).unwrap();
        assert!(is_sealed_window_resume(&sealed));
        assert_eq!(read_window_resume_record(sealed.as_fd()).unwrap(), original);
        assert!(
            rustix::io::fcntl_getfd(&sealed)
                .unwrap()
                .contains(rustix::io::FdFlags::CLOEXEC)
        );
        // The record is immutable.
        assert!(rustix::io::write(&sealed, b"x").is_err());

        let unsealed =
            memfd_create("unsealed", MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING).unwrap();
        let bytes = original.encode().unwrap();
        assert_eq!(pwrite(&unsealed, &bytes, 0).unwrap(), bytes.len());
        assert_eq!(
            read_window_resume_record(unsealed.as_fd()),
            Err(WindowResumeError::Descriptor)
        );
        let empty = memfd_create("empty", MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING).unwrap();
        fcntl_add_seals(&empty, REQUIRED_WINDOW_RESUME_SEALS).unwrap();
        assert_eq!(
            read_window_resume_record(empty.as_fd()),
            Err(WindowResumeError::Descriptor)
        );
        let oversized =
            memfd_create("oversized", MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING).unwrap();
        let big = vec![0_u8; MAX_WINDOW_RESUME_RECORD_BYTES + 1];
        assert_eq!(pwrite(&oversized, &big, 0).unwrap(), big.len());
        fcntl_add_seals(&oversized, REQUIRED_WINDOW_RESUME_SEALS).unwrap();
        assert_eq!(
            read_window_resume_record(oversized.as_fd()),
            Err(WindowResumeError::Descriptor)
        );
        let garbage =
            memfd_create("garbage", MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING).unwrap();
        assert_eq!(
            pwrite(&garbage, b"not a record at all, 24+", 0).unwrap(),
            24
        );
        fcntl_add_seals(&garbage, REQUIRED_WINDOW_RESUME_SEALS).unwrap();
        assert_eq!(
            read_window_resume_record(garbage.as_fd()),
            Err(WindowResumeError::Magic)
        );
        let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        assert_eq!(
            read_window_resume_record(socket.as_fd()),
            Err(WindowResumeError::Descriptor)
        );
        let (reader, _writer) = io::pipe().unwrap();
        assert_eq!(
            read_window_resume_record(reader.as_fd()),
            Err(WindowResumeError::Descriptor)
        );
    }

    #[test]
    fn activation_arguments_have_one_exact_spelling() {
        assert_eq!(
            window_resume_arguments(5, 9),
            [
                "--splinterm-internal-window-resume-fd-v1=5".to_owned(),
                "--splinterm-internal-continuation-fd-v1=9".to_owned(),
            ]
        );
        assert!(WINDOW_RESUME_FD_ARGUMENT.starts_with(WINDOW_RESUME_FD_FAMILY));
        assert!(CONTINUATION_FD_ARGUMENT.starts_with(CONTINUATION_FD_FAMILY));
    }
}
