//! The continuation server and resume tickets of an adopted daemon (Plan
//! 0061 M3.16; ADR 0011, "Smooth local client recovery").
//!
//! After an adoption commits, the daemon holds every carried Window for
//! [`HANDOFF_WINDOW_RESUME_HOLD`](crate::handoff_window_carry::HANDOFF_WINDOW_RESUME_HOLD)
//! ([`crate::handoff_window_carry`]): its pinned process (a pidfd), the
//! daemon end of its continuation channel, and the controllers its old
//! connections held at the fence as placeholders. This module turns a
//! placeholder back into a controller lease, for the relaunched Window only.
//!
//! # The claim model
//!
//! A Window holds one control connection per pane, and a controller lease is
//! bound to one connection ([`splinterm_protocol::Request::ClaimHandoffControl`]).
//! So the claim has two steps:
//!
//! 1. **One continuation claim per Window.** The relaunched Window sends one
//!    [`HandoffTicketClaim`] on its continuation channel. The daemon
//!    authenticates the sender ([`verify_window_process`]) and answers with
//!    one [`HandoffTicketGrant`] carrying a fresh resume ticket, then closes
//!    the channel. A refusal closes the channel with nothing sent and drops
//!    the Window and its placeholders. The channel carries nothing else.
//! 2. **One redemption per pane.** Each pane's new control connection sends
//!    `ClaimHandoffControl { ticket, splint }` naming its own Splint, and the
//!    daemon restores that Splint's controller to that connection
//!    ([`HandoffWindowHold::redeem`]).
//!
//! A single claim naming `(Splint, connection)` pairs was considered and not
//! taken: connection IDs are daemon-private, the continuation channel would
//! then have to carry controller IDs back (ADR 0011 allows it one ticket),
//! and each pane still has to learn its controller ID on its own connection.
//!
//! # Authentication chain
//!
//! - The continuation end is a credentialed `SOCK_SEQPACKET` socket
//!   (`SO_PASSCRED`, checked at staging), so the kernel attaches
//!   `SCM_CREDENTIALS` to every packet. A sender without `CAP_SYS_ADMIN`
//!   cannot forge them: the kernel refuses a supplied PID other than the
//!   sender's own with `EPERM` (`scm_check_creds`), and fills in the real
//!   sender when none is supplied. A root sender can forge anything, as it
//!   can ptrace the Window; that is outside the threat model. The kernel
//!   reports PID 0 for a sender outside the daemon's PID namespace; the
//!   credentials are decoded with nix as a plain `pid_t`, never into a
//!   non-zero type, and a PID of zero or below is refused before anything
//!   else reads them ([`sender_pid`]).
//! - The credentials' UID must be the daemon's, and their PID the held
//!   Window's: a forked child, or any other process holding a duplicate of
//!   the Window's end (the #175 note), has another PID and is refused.
//! - The PID must still name the pinned process: the pidfd has not exited
//!   and the start time is the recorded one, read between pidfd checks
//!   ([`crate::handoff_window_carry::inspect_window_process`]).
//! - `/proc/<pid>/exe` must be the sealed forward client snapshot this
//!   daemon pinned, by device and inode only (AGENTS.md §4.4, the 2026-10-18
//!   exception). It is read before that pidfd check, so a live pidfd proves
//!   it described the pinned process. A copy with the same bytes, or the
//!   stale pre-upgrade inode, does not match. A rollback daemon pins no
//!   snapshot and issues no ticket: its Windows take control the ordinary
//!   way.
//! - Each redemption repeats the process checks against the redeeming
//!   connection: its peer PID and recorded start time must be the Window's,
//!   the pidfd alive, and its executable, read again, the snapshot the
//!   ticket was bound to. The handler has already required a trusted-UI
//!   connection running a matching client, so automation and relay
//!   connections are refused before the ticket is looked at and cannot end
//!   it.
//!
//! # The ticket
//!
//! [`HANDOFF_RESUME_TICKET_BYTES`] (256 bits) from `getrandom`, held only in
//! this daemon's memory, never logged or persisted, and compared in full
//! without an early exit. It is bound to its Window (index, PID, start time,
//! pidfd), the snapshot identity, and this daemon's generation, and it ends
//! with the hold. It is single-use per controller: each placeholder it
//! carries is restored at most once, a second presentation for the same
//! Splint is a replay, and the ticket ends with its last placeholder. A
//! presentation that fails the process binding ends the ticket and drops
//! the Window's placeholders, as a transferred ticket must.
//!
//! # Revalidation
//!
//! A redemption grants a lease only for the Splint it names, when that
//! Window holds a placeholder for it whose incarnation is the named one and
//! the live one, and only through `restore`, which the caller runs against
//! the live controller state: no controller on the Splint now, none on the
//! connection, never grant-bound. Anything else is skipped and reported,
//! and the placeholder is spent; nothing is forced. A `ReleaseControl`
//! admitted under the fence after the `HeldBy` snapshot (#167 note b)
//! therefore restores only if the Window names that Splint again.
//!
//! # Expiry and concurrency
//!
//! Every step runs under the hold's one mutex: the continuation claim, each
//! redemption, every refusal that drops a Window, and the release at expiry.
//! A claim therefore either completes before the release or finds nothing
//! left, and a redemption past the deadline releases the hold itself
//! instead of using it. Whatever takes the held Windows takes them once.
//! A Window that never claims loses its placeholders at expiry, and its
//! panes acquire control the ordinary way.

