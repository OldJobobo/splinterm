//! A local Window's part in a daemon handoff, up to its relaunch (Plan 0061
//! M3.14; ADR 0011, "Smooth local client recovery").
//!
//! Every local managed Window holds one dedicated **anchor** connection that
//! declares `Hello.handoff_window` and sends nothing else but its
//! acknowledgement. A separate connection is required: the Window's other
//! connections send requests that take the daemon's topology permit
//! (`InspectTopology` every 250 ms on the topology manager's connection,
//! `PrepareMutation`, `RevokeAccess`), and a connection runs its requests in
//! order, so under the handoff fence such a request would hold the
//! acknowledgement behind it for up to about 27 s, far past the daemon's 2 s
//! wait. Pane connections come and go with their Splints, so none can carry
//! the Window's identity either.
//!
//! **Receiving.** The anchor reads each frame with exact-length reads: its
//! four-byte header, then exactly its body ([`AnchorConnection`]). The
//! daemon writes a `HandoffPrepare` frame with one `sendmsg` whose
//! descriptors ride its first byte, and a Unix stream never merges data
//! across a descriptor-carrying segment into an earlier read, so the
//! descriptors arrive with that frame's header read and with no other. A
//! buffered reader cannot say which frame descriptors belong to once a read
//! spans frames, so it is not used here. The cost, two receive calls per
//! frame, falls only on this idle connection, which carries a handshake, at
//! most one prepare frame per attempt, and its acknowledgement's answer; the
//! busy pane streams keep their buffered reader. Descriptors that arrive
//! with a body read, a frame without exactly two, or two beside any other
//! frame are refused, and refusal closes them.
//!
//! **Taking part.** On a valid prepare frame from its own daemon
//! generation, the Window builds its resume record from the frontend's
//! current tabs and focus ([`WindowLayoutLedger`]), seals it, pauses child
//! spawns ([`crate::child_spawn`]), enters
//! [`WindowHandoffPhase::Continuing`](crate::handoff_status::WindowHandoffPhase::Continuing),
//! which pauses input and gates ordinary reconnection, and only then sends
//! `AcknowledgeHandoffPrepare` echoing the frame's generation and
//! `prepare_id`. All of this is local and well inside the daemon's 2 s.
//!
//! **The outcome** arrives as one packet on the continuation socket, read
//! into a buffer larger than the record and decoded from exactly what was
//! read ([`HandoffOutcome::decode`]):
//! - `Adopted` at the next generation: the Window `execveat`s the sealed
//!   forward client snapshot (`AT_EMPTY_PATH`), which the adopted daemon
//!   pins and trusts by device and inode (AGENTS.md §4.4, the 2026-10-18
//!   exception);
//! - `RolledBack` at the same generation: the Window discards the snapshot
//!   and execs the installed `<prefix>/splinterm` by path, which the
//!   rollback daemon trusts; it never keeps running its pre-upgrade image,
//!   which cannot regain trusted UI;
//! - end of file (a cancelled attempt, or a dropped Window), a malformed
//!   packet, a generation that does not fit, or no packet within
//!   [`HANDOFF_OUTCOME_DEADLINE`]: the Window closes every handoff artifact,
//!   resumes spawns, and returns to M3.12's ordinary path, which resumes, or
//!   reconnects if a connection was lost meanwhile.
//!
//! **The relaunch** keeps the process (the daemon pinned it by pidfd) and
//! replaces its image. Argv is rebuilt, never copied: the program name and
//! the two activation flags naming the record and continuation descriptors
//! at their own numbers. The environment is passed on unchanged. Only those
//! two descriptors have close-on-exec cleared, right before the exec and
//! after every child spawn under way has returned; the snapshot descriptor
//! stays close-on-exec, which `execveat(AT_EMPTY_PATH)` of an ELF image
//! permits. This library holds no unsafe code, so it cannot change the flags
//! of descriptors it does not own: any descriptor the Window inherited
//! already inheritable from its own launcher crosses the exec too, and the
//! relaunched image's audited claim closes every such unexpected descriptor
//! before it does anything else. A failed exec restores both flags and
//! falls back like a cancelled attempt. Nothing typed is replayed: M3.12's
//! pause dropped it.

