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
    fs::write(&path, r#"{"schema":"splinterm.policy.v1","rules":[]}"#).unwrap();
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
        "valid splinterm.policy.v1 policy (0 rules)\n"
    );

    let inspected = output(binary().args(["policy", "inspect"]).arg(&policy));
    assert!(inspected.status.success(), "{:?}", inspected.stderr);
    let document: serde_json::Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(document["schema"], "splinterm.policy.v1");
    assert_eq!(document["rules"], serde_json::json!([]));

    fs::write(
        &policy,
        format!(
            r#"{{"schema":"splinterm.policy.v1","rules":[{{"id":"reader","executable":{{"path":"/usr/bin/client","sha256":"{}"}},"scopes":["topology_metadata_read"],"resources":[{{"kind":"lair"}}],"limits":{{"max_results":1}}}}]}}"#,
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