use std::{
    fmt,
    io::IoSliceMut,
    os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd, RawFd},
    time::Instant,
};

use nix::{
    errno::Errno,
    sys::socket::{ControlMessageOwned, MsgFlags, UnixCredentials, recvmsg},
};
use rustix::{
    net::{SendFlags, send},
    rand::{GetRandomFlags, getrandom},
};
use splinterm_core::SplintId;
use splinterm_protocol::{
    HANDOFF_RESUME_TICKET_BYTES, HandoffResumeTicket,
    handoff_continuation::{HandoffTicketClaim, HandoffTicketGrant},
};
use tokio::io::{Interest, unix::AsyncFd};
use tracing::{info, warn};

use crate::handoff_window_carry::{
    ControllerPlaceholder, HandoffWindowHold, HeldHandoffWindow, HeldHandoffWindows,
    WindowProcessState, inspect_window_process,
};

/// The receive buffer: larger than a claim, so a longer packet is seen whole
/// and refused instead of truncated silently.
const RECEIVE_BUFFER_BYTES: usize = 64;
/// Descriptors a refused packet may carry that are still received, so they
/// are closed at once rather than left queued.
const MAX_REFUSED_DESCRIPTORS: usize = 8;

/// What a continuation claim is checked against.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContinuationBinding {
    /// The generation this daemon serves in `Hello`.
    pub daemon_generation: u64,
    /// Device and inode of the sealed forward client snapshot this daemon
    /// pinned; `None` in a rollback daemon, which issues no ticket.
    pub client_snapshot: Option<(u64, u64)>,
    /// The daemon's effective UID.
    pub uid: u32,
}

/// A resume ticket a valid continuation claim issued to one Window.
pub(crate) struct IssuedTicket {
    bytes: [u8; HANDOFF_RESUME_TICKET_BYTES],
    daemon_generation: u64,
    client_snapshot: (u64, u64),
    /// The Splints whose placeholder this ticket already spent.
    spent: Vec<SplintId>,
}

impl fmt::Debug for IssuedTicket {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IssuedTicket")
            .field("daemon_generation", &self.daemon_generation)
            .field("spent", &self.spent.len())
            .finish_non_exhaustive()
    }
}

/// Why a continuation claim was refused. Every refusal closes the channel
/// and drops the Window with its placeholders.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimRefusal {
    /// The Window closed its end without claiming.
    EndOfFile,
    /// The channel could not be read.
    Receive,
    /// A packet longer than any claim, or truncated control data.
    Truncated,
    /// The packet carried descriptors; they were closed on receipt.
    Descriptors,
    /// No kernel credentials, or more than one set.
    Credentials,
    /// Not a claim packet.
    Malformed,
    /// The claim names a generation this daemon does not serve.
    Generation,
    /// Credentials of another user.
    User,
    /// Credentials of another process than the held Window's.
    Process,
    /// The Window's pinned process exited.
    Exited,
    /// The PID names another process than the pinned one, or the process
    /// cannot be inspected.
    Identity,
    /// The process does not execute the pinned forward client snapshot.
    Executable,
    /// This daemon pinned no snapshot (a rollback) and issues no ticket.
    NoSnapshot,
    /// The Window holds no controller to restore.
    NothingHeld,
    /// The Window already claimed its ticket.
    AlreadyClaimed,
    /// The hold ended, or no longer holds this Window.
    Released,
    /// No randomness for a ticket.
    Random,
    /// The grant could not be sent whole.
    Send,
}

