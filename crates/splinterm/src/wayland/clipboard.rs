//! Bounded clipboard policy, worker admission, and deadline-driven pipe I/O.

use std::{
    io,
    os::fd::{AsFd, OwnedFd},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::Sender as StdSender,
    },
    task::Waker,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use rustix::event::{PollFd, PollFlags, Timespec, poll};

use super::{
    DragOffer,
    file_drop::{FileDropTarget, MAX_DROP_BYTES},
};

pub(super) const TEXT_MIMES: [&str; 3] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"];
const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;
const MAX_CLIPBOARD_WORKERS: usize = 4;
pub(super) const CLIPBOARD_IO_TIMEOUT: Duration = Duration::from_secs(2);
pub(super) static ACTIVE_CLIPBOARD_WORKERS: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OwnedFieldTarget {
    CommandPalette,
    DojoPrompt,
    Search,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PasteTarget {
    Clipboard,
    Primary,
    OwnedField(OwnedFieldTarget),
    FileDrop(FileDropTarget),
}

pub(super) struct ClipboardRead {
    pub(super) target: PasteTarget,
    pub(super) input_generation: u64,
    pub(super) drag_offer: Option<DragOffer>,
    pub(super) bytes: io::Result<Vec<u8>>,
}

pub(super) struct ClipboardWorkerPermit<'a> {
    active: &'a AtomicUsize,
}

impl Drop for ClipboardWorkerPermit<'_> {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Encodes clipboard bytes for the terminal's current bracketed-paste mode.
///
/// Clipboard acquisition remains Phase 5 client work; this helper only defines
/// the PTY byte boundary used once a user-authorized paste exists.
#[must_use]
pub fn encode_bracketed_paste(bytes: &[u8], enabled: bool) -> Vec<u8> {
    if !enabled {
        return bytes.to_vec();
    }
    let mut encoded = Vec::with_capacity(bytes.len() + 12);
    encoded.extend_from_slice(b"\x1b[200~");
    encoded.extend_from_slice(bytes);
    encoded.extend_from_slice(b"\x1b[201~");
    encoded
}

pub(super) fn accepted_text_mime(mimes: &[String]) -> Option<String> {
    TEXT_MIMES.iter().find_map(|supported| {
        mimes
            .iter()
            .find(|mime| mime.as_str() == *supported)
            .cloned()
    })
}

pub(super) fn safe_paste(bytes: &[u8]) -> Result<&[u8]> {
    if bytes.len() > MAX_CLIPBOARD_BYTES {
        anyhow::bail!("clipboard offer exceeds the 1 MiB limit");
    }
    std::str::from_utf8(bytes).context("clipboard text is not UTF-8")?;
    if bytes
        .iter()
        .any(|byte| matches!(*byte, 0..=8 | 11..=12 | 14..=31 | 127))
    {
        anyhow::bail!("clipboard text contains unsafe control characters");
    }
    Ok(bytes)
}

pub(super) fn try_clipboard_worker(active: &AtomicUsize) -> Option<ClipboardWorkerPermit<'_>> {
    active
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            (count < MAX_CLIPBOARD_WORKERS).then_some(count + 1)
        })
        .ok()
        .map(|_| ClipboardWorkerPermit { active })
}

fn poll_timeout(deadline: Instant) -> Option<Timespec> {
    let remaining = deadline.checked_duration_since(Instant::now())?;
    Some(Timespec {
        tv_sec: i64::try_from(remaining.as_secs()).unwrap_or(i64::MAX),
        tv_nsec: i64::from(remaining.subsec_nanos()),
    })
}

fn wait_for_fd(fd: &impl AsFd, events: PollFlags, deadline: Instant) -> io::Result<bool> {
    let Some(timeout) = poll_timeout(deadline) else {
        return Ok(false);
    };
    let mut descriptor = [PollFd::new(fd, events)];
    let ready = poll(&mut descriptor, Some(&timeout)).map_err(io::Error::from)?;
    if ready == 0 {
        return Ok(false);
    }
    let returned = descriptor[0].revents();
    if returned.intersects(PollFlags::ERR | PollFlags::NVAL) {
        return Err(io::Error::other("clipboard pipe reported an I/O error"));
    }
    Ok(returned.intersects(events | PollFlags::HUP))
}