use std::{
    ffi::{CString, OsStr},
    fs, io,
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
        unix::{
            ffi::{OsStrExt as _, OsStringExt as _},
            fs::MetadataExt as _,
        },
    },
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use rustix::{
    io::{FdFlags, fcntl_getfd, fcntl_setfd},
    net::RecvFlags,
};
use splinterm_protocol::{
    ClientFrame, ClientRole, ErrorCode, PROTOCOL_VERSION, ProtocolError, Request, Response,
    ServerFrame, encode_frame,
    handoff_outcome::{HandoffOutcome, HandoffOutcomeKind},
    handoff_prepare::{
        HandoffDescriptorError, HandoffPrepareDescriptors, ReceivedBytes, accept_handoff_prepare,
        recv_with_descriptors,
    },
    window_resume::{
        WindowResumeRecord, WindowResumeTab, seal_window_resume_record, valid_app_id,
        window_resume_arguments,
    },
};
use tokio::{
    io::{AsyncWriteExt as _, Interest, unix::AsyncFd},
    net::UnixStream,
};

use crate::{
    child_spawn::{self, SpawnPause},
    handoff_status::{ContinuationParticipation, HandoffStatusHub},
};

/// Largest frame the anchor accepts. It receives only the handshake, prepare
/// frames, its acknowledgement's answer, and connection errors, each a few
/// hundred bytes; anything larger is a protocol violation.
pub const MAX_ANCHOR_FRAME_BYTES: usize = 64 * 1024;
/// How long the anchor waits for the daemon's answer to its acknowledgement.
/// The daemon's own wait for acknowledgements is 2 s, so a later answer
/// changes nothing; without one the Window still waits for the outcome,
/// which is the authority.
pub const ACKNOWLEDGEMENT_RESPONSE_DEADLINE: Duration = Duration::from_secs(2);
/// How long a participating Window waits for its continuation's outcome.
/// It covers the acknowledgement wait (2 s), the longest decision prompt a
/// daemon may hold the fence for (60 s,
/// [`splinterm_protocol::MAX_HANDOFF_DECISION_DEADLINE_MS`]), capture, exec,
/// and the candidate's adoption, with margin. A Window that gives up returns
/// to the ordinary path.
pub const HANDOFF_OUTCOME_DEADLINE: Duration = Duration::from_secs(120);
/// How long the relaunch waits for child spawns admitted before the pause to
/// return before it clears close-on-exec; it refuses to relaunch after that.
pub const SPAWN_IDLE_DEADLINE: Duration = Duration::from_secs(2);
/// Pause between anchor connection attempts.
pub const ANCHOR_RETRY_DELAY: Duration = Duration::from_secs(2);
/// The outcome receive buffer: larger than the record, so a longer packet is
/// seen whole and refused rather than truncated to look valid.
const OUTCOME_BUFFER_BYTES: usize = 64;
const _: () =
    assert!(OUTCOME_BUFFER_BYTES > splinterm_protocol::handoff_outcome::HANDOFF_OUTCOME_BYTES);

/// The Window's tabs and focus as its frontend shows them now.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowLayout {
    /// Tabs in order.
    pub tabs: Vec<WindowResumeTab>,
    /// Index of the active tab.
    pub active: usize,
}

/// Where the frontend publishes its [`WindowLayout`] each event-loop turn
/// and the anchor reads it when a prepare frame arrives. Clones share it.
#[derive(Clone, Debug, Default)]
pub struct WindowLayoutLedger {
    layout: Arc<Mutex<Option<WindowLayout>>>,
}

impl WindowLayoutLedger {
    /// Replaces the published layout when it changed.
    pub fn publish(&self, layout: WindowLayout) {
        let mut current = self.layout.lock().unwrap_or_else(PoisonError::into_inner);
        if current.as_ref() != Some(&layout) {
            *current = Some(layout);
        }
    }

