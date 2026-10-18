//! Real exec of the `splinterd` entrypoint as an idle handoff candidate.
//!
//! Each case launches the actual binary from a sealed memfd with a sealed
//! forward/rollback manifest pair and every idle descriptor role inherited.
//! The candidate claims, verifies, and stages them, then stops at the
//! deliberate adoption-not-wired boundary. It never binds or accepts on the
//! fixture listeners and touches no installed daemon.

#![cfg(feature = "integration-test")]

use std::{
    fs,
    os::{
        fd::{AsFd, AsRawFd, OwnedFd, RawFd},
        unix::{fs::PermissionsExt, net::UnixListener},
    },
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use rustix::{
    fs::{MemfdFlags, fcntl_add_seals, fstat, memfd_create},
    io::{FdFlags, fcntl_getfd, fcntl_setfd, write},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use splinterd::{
    encode_manifest_pair_for_integration_test, executable_snapshot::REQUIRED_EXECUTABLE_SEALS,
    handoff_descriptors::HandoffDescriptorSlot,
};

const MANIFEST_ENV: &str = "SPLINTERM_INTERNAL_HANDOFF_MANIFEST_FD";
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "sd-cand-{}-{unique}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn generation(byte: u8) -> Value {
    json!({
        "build_identity": vec![byte; 32],
        "build_version": {
            "major": 0,
            "minor": 2,
            "patch": u16::from(byte),
        },
    })
}

fn write_all(fd: &OwnedFd, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        let count = write(fd, bytes).unwrap();
        assert!(count > 0);
        bytes = &bytes[count..];
    }
}

fn sealed_bytes(name: &str, bytes: &[u8]) -> OwnedFd {
    let fd = memfd_create(
        name,
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING | MemfdFlags::EXEC,
    )
    .unwrap();
    write_all(&fd, bytes);
    fcntl_add_seals(&fd, REQUIRED_EXECUTABLE_SEALS).unwrap();
    fd
}

fn executable_image(fd: &OwnedFd, bytes: &[u8]) -> Value {
    let stat = fstat(fd).unwrap();
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    json!({
        "source_device": stat.st_dev.saturating_add(10_000),
        "source_inode": stat.st_ino.saturating_add(10_000),
        "source_owner_uid": 0,
        "source_mode": 0o100_755,
        "source_size": bytes.len() as u64,
        "source_sha256": digest,
        "snapshot_device": stat.st_dev,
        "snapshot_inode": stat.st_ino,
        "snapshot_size": bytes.len() as u64,
        "snapshot_sha256": digest,
        "snapshot_seals": REQUIRED_EXECUTABLE_SEALS.bits(),
    })
}

fn empty_manifest_memfd(name: &str) -> OwnedFd {
    memfd_create(name, MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING).unwrap()
}

fn seal_manifest(fd: &OwnedFd, bytes: &[u8]) {
    write_all(fd, bytes);
    fcntl_add_seals(fd, REQUIRED_EXECUTABLE_SEALS).unwrap();
}

/// Every idle-candidate owner, kept alive across one launch.
struct Fixture {
    directory: TestDirectory,
    listener_path: PathBuf,
    daemon: OwnedFd,
    /// Owners in canonical manifest order, each paired with its role.
    inherited: Vec<(HandoffDescriptorSlot, OwnedFd)>,
    forward_manifest_fd: RawFd,
}

const FORWARD_CLIENT: &[u8] = b"forward-client-fixture";
const ROLLBACK_DAEMON: &[u8] = b"rollback-daemon-fixture";
const ROLLBACK_CLIENT: &[u8] = b"rollback-client-fixture";

