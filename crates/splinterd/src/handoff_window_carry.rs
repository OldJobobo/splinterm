//! Trusted local Windows carried across a daemon handoff exec (Plan 0061
//! M3.13b2 and M3.13c; ADR 0011, "Smooth local client recovery").
//!
//! The old generation records each participating Window in both sealed
//! manifests ([`crate::handoff_manifest::ManifestWindow`]) and inherits two
//! descriptors for it across exec: its anchor's pidfd at
//! `ClientProcess(index)` and the daemon end of its continuation channel at
//! `Continuation(index)`. This module holds what the adopting side does
//! with them:
//!
//! - **Staging** ([`inspect_window_process`],
//!   [`require_continuation_endpoint`]): the pidfd must be a pidfd whose
//!   fdinfo `Pid:` is the recorded PID and whose process still has the
//!   recorded start time; the continuation end must be an `AF_UNIX`
//!   `SOCK_SEQPACKET` socket with `SO_PASSCRED` set. A wrong type, or a
//!   live process that is not the recorded one, fails staging, and the
//!   candidate rolls back. A process that has exited is reported as such:
//!   staging drops that Window logically and never rolls back for it.
//! - **After commit** ([`announce_and_hold`]): every live Window is sent
//!   one [`HandoffOutcome`] on its continuation channel, `Adopted` from the
//!   forward daemon and `RolledBack` from the rollback daemon, and is then
//!   held, with the controllers its old connections held at the fence as
//!   [`ControllerPlaceholder`]s, for at most [`HANDOFF_WINDOW_RESUME_HOLD`].
//!   Expiry closes its descriptors and forgets its placeholders. Nothing
//!   here grants a controller: a valid resume claim (Plan 0061 M3.16) is
//!   the only way a placeholder becomes a lease.
//!
//! An exited Window's descriptors stay owned until commit, because the
//! sealed rollback manifest still names them and rollback requires every
//! recorded owner at its original number; commit closes them.

use std::{
    fmt, fs, io,
    os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fs::{FileType, Stat},
    net::{AddressFamily, SendFlags, SocketType, send, sockopt},
};
use splinterm_core::SplintId;
use splinterm_protocol::handoff_outcome::HandoffOutcome;
use tracing::{info, warn};

use crate::{
    candidate_bootstrap::CandidateWindowStage,
    handoff_manifest::{ControllerDisposition, ManifestSplint, ManifestWindow},
};

/// How long an adopting daemon holds each carried Window, its pidfd and
/// continuation end and the controllers its old connections held, for the
/// replacement client's resume claim (Plan 0061 M3.16). The Window has to
/// relaunch, reconnect, and resnapshot within it; after it, the claim is
/// gone and the panes stay view-only until control is requested again.
pub const HANDOFF_WINDOW_RESUME_HOLD: Duration = Duration::from_secs(30);

/// What a pidfd's `/proc/self/fd` link reads. Kernels since 6.9 back pidfds
/// with pidfs and still report this name for compatibility.
const PIDFD_LINK: &str = "anon_inode:[pidfd]";

/// The PID a pidfd's fdinfo names: the process in this PID namespace, `-1`
/// once it has been reaped, or `0` when it is not visible here. `None` when
/// the text has no well-formed `Pid:` line, which no pidfd omits.
#[must_use]
pub(crate) fn fdinfo_pid(fdinfo: &str) -> Option<i64> {
    fdinfo
        .lines()
        .find_map(|line| line.strip_prefix("Pid:"))
        .and_then(|value| value.trim().parse().ok())
}

/// The start time, in clock ticks after boot, field 22 of
/// `/proc/<pid>/stat`. The command name may hold any bytes, including `)`
/// and invalid UTF-8, so the fields are read after its final `)`.
///
/// The daemon's one start-time parser: the old generation pins a Window
/// (and verifies every peer) with it, and a candidate checks an inherited
/// Window pidfd's process with it, so both sides read the same field.
#[must_use]
pub fn stat_start_time(stat: &[u8]) -> Option<u64> {
    let close = stat.iter().rposition(|&byte| byte == b')')?;
    let rest = std::str::from_utf8(&stat[close + 1..]).ok()?;
    // Field 3 (state) is the first after the name; starttime is the 20th
    // after it.
    rest.split_ascii_whitespace().nth(19)?.parse().ok()
}