    /// The layout last published, if any.
    #[must_use]
    pub fn current(&self) -> Option<WindowLayout> {
        self.layout
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Why the anchor connection failed.
#[derive(Debug)]
pub enum AnchorError {
    Refused(ProtocolError),
    Closed,
    Protocol(&'static str),
    Descriptors(HandoffDescriptorError),
    Io(io::Error),
}

impl std::fmt::Display for AnchorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(error) => {
                write!(
                    formatter,
                    "splinterd refused the Window anchor: {}",
                    error.message
                )
            }
            Self::Closed => formatter.write_str("splinterd closed the Window anchor connection"),
            Self::Protocol(what) => {
                write!(
                    formatter,
                    "the Window anchor connection broke a frame: {what}"
                )
            }
            Self::Descriptors(error) => {
                write!(
                    formatter,
                    "a handoff prepare frame's descriptors were refused: {error}"
                )
            }
            Self::Io(error) => write!(formatter, "the Window anchor connection failed: {error}"),
        }
    }
}

impl std::error::Error for AnchorError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Descriptors(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::Refused(_) | Self::Closed | Self::Protocol(_) => None,
        }
    }
}

impl From<io::Error> for AnchorError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// How the daemon answered an acknowledgement.
#[derive(Debug, Eq, PartialEq)]
pub enum AcknowledgementAnswer {
    Acknowledged,
    Refused(ErrorCode),
}

/// One Window's dedicated anchor connection; see the module documentation.
#[derive(Debug)]
pub struct AnchorConnection {
    stream: UnixStream,
    daemon_generation: u64,
    next_request: u64,
}

impl AnchorConnection {
    /// Connects to the local daemon at `socket` and declares the anchor.
    ///
    /// # Errors
    /// A connection failure, or the daemon's refusal of the declaration.
    pub async fn connect(socket: &Path) -> Result<Self, AnchorError> {
        Self::handshake(UnixStream::connect(socket).await?).await
    }

    /// Declares the anchor on an already connected stream.
    ///
    /// # Errors
    /// The daemon's refusal, or a malformed handshake.
    pub async fn handshake(stream: UnixStream) -> Result<Self, AnchorError> {
        let mut anchor = Self {
            stream,
            daemon_generation: 0,
            next_request: 1,
        };
        anchor
            .write(&ClientFrame::Hello {
                minimum_version: PROTOCOL_VERSION,
                maximum_version: PROTOCOL_VERSION,
                role: ClientRole::TrustedUi,
                handoff_window: true,
            })
            .await?;
        let (frame, descriptors) = anchor.read_frame().await?;
        if descriptors.is_some() {
            return Err(AnchorError::Descriptors(
                HandoffDescriptorError::UnexpectedDescriptors,
            ));
        }
        match frame {
            ServerFrame::Hello {
                version,
                daemon_generation,
                ..
            } if version == PROTOCOL_VERSION && daemon_generation != 0 => {
                anchor.daemon_generation = daemon_generation;
                Ok(anchor)
            }
            ServerFrame::Error { error, .. } => Err(AnchorError::Refused(error)),
            _ => Err(AnchorError::Protocol("invalid handshake")),
        }
    }

    /// The generation of the daemon that accepted the anchor.
    #[must_use]
    pub const fn daemon_generation(&self) -> u64 {
        self.daemon_generation
    }

    /// Waits for the next prepare frame and returns it with its verified
    /// descriptors. Other frames an idle anchor may see are skipped; a
    /// connection-level error ends the connection.
    ///
    /// # Errors
    /// End of stream, a framing or descriptor violation, or a daemon error.
    pub async fn next_prepare(
        &mut self,
    ) -> Result<
        (
            splinterm_protocol::HandoffPrepare,
            HandoffPrepareDescriptors,
        ),
        AnchorError,
    > {
        loop {
            let (frame, descriptors) = self.read_frame().await?;
            match accept_handoff_prepare(&frame, descriptors).map_err(AnchorError::Descriptors)? {
                Some(prepared) => return Ok(prepared),
                None => {
                    if let ServerFrame::Error {
                        request_id: None,
                        error,
                    } = frame
                    {
                        return Err(AnchorError::Refused(error));
                    }
                }
            }
        }
    }

