use std::{
    env, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_splinterm"))
}

fn test_directory(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = env::temp_dir().join(format!(
        "splinterm-policy-cli-{label}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&path).unwrap();
    path
}

fn write_policy(directory: &Path, mode: u32) -> PathBuf {
    let path = directory.join("policy.json");
    fs::write(&path, r#"{"schema":"splinterm.policy.v2","rules":[]}"#).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    path
}

fn output(command: &mut Command) -> Output {
    command.output().unwrap()
}

#[test]
fn policy_validate_and_inspect_use_secure_daemon_loader() {
    let directory = test_directory("inspect");
    let policy = write_policy(&directory, 0o600);

    let validated = output(binary().args(["policy", "validate"]).arg(&policy));
    assert!(validated.status.success(), "{:?}", validated.stderr);
    assert_eq!(
        String::from_utf8(validated.stdout).unwrap(),
        "valid splinterm.policy.v2 policy (0 rules)\n"
    );

    let inspected = output(binary().args(["policy", "inspect"]).arg(&policy));
    assert!(inspected.status.success(), "{:?}", inspected.stderr);
    let document: serde_json::Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(document["schema"], "splinterm.policy.v2");
    assert_eq!(document["rules"], serde_json::json!([]));

    fs::write(
        &policy,
        format!(
            r#"{{"schema":"splinterm.policy.v2","rules":[{{"id":"reader","executable":{{"path":"/usr/bin/client","sha256":"{}"}},"scopes":["topology_metadata_read"],"resources":[{{"kind":"daemon"}}],"limits":{{"max_results":1}}}}]}}"#,
            "a".repeat(64)
        ),
    )
    .unwrap();
    let normalized = output(binary().args(["policy", "inspect"]).arg(&policy));
    assert!(normalized.status.success(), "{:?}", normalized.stderr);
    let normalized: serde_json::Value = serde_json::from_slice(&normalized.stdout).unwrap();
    let rule = &normalized["rules"][0];
    assert!(rule.get("expires_at_unix_seconds").is_none());
    assert_eq!(rule["limits"], serde_json::json!({"max_results": 1}));

    fs::write(&policy, r#"{"schema":"splinterm.policy.v1","rules":[]}"#).unwrap();
    let legacy = output(binary().args(["policy", "validate"]).arg(&policy));
    assert!(!legacy.status.success());
    assert!(legacy.stdout.is_empty());
    assert!(
        String::from_utf8(legacy.stderr)
            .unwrap()
            .contains("unsupported policy schema")
    );

    fs::write(&policy, r#"{"schema":"splinterm.policy.v2","rules":[]}"#).unwrap();
    fs::set_permissions(&policy, fs::Permissions::from_mode(0o644)).unwrap();
    let rejected = output(binary().args(["policy", "validate"]).arg(&policy));
    assert!(!rejected.status.success());
    assert!(rejected.stdout.is_empty());
    assert!(
        String::from_utf8(rejected.stderr)
            .unwrap()
            .contains("daemon-owned policy mode must be 0600")
    );

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn policy_reload_invokes_only_the_canonical_user_unit() {
    let directory = test_directory("reload");
    let record = directory.join("record");
    let systemctl = directory.join("systemctl");
    fs::write(
        &systemctl,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >\"$SPLINTERM_TEST_RECORD\"\n",
    )
    .unwrap();
    fs::set_permissions(&systemctl, fs::Permissions::from_mode(0o700)).unwrap();

    let existing_path = env::var_os("PATH").unwrap_or_default();
    let reloaded = output(
        binary()
            .args(["policy", "reload"])
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    directory.display(),
                    PathBuf::from(existing_path).display()
                ),
            )
            .env("SPLINTERM_TEST_RECORD", &record),
    );
    assert!(reloaded.status.success(), "{:?}", reloaded.stderr);
    assert_eq!(
        fs::read_to_string(record).unwrap(),
        "--user reload splinterd.service\n"
    );
    assert!(
        String::from_utf8(reloaded.stdout)
            .unwrap()
            .contains("reload requested")
    );

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn policy_commands_reject_machine_mode_without_touching_stdout() {
    let result = output(binary().args(["--output", "json", "policy", "reload"]));
    assert_eq!(result.status.code(), Some(2));
    assert!(result.stdout.is_empty());
    assert!(!result.stderr.is_empty());
}

#[test]
fn reset_requires_confirmation_and_rejects_machine_mode_before_service_access() {
    let guarded = output(binary().arg("reset"));
    assert!(!guarded.status.success());
    assert!(guarded.stdout.is_empty());
    assert!(
        String::from_utf8(guarded.stderr)
            .unwrap()
            .contains("pass --yes to confirm")
    );

    for arguments in [
        vec!["--output", "json", "reset", "--yes"],
        vec!["--output", "ndjson", "reset", "--yes"],
        vec!["--schema-major", "1", "reset", "--yes"],
        vec!["--timeout-ms", "1000", "reset", "--yes"],
    ] {
        let machine = output(binary().args(arguments));
        assert_eq!(machine.status.code(), Some(2));
        assert!(machine.stdout.is_empty());
        assert!(!machine.stderr.is_empty());
    }
}

#[test]
fn reset_backup_failure_restarts_service_and_preserves_state() {
    let directory = test_directory("reset-backup-failure");
    let state_home = directory.join("state");
    let state = state_home.join("splinterm");
    let runtime = directory.join("runtime");
    let fake_bin = directory.join("bin");
    let record = directory.join("record");
    fs::create_dir_all(&state_home).unwrap();
    fs::create_dir_all(&runtime).unwrap();
    fs::create_dir_all(&fake_bin).unwrap();
    fs::write(&state, b"not-a-directory").unwrap();

    let systemctl = fake_bin.join("systemctl");
    fs::write(
        &systemctl,
        r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >>"$SPLINTERM_TEST_RECORD"
if [ "$2" = start ]; then
  mkdir -p "$XDG_RUNTIME_DIR/splinterm"
  /usr/bin/python -c 'import socket,sys; s=socket.socket(socket.AF_UNIX); s.bind(sys.argv[1]); s.close()' "$XDG_RUNTIME_DIR/splinterm/splinterd.sock"
fi
"#,
    )
    .unwrap();
    fs::set_permissions(&systemctl, fs::Permissions::from_mode(0o700)).unwrap();

    let reset = output(
        binary()
            .args(["reset", "--yes"])
            .env("PATH", format!("{}:/usr/bin", fake_bin.display()))
            .env("XDG_STATE_HOME", &state_home)
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("SPLINTERM_TEST_RECORD", &record),
    );
    assert!(!reset.status.success());
    assert!(reset.stdout.is_empty());
    assert!(
        String::from_utf8(reset.stderr)
            .unwrap()
            .contains("restarted unchanged")
    );
    assert_eq!(fs::read(&state).unwrap(), b"not-a-directory");
    assert_eq!(
        fs::read_to_string(&record).unwrap(),
        "--user stop splinterd.service\n--user start splinterd.service\n"
    );

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn reset_start_failure_reports_the_completed_backup() {
    let directory = test_directory("reset-start-failure");
    let state_home = directory.join("state");
    let state = state_home.join("splinterm");
    let runtime = directory.join("runtime");
    let fake_bin = directory.join("bin");
    fs::create_dir_all(&state).unwrap();
    fs::create_dir_all(&runtime).unwrap();
    fs::create_dir_all(&fake_bin).unwrap();
    fs::write(state.join("lair.json"), b"sessions").unwrap();

    let systemctl = fake_bin.join("systemctl");
    fs::write(&systemctl, "#!/bin/sh\n[ \"$2\" != start ]\n").unwrap();
    fs::set_permissions(&systemctl, fs::Permissions::from_mode(0o700)).unwrap();

    let reset = output(
        binary()
            .args(["reset", "--yes"])
            .env("PATH", format!("{}:/usr/bin", fake_bin.display()))
            .env("XDG_STATE_HOME", &state_home)
            .env("XDG_RUNTIME_DIR", &runtime),
    );
    assert!(!reset.status.success());
    let backups: Vec<_> = fs::read_dir(&state_home)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(backups.len(), 1);
    let stderr = String::from_utf8(reset.stderr).unwrap();
    assert!(stderr.contains("backed up to"));
    assert!(stderr.contains(backups[0].to_string_lossy().as_ref()));

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn reset_is_one_guarded_stop_backup_start_sequence() {
    let directory = test_directory("reset");
    let state_home = directory.join("state");
    let state = state_home.join("splinterm");
    let runtime = directory.join("runtime");
    let socket = directory.join("custom-splinterd.sock");
    let fake_bin = directory.join("bin");
    let record = directory.join("record");
    fs::create_dir_all(&state).unwrap();
    fs::create_dir_all(&runtime).unwrap();
    fs::create_dir_all(&fake_bin).unwrap();
    fs::write(state.join("lair.json"), b"sessions").unwrap();

    let systemctl = fake_bin.join("systemctl");
    fs::write(
        &systemctl,
        r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >>"$SPLINTERM_TEST_RECORD"
if [ "$2" = stop ]; then
  rm -f "$SPLINTERM_SOCKET"
elif [ "$2" = start ]; then
  /usr/bin/python -c 'import socket,sys; s=socket.socket(socket.AF_UNIX); s.bind(sys.argv[1]); s.close()' "$SPLINTERM_SOCKET"
fi
"#,
    )
    .unwrap();
    fs::set_permissions(&systemctl, fs::Permissions::from_mode(0o700)).unwrap();

    let reset = output(
        binary()
            .args(["reset", "--yes"])
            .env("PATH", format!("{}:/usr/bin", fake_bin.display()))
            .env("XDG_STATE_HOME", &state_home)
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("SPLINTERM_SOCKET", &socket)
            .env("SPLINTERM_TEST_RECORD", &record),
    );
    assert!(reset.status.success(), "{:?}", reset.stderr);
    assert_eq!(
        fs::read_to_string(&record).unwrap(),
        "--user stop splinterd.service\n--user start splinterd.service\n"
    );
    assert!(!state.exists());
    let backups: Vec<_> = fs::read_dir(&state_home)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("splinterm.reset-")
        })
        .collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(fs::read(backups[0].join("lair.json")).unwrap(), b"sessions");
    let stdout = String::from_utf8(reset.stdout).unwrap();
    assert!(stdout.contains("restarted with no sessions"));
    assert!(stdout.contains(backups[0].to_string_lossy().as_ref()));

    fs::remove_dir_all(directory).unwrap();
}