/// The start time of `pid` ([`stat_start_time`] of its `/proc/<pid>/stat`),
/// or `None` when it cannot be read.
#[must_use]
pub fn process_start_time(pid: u32) -> Option<u64> {
    stat_start_time(&fs::read(format!("/proc/{pid}/stat")).ok()?)
}

/// Whether a pidfd reports its process exited: it becomes readable once
/// the process has exited, reaped or not.
fn pidfd_exited(pidfd: BorrowedFd<'_>) -> io::Result<bool> {
    let mut descriptors = [PollFd::new(&pidfd, PollFlags::IN)];
    let immediately = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let ready = poll(&mut descriptors, Some(&immediately))?;
    Ok(ready != 0)
}

/// What staging learned about a carried Window's process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WindowProcessState {
    /// The recorded process is alive, with its recorded start time.
    Live,
    /// The recorded process has exited; the Window is dropped logically.
    Exited,
}

/// Why a `ClientProcess` descriptor fails staging.
#[derive(Debug)]
pub(crate) enum WindowProcessRefusal {
    /// The descriptor is not a pidfd.
    NotPidfd,
    /// A live process that is not the recorded one: another PID, or the
    /// recorded PID with another start time.
    Identity,
    /// The descriptor or the process could not be inspected.
    Inspect(io::Error),
}

/// Checks one inherited `ClientProcess` descriptor against its Window
/// record, without changing anything.
///
/// The descriptor must be a pidfd: its `/proc/self/fd` link names one and
/// its fdinfo has a `Pid:` line. A `Pid:` of `-1` (reaped) or `0` (no
/// longer visible) is an exited process. Any other PID must be the
/// recorded one. Its start time is read between that and a second pidfd
/// check: a pidfd still alive after the read proves the read described the
/// pinned process, not a later one reusing its PID, so a start time that
/// then differs is an identity mismatch, never an exit. A pidfd readable at
/// that point means the process exited (a zombie included), whatever the
/// read returned.
///
/// # Errors
/// [`WindowProcessRefusal::NotPidfd`] for any other descriptor,
/// [`WindowProcessRefusal::Identity`] for a live process that is not the
/// recorded one, and [`WindowProcessRefusal::Inspect`] when procfs or the
/// pidfd cannot be read, which fails closed.
pub(crate) fn inspect_window_process(
    pidfd: BorrowedFd<'_>,
    record: &ManifestWindow,
) -> Result<WindowProcessState, WindowProcessRefusal> {
    let raw = pidfd.as_raw_fd();
    let link =
        fs::read_link(format!("/proc/self/fd/{raw}")).map_err(WindowProcessRefusal::Inspect)?;
    if link.as_os_str() != PIDFD_LINK {
        return Err(WindowProcessRefusal::NotPidfd);
    }
    let fdinfo = fs::read_to_string(format!("/proc/self/fdinfo/{raw}"))
        .map_err(WindowProcessRefusal::Inspect)?;
    let pid = fdinfo_pid(&fdinfo).ok_or(WindowProcessRefusal::NotPidfd)?;
    if pid == -1 || pid == 0 {
        return Ok(WindowProcessState::Exited);
    }
    if pid != i64::from(record.pid) {
        return Err(WindowProcessRefusal::Identity);
    }
    let start_time = process_start_time(record.pid);
    if pidfd_exited(pidfd).map_err(WindowProcessRefusal::Inspect)? {
        return Ok(WindowProcessState::Exited);
    }
    match start_time {
        Some(start_time) if start_time == record.start_time => Ok(WindowProcessState::Live),
        Some(_) => Err(WindowProcessRefusal::Identity),
        None => Err(WindowProcessRefusal::Inspect(io::Error::new(
            io::ErrorKind::NotFound,
            "a live Window process has no readable start time",
        ))),
    }
}