/// One claim packet as received, with its kernel credentials.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ReceivedClaim {
    pub(crate) claim: HandoffTicketClaim,
    /// The sender's PID, translated into this daemon's PID namespace; never
    /// zero ([`sender_pid`]).
    pub(crate) pid: u32,
    pub(crate) uid: u32,
}

/// The sender PID from a packet's kernel credentials. The kernel reports 0
/// for a sender whose PID is not visible in this daemon's PID namespace,
/// which can name no held Window, and no PID is negative: both are refused.
///
/// # Errors
/// [`ClaimRefusal::Credentials`] for a PID of zero or below.
pub(crate) fn sender_pid(pid: libc::pid_t) -> Result<u32, ClaimRefusal> {
    match u32::try_from(pid) {
        Ok(pid) if pid > 0 => Ok(pid),
        _ => Err(ClaimRefusal::Credentials),
    }
}

/// What one `recvmsg` delivered, copied out of its borrowed buffers.
struct Received {
    bytes: usize,
    truncated: bool,
    descriptors: bool,
    /// `(pid, uid)` of every `SCM_CREDENTIALS` message, as the kernel wrote
    /// them: decoded by nix into a plain `pid_t`, so a PID of zero is a value
    /// to refuse, never an invalid one.
    credentials: Vec<(libc::pid_t, u32)>,
}

/// Reads at most one packet from a continuation end, without blocking.
///
/// `Ok(None)` means nothing is queued. Descriptors are received
/// close-on-exec and closed at once; a packet carrying any, truncated
/// data or control, not exactly one set of credentials, credentials naming
/// no visible PID, or not a claim layout is refused. Zero bytes is end of
/// file.
pub(crate) fn receive_claim(
    continuation: BorrowedFd<'_>,
) -> Result<Option<ReceivedClaim>, ClaimRefusal> {
    let mut buffer = [0_u8; RECEIVE_BUFFER_BYTES];
    let mut space = nix::cmsg_space!(UnixCredentials, [RawFd; MAX_REFUSED_DESCRIPTORS]);
    let received = loop {
        let mut iov = [IoSliceMut::new(&mut buffer)];
        match recvmsg::<()>(
            continuation.as_raw_fd(),
            &mut iov,
            Some(&mut space),
            MsgFlags::MSG_DONTWAIT | MsgFlags::MSG_CMSG_CLOEXEC,
        ) {
            Ok(message) => {
                let mut received = Received {
                    bytes: message.bytes,
                    truncated: message
                        .flags
                        .intersects(MsgFlags::MSG_TRUNC | MsgFlags::MSG_CTRUNC),
                    descriptors: false,
                    credentials: Vec::with_capacity(1),
                };
                let Ok(messages) = message.cmsgs() else {
                    return Err(ClaimRefusal::Receive);
                };
                for control in messages {
                    match control {
                        ControlMessageOwned::ScmRights(descriptors) => {
                            received.descriptors = true;
                            // Received close-on-exec and owned by nothing
                            // else: closed here, at once.
                            for descriptor in descriptors {
                                let _ = nix::unistd::close(descriptor);
                            }
                        }
                        ControlMessageOwned::ScmCredentials(sender) => {
                            received.credentials.push((sender.pid(), sender.uid()));
                        }
                        _ => {}
                    }
                }
                break received;
            }
            Err(Errno::EINTR) => {}
            Err(Errno::EAGAIN) => return Ok(None),
            Err(_) => return Err(ClaimRefusal::Receive),
        }
    };
    if received.truncated {
        return Err(ClaimRefusal::Truncated);
    }
    if received.descriptors {
        return Err(ClaimRefusal::Descriptors);
    }
    if received.bytes == 0 {
        return Err(ClaimRefusal::EndOfFile);
    }
    let [(pid, uid)] = received.credentials.as_slice() else {
        return Err(ClaimRefusal::Credentials);
    };
    let pid = sender_pid(*pid)?;
    let claim = HandoffTicketClaim::decode(&buffer[..received.bytes])
        .map_err(|_| ClaimRefusal::Malformed)?;
    Ok(Some(ReceivedClaim {
        claim,
        pid,
        uid: *uid,
    }))
}

