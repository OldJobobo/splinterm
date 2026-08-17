use std::{
    collections::BTreeMap,
    env,
    ffi::{CString, OsString},
    fs::File,
    io::{self, ErrorKind, Read, Seek, SeekFrom, Write},
    os::{
        fd::OwnedFd,
        unix::{ffi::OsStrExt, process::ExitStatusExt},
    },
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use sha2::{Digest, Sha256};
use splinterm_pty::{
    AdoptableLinuxPtySession, LinuxPtyBackend, LinuxPtyIdentity, LinuxPtySession, PtyCommand,
    PtySession, PtySignal, PtySize,
};

const HELPER: &str = env!("CARGO_BIN_EXE_splinterm-pty-child");
const PROBE: &str = env!("CARGO_BIN_EXE_pty-probe");
const STAGE: &str = "SPLINTERM_PTY_ADOPTION_STAGE";
const CHILD: &str = "SPLINTERM_PTY_ADOPTION_CHILD";
const PROCESS_GROUP: &str = "SPLINTERM_PTY_ADOPTION_PROCESS_GROUP";
const SESSION: &str = "SPLINTERM_PTY_ADOPTION_SESSION";
const DAEMON: &str = "SPLINTERM_PTY_ADOPTION_DAEMON";
const PAUSE_STARTED: &str = "SPLINTERM_PTY_ADOPTION_PAUSE_STARTED";
const TEST_NAME: &str = "exec_adoption_preserves_identity_io_resize_signaling_and_reaping";
const BURST_LINES: usize = 512;

fn now_ns() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_nanos()
}