/// Why a `Continuation` descriptor fails staging.
#[derive(Debug)]
pub(crate) enum ContinuationRefusal {
    /// Not an `AF_UNIX` `SOCK_SEQPACKET` socket with `SO_PASSCRED` set.
    Role,
    Inspect(io::Error),
}

/// Requires an inherited `Continuation` descriptor to be the daemon end of
/// a continuation channel: an `AF_UNIX` `SOCK_SEQPACKET` socket with
/// `SO_PASSCRED` set, so every message the Window sends carries kernel
/// credentials (ADR 0011).
///
/// # Errors
/// [`ContinuationRefusal::Role`] for any other object, and
/// [`ContinuationRefusal::Inspect`] when the socket cannot be queried.
pub(crate) fn require_continuation_endpoint(
    descriptor: BorrowedFd<'_>,
    stat: &Stat,
) -> Result<(), ContinuationRefusal> {
    if FileType::from_raw_mode(stat.st_mode) != FileType::Socket {
        return Err(ContinuationRefusal::Role);
    }
    let inspect = |error: rustix::io::Errno| ContinuationRefusal::Inspect(error.into());
    let domain = sockopt::socket_domain(descriptor).map_err(inspect)?;
    let socket_type = sockopt::socket_type(descriptor).map_err(inspect)?;
    // `SO_PASSCRED` is a socket-level option, but only asked of the one
    // family whose answer means something here.
    if domain != AddressFamily::UNIX || socket_type != SocketType::SEQPACKET {
        return Err(ContinuationRefusal::Role);
    }
    if !sockopt::socket_passcred(descriptor).map_err(inspect)? {
        return Err(ContinuationRefusal::Role);
    }
    Ok(())
}

/// A Splint whose controller the Window `window`'s old connection
/// `connection` held at the fence (`ControllerDisposition::HeldBy`). It
/// grants nothing: only a valid resume claim from that Window
/// (`ClaimHandoffControl`, Plan 0061 M3.16) may restore it, and the lease
/// stays released until then.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControllerPlaceholder {
    pub splint_id: SplintId,
    pub incarnation: u64,
    pub window: u8,
    pub connection: u64,
}

/// One carried Window an adopted daemon holds after its outcome was sent.
pub struct HeldHandoffWindow {
    record: ManifestWindow,
    process: OwnedFd,
    continuation: OwnedFd,
}

impl HeldHandoffWindow {
    /// The Window's dense manifest index.
    #[must_use]
    pub const fn index(&self) -> u8 {
        self.record.index
    }

    /// The PID of the Window's pinned process.
    #[must_use]
    pub const fn pid(&self) -> u32 {
        self.record.pid
    }

    /// The start time the pidfd's process was recorded with.
    #[must_use]
    pub const fn start_time(&self) -> u64 {
        self.record.start_time
    }

    /// Device and inode of the executable the process ran when pinned.
    #[must_use]
    pub const fn client_executable(&self) -> (u64, u64) {
        (self.record.client_device, self.record.client_inode)
    }

    /// The Window's old connection IDs, ascending.
    #[must_use]
    pub fn old_connections(&self) -> &[u64] {
        &self.record.old_connections
    }

    /// The pidfd pinning the Window's process.
    #[must_use]
    pub fn process(&self) -> BorrowedFd<'_> {
        self.process.as_fd()
    }

    /// The daemon end of the Window's continuation channel.
    #[must_use]
    pub fn continuation(&self) -> BorrowedFd<'_> {
        self.continuation.as_fd()
    }
}

impl fmt::Debug for HeldHandoffWindow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HeldHandoffWindow")
            .field("index", &self.record.index)
            .field("pid", &self.record.pid)
            .finish_non_exhaustive()
    }
}

/// Every Window an adopted daemon holds for resume claims, and the
/// controllers they may restore.
#[derive(Debug)]
pub struct HeldHandoffWindows {
    windows: Vec<HeldHandoffWindow>,
    placeholders: Vec<ControllerPlaceholder>,
    expires_at: Instant,
}