/// Device and inode of the file `pid` executes, from its opened executable
/// link: `None` when it cannot be opened or is not a regular file. A memfd
/// image is a regular file and is named by its own inode.
#[must_use]
pub fn process_executable(pid: u32) -> Option<(u64, u64)> {
    let executable = rustix::fs::open(
        format!("/proc/{pid}/exe").as_str(),
        rustix::fs::OFlags::PATH | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .ok()?;
    let status = rustix::fs::fstat(&executable).ok()?;
    (rustix::fs::FileType::from_raw_mode(status.st_mode) == rustix::fs::FileType::RegularFile)
        .then_some((status.st_dev, status.st_ino))
}

/// Why a process is not the held Window running the pinned snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessMismatch {
    /// Another PID.
    Process,
    /// The pinned process exited.
    Exited,
    /// The PID names another process, or cannot be inspected.
    Identity,
    /// Not executing the snapshot by device and inode.
    Executable,
}

/// Requires `pid` to be `window`'s pinned process, still alive, executing
/// exactly `snapshot` (device, inode).
///
/// The executable is read first and the pidfd checked after, so a pidfd
/// still alive proves the read described the pinned process and not a later
/// one reusing its PID.
///
/// # Errors
/// The first [`ProcessMismatch`] found.
pub(crate) fn verify_window_process(
    window: &HeldHandoffWindow,
    pid: u32,
    snapshot: (u64, u64),
) -> Result<(), ProcessMismatch> {
    if pid != window.record.pid {
        return Err(ProcessMismatch::Process);
    }
    let executable = process_executable(pid);
    match inspect_window_process(window.process.as_fd(), &window.record) {
        Ok(WindowProcessState::Live) => {}
        Ok(WindowProcessState::Exited) => return Err(ProcessMismatch::Exited),
        Err(_) => return Err(ProcessMismatch::Identity),
    }
    if executable != Some(snapshot) {
        return Err(ProcessMismatch::Executable);
    }
    Ok(())
}

impl From<ProcessMismatch> for ClaimRefusal {
    fn from(mismatch: ProcessMismatch) -> Self {
        match mismatch {
            ProcessMismatch::Process => Self::Process,
            ProcessMismatch::Exited => Self::Exited,
            ProcessMismatch::Identity => Self::Identity,
            ProcessMismatch::Executable => Self::Executable,
        }
    }
}

fn random_ticket() -> Option<[u8; HANDOFF_RESUME_TICKET_BYTES]> {
    let mut bytes = [0_u8; HANDOFF_RESUME_TICKET_BYTES];
    let mut filled = 0;
    while filled < bytes.len() {
        match getrandom(&mut bytes[filled..], GetRandomFlags::empty()) {
            Ok(0) => return None,
            Ok(read) => filled += read,
            Err(rustix::io::Errno::INTR) => {}
            Err(_) => return None,
        }
    }
    Some(bytes)
}

/// Compares two tickets in full, without an early exit.
fn same_ticket(
    left: &[u8; HANDOFF_RESUME_TICKET_BYTES],
    right: &[u8; HANDOFF_RESUME_TICKET_BYTES],
) -> bool {
    let difference = left
        .iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        });
    std::hint::black_box(difference) == 0
}

/// Sends `packet` once, whole, without blocking and without `SIGPIPE`.
fn send_whole(continuation: BorrowedFd<'_>, packet: &[u8]) -> bool {
    send(
        continuation,
        packet,
        SendFlags::NOSIGNAL | SendFlags::DONTWAIT,
    )
    .is_ok_and(|sent| sent == packet.len())
}

/// A ticket presented with `ClaimHandoffControl` on one connection.
#[derive(Clone, Copy)]
pub struct TicketPresentation {
    pub ticket: [u8; HANDOFF_RESUME_TICKET_BYTES],
    /// The presenting connection's peer PID and its start time, recorded
    /// when the connection was verified.
    pub claimant_pid: u32,
    pub claimant_start_time: Option<u64>,
    /// The Splint the connection names, and the incarnation it attached to.
    pub splint_id: SplintId,
    pub incarnation: u64,
    /// The Splint's live incarnation now, read by the caller.
    pub live_incarnation: Option<u64>,
    /// The generation this daemon serves.
    pub daemon_generation: u64,
}

impl fmt::Debug for TicketPresentation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TicketPresentation")
            .field("claimant_pid", &self.claimant_pid)
            .field("splint_id", &self.splint_id)
            .field("incarnation", &self.incarnation)
            .finish_non_exhaustive()
    }
}