fn read_until(reader: &mut impl Read, needle: &str) -> String {
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
            Err(error) => panic!("failed reading PTY during adoption spike: {error}"),
        }
        assert!(
            Instant::now() < deadline,
            "timed out; output={}",
            String::from_utf8_lossy(&output)
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn assert_burst_order(output: &str, label: &str) {
    let observed = output
        .lines()
        .filter_map(|line| {
            line.strip_suffix('\r')
                .unwrap_or(line)
                .strip_prefix("BURST:")
        })
        .filter_map(|line| line.strip_prefix(label))
        .filter_map(|line| line.strip_prefix(':'))
        .map(|index| index.parse::<usize>().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(observed, (0..BURST_LINES).collect::<Vec<_>>());
}

fn manifest_identity() -> LinuxPtyIdentity {
    LinuxPtyIdentity::from_raw(
        env::var(CHILD).unwrap().parse().unwrap(),
        env::var(PROCESS_GROUP).unwrap().parse().unwrap(),
        env::var(SESSION).unwrap().parse().unwrap(),
    )
    .unwrap()
}

fn null_descriptor() -> OwnedFd {
    rustix::fs::open(
        "/dev/null",
        rustix::fs::OFlags::RDWR | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .unwrap()
}

fn replace_inherited_stdin() -> OwnedFd {
    let master = rustix::io::dup(std::io::stdin()).unwrap();
    nix::unistd::dup2_stdin(null_descriptor()).unwrap();
    master
}

fn take_inherited_rollback() -> OwnedFd {
    let rollback = rustix::io::dup(std::io::stderr()).unwrap();
    nix::unistd::dup2_stderr(std::io::stdout()).unwrap();
    let flags = rustix::io::fcntl_getfd(&rollback).unwrap();
    rustix::io::fcntl_setfd(&rollback, flags | rustix::io::FdFlags::CLOEXEC).unwrap();
    rollback
}

fn digest(file: &mut File) -> [u8; 32] {
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    hasher.finalize().into()
}

fn sealed_snapshot(source: &Path, name: &str) -> OwnedFd {
    let mut source = File::open(source).unwrap();
    let source_digest = digest(&mut source);
    source.seek(SeekFrom::Start(0)).unwrap();
    let descriptor = rustix::fs::memfd_create(
        name,
        rustix::fs::MemfdFlags::ALLOW_SEALING
            | rustix::fs::MemfdFlags::CLOEXEC
            | rustix::fs::MemfdFlags::EXEC,
    )
    .unwrap();
    let mut snapshot = File::from(descriptor);
    io::copy(&mut source, &mut snapshot).unwrap();
    snapshot.flush().unwrap();
    assert_eq!(digest(&mut snapshot), source_digest);

    let seals = rustix::fs::SealFlags::WRITE
        | rustix::fs::SealFlags::GROW
        | rustix::fs::SealFlags::SHRINK
        | rustix::fs::SealFlags::SEAL;
    rustix::fs::fcntl_add_seals(&snapshot, seals).unwrap();
    assert_eq!(rustix::fs::fcntl_get_seals(&snapshot).unwrap(), seals);
    assert!(rustix::io::write(&snapshot, b"mutation").is_err());
    let size = u64::try_from(rustix::fs::fstat(&snapshot).unwrap().st_size).unwrap();
    assert!(rustix::fs::ftruncate(&snapshot, size.saturating_add(1)).is_err());
    assert!(rustix::fs::ftruncate(&snapshot, size.saturating_sub(1)).is_err());
    assert!(rustix::fs::fcntl_add_seals(&snapshot, rustix::fs::SealFlags::empty()).is_err());
    snapshot.into()
}

fn temporary_source() -> PathBuf {
    env::temp_dir().join(format!(
        "splinterm-pty-adoption-source-{}-{}",
        std::process::id(),
        now_ns()
    ))
}

fn adopt_inherited() -> LinuxPtySession {
    assert_eq!(
        env::var(DAEMON).unwrap().parse::<u32>().unwrap(),
        std::process::id(),
        "daemon PID changed across exec"
    );
    let identity = manifest_identity();
    let master = replace_inherited_stdin();
    let session = AdoptableLinuxPtySession::from_parts(identity, master)
        .adopt()
        .unwrap();
    assert_eq!(session.identity(), identity);
    session
}

fn exec_snapshot(snapshot: &OwnedFd, identity: LinuxPtyIdentity, stage: &str) -> ! {
    let arguments = [
        CString::new("pty-adoption-exec").unwrap(),
        CString::new(TEST_NAME).unwrap(),
        CString::new("--exact").unwrap(),
        CString::new("--nocapture").unwrap(),
        CString::new("--test-threads=1").unwrap(),
    ];
    let mut environment = env::vars_os().collect::<BTreeMap<OsString, OsString>>();
    for (name, value) in [
        (STAGE, stage.to_owned()),
        (CHILD, identity.child_pid().to_string()),
        (PROCESS_GROUP, identity.process_group().to_string()),
        (SESSION, identity.session_id().to_string()),
        (DAEMON, std::process::id().to_string()),
        (PAUSE_STARTED, now_ns().to_string()),
    ] {
        environment.insert(OsString::from(name), OsString::from(value));
    }
    let environment = environment
        .into_iter()
        .map(|(name, value)| {
            let mut entry = name.as_bytes().to_vec();
            entry.push(b'=');
            entry.extend_from_slice(value.as_bytes());
            CString::new(entry).unwrap()
        })
        .collect::<Vec<_>>();
    let empty_path = CString::new("").unwrap();
    let error = nix::unistd::execveat(
        snapshot,
        &empty_path,
        &arguments,
        &environment,
        nix::fcntl::AtFlags::AT_EMPTY_PATH,
    )
    .unwrap_err();
    panic!("descriptor exec of {stage} generation failed: {error}");
}

fn session_into_stdin(session: LinuxPtySession) -> LinuxPtyIdentity {
    let identity = session.identity();
    let (_, master) = session.try_into_adoptable().unwrap().into_parts();
    nix::unistd::dup2_stdin(&master).unwrap();
    identity
}

fn exec_forward(session: LinuxPtySession) -> ! {
    let identity = session_into_stdin(session);
    let executable = env::current_exe().unwrap();
    let source = temporary_source();
    std::fs::copy(&executable, &source).unwrap();
    let forward = sealed_snapshot(&source, "splinterm-forward");
    let rollback = sealed_snapshot(Path::new("/proc/self/exe"), "splinterm-rollback");
    std::fs::write(&source, b"replaced after sealing").unwrap();
    std::fs::remove_file(&source).unwrap();
    nix::unistd::dup2_stderr(&rollback).unwrap();
    exec_snapshot(&forward, identity, "forward")
}

fn exec_rollback(session: LinuxPtySession, rollback: &OwnedFd) -> ! {
    let identity = session_into_stdin(session);
    exec_snapshot(rollback, identity, "rollback")
}

fn forward_generation() -> ! {
    let rollback = take_inherited_rollback();
    let mut session = adopt_inherited();
    let pause_ns = now_ns() - env::var(PAUSE_STARTED).unwrap().parse::<u128>().unwrap();
    let mut reader = session.try_clone_reader().unwrap();
    let output = read_until(
        &mut reader,
        &format!("BURST:forward:{:04}", BURST_LINES - 1),
    );
    assert_burst_order(&output, "forward");
    session.write(b"forward\n").unwrap();
    let output = read_until(&mut reader, "ECHO:forward");
    assert!(output.contains("ECHO:forward"));
    session
        .resize(PtySize {
            columns: 100,
            rows: 40,
            pixel_width: 900,
            pixel_height: 720,
        })
        .unwrap();
    session.write(b"size\n").unwrap();
    let output = read_until(&mut reader, "SIZE=");
    assert!(output.contains("SIZE=100x40+900x720"));
    println!(
        "FORWARD child={} no_reader_ns={pause_ns} ordered_lines={BURST_LINES}",
        session.child_id()
    );
    session
        .write(format!("burst rollback {BURST_LINES}\n").as_bytes())
        .unwrap();
    drop(reader);
    exec_rollback(session, &rollback)
}

fn descriptor_targets() -> Vec<PathBuf> {
    std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|entry| std::fs::read_link(entry.path()).ok())
        .collect()
}

fn rollback_generation() {
    let mut session = adopt_inherited();
    let pause_ns = now_ns() - env::var(PAUSE_STARTED).unwrap().parse::<u128>().unwrap();
    let mut reader = session.try_clone_reader().unwrap();
    let output = read_until(
        &mut reader,
        &format!("BURST:rollback:{:04}", BURST_LINES - 1),
    );
    assert_burst_order(&output, "rollback");
    session.write(b"rollback\n").unwrap();
    let output = read_until(&mut reader, "ECHO:rollback");
    assert!(output.contains("ECHO:rollback"));
    println!(
        "ROLLBACK child={} no_reader_ns={pause_ns} ordered_lines={BURST_LINES}",
        session.child_id()
    );
    drop(reader);
    let targets = descriptor_targets();
    assert_eq!(
        targets
            .iter()
            .filter(|target| target.as_os_str().as_bytes().ends_with(b"/ptmx"))
            .count(),
        1,
        "only the canonical PTY master remains after reader teardown"
    );
    assert!(
        targets
            .iter()
            .all(|target| !target.to_string_lossy().contains("memfd:splinterm")),
        "sealed executable descriptors must close after serving as exec targets"
    );
    assert_eq!(
        std::fs::read_link("/proc/self/fd/0").unwrap(),
        Path::new("/dev/null")
    );

    session.signal_process_group(PtySignal::Hangup).unwrap();
    let status = session.wait().unwrap();
    assert_eq!(status.signal(), Some(1));
    assert_eq!(session.try_wait().unwrap(), Some(status));
}

#[test]
fn exec_adoption_preserves_identity_io_resize_signaling_and_reaping() {
    match env::var(STAGE).as_deref() {
        Ok("forward") => forward_generation(),
        Ok("rollback") => rollback_generation(),
        Ok(stage) => panic!("unexpected adoption stage {stage}"),
        Err(env::VarError::NotPresent) => {
            let mut session = LinuxPtyBackend::new(HELPER)
                .spawn(
                    &PtyCommand::new(PROBE, env::current_dir().unwrap()).arg("handoff"),
                    PtySize::cells(80, 24),
                )
                .unwrap();
            let mut reader = session.try_clone_reader().unwrap();
            let output = read_until(&mut reader, "READY");
            assert!(output.contains("READY"));
            session.write(b"old\n").unwrap();
            let output = read_until(&mut reader, "ECHO:old");
            assert!(output.contains("ECHO:old"));
            session
                .write(format!("burst forward {BURST_LINES}\n").as_bytes())
                .unwrap();
            drop(reader);
            exec_forward(session);
        }
        Err(error) => panic!("invalid adoption stage: {error}"),
    }
}
