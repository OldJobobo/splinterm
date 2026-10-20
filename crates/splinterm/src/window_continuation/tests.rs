//! The anchor receiver, the acknowledgement, and outcome handling, against a
//! stand-in daemon on a socket pair. The relaunch itself is injected
//! ([`ContinuationExec`]), so no test here replaces its process image;
//! `tests/window_resume_exec.rs` performs real ones.

use std::{
    io::{IoSlice, Read as _, Write as _},
    mem::MaybeUninit,
    os::unix::net::UnixStream as StdUnixStream,
    sync::Mutex,
    thread,
};

use rustix::{
    fs::{MemfdFlags, fcntl_add_seals, fstat, memfd_create},
    net::{
        AddressFamily, SendAncillaryBuffer, SendAncillaryMessage, SendFlags, SocketFlags,
        SocketType, sendmsg, socketpair,
    },
};
use splinterm_core::{DojoId, LairId, SplintId};
use splinterm_protocol::{
    HANDOFF_PREPARE_DESCRIPTORS, HandoffClientSnapshot, HandoffPrepare, ServerLimits,
    handoff_prepare::{
        REQUIRED_CLIENT_SNAPSHOT_SEALS, encode_handoff_prepare, send_handoff_prepare,
    },
    window_resume::read_window_resume_record,
};

use super::*;
use crate::handoff_status::WindowHandoffPhase;

const GENERATION: u64 = 4;
const PREPARE_ID: u64 = 17;

fn sealed_snapshot() -> OwnedFd {
    let fd = memfd_create(
        "client-snapshot",
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )
    .unwrap();
    let bytes = b"\x7fELF stand-in forward client";
    assert_eq!(rustix::io::write(&fd, bytes).unwrap(), bytes.len());
    fcntl_add_seals(&fd, REQUIRED_CLIENT_SNAPSHOT_SEALS).unwrap();
    fd
}

fn prepare_for(snapshot: &OwnedFd, generation: u64) -> HandoffPrepare {
    let stat = fstat(snapshot).unwrap();
    HandoffPrepare {
        daemon_generation: generation,
        prepare_id: PREPARE_ID,
        descriptors: HANDOFF_PREPARE_DESCRIPTORS,
        client_snapshot: HandoffClientSnapshot {
            device: stat.st_dev,
            inode: stat.st_ino,
            size: u64::try_from(stat.st_size).unwrap(),
            sha256: [9; 32],
        },
    }
}

fn continuation_pair() -> (OwnedFd, OwnedFd) {
    socketpair(
        AddressFamily::UNIX,
        SocketType::SEQPACKET,
        SocketFlags::CLOEXEC,
        None,
    )
    .unwrap()
}

fn write_frame(stream: &mut StdUnixStream, frame: &ServerFrame) {
    stream.write_all(&encode_frame(frame).unwrap()).unwrap();
}

fn read_client(stream: &mut StdUnixStream) -> Option<ClientFrame> {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length).ok()?;
    let mut body = vec![0_u8; u32::from_be_bytes(length) as usize];
    stream.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn hello(generation: u64) -> ServerFrame {
    ServerFrame::Hello {
        version: PROTOCOL_VERSION,
        limits: ServerLimits::default(),
        development_terminal_access: false,
        daemon_hostname: None,
        daemon_generation: generation,
    }
}

fn pong(request_id: u64) -> ServerFrame {
    ServerFrame::Response {
        request_id,
        result: Response::Pong,
    }
}

fn send_rights(stream: &StdUnixStream, bytes: &[u8], rights: &[BorrowedFd<'_>]) {
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(8))];
    let mut ancillary = SendAncillaryBuffer::new(&mut space);
    assert!(ancillary.push(SendAncillaryMessage::ScmRights(rights)));
    assert_eq!(
        sendmsg(
            stream,
            &[IoSlice::new(bytes)],
            &mut ancillary,
            SendFlags::NOSIGNAL
        )
        .unwrap(),
        bytes.len()
    );
}