/// Why a presented ticket restores nothing and fails with
/// `InvalidHandoffTicket`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TicketRefusal {
    /// No held Window was issued this ticket: never issued, ended, or the
    /// hold was released.
    Unknown,
    /// The hold's deadline passed; the hold is released now.
    Expired,
    /// Bound to another daemon generation. Ends the ticket.
    Generation,
    /// Presented by a connection that is not the Window's pinned process
    /// running the snapshot. Ends the ticket.
    Binding(ProcessMismatch),
    /// The named Splint's placeholder was already spent by this ticket.
    Replayed,
}

/// Why a valid ticket restored nothing for the named Splint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimSkip {
    /// The Window holds no placeholder for the Splint.
    NotHeld,
    /// The named, recorded, or live incarnation differ.
    IncarnationChanged,
    /// The live controller state refused it: the Splint has a controller
    /// now, or the connection already holds one.
    Unavailable,
}

/// What a valid redemption did.
#[derive(Debug, Eq, PartialEq)]
pub enum Redemption<T> {
    /// The placeholder became a lease; `T` is what `restore` returned.
    Restored(T),
    /// Nothing was restored.
    Skipped(ClaimSkip),
}

/// Drops the Window at `position` with its placeholders and ticket, and
/// ends the hold when it held the last Window.
fn drop_window(owner: &HandoffWindowHold, slot: &mut Option<HeldHandoffWindows>, position: usize) {
    let Some(held) = slot.as_mut() else {
        return;
    };
    if position >= held.windows.len() {
        return;
    }
    let window = held.windows.remove(position);
    let index = window.record.index;
    held.placeholders
        .retain(|placeholder| placeholder.window != index);
    drop(window);
    if held.windows.is_empty() {
        *slot = None;
        owner.released.send_replace(true);
    }
}

impl HandoffWindowHold {
    /// Drops Window `index` with its placeholders and ticket, under the
    /// hold's mutex: its continuation cannot be served.
    pub(crate) fn refuse_window(&self, index: u8) {
        if let Ok(mut slot) = self.held.lock() {
            let position = slot.as_ref().and_then(|held| {
                held.windows
                    .iter()
                    .position(|window| window.record.index == index)
            });
            if let Some(position) = position {
                drop_window(self, &mut slot, position);
            }
        }
    }

    /// Completes Window `index`'s continuation claim: on a valid `received`
    /// claim, issues its ticket and sends the grant on `continuation`.
    ///
    /// Everything runs under the hold's mutex. Any refusal, a receive
    /// refusal included, drops the Window with its placeholders; the
    /// caller then closes the channel.
    ///
    /// # Errors
    /// The [`ClaimRefusal`] that refused it.
    pub(crate) fn complete_claim(
        &self,
        index: u8,
        continuation: BorrowedFd<'_>,
        received: Result<ReceivedClaim, ClaimRefusal>,
        binding: &ContinuationBinding,
        now: Instant,
    ) -> Result<(), ClaimRefusal> {
        let Ok(mut slot) = self.held.lock() else {
            return Err(ClaimRefusal::Released);
        };
        let Some(held) = slot.as_mut() else {
            return Err(ClaimRefusal::Released);
        };
        let Some(position) = held
            .windows
            .iter()
            .position(|window| window.record.index == index)
        else {
            return Err(ClaimRefusal::Released);
        };
        if held.windows[position].ticket.is_some() {
            // A claimed Window keeps its ticket; only this attempt fails.
            return Err(ClaimRefusal::AlreadyClaimed);
        }
        let issued = (|| {
            let received = received?;
            if now >= held.expires_at {
                return Err(ClaimRefusal::Released);
            }
            if received.claim.daemon_generation != binding.daemon_generation {
                return Err(ClaimRefusal::Generation);
            }
            if received.uid != binding.uid {
                return Err(ClaimRefusal::User);
            }
            let snapshot = binding.client_snapshot.ok_or(ClaimRefusal::NoSnapshot)?;
            verify_window_process(&held.windows[position], received.pid, snapshot)?;
            if !held
                .placeholders
                .iter()
                .any(|placeholder| placeholder.window == index)
            {
                return Err(ClaimRefusal::NothingHeld);
            }
            let bytes = random_ticket().ok_or(ClaimRefusal::Random)?;
            let grant = HandoffTicketGrant {
                daemon_generation: binding.daemon_generation,
                ticket: HandoffResumeTicket::new(bytes),
            };
            if !send_whole(continuation, &grant.encode()) {
                return Err(ClaimRefusal::Send);
            }
            Ok(IssuedTicket {
                bytes,
                daemon_generation: binding.daemon_generation,
                client_snapshot: snapshot,
                spent: Vec::new(),
            })
        })();
        match issued {
            Ok(ticket) => {
                held.windows[position].ticket = Some(ticket);
                Ok(())
            }
            Err(refusal) => {
                drop_window(self, &mut slot, position);
                Err(refusal)
            }
        }
    }

