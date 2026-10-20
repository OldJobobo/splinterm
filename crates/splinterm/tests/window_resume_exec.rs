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
    ClientFrame, PROTOCOL_VERSION, Request, Response, ServerFrame, ServerLimits, encode_frame,
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
}

/// Starts the client with `arguments` while exactly `inherited` are
/// inheritable, and waits for it within [`CASE_DEADLINE`].
fn run_client(
    scratch: &Scratch,
    socket: &Path,
    arguments: &[String],
    inherited: &[BorrowedFd<'_>],
) -> Outcome {
    for descriptor in inherited {
        set_inheritable(*descriptor, true);
    }
    let child = Command::new(CLIENT)
        .args(arguments)
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

fn restores(scratch_name: &str, generation_now: u64, expect_rolled_back: bool) {
    let scratch = Scratch::new(scratch_name);
    let socket = scratch.0.join("splinterd.sock");
    let lair = lair_with(vec![running_dojo("first"), running_dojo("second")]);
    let record = record_for(&lair, 4);
    let trusted = stand_in_daemon(&socket, generation_now, vec![lair]);
    let sealed = seal_window_resume_record(&record).unwrap();
    let (continuation, daemon_end) = continuation_pair();
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
    // The client held the continuation until it exited; with this process's
    // copy closed too, the daemon end reads end of file.
    drop(continuation);
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

fn main() {
    restores("adopted", 5, false);
    restores("rolled_back", 4, true);
    refuses_an_unsealed_record();
    refuses_a_descriptor_that_was_not_inherited();
    fails_clearly_when_no_dojo_survives();
    println!("\ntest result: ok. 5 passed; 0 failed");
}
