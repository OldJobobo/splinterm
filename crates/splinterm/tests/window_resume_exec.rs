//! A Window relaunched across a daemon upgrade, exercised with the real
//! client binary (Plan 0061 M3.15).
//!
//! Each case builds a sealed resume record and a continuation socket pair,
//! makes exactly those descriptors (and, where a case says so, a stray one)
//! inheritable, and starts the installed-layout `splinterm` binary with the
//! two activation flags. The client claims its descriptors before Tokio,
//! validates them, connects to a stand-in daemon on a private socket, and
//! reports on standard error what it restores. No display exists, so the
//! Window itself never opens: the client then fails to reconnect for its
//! first pane and exits, which ends each case.
//!
//! This target uses `harness = false`: descriptors are made inheritable only
//! while the main thread spawns the client, and no libtest worker thread may
//! spawn a process meanwhile. Harness arguments are accepted and ignored.

use std::{
    env,
    fs::{self, File},
    io::{Read, Write},
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd},
        unix::net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rustix::{
    fs::{MemfdFlags, memfd_create},
    io::{FdFlags, fcntl_getfd, fcntl_setfd},
    net::{AddressFamily, SocketFlags, SocketType, socketpair},
};
use splinterm_core::{Dojo, Lair, LayoutNode, SplintState, TopologyRevision};
use splinterm_protocol::{
    ClientFrame, HandoffResumeTicket, PROTOCOL_VERSION, Request, Response, ServerFrame,
    ServerLimits, encode_frame,
    handoff_continuation::{HandoffTicketClaim, HandoffTicketGrant},
    window_resume::{
        WindowResumeRecord, WindowResumeTab, seal_window_resume_record, window_resume_arguments,
    },
};

const CLIENT: &str = env!("CARGO_BIN_EXE_splinterm");
const CASE_DEADLINE: Duration = Duration::from_secs(60);

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "splinterm-resume-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn running_dojo(name: &str) -> Dojo {
    let mut dojo = Dojo::with_shell(name, env::temp_dir());
    if let LayoutNode::Leaf(splint) = &mut dojo.root {
        splint.state = SplintState::Running;
    }
    dojo
}

fn lair_with(dojos: Vec<Dojo>) -> Lair {
    let mut lair = Lair::new("resume", env::temp_dir());
    lair.dojos = dojos;
    lair
}

fn record_for(lair: &Lair, generation: u64) -> WindowResumeRecord {
    WindowResumeRecord {
        daemon_generation: generation,
        prepare_id: 11,
        active: lair.dojos.len() - 1,
        tabs: lair
            .dojos
            .iter()
            .map(|dojo| WindowResumeTab {
                lair_id: lair.id,
                dojo_id: dojo.id,
                focus: Some(dojo.default_focus),
            })
            .collect(),
        app_id: None,
    }
}

fn read_client_frame(stream: &mut UnixStream) -> Option<ClientFrame> {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length).ok()?;
    let mut body = vec![0_u8; u32::from_be_bytes(length) as usize];
    stream.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

fn write_server_frame(stream: &mut UnixStream, frame: &ServerFrame) {
    stream.write_all(&encode_frame(frame).unwrap()).unwrap();
}

/// A daemon stand-in that serves exactly one connection: the handshake at
/// `generation`, then `lairs` for `ListLairs`. It reports whether the client
/// asked for the trusted UI role. The socket is removed once that connection
/// is accepted, so every later connection fails at once.
fn stand_in_daemon(socket: &Path, generation: u64, lairs: Vec<Lair>) -> mpsc::Receiver<bool> {
    let listener = UnixListener::bind(socket).unwrap();
    let path = socket.to_owned();
    let (report, reported) = mpsc::channel();
    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        drop(listener);
        let _ = fs::remove_file(&path);
        let Some(ClientFrame::Hello { role, .. }) = read_client_frame(&mut stream) else {
            return;
        };
        let _ = report.send(role == splinterm_protocol::ClientRole::TrustedUi);
        write_server_frame(
            &mut stream,
            &ServerFrame::Hello {
                version: PROTOCOL_VERSION,
                limits: ServerLimits::default(),
                development_terminal_access: false,
                daemon_hostname: None,
                daemon_generation: generation,
            },
        );
        while let Some(frame) = read_client_frame(&mut stream) {
            if let ClientFrame::Request {
                request_id,
                request: Request::ListLairs,
                ..
            } = frame
            {
                write_server_frame(
                    &mut stream,
                    &ServerFrame::Response {
                        request_id,
                        result: Response::Lairs {
                            lairs: lairs.clone(),
                            topology_revision: TopologyRevision::new(3),
                        },
                    },
                );
            }
        }
    });
    reported
}