    /// Redeems a ticket presented with `ClaimHandoffControl`: restores the
    /// named Splint's placeholder through `restore`, at most once.
    ///
    /// Under the hold's mutex, it finds the Window the ticket was issued to,
    /// requires the presentation's generation and process binding, refuses
    /// a Splint whose placeholder this ticket already spent, and then
    /// spends the Window's placeholder for the named Splint, if any: only
    /// when the named, recorded, and live incarnations agree does it call
    /// `restore`, which grants the lease against the live controller state
    /// and returns `None` when it cannot. The ticket ends with the Window's
    /// last placeholder, and the hold with its last Window. A presentation
    /// past the deadline releases the hold.
    ///
    /// # Errors
    /// [`TicketRefusal`]; a generation or binding refusal also ends the
    /// ticket and drops the Window's placeholders.
    pub fn redeem<T>(
        &self,
        presentation: &TicketPresentation,
        now: Instant,
        restore: impl FnOnce(&ControllerPlaceholder) -> Option<T>,
    ) -> Result<Redemption<T>, TicketRefusal> {
        let Ok(mut slot) = self.held.lock() else {
            return Err(TicketRefusal::Unknown);
        };
        let Some(held) = slot.as_mut() else {
            return Err(TicketRefusal::Unknown);
        };
        if now >= held.expires_at {
            let expired = slot.take();
            self.released.send_replace(true);
            drop(slot);
            if let Some(expired) = expired {
                info!(
                    windows = expired.windows.len(),
                    placeholders = expired.placeholders.len(),
                    "daemon handoff: the carried Windows' resume hold ended at a late claim"
                );
            }
            return Err(TicketRefusal::Expired);
        }
        // Every issued ticket is compared in full.
        let mut found = None;
        for (position, window) in held.windows.iter().enumerate() {
            if window
                .ticket
                .as_ref()
                .is_some_and(|ticket| same_ticket(&ticket.bytes, &presentation.ticket))
            {
                found = Some(position);
            }
        }
        let Some(position) = found else {
            return Err(TicketRefusal::Unknown);
        };
        let window = &held.windows[position];
        let index = window.record.index;
        let Some(ticket) = window.ticket.as_ref() else {
            return Err(TicketRefusal::Unknown);
        };
        let bound = if ticket.daemon_generation == presentation.daemon_generation {
            if presentation.claimant_start_time == Some(window.record.start_time) {
                verify_window_process(window, presentation.claimant_pid, ticket.client_snapshot)
                    .map_err(TicketRefusal::Binding)
            } else {
                Err(TicketRefusal::Binding(ProcessMismatch::Process))
            }
        } else {
            Err(TicketRefusal::Generation)
        };
        if let Err(refusal) = bound {
            drop_window(self, &mut slot, position);
            return Err(refusal);
        }
        if ticket.spent.contains(&presentation.splint_id) {
            return Err(TicketRefusal::Replayed);
        }
        let outcome = match held.placeholders.iter().position(|placeholder| {
            placeholder.window == index && placeholder.splint_id == presentation.splint_id
        }) {
            None => Redemption::Skipped(ClaimSkip::NotHeld),
            Some(spent) => {
                let placeholder = held.placeholders.remove(spent);
                if let Some(ticket) = held.windows[position].ticket.as_mut() {
                    ticket.spent.push(placeholder.splint_id);
                }
                if placeholder.incarnation != presentation.incarnation
                    || presentation.live_incarnation != Some(placeholder.incarnation)
                {
                    Redemption::Skipped(ClaimSkip::IncarnationChanged)
                } else {
                    restore(&placeholder).map_or(
                        Redemption::Skipped(ClaimSkip::Unavailable),
                        Redemption::Restored,
                    )
                }
            }
        };
        if !held
            .placeholders
            .iter()
            .any(|placeholder| placeholder.window == index)
        {
            drop_window(self, &mut slot, position);
        }
        Ok(outcome)
    }