impl Fixture {
    /// Builds and seals the manifest pair; `listener` replaces the control
    /// listener owner when given.
    #[allow(
        clippy::too_many_lines,
        reason = "one fixture binds every idle role into one sealed manifest pair"
    )]
    fn new(listener: Option<OwnedFd>) -> Self {
        let directory = TestDirectory::new();
        let listener_path = directory.0.join("daemon.sock");
        let image_listener_path = splinterm_protocol::image_content_socket_path(&listener_path);
        let control = UnixListener::bind(&listener_path).unwrap();
        let image = UnixListener::bind(&image_listener_path).unwrap();
        control.set_nonblocking(true).unwrap();
        image.set_nonblocking(true).unwrap();
        fs::set_permissions(&listener_path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&image_listener_path, fs::Permissions::from_mode(0o600)).unwrap();
        let control = listener.unwrap_or_else(|| control.into());

        let daemon_bytes = fs::read(env!("CARGO_BIN_EXE_splinterd")).unwrap();
        let daemon = sealed_bytes("candidate-forward-daemon", &daemon_bytes);
        let forward_client = sealed_bytes("candidate-forward-client", FORWARD_CLIENT);
        let rollback_daemon = sealed_bytes("candidate-rollback-daemon", ROLLBACK_DAEMON);
        let rollback_client = sealed_bytes("candidate-rollback-client", ROLLBACK_CLIENT);
        let forward_manifest = empty_manifest_memfd("candidate-forward-manifest");
        let rollback_manifest = empty_manifest_memfd("candidate-rollback-manifest");

        let source = generation(1);
        let compiled = splinterd::handoff_preflight::compiled_handoff_generation().unwrap();
        let target = json!({
            "build_identity": compiled.build_identity.as_bytes(),
            "build_version": {
                "major": compiled.build_version.major,
                "minor": compiled.build_version.minor,
                "patch": compiled.build_version.patch,
            },
        });
        let executable_pairs = json!({
            "forward": {
                "daemon": executable_image(&daemon, &daemon_bytes),
                "client": executable_image(&forward_client, FORWARD_CLIENT),
                "generation": target,
            },
            "rollback": {
                "daemon": executable_image(&rollback_daemon, ROLLBACK_DAEMON),
                "client": executable_image(&rollback_client, ROLLBACK_CLIENT),
                "generation": source,
            },
        });
        let inherited = vec![
            (HandoffDescriptorSlot::Listener, control),
            (HandoffDescriptorSlot::ImageListener, image.into()),
            (HandoffDescriptorSlot::CheckpointManifest, forward_manifest),
            (HandoffDescriptorSlot::RollbackManifest, rollback_manifest),
            (HandoffDescriptorSlot::ForwardClient, forward_client),
            (HandoffDescriptorSlot::RollbackDaemon, rollback_daemon),
            (HandoffDescriptorSlot::RollbackClient, rollback_client),
        ];
        let descriptors = inherited
            .iter()
            .map(|(slot, fd)| {
                json!({
                    "slot": slot,
                    "inherited_fd": u32::try_from(fd.as_raw_fd()).unwrap(),
                })
            })
            .collect::<Vec<_>>();
        let forward = json!({
            "schema": 1,
            "kind": "forward",
            "handoff_id": vec![7; 16],
            "source_generation": source.clone(),
            "target_generation": target.clone(),
            "source_daemon_generation": 1,
            "target_daemon_generation": 2,
            "topology_revision": 1,
            "durable_state_generation": 1,
            "private_protocol": compiled.capabilities.private_protocol.maximum(),
            "handoff_protocol": compiled.capabilities.handoff_protocol.maximum(),
            "terminal_checkpoint_schema": compiled.capabilities.terminal_checkpoint.maximum(),
            "descriptor_manifest_schema": compiled.capabilities.descriptor_manifest.maximum(),
            "install_prefix": "/usr/bin",
            "executable_pairs": executable_pairs,
            "counterpart_manifest_slot": HandoffDescriptorSlot::RollbackManifest,
            "splints": [],
            "descriptors": descriptors,
        });
        let mut rollback = forward.clone();
        rollback["kind"] = json!("rollback");
        rollback["source_generation"] = target;
        rollback["target_generation"] = source;
        rollback["source_daemon_generation"] = json!(2);
        rollback["target_daemon_generation"] = json!(1);
        rollback["counterpart_manifest_slot"] = json!(HandoffDescriptorSlot::CheckpointManifest);
        let (forward_bytes, rollback_bytes) =
            encode_manifest_pair_for_integration_test(forward, rollback).unwrap();
        seal_manifest(&inherited[2].1, &forward_bytes);
        seal_manifest(&inherited[3].1, &rollback_bytes);
        let forward_manifest_fd = inherited[2].1.as_raw_fd();
        Self {
            directory,
            listener_path,
            daemon,
            inherited,
            forward_manifest_fd,
        }
    }

    /// Runs the candidate with every role inherited, restoring close-on-exec
    /// on the fixture owners afterward.
    fn launch(&self, activation_fd: RawFd) -> Output {
        let original_flags = self
            .inherited
            .iter()
            .map(|(_, fd)| {
                let flags = fcntl_getfd(fd).unwrap();
                fcntl_setfd(fd, flags - FdFlags::CLOEXEC).unwrap();
                flags
            })
            .collect::<Vec<_>>();
        let output = Command::new(Path::new(&format!(
            "/proc/self/fd/{}",
            self.daemon.as_raw_fd()
        )))
        .env(MANIFEST_ENV, activation_fd.to_string())
        .env("SPLINTERM_SOCKET", &self.listener_path)
        .env_remove("RUST_LOG")
        .current_dir(&self.directory.0)
        .output()
        .unwrap();
        for ((_, fd), flags) in self.inherited.iter().zip(original_flags) {
            fcntl_setfd(fd.as_fd(), flags).unwrap();
        }
        output
    }
}

fn assert_candidate_error(output: &Output, expected: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "candidate succeeded: {stderr}");
    assert!(
        stderr.contains(expected),
        "expected {expected:?}, got: {stderr}"
    );
}

#[test]
fn sealed_idle_candidate_exec_claims_verifies_and_stages_every_role() {
    let fixture = Fixture::new(None);
    let output = fixture.launch(fixture.forward_manifest_fd);
    assert_candidate_error(&output, "candidate runtime adoption is not wired");
}

#[test]
fn candidate_rejects_a_sealed_body_in_the_listener_role() {
    // A sealed memfd satisfies every manifest and claim check for its number,
    // so only the per-role kernel type check can reject it.
    let impostor = sealed_bytes("candidate-impostor-listener", b"not a listener");
    let fixture = Fixture::new(Some(impostor));
    let output = fixture.launch(fixture.forward_manifest_fd);
    assert_candidate_error(&output, "Listener");
    assert_candidate_error(&output, "wrong kernel object type");
}

#[test]
fn candidate_rejects_an_activation_descriptor_that_was_not_inherited() {
    let fixture = Fixture::new(None);
    // A number this process never opened cannot have crossed exec.
    let absent = 1_000_000;
    let output = fixture.launch(absent);
    assert_candidate_error(
        &output,
        &format!("manifest descriptor {absent} was not inherited"),
    );
}
