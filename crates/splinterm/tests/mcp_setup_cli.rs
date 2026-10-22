//! `splinterm mcp setup|remove` end to end, in private temporary homes only
//! (ADR 0014, Plan 0053 PR 6). The machine documents are checked against the
//! checked-in contract fixtures, which `validate-contract-fixtures.py` checks
//! against the public schema.

use std::{
    env, fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

use serde_json::Value;

static NEXT: AtomicU64 = AtomicU64::new(1);

struct Home {
    root: PathBuf,
    adapter: PathBuf,
}

impl Home {
    fn new() -> Self {
        let root = env::temp_dir().join(format!(
            "splinterm-mcp-setup-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let adapter = root.join("bin/splinterm-mcp");
        fs::create_dir(root.join("bin")).unwrap();
        fs::write(&adapter, "#!/bin/false\n").unwrap();
        fs::set_permissions(&adapter, fs::Permissions::from_mode(0o755)).unwrap();
        for directory in [".config", ".config/Code", ".config/Code/User"] {
            fs::create_dir(root.join(directory)).unwrap();
            fs::set_permissions(root.join(directory), fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::set_permissions(root.join("bin"), fs::Permissions::from_mode(0o755)).unwrap();
        Self { root, adapter }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_splinterm"));
        command
            .env("HOME", &self.root)
            .env("XDG_CONFIG_HOME", self.root.join(".config"))
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .env_remove("SPLINTERM_SOCKET");
        command
    }

    fn run(&self, arguments: &[&str]) -> Output {
        self.command().args(arguments).output().unwrap()
    }

    fn adapter(&self) -> &str {
        self.adapter.to_str().unwrap()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn document(output: &Output) -> Value {
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    assert_eq!(stdout.lines().count(), 1, "one document: {stdout}");
    serde_json::from_str(&stdout).unwrap()
}

fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/automation/fixtures/valid")
        .join(name);
    let fixture: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    fixture["document"].clone()
}

/// The same keys at every depth and the same JSON type at every leaf.
fn assert_same_shape(actual: &Value, expected: &Value, at: &str) {
    match (actual, expected) {
        (Value::Object(actual), Value::Object(expected)) => {
            let mut actual_keys: Vec<_> = actual.keys().collect();
            let mut expected_keys: Vec<_> = expected.keys().collect();
            actual_keys.sort();
            expected_keys.sort();
            assert_eq!(actual_keys, expected_keys, "keys at {at}");
            for (key, value) in actual {
                assert_same_shape(value, &expected[key], &format!("{at}/{key}"));
            }
        }
        (Value::Array(actual), Value::Array(expected)) => {
            assert_eq!(actual.len(), expected.len(), "length at {at}");
            for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                assert_same_shape(actual, expected, &format!("{at}/{index}"));
            }
        }
        (Value::String(_), Value::String(_))
        | (Value::Bool(_), Value::Bool(_))
        | (Value::Number(_), Value::Number(_))
        | (Value::Null, Value::Null) => {}
        _ => panic!("type mismatch at {at}: {actual} vs {expected}"),
    }
}

/// Conformance with a fixture: the same shape, and every value equal except
/// the temporary paths, which keep the fixture's file name.
fn assert_conforms(actual: &Value, fixture_name: &str, home: &Home) {
    let expected = fixture(fixture_name);
    assert_same_shape(actual, &expected, "");
    for key in ["schema", "request_id", "operation", "ok", "truncated"] {
        assert_eq!(actual[key], expected[key], "{key}");
    }
    if let Some(data) = expected.get("data") {
        for key in [
            "host",
            "config_format",
            "server_name",
            "action",
            "changed",
            "dry_run",
            "written",
            "grants_access",
        ] {
            assert_eq!(actual["data"][key], data[key], "data/{key}");
        }
        let path = actual["data"]["config_path"].as_str().unwrap();
        assert!(path.starts_with(home.root.to_str().unwrap()), "{path}");
        let expected_path = data["config_path"].as_str().unwrap();
        let suffix = &expected_path[expected_path.find("/.").unwrap()..];
        assert!(path.ends_with(suffix), "{path} ends with {suffix}");
        if data.get("adapter_path").is_some() {
            assert_eq!(actual["data"]["adapter_path"], home.adapter());
            // A temporary home's adapter is the user's own: never "Always".
            assert_eq!(actual["data"]["adapter_identity"], "unprotected");
            assert_eq!(actual["data"]["adapter_durable"], false);
            assert_eq!(actual["data"]["entry"]["command"], home.adapter());
            let mut entry = actual["data"]["entry"].clone();
            entry["command"] = data["entry"]["command"].clone();
            assert_eq!(entry, data["entry"]);
        }
    } else {
        assert_eq!(actual["error"]["code"], expected["error"]["code"]);
        assert_eq!(actual["error"]["retryable"], expected["error"]["retryable"]);
    }
}

#[test]
fn machine_documents_conform_to_the_contract_fixtures() {
    let home = Home::new();
    let adapter = home.adapter();

    let claude_path = home.root.join(".claude.json");
    fs::write(&claude_path, "{\n  \"numStartups\": 1\n}\n").unwrap();
    fs::set_permissions(&claude_path, fs::Permissions::from_mode(0o600)).unwrap();
    let claude = home.run(&[
        "--output",
        "json",
        "mcp",
        "setup",
        "claude-code",
        "--adapter",
        adapter,
    ]);
    assert!(claude.status.success(), "{claude:?}");
    assert_conforms(&document(&claude), "cli-mcp-setup.json", &home);
    let text = fs::read_to_string(&claude_path).unwrap();
    assert!(
        text.starts_with("{\n  \"numStartups\": 1,\n  \"mcpServers\": {\n"),
        "{text}"
    );

    let dry = home.run(&[
        "--output",
        "json",
        "mcp",
        "setup",
        "codex",
        "--dry-run",
        "--adapter",
        adapter,
    ]);
    assert!(dry.status.success(), "{dry:?}");
    assert_conforms(&document(&dry), "cli-mcp-setup-codex-dry-run.json", &home);
    assert!(!home.root.join(".codex").exists());

    let first = home.run(&[
        "--output",
        "json",
        "mcp",
        "setup",
        "vscode",
        "--adapter",
        adapter,
    ]);
    assert!(first.status.success(), "{first:?}");
    assert_eq!(document(&first)["data"]["action"], "created");
    let again = home.run(&[
        "--output",
        "json",
        "mcp",
        "setup",
        "vscode",
        "--adapter",
        adapter,
    ]);
    assert!(again.status.success(), "{again:?}");
    assert_conforms(&document(&again), "cli-mcp-setup-unchanged.json", &home);

    let codex = home.run(&[
        "--output",
        "json",
        "mcp",
        "setup",
        "codex",
        "--adapter",
        adapter,
    ]);
    assert!(codex.status.success(), "{codex:?}");
    let remove = home.run(&["--output", "json", "mcp", "remove", "codex"]);
    assert!(remove.status.success(), "{remove:?}");
    assert_conforms(&document(&remove), "cli-mcp-remove.json", &home);
}

#[test]
fn machine_failures_are_typed_documents() {
    let home = Home::new();
    let custom = "{\"mcpServers\": {\"splinterm\": {\"command\": \"/elsewhere\"}}}\n";
    fs::write(home.root.join(".claude.json"), custom).unwrap();
    fs::set_permissions(
        home.root.join(".claude.json"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let conflict = home.run(&[
        "--output",
        "json",
        "mcp",
        "setup",
        "claude-code",
        "--adapter",
        home.adapter(),
    ]);
    assert_eq!(conflict.status.code(), Some(3), "{conflict:?}");
    assert_conforms(&document(&conflict), "cli-mcp-setup-conflict.json", &home);
    assert_eq!(
        fs::read_to_string(home.root.join(".claude.json")).unwrap(),
        custom
    );

    let missing = home.root.join("nowhere/splinterm-mcp");
    let output = home.run(&[
        "--output",
        "json",
        "mcp",
        "setup",
        "codex",
        "--adapter",
        missing.to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(5), "{output:?}");
    let failure = document(&output);
    assert_eq!(failure["operation"], "mcp_setup");
    assert_eq!(failure["error"]["code"], "not_found");
    assert!(
        failure["error"]["message"]
            .as_str()
            .unwrap()
            .contains("splinterm-mcp package")
    );
    assert!(!home.root.join(".codex").exists());

    fs::write(home.root.join(".claude.json"), "{\"a\": 1, \"a\": 2}").unwrap();
    let duplicate = home.run(&["--output", "json", "mcp", "remove", "claude-code"]);
    assert_eq!(duplicate.status.code(), Some(5), "{duplicate:?}");
    assert_eq!(document(&duplicate)["error"]["code"], "invalid_argument");

    let foreign = "[mcp_servers.splinterm]\ncommand = \"/usr/bin/other-tool\"\n";
    fs::create_dir(home.root.join(".codex")).unwrap();
    fs::set_permissions(home.root.join(".codex"), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(home.root.join(".codex/config.toml"), foreign).unwrap();
    fs::set_permissions(
        home.root.join(".codex/config.toml"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let refused = home.run(&["--output", "json", "mcp", "remove", "codex"]);
    assert_eq!(refused.status.code(), Some(3), "{refused:?}");
    assert_eq!(document(&refused)["error"]["code"], "confirmation_required");
    assert_eq!(
        fs::read_to_string(home.root.join(".codex/config.toml")).unwrap(),
        foreign
    );
    let forced = home.run(&["--output", "json", "mcp", "remove", "codex", "--force"]);
    assert!(forced.status.success(), "{forced:?}");
    assert_eq!(document(&forced)["data"]["action"], "removed");

    let schema = home.run(&[
        "--output",
        "json",
        "--schema-major",
        "3",
        "mcp",
        "remove",
        "codex",
    ]);
    assert_eq!(schema.status.code(), Some(4), "{schema:?}");
    assert_eq!(document(&schema)["error"]["code"], "unsupported_schema");
}

#[test]
fn human_setup_says_it_grants_nothing_and_a_dry_run_writes_nothing() {
    let home = Home::new();
    let dry = home.run(&[
        "mcp",
        "setup",
        "codex",
        "--dry-run",
        "--adapter",
        home.adapter(),
    ]);
    assert!(dry.status.success(), "{dry:?}");
    let text = String::from_utf8(dry.stdout).unwrap();
    assert!(text.starts_with("Dry run: this would create "), "{text}");
    assert!(text.contains("+[mcp_servers.splinterm]\n"), "{text}");
    assert!(text.contains("Nothing was written."));
    assert!(text.contains("grants no access and writes no policy"));
    assert!(!home.root.join(".codex").exists());

    let setup = home.run(&["mcp", "setup", "codex", "--adapter", home.adapter()]);
    assert!(setup.status.success(), "{setup:?}");
    let text = String::from_utf8(setup.stdout).unwrap();
    assert!(text.starts_with("Created "), "{text}");
    assert!(text.contains("grants no access and writes no policy"));
    assert!(text.contains("asks for access to a workspace the first time"));

    let remove = home.run(&["mcp", "remove", "codex"]);
    assert!(remove.status.success(), "{remove:?}");
    let text = String::from_utf8(remove.stdout).unwrap();
    assert!(text.contains("revokes nothing"), "{text}");

    let failure = home.run(&[
        "mcp",
        "setup",
        "claude-code",
        "--adapter",
        "/nonexistent/splinterm-mcp",
    ]);
    assert_eq!(failure.status.code(), Some(1));
    assert!(
        String::from_utf8(failure.stderr)
            .unwrap()
            .contains("--adapter PATH")
    );
}

#[test]
fn unsupported_hosts_and_options_are_usage_errors() {
    let home = Home::new();
    let output = home.run(&["mcp", "setup", "cursor"]);
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    for host in ["claude-code", "codex", "vscode"] {
        assert!(stderr.contains(host), "{stderr}");
    }
    for arguments in [
        &["--remote", "box", "mcp", "setup", "codex"][..],
        &["--timeout-ms", "100", "mcp", "setup", "codex"][..],
        &["--output", "ndjson", "mcp", "setup", "codex"][..],
        &["--schema-major", "2", "mcp", "setup", "codex"][..],
        &["mcp", "remove", "codex", "--replace"][..],
        &["mcp", "remove", "codex", "--adapter", "/x"][..],
        &["mcp", "setup", "codex", "--force"][..],
    ] {
        let output = home.run(arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}: {output:?}");
    }
    let mut created: Vec<_> = fs::read_dir(&home.root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    created.sort();
    assert_eq!(created, [".config", "bin"]);
}