/// The client end as a Tokio stream and the daemon end as a blocking one.
fn stream_pair() -> (UnixStream, StdUnixStream) {
    let (client, daemon) = StdUnixStream::pair().unwrap();
    client.set_nonblocking(true).unwrap();
    (UnixStream::from_std(client).unwrap(), daemon)
}

/// The daemon side of the handshake: reads the anchor's Hello, checks it
/// declares the anchor as trusted UI, and answers at `generation`.
fn accept_anchor(daemon: &mut StdUnixStream, generation: u64) {
    let Some(ClientFrame::Hello {
        role,
        handoff_window,
        ..
    }) = read_client(daemon)
    else {
        panic!("the anchor sends Hello first");
    };
    assert_eq!(role, ClientRole::TrustedUi);
    assert!(handoff_window, "the anchor declares itself");
    write_frame(daemon, &hello(generation));
}

#[tokio::test]
async fn a_prepare_after_a_busy_stream_arrives_with_its_descriptors_and_is_acknowledged() {
    let (client, mut daemon) = stream_pair();
    let snapshot = sealed_snapshot();
    let snapshot_identity = {
        let stat = fstat(&snapshot).unwrap();
        (stat.st_dev, stat.st_ino)
    };
    let prepare = prepare_for(&snapshot, GENERATION);
    let server = thread::spawn(move || {
        accept_anchor(&mut daemon, GENERATION);
        // Ordinary frames first: several in one write, then one alone, then
        // one written in pieces, so frames and reads never line up.
        let mut batch = Vec::new();
        for request_id in 1..=5 {
            batch.extend(encode_frame(&pong(request_id)).unwrap());
        }
        daemon.write_all(&batch).unwrap();
        write_frame(&mut daemon, &pong(6));
        let split = encode_frame(&pong(7)).unwrap();
        daemon.write_all(&split[..3]).unwrap();
        daemon.write_all(&split[3..]).unwrap();
        let (continuation, _daemon_end) = continuation_pair();
        let encoded = encode_handoff_prepare(&prepare).unwrap();
        let descriptors = HandoffPrepareDescriptors::new(snapshot, continuation);
        assert_eq!(
            send_handoff_prepare(daemon.as_fd(), &encoded, &descriptors).unwrap(),
            encoded.len()
        );
        drop(descriptors);
        // An ordinary frame right behind it never carries its descriptors.
        write_frame(&mut daemon, &pong(8));
        let Some(ClientFrame::Request {
            request_id,
            request:
                Request::AcknowledgeHandoffPrepare {
                    daemon_generation,
                    prepare_id,
                },
            ..
        }) = read_client(&mut daemon)
        else {
            panic!("the anchor acknowledges");
        };
        assert_eq!((daemon_generation, prepare_id), (GENERATION, PREPARE_ID));
        write_frame(
            &mut daemon,
            &ServerFrame::Response {
                request_id,
                result: Response::Acknowledged,
            },
        );
        daemon
    });
    let mut anchor = AnchorConnection::handshake(client).await.unwrap();
    assert_eq!(anchor.daemon_generation(), GENERATION);
    let (received, descriptors) = anchor.next_prepare().await.unwrap();
    assert_eq!(received, prepare);
    let stat = fstat(descriptors.client_snapshot()).unwrap();
    assert_eq!((stat.st_dev, stat.st_ino), snapshot_identity);
    assert!(
        fcntl_getfd(descriptors.continuation())
            .unwrap()
            .contains(FdFlags::CLOEXEC)
    );
    let request_id = anchor.acknowledge(&received).await.unwrap();
    assert_eq!(
        anchor.acknowledgement_answer(request_id).await.unwrap(),
        AcknowledgementAnswer::Acknowledged
    );
    drop(server.join().unwrap());
    assert!(matches!(
        anchor.next_prepare().await,
        Err(AnchorError::Closed)
    ));
}

