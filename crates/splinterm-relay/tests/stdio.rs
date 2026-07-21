use std::{
    fs,
    os::unix::{
        fs::{PermissionsExt, symlink},
        net::UnixListener,
    },
    path::PathBuf,
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

const RELAY: &str = env!("CARGO_BIN_EXE_splinterm-relay");

fn test_directory(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "splinterm-relay-cli-{label}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

fn output_for(socket: &PathBuf) -> std::process::Output {
    Command::new(RELAY)
        .arg("--stdio")
        .env("SPLINTERM_SOCKET", socket)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap()
}

#[test]
fn invalid_invocation_and_unsafe_sockets_fail_on_stderr_only() {
    let invocation = Command::new(RELAY).output().unwrap();
    assert_eq!(invocation.status.code(), Some(2));
    assert!(invocation.stdout.is_empty());
    assert!(String::from_utf8_lossy(&invocation.stderr).contains("usage:"));

    let directory = test_directory("unsafe-parent");
    let socket = directory.join("daemon.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
    let unsafe_parent = output_for(&socket);
    assert_eq!(unsafe_parent.status.code(), Some(1));
    assert!(unsafe_parent.stdout.is_empty());
    assert!(String::from_utf8_lossy(&unsafe_parent.stderr).contains("owner-only"));
    drop(listener);
    fs::remove_dir_all(&directory).unwrap();

    let directory = test_directory("unsafe-endpoint");
    let socket = directory.join("daemon.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o660)).unwrap();
    let unsafe_endpoint = output_for(&socket);
    assert_eq!(unsafe_endpoint.status.code(), Some(1));
    assert!(unsafe_endpoint.stdout.is_empty());
    assert!(String::from_utf8_lossy(&unsafe_endpoint.stderr).contains("owner-only Unix socket"));
    drop(listener);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn symlinked_parent_and_endpoint_are_rejected() {
    let directory = test_directory("symlinks");
    let real = directory.join("real");
    fs::create_dir(&real).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
    let socket = real.join("daemon.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();

    let linked_parent = directory.join("linked");
    symlink(&real, &linked_parent).unwrap();
    let parent_result = output_for(&linked_parent.join("daemon.sock"));
    assert_eq!(parent_result.status.code(), Some(1));
    assert!(parent_result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&parent_result.stderr).contains("symlinks"));

    let linked_endpoint = real.join("linked.sock");
    symlink(&socket, &linked_endpoint).unwrap();
    let endpoint_result = output_for(&linked_endpoint);
    assert_eq!(endpoint_result.status.code(), Some(1));
    assert!(endpoint_result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&endpoint_result.stderr).contains("Unix socket"));

    drop(listener);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn relative_and_dot_component_paths_are_rejected() {
    for path in [
        PathBuf::from("relative.sock"),
        PathBuf::from("/tmp/../tmp/relay.sock"),
    ] {
        let output = output_for(&path);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("absolute and normalized"));
    }
}
