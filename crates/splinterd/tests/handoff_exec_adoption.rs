//! A complete two-generation daemon handoff through a real exec (Plan 0061
//! slice M3.10b), and its cooperative rollback through a second real exec
//! (M3.11a).
//!
//! The old generation runs in a stage process: it installs an unsalted copy
//! of the `splinterd` it was built with at a private prefix and retains that
//! pair as its rollback authority, then "upgrades" the prefix to a salted
//! copy of the same build (Plan 0061 D9) and a new client, and seals the
//! forward pair. It spawns live PTY Splints and one that exits at once,
//! captures and hands off the live ones, stops its runtime, writes the
//! durable Topology, builds the manifest pair, and crosses the real exec
//! boundary ([`cross_exec_boundary`] with
//! [`ExecInheritancePlan::with_forward_daemon`]): `execveat` of the sealed
//! forward daemon, in the same process. The forward daemon claims, stages,
//! adopts, commits, and serves as generation 2. The parent then checks the
//! adopted daemon over its real sockets and shuts it down.
//!
//! # Rollback
//!
//! The other cases make the forward candidate fail before its commit: the
//! forward client disappears from the prefix after the manifests are built
//! (adoption refuses the prefix), or a test-only fault point fires
//! (`CANDIDATE_FAULT_ENV`, one case per pre-commit step). The candidate's
//! rollback executor then re-executes the sealed rollback daemon snapshot
//! (the unsalted copy) with the unchanged descriptors and the rollback
//! manifest, in the same process again, and the parent checks that the
//! original generation 1 serves every carried Splint with its process and
//! incarnation. A failure on the rollback generation's own adoption has no
//! rollback edge and exits loudly with no further exec. Two cases leave a
//! SIGTERM pending across exec (raised inside the exec boundary, after its
//! own block), which must become an ordered shutdown once the adopting
//! generation installed its handler, never a killed candidate. A rollback
//! image whose digest does not match its manifest is never executed, and a
//! stray inheritable descriptor in the failed candidate crosses the rollback
//! exec to be closed by the rollback daemon's claim instead of costing every
//! Splint. After every adoption and rollback, no daemon thread and no newly
//! launched Splint holds SIGINT, SIGTERM, or SIGHUP blocked.
//!
//! # Why the boundary is driven at the function level
//!
//! The real request path (a trusted-UI `PrepareDaemonHandoff { Execute }`
//! to a running `splinterd`) cannot reach exec in a test build, whatever the
//! client: a `splinterm` copy at the private prefix would satisfy
//! trusted-UI matching, but
//!
//! * `cargo test` builds are unpackaged (zero source commit), so the report
//!   names `NotPackaged` and `Execute` is refused before the gate;
//! * the running daemon retains rollback authority, and finds the forward
//!   pair's package record, only beside `/usr/bin` or a `/nix/store` output
//!   (`package_provenance_path`), never at a private prefix, and a package
//!   record for an unpackaged build is refused outright;
//! * forward preparation preflights the client too, and no salted
//!   `integration-test` `splinterm` is built by `cargo test -p splinterd`.
//!
//! So this test builds the manifests from the stage's real captures through
//! `encode_manifest_pair_for_integration_test`, and everything from the
//! exec boundary on is the production code. The salted forward pair's
//! package provenance treatment is pinned by
//! `package_provenance::tests::a_salted_record_verifies_only_with_its_own_salt`.
//!
//! # Isolation
//!
//! Each stage runs with a private socket, state, configuration home, and
//! prefix, and no session bus, so no workload unit manager exists on either
//! side: every Splint runs uncontained and no scope can be stopped. The
//! stage starts without inherited descriptors (`close_fds`) and with only
//! its main thread, as the process guard requires. The parent kills every
//! process a stage left behind if a check fails, and a parent-death signal
//! ends the stage (by then the adopted daemon) if the parent itself is
//! killed.

#![cfg(feature = "integration-test")]