#[tokio::test]
async fn a_prepare_without_exactly_two_descriptors_is_refused() {
    // Missing: the frame written with no descriptors at all.
    let (client, mut daemon) = stream_pair();
    let snapshot = sealed_snapshot();
    let encoded = encode_handoff_prepare(&prepare_for(&snapshot, GENERATION)).unwrap();
    let frame = encoded.clone();
    let server = thread::spawn(move || {
        accept_anchor(&mut daemon, GENERATION);
        daemon.write_all(&frame).unwrap();
        daemon
    });
    let mut anchor = AnchorConnection::handshake(client).await.unwrap();
    assert!(matches!(
        anchor.next_prepare().await,
        Err(AnchorError::Descriptors(
            HandoffDescriptorError::MissingDescriptors
        ))
    ));
    drop(server.join().unwrap());

    // One too few, and one too many.
    for count in [1, 3] {
        let (client, mut daemon) = stream_pair();
        let snapshot = sealed_snapshot();
        let frame = encoded.clone();
        let server = thread::spawn(move || {
            accept_anchor(&mut daemon, GENERATION);
            let (continuation, extra) = continuation_pair();
            let rights = [snapshot.as_fd(), continuation.as_fd(), extra.as_fd()];
            send_rights(&daemon, &frame, &rights[..count]);
            daemon
        });
        let mut anchor = AnchorConnection::handshake(client).await.unwrap();
        assert!(
            matches!(
                anchor.next_prepare().await,
                Err(AnchorError::Descriptors(
                    HandoffDescriptorError::Count { .. } | HandoffDescriptorError::Truncated
                ))
            ),
            "{count} descriptors"
        );
        drop(server.join().unwrap());
    }
}

#[tokio::test]
async fn descriptors_with_a_body_read_or_beside_an_ordinary_frame_are_refused() {
    let (client, mut daemon) = stream_pair();
    let server = thread::spawn(move || {
        accept_anchor(&mut daemon, GENERATION);
        let frame = encode_frame(&pong(1)).unwrap();
        daemon.write_all(&frame[..4]).unwrap();
        let (one, two) = continuation_pair();
        send_rights(&daemon, &frame[4..], &[one.as_fd(), two.as_fd()]);
        daemon
    });
    let mut anchor = AnchorConnection::handshake(client).await.unwrap();
    assert!(matches!(
        anchor.next_prepare().await,
        Err(AnchorError::Descriptors(
            HandoffDescriptorError::UnexpectedDescriptors
        ))
    ));
    drop(server.join().unwrap());

    let (client, mut daemon) = stream_pair();
    let server = thread::spawn(move || {
        accept_anchor(&mut daemon, GENERATION);
        let (one, two) = continuation_pair();
        send_rights(
            &daemon,
            &encode_frame(&pong(1)).unwrap(),
            &[one.as_fd(), two.as_fd()],
        );
        daemon
    });
    let mut anchor = AnchorConnection::handshake(client).await.unwrap();
    assert!(matches!(
        anchor.next_prepare().await,
        Err(AnchorError::Descriptors(
            HandoffDescriptorError::UnexpectedDescriptors
        ))
    ));
    drop(server.join().unwrap());
}

#[tokio::test]
async fn a_refused_declaration_or_an_oversized_frame_ends_the_anchor() {
    let (client, mut daemon) = stream_pair();
    let server = thread::spawn(move || {
        let _ = read_client(&mut daemon);
        write_frame(
            &mut daemon,
            &ServerFrame::Error {
                request_id: None,
                error: ProtocolError::new(ErrorCode::Unauthorized, "not the installed client"),
            },
        );
        daemon
    });
    assert!(matches!(
        AnchorConnection::handshake(client).await,
        Err(AnchorError::Refused(error)) if error.code == ErrorCode::Unauthorized
    ));
    drop(server.join().unwrap());

    let (client, mut daemon) = stream_pair();
    let server = thread::spawn(move || {
        accept_anchor(&mut daemon, GENERATION);
        let length = u32::try_from(MAX_ANCHOR_FRAME_BYTES + 1).unwrap();
        daemon.write_all(&length.to_be_bytes()).unwrap();
        daemon
    });
    let mut anchor = AnchorConnection::handshake(client).await.unwrap();
    assert!(matches!(
        anchor.next_prepare().await,
        Err(AnchorError::Protocol(_))
    ));
    drop(server.join().unwrap());
}