    /// Sends the acknowledgement of `prepare`, echoing its generation and
    /// prepare id; returns its request id.
    ///
    /// # Errors
    /// A write failure.
    pub async fn acknowledge(
        &mut self,
        prepare: &splinterm_protocol::HandoffPrepare,
    ) -> Result<u64, AnchorError> {
        let request_id = self.next_request;
        self.next_request += 1;
        self.write(&ClientFrame::Request {
            request_id,
            diagnostic_correlation: None,
            request: Request::AcknowledgeHandoffPrepare {
                daemon_generation: prepare.daemon_generation,
                prepare_id: prepare.prepare_id,
            },
        })
        .await?;
        Ok(request_id)
    }

    /// Reads the daemon's answer to the acknowledgement `request_id`.
    ///
    /// # Errors
    /// End of stream or a framing violation before the answer.
    pub async fn acknowledgement_answer(
        &mut self,
        request_id: u64,
    ) -> Result<AcknowledgementAnswer, AnchorError> {
        loop {
            let (frame, descriptors) = self.read_frame().await?;
            if descriptors.is_some() {
                // A second prepare while this one is answered is refused.
                return Err(AnchorError::Descriptors(
                    HandoffDescriptorError::UnexpectedDescriptors,
                ));
            }
            match frame {
                ServerFrame::Response {
                    request_id: answered,
                    result: Response::Acknowledged,
                } if answered == request_id => return Ok(AcknowledgementAnswer::Acknowledged),
                ServerFrame::Error {
                    request_id: Some(answered),
                    error,
                } if answered == request_id => {
                    return Ok(AcknowledgementAnswer::Refused(error.code));
                }
                ServerFrame::Error {
                    request_id: None,
                    error,
                } => return Err(AnchorError::Refused(error)),
                _ => {}
            }
        }
    }

    async fn write(&mut self, frame: &ClientFrame) -> Result<(), AnchorError> {
        let encoded = encode_frame(frame).map_err(|_| AnchorError::Protocol("frame encoding"))?;
        self.stream.write_all(&encoded).await?;
        Ok(())
    }

    /// Receives into `buffer` with any attached descriptors.
    async fn receive(&self, buffer: &mut [u8]) -> Result<ReceivedBytes, AnchorError> {
        loop {
            self.stream.readable().await?;
            let received = self.stream.try_io(Interest::READABLE, || {
                match recv_with_descriptors(self.stream.as_fd(), buffer) {
                    Ok(received) => Ok(Ok(received)),
                    Err(HandoffDescriptorError::Io(errno)) => {
                        Err(io::Error::from_raw_os_error(errno.raw_os_error()))
                    }
                    Err(error) => Ok(Err(error)),
                }
            });
            match received {
                Ok(Ok(received)) => return Ok(received),
                Ok(Err(error)) => return Err(AnchorError::Descriptors(error)),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(AnchorError::Io(error)),
            }
        }
    }