use std::{
    env, fs,
    io::Write as _,
    os::{
        fd::{AsFd, OwnedFd},
        unix::{
            fs::{OpenOptionsExt, PermissionsExt},
            net::UnixListener,
        },
    },
    path::{Path, PathBuf},
    process::{Child, Command, ExitCode, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rustix::{
    fs::{MemfdFlags, memfd_create},
    process::{Pid, Signal, geteuid, kill_process},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use splinterd::{
    HandoffCheckpointBudget, LiveError, LiveSplintConfig, LiveSplintHandle, LiveSplintRuntime,
    encode_manifest_pair_for_integration_test,
    executable_snapshot::{
        ExecutableSnapshotPolicy, ExecutableSourcePair, HandoffExecutableSnapshots,
        RetainedRollbackExecutables, SealedExecutableSnapshot,
    },
    handoff_compatibility::{BuildIdentity, BuildVersion},
    handoff_descriptors::{HandoffDescriptor, HandoffDescriptorSlot, inheritable_descriptors},
    handoff_exec::{
        ExecBoundaryStage, ExecInheritancePlan, ManifestSeal, candidate_daemon_arguments,
        cross_exec_boundary_hooked_for_integration_test, exec_environment,
    },
    handoff_preflight::compiled_handoff_generation,
    handoff_prepare::{CaptureLimits, HandedOffCapture, capture_live_splints},
    integration_test_seams::{
        BUILD_SALT_BYTES, CANDIDATE_FAULT_ENV, CandidateFaultPoint, PACKAGED_OWNER_UID_ENV,
        salt_slot_offset, salted_pair_build_identity,
    },
};
use splinterm_core::{
    Dojo, DojoId, Lair, LairId, LairLifetime, LairRetention, LayoutNode, Splint, SplintId,
    SplintLaunchMetadata, SplintState, Topology, TopologyDocument,
};
use splinterm_protocol::{
    ClientFrame, ClientRole, ControlMode, LaunchParameters, MAX_FRAME_BYTES,
    MAX_IMAGE_BYTES_PER_DAEMON, PROTOCOL_VERSION, ProtocolError, Request, Response, ServerFrame,
    TerminalSnapshot, encode_frame, image_content_socket_path,
};
use splinterm_pty::{LinuxPtyBackend, PtyCommand};
use splinterm_terminal::{
    DEFAULT_KITTY_UPLOAD_BYTES_PER_DAEMON, SharedImageBudget, SharedKittyUploadBudget,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    time,
};

const STAGE: &str = "SPLINTERD_EXEC_ADOPTION_STAGE";
const DIRECTORY: &str = "SPLINTERD_EXEC_ADOPTION_DIRECTORY";
/// The stage removes the forward client after building the manifests.
const REMOVE_FORWARD_CLIENT: &str = "SPLINTERD_EXEC_ADOPTION_REMOVE_FORWARD_CLIENT";
/// The stage leaves a SIGTERM pending (blocked) across its exec.
const PENDING_SIGTERM: &str = "SPLINTERD_EXEC_ADOPTION_PENDING_SIGTERM";
/// The stage records a wrong SHA-256 for the rollback daemon snapshot in
/// both manifests, so staging and the rollback executor both refuse it.
const TAMPER_ROLLBACK_IMAGE: &str = "SPLINTERD_EXEC_ADOPTION_TAMPER_ROLLBACK_IMAGE";
/// Every pre-commit step the fault matrix fails the forward candidate at.
const FAULT_POINTS: [CandidateFaultPoint; 6] = [
    CandidateFaultPoint::Staging,
    CandidateFaultPoint::RuntimeEntry,
    CandidateFaultPoint::Startup,
    CandidateFaultPoint::ListenerRegistration,
    CandidateFaultPoint::ScopeAdoption,
    CandidateFaultPoint::ActorStaging,
];
const CASES: [&str; 7] = [
    "adopt",
    "rollback",
    "rollback-of-rollback",
    "pending-sigterm",
    "tampered-rollback-image",
    "stray-descriptor",
    "fault-matrix",
];
const DAEMON: &str = env!("CARGO_BIN_EXE_splinterd");
/// The salt the forward image carries: any nonzero value makes its pair
/// identity differ from the rollback image's.
const SALT: [u8; BUILD_SALT_BYTES] = *b"m3.10b-exec-salt";
const ROLLBACK_CLIENT: &[u8] = b"#!/bin/sh\n# rollback client fixture\nexit 0\n";
const FORWARD_CLIENT: &[u8] = b"#!/bin/sh\n# forward client fixture\nexit 0\n";
const CAT: &str = "printf ready; exec cat";
const EXCLUDED_EXIT: i32 = 3;
const GUARD_WAIT: Duration = Duration::from_secs(2);
const WAIT: Duration = Duration::from_secs(10);
const STAGE_WAIT: Duration = Duration::from_secs(60);
const RECORD: &str = "stage-record.json";
const STAGED_TOPOLOGY: &str = "topology.staged.json";
const STDOUT: &str = "stage.stdout";
const STDERR: &str = "stage.stderr";
/// Names this test process's PID for the launcher's parent-death check.
const PARENT: &str = "SPLINTERD_EXEC_ADOPTION_PARENT";
/// Runs the stage with `close_fds`, so it inherits no descriptor, and ties
/// both lifetimes to their parents with `PR_SET_PDEATHSIG` = `SIGKILL`.
/// Drop guards cannot run when this test binary is itself killed or timed
/// out, and an adopted daemon never exits on its own: without this it and
/// its Splints would be orphaned.
///
/// The launcher dies with this test process (the thread that spawned it is
/// `main`, which outlives every stage), and the stage with the launcher
/// (whose only thread forks it). The signal is set after fork in the stage
/// (`preexec_fn`) and survives `execve` of the test binary and the stage's
/// own `execveat` into the forward daemon, neither of which is set-user-ID
/// or file-capability bearing; `fork` clears it, so Splint shells do not
/// inherit it (they hang up when the daemon's PTY masters close). Each
/// side re-checks its parent after `prctl`, closing the race with a parent
/// that died first.
const LAUNCHER: &str = r#"
import ctypes, os, signal, subprocess, sys
libc = ctypes.CDLL(None, use_errno=True)
PR_SET_PDEATHSIG = 1
def die_with(parent):
    if libc.prctl(PR_SET_PDEATHSIG, ctypes.c_ulong(signal.SIGKILL), ctypes.c_ulong(0), ctypes.c_ulong(0), ctypes.c_ulong(0)) != 0:
        os._exit(126)
    if os.getppid() != parent:
        os._exit(126)
die_with(int(os.environ["SPLINTERD_EXEC_ADOPTION_PARENT"]))
launcher = os.getpid()
raise SystemExit(subprocess.run(sys.argv[1:], close_fds=True, preexec_fn=lambda: die_with(launcher)).returncode)
"#;

fn main() -> ExitCode {
    if env::args().skip(1).any(|argument| argument == "--list") {
        for case in CASES {
            println!("handoff_exec_adoption::{case}: test");
        }
        return ExitCode::SUCCESS;
    }
    match env::var(STAGE).ok().as_deref() {
        Some("old-generation") => old_generation_stage(&StageOptions {
            remove_forward_client: env::var_os(REMOVE_FORWARD_CLIENT).is_some(),
            pending_sigterm: env::var_os(PENDING_SIGTERM).is_some(),
            tamper_rollback_image: env::var_os(TAMPER_ROLLBACK_IMAGE).is_some(),
            fault: None,
        }),
        Some(stage) => panic!("unknown stage {stage:?}"),
        None => {
            // A harness may leak an inheritable descriptor into this process
            // (CI run 39137743159 did); each stage starts without one anyway,
            // and this deliberately inheritable one proves it on every run.
            let stray = fs::File::open("/dev/null").unwrap();
            let flags = rustix::io::fcntl_getfd(&stray).unwrap();
            rustix::io::fcntl_setfd(&stray, flags - rustix::io::FdFlags::CLOEXEC).unwrap();
            adopt_case();
            println!("handoff_exec_adoption: adopt passed");
            rollback_case();
            println!("handoff_exec_adoption: rollback passed");
            rollback_of_rollback_case();
            println!("handoff_exec_adoption: rollback-of-rollback passed");
            pending_sigterm_case(false);
            println!("handoff_exec_adoption: pending-sigterm (adopt) passed");
            pending_sigterm_case(true);
            println!("handoff_exec_adoption: pending-sigterm (rollback) passed");
            tampered_rollback_image_case();
            println!("handoff_exec_adoption: tampered-rollback-image passed");
            stray_descriptor_case();
            println!("handoff_exec_adoption: stray-descriptor passed");
            for point in FAULT_POINTS {
                fault_case(point);
                println!(
                    "handoff_exec_adoption: fault-matrix {} passed",
                    point.name()
                );
            }
            drop(stray);
            println!("handoff_exec_adoption: ok");
        }
    }
    ExitCode::SUCCESS
}

/// How one stage's old generation behaves, and what fails its forward
/// candidate.
#[derive(Clone, Debug, Default)]
struct StageOptions {
    /// Remove the forward client after building the manifests, so forward
    /// adoption refuses the prefix (rollback adoption only reports it).
    remove_forward_client: bool,
    /// Raise SIGTERM from inside the exec boundary, once it has blocked the
    /// handoff signals, so it is pending across exec.
    pending_sigterm: bool,
    /// Record a wrong digest for the rollback daemon snapshot.
    tamper_rollback_image: bool,
    /// Candidate fault points, as `CANDIDATE_FAULT_ENV` spells them; the
    /// parent sets the variable, and it crosses both execs.
    fault: Option<String>,
}

// ---------------------------------------------------------------------------
// The old generation (stage process)
// ---------------------------------------------------------------------------

/// What the stage tells the parent, written just before the exec boundary.
#[derive(Debug, Deserialize, Serialize)]
struct StageRecord {
    pid: u32,
    source_daemon_generation: u64,
    topology_revision: u64,
    lair_id: LairId,
    carried: Vec<RecordedSplint>,
    excluded: RecordedSplint,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct RecordedSplint {
    splint_id: SplintId,
    dojo_id: DojoId,
    incarnation: u64,
    child_pid: u32,
}

/// The old generation's Splints after its runtime stopped.
struct Stopped {
    handed_off: Vec<HandedOffCapture>,
    carried: Vec<RecordedSplint>,
    excluded: RecordedSplint,
}

fn assert_single_threaded() {
    assert_eq!(
        fs::read_dir("/proc/self/task").unwrap().count(),
        1,
        "stage must start with only its main thread"
    );
    assert_eq!(
        inheritable_descriptors(16).unwrap(),
        [],
        "stage must start without inherited descriptors"
    );
}

/// Runs the old generation and crosses the exec boundary; returns only by
/// panicking. See [`StageOptions`].
#[allow(
    clippy::too_many_lines,
    reason = "one old generation, from install through the exec call, in production order"
)]
fn old_generation_stage(options: &StageOptions) -> ! {
    assert_single_threaded();
    let directory = PathBuf::from(env::var_os(DIRECTORY).expect("stage directory"));
    let owner = geteuid().as_raw();
    assert_eq!(
        env::var(PACKAGED_OWNER_UID_ENV).ok(),
        Some(owner.to_string()),
        "manifest validation on both sides accepts this uid as the package owner"
    );
    let policy = ExecutableSnapshotPolicy {
        expected_owner_uid: owner,
    };
    let prefix = directory.join("prefix");
    fs::create_dir(&prefix).unwrap();
    let daemon_path = prefix.join("splinterd");
    let client_path = prefix.join("splinterm");
    let pair = ExecutableSourcePair::new(&daemon_path, &client_path).unwrap();

    // The running generation's installed pair, retained before the upgrade.
    let daemon_bytes = fs::read(DAEMON).unwrap();
    install(&daemon_path, &daemon_bytes);
    install(&client_path, ROLLBACK_CLIENT);
    // Launches in the prefix use its PTY helper, as a packaged daemon's do.
    install(
        &prefix.join("splinterm-pty-child"),
        &fs::read(pty_helper()).unwrap(),
    );
    let rollback = RetainedRollbackExecutables::capture_declared_for_test(&pair, policy).unwrap();

    // The package upgrade: a salted image of the same build, whose pair
    // identity therefore differs, and a new client.
    let mut forward_daemon = daemon_bytes;
    let slot = salt_slot_offset(&forward_daemon)
        .expect("the integration-test daemon carries exactly one patchable salt slot");
    forward_daemon[slot..slot + BUILD_SALT_BYTES].copy_from_slice(&SALT);
    install(&daemon_path, &forward_daemon);
    install(&client_path, FORWARD_CLIENT);
    let snapshots = HandoffExecutableSnapshots::materialize(&pair, &rollback, policy).unwrap();
    drop(rollback);

    let socket = PathBuf::from(env::var_os("SPLINTERM_SOCKET").expect("stage socket"));
    let control = bound_listener(&socket);
    let image = bound_listener(&image_content_socket_path(&socket));

    let stopped = hand_off_and_stop();
    // The excluded child is still an unreaped zombie of this process: only
    // the forward daemon can reap it and record its real exit.
    assert_eq!(
        proc_stat(stopped.excluded.child_pid).map(|(state, _)| state),
        Some('Z'),
        "the excluded child must cross exec unreaped"
    );

    let lair_id = LairId::new();
    let topology = durable_topology(lair_id, &stopped);
    let revision = topology.revision().get();
    let encoded = TopologyDocument::from_topology(&topology)
        .unwrap()
        .encode()
        .unwrap();
    write_state(&directory, &encoded);
    fs::write(directory.join(STAGED_TOPOLOGY), &encoded).unwrap();

    let storage = [
        manifest_storage("exec-adoption-forward-manifest"),
        manifest_storage("exec-adoption-rollback-manifest"),
    ];
    let mut owners = vec![
        HandoffDescriptor::new(HandoffDescriptorSlot::Listener, control.as_fd()),
        HandoffDescriptor::new(HandoffDescriptorSlot::ImageListener, image.as_fd()),
        HandoffDescriptor::new(
            HandoffDescriptorSlot::CheckpointManifest,
            storage[0].as_fd(),
        ),
        HandoffDescriptor::new(HandoffDescriptorSlot::RollbackManifest, storage[1].as_fd()),
        HandoffDescriptor::new(
            HandoffDescriptorSlot::ForwardClient,
            snapshots.forward().client().descriptor(),
        ),
        HandoffDescriptor::new(
            HandoffDescriptorSlot::RollbackDaemon,
            snapshots.rollback().daemon().descriptor(),
        ),
        HandoffDescriptor::new(
            HandoffDescriptorSlot::RollbackClient,
            snapshots.rollback().client().descriptor(),
        ),
    ];
    for capture in &stopped.handed_off {
        owners.push(HandoffDescriptor::new(
            HandoffDescriptorSlot::PtyMaster(capture.slot()),
            capture.handed_off.session.master(),
        ));
        owners.push(capture.checkpoint.handoff_descriptor());
    }
    let (forward_bytes, rollback_bytes) = manifest_pair(
        &prefix,
        &snapshots,
        &owners,
        &stopped,
        lair_id,
        revision,
        options.tamper_rollback_image,
    );
    let recorded = owners
        .iter()
        .map(|owner| (owner.slot(), owner.raw_fd()))
        .collect::<Vec<_>>();
    let plan = ExecInheritancePlan::new(
        &recorded,
        &owners,
        [
            ManifestSeal::new(storage[0].as_fd(), &forward_bytes),
            ManifestSeal::new(storage[1].as_fd(), &rollback_bytes),
        ],
    )
    .unwrap();
    let forward_manifest = plan.forward_manifest_descriptor().unwrap();
    // Exactly what the binary's `arm_forward_exec` passes: argv rebuilt from
    // state (no workload cgroups required here), the environment unchanged.
    let arguments =
        candidate_daemon_arguments(daemon_path.as_os_str(), forward_manifest, false).unwrap();
    let plan = plan.with_forward_daemon(
        snapshots.forward().daemon().descriptor(),
        arguments,
        exec_environment(),
    );

    if options.remove_forward_client {
        fs::remove_file(&client_path).unwrap();
    }
    let record = StageRecord {
        pid: std::process::id(),
        source_daemon_generation: 1,
        topology_revision: revision,
        lair_id,
        carried: stopped.carried.clone(),
        excluded: stopped.excluded,
    };
    write_atomically(
        &directory.join(RECORD),
        &serde_json::to_vec(&record).unwrap(),
    );

    // As if `systemctl stop` arrived in the exec window: raised from inside
    // the boundary, after it blocked the handoff signals and before exec,
    // with nothing blocked here first. Without the boundary's own block the
    // signal's default action would end this stage at once.
    let mut raised = false;
    let reason = cross_exec_boundary_hooked_for_integration_test(
        &plan,
        Instant::now() + GUARD_WAIT,
        &mut |stage| {
            if options.pending_sigterm && stage == ExecBoundaryStage::Guard && !raised {
                raised = true;
                nix::sys::signal::raise(nix::sys::signal::Signal::SIGTERM).unwrap();
            }
            Ok(())
        },
    );
    panic!("the exec boundary returned instead of executing the forward daemon: {reason}");
}

/// Installs `bytes` at `path` the way a package upgrade does: a new inode
/// renamed over the old one.
fn install(path: &Path, bytes: &[u8]) {
    let staging = path.with_extension("staging");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o755)
        .open(&staging)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
    drop(file);
    fs::set_permissions(&staging, fs::Permissions::from_mode(0o755)).unwrap();
    fs::rename(&staging, path).unwrap();
}

fn bound_listener(path: &Path) -> UnixListener {
    let listener = UnixListener::bind(path).unwrap();
    listener.set_nonblocking(true).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    listener
}

/// One empty, close-on-exec, sealable memfd, as manifest building creates.
fn manifest_storage(name: &str) -> OwnedFd {
    memfd_create(name, MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING).unwrap()
}

fn daemon_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

/// The `splinterm-pty-child` this test was built with (build it first with
/// `cargo build -p splinterm-pty --bin splinterm-pty-child`).
fn pty_helper() -> PathBuf {
    let test_binary = env::current_exe().unwrap();
    let helper = test_binary
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("splinterm-pty-child");
    assert!(helper.is_file(), "missing PTY helper: {}", helper.display());
    helper
}

fn backend() -> LinuxPtyBackend {
    LinuxPtyBackend::new(pty_helper())
}

fn config() -> LiveSplintConfig {
    let mut config = LiveSplintConfig {
        columns: 60,
        rows: 24,
        hangup_grace: Duration::from_millis(30),
        terminate_grace: Duration::from_millis(30),
        poll_interval: Duration::from_millis(5),
        exit_drain_timeout: Duration::from_millis(50),
        ..LiveSplintConfig::default()
    };
    // The daemon's own budget limits: a checkpoint restores only against the
    // budget authority it was taken under.
    config.terminal.shared_image_budget = Some(SharedImageBudget::new(MAX_IMAGE_BYTES_PER_DAEMON));
    config.terminal.shared_kitty_upload_budget = Some(SharedKittyUploadBudget::new(
        DEFAULT_KITTY_UPLOAD_BYTES_PER_DAEMON,
    ));
    config
}

async fn screen_text(handle: &LiveSplintHandle) -> String {
    handle
        .snapshot()
        .await
        .unwrap()
        .visible_rows
        .iter()
        .flat_map(|row| row.cells.iter())
        .filter(|cell| cell.spacer_remaining.is_none())
        .map(|cell| cell.content.as_str())
        .collect()
}

/// Spawns two echoing Splints and one whose shell exits with
/// [`EXCLUDED_EXIT`] that its actor never reaps; captures every live one,
/// ends it for handoff, and stops the runtime.
fn hand_off_and_stop() -> Stopped {
    let runtime = daemon_runtime();
    let stopped = runtime.block_on(async {
        let mut live = Vec::new();
        for _ in 0..2 {
            let command = PtyCommand::new("/bin/sh", PathBuf::from("/tmp")).args(["-c", CAT]);
            let runtime = LiveSplintRuntime::spawn(SplintId::new(), backend(), command, config())
                .await
                .unwrap();
            let handle = runtime.handle();
            time::timeout(WAIT, async {
                while !screen_text(&handle).await.contains("ready") {
                    time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("a Splint never became ready");
            live.push(runtime);
        }
        // Polled once an hour, and its shell outlives the first poll, so its
        // actor never reaps the exited child.
        let mut lazy = config();
        lazy.poll_interval = Duration::from_secs(3600);
        let command = PtyCommand::new("/bin/sh", PathBuf::from("/tmp"))
            .args(["-c", &format!("sleep 0.3; exit {EXCLUDED_EXIT}")]);
        let excluded = LiveSplintRuntime::spawn(SplintId::new(), backend(), command, lazy)
            .await
            .unwrap();
        let excluded_handle = excluded.handle();
        time::timeout(WAIT, async {
            while excluded_handle.root_process_is_live() {
                time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the excluded Splint's shell never exited");

        let mut handles = live
            .iter()
            .map(LiveSplintRuntime::handle)
            .collect::<Vec<_>>();
        handles.push(excluded_handle.clone());
        let captures = capture_live_splints(
            handles,
            CaptureLimits::DEFAULT,
            |handle: LiveSplintHandle, budget: HandoffCheckpointBudget| async move {
                handle.prepare_pty_handoff_within(&budget).await
            },
        )
        .await
        .unwrap();
        assert_eq!(captures.captured().len(), 2);
        assert_eq!(captures.excluded().len(), 1);
        let (handed_off, _) = captures.hand_off();
        let mut carried = Vec::new();
        for runtime in live {
            let handle = runtime.handle();
            carried.push(RecordedSplint {
                splint_id: handle.splint_id,
                dojo_id: DojoId::new(),
                incarnation: handle.incarnation.value(),
                child_pid: handle.child_pid(),
            });
            // Each actor leaves through its handoff path.
            assert!(matches!(runtime.wait().await, Err(LiveError::HandedOff)));
        }
        let excluded_record = RecordedSplint {
            splint_id: excluded_handle.splint_id,
            dojo_id: DojoId::new(),
            incarnation: excluded_handle.incarnation.value(),
            child_pid: excluded_handle.child_pid(),
        };
        // Still running; it ends with the runtime, without reaping.
        drop(excluded);
        Stopped {
            handed_off,
            carried,
            excluded: excluded_record,
        }
    });
    runtime.shutdown_timeout(Duration::from_secs(2));
    stopped
}

/// The Topology the old generation persisted at its fence: one Lair, each
/// Splint alone in its own Dojo at its live incarnation.
fn durable_topology(lair_id: LairId, stopped: &Stopped) -> Topology {
    let dojos = stopped
        .carried
        .iter()
        .chain([&stopped.excluded])
        .enumerate()
        .map(|(index, splint)| Dojo {
            id: splint.dojo_id,
            name: format!("Dojo {}", index + 1),
            default_focus: splint.splint_id,
            root: LayoutNode::Leaf(Splint {
                id: splint.splint_id,
                title: "shell".into(),
                cwd: PathBuf::from("/tmp"),
                command: Vec::new(),
                launch: Box::new(SplintLaunchMetadata::default()),
                last_incarnation: Some(splint.incarnation),
                state: SplintState::Running,
            }),
        })
        .collect();
    let mut topology = Topology::new();
    topology
        .insert_lair_at(
            topology.revision(),
            Lair {
                id: lair_id,
                name: "handoff-exec".into(),
                lifetime: LairLifetime::Persistent,
                retention: LairRetention::Disposable,
                provenance: None,
                dojos,
            },
        )
        .unwrap();
    topology
}

/// Writes the durable Topology where the forward daemon's metadata store
/// reads it: `$XDG_STATE_HOME/splinterm/topology.json`, owner-only.
fn write_state(directory: &Path, encoded: &[u8]) {
    let state = directory.join("state/splinterm");
    fs::create_dir_all(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(state.join("topology.json"))
        .unwrap();
    file.write_all(encoded).unwrap();
    file.sync_all().unwrap();
}

fn write_atomically(path: &Path, bytes: &[u8]) {
    let staging = path.with_extension("staging");
    fs::write(&staging, bytes).unwrap();
    fs::rename(&staging, path).unwrap();
}

fn generation(identity: BuildIdentity, version: BuildVersion) -> Value {
    json!({
        "build_identity": identity.as_bytes(),
        "build_version": {
            "major": version.major,
            "minor": version.minor,
            "patch": version.patch,
        },
    })
}

fn executable_image(snapshot: &SealedExecutableSnapshot) -> Value {
    let source = snapshot.source();
    let sealed = snapshot.snapshot();
    json!({
        "source_device": source.device,
        "source_inode": source.inode,
        "source_owner_uid": source.owner_uid,
        "source_mode": source.mode,
        "source_size": source.size,
        "source_sha256": source.sha256,
        "snapshot_device": sealed.device,
        "snapshot_inode": sealed.inode,
        "snapshot_size": sealed.size,
        "snapshot_sha256": sealed.sha256,
        "snapshot_seals": sealed.seals.bits(),
    })
}

/// The forward and rollback manifests for this attempt, as the old
/// generation's manifest builder writes them from its captures: the
/// rollback pair is this build unsalted, the forward pair the salted image.
#[allow(
    clippy::too_many_lines,
    reason = "one fixture binds every carried role into one manifest pair"
)]
fn manifest_pair(
    prefix: &Path,
    snapshots: &HandoffExecutableSnapshots,
    owners: &[HandoffDescriptor<'_>],
    stopped: &Stopped,
    lair_id: LairId,
    revision: u64,
    tamper_rollback_image: bool,
) -> (Vec<u8>, Vec<u8>) {
    let compiled = compiled_handoff_generation().unwrap();
    let source = generation(compiled.build_identity, compiled.build_version);
    let target = generation(
        salted_pair_build_identity(compiled.build_identity, SALT),
        compiled.build_version,
    );
    let capabilities = compiled.capabilities;
    let splints = stopped
        .handed_off
        .iter()
        .map(|capture| {
            let evidence = capture.handed_off.evidence;
            let recorded = stopped
                .carried
                .iter()
                .find(|splint| splint.splint_id == evidence.splint_id)
                .unwrap();
            json!({
                "lair_id": lair_id,
                "dojo_id": recorded.dojo_id,
                "splint_id": evidence.splint_id,
                "incarnation": evidence.incarnation.value(),
                "pty": {
                    "child_pid": evidence.pty_identity.child_pid(),
                    "process_group": evidence.pty_identity.process_group(),
                    "session_id": evidence.pty_identity.session_id(),
                    "master_slot": capture.slot(),
                },
                "size": {
                    "columns": evidence.size.columns,
                    "rows": evidence.size.rows,
                    "pixel_width": evidence.size.pixel_width,
                    "pixel_height": evidence.size.pixel_height,
                },
                "foreground_process_group": evidence.foreground_process_group,
                "terminal_revision": evidence.terminal_revision.value(),
                "history_generation": evidence.history_generation,
                "exit": "running",
                "terminal_checkpoint": capture.checkpoint.terminal_disposition(),
                "image_state": capture.checkpoint.image_disposition(),
                "controller": "released",
                "subscriptions": "dropped",
            })
        })
        .collect::<Vec<_>>();
    let excluded = stopped.excluded;
    let descriptors = owners
        .iter()
        .map(|owner| {
            json!({
                "slot": owner.slot(),
                "inherited_fd": u32::try_from(owner.raw_fd()).unwrap(),
            })
        })
        .collect::<Vec<_>>();
    let handoff_id = [0x6d_u8; 16];
    let mut forward = json!({
        "schema": 1,
        "kind": "forward",
        "handoff_id": handoff_id,
        "source_generation": source.clone(),
        "target_generation": target.clone(),
        "source_daemon_generation": 1,
        "target_daemon_generation": 2,
        "topology_revision": revision,
        "durable_state_generation": revision,
        "private_protocol": capabilities.private_protocol.maximum(),
        "handoff_protocol": capabilities.handoff_protocol.maximum(),
        "terminal_checkpoint_schema": capabilities.terminal_checkpoint.maximum(),
        "descriptor_manifest_schema": capabilities.descriptor_manifest.maximum(),
        "install_prefix": prefix,
        "executable_pairs": {
            "forward": {
                "daemon": executable_image(snapshots.forward().daemon()),
                "client": executable_image(snapshots.forward().client()),
                "generation": target.clone(),
            },
            "rollback": {
                "daemon": executable_image(snapshots.rollback().daemon()),
                "client": executable_image(snapshots.rollback().client()),
                "generation": source.clone(),
            },
        },
        "counterpart_manifest_slot": HandoffDescriptorSlot::RollbackManifest,
        "splints": splints,
        // No exit recorded: the forward daemon must reap the zombie itself.
        "excluded_splints": [{
            "lair_id": lair_id,
            "dojo_id": excluded.dojo_id,
            "splint_id": excluded.splint_id,
            "incarnation": excluded.incarnation,
            "child_pid": excluded.child_pid,
            "exit": null,
        }],
        "descriptors": descriptors,
    });
    if tamper_rollback_image {
        // Both manifests stay a valid pair (validation requires the source
        // and snapshot digests to agree); only the digest is wrong.
        let image = &mut forward["executable_pairs"]["rollback"]["daemon"];
        let mut digest: Vec<u8> = serde_json::from_value(image["snapshot_sha256"].clone()).unwrap();
        digest[0] ^= 0xff;
        image["snapshot_sha256"] = json!(digest);
        image["source_sha256"] = json!(digest);
    }
    let mut rollback = forward.clone();
    rollback["kind"] = json!("rollback");
    rollback["source_generation"] = target;
    rollback["target_generation"] = source;
    rollback["source_daemon_generation"] = json!(2);
    rollback["target_daemon_generation"] = json!(1);
    rollback["counterpart_manifest_slot"] = json!(HandoffDescriptorSlot::CheckpointManifest);
    encode_manifest_pair_for_integration_test(forward, rollback).unwrap()
}

// ---------------------------------------------------------------------------
// The parent: launch, observe, and clean up
// ---------------------------------------------------------------------------

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

/// A private directory, short enough for the socket paths under it.
struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            % 1_000_000_000;
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path =
            env::temp_dir().join(format!("sd-hx-{}-{unique}-{sequence}", std::process::id()));
        fs::create_dir(&path).unwrap();
        // The manifest's install prefix must be canonical.
        Self(fs::canonicalize(path).unwrap())
    }

    fn socket(&self) -> PathBuf {
        self.0.join("s.sock")
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// One launched stage: the `close_fds` launcher and, under it, the stage
/// process that becomes the forward daemon. Dropping it kills every process
/// the stage left: its children first, while they cannot have been reaped,
/// then the stage itself.
struct Stage {
    launcher: Child,
    directory: PathBuf,
}

impl Stage {
    fn launch(directory: &TestDirectory, options: StageOptions) -> Self {
        let stdout = fs::File::create(directory.0.join(STDOUT)).unwrap();
        let stderr = fs::File::create(directory.0.join(STDERR)).unwrap();
        let mut command = Command::new("python3");
        command.env_remove(REMOVE_FORWARD_CLIENT);
        command.env_remove(PENDING_SIGTERM);
        command.env_remove(TAMPER_ROLLBACK_IMAGE);
        command.env_remove(CANDIDATE_FAULT_ENV);
        if options.remove_forward_client {
            command.env(REMOVE_FORWARD_CLIENT, "1");
        }
        if options.pending_sigterm {
            command.env(PENDING_SIGTERM, "1");
        }
        if options.tamper_rollback_image {
            command.env(TAMPER_ROLLBACK_IMAGE, "1");
        }
        if let Some(fault) = options.fault {
            command.env(CANDIDATE_FAULT_ENV, fault);
        }
        let launcher = command
            .arg("-c")
            .arg(LAUNCHER)
            .arg(env::current_exe().unwrap())
            .env(PARENT, std::process::id().to_string())
            .env(STAGE, "old-generation")
            .env(DIRECTORY, &directory.0)
            .env("SPLINTERM_SOCKET", directory.socket())
            .env("XDG_STATE_HOME", directory.0.join("state"))
            .env("XDG_CONFIG_HOME", directory.0.join("config"))
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                "unix:path=/nonexistent/splinterd-exec-adoption-bus",
            )
            .env(PACKAGED_OWNER_UID_ENV, geteuid().as_raw().to_string())
            .env("SPLINTERM_ENABLE_DEV_ATTACH", "1")
            .env("SPLINTERM_TEST_SHUTDOWN_GRACE_MS", "200")
            .env("RUST_LOG", "info")
            .env("NO_COLOR", "1")
            .env_remove("SPLINTERM_POLICY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .current_dir(&directory.0)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .expect("launch a stage without inherited descriptors");
        Self {
            launcher,
            directory: directory.0.clone(),
        }
    }

    fn logs(&self) -> String {
        let read = |name| {
            strip_ansi(&String::from_utf8_lossy(
                &fs::read(self.directory.join(name)).unwrap_or_default(),
            ))
        };
        format!("stdout:\n{}\nstderr:\n{}", read(STDOUT), read(STDERR))
    }

    /// # Panics
    /// When the launcher's status cannot be read; `Drop` never calls this.
    fn exited(&mut self) -> Option<std::process::ExitStatus> {
        self.launcher.try_wait().unwrap()
    }

    /// Waits for the record the stage writes just before exec.
    fn record(&mut self) -> StageRecord {
        let path = self.directory.join(RECORD);
        let deadline = Instant::now() + STAGE_WAIT;
        loop {
            if let Ok(bytes) = fs::read(&path) {
                return serde_json::from_slice(&bytes).unwrap();
            }
            if let Some(status) = self.exited() {
                panic!("stage exited with {status} before exec\n{}", self.logs());
            }
            assert!(
                Instant::now() < deadline,
                "stage never reached exec\n{}",
                self.logs()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Waits for the launcher, and so the stage, to exit.
    fn wait(&mut self, timeout: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.exited() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "stage did not exit\n{}",
                self.logs()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Stage {
    // Runs during unwinding too, so nothing here may panic.
    fn drop(&mut self) {
        if self.launcher.try_wait().ok().flatten().is_none() {
            for stage in children_of(self.launcher.id()) {
                for child in children_of(stage) {
                    signal(child, Signal::KILL);
                }
                signal(stage, Signal::KILL);
            }
            let _ = self.launcher.kill();
            let _ = self.launcher.wait();
        }
    }
}

fn signal(pid: u32, signal: Signal) {
    if let Some(pid) = i32::try_from(pid).ok().and_then(Pid::from_raw) {
        let _ = kill_process(pid, signal);
    }
}

/// A process's state letter and parent PID, or `None` once it is gone.
fn proc_stat(pid: u32) -> Option<(char, u32)> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let mut fields = text[text.rfind(')')? + 1..].split_whitespace();
    let state = fields.next()?.chars().next()?;
    let parent = fields.next()?.parse().ok()?;
    Some((state, parent))
}

fn children_of(parent: u32) -> Vec<u32> {
    fs::read_dir("/proc")
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u32>().ok())
                .filter(|pid| proc_stat(*pid).is_some_and(|(_, ppid)| ppid == parent))
                .collect()
        })
        .unwrap_or_default()
}

/// A live (not zombie) child of `parent`.
fn is_live_child(pid: u32, parent: u32) -> bool {
    proc_stat(pid).is_some_and(|(state, ppid)| ppid == parent && state != 'Z')
}

/// Waits until `pid` has exited (it may linger as someone else's zombie).
fn wait_until_exited(pid: u32, what: &str) {
    let deadline = Instant::now() + WAIT;
    while proc_stat(pid).is_some_and(|(state, _)| state != 'Z') {
        assert!(Instant::now() < deadline, "{what} {pid} is still running");
        thread::sleep(Duration::from_millis(20));
    }
}

fn strip_ansi(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character == '\u{1b}' {
            for next in characters.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(character);
        }
    }
    plain
}

/// The persisted Topology, read back as the daemon's restore path reads it.
fn persisted_topology(directory: &TestDirectory) -> Topology {
    let bytes = fs::read(directory.0.join("state/splinterm/topology.json")).unwrap();
    TopologyDocument::decode(&bytes)
        .unwrap()
        .into_topology()
        .unwrap()
}

/// A protocol client of the adopted daemon.
struct Client {
    stream: UnixStream,
    request_id: u64,
}

impl Client {
    /// Connects and says hello; returns the client and the served
    /// generation, or `None` when the daemon did not answer in time.
    async fn hello(socket: &Path) -> Option<(Self, u64)> {
        let mut stream = UnixStream::connect(socket).await.ok()?;
        write_frame(
            &mut stream,
            &ClientFrame::Hello {
                minimum_version: PROTOCOL_VERSION,
                maximum_version: PROTOCOL_VERSION,
                role: ClientRole::Automation,
            },
        )
        .await;
        match time::timeout(Duration::from_secs(2), read_frame(&mut stream))
            .await
            .ok()??
        {
            ServerFrame::Hello {
                version,
                development_terminal_access,
                daemon_generation,
                ..
            } => {
                assert_eq!(version, PROTOCOL_VERSION);
                assert!(development_terminal_access);
                Some((
                    Self {
                        stream,
                        request_id: 1,
                    },
                    daemon_generation,
                ))
            }
            frame => panic!("unexpected hello: {frame:?}"),
        }
    }

    async fn request_result(&mut self, request: Request) -> Result<Response, ProtocolError> {
        let request_id = self.request_id;
        self.request_id += 1;
        write_frame(
            &mut self.stream,
            &ClientFrame::Request {
                request_id,
                diagnostic_correlation: None,
                request,
            },
        )
        .await;
        loop {
            let frame = time::timeout(WAIT, read_frame(&mut self.stream))
                .await
                .expect("timed out waiting for a response")
                .expect("connection closed before a response");
            match frame {
                ServerFrame::Response {
                    request_id: response_id,
                    result,
                } if response_id == request_id => return Ok(result),
                ServerFrame::Error {
                    request_id: Some(response_id),
                    error,
                } if response_id == request_id => return Err(error),
                ServerFrame::Event { .. } => {}
                frame => panic!("unexpected frame: {frame:?}"),
            }
        }
    }

    async fn request(&mut self, request: Request) -> Response {
        self.request_result(request)
            .await
            .unwrap_or_else(|error| panic!("request failed: {:?}: {}", error.code, error.message))
    }

    async fn snapshot(&mut self, splint_id: SplintId, incarnation: u64) -> TerminalSnapshot {
        let Response::Attached {
            subscription_id,
            snapshot,
            ..
        } = self
            .request(Request::Attach {
                splint_id,
                incarnation: Some(incarnation),
                scrollback_rows: 16,
            })
            .await
        else {
            panic!("attach was not granted");
        };
        assert_eq!(
            self.request(Request::Detach { subscription_id }).await,
            Response::Acknowledged
        );
        snapshot
    }

    async fn wait_for_text(&mut self, splint_id: SplintId, incarnation: u64, text: &str) {
        self.wait_for_occurrences(splint_id, incarnation, text, 1)
            .await;
    }

    async fn wait_for_occurrences(
        &mut self,
        splint_id: SplintId,
        incarnation: u64,
        text: &str,
        occurrences: usize,
    ) {
        let deadline = Instant::now() + WAIT;
        loop {
            let screen = snapshot_text(&self.snapshot(splint_id, incarnation).await);
            if screen.matches(text).count() >= occurrences {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{splint_id} never showed {text:?} {occurrences} times: {screen:?}"
            );
            time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn echo(&mut self, splint_id: SplintId, incarnation: u64, token: &str) {
        let Response::ControlGranted { controller_id, .. } = self
            .request(Request::AcquireControl {
                splint_id,
                incarnation,
                modes: vec![ControlMode::Input],
            })
            .await
        else {
            panic!("control was not granted");
        };
        assert!(matches!(
            self.request(Request::Input {
                controller_id,
                splint_id,
                incarnation,
                bytes: format!("{token}\n").into_bytes(),
                focus_report: false,
            })
            .await,
            Response::TerminalActionAcknowledged { .. }
        ));
        // Twice: the line discipline echoes the input, and only a live shell
        // (`cat`) writes it back again.
        self.wait_for_occurrences(splint_id, incarnation, token, 2)
            .await;
        assert_eq!(
            self.request(Request::ReleaseControl { controller_id })
                .await,
            Response::Acknowledged
        );
    }
}

async fn write_frame(stream: &mut UnixStream, frame: &ClientFrame) {
    stream
        .write_all(&encode_frame(frame).unwrap())
        .await
        .unwrap();
}

async fn read_frame(stream: &mut UnixStream) -> Option<ServerFrame> {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length).await.ok()?;
    let length = u32::from_be_bytes(length) as usize;
    assert!((1..=MAX_FRAME_BYTES).contains(&length));
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body).await.ok()?;
    Some(serde_json::from_slice(&body).unwrap())
}

fn snapshot_text(snapshot: &TerminalSnapshot) -> String {
    snapshot
        .scrollback_rows
        .iter()
        .chain(&snapshot.visible_rows)
        .flat_map(|row| row.cells.iter())
        .map(|cell| cell.content.as_str())
        .collect()
}

/// Connects to the adopted daemon, retrying until it serves or the stage
/// exits. A connection queued while the old generation still held the
/// listener is answered by the forward daemon, which inherited it.
async fn connect_adopted(stage: &mut Stage, socket: &Path) -> (Client, u64) {
    let deadline = Instant::now() + STAGE_WAIT;
    loop {
        if let Some(connected) = Client::hello(socket).await {
            return connected;
        }
        if let Some(status) = stage.exited() {
            panic!(
                "the stage exited with {status} instead of serving\n{}",
                stage.logs()
            );
        }
        assert!(
            Instant::now() < deadline,
            "the forward daemon never served\n{}",
            stage.logs()
        );
        time::sleep(Duration::from_millis(50)).await;
    }
}

/// The full handoff: the stage execs the salted forward daemon, which
/// adopts, commits, and serves as generation 2 in the same process.
fn adopt_case() {
    let directory = TestDirectory::new();
    let mut stage = Stage::launch(&directory, StageOptions::default());
    let record = stage.record();
    let pid = record.pid;
    // The stage is the launcher's only child.
    assert_eq!(children_of(stage.launcher.id()), [pid]);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let launched = runtime.block_on(async {
        let (mut client, daemon_generation) =
            connect_adopted(&mut stage, &directory.socket()).await;
        // The next served generation.
        assert_eq!(daemon_generation, record.source_daemon_generation + 1);
        assert_same_process(&client, &stage, pid);

        // Every carried Splint: the same process, still this daemon's child,
        // at its original incarnation, its captured screen restored, and
        // echoing through the adopted daemon.
        for splint in &record.carried {
            assert!(
                is_live_child(splint.child_pid, pid),
                "carried Splint {} lost its process",
                splint.splint_id
            );
            let Response::Splint { runtime, .. } = client
                .request(Request::InspectSplint {
                    splint_id: splint.splint_id,
                })
                .await
            else {
                panic!("no Splint response");
            };
            assert_eq!(runtime.live_incarnation, Some(splint.incarnation));
            client
                .wait_for_text(splint.splint_id, splint.incarnation, "ready")
                .await;
            client
                .echo(splint.splint_id, splint.incarnation, "echo-after-exec")
                .await;
        }

        assert_exits_recorded(&mut client, &directory, &record).await;
        assert_listeners_accept(&directory, daemon_generation).await;
        assert_daemon_threads_unblocked(pid);
        assert_new_splint_unblocked(&mut client, pid, &record).await
    });

    // No workload unit manager on either side: nothing was contained, so no
    // scope could be stopped, and the adoption carried exactly what the
    // manifest named with no stray descriptor.
    let logs = stage.logs();
    for expected in [
        "daemon handoff candidate: adopting",
        "workload_units=false",
        "daemon handoff: adoption committed",
        "daemon_generation=2",
        "carried=2",
        "excluded=1",
        "excluded_exit_unknown=0",
        "closed_unexpected=0",
        "daemon handoff: adopted generation published",
    ] {
        assert!(logs.contains(expected), "missing {expected:?}\n{logs}");
    }

    // An ordered shutdown of the adopted daemon ends it and its Splints.
    shut_down(&mut stage, &record, &[launched]);
}

/// Bits of SIGHUP (1), SIGINT (2), and SIGTERM (15) in a `/proc` signal mask:
/// signal `n` is bit `n - 1`.
const HANDOFF_SIGNAL_BITS: u64 = 0x4003;

/// The `SigBlk` mask in a `/proc` status file, or `None` once it is gone.
fn blocked_signals(status: &Path) -> Option<u64> {
    let text = fs::read_to_string(status).ok()?;
    let mask = text.lines().find_map(|line| line.strip_prefix("SigBlk:"))?;
    u64::from_str_radix(mask.trim(), 16).ok()
}

/// No thread of the serving daemon holds SIGINT, SIGTERM, or SIGHUP
/// blocked: the mask the handoff exec carried was released before any
/// runtime thread existed, so none inherited it.
fn assert_daemon_threads_unblocked(pid: u32) {
    for task in fs::read_dir(format!("/proc/{pid}/task")).unwrap() {
        let status = task.unwrap().path().join("status");
        if let Some(mask) = blocked_signals(&status) {
            assert_eq!(
                mask & HANDOFF_SIGNAL_BITS,
                0,
                "{} blocks a handoff signal: SigBlk {mask:016x}",
                status.display()
            );
        }
    }
}

/// A Splint launched now, through the serving daemon, starts with none of
/// SIGINT, SIGTERM, or SIGHUP blocked. Returns its process.
async fn assert_new_splint_unblocked(client: &mut Client, pid: u32, record: &StageRecord) -> u32 {
    let Response::Topology { snapshot } = client.request(Request::InspectTopology).await else {
        panic!("no Topology response");
    };
    let created = client
        .request(Request::CreateLair {
            expected_topology_revision: snapshot.revision,
            name: "after-handoff".to_owned(),
            launch: LaunchParameters {
                cwd: PathBuf::from("/tmp"),
                command: vec![
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    "exec sleep 3600".to_owned(),
                ],
                shell: None,
                login_shell: false,
                scrollback_lines: 100,
            },
            generated_name: false,
        })
        .await;
    assert!(
        matches!(created, Response::LairCreated { .. }),
        "{created:?}"
    );
    // The daemon's one child that is not a carried Splint, once its shell
    // has become `sleep`.
    let deadline = Instant::now() + WAIT;
    let launched = loop {
        let launched = children_of(pid).into_iter().find(|child| {
            record
                .carried
                .iter()
                .all(|splint| splint.child_pid != *child)
                && fs::read_to_string(format!("/proc/{child}/comm"))
                    .is_ok_and(|comm| comm.trim() == "sleep")
        });
        if let Some(launched) = launched {
            break launched;
        }
        assert!(Instant::now() < deadline, "the new Splint never ran");
        time::sleep(Duration::from_millis(20)).await;
    };
    let mask = blocked_signals(Path::new(&format!("/proc/{launched}/status")))
        .expect("the new Splint's status");
    assert_eq!(
        mask & HANDOFF_SIGNAL_BITS,
        0,
        "a Splint launched after the handoff blocks a handoff signal: SigBlk {mask:016x}"
    );
    launched
}

/// The daemon serving `client` is the stage process itself, under the same
/// launcher, now executing the sealed forward snapshot with its activation
/// flag.
fn assert_same_process(client: &Client, stage: &Stage, pid: u32) {
    let peer = rustix::net::sockopt::socket_peercred(&client.stream).unwrap();
    assert_eq!(peer.pid.as_raw_nonzero().get(), i32::try_from(pid).unwrap());
    let executable = fs::read_link(format!("/proc/{pid}/exe")).unwrap();
    assert!(
        executable.to_string_lossy().starts_with("/memfd:"),
        "the adopted daemon runs from its sealed snapshot: {}",
        executable.display()
    );
    let command_line = fs::read(format!("/proc/{pid}/cmdline")).unwrap();
    assert!(
        String::from_utf8_lossy(&command_line).contains(splinterd::CANDIDATE_ACTIVATION_ARGUMENT),
        "{}",
        String::from_utf8_lossy(&command_line)
    );
    assert_eq!(
        proc_stat(pid).map(|(_, ppid)| ppid),
        Some(stage.launcher.id())
    );
}

/// The excluded Splint's zombie was reaped by the forward daemon, and its
/// real exit is recorded live and persisted; every Splint keeps its
/// incarnation in the durable Topology.
async fn assert_exits_recorded(
    client: &mut Client,
    directory: &TestDirectory,
    record: &StageRecord,
) {
    let excluded = record.excluded;
    assert!(
        proc_stat(excluded.child_pid).is_none_or(|(_, ppid)| ppid != record.pid),
        "the excluded Splint's zombie was not reaped"
    );
    let Response::Topology { snapshot } = client.request(Request::InspectTopology).await else {
        panic!("no Topology response");
    };
    let live = |splint_id| snapshot.topology.find_splint(splint_id).unwrap().state;
    assert_eq!(live(excluded.splint_id), SplintState::Exited(EXCLUDED_EXIT));
    for splint in &record.carried {
        assert_eq!(live(splint.splint_id), SplintState::Running);
    }
    let persisted = persisted_topology(directory);
    assert_eq!(persisted.revision().get(), record.topology_revision);
    assert_eq!(
        persisted.find_splint(excluded.splint_id).unwrap().state,
        SplintState::Exited(EXCLUDED_EXIT)
    );
    for splint in record.carried.iter().chain([&excluded]) {
        assert_eq!(
            persisted
                .find_splint(splint.splint_id)
                .unwrap()
                .last_incarnation,
            Some(splint.incarnation)
        );
    }
}

/// Both inherited listeners accept. The image listener closes a connection
/// whose token it cannot claim; one it never accepted would stay open in the
/// backlog instead.
async fn assert_listeners_accept(directory: &TestDirectory, daemon_generation: u64) {
    let mut image = UnixStream::connect(image_content_socket_path(&directory.socket()))
        .await
        .unwrap();
    image.write_all(&[0; 32]).await.unwrap();
    let mut rest = Vec::new();
    let closed = time::timeout(WAIT, image.read_to_end(&mut rest)).await;
    assert!(
        matches!(closed, Ok(Ok(0) | Err(_))),
        "the image listener never accepted: {closed:?}"
    );
    let (_second, generation) = Client::hello(&directory.socket())
        .await
        .expect("the control listener accepts a new connection");
    assert_eq!(generation, daemon_generation);
}

/// The one log line containing `marker`.
fn log_line<'a>(logs: &'a str, marker: &str) -> &'a str {
    let mut lines = logs.lines().filter(|line| line.contains(marker));
    let line = lines
        .next()
        .unwrap_or_else(|| panic!("no {marker:?} line\n{logs}"));
    assert!(
        lines.next().is_none(),
        "more than one {marker:?} line\n{logs}"
    );
    line
}

/// Every `closed_unexpected=` the logs report is zero: neither exec leaked
/// an unmanifested descriptor into the next image.
fn assert_no_stray_descriptors(logs: &str) {
    const FIELD: &str = "closed_unexpected=";
    for (index, _) in logs.match_indices(FIELD) {
        let value = logs[index + FIELD.len()..]
            .split(|character: char| !character.is_ascii_digit())
            .next()
            .unwrap_or_default();
        assert_eq!(value, "0", "a stray descriptor crossed exec\n{logs}");
    }
}

/// The checks every rollback makes: the same process, now executing the
/// sealed rollback daemon snapshot, serves the original generation 1; every
/// carried Splint kept its process (still this daemon's child) and its
/// incarnation, shows its captured screen, and echoes; the excluded
/// Splint's zombie was reaped and its exit recorded; both listeners accept;
/// exactly one rollback exec happened and only the rollback generation
/// committed.
fn assert_rolled_back(
    stage: &mut Stage,
    directory: &TestDirectory,
    record: &StageRecord,
    install_pair_mismatch: bool,
    stray_descriptors: bool,
) -> u32 {
    let pid = record.pid;
    assert_eq!(children_of(stage.launcher.id()), [pid]);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let launched = runtime.block_on(async {
        let (mut client, daemon_generation) = connect_adopted(stage, &directory.socket()).await;
        assert_eq!(
            daemon_generation, record.source_daemon_generation,
            "rollback serves the original generation"
        );
        assert_same_process(&client, stage, pid);
        for splint in &record.carried {
            assert!(
                is_live_child(splint.child_pid, pid),
                "carried Splint {} lost its process in rollback",
                splint.splint_id
            );
            let Response::Splint { runtime, .. } = client
                .request(Request::InspectSplint {
                    splint_id: splint.splint_id,
                })
                .await
            else {
                panic!("no Splint response");
            };
            assert_eq!(runtime.live_incarnation, Some(splint.incarnation));
            client
                .wait_for_text(splint.splint_id, splint.incarnation, "ready")
                .await;
            client
                .echo(splint.splint_id, splint.incarnation, "echo-after-rollback")
                .await;
        }
        assert_exits_recorded(&mut client, directory, record).await;
        assert_listeners_accept(directory, daemon_generation).await;
        assert_daemon_threads_unblocked(pid);
        assert_new_splint_unblocked(&mut client, pid, record).await
    });

    let logs = stage.logs();
    for expected in [
        "daemon handoff candidate failed before its adoption commit",
        "rollback_edge=true",
        "rolling back to the previous generation with the unchanged descriptors",
        "daemon handoff: adopted generation published",
    ] {
        assert!(logs.contains(expected), "missing {expected:?}\n{logs}");
    }
    log_line(
        &logs,
        "daemon handoff: executing the sealed rollback daemon",
    );
    let committed = log_line(&logs, "daemon handoff: adoption committed");
    for expected in [
        "daemon_generation=1",
        "source_daemon_generation=2",
        "rollback=true",
        "carried=2",
        "excluded=1",
        "excluded_exit_unknown=0",
    ] {
        assert!(
            committed.contains(expected),
            "missing {expected:?} in {committed:?}\n{logs}"
        );
    }
    assert_eq!(
        logs.contains("install_pair_mismatch=true"),
        install_pair_mismatch,
        "{logs}"
    );
    if stray_descriptors {
        // Reported at the rollback exec, closed by the rollback daemon's claim.
        assert!(
            logs.contains("unexpected inheritable descriptors cross the rollback exec"),
            "{logs}"
        );
        assert!(
            committed.contains("closed_unexpected=1"),
            "{committed:?}\n{logs}"
        );
    } else {
        assert_no_stray_descriptors(&logs);
    }
    launched
}

/// An ordered shutdown of the serving daemon ends it and its Splints.
fn shut_down(stage: &mut Stage, record: &StageRecord, launched: &[u32]) {
    signal(record.pid, Signal::TERM);
    let status = stage.wait(WAIT * 3);
    assert!(status.success(), "{status}\n{}", stage.logs());
    for splint in &record.carried {
        wait_until_exited(splint.child_pid, "carried Splint");
    }
    for launched in launched {
        wait_until_exited(*launched, "Splint launched after the handoff");
    }
}

/// The forward client is gone from the prefix when the forward daemon
/// adopts, so adoption refuses before its commit and the candidate rolls
/// back (Plan 0061 M3.11a): the rollback daemon adopts the unchanged PTYs
/// and only reports the changed install pair.
fn rollback_case() {
    let directory = TestDirectory::new();
    let mut stage = Stage::launch(
        &directory,
        StageOptions {
            remove_forward_client: true,
            ..StageOptions::default()
        },
    );
    let record = stage.record();
    let launched = assert_rolled_back(&mut stage, &directory, &record, true, false);
    let logs = stage.logs();
    for expected in [
        // The forward exec happened: the forward daemon claimed and staged.
        "daemon handoff candidate: inherited descriptors claimed",
        "every inherited descriptor and checkpoint verified",
        // It refused at adoption and said why.
        "phase=\"adoption\"",
        "the install prefix does not hold the forward pair the manifest recorded",
        // The rollback daemon adopts despite the changed pair.
        "installed pair changed since handoff preparation; rollback adopts anyway",
        "the installed pair changed since preparation",
    ] {
        assert!(logs.contains(expected), "missing {expected:?}\n{logs}");
    }
    assert!(!logs.contains("rollback failed"), "{logs}");
    shut_down(&mut stage, &record, &[launched]);
}

/// The rollback daemon's own adoption fails too. A rollback generation has
/// no rollback edge: it exits loudly and nothing is executed again.
fn rollback_of_rollback_case() {
    let directory = TestDirectory::new();
    let mut stage = Stage::launch(
        &directory,
        StageOptions {
            remove_forward_client: true,
            fault: Some(format!("{}@rollback", CandidateFaultPoint::Startup.name())),
            ..StageOptions::default()
        },
    );
    let record = stage.record();
    let status = stage.wait(STAGE_WAIT);
    let logs = stage.logs();
    assert!(!status.success(), "the refused rollback exited 0\n{logs}");
    for expected in [
        "the install prefix does not hold the forward pair the manifest recorded",
        "rollback_edge=true",
        "injected candidate fault at startup",
        "rollback_edge=false",
        "this daemon is itself a rollback generation, so a failure before its commit has no rollback edge",
        "live-process continuity is not guaranteed",
    ] {
        assert!(logs.contains(expected), "missing {expected:?}\n{logs}");
    }
    // Exactly one rollback exec: the failed rollback generation executed
    // nothing, and nothing committed.
    log_line(
        &logs,
        "daemon handoff: executing the sealed rollback daemon",
    );
    assert!(!logs.contains("adoption committed"), "{logs}");
    // Exiting closed the carried masters, which hung up each carried shell.
    for splint in &record.carried {
        wait_until_exited(splint.child_pid, "carried Splint after a refused rollback");
    }
    // Neither generation committed, so the durable Topology is exactly what
    // the old generation persisted.
    assert_eq!(
        fs::read(directory.0.join("state/splinterm/topology.json")).unwrap(),
        fs::read(directory.0.join(STAGED_TOPOLOGY)).unwrap()
    );
}

/// A SIGTERM pending across the handoff exec (as `systemctl stop` in the
/// exec window) must not kill the candidate: the exec carries it blocked,
/// the candidate unblocks it once its handler exists, and the adopted
/// daemon then shuts down in order. With `rollback`, the forward candidate
/// fails after its handler took the signal, and the rollback executor makes
/// it pending again across the rollback exec, so the rollback daemon shuts
/// down in order once it adopted.
fn pending_sigterm_case(rollback: bool) {
    let directory = TestDirectory::new();
    let mut stage = Stage::launch(
        &directory,
        StageOptions {
            remove_forward_client: rollback,
            pending_sigterm: true,
            ..StageOptions::default()
        },
    );
    let record = stage.record();
    let status = stage.wait(STAGE_WAIT);
    let logs = stage.logs();
    assert!(
        status.success(),
        "a pending SIGTERM must become an ordered shutdown, not kill the candidate: {status}\n{logs}"
    );
    let committed = log_line(&logs, "daemon handoff: adoption committed");
    let generation = if rollback {
        "daemon_generation=1"
    } else {
        "daemon_generation=2"
    };
    assert!(committed.contains(generation), "{committed:?}\n{logs}");
    assert!(
        logs.contains("daemon handoff: adopted generation published"),
        "{logs}"
    );
    if rollback {
        log_line(
            &logs,
            "daemon handoff: executing the sealed rollback daemon",
        );
        assert!(logs.contains("terminate: true"), "{logs}");
    } else {
        assert!(
            !logs.contains("executing the sealed rollback daemon"),
            "{logs}"
        );
    }
    for splint in &record.carried {
        wait_until_exited(splint.child_pid, "carried Splint after an ordered shutdown");
    }
}

/// The forward candidate fails at one pre-commit step and rolls back with
/// every Splint alive (Plan 0061 M3.11a fault matrix).
fn fault_case(point: CandidateFaultPoint) {
    let directory = TestDirectory::new();
    let mut stage = Stage::launch(
        &directory,
        StageOptions {
            fault: Some(format!("{}@forward", point.name())),
            ..StageOptions::default()
        },
    );
    let record = stage.record();
    let launched = assert_rolled_back(&mut stage, &directory, &record, false, false);
    let (phase, cause) = match point {
        CandidateFaultPoint::Staging => ("staging", "injected candidate fault at staging"),
        CandidateFaultPoint::RuntimeEntry => ("runtime entry", "candidate runtime entry refused"),
        CandidateFaultPoint::Startup => ("startup", "injected candidate fault at startup"),
        CandidateFaultPoint::ListenerRegistration => (
            "adoption",
            "injected candidate fault at listener registration",
        ),
        CandidateFaultPoint::ScopeAdoption => {
            ("adoption", "injected candidate fault at scope adoption")
        }
        CandidateFaultPoint::ActorStaging => {
            ("adoption", "injected candidate fault at actor staging")
        }
        CandidateFaultPoint::StrayDescriptor => unreachable!("not a fault point"),
    };
    let logs = stage.logs();
    for expected in [format!("phase=\"{phase}\""), cause.to_owned()] {
        assert!(logs.contains(&expected), "missing {expected:?}\n{logs}");
    }
    shut_down(&mut stage, &record, &[launched]);
}

/// The manifests record a wrong digest for the rollback daemon snapshot.
/// Forward staging refuses it, and the rollback executor, which verifies the
/// image it would run whatever staging got to, refuses to execute it: the
/// candidate exits loudly with no exec at all (Plan 0061 M3.11a review).
fn tampered_rollback_image_case() {
    let directory = TestDirectory::new();
    let mut stage = Stage::launch(
        &directory,
        StageOptions {
            tamper_rollback_image: true,
            ..StageOptions::default()
        },
    );
    let record = stage.record();
    let status = stage.wait(STAGE_WAIT);
    let logs = stage.logs();
    assert!(!status.success(), "the refused candidate exited 0\n{logs}");
    for expected in [
        "phase=\"staging\"",
        "candidate immutable descriptor does not match manifest authority",
        "the inherited rollback daemon image is not the one the rollback manifest records",
        "live-process continuity is not guaranteed",
    ] {
        assert!(logs.contains(expected), "missing {expected:?}\n{logs}");
    }
    assert!(
        !logs.contains("executing the sealed rollback daemon"),
        "{logs}"
    );
    for splint in &record.carried {
        wait_until_exited(splint.child_pid, "carried Splint after a refused rollback");
    }
    assert_eq!(
        fs::read(directory.0.join("state/splinterm/topology.json")).unwrap(),
        fs::read(directory.0.join(STAGED_TOPOLOGY)).unwrap()
    );
}

/// The failed forward candidate holds an inheritable descriptor no manifest
/// names (as a leaked one would be). The rollback exec reports it instead of
/// refusing, the rollback daemon's own claim closes it, and rollback keeps
/// every Splint (Plan 0061 M3.11a review).
fn stray_descriptor_case() {
    let directory = TestDirectory::new();
    let mut stage = Stage::launch(
        &directory,
        StageOptions {
            remove_forward_client: true,
            fault: Some(format!(
                "{}@forward",
                CandidateFaultPoint::StrayDescriptor.name()
            )),
            ..StageOptions::default()
        },
    );
    let record = stage.record();
    let launched = assert_rolled_back(&mut stage, &directory, &record, true, true);
    let logs = stage.logs();
    assert!(
        logs.contains("test seam: left an inheritable descriptor open"),
        "{logs}"
    );
    shut_down(&mut stage, &record, &[launched]);
}