/// One relaunch the stand-in exec observed.
#[derive(Debug)]
struct Recorded {
    snapshot: Option<(u64, u64)>,
    installed: Option<PathBuf>,
    arguments: Vec<String>,
    inherited: [i32; 2],
    record: WindowResumeRecord,
    hub_phase: WindowHandoffPhase,
    spawning_paused: bool,
}

struct RecordingExec {
    hub: HandoffStatusHub,
    recorded: Mutex<Vec<Recorded>>,
}

impl ContinuationExec for RecordingExec {
    fn relaunch(
        &self,
        image: RelaunchImage<'_>,
        inherited: [BorrowedFd<'_>; 2],
        arguments: &[CString],
        environment: &[CString],
    ) -> io::Error {
        assert!(!environment.is_empty(), "the environment is passed on");
        let (snapshot, installed) = match image {
            RelaunchImage::Snapshot(snapshot) => {
                let stat = fstat(snapshot).unwrap();
                (Some((stat.st_dev, stat.st_ino)), None)
            }
            RelaunchImage::Installed(path) => (None, Some(path.to_owned())),
        };
        self.recorded.lock().unwrap().push(Recorded {
            snapshot,
            installed,
            arguments: arguments
                .iter()
                .map(|argument| argument.to_str().unwrap().to_owned())
                .collect(),
            inherited: inherited.map(|descriptor| descriptor.as_raw_fd()),
            record: read_window_resume_record(inherited[0]).unwrap(),
            hub_phase: self.hub.view().phase,
            spawning_paused: child_spawn::spawn(|| Ok(())).is_err(),
        });
        io::Error::other("recorded instead of relaunching")
    }
}

/// What the stand-in daemon does after the prepare frame.
#[derive(Clone, Copy)]
enum Script {
    /// Acknowledges and sends these outcome bytes.
    Outcome(&'static [u8]),
    /// Acknowledges, then closes the continuation without an outcome.
    Close,
    /// Refuses the acknowledgement as no longer waited for.
    RefuseAcknowledgement,
    /// Sends a prepare frame naming another generation.
    WrongGeneration,
}

fn outcome(kind: HandoffOutcomeKind, daemon_generation: u64) -> &'static [u8] {
    Box::leak(Box::new(
        HandoffOutcome {
            kind,
            daemon_generation,
        }
        .encode(),
    ))
}

fn layout() -> WindowLayout {
    WindowLayout {
        tabs: (0..3)
            .map(|index| WindowResumeTab {
                lair_id: LairId::new(),
                dojo_id: DojoId::new(),
                focus: (index != 1).then(SplintId::new),
            })
            .collect(),
        active: 2,
    }
}

struct Participation {
    end: ParticipationEnd,
    recorded: Vec<Recorded>,
    hub: HandoffStatusHub,
    snapshot: (u64, u64),
    layout: WindowLayout,
}

async fn participate_with(script: Script) -> Participation {
    let (client, mut daemon) = stream_pair();
    let snapshot = sealed_snapshot();
    let identity = {
        let stat = fstat(&snapshot).unwrap();
        (stat.st_dev, stat.st_ino)
    };
    let prepare = prepare_for(
        &snapshot,
        if matches!(script, Script::WrongGeneration) {
            GENERATION - 1
        } else {
            GENERATION
        },
    );
    let server = thread::spawn(move || {
        accept_anchor(&mut daemon, GENERATION);
        let (continuation, daemon_end) = continuation_pair();
        let encoded = encode_handoff_prepare(&prepare).unwrap();
        let descriptors = HandoffPrepareDescriptors::new(snapshot, continuation);
        send_handoff_prepare(daemon.as_fd(), &encoded, &descriptors).unwrap();
        drop(descriptors);
        if matches!(script, Script::WrongGeneration) {
            return daemon;
        }
        let Some(ClientFrame::Request {
            request_id,
            request: Request::AcknowledgeHandoffPrepare { prepare_id, .. },
            ..
        }) = read_client(&mut daemon)
        else {
            panic!("the anchor acknowledges");
        };
        assert_eq!(prepare_id, PREPARE_ID);
        if matches!(script, Script::RefuseAcknowledgement) {
            write_frame(
                &mut daemon,
                &ServerFrame::Error {
                    request_id: Some(request_id),
                    error: ProtocolError::new(ErrorCode::NotFound, "no pending prepare"),
                },
            );
            return daemon;
        }
        write_frame(
            &mut daemon,
            &ServerFrame::Response {
                request_id,
                result: Response::Acknowledged,
            },
        );
        if let Script::Outcome(bytes) = script {
            rustix::net::send(&daemon_end, bytes, SendFlags::empty()).unwrap();
            // Keep the channel open until the Window has read the packet.
            let mut wait = [0_u8; 1];
            let _ = rustix::net::recv(&daemon_end, &mut wait, rustix::net::RecvFlags::empty());
        }
        daemon
    });
    let hub = HandoffStatusHub::default();
    let ledger = WindowLayoutLedger::default();
    let layout = layout();
    ledger.publish(layout.clone());
    let exec = Arc::new(RecordingExec {
        hub: hub.clone(),
        recorded: Mutex::new(Vec::new()),
    });
    let context = ContinuationContext {
        hub: hub.clone(),
        ledger,
        app_id: Some("org.example.window".to_owned()),
        installed_client: Some(PathBuf::from("/usr/bin/splinterm")),
        exec: exec.clone(),
    };
    let mut anchor = AnchorConnection::handshake(client).await.unwrap();
    let (prepare, descriptors) = anchor.next_prepare().await.unwrap();
    let end = participate(&context, &mut anchor, prepare, descriptors).await;
    drop(anchor);
    drop(server.join().unwrap());
    let recorded = std::mem::take(&mut *exec.recorded.lock().unwrap());
    Participation {
        end,
        recorded,
        hub,
        snapshot: identity,
        layout,
    }
}

fn assert_back_on_the_ordinary_path(participation: &Participation) {
    assert!(!participation.hub.continuing());
    assert!(
        child_spawn::spawn(|| Ok(())).is_ok(),
        "spawning resumes once the participation ends"
    );
}

#[tokio::test]
async fn adopted_relaunches_the_sealed_snapshot_with_the_record_and_continuation() {
    let participation = participate_with(Script::Outcome(outcome(
        HandoffOutcomeKind::Adopted,
        GENERATION + 1,
    )))
    .await;
    assert!(matches!(
        participation.end,
        ParticipationEnd::RelaunchFailed(_)
    ));
    let [recorded] = participation.recorded.as_slice() else {
        panic!("exactly one relaunch: {:?}", participation.recorded);
    };
    assert_eq!(recorded.snapshot, Some(participation.snapshot));
    assert_eq!(recorded.installed, None);
    let [resume, continuation] = recorded.inherited;
    assert_eq!(
        recorded.arguments,
        [
            "splinterm".to_owned(),
            format!("--splinterm-internal-window-resume-fd-v1={resume}"),
            format!("--splinterm-internal-continuation-fd-v1={continuation}"),
        ]
    );
    assert_eq!(recorded.record.daemon_generation, GENERATION);
    assert_eq!(recorded.record.prepare_id, PREPARE_ID);
    assert_eq!(recorded.record.tabs, participation.layout.tabs);
    assert_eq!(recorded.record.active, participation.layout.active);
    assert_eq!(
        recorded.record.app_id.as_deref(),
        Some("org.example.window")
    );
    assert_eq!(recorded.hub_phase, WindowHandoffPhase::Continuing);
    assert!(recorded.spawning_paused);
    // The relaunch failed here, so the Window falls back.
    assert_back_on_the_ordinary_path(&participation);
}

#[tokio::test]
async fn rolled_back_discards_the_snapshot_and_relaunches_the_installed_client() {
    let participation = participate_with(Script::Outcome(outcome(
        HandoffOutcomeKind::RolledBack,
        GENERATION,
    )))
    .await;
    let [recorded] = participation.recorded.as_slice() else {
        panic!("exactly one relaunch: {:?}", participation.recorded);
    };
    assert_eq!(recorded.snapshot, None);
    assert_eq!(
        recorded.installed.as_deref(),
        Some(Path::new("/usr/bin/splinterm"))
    );
    assert_eq!(recorded.arguments.len(), 3);
    assert_back_on_the_ordinary_path(&participation);
}

#[tokio::test]
async fn end_of_file_garbage_or_a_mismatched_generation_falls_back_without_a_relaunch() {
    let mut garbage = HandoffOutcome {
        kind: HandoffOutcomeKind::Adopted,
        daemon_generation: GENERATION + 1,
    }
    .encode()
    .to_vec();
    garbage.push(0);
    let garbage: &'static [u8] = Box::leak(garbage.into_boxed_slice());
    for (script, expected) in [
        (Script::Close, "the attempt ended without an outcome"),
        (Script::Outcome(garbage), "the outcome packet is malformed"),
        (
            Script::Outcome(b"not an outcome at all"),
            "the outcome packet is malformed",
        ),
        (
            Script::Outcome(outcome(HandoffOutcomeKind::Adopted, GENERATION)),
            "the outcome names an unexpected generation",
        ),
        (
            Script::Outcome(outcome(HandoffOutcomeKind::RolledBack, GENERATION + 1)),
            "the outcome names an unexpected generation",
        ),
        (
            Script::RefuseAcknowledgement,
            "splinterd no longer waited for this Window",
        ),
    ] {
        let participation = participate_with(script).await;
        assert!(
            matches!(participation.end, ParticipationEnd::Ended(reason) if reason == expected),
            "{expected}: {:?}",
            participation.end
        );
        assert!(participation.recorded.is_empty(), "{expected}");
        assert_back_on_the_ordinary_path(&participation);
    }
}

#[tokio::test]
async fn a_prepare_from_another_generation_is_never_acknowledged() {
    let participation = participate_with(Script::WrongGeneration).await;
    assert!(matches!(participation.end, ParticipationEnd::Declined(_)));
    assert!(participation.recorded.is_empty());
    assert_eq!(participation.hub.view().phase, WindowHandoffPhase::Idle);
}

#[test]
fn client_inheritance_clears_close_on_exec_on_exactly_two_and_restores_it() {
    let (record, continuation) = continuation_pair();
    let other = std::fs::File::open("/dev/null").unwrap();
    let prepared = ClientInheritance::prepare([record.as_fd(), continuation.as_fd()]).unwrap();
    assert!(!fcntl_getfd(&record).unwrap().contains(FdFlags::CLOEXEC));
    assert!(
        !fcntl_getfd(&continuation)
            .unwrap()
            .contains(FdFlags::CLOEXEC)
    );
    assert!(fcntl_getfd(&other).unwrap().contains(FdFlags::CLOEXEC));
    prepared.restore();
    assert!(fcntl_getfd(&record).unwrap().contains(FdFlags::CLOEXEC));
    assert!(
        fcntl_getfd(&continuation)
            .unwrap()
            .contains(FdFlags::CLOEXEC)
    );
    assert!(ClientInheritance::prepare([record.as_fd(), record.as_fd()]).is_err());
}

#[test]
fn the_ledger_keeps_the_last_layout() {
    let ledger = WindowLayoutLedger::default();
    assert_eq!(ledger.current(), None);
    let first = layout();
    ledger.clone().publish(first.clone());
    assert_eq!(ledger.current(), Some(first));
    let second = layout();
    ledger.publish(second.clone());
    assert_eq!(ledger.current(), Some(second));
}

#[test]
fn relaunch_argv_is_rebuilt_and_the_environment_is_kept() {
    let (record, continuation) = continuation_pair();
    let arguments = relaunch_arguments(&record, &continuation);
    assert_eq!(arguments.len(), 3);
    assert_eq!(arguments[0].as_bytes(), b"splinterm");
    let environment = relaunch_environment();
    assert_eq!(environment.len(), std::env::vars_os().count());
}
