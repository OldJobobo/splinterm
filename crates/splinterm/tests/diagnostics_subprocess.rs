use std::{
    env, fs,
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use splinterm::diagnostics::{
    DiagnosticEvent, ExitClass, initialize_graphical, install_termination_signal_task,
};
use uuid::Uuid;

const PANIC_SENTINEL: &str = "SECRET_PTY_KEY_CLIPBOARD_ARGV_CWD_PANIC_SENTINEL";

fn child_mode() -> Option<String> {
    env::var("SPLINTERM_DIAGNOSTIC_CHILD").ok()
}

fn child_state_root() -> PathBuf {
    PathBuf::from(env::var_os("XDG_STATE_HOME").unwrap())
}

fn initialize_child() {
    initialize_graphical().unwrap().begin_window(None, None);
}

fn child_entry(mode: &str) {
    initialize_child();
    match mode {
        "panic" => panic!("{PANIC_SENTINEL}"),
        "sigabrt" => std::process::abort(),
        "sigterm" => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                install_termination_signal_task();
                fs::write(child_state_root().join("sigterm-ready"), b"ready").unwrap();
                tokio::time::sleep(Duration::from_secs(30)).await;
            });
        }
        _ => panic!("unknown diagnostic child mode"),
    }
}

fn test_directory(label: &str) -> PathBuf {
    let path = env::temp_dir().join(format!(
        "splinterm-diagnostic-subprocess-{label}-{}-{}",
        std::process::id(),
        Uuid::new_v4()
    ));
    fs::create_dir(&path).unwrap();
    path
}

fn spawn_child(mode: &str, state_home: &Path) -> std::process::Child {
    Command::new(env::current_exe().unwrap())
        .args(["--exact", "diagnostic_subprocess_child", "--nocapture"])
        .env("SPLINTERM_DIAGNOSTIC_CHILD", mode)
        .env("XDG_STATE_HOME", state_home)
        .env("SPLINTERM_PRIVATE_SENTINEL", PANIC_SENTINEL)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn summary(root: &Path) -> Option<DiagnosticEvent> {
    fs::read(root.join("splinterm/last-client-exit.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn retained_text(root: &Path) -> String {
    let logs = root.join("splinterm/logs");
    fs::read_dir(logs)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .filter_map(|entry| fs::read_to_string(entry.path()).ok())
        .collect()
}

#[test]
fn diagnostic_subprocess_child() {
    if let Some(mode) = child_mode() {
        child_entry(&mode);
    }
}

#[test]
fn panic_hook_retains_typed_record_without_payload() {
    let root = test_directory("panic");
    let output = spawn_child("panic", &root).wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains(PANIC_SENTINEL));
    assert_eq!(summary(&root).unwrap().exit_class, Some(ExitClass::Panic));
    let retained = retained_text(&root);
    assert!(!retained.contains(PANIC_SENTINEL));
    assert!(retained.contains("\"exit_class\":\"panic\""));
    fs::remove_dir_all(root).unwrap();
}

fn assert_graceful_signal(signal: rustix::process::Signal, number: i32, label: &str) {
    let root = test_directory(label);
    let mut child = spawn_child("sigterm", &root);
    let ready = root.join("sigterm-ready");
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !ready.exists() && std::time::Instant::now() < deadline {
        assert!(
            child.try_wait().unwrap().is_none(),
            "signal child exited before readiness"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(ready.exists(), "signal child did not become ready");
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(child.id().cast_signed()).unwrap(),
        signal,
    )
    .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(128 + number));
    assert_eq!(
        summary(&root).unwrap().exit_class,
        Some(ExitClass::SignalTermination)
    );
    assert!(!retained_text(&root).contains(PANIC_SENTINEL));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn sigterm_writes_graceful_terminal_record() {
    assert_graceful_signal(rustix::process::Signal::TERM, libc::SIGTERM, "sigterm");
}

#[test]
fn sigint_writes_graceful_terminal_record() {
    assert_graceful_signal(rustix::process::Signal::INT, libc::SIGINT, "sigint");
}

#[test]
fn graphical_top_level_error_never_formats_arbitrary_error_text() {
    let root = test_directory("top-level-error");
    let config = root.join("invalid-config.ini");
    fs::write(&config, format!("[font]\nsize={PANIC_SENTINEL}\n")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_splinterm"))
        .arg("launch")
        .env("XDG_STATE_HOME", &root)
        .env("SPLINTERM_CONFIG", &config)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("splinterm graphical client failed"));
    assert!(!stderr.contains(PANIC_SENTINEL));
    assert_eq!(summary(&root).unwrap().exit_class, Some(ExitClass::Unknown));
    assert!(!retained_text(&root).contains(PANIC_SENTINEL));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn graphical_pre_map_error_survives_null_stdio() {
    let root = test_directory("null-stdio-error");
    let config = root.join("invalid-config.ini");
    fs::write(&config, "[font]\nsize=not-a-number\n").unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_splinterm"))
        .arg("launch")
        .env("XDG_STATE_HOME", &root)
        .env("SPLINTERM_CONFIG", &config)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(1));
    assert_eq!(summary(&root).unwrap().exit_class, Some(ExitClass::Unknown));
    assert!(retained_text(&root).contains("\"error_code\":\"internal_error\""));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn fatal_signal_does_not_claim_an_in_process_exit_record() {
    let root = test_directory("sigabrt");
    let output = spawn_child("sigabrt", &root).wait_with_output().unwrap();
    assert_eq!(output.status.signal(), Some(libc::SIGABRT));
    assert!(summary(&root).is_none());
    assert!(retained_text(&root).is_empty());
    fs::remove_dir_all(root).unwrap();
}
