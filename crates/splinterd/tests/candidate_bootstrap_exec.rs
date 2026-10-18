//! Real exec of the `splinterd` entrypoint as an idle handoff candidate.
//!
//! Each case launches the actual binary from a sealed memfd with a sealed
//! forward/rollback manifest pair and every idle descriptor role inherited.
//! The candidate claims, verifies, and stages them, then adopts: the
//! fixture's forward pair is not installed at its recorded prefix, so
//! adoption refuses before its commit. Until the rollback executor exists
//! (Plan 0061 M3.11) that refusal is a loud, non-zero exit with diagnostics,
//! which these cases pin as the pre-commit failure behaviour. The candidate
//! never accepts on the fixture listeners, and it runs with a private state
//! and configuration home and no session bus, so it touches no installed
//! daemon, user state, or systemd manager.

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

/// The candidate activation flag `candidate_acquire` owns in the binary.
const ACTIVATION_ARGUMENT: &str = "--splinterm-internal-handoff-manifest-fd-v1";
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

    /// Runs the candidate activated on `activation_fd` with no daemon
    /// arguments.
    fn launch(&self, activation_fd: RawFd) -> Output {
        self.launch_with(&[activation(activation_fd)], &[])
    }

    /// Runs the binary with exactly `arguments` and every role inherited,
    /// restoring close-on-exec on the fixture owners afterward.
    fn launch_with(&self, arguments: &[String], environment: &[(&str, &str)]) -> Output {
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
        .args(arguments)
        .envs(environment.iter().copied())
        .env("SPLINTERM_SOCKET", &self.listener_path)
        .env("XDG_STATE_HOME", self.directory.0.join("state"))
        .env("XDG_CONFIG_HOME", self.directory.0.join("config"))
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent/splinterd-test-bus",
        )
        .env_remove("SPLINTERM_POLICY")
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

/// The pre-commit refusal every case below reaches once staging succeeds.
const ADOPTION_REFUSAL: &str = "failed before its adoption commit (adoption)";

/// Staging accepts every role, and adoption then refuses the fixture's
/// uninstalled forward pair. Before Plan 0061 M3.11 the refusal exits
/// non-zero, naming the failed phase and cause; exiting closes the claimed
/// descriptors, which is all it ends (the fixture carries no Splint).
#[test]
fn sealed_idle_candidate_stages_every_role_then_fails_adoption_loudly() {
    let fixture = Fixture::new(None);
    let output = fixture.launch(fixture.forward_manifest_fd);
    assert_candidate_error(&output, ADOPTION_REFUSAL);
    assert_candidate_error(
        &output,
        "the install prefix does not hold the forward pair the manifest recorded",
    );
    assert_candidate_error(&output, "live-process continuity is not guaranteed");
    // Adoption ran no further than its first check: nothing was persisted.
    assert!(
        !fixture
            .directory
            .0
            .join("state/splinterm/topology.json")
            .exists()
    );
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

const CANDIDATE_REFUSAL: &str = "started as an upgrade-handoff candidate and will not serve";

#[test]
fn candidate_accepts_the_packaged_daemon_argument_after_activation() {
    let fixture = Fixture::new(None);
    let output = fixture.launch_with(
        &[
            activation(fixture.forward_manifest_fd),
            "--require-workload-cgroups".to_owned(),
        ],
        &[],
    );
    assert_candidate_error(&output, ADOPTION_REFUSAL);
}

#[test]
fn malformed_activation_fails_closed_as_a_candidate_before_claiming() {
    let fixture = Fixture::new(None);
    let valid = activation(fixture.forward_manifest_fd);
    for (arguments, expected) in [
        (
            vec![format!("{ACTIVATION_ARGUMENT}=2")],
            "candidate manifest descriptor is a standard stream",
        ),
        (
            vec![format!(
                "{ACTIVATION_ARGUMENT}=0{}",
                fixture.forward_manifest_fd
            )],
            "candidate manifest descriptor is not a canonical decimal number",
        ),
        (
            vec![ACTIVATION_ARGUMENT.to_owned()],
            "candidate activation names no manifest descriptor",
        ),
        (
            vec![valid.clone(), valid.clone()],
            "candidate activation appears more than once",
        ),
        (
            vec!["--require-workload-cgroups".to_owned(), valid.clone()],
            "candidate activation is not the first argument",
        ),
        (
            vec![format!(
                "--splinterm-internal-handoff-manifest-fd={}",
                fixture.forward_manifest_fd
            )],
            "candidate activation argument is not recognized",
        ),
        // The daemon arguments are judged before any descriptor is claimed.
        (
            vec![valid.clone(), "--unknown".to_owned()],
            "unsupported splinterd argument",
        ),
    ] {
        let output = fixture.launch_with(&arguments, &[]);
        assert_candidate_error(&output, CANDIDATE_REFUSAL);
        assert_candidate_error(&output, expected);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("not inherited") && !stderr.contains("adoption commit"),
            "{arguments:?} reached the manifest: {stderr}"
        );
    }
}

#[test]
fn the_retired_activation_environment_variable_is_ignored() {
    let fixture = Fixture::new(None);
    // The removed variable once failed closed as a candidate on this invalid
    // value; now only argv activates, so the stray argument fails ordinary
    // startup before anything else runs.
    let output = fixture.launch_with(
        &["--unknown".to_owned()],
        &[("SPLINTERM_INTERNAL_HANDOFF_MANIFEST_FD", "2")],
    );
    assert_candidate_error(&output, "unsupported splinterd argument");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("candidate"), "{stderr}");
}

/// The packaging guard refuses a binary containing the build-salt marker, so
/// every `integration-test` daemon must contain it; and a salted copy needs
/// exactly one patchable slot, the static: the marker followed by an
/// all-zero salt. Other copies of the marker constant (debug info, a
/// materialized constant) may appear and are allowed. A refactor that
/// dropped `#[used]` or the static's only reference would otherwise let the
/// guard pass a feature build silently.
#[test]
fn the_integration_test_daemon_carries_the_salt_marker_and_one_salt_slot() {
    use splinterd::integration_test_seams::{BUILD_SALT_MARKER, salt_slot_offset};

    let daemon = fs::read(env!("CARGO_BIN_EXE_splinterd")).unwrap();
    let occurrences = daemon
        .windows(BUILD_SALT_MARKER.len())
        .filter(|window| *window == BUILD_SALT_MARKER.as_slice())
        .count();
    assert!(occurrences >= 1, "the packaging guard's marker is missing");
    assert!(
        salt_slot_offset(&daemon).is_some(),
        "{occurrences} marker occurrences, but not exactly one patchable salt slot"
    );
}

fn activation(manifest_fd: RawFd) -> String {
    format!("{ACTIVATION_ARGUMENT}={manifest_fd}")
}