impl HeldHandoffWindows {
    #[must_use]
    pub fn windows(&self) -> &[HeldHandoffWindow] {
        &self.windows
    }

    #[must_use]
    pub fn placeholders(&self) -> &[ControllerPlaceholder] {
        &self.placeholders
    }

    /// When the hold ends and every held Window is closed.
    #[must_use]
    pub const fn expires_at(&self) -> Instant {
        self.expires_at
    }
}

/// The held Windows, shared with the task that ends the hold at
/// [`HeldHandoffWindows::expires_at`]. Empty once it expired, or when no
/// Window was held at all.
#[derive(Clone, Default)]
pub struct HandoffWindowHold {
    held: Arc<Mutex<Option<HeldHandoffWindows>>>,
}

impl HandoffWindowHold {
    /// Whether any Window is still held.
    #[must_use]
    pub fn is_held(&self) -> bool {
        self.held.lock().is_ok_and(|held| held.is_some())
    }

    /// How many Windows are still held.
    #[must_use]
    pub fn window_count(&self) -> usize {
        self.held
            .lock()
            .ok()
            .and_then(|held| held.as_ref().map(|held| held.windows.len()))
            .unwrap_or(0)
    }

    /// The placeholders still held.
    #[must_use]
    pub fn placeholders(&self) -> Vec<ControllerPlaceholder> {
        self.held
            .lock()
            .ok()
            .and_then(|held| held.as_ref().map(|held| held.placeholders.clone()))
            .unwrap_or_default()
    }

    /// Ends the hold now: closes every held Window's descriptors and
    /// forgets every placeholder. Expiry calls it, and a claim (M3.16) takes
    /// what it needs first.
    pub fn release(&self) {
        let released = self.held.lock().ok().and_then(|mut held| held.take());
        if let Some(released) = released {
            info!(
                windows = released.windows.len(),
                placeholders = released.placeholders.len(),
                "daemon handoff: the carried Windows' resume hold ended"
            );
        }
    }
}

impl fmt::Debug for HandoffWindowHold {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HandoffWindowHold")
            .field("windows", &self.window_count())
            .finish_non_exhaustive()
    }
}

/// What the adopting daemon did with its carried Windows at commit.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WindowAnnouncement {
    /// Windows sent their outcome and held.
    pub announced: usize,
    /// Windows whose process had exited by staging or by commit, or that a
    /// rollback generation refused at staging, sent nothing.
    pub exited: usize,
    /// Windows whose outcome could not be sent, dropped.
    pub unsent: usize,
    /// Controller placeholders held for the announced Windows.
    pub placeholders: usize,
}

/// Sends `outcome` once on the continuation channel, without blocking and
/// without `SIGPIPE`.
fn send_outcome(continuation: BorrowedFd<'_>, outcome: HandoffOutcome) -> io::Result<()> {
    let packet = outcome.encode();
    let sent = send(
        continuation,
        &packet,
        SendFlags::NOSIGNAL | SendFlags::DONTWAIT,
    )?;
    if sent == packet.len() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "the handoff outcome packet was not sent whole",
        ))
    }
}

