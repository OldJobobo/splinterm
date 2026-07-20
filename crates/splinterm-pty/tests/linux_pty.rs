use std::{
    fs,
    io::{ErrorKind, Read},
    os::unix::process::ExitStatusExt,
    path::Path,
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