fn set_inheritable(descriptor: BorrowedFd<'_>, inheritable: bool) {
    let flags = fcntl_getfd(descriptor).unwrap();
    let flags = if inheritable {
        flags - FdFlags::CLOEXEC
    } else {
        flags | FdFlags::CLOEXEC
    };
    fcntl_setfd(descriptor, flags).unwrap();
}

struct Outcome {
    success: bool,
    stderr: String,
    pid: u32,
}

/// Starts the client with `arguments` while exactly `inherited` are
/// inheritable, and waits for it within [`CASE_DEADLINE`].
fn run_client(
    scratch: &Scratch,
    socket: &Path,
    arguments: &[String],
    inherited: &[BorrowedFd<'_>],
) -> Outcome {
    run_program(CLIENT, &[], scratch, socket, arguments, inherited)
}

/// Starts `program` with `arguments` and `environment` added, while exactly
/// `inherited` are inheritable, and waits for it within [`CASE_DEADLINE`].
fn run_program(
    program: &str,
    environment: &[(&str, String)],
    scratch: &Scratch,
    socket: &Path,
    arguments: &[String],
    inherited: &[BorrowedFd<'_>],
) -> Outcome {
    for descriptor in inherited {
        set_inheritable(*descriptor, true);
    }
    let child = Command::new(program)
        .args(arguments)
        .envs(environment.iter().map(|(key, value)| (key, value)))
        .env("SPLINTERM_SOCKET", socket)
        .env("HOME", &scratch.0)
        .env("XDG_STATE_HOME", scratch.0.join("state"))
        .env("XDG_CONFIG_HOME", scratch.0.join("config"))
        .env("XDG_RUNTIME_DIR", &scratch.0)
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn();
    for descriptor in inherited {
        set_inheritable(*descriptor, false);
    }
    let mut child = child.unwrap();
    let pid = child.id();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > CASE_DEADLINE {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the relaunched client did not exit within {CASE_DEADLINE:?}");
        }
        thread::sleep(Duration::from_millis(20));
    };
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    Outcome {
        success: status.success(),
        stderr,
        pid,
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

/// What the adopted daemon's continuation server saw (Plan 0061 M3.16).
struct ObservedClaim {
    claim: HandoffTicketClaim,
    /// The sender PID the kernel attached.
    sender: i32,
}

/// Serves the daemon end of a continuation channel the way an adopted
/// daemon does: waits for one packet, reads it with its kernel credentials,
/// answers with a ticket grant naming `generation`, and hands its end back.
/// `None` when no packet arrived before the Window closed its end.
fn stand_in_continuation_server(
    daemon_end: OwnedFd,
    generation: u64,
) -> thread::JoinHandle<(Option<ObservedClaim>, OwnedFd)> {
    use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags};
    thread::spawn(move || {
        let mut descriptors = [rustix::event::PollFd::new(
            &daemon_end,
            rustix::event::PollFlags::IN,
        )];
        let timeout = rustix::event::Timespec {
            tv_sec: i64::try_from(CASE_DEADLINE.as_secs()).unwrap(),
            tv_nsec: 0,
        };
        if rustix::event::poll(&mut descriptors, Some(&timeout)).unwrap() == 0 {
            return (None, daemon_end);
        }
        let mut buffer = [0_u8; 64];
        let mut space = [std::mem::MaybeUninit::uninit(); rustix::cmsg_space!(ScmCredentials(1))];
        let mut control = RecvAncillaryBuffer::new(&mut space);
        let received = rustix::net::recvmsg(
            &daemon_end,
            &mut [std::io::IoSliceMut::new(&mut buffer)],
            &mut control,
            RecvFlags::DONTWAIT,
        )
        .unwrap();
        if received.bytes == 0 {
            return (None, daemon_end);
        }
        let sender = control
            .drain()
            .find_map(|message| match message {
                RecvAncillaryMessage::ScmCredentials(sender) => {
                    Some(sender.pid.as_raw_nonzero().get())
                }
                _ => None,
            })
            .expect("the kernel attaches the sender's credentials");
        let claim = HandoffTicketClaim::decode(&buffer[..received.bytes]).unwrap();
        let grant = HandoffTicketGrant {
            daemon_generation: generation,
            ticket: HandoffResumeTicket::new([0x6b; 32]),
        };
        rustix::net::send(
            &daemon_end,
            &grant.encode(),
            rustix::net::SendFlags::empty(),
        )
        .unwrap();
        (Some(ObservedClaim { claim, sender }), daemon_end)
    })
}

fn restores(scratch_name: &str, generation_now: u64, expect_rolled_back: bool) {
    let scratch = Scratch::new(scratch_name);
    let socket = scratch.0.join("splinterd.sock");
    let lair = lair_with(vec![running_dojo("first"), running_dojo("second")]);
    let record = record_for(&lair, 4);
    let trusted = stand_in_daemon(&socket, generation_now, vec![lair]);
    let sealed = seal_window_resume_record(&record).unwrap();
    let (continuation, daemon_end) = continuation_pair();
    // The daemon end receives credentials, as an adopted daemon's does.
    rustix::net::sockopt::set_socket_passcred(&daemon_end, true).unwrap();
    let server = stand_in_continuation_server(daemon_end, generation_now);
    let stray = File::open("/dev/null").unwrap();
    let arguments = window_resume_arguments(sealed.as_raw_fd(), continuation.as_raw_fd());
    let outcome = run_client(
        &scratch,
        &socket,
        &arguments,
        &[sealed.as_fd(), continuation.as_fd(), stray.as_fd()],
    );
    assert!(
        trusted.recv_timeout(Duration::from_secs(1)) == Ok(true),
        "the relaunched client connects as trusted UI: {}",
        outcome.stderr
    );
    assert!(
        outcome.stderr.contains(
            "relaunched across a daemon upgrade from generation 4 (prepare 11): 2 recorded tabs;"
        ),
        "{}",
        outcome.stderr
    );
    let closed = outcome
        .stderr
        .split("recorded tabs; ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .and_then(|count| count.parse::<usize>().ok())
        .unwrap_or_else(|| panic!("no stray count: {}", outcome.stderr));
    assert!(
        closed >= 1,
        "the stray descriptor is closed: {}",
        outcome.stderr
    );
    let expected = format!(
        "restoring 2 of 2 tabs from daemon generation {generation_now}{}",
        if expect_rolled_back {
            " (rolled back)"
        } else {
            ""
        }
    );
    assert!(outcome.stderr.contains(&expected), "{}", outcome.stderr);
    // Without a display the Window cannot open; the client exits.
    assert!(!outcome.success);
    // With this process's copy of the client end closed too, the server
    // sees either the claim or end of file.
    drop(continuation);
    let (observed, daemon_end) = server.join().unwrap();
    if expect_rolled_back {
        // A rollback daemon issues no ticket: the client never claims.
        assert!(observed.is_none(), "{}", outcome.stderr);
    } else {
        // After an adoption, the relaunched client claims once, from its own
        // process (kernel credentials), naming the generation it reached,
        // and arms the ticket for both restored panes.
        let observed = observed.unwrap_or_else(|| panic!("no claim: {}", outcome.stderr));
        assert_eq!(observed.claim.daemon_generation, generation_now);
        assert_eq!(observed.sender, i32::try_from(outcome.pid).unwrap());
        assert!(
            outcome
                .stderr
                .contains("claimed this Window's resume ticket for 2 panes"),
            "{}",
            outcome.stderr
        );
    }
    // The client closed its end; nothing more arrives.
    let mut buffer = [0_u8; 32];
    assert_eq!(
        rustix::net::recv(&daemon_end, &mut buffer, rustix::net::RecvFlags::DONTWAIT)
            .unwrap()
            .0,
        0
    );
    println!("test window_resume_exec::{scratch_name} ... ok");
}

fn refuses_an_unsealed_record() {
    let scratch = Scratch::new("unsealed");
    let socket = scratch.0.join("splinterd.sock");
    let lair = lair_with(vec![running_dojo("only")]);
    let bytes = record_for(&lair, 4).encode().unwrap();
    let unsealed =
        memfd_create("unsealed", MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING).unwrap();
    assert_eq!(rustix::io::write(&unsealed, &bytes).unwrap(), bytes.len());
    let (continuation, _daemon_end) = continuation_pair();
    let arguments = window_resume_arguments(unsealed.as_raw_fd(), continuation.as_raw_fd());
    let outcome = run_client(
        &scratch,
        &socket,
        &arguments,
        &[unsealed.as_fd(), continuation.as_fd()],
    );
    assert!(!outcome.success);
    assert!(
        outcome
            .stderr
            .contains("the inherited resume record is invalid")
            && outcome
                .stderr
                .contains("reopen its Dojos from the launcher"),
        "{}",
        outcome.stderr
    );
    println!("test window_resume_exec::refuses_an_unsealed_record ... ok");
}

fn refuses_a_descriptor_that_was_not_inherited() {
    let scratch = Scratch::new("uninherited");
    let socket = scratch.0.join("splinterd.sock");
    let lair = lair_with(vec![running_dojo("only")]);
    let sealed = seal_window_resume_record(&record_for(&lair, 4)).unwrap();
    let (continuation, _daemon_end) = continuation_pair();
    let arguments = window_resume_arguments(sealed.as_raw_fd(), continuation.as_raw_fd());
    // The continuation stays close-on-exec, so it never reaches the client.
    let outcome = run_client(&scratch, &socket, &arguments, &[sealed.as_fd()]);
    assert!(!outcome.success);
    assert!(
        outcome
            .stderr
            .contains("cannot claim the inherited resume descriptors"),
        "{}",
        outcome.stderr
    );
    println!("test window_resume_exec::refuses_a_descriptor_that_was_not_inherited ... ok");
}

fn fails_clearly_when_no_dojo_survives() {
    let scratch = Scratch::new("no-dojo");
    let socket = scratch.0.join("splinterd.sock");
    let recorded = lair_with(vec![running_dojo("gone")]);
    let _trusted = stand_in_daemon(&socket, 5, vec![lair_with(vec![running_dojo("other")])]);
    let sealed = seal_window_resume_record(&record_for(&recorded, 4)).unwrap();
    let (continuation, _daemon_end) = continuation_pair();
    let arguments = window_resume_arguments(sealed.as_raw_fd(), continuation.as_raw_fd());
    let outcome = run_client(
        &scratch,
        &socket,
        &arguments,
        &[sealed.as_fd(), continuation.as_fd()],
    );
    assert!(!outcome.success);
    assert!(
        outcome.stderr.contains("restoring 0 of 1 tabs")
            && outcome
                .stderr
                .contains("none of this Window's Dojos is still running"),
        "{}",
        outcome.stderr
    );
    println!("test window_resume_exec::fails_clearly_when_no_dojo_survives ... ok");
}

/// Selects a stand-in Window generation of this test binary.
const STAGE: &str = "SPLINTERM_WINDOW_CONTINUATION_STAGE";
/// The Lair (JSON) whose Dojos the stand-in Window shows.
const STAGE_LAIR: &str = "SPLINTERM_WINDOW_CONTINUATION_LAIR";
const OLD_GENERATION: u64 = 4;

/// A sealed memfd holding the client binary: the forward client snapshot a
/// handing-off daemon would pass.
fn client_snapshot() -> OwnedFd {
    use rustix::fs::{SealFlags, fcntl_add_seals};
    let bytes = fs::read(CLIENT).unwrap();
    let memfd = memfd_create(
        "splinterm-forward-client",
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING | MemfdFlags::EXEC,
    )
    .unwrap();
    let mut file = File::from(memfd);
    file.write_all(&bytes).unwrap();
    let memfd = OwnedFd::from(file);
    fcntl_add_seals(
        &memfd,
        SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL,
    )
    .unwrap();
    memfd
}

/// A stand-in Window (this process, single-threaded but for its stand-in
/// daemon thread): it declares an anchor on a socket pair, receives a prepare
/// frame carrying the sealed client snapshot and a continuation, takes part
/// through the library's real path, and reaches the real relaunch exec. The
/// daemon thread answers the acknowledgement and then sends the stage's
/// outcome, or closes the continuation.
#[allow(
    clippy::too_many_lines,
    reason = "one stand-in Window generation: its daemon, its anchor, and its relaunch"
)]
fn stand_in_window(stage: &str) {
    use splinterm::{
        handoff_status::HandoffStatusHub,
        window_continuation::{
            AnchorConnection, ContinuationContext, ParticipationEnd, SystemExec, WindowLayout,
            WindowLayoutLedger, participate,
        },
    };
    use splinterm_protocol::{
        HANDOFF_PREPARE_DESCRIPTORS, HandoffClientSnapshot, HandoffPrepare,
        handoff_outcome::{HandoffOutcome, HandoffOutcomeKind},
        handoff_prepare::{
            HandoffPrepareDescriptors, encode_handoff_prepare, send_handoff_prepare,
        },
    };

    let lair: Lair = serde_json::from_str(&env::var(STAGE_LAIR).unwrap()).unwrap();
    let snapshot = client_snapshot();
    let stat = rustix::fs::fstat(&snapshot).unwrap();
    let prepare = HandoffPrepare {
        daemon_generation: OLD_GENERATION,
        prepare_id: 23,
        descriptors: HANDOFF_PREPARE_DESCRIPTORS,
        client_snapshot: HandoffClientSnapshot {
            device: stat.st_dev,
            inode: stat.st_ino,
            size: u64::try_from(stat.st_size).unwrap(),
            sha256: [5; 32],
        },
    };
    let (window_end, mut daemon) = UnixStream::pair().unwrap();
    let outcome = match stage {
        "adopted" => Some(HandoffOutcomeKind::Adopted),
        "rolled-back" => Some(HandoffOutcomeKind::RolledBack),
        _ => None,
    };
    let _daemon = thread::spawn(move || {
        let _ = read_client_frame(&mut daemon);
        write_server_frame(
            &mut daemon,
            &ServerFrame::Hello {
                version: PROTOCOL_VERSION,
                limits: ServerLimits::default(),
                development_terminal_access: false,
                daemon_hostname: None,
                daemon_generation: OLD_GENERATION,
            },
        );
        let (continuation, daemon_end) = continuation_pair();
        let encoded = encode_handoff_prepare(&prepare).unwrap();
        let descriptors = HandoffPrepareDescriptors::new(snapshot, continuation);
        send_handoff_prepare(daemon.as_fd(), &encoded, &descriptors).unwrap();
        drop(descriptors);
        let Some(ClientFrame::Request { request_id, .. }) = read_client_frame(&mut daemon) else {
            return;
        };
        write_server_frame(
            &mut daemon,
            &ServerFrame::Response {
                request_id,
                result: Response::Acknowledged,
            },
        );
        let Some(kind) = outcome else {
            // A cancelled attempt: the continuation closes with no outcome.
            return;
        };
        let generation = match kind {
            HandoffOutcomeKind::Adopted => OLD_GENERATION + 1,
            HandoffOutcomeKind::RolledBack => OLD_GENERATION,
        };
        let packet = HandoffOutcome {
            kind,
            daemon_generation: generation,
        }
        .encode();
        rustix::net::send(&daemon_end, &packet, rustix::net::SendFlags::empty()).unwrap();
        // Hold the channel until the exec replaces this process.
        let mut wait = [0_u8; 1];
        let _ = rustix::net::recv(&daemon_end, &mut wait, rustix::net::RecvFlags::empty());
    });
    let ledger = WindowLayoutLedger::default();
    ledger.publish(WindowLayout {
        tabs: lair
            .dojos
            .iter()
            .map(|dojo| WindowResumeTab {
                lair_id: lair.id,
                dojo_id: dojo.id,
                focus: Some(dojo.default_focus),
            })
            .collect(),
        active: 0,
    });
    let context = ContinuationContext {
        hub: HandoffStatusHub::default(),
        ledger,
        app_id: None,
        installed_client: Some(PathBuf::from(CLIENT)),
        exec: std::sync::Arc::new(SystemExec),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let end = runtime.block_on(async {
        window_end.set_nonblocking(true).unwrap();
        let stream = tokio::net::UnixStream::from_std(window_end).unwrap();
        let mut anchor = AnchorConnection::handshake(stream).await.unwrap();
        let (prepare, descriptors) = anchor.next_prepare().await.unwrap();
        participate(&context, &mut anchor, prepare, descriptors).await
    });
    // Reached only when the Window keeps running this image.
    match end {
        ParticipationEnd::Ended(reason) if outcome.is_none() => {
            eprintln!("stand-in Window: participation ended: {reason}");
            assert!(
                splinterm::child_spawn::spawn(|| Ok(())).is_ok(),
                "spawning resumes"
            );
            std::process::exit(0);
        }
        end => {
            eprintln!("stand-in Window: unexpected end: {end:?}");
            std::process::exit(1);
        }
    }
}

/// Runs a stand-in Window stage against a stand-in serving daemon at
/// `generation_now`, and returns its outcome.
fn run_stand_in_window(stage: &str, generation_now: u64) -> (Outcome, mpsc::Receiver<bool>) {
    let scratch = Scratch::new(&format!("window-{stage}"));
    let socket = scratch.0.join("splinterd.sock");
    let lair = lair_with(vec![running_dojo("first"), running_dojo("second")]);
    let trusted = stand_in_daemon(&socket, generation_now, vec![lair.clone()]);
    let outcome = run_program(
        &env::current_exe().unwrap().to_string_lossy(),
        &[
            (STAGE, stage.to_owned()),
            (STAGE_LAIR, serde_json::to_string(&lair).unwrap()),
        ],
        &scratch,
        &socket,
        &[],
        &[],
    );
    (outcome, trusted)
}

/// An adoption: the stand-in Window `execveat`s the sealed client snapshot,
/// which claims the record and continuation and restores both tabs from the
/// next generation.
fn a_window_relaunches_into_the_snapshot_after_adoption() {
    let (outcome, trusted) = run_stand_in_window("adopted", OLD_GENERATION + 1);
    assert_eq!(trusted.recv_timeout(Duration::from_secs(1)), Ok(true));
    assert!(
        outcome.stderr.contains(&format!(
            "relaunched across a daemon upgrade from generation {OLD_GENERATION} (prepare 23): 2 recorded tabs;"
        )) && outcome
            .stderr
            .contains("restoring 2 of 2 tabs from daemon generation 5\n"),
        "{}",
        outcome.stderr
    );
    println!(
        "test window_resume_exec::a_window_relaunches_into_the_snapshot_after_adoption ... ok"
    );
}

/// A rollback: the stand-in Window discards the snapshot and execs the
/// installed client by path, which restores from the old generation.
fn a_window_relaunches_the_installed_client_after_rollback() {
    let (outcome, trusted) = run_stand_in_window("rolled-back", OLD_GENERATION);
    assert_eq!(trusted.recv_timeout(Duration::from_secs(1)), Ok(true));
    assert!(
        outcome.stderr.contains(&format!(
            "restoring 2 of 2 tabs from daemon generation {OLD_GENERATION} (rolled back)"
        )),
        "{}",
        outcome.stderr
    );
    println!(
        "test window_resume_exec::a_window_relaunches_the_installed_client_after_rollback ... ok"
    );
}

/// A cancelled attempt: the continuation closes with no outcome, and the
/// stand-in Window keeps running with spawning resumed.
fn a_window_keeps_running_when_the_attempt_closes_without_an_outcome() {
    let (outcome, _) = run_stand_in_window("cancelled", OLD_GENERATION);
    assert!(outcome.success, "{}", outcome.stderr);
    assert!(
        outcome
            .stderr
            .contains("participation ended: the attempt ended without an outcome"),
        "{}",
        outcome.stderr
    );
    println!(
        "test window_resume_exec::a_window_keeps_running_when_the_attempt_closes_without_an_outcome ... ok"
    );
}

fn main() {
    if let Ok(stage) = env::var(STAGE) {
        stand_in_window(&stage);
        return;
    }
    restores("adopted", 5, false);
    restores("rolled_back", 4, true);
    refuses_an_unsealed_record();
    refuses_a_descriptor_that_was_not_inherited();
    fails_clearly_when_no_dojo_survives();
    a_window_relaunches_into_the_snapshot_after_adoption();
    a_window_relaunches_the_installed_client_after_rollback();
    a_window_keeps_running_when_the_attempt_closes_without_an_outcome();
    println!("\ntest result: ok. 8 passed; 0 failed");
}