    /// Reads exactly one frame: its header, then exactly its body.
    /// Descriptors may arrive only with a header read (once), never with a
    /// body read.
    async fn read_frame(
        &self,
    ) -> Result<(ServerFrame, Option<HandoffPrepareDescriptors>), AnchorError> {
        let mut header = [0_u8; 4];
        let mut filled = 0;
        let mut descriptors = None;
        while filled < header.len() {
            let received = self.receive(&mut header[filled..]).await?;
            if received.bytes == 0 {
                return Err(if filled == 0 {
                    AnchorError::Closed
                } else {
                    AnchorError::Protocol("truncated frame header")
                });
            }
            if let Some(attached) = received.descriptors {
                if descriptors.is_some() {
                    return Err(AnchorError::Descriptors(
                        HandoffDescriptorError::UnexpectedAncillary,
                    ));
                }
                descriptors = Some(attached);
            }
            filled += received.bytes;
        }
        let length = usize::try_from(u32::from_be_bytes(header))
            .map_err(|_| AnchorError::Protocol("frame length"))?;
        if length == 0 || length > MAX_ANCHOR_FRAME_BYTES {
            return Err(AnchorError::Protocol("frame length out of bounds"));
        }
        let mut body = vec![0_u8; length];
        let mut filled = 0;
        while filled < length {
            let received = self.receive(&mut body[filled..]).await?;
            if received.descriptors.is_some() {
                return Err(AnchorError::Descriptors(
                    HandoffDescriptorError::UnexpectedDescriptors,
                ));
            }
            if received.bytes == 0 {
                return Err(AnchorError::Protocol("truncated frame body"));
            }
            filled += received.bytes;
        }
        let frame = serde_json::from_slice(&body)
            .map_err(|_| AnchorError::Protocol("undecodable frame"))?;
        Ok((frame, descriptors))
    }
}

/// The image a Window relaunches into.
#[derive(Clone, Copy, Debug)]
pub enum RelaunchImage<'a> {
    /// The sealed forward client snapshot, after an adoption.
    Snapshot(BorrowedFd<'a>),
    /// The installed `<prefix>/splinterm`, after a rollback.
    Installed(&'a Path),
}

/// Replaces the process image; injectable so tests observe the relaunch
/// without performing it.
pub trait ContinuationExec: Send + Sync {
    /// Executes `image` with `arguments` and `environment`, inheriting
    /// exactly `inherited`. Returns only when the relaunch failed, with
    /// every descriptor flag as it was.
    fn relaunch(
        &self,
        image: RelaunchImage<'_>,
        inherited: [BorrowedFd<'_>; 2],
        arguments: &[CString],
        environment: &[CString],
    ) -> io::Error;
}

/// The real relaunch: `execveat(AT_EMPTY_PATH)` of the snapshot, or `execve`
/// of the installed client.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemExec;

impl ContinuationExec for SystemExec {
    fn relaunch(
        &self,
        image: RelaunchImage<'_>,
        inherited: [BorrowedFd<'_>; 2],
        arguments: &[CString],
        environment: &[CString],
    ) -> io::Error {
        let prepared = match ClientInheritance::prepare(inherited) {
            Ok(prepared) => prepared,
            Err(error) => return error,
        };
        // ---- The Window relaunch exec site (Plan 0061 M3.14) ----
        // Reached only after a continuation outcome, with child spawns paused
        // and idle, and close-on-exec cleared on exactly the resume record and
        // the continuation (`ClientInheritance`). It returns only on failure.
        let error: io::Error = match image {
            RelaunchImage::Snapshot(snapshot) => match nix::unistd::execveat(
                snapshot,
                c"",
                arguments,
                environment,
                nix::fcntl::AtFlags::AT_EMPTY_PATH,
            ) {
                Err(errno) => errno.into(),
                Ok(never) => match never {},
            },
            RelaunchImage::Installed(path) => match CString::new(path.as_os_str().as_bytes()) {
                Ok(path) => match nix::unistd::execve(&path, arguments, environment) {
                    Err(errno) => errno.into(),
                    Ok(never) => match never {},
                },
                Err(error) => io::Error::new(io::ErrorKind::InvalidInput, error),
            },
        };
        prepared.restore();
        error
    }
}

/// Close-on-exec cleared on exactly the two inherited descriptors, with
/// their previous flags kept for a failed exec.
struct ClientInheritance<'a> {
    restore: Vec<(BorrowedFd<'a>, FdFlags)>,
}

impl<'a> ClientInheritance<'a> {
    fn prepare(inherited: [BorrowedFd<'a>; 2]) -> io::Result<Self> {
        if inherited[0].as_raw_fd() == inherited[1].as_raw_fd() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the resume record and continuation must be distinct descriptors",
            ));
        }
        let mut prepared = Self {
            restore: Vec::with_capacity(2),
        };
        for descriptor in inherited {
            let flags = fcntl_getfd(descriptor)?;
            fcntl_setfd(descriptor, flags - FdFlags::CLOEXEC)?;
            prepared.restore.push((descriptor, flags));
        }
        Ok(prepared)
    }

    fn restore(self) {
        drop(self);
    }
}

impl Drop for ClientInheritance<'_> {
    fn drop(&mut self) {
        for (descriptor, flags) in self.restore.drain(..) {
            let _ = fcntl_setfd(descriptor, flags);
        }
    }
}