    /// A hold of one Window whose continuation claim already issued
    /// `ticket`, for the daemon binary's handler tests: the lib's types are
    /// not constructible from there. Only with the `test-support` feature,
    /// which only this package's own dev-dependency enables.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    #[must_use]
    #[allow(clippy::too_many_arguments, reason = "a test seam names every binding")]
    pub fn ticketed_for_test(
        pid: u32,
        start_time: u64,
        process: OwnedFd,
        client_snapshot: (u64, u64),
        daemon_generation: u64,
        ticket: [u8; HANDOFF_RESUME_TICKET_BYTES],
        placeholders: Vec<ControllerPlaceholder>,
        expires_at: Instant,
    ) -> Self {
        let hold = Self::default();
        let window = HeldHandoffWindow {
            record: crate::handoff_manifest::ManifestWindow {
                index: 0,
                pid,
                start_time,
                client_device: 0,
                client_inode: 0,
                old_connections: placeholders
                    .iter()
                    .map(|placeholder| placeholder.connection)
                    .collect(),
            },
            process,
            continuation: None,
            ticket: Some(IssuedTicket {
                bytes: ticket,
                daemon_generation,
                client_snapshot,
                spent: Vec::new(),
            }),
        };
        if let Ok(mut slot) = hold.held.lock() {
            *slot = Some(HeldHandoffWindows {
                windows: vec![window],
                placeholders,
                expires_at,
            });
        }
        hold
    }
}

/// Serves every held Window's continuation channel for its one claim, on
/// the current Tokio runtime: takes each daemon end the hold still owns and
/// spawns one task per Window. Returns how many it serves; without a
/// runtime it serves none, and expiry closes them.
#[must_use]
pub fn serve_continuations(hold: &HandoffWindowHold, binding: ContinuationBinding) -> usize {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        warn!("daemon handoff: no runtime can serve the carried Windows' continuations");
        return 0;
    };
    let taken = hold
        .held
        .lock()
        .ok()
        .and_then(|mut slot| {
            slot.as_mut().map(|held| {
                held.windows
                    .iter_mut()
                    .filter_map(|window| {
                        window
                            .continuation
                            .take()
                            .map(|continuation| (window.record.index, continuation))
                    })
                    .collect::<Vec<_>>()
            })
        })
        .unwrap_or_default();
    let served = taken.len();
    for (index, continuation) in taken {
        drop(runtime.spawn(serve_window(hold.clone(), index, continuation, binding)));
    }
    served
}

/// Waits for Window `index`'s one claim, or the hold's release, and
/// completes it. Dropping the descriptor at the end closes the daemon end:
/// after a grant the Window reads end of file next, after a refusal at once.
async fn serve_window(
    hold: HandoffWindowHold,
    index: u8,
    continuation: OwnedFd,
    binding: ContinuationBinding,
) {
    let mut released = hold.released.subscribe();
    let continuation = match AsyncFd::with_interest(continuation, Interest::READABLE) {
        Ok(continuation) => continuation,
        Err(error) => {
            warn!(%error, index, "daemon handoff: cannot serve a carried Window's continuation");
            hold.refuse_window(index);
            return;
        }
    };
    let received = loop {
        tokio::select! {
            biased;
            _ = released.wait_for(|released| *released) => return,
            ready = continuation.readable() => {
                let Ok(mut ready) = ready else {
                    break Err(ClaimRefusal::Receive);
                };
                match receive_claim(continuation.get_ref().as_fd()) {
                    Ok(None) => ready.clear_ready(),
                    Ok(Some(received)) => break Ok(received),
                    Err(refusal) => break Err(refusal),
                }
            }
        }
    };
    match hold.complete_claim(
        index,
        continuation.get_ref().as_fd(),
        received,
        &binding,
        Instant::now(),
    ) {
        Ok(()) => info!(
            index,
            "daemon handoff: issued a carried Window its resume ticket"
        ),
        Err(refusal) => warn!(
            ?refusal,
            index, "daemon handoff: refused a carried Window's continuation claim"
        ),
    }
}

#[cfg(test)]
#[path = "handoff_continuation/tests.rs"]
mod tests;