/// Runs right after an adoption commit: closes every Window staging found
/// exited, sends every live one `outcome`, drops one whose process exited
/// since or whose send fails, and holds the rest, with the controllers
/// their old connections held (`HeldBy` in `splints`), until `duration` passes.
///
/// It cannot fail the committed adoption: a Window it cannot announce is
/// dropped, which closes its descriptors, and the Window reads end of file.
/// The expiry runs on the current Tokio runtime; without one, nothing could
/// end a hold, so no Window is sent an outcome or held.
pub(crate) fn announce_and_hold(
    windows: Vec<CandidateWindowStage>,
    splints: &[ManifestSplint],
    outcome: HandoffOutcome,
    duration: Duration,
) -> (HandoffWindowHold, WindowAnnouncement) {
    let mut announcement = WindowAnnouncement::default();
    let shared = HandoffWindowHold::default();
    let runtime = tokio::runtime::Handle::try_current().ok();
    if runtime.is_none() && windows.iter().any(|window| window.live) {
        warn!("daemon handoff: no runtime can end a Window hold; closing the carried Windows");
        for window in windows {
            if window.live {
                announcement.unsent += 1;
            } else {
                announcement.exited += 1;
            }
        }
        return (shared, announcement);
    }
    let mut kept = Vec::new();
    for window in windows {
        let CandidateWindowStage {
            record,
            process,
            continuation,
            live,
        } = window;
        let exited = !live || pidfd_exited(process.as_fd()).unwrap_or(true);
        if exited {
            announcement.exited += 1;
            continue;
        }
        if let Err(error) = send_outcome(continuation.as_fd(), outcome) {
            warn!(
                %error,
                index = record.index,
                pid = record.pid,
                "daemon handoff: cannot send a carried Window its outcome; dropping it"
            );
            announcement.unsent += 1;
            continue;
        }
        kept.push(HeldHandoffWindow {
            record,
            process,
            continuation,
        });
    }
    announcement.announced = kept.len();
    let placeholders = splints
        .iter()
        .filter_map(|splint| match splint.controller {
            ControllerDisposition::HeldBy { window, connection }
                if kept.iter().any(|held| {
                    held.record.index == window && held.record.old_connections.contains(&connection)
                }) =>
            {
                Some(ControllerPlaceholder {
                    splint_id: splint.splint_id,
                    incarnation: splint.incarnation,
                    window,
                    connection,
                })
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    announcement.placeholders = placeholders.len();
    let Some(runtime) = runtime.filter(|_| !kept.is_empty()) else {
        return (shared, announcement);
    };
    let expires_at = Instant::now() + duration;
    if let Ok(mut slot) = shared.held.lock() {
        *slot = Some(HeldHandoffWindows {
            windows: kept,
            placeholders,
            expires_at,
        });
    }
    let expiring = shared.clone();
    drop(runtime.spawn(async move {
        tokio::time::sleep_until(tokio::time::Instant::from_std(expires_at)).await;
        expiring.release();
    }));
    (shared, announcement)
}

/// Asserts that the continuation channel's client end reads end of file,
/// with no packet before it.
///
/// It waits up to 5 s for the channel to become readable first. A sibling
/// test's `Command::spawn` duplicates this process's descriptor table until
/// its child executes, so the daemon end can stay open for a moment after
/// it was dropped; an immediate read would then see an empty channel, not
/// end of file.
#[cfg(test)]
pub(crate) fn assert_end_of_file(client: &OwnedFd) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut descriptors = [PollFd::new(client, PollFlags::IN | PollFlags::HUP)];
        let left = deadline.saturating_duration_since(Instant::now());
        let timeout = Timespec {
            tv_sec: i64::try_from(left.as_secs()).unwrap_or(5),
            tv_nsec: i64::from(left.subsec_nanos()),
        };
        match poll(&mut descriptors, Some(&timeout)) {
            Ok(0) => panic!("the continuation channel never reached end of file"),
            Ok(_) => break,
            Err(rustix::io::Errno::INTR) => {}
            Err(error) => panic!("cannot poll the continuation channel: {error}"),
        }
    }
    let mut buffer = [0_u8; 64];
    let (read, _) =
        rustix::net::recv(client, &mut buffer, rustix::net::RecvFlags::DONTWAIT).unwrap();
    assert_eq!(read, 0, "a packet arrived where end of file was expected");
}

#[cfg(test)]
mod tests {
    use std::{
        os::unix::net::UnixDatagram,
        process::{Child, Command},
    };

    use rustix::{
        net::{RecvFlags, SocketFlags, recv, socketpair},
        process::{Pid, PidfdFlags, pidfd_open},
    };
    use splinterm_protocol::handoff_outcome::{HANDOFF_OUTCOME_BYTES, HandoffOutcomeKind};

    use super::*;

    #[test]
    fn fdinfo_pid_reads_the_pid_line_only() {
        let fdinfo =
            "pos:\t0\nflags:\t02000002\nmnt_id:\t15\nino:\t1057\nPid:\t4242\nNSpid:\t4242\n";
        assert_eq!(fdinfo_pid(fdinfo), Some(4242));
        assert_eq!(fdinfo_pid("Pid:\t-1\nNSpid:\t-1\n"), Some(-1));
        assert_eq!(fdinfo_pid("Pid:\t0\n"), Some(0));
        // NSpid alone, a missing line, and junk are not a pidfd's PID.
        assert_eq!(fdinfo_pid("pos:\t0\nNSpid:\t4242\n"), None);
        assert_eq!(fdinfo_pid("pos:\t0\nflags:\t02\n"), None);
        assert_eq!(fdinfo_pid("Pid:\tfour\n"), None);
        assert_eq!(fdinfo_pid(""), None);
    }

    #[test]
    fn stat_start_time_reads_field_22_after_the_last_parenthesis() {
        let fields = (4..=21).map(|field| field.to_string()).collect::<Vec<_>>();
        let tail = format!("S {} 987654 23 24", fields.join(" "));
        assert_eq!(
            stat_start_time(format!("123 (sh) {tail}").as_bytes()),
            Some(987_654)
        );
        // A name holding `)`, spaces, and invalid UTF-8.
        let mut tricky = b"77 (a) b\xff (c) ".to_vec();
        tricky.extend_from_slice(tail.as_bytes());
        assert_eq!(stat_start_time(&tricky), Some(987_654));
        assert_eq!(stat_start_time(b"77 (sh) S 1 2 3"), None);
        assert_eq!(stat_start_time(b"no parenthesis"), None);
        // This process's own start time is readable and stable.
        let own = process_start_time(std::process::id()).unwrap();
        assert_eq!(process_start_time(std::process::id()), Some(own));
    }

    struct Window {
        child: Child,
        record: ManifestWindow,
        pidfd: OwnedFd,
    }

    impl Window {
        fn spawn() -> Self {
            let child = Command::new("sleep").arg("60").spawn().unwrap();
            let pid = child.id();
            let pidfd = pidfd_open(
                Pid::from_raw(i32::try_from(pid).unwrap()).unwrap(),
                PidfdFlags::empty(),
            )
            .unwrap();
            let record = ManifestWindow {
                index: 0,
                pid,
                start_time: process_start_time(pid).unwrap(),
                client_device: 1,
                client_inode: 2,
                old_connections: vec![7],
            };
            Self {
                child,
                record,
                pidfd,
            }
        }
    }

    impl Drop for Window {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    #[test]
    fn a_live_recorded_process_is_live_and_any_other_identity_is_refused() {
        let window = Window::spawn();
        assert_eq!(
            inspect_window_process(window.pidfd.as_fd(), &window.record).unwrap(),
            WindowProcessState::Live
        );
        let mut other_pid = window.record.clone();
        other_pid.pid = std::process::id();
        assert!(matches!(
            inspect_window_process(window.pidfd.as_fd(), &other_pid),
            Err(WindowProcessRefusal::Identity)
        ));
        let mut other_start = window.record.clone();
        other_start.start_time += 1;
        assert!(matches!(
            inspect_window_process(window.pidfd.as_fd(), &other_start),
            Err(WindowProcessRefusal::Identity)
        ));
    }

    #[test]
    fn an_exited_process_is_exited_whether_or_not_it_was_reaped() {
        let mut window = Window::spawn();
        window.child.kill().unwrap();
        // A zombie: the pidfd is readable before the reap.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !pidfd_exited(window.pidfd.as_fd()).unwrap() {
            assert!(Instant::now() < deadline, "the killed child never exited");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            inspect_window_process(window.pidfd.as_fd(), &window.record).unwrap(),
            WindowProcessState::Exited
        );
        window.child.wait().unwrap();
        // Reaped: the fdinfo names no process. Even a record naming another
        // PID is an exit then, not a mismatch.
        let fdinfo =
            fs::read_to_string(format!("/proc/self/fdinfo/{}", window.pidfd.as_raw_fd())).unwrap();
        assert!(matches!(fdinfo_pid(&fdinfo), Some(-1 | 0)));
        let mut other = window.record.clone();
        other.pid = std::process::id();
        assert_eq!(
            inspect_window_process(window.pidfd.as_fd(), &other).unwrap(),
            WindowProcessState::Exited
        );
    }

    #[test]
    fn only_a_pidfd_is_a_window_process() {
        let window = Window::spawn();
        let memfd =
            rustix::fs::memfd_create("not-a-pidfd", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
        let (socket, _peer) = UnixDatagram::pair().unwrap();
        for descriptor in [memfd.as_fd(), socket.as_fd()] {
            assert!(matches!(
                inspect_window_process(descriptor, &window.record),
                Err(WindowProcessRefusal::NotPidfd)
            ));
        }
    }

    fn seqpacket_pair(passcred: bool) -> (OwnedFd, OwnedFd) {
        let (daemon, client) = socketpair(
            AddressFamily::UNIX,
            SocketType::SEQPACKET,
            SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        sockopt::set_socket_passcred(&daemon, passcred).unwrap();
        (daemon, client)
    }

    #[test]
    fn a_continuation_end_is_a_credentialed_unix_seqpacket_socket() {
        let check = |descriptor: BorrowedFd<'_>| {
            let stat = rustix::fs::fstat(descriptor).unwrap();
            require_continuation_endpoint(descriptor, &stat)
        };
        let (daemon, _client) = seqpacket_pair(true);
        check(daemon.as_fd()).unwrap();
        let (uncredentialed, _client) = seqpacket_pair(false);
        let (stream, _peer) = socketpair(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        sockopt::set_socket_passcred(&stream, true).unwrap();
        let (datagram, _peer) = socketpair(
            AddressFamily::UNIX,
            SocketType::DGRAM,
            SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        sockopt::set_socket_passcred(&datagram, true).unwrap();
        let inet = rustix::net::socket(AddressFamily::INET, SocketType::DGRAM, None).unwrap();
        let memfd =
            rustix::fs::memfd_create("not-a-socket", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
        for descriptor in [
            uncredentialed.as_fd(),
            stream.as_fd(),
            datagram.as_fd(),
            inet.as_fd(),
            memfd.as_fd(),
        ] {
            assert!(matches!(check(descriptor), Err(ContinuationRefusal::Role)));
        }
    }

    fn read_packet(client: &OwnedFd) -> Option<Vec<u8>> {
        let mut buffer = [0_u8; 64];
        let (read, _) = recv(client, &mut buffer, RecvFlags::DONTWAIT).ok()?;
        Some(buffer[..read].to_vec())
    }

    fn stage(
        window: &Window,
        index: u8,
        connections: &[u64],
        live: bool,
    ) -> (CandidateWindowStage, OwnedFd) {
        let (daemon, client) = seqpacket_pair(true);
        let mut record = window.record.clone();
        record.index = index;
        record.old_connections = connections.to_vec();
        (
            CandidateWindowStage {
                record,
                process: window.pidfd.try_clone().unwrap(),
                continuation: daemon,
                live,
            },
            client,
        )
    }

    fn held_splint(window: u8, connection: u64) -> ManifestSplint {
        use crate::handoff_manifest::{
            BodyDisposition, ExitDisposition, ManifestPtyIdentity, ManifestPtySize,
            SubscriptionDisposition,
        };
        ManifestSplint {
            lair_id: splinterm_core::LairId::new(),
            dojo_id: splinterm_core::DojoId::new(),
            splint_id: SplintId::new(),
            incarnation: 4,
            pty: ManifestPtyIdentity {
                child_pid: 42,
                process_group: 42,
                session_id: 42,
                master_slot: 0,
            },
            size: ManifestPtySize {
                columns: 80,
                rows: 24,
                pixel_width: 0,
                pixel_height: 0,
            },
            foreground_process_group: Some(42),
            terminal_revision: 1,
            history_generation: 1,
            exit: ExitDisposition::Running,
            terminal_checkpoint: BodyDisposition::Descriptor(0),
            image_state: BodyDisposition::NotPresent,
            controller: ControllerDisposition::HeldBy { window, connection },
            subscriptions: SubscriptionDisposition::Dropped,
        }
    }

    #[tokio::test]
    async fn live_windows_get_one_outcome_and_are_held_until_expiry() {
        let (first, second, gone) = (Window::spawn(), Window::spawn(), Window::spawn());
        let (live, live_client) = stage(&first, 0, &[11, 12], true);
        let (unsent, unsent_client) = stage(&second, 1, &[13], true);
        // The Window's end is gone: the send fails and it is dropped.
        drop(unsent_client);
        let (exited, exited_client) = stage(&gone, 2, &[14], false);
        let mut splints = vec![held_splint(0, 12), held_splint(1, 13), held_splint(2, 14)];
        let mut released = held_splint(0, 11);
        released.controller = ControllerDisposition::Released;
        splints.push(released);
        let outcome = HandoffOutcome {
            kind: HandoffOutcomeKind::Adopted,
            daemon_generation: 2,
        };
        let (hold, announcement) = announce_and_hold(
            vec![live, unsent, exited],
            &splints,
            outcome,
            Duration::from_millis(300),
        );
        assert_eq!(
            announcement,
            WindowAnnouncement {
                announced: 1,
                exited: 1,
                unsent: 1,
                placeholders: 1,
            }
        );
        let packet = read_packet(&live_client).unwrap();
        assert_eq!(packet.len(), HANDOFF_OUTCOME_BYTES);
        assert_eq!(HandoffOutcome::decode(&packet), Ok(outcome));
        // Exactly one packet.
        assert_eq!(read_packet(&live_client), None);
        // The exited Window got nothing and its end closed at once.
        assert_end_of_file(&exited_client);
        assert!(hold.is_held());
        assert_eq!(hold.window_count(), 1);
        assert_eq!(
            hold.placeholders(),
            [ControllerPlaceholder {
                splint_id: splints[0].splint_id,
                incarnation: splints[0].incarnation,
                window: 0,
                connection: 12,
            }]
        );
        // Expiry closes the held Window: it reads end of file.
        tokio::time::timeout(Duration::from_secs(5), async {
            while hold.is_held() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the hold expires");
        assert!(hold.placeholders().is_empty());
        assert_end_of_file(&live_client);
    }

    #[tokio::test]
    async fn a_rollback_announces_rolled_back_and_release_ends_the_hold() {
        let window = Window::spawn();
        let (live, client) = stage(&window, 0, &[5], true);
        let outcome = HandoffOutcome {
            kind: HandoffOutcomeKind::RolledBack,
            daemon_generation: 1,
        };
        let (hold, announcement) =
            announce_and_hold(vec![live], &[], outcome, HANDOFF_WINDOW_RESUME_HOLD);
        assert_eq!(announcement.announced, 1);
        assert_eq!(
            HandoffOutcome::decode(&read_packet(&client).unwrap()),
            Ok(outcome)
        );
        assert!(hold.is_held());
        hold.release();
        assert!(!hold.is_held());
        assert_end_of_file(&client);
    }

    #[test]
    fn without_a_runtime_nothing_is_held() {
        let window = Window::spawn();
        let (live, client) = stage(&window, 0, &[5], true);
        let (hold, announcement) = announce_and_hold(
            vec![live],
            &[held_splint(0, 5)],
            HandoffOutcome {
                kind: HandoffOutcomeKind::Adopted,
                daemon_generation: 2,
            },
            HANDOFF_WINDOW_RESUME_HOLD,
        );
        assert_eq!(
            announcement,
            WindowAnnouncement {
                unsent: 1,
                ..WindowAnnouncement::default()
            }
        );
        assert!(!hold.is_held());
        // No outcome went out: the channel just closed.
        assert_end_of_file(&client);
    }
}