/// The installed client this Window runs from: the canonical path of the
/// running executable, when it is named `splinterm` and still names the
/// running image itself. Captured when the Window starts, before an upgrade
/// can replace the file. `None` for a Window running from a memfd (one
/// already relaunched across an adoption) or from a replaced file.
#[must_use]
pub fn installed_client_path() -> Option<PathBuf> {
    let running = fs::metadata("/proc/self/exe").ok()?;
    let path = fs::canonicalize(std::env::current_exe().ok()?).ok()?;
    if path.file_name() != Some(OsStr::new("splinterm")) {
        return None;
    }
    let installed = fs::metadata(&path).ok()?;
    (installed.is_file() && installed.dev() == running.dev() && installed.ino() == running.ino())
        .then_some(path)
}

/// Everything a Window's anchor needs to take part in a handoff.
#[derive(Clone)]
pub struct ContinuationContext {
    pub hub: HandoffStatusHub,
    pub ledger: WindowLayoutLedger,
    /// The Window's application id when it is not the default.
    pub app_id: Option<String>,
    /// Where a rollback relaunches from; see [`installed_client_path`].
    pub installed_client: Option<PathBuf>,
    pub exec: Arc<dyn ContinuationExec>,
}

impl std::fmt::Debug for ContinuationContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ContinuationContext")
            .field("app_id", &self.app_id)
            .field("installed_client", &self.installed_client)
            .finish_non_exhaustive()
    }
}

