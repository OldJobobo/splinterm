use std::{
    fs,
    io::{ErrorKind, Read},
    os::{fd::OwnedFd, unix::process::ExitStatusExt},
    path::Path,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use tokio::io::unix::AsyncFd;

use splinterm_pty::{
    LinuxPtyBackend, LinuxPtySession, PtyCommand, PtyError, PtySession, PtySignal, PtySize,
};

const HELPER: &str = env!("CARGO_BIN_EXE_splinterm-pty-child");
const PROBE: &str = env!("CARGO_BIN_EXE_pty-probe");

fn backend() -> LinuxPtyBackend {
    LinuxPtyBackend::new(HELPER)
}

fn test_directory() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("splinterm-pty-{}-{nonce}", std::process::id()))
}

fn read_until(session: &mut LinuxPtySession, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        match session.read(&mut buffer) {
            Ok(0) => {}
            Ok(count) => {
                output.extend_from_slice(&buffer[..count]);
                let text = String::from_utf8_lossy(&output);
                if text.contains(needle) {
                    return text.into_owned();
                }
            }
            Err(PtyError::Io { source, .. })
                if matches!(
                    source.kind(),
                    ErrorKind::WouldBlock | ErrorKind::Interrupted
                ) => {}
            Err(error) => panic!("failed reading PTY: {error}"),
        }
        assert!(
            Instant::now() < deadline,
            "timed out; output={}",
            String::from_utf8_lossy(&output)
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn read_file_until(reader: &mut fs::File, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => {}
            Ok(count) => {
                output.extend_from_slice(&buffer[..count]);
                let text = String::from_utf8_lossy(&output);
                if text.contains(needle) {
                    return text.into_owned();
                }
            }
            Err(error)
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
            Err(error) => panic!("failed reading adopted PTY: {error}"),
        }
        assert!(
            Instant::now() < deadline,
            "timed out; output={}",
            String::from_utf8_lossy(&output)
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn command(mode: &str, cwd: &Path) -> PtyCommand {
    PtyCommand::new(PROBE, cwd).arg(mode)
}

#[test]
fn child_has_compatible_session_terminal_environment_and_cwd() {
    let cwd = test_directory();
    fs::create_dir(&cwd).unwrap();
    let spec = command("inspect", &cwd)
        .inherit_environment(false)
        .env("SPLINTERM_PTY_TEST", "present");
    let mut session = backend().spawn(&spec, PtySize::cells(80, 24)).unwrap();
    let output = read_until(&mut session, "FDS=");
    let status = session.wait().unwrap();

    let value = |name: &str| {
        output
            .lines()
            .find_map(|line| line.strip_suffix('\r').unwrap_or(line).strip_prefix(name))
            .unwrap()
    };
    assert_eq!(value("PID="), value("SID="));
    assert_eq!(value("PID="), value("PGRP="));
    assert_eq!(value("PID="), value("TTY_SID="));
    assert_eq!(value("PID="), value("TTY_PGRP="));
    assert_eq!(value("TTY="), "111");
    assert_eq!(value("IUTF8="), "1");
    assert_eq!(value("CWD="), cwd.to_str().unwrap());
    assert_eq!(value("TERM="), "xterm-256color");
    assert_eq!(value("COLORTERM="), "truecolor");
    assert_eq!(value("CUSTOM="), "present");
    assert_eq!(value("FOREIGN="), "");
    assert!(value("FDS=").contains("0:/dev/pts/"));
    assert!(value("FDS=").contains("1:/dev/pts/"));
    assert!(value("FDS=").contains("2:/dev/pts/"));
    assert!(!value("FDS=").contains("ptmx"));
    assert!(status.success());
    fs::remove_dir(cwd).unwrap();
}

#[test]
fn resize_and_bidirectional_io_reach_the_slave() {
    let cwd = std::env::current_dir().unwrap();
    let mut resize = backend()
        .spawn(
            &command("resize", &cwd),
            PtySize {
                columns: 80,
                rows: 24,
                pixel_width: 640,
                pixel_height: 384,
            },
        )
        .unwrap();
    let initial = read_until(&mut resize, "READY");
    assert!(initial.contains("INITIAL=80x24+640x384"));
    resize
        .resize(PtySize {
            columns: 100,
            rows: 40,
            pixel_width: 900,
            pixel_height: 720,
        })
        .unwrap();
    resize.write(b"\n").unwrap();
    let resized = read_until(&mut resize, "RESIZED=");
    assert!(resized.contains("RESIZED=100x40+900x720"));
    assert!(resize.wait().unwrap().success());

    let mut echo = backend()
        .spawn(&command("echo", &cwd), PtySize::cells(40, 10))
        .unwrap();
    read_until(&mut echo, "READY");
    echo.write(b"hello from master\n").unwrap();
    let echoed = read_until(&mut echo, "ECHO:hello from master");
    assert!(echoed.contains("ECHO:hello from master"));
    assert!(echo.wait().unwrap().success());
}

#[test]
fn login_shell_changes_only_argv_zero() {
    let cwd = std::env::current_dir().unwrap();
    let spec = command("argv", &cwd).login_shell(true);
    let mut session = backend().spawn(&spec, PtySize::cells(40, 10)).unwrap();
    let output = read_until(&mut session, "ARGV0=");
    assert!(output.contains(&format!("ARGV0=-{PROBE}")));
    assert!(session.wait().unwrap().success());
}

#[test]
fn process_group_signals_and_wait_status_are_observable() {
    let cwd = std::env::current_dir().unwrap();
    let mut session = backend()
        .spawn(&command("wait", &cwd), PtySize::cells(40, 10))
        .unwrap();
    assert!(session.try_wait().unwrap().is_none());
    session.signal_process_group(PtySignal::Hangup).unwrap();
    let status = session.wait().unwrap();
    assert_eq!(status.signal(), Some(1));
    assert_eq!(session.wait().unwrap(), status);
    assert_eq!(session.try_wait().unwrap(), Some(status));
}

#[test]
fn live_session_identity_and_single_master_adoption_round_trip() {
    let cwd = std::env::current_dir().unwrap();
    let mut session = backend()
        .spawn(
            &command("resize", &cwd),
            PtySize {
                columns: 80,
                rows: 24,
                pixel_width: 640,
                pixel_height: 384,
            },
        )
        .unwrap();
    let initial = read_until(&mut session, "READY");
    assert!(initial.contains("INITIAL=80x24+640x384"));

    let identity = session.identity();
    assert_eq!(identity.child_pid(), session.child_id());
    assert_eq!(identity.process_group(), identity.child_pid());
    assert_eq!(identity.session_id(), identity.child_pid());
    let master_fd = session.master_raw_fd();
    let adoptable = session.try_into_adoptable().unwrap();
    assert_eq!(adoptable.identity(), identity);
    let (manifest_identity, master) = adoptable.into_parts();
    let mut adopted =
        splinterm_pty::AdoptableLinuxPtySession::from_parts(manifest_identity, master)
            .adopt()
            .unwrap();
    assert_eq!(adopted.identity(), identity);
    assert_eq!(adopted.master_raw_fd(), master_fd);

    let mut reader = adopted.try_clone_reader().unwrap();
    adopted
        .resize(PtySize {
            columns: 100,
            rows: 40,
            pixel_width: 900,
            pixel_height: 720,
        })
        .unwrap();
    adopted.write(b"\n").unwrap();
    let resized = read_file_until(&mut reader, "RESIZED=");
    assert!(resized.contains("RESIZED=100x40+900x720"));
    drop(reader);

    let status = adopted.wait().unwrap();
    assert!(status.success());
    assert_eq!(adopted.try_wait().unwrap(), Some(status));
}

#[test]
fn failed_post_exec_adoption_retains_the_canonical_master() {
    let cwd = std::env::current_dir().unwrap();
    let session = backend()
        .spawn(&command("resize", &cwd), PtySize::cells(40, 10))
        .unwrap();
    let identity = session.identity();
    let adoptable = session.try_into_adoptable().unwrap();
    let master_raw_fd = adoptable.master_raw_fd();
    let (manifest_identity, master) = adoptable.into_parts();
    let mismatched_identity = splinterm_pty::LinuxPtyIdentity::from_raw(
        manifest_identity.child_pid(),
        manifest_identity.child_pid().saturating_add(1),
        manifest_identity.session_id(),
    )
    .unwrap();
    let (error, retained) =
        splinterm_pty::AdoptableLinuxPtySession::from_parts(mismatched_identity, master)
            .try_adopt()
            .unwrap_err();
    assert!(matches!(error, PtyError::IdentityMismatch));
    assert_eq!(retained.master_raw_fd(), master_raw_fd);

    let (_, master) = retained.into_parts();
    let mut recovered = splinterm_pty::AdoptableLinuxPtySession::from_parts(identity, master)
        .adopt()
        .unwrap();
    recovered.signal_process_group(PtySignal::Hangup).unwrap();
    assert_eq!(recovered.wait().unwrap().signal(), Some(1));
}

#[test]
fn adoption_reaps_a_child_that_exits_while_the_master_is_held() {
    let cwd = std::env::current_dir().unwrap();
    let session = backend()
        .spawn(
            &PtyCommand::new("/bin/sh", cwd).args(["-c", "sleep 0.05; exit 7"]),
            PtySize::cells(40, 10),
        )
        .unwrap();
    let child_pid = session.child_id();
    let adoptable = session.try_into_adoptable().unwrap();
    thread::sleep(Duration::from_millis(100));

    let mut adopted = adoptable.try_adopt().unwrap();
    assert_eq!(adopted.wait().unwrap().code(), Some(7));
    assert_eq!(adopted.try_wait().unwrap().unwrap().code(), Some(7));
    assert!(!Path::new(&format!("/proc/{child_pid}")).exists());
}

#[test]
fn failed_recovery_kills_and_reaps_a_verified_exact_child() {
    let cwd = std::env::current_dir().unwrap();
    let session = backend()
        .spawn(
            &PtyCommand::new("/bin/sh", cwd).args(["-c", "trap '' HUP TERM; sleep 30"]),
            PtySize::cells(40, 10),
        )
        .unwrap();
    let child_pid = session.child_id();
    let status = session
        .try_into_adoptable()
        .unwrap()
        .kill_child_and_wait()
        .unwrap();
    assert_eq!(status.signal(), Some(9));
    assert!(!Path::new(&format!("/proc/{child_pid}")).exists());
}

#[test]
fn failed_recovery_refuses_to_signal_without_child_authority() {
    let output = Command::new("/bin/sh")
        .args(["-c", "sleep 30 </dev/null >/dev/null 2>&1 & printf '%s' $!"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let unrelated_pid = String::from_utf8(output.stdout)
        .unwrap()
        .parse::<u32>()
        .unwrap();
    let identity =
        splinterm_pty::LinuxPtyIdentity::from_raw(unrelated_pid, unrelated_pid, unrelated_pid)
            .unwrap();
    let master: OwnedFd = fs::File::open("/dev/null").unwrap().into();
    let error = splinterm_pty::AdoptableLinuxPtySession::from_parts(identity, master)
        .kill_child_and_wait()
        .unwrap_err();
    assert!(matches!(error, PtyError::Io { .. }));
    assert!(Path::new(&format!("/proc/{unrelated_pid}")).exists());

    let _ = Command::new("/bin/kill")
        .args(["-KILL", &unrelated_pid.to_string()])
        .status();
}

#[test]
fn adoption_manifest_rejects_invalid_pid_values() {
    assert!(matches!(
        splinterm_pty::LinuxPtyIdentity::from_raw(0, 1, 1),
        Err(PtyError::InvalidChildId)
    ));
}

#[test]
fn failed_adoption_returns_the_original_session() {
    let cwd = std::env::current_dir().unwrap();
    let mut session = backend()
        .spawn(&command("argv", &cwd), PtySize::cells(40, 10))
        .unwrap();
    let output = read_until(&mut session, "ARGV0=");
    assert!(output.contains("ARGV0="));
    let status = session.wait().unwrap();
    let (error, mut recovered) = session.try_into_adoptable().unwrap_err();
    assert!(matches!(error, PtyError::ProcessExited));
    assert_eq!(recovered.wait().unwrap(), status);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_after_runtime_start_and_async_fd_read_work() {
    tokio::task::yield_now().await;
    let cwd = std::env::current_dir().unwrap();
    let mut session = backend()
        .spawn(&command("inspect", &cwd), PtySize::cells(40, 10))
        .unwrap();
    let reader = session.try_clone_reader().unwrap();
    let async_reader = AsyncFd::new(reader).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    while !String::from_utf8_lossy(&output).contains("FDS=") {
        let mut ready = tokio::time::timeout_at(deadline, async_reader.readable())
            .await
            .expect("timed out waiting for PTY")
            .unwrap();
        let mut buffer = [0_u8; 4096];
        if let Ok(result) = ready.try_io(|inner| inner.get_ref().read(&mut buffer)) {
            let count = result.unwrap();
            output.extend_from_slice(&buffer[..count]);
        }
    }
    assert!(String::from_utf8_lossy(&output).contains("TTY=111"));
    assert!(session.wait().unwrap().success());
}

#[test]
fn placement_completes_before_target_exec_and_returns_owned_state() {
    let marker = test_directory();
    let script = format!(
        "test -f '{}' && printf 'PLACED\\n'",
        marker.to_string_lossy()
    );
    let spec =
        PtyCommand::new("/bin/sh", std::env::current_dir().unwrap()).args(["-c", script.as_str()]);
    let placement_marker = marker.clone();
    let (mut session, placed_pid) = backend()
        .spawn_with_placement(&spec, PtySize::cells(40, 10), move |identity| {
            fs::write(&placement_marker, b"placed")?;
            Ok(identity.child_pid())
        })
        .unwrap();

    assert_eq!(placed_pid, session.child_id());
    assert!(read_until(&mut session, "PLACED").contains("PLACED"));
    assert!(session.wait().unwrap().success());
    fs::remove_file(marker).unwrap();
}

#[test]
fn placement_failure_prevents_target_exec_and_reaps_helper() {
    let executed = test_directory();
    let script = format!("printf executed > '{}'", executed.to_string_lossy());
    let spec =
        PtyCommand::new("/bin/sh", std::env::current_dir().unwrap()).args(["-c", script.as_str()]);
    let child_pid = Arc::new(AtomicU32::new(0));
    let observed_pid = Arc::clone(&child_pid);
    let result = backend().spawn_with_placement(
        &spec,
        PtySize::cells(40, 10),
        move |identity| -> std::io::Result<()> {
            observed_pid.store(identity.child_pid(), Ordering::SeqCst);
            Err(std::io::Error::other("injected placement failure"))
        },
    );

    assert!(matches!(
        result,
        Err(PtyError::Io {
            operation: "place PTY child",
            ..
        })
    ));
    assert!(!executed.exists());
    let child_pid = child_pid.load(Ordering::SeqCst);
    assert_ne!(child_pid, 0);
    assert!(!Path::new(&format!("/proc/{child_pid}")).exists());
}

#[test]
fn invalid_cwd_and_program_fail_without_affecting_the_daemon() {
    let cwd = test_directory();
    let invalid_cwd = backend().spawn(&command("inspect", &cwd), PtySize::cells(80, 24));
    assert!(invalid_cwd.is_err());

    let invalid_program = backend().spawn(
        &PtyCommand::new(
            "/definitely/missing/splinterm-command",
            std::env::current_dir().unwrap(),
        ),
        PtySize::cells(80, 24),
    );
    assert!(matches!(invalid_program, Err(PtyError::TargetExec)));
}