fn read_clipboard_with_deadline(
    fd: &OwnedFd,
    timeout: Duration,
    maximum_bytes: usize,
) -> io::Result<Vec<u8>> {
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        if !wait_for_fd(fd, PollFlags::IN, deadline)? {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "clipboard read timed out",
            ));
        }
        let remaining = maximum_bytes.saturating_add(1).saturating_sub(bytes.len());
        if remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "clipboard offer exceeds byte limit",
            ));
        }
        let chunk_len = remaining.min(chunk.len());
        let read = rustix::io::read(fd, &mut chunk[..chunk_len]).map_err(io::Error::from)?;
        if read == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.len() > maximum_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "clipboard offer exceeds byte limit",
            ));
        }
    }
}

pub(super) fn write_clipboard_with_deadline(
    fd: &OwnedFd,
    payload: &[u8],
    timeout: Duration,
) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    let mut written = 0;
    while written < payload.len() {
        if !wait_for_fd(fd, PollFlags::OUT, deadline)? {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "clipboard write timed out",
            ));
        }
        let end = (written + 4096).min(payload.len());
        let count = rustix::io::write(fd, &payload[written..end]).map_err(io::Error::from)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "clipboard pipe accepted no bytes",
            ));
        }
        written += count;
    }
    Ok(())
}

pub(super) fn spawn_clipboard_read(
    fd: OwnedFd,
    target: PasteTarget,
    input_generation: u64,
    drag_offer: Option<DragOffer>,
    tx: StdSender<ClipboardRead>,
    waker: Waker,
) {
    let Some(permit) = try_clipboard_worker(&ACTIVE_CLIPBOARD_WORKERS) else {
        let _ = tx.send(ClipboardRead {
            target,
            input_generation,
            drag_offer,
            bytes: Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "clipboard worker limit reached",
            )),
        });
        waker.wake();
        return;
    };
    std::thread::spawn(move || {
        let _permit = permit;
        let maximum_bytes = match target {
            PasteTarget::FileDrop(_) => MAX_DROP_BYTES,
            PasteTarget::Clipboard | PasteTarget::Primary | PasteTarget::OwnedField(_) => {
                MAX_CLIPBOARD_BYTES
            }
        };
        let bytes = read_clipboard_with_deadline(&fd, CLIPBOARD_IO_TIMEOUT, maximum_bytes);
        let _ = tx.send(ClipboardRead {
            target,
            input_generation,
            drag_offer,
            bytes,
        });
        waker.wake();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_policy_filters_mime_size_utf8_and_controls() {
        assert_eq!(
            accepted_text_mime(&[
                "image/png".to_owned(),
                "text/plain".to_owned(),
                "text/plain;charset=utf-8".to_owned(),
            ]),
            Some("text/plain;charset=utf-8".to_owned())
        );
        assert_eq!(accepted_text_mime(&["image/png".to_owned()]), None);
        assert_eq!(
            safe_paste(b"line one\nline two\t").expect("safe text"),
            b"line one\nline two\t"
        );
        assert!(safe_paste(b"unsafe\x1bsequence").is_err());
        assert!(safe_paste(&[0xff]).is_err());
        assert!(safe_paste(&vec![b'x'; MAX_CLIPBOARD_BYTES + 1]).is_err());
    }

    #[test]
    fn bracketed_paste_wraps_only_when_mode_is_enabled() {
        assert_eq!(encode_bracketed_paste(b"hello", false), b"hello");
        assert_eq!(
            encode_bracketed_paste(b"hello", true),
            b"\x1b[200~hello\x1b[201~"
        );
    }

    #[test]
    fn clipboard_worker_budget_is_strict_and_released() {
        let active = AtomicUsize::new(0);
        let permits: Vec<_> = (0..MAX_CLIPBOARD_WORKERS)
            .map(|_| try_clipboard_worker(&active).expect("worker slot"))
            .collect();
        assert!(try_clipboard_worker(&active).is_none());
        drop(permits);
        assert_eq!(active.load(Ordering::Acquire), 0);
        assert!(try_clipboard_worker(&active).is_some());
    }

    #[test]
    fn clipboard_read_deadline_expires_without_a_writer() {
        use std::os::unix::net::UnixStream;

        let (reader, _writer) = UnixStream::pair().expect("socket pair");
        let fd = OwnedFd::from(reader);
        let error =
            read_clipboard_with_deadline(&fd, Duration::from_millis(5), MAX_CLIPBOARD_BYTES)
                .expect_err("idle peer times out");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }
}