/// How one prepare frame ended without a relaunch.
#[derive(Debug)]
pub enum ParticipationEnd {
    /// The Window did not acknowledge: the frame named another generation,
    /// no layout or record could be made, or the Window is lost.
    Declined(&'static str),
    /// The Window acknowledged but the attempt ended without an outcome it
    /// relaunches for: refused acknowledgement, end of file, a malformed or
    /// mismatched outcome, or no outcome in time.
    Ended(&'static str),
    /// An outcome said to relaunch, but the relaunch failed.
    RelaunchFailed(io::Error),
}

/// What the continuation delivered.
#[derive(Debug)]
enum OutcomeRead {
    Outcome(HandoffOutcome),
    Closed,
    Malformed,
    TimedOut,
    Failed,
}

async fn read_outcome(continuation: OwnedFd, deadline: Duration) -> (OutcomeRead, OwnedFd) {
    if rustix::io::ioctl_fionbio(&continuation, true).is_err() {
        return (OutcomeRead::Failed, continuation);
    }
    let channel = match AsyncFd::try_with_interest(continuation, Interest::READABLE) {
        Ok(channel) => channel,
        Err(error) => return (OutcomeRead::Failed, error.into_parts().0),
    };
    let read = tokio::time::timeout(deadline, async {
        let mut buffer = [0_u8; OUTCOME_BUFFER_BYTES];
        loop {
            let Ok(mut ready) = channel.readable().await else {
                return OutcomeRead::Failed;
            };
            match ready.try_io(|inner| {
                rustix::net::recv(inner.get_ref(), &mut buffer, RecvFlags::empty())
                    .map_err(io::Error::from)
            }) {
                Ok(Ok((0, _))) => return OutcomeRead::Closed,
                Ok(Ok((bytes, _))) => {
                    let bytes = bytes.min(buffer.len());
                    return HandoffOutcome::decode(&buffer[..bytes])
                        .map_or(OutcomeRead::Malformed, OutcomeRead::Outcome);
                }
                Ok(Err(_)) => return OutcomeRead::Failed,
                Err(_would_block) => {}
            }
        }
    })
    .await
    .unwrap_or(OutcomeRead::TimedOut);
    let continuation = channel.into_inner();
    let _ = rustix::io::ioctl_fionbio(&continuation, false);
    (read, continuation)
}

/// Takes part in the handoff `prepare` announced, through to the relaunch;
/// returns only when the Window keeps running this image.
pub async fn participate(
    context: &ContinuationContext,
    anchor: &mut AnchorConnection,
    prepare: splinterm_protocol::HandoffPrepare,
    descriptors: HandoffPrepareDescriptors,
) -> ParticipationEnd {
    if prepare.daemon_generation != anchor.daemon_generation() {
        return ParticipationEnd::Declined("the prepare frame names another daemon generation");
    }
    let Some(layout) = context.ledger.current() else {
        return ParticipationEnd::Declined("the Window has no tabs to record yet");
    };
    let record = WindowResumeRecord {
        daemon_generation: prepare.daemon_generation,
        prepare_id: prepare.prepare_id,
        active: layout.active,
        tabs: layout.tabs,
        app_id: context.app_id.clone().filter(|app_id| valid_app_id(app_id)),
    };
    let Ok(sealed) = seal_window_resume_record(&record) else {
        return ParticipationEnd::Declined("the resume record could not be sealed");
    };
    let Some(participation) = context.hub.begin_continuation() else {
        return ParticipationEnd::Declined("the Window is lost or already takes part");
    };
    let spawns = child_spawn::pause();
    let Ok(request_id) = anchor.acknowledge(&prepare).await else {
        return ParticipationEnd::Ended("the acknowledgement could not be sent");
    };
    match tokio::time::timeout(
        ACKNOWLEDGEMENT_RESPONSE_DEADLINE,
        anchor.acknowledgement_answer(request_id),
    )
    .await
    {
        Ok(Ok(AcknowledgementAnswer::Refused(_))) => {
            return ParticipationEnd::Ended("splinterd no longer waited for this Window");
        }
        // Acknowledged, or no answer: the daemon may already have moved on
        // to its exec. The continuation tells.
        Ok(Ok(AcknowledgementAnswer::Acknowledged) | Err(_)) | Err(_) => {}
    }
    let (snapshot, continuation) = descriptors.into_parts();
    let (outcome, continuation) = read_outcome(continuation, HANDOFF_OUTCOME_DEADLINE).await;
    let image = match outcome {
        OutcomeRead::Outcome(HandoffOutcome {
            kind: HandoffOutcomeKind::Adopted,
            daemon_generation,
        }) if prepare.daemon_generation.checked_add(1) == Some(daemon_generation) => {
            Relaunch::Snapshot(snapshot)
        }
        OutcomeRead::Outcome(HandoffOutcome {
            kind: HandoffOutcomeKind::RolledBack,
            daemon_generation,
        }) if daemon_generation == prepare.daemon_generation => {
            // The forward snapshot is discarded: a rollback daemon trusts only
            // the installed client.
            drop(snapshot);
            match context.installed_client.clone() {
                Some(path) => Relaunch::Installed(path),
                None => {
                    return ParticipationEnd::RelaunchFailed(io::Error::new(
                        io::ErrorKind::NotFound,
                        "this Window does not know its installed client",
                    ));
                }
            }
        }
        OutcomeRead::Outcome(_) => {
            return ParticipationEnd::Ended("the outcome names an unexpected generation");
        }
        OutcomeRead::Closed => {
            return ParticipationEnd::Ended("the attempt ended without an outcome");
        }
        OutcomeRead::Malformed => {
            return ParticipationEnd::Ended("the outcome packet is malformed");
        }
        OutcomeRead::TimedOut => return ParticipationEnd::Ended("no outcome arrived in time"),
        OutcomeRead::Failed => return ParticipationEnd::Ended("the continuation failed"),
    };
    let exec = Arc::clone(&context.exec);
    let failed = tokio::task::spawn_blocking(move || {
        relaunch(
            exec.as_ref(),
            &image,
            &sealed,
            &continuation,
            &spawns,
            &participation,
        )
    })
    .await
    .unwrap_or_else(|_| io::Error::other("the relaunch task failed"));
    ParticipationEnd::RelaunchFailed(failed)
}

/// What a relaunch executes, owned.
enum Relaunch {
    Snapshot(OwnedFd),
    Installed(PathBuf),
}

/// The argv a relaunched Window receives: the program name and the two
/// activation flags; nothing from the current argv.
fn relaunch_arguments(record: &OwnedFd, continuation: &OwnedFd) -> Vec<CString> {
    let [resume, continuation] =
        window_resume_arguments(record.as_raw_fd(), continuation.as_raw_fd());
    [c"splinterm".to_owned()]
        .into_iter()
        .chain(
            [resume, continuation]
                .map(|flag| CString::new(flag).expect("activation flags hold no NUL byte")),
        )
        .collect()
}

/// The environment, unchanged. Environment entries can hold no NUL byte.
fn relaunch_environment() -> Vec<CString> {
    std::env::vars_os()
        .filter_map(|(key, value)| {
            let mut entry = key.into_vec();
            entry.push(b'=');
            entry.extend_from_slice(value.as_bytes());
            CString::new(entry).ok()
        })
        .collect()
}

/// Waits for spawns under way, then replaces the image. Returns only on
/// failure; the participation guard and spawn pause stay held until the
/// caller drops them.
fn relaunch(
    exec: &dyn ContinuationExec,
    image: &Relaunch,
    record: &OwnedFd,
    continuation: &OwnedFd,
    spawns: &SpawnPause,
    _participation: &ContinuationParticipation,
) -> io::Error {
    if !spawns.wait_idle(SPAWN_IDLE_DEADLINE) {
        return io::Error::new(
            io::ErrorKind::TimedOut,
            "a child process was still being started; the Window does not relaunch",
        );
    }
    let arguments = relaunch_arguments(record, continuation);
    let environment = relaunch_environment();
    let image = match image {
        Relaunch::Snapshot(snapshot) => RelaunchImage::Snapshot(snapshot.as_fd()),
        Relaunch::Installed(path) => RelaunchImage::Installed(path),
    };
    exec.relaunch(
        image,
        [record.as_fd(), continuation.as_fd()],
        &arguments,
        &environment,
    )
}

/// Runs a Window's anchor for the Window's life: connects, declares, waits
/// for prepare frames, and takes part in each handoff. It reconnects after a
/// lost connection, and stops when the daemon refuses this client as
/// untrusted (as after an upgrade replaced it) or the Window is lost.
pub async fn run_window_anchor(context: ContinuationContext, socket: PathBuf) {
    loop {
        if context.hub.view().phase == crate::handoff_status::WindowHandoffPhase::Lost {
            return;
        }
        let mut anchor = match AnchorConnection::connect(&socket).await {
            Ok(anchor) => anchor,
            Err(AnchorError::Refused(error)) if error.code == ErrorCode::Unauthorized => return,
            Err(_) => {
                tokio::time::sleep(ANCHOR_RETRY_DELAY).await;
                continue;
            }
        };
        // A lost or refused anchor connection, or a refused prepare frame,
        // ends this connection; the Window declares a new anchor.
        while let Ok((prepare, descriptors)) = anchor.next_prepare().await {
            match participate(&context, &mut anchor, prepare, descriptors).await {
                ParticipationEnd::Declined(_) | ParticipationEnd::Ended(_) => {}
                ParticipationEnd::RelaunchFailed(error) => {
                    eprintln!("splinterm: could not relaunch this Window for the upgrade: {error}");
                }
            }
        }
        tokio::time::sleep(ANCHOR_RETRY_DELAY).await;
    }
}

#[cfg(test)]
mod tests;
