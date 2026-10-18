//! The old daemon's exec boundary under the real process guard, ending in the
//! in-process resume that stands in for exec until M3.10 (Plan 0061 M3.9,
//! operator decision D5).
//!
//! Real PTY Splints are captured, ended for handoff, and the Tokio runtime is
//! dropped; the boundary then takes the guard, seals both manifest memfds,
//! clears close-on-exec on exactly the recorded descriptors, restores it, and
//! releases the guard; a new runtime resumes every Splint. Each stage runs in
//! a fresh child copy of this binary, because the guard needs a process whose
//! only thread is the main thread, which libtest never provides (see
//! `handoff_runtime_stop.rs`). Harness arguments are accepted and ignored;
//! `--list` prints the stages the way libtest lists tests.
//!
//! Workload scopes are test doubles: the old placement records whether it
//! was retained, released or dropped (dropping a real scope stops it, which
//! kills the Splint), and the adopter records every adoption.

#![cfg(feature = "integration-test")]

use std::{
    env, fs,
    os::{
        fd::{AsFd, BorrowedFd, OwnedFd},
        unix::net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    process::{Command, ExitCode},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use rustix::{
    fs::{MemfdFlags, fcntl_get_seals, fstat, memfd_create},
    io::{FdFlags, fcntl_getfd, fcntl_setfd},
};
use splinterd::{
    HandoffCheckpointBudget, LiveError, LiveSplintConfig, LiveSplintHandle, LiveSplintRuntime,
    ProcessIncarnation, ProcessPlacement,
    cgroup::{DojoSliceReclaim, RetainedWorkloadScope, WorkloadUnitError},
    handoff_adoption::WorkloadScopeAdopter,
    handoff_descriptors::{HandoffDescriptor, HandoffDescriptorSlot, inheritable_descriptors},
    handoff_exec::{
        ExecBoundaryError, ExecBoundaryStage, ExecInheritancePlan, ManifestSeal, ResumableSplint,
        ResumeBudgets, ResumedFleet, cross_exec_boundary_hooked_for_integration_test,
        reap_exited_child, resume_handed_off,
    },
    handoff_prepare::{CaptureLimits, HandedOffCapture, capture_live_splints},
};
use splinterm_core::{DojoId, SplintId};
use splinterm_pty::{LinuxPtyBackend, PtyCommand};
use splinterm_terminal::{SharedImageBudget, SharedKittyUploadBudget};

const STAGE: &str = "SPLINTERD_EXEC_RESUME_STAGE";
const STAGES: [&str; 3] = ["resume", "guard-timeout", "faults"];
const CAT: &str = "printf ready; exec cat";
const READ_THEN_EXIT: &str = "printf ready; read line; exit 7";
const BUDGET_BYTES: usize = 1 << 20;
const GUARD_WAIT: Duration = Duration::from_secs(2);
const WAIT: Duration = Duration::from_secs(5);

fn main() -> ExitCode {
    if env::args().skip(1).any(|argument| argument == "--list") {
        for stage in STAGES {
            println!("handoff_exec_resume::{stage}: test");
        }
        return ExitCode::SUCCESS;
    }
    match env::var(STAGE).ok().as_deref() {
        Some("resume") => resume_stage(),
        Some("guard-timeout") => guard_timeout_stage(),
        Some("faults") => faults_stage(),
        Some(stage) => panic!("unknown stage {stage:?}"),
        None => {
            // The process-wide audit refuses any stray inheritable
            // descriptor, and a harness may leak one into this process (CI
            // run 39137743159 did). Each stage starts without descriptors
            // inherited from here, as the sibling descriptor tests do; this
            // deliberately inheritable one proves it on every run.
            let stray = fs::File::open("/dev/null").unwrap();
            let flags = fcntl_getfd(&stray).unwrap();
            fcntl_setfd(&stray, flags - FdFlags::CLOEXEC).unwrap();
            for stage in STAGES {
                let output = Command::new("python3")
                    .arg("-c")
                    .arg(
                        "import subprocess, sys; raise SystemExit(subprocess.run(sys.argv[1:], close_fds=True).returncode)",
                    )
                    .arg(env::current_exe().unwrap())
                    .env(STAGE, stage)
                    .output()
                    .expect("launch a stage without inherited descriptors");
                assert!(
                    output.status.success(),
                    "{stage} stage failed with {}\nstdout:\n{}\nstderr:\n{}",
                    output.status,
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                println!("handoff_exec_resume: {stage} stage passed");
            }
            println!("handoff_exec_resume: ok");
        }
    }
    ExitCode::SUCCESS
}

/// What happened to each placement: `(splint, owner, event)`.
static EVENTS: Mutex<Vec<(SplintId, &str, &str)>> = Mutex::new(Vec::new());

fn events_for(owner: &str) -> Vec<(SplintId, &'static str)> {
    EVENTS
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, recorded, _)| *recorded == owner)
        .map(|(splint, _, event)| (*splint, *event))
        .collect()
}

/// A workload scope stand-in. Dropping it unsettled is what stops a real
/// scope and kills its Splint.
#[derive(Debug)]
struct FakeScope {
    splint: SplintId,
    owner: &'static str,
    settled: bool,
}

impl FakeScope {
    fn record(&self, event: &'static str) {
        EVENTS
            .lock()
            .unwrap()
            .push((self.splint, self.owner, event));
    }
}

impl ProcessPlacement for FakeScope {
    fn release(mut self) {
        self.settled = true;
        self.record("released");
    }

    fn retain_for_handoff(mut self) {
        self.settled = true;
        self.record("retained");
    }
}

impl Drop for FakeScope {
    fn drop(&mut self) {
        if !self.settled {
            self.record("dropped");
        }
    }
}

/// A fresh manager stand-in: adopts every identity, except `refuse`'s.
#[derive(Clone, Default)]
struct FakeAdopter {
    adopted: Arc<Mutex<Vec<RetainedWorkloadScope>>>,
    refuse: Option<SplintId>,
}

impl WorkloadScopeAdopter for FakeAdopter {
    type Scope = FakeScope;

    fn adopt_scope(
        &self,
        retained: &RetainedWorkloadScope,
    ) -> Result<FakeScope, WorkloadUnitError> {
        if self.refuse == Some(retained.splint_id()) {
            return Err(WorkloadUnitError::RetainedScopeMismatch);
        }
        self.adopted.lock().unwrap().push(retained.clone());
        Ok(FakeScope {
            splint: retained.splint_id(),
            owner: "adopted",
            settled: false,
        })
    }

    fn reclaim_empty_dojo_slice(&self, _: DojoId) -> Result<DojoSliceReclaim, WorkloadUnitError> {
        Ok(DojoSliceReclaim::Absent)
    }

    fn retained_units_absent(&self, _: &RetainedWorkloadScope) -> Result<bool, WorkloadUnitError> {
        Ok(false)
    }
}

fn daemon_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

fn backend() -> LinuxPtyBackend {
    let test_binary = env::current_exe().unwrap();
    let helper = test_binary
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("splinterm-pty-child");
    assert!(helper.is_file(), "missing PTY helper: {}", helper.display());
    LinuxPtyBackend::new(helper)
}

fn budgets() -> ResumeBudgets {
    ResumeBudgets {
        image: SharedImageBudget::new(BUDGET_BYTES),
        kitty_upload: SharedKittyUploadBudget::new(BUDGET_BYTES),
    }
}

fn config(budgets: &ResumeBudgets) -> LiveSplintConfig {
    let mut config = LiveSplintConfig {
        columns: 60,
        rows: 24,
        hangup_grace: Duration::from_millis(30),
        terminate_grace: Duration::from_millis(30),
        poll_interval: Duration::from_millis(5),
        exit_drain_timeout: Duration::from_millis(50),
        ..LiveSplintConfig::default()
    };
    config.terminal.shared_image_budget = Some(budgets.image.clone());
    config.terminal.shared_kitty_upload_budget = Some(budgets.kitty_upload.clone());
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

async fn wait_for_text(handle: &LiveSplintHandle, text: &str) {
    tokio::time::timeout(WAIT, async {
        while !screen_text(handle).await.contains(text) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{} never showed {text:?}", handle.splint_id));
}

async fn assert_echoes(handle: &LiveSplintHandle, token: &str) {
    handle
        .input(format!("{token}\n").into_bytes())
        .await
        .unwrap();
    wait_for_text(handle, token).await;
}

/// One live Splint the old generation hands off.
struct Spawned {
    dojo_id: DojoId,
    runtime: LiveSplintRuntime,
}

/// The old generation after its runtime stopped: every captured Splint ended
/// for handoff, and the child of a Splint excluded because it had exited.
struct Stopped {
    dojos: Vec<(SplintId, DojoId)>,
    handed_off: Vec<HandedOffCapture>,
    incarnations: Vec<(SplintId, ProcessIncarnation, u32)>,
    excluded_pid: Option<u32>,
}

/// Spawns `scripts` (each in the Dojo its index names) and, when `exited`, a
/// Splint whose shell exits at once with code 3 that no actor reaps; captures
/// every live one, ends it for handoff, and drops the runtime.
fn hand_off_and_stop(scripts: &[(&str, usize)], exited: bool, budgets: &ResumeBudgets) -> Stopped {
    let runtime = daemon_runtime();
    let dojo_ids = (0..scripts.len())
        .map(|_| DojoId::new())
        .collect::<Vec<_>>();
    let stopped = runtime.block_on(async {
        let mut spawned = Vec::new();
        for (script, dojo) in scripts {
            let splint_id = SplintId::new();
            let command = PtyCommand::new("/bin/sh", PathBuf::from("/tmp")).args(["-c", script]);
            let runtime = LiveSplintRuntime::spawn_with_placement(
                splint_id,
                ProcessIncarnation::allocate(),
                backend(),
                command,
                config(budgets),
                move |_| {
                    Ok(FakeScope {
                        splint: splint_id,
                        owner: "old",
                        settled: false,
                    })
                },
            )
            .await
            .unwrap();
            wait_for_text(&runtime.handle(), "ready").await;
            spawned.push(Spawned {
                dojo_id: dojo_ids[*dojo],
                runtime,
            });
        }
        let excluded = if exited {
            // Polled once an hour, and its shell outlives the first poll, so
            // its actor never reaps the exited child.
            let mut lazy = config(budgets);
            lazy.poll_interval = Duration::from_secs(3600);
            let command =
                PtyCommand::new("/bin/sh", PathBuf::from("/tmp")).args(["-c", "sleep 0.3; exit 3"]);
            let runtime = LiveSplintRuntime::spawn(SplintId::new(), backend(), command, lazy)
                .await
                .unwrap();
            let handle = runtime.handle();
            tokio::time::timeout(WAIT, async {
                while handle.root_process_is_live() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            Some(runtime)
        } else {
            None
        };
        let mut handles = spawned
            .iter()
            .map(|spawned| spawned.runtime.handle())
            .collect::<Vec<_>>();
        handles.extend(excluded.iter().map(LiveSplintRuntime::handle));
        let captures = capture_live_splints(
            handles,
            CaptureLimits::DEFAULT,
            |handle: LiveSplintHandle, budget: HandoffCheckpointBudget| async move {
                handle.prepare_pty_handoff_within(&budget).await
            },
        )
        .await
        .unwrap();
        assert_eq!(captures.excluded().len(), usize::from(exited));
        let (handed_off, _) = captures.hand_off();
        let mut dojos = Vec::new();
        let mut incarnations = Vec::new();
        for Spawned { dojo_id, runtime } in spawned {
            let handle = runtime.handle();
            dojos.push((handle.splint_id, dojo_id));
            incarnations.push((handle.splint_id, handle.incarnation, handle.child_pid()));
            // Each actor leaves through its handoff path.
            assert!(matches!(runtime.wait().await, Err(LiveError::HandedOff)));
        }
        let excluded_pid = excluded
            .as_ref()
            .map(|runtime| runtime.handle().child_pid());
        // The excluded actor is still running; it ends with the runtime.
        drop(excluded);
        Stopped {
            dojos,
            handed_off,
            incarnations,
            excluded_pid,
        }
    });
    runtime.shutdown_timeout(Duration::from_secs(2));
    // Every captured placement was retained, none released or dropped.
    let old = events_for("old");
    for (splint_id, ..) in &stopped.incarnations {
        assert_eq!(
            old.iter()
                .filter(|(splint, _)| splint == splint_id)
                .map(|(_, event)| *event)
                .collect::<Vec<_>>(),
            ["retained"],
            "old placement of {splint_id}"
        );
    }
    stopped
}

/// Two empty sealable memfds, like the ones manifest building creates.
fn manifest_storage() -> [OwnedFd; 2] {
    let memfd = || {
        memfd_create(
            "test-handoff-manifest",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        )
        .unwrap()
    };
    [memfd(), memfd()]
}

const FORWARD_BYTES: &[u8] = b"forward manifest encoding";
const ROLLBACK_BYTES: &[u8] = b"rollback manifest encoding";

fn owners<'a>(
    listener: &'a UnixListener,
    manifests: &'a [OwnedFd; 2],
    handed_off: &'a [HandedOffCapture],
) -> Vec<HandoffDescriptor<'a>> {
    let mut owners = vec![
        HandoffDescriptor::new(HandoffDescriptorSlot::Listener, listener.as_fd()),
        HandoffDescriptor::new(
            HandoffDescriptorSlot::CheckpointManifest,
            manifests[0].as_fd(),
        ),
        HandoffDescriptor::new(
            HandoffDescriptorSlot::RollbackManifest,
            manifests[1].as_fd(),
        ),
    ];
    for capture in handed_off {
        owners.push(HandoffDescriptor::new(
            HandoffDescriptorSlot::PtyMaster(capture.slot()),
            capture.handed_off.session.master(),
        ));
        owners.push(capture.checkpoint.handoff_descriptor());
    }
    owners
}

fn plan<'a>(
    owners: &[HandoffDescriptor<'a>],
    manifests: &'a [OwnedFd; 2],
) -> ExecInheritancePlan<'a> {
    let recorded = owners
        .iter()
        .map(|owner| (owner.slot(), owner.raw_fd()))
        .collect::<Vec<_>>();
    ExecInheritancePlan::new(
        &recorded,
        owners,
        [
            ManifestSeal::new(manifests[0].as_fd(), FORWARD_BYTES),
            ManifestSeal::new(manifests[1].as_fd(), ROLLBACK_BYTES),
        ],
    )
    .unwrap()
}

fn close_on_exec(descriptor: BorrowedFd<'_>) -> bool {
    fcntl_getfd(descriptor).unwrap().contains(FdFlags::CLOEXEC)
}

fn sealed_size(descriptor: BorrowedFd<'_>) -> (i64, bool) {
    let size = fstat(descriptor).unwrap().st_size;
    let sealed = fcntl_get_seals(descriptor)
        .unwrap()
        .contains(splinterm_filemap::REQUIRED_IMMUTABLE_SEALS);
    (size, sealed)
}

/// Crosses the boundary with `fault` injected; at the exec call site, checks
/// that exactly the plan's descriptors are inheritable.
fn cross(
    plan: &ExecInheritancePlan<'_>,
    fault: Option<ExecBoundaryStage>,
) -> (ExecBoundaryError, bool) {
    let mut inspected = false;
    let reason = cross_exec_boundary_hooked_for_integration_test(
        plan,
        Instant::now() + GUARD_WAIT,
        &mut |stage| {
            if stage == ExecBoundaryStage::Exec && fault != Some(ExecBoundaryStage::Exec) {
                let mut expected = plan
                    .descriptors()
                    .iter()
                    .map(|descriptor| descriptor.raw_fd())
                    .collect::<Vec<_>>();
                expected.sort_unstable();
                let mut inheritable = inheritable_descriptors(4096)
                    .unwrap()
                    .into_iter()
                    .map(|(fd, _)| fd)
                    .collect::<Vec<_>>();
                inheritable.sort_unstable();
                assert_eq!(
                    inheritable, expected,
                    "only the recorded descriptors are inheritable"
                );
                inspected = true;
            }
            if Some(stage) == fault {
                Err(())
            } else {
                Ok(())
            }
        },
    );
    (reason, inspected)
}

/// Resumes every handed-off Splint on a new runtime with `adopter`.
fn resume(
    runtime: &tokio::runtime::Runtime,
    stopped: Stopped,
    adopter: &FakeAdopter,
    budgets: &ResumeBudgets,
) -> (
    ResumedFleet,
    Vec<(SplintId, ProcessIncarnation, u32)>,
    Option<u32>,
) {
    let Stopped {
        dojos,
        handed_off,
        incarnations,
        excluded_pid,
    } = stopped;
    let splints = handed_off
        .into_iter()
        .map(|capture| {
            let splint_id = capture.handed_off.evidence.splint_id;
            ResumableSplint {
                dojo_id: dojos
                    .iter()
                    .find(|(splint, _)| *splint == splint_id)
                    .unwrap()
                    .1,
                handed_off: capture.handed_off,
                checkpoint: capture.checkpoint,
                config: config(budgets),
            }
        })
        .collect();
    let fleet = runtime.block_on(resume_handed_off(
        splints,
        Some(adopter.clone()),
        budgets,
        Duration::from_secs(5),
    ));
    (fleet, incarnations, excluded_pid)
}

/// Every resumed Splint is the same incarnation and process, and its terminal
/// is the captured one, not a blank one.
fn assert_resumed(
    runtime: &tokio::runtime::Runtime,
    fleet: &ResumedFleet,
    incarnations: &[(SplintId, ProcessIncarnation, u32)],
) {
    assert!(fleet.stranded.is_empty());
    assert!(fleet.blank_terminals.is_empty());
    assert_eq!(fleet.resumed.len(), incarnations.len());
    runtime.block_on(async {
        for resumed in &fleet.resumed {
            let handle = resumed.runtime.handle();
            let (_, incarnation, pid) = incarnations
                .iter()
                .find(|(splint, ..)| *splint == resumed.splint_id)
                .unwrap();
            assert_eq!(handle.incarnation, *incarnation);
            assert_eq!(resumed.incarnation, *incarnation);
            assert_eq!(handle.child_pid(), *pid);
            assert!(handle.root_process_is_live());
            assert!(screen_text(&handle).await.contains("ready"));
        }
    });
}

fn shut_down(runtime: &tokio::runtime::Runtime, fleet: ResumedFleet) {
    runtime.block_on(async {
        for resumed in fleet.resumed {
            let _ = resumed.runtime.shutdown().await;
        }
    });
}

fn bound_listener(name: &str) -> (UnixListener, PathBuf, PathBuf) {
    let directory = env::temp_dir().join(format!("sd-exec-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir(&directory).unwrap();
    let path = directory.join("socket");
    let listener = UnixListener::bind(&path).unwrap();
    listener.set_nonblocking(true).unwrap();
    (listener, path, directory)
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

/// The full sequence under the real guard: seal, inherit exactly the recorded
/// descriptors, restore, resume. Two Splints share a Dojo whose second scope
/// cannot be adopted, so both run uncontained and the first's adopted scope
/// stays retained; the third Splint's child exits after resume and is reaped
/// by its resumed actor; the excluded child is reaped by the resumed
/// generation.
fn resume_stage() {
    assert_single_threaded();
    let budgets = budgets();
    let (listener, path, directory) = bound_listener("resume");
    let stopped = hand_off_and_stop(&[(CAT, 0), (CAT, 0), (READ_THEN_EXIT, 1)], true, &budgets);
    let refused = stopped.incarnations[1].0;
    let manifests = manifest_storage();
    let owners = owners(&listener, &manifests, &stopped.handed_off);
    let plan = plan(&owners, &manifests);
    let (reason, inspected) = cross(&plan, None);
    assert!(
        matches!(reason, ExecBoundaryError::ExecNotWired),
        "{reason}"
    );
    assert!(inspected, "the exec call site was reached under the guard");
    // Restored, and the guard released.
    assert!(
        plan.descriptors()
            .iter()
            .all(|d| close_on_exec(d.descriptor()))
    );
    assert_eq!(sealed_size(manifests[0].as_fd()), (25, true));
    assert_eq!(sealed_size(manifests[1].as_fd()), (26, true));
    drop((owners, plan));

    let runtime = daemon_runtime();
    let adopter = FakeAdopter {
        refuse: Some(refused),
        ..FakeAdopter::default()
    };
    let shared_dojo = stopped.dojos[0].1;
    let (fleet, incarnations, excluded_pid) = resume(&runtime, stopped, &adopter, &budgets);
    assert_resumed(&runtime, &fleet, &incarnations);
    // Each identity was adopted at most once; the refused one never.
    let adoptions = adopter.adopted.lock().unwrap().clone();
    assert_eq!(adoptions.len(), 2);
    assert!(adoptions.iter().all(|scope| scope.splint_id() != refused));
    assert_eq!(fleet.uncontained_dojos, [shared_dojo]);
    // The shared Dojo's adopted scope is retained (pinned), never dropped.
    let first = incarnations[0].0;
    assert_eq!(events_for("adopted"), [(first, "retained")]);

    runtime.block_on(async {
        let exiting = fleet
            .resumed
            .iter()
            .find(|resumed| resumed.splint_id == incarnations[2].0)
            .unwrap()
            .runtime
            .handle();
        exiting.input(b"go\n".to_vec()).await.unwrap();
        let exit = tokio::time::timeout(WAIT, exiting.wait_for_exit())
            .await
            .unwrap()
            .expect("the resumed actor reaps its child");
        assert_eq!(exit.code, Some(7));
        // Every echoing Splint answers on the new runtime.
        for resumed in fleet
            .resumed
            .iter()
            .filter(|r| r.splint_id != exiting.splint_id)
        {
            assert_echoes(&resumed.runtime.handle(), "echo-after-resume").await;
        }
        let listener = tokio::net::UnixListener::from_std(listener).unwrap();
        let client = UnixStream::connect(&path).unwrap();
        let accepted = tokio::time::timeout(WAIT, listener.accept()).await.unwrap();
        assert!(accepted.is_ok());
        drop(client);
    });
    // The excluded child is still an unreaped zombie of this process; the
    // resumed generation reaps it.
    let pid = excluded_pid.unwrap();
    let exit = reap_exited_child(pid).expect("the excluded child is reaped after resume");
    assert_eq!(exit.code, Some(3));
    assert!(reap_exited_child(pid).is_none());
    // Nothing old was ever dropped or released: no scope was stopped.
    assert!(
        events_for("old")
            .iter()
            .all(|(_, event)| *event == "retained")
    );
    shut_down(&runtime, fleet);
    let _ = fs::remove_dir_all(directory);
}

/// Another thread keeps the guard refused: nothing is sealed or made
/// inheritable, and every Splint resumes.
fn guard_timeout_stage() {
    assert_single_threaded();
    let budgets = budgets();
    let (listener, _, directory) = bound_listener("guard");
    let stopped = hand_off_and_stop(&[(CAT, 0)], false, &budgets);
    let (started, ready) = mpsc::channel();
    drop(thread::spawn(move || {
        started.send(()).unwrap();
        loop {
            thread::park();
        }
    }));
    ready.recv().unwrap();
    let manifests = manifest_storage();
    let owners = owners(&listener, &manifests, &stopped.handed_off);
    let plan = plan(&owners, &manifests);
    let (reason, inspected) = cross(&plan, None);
    assert!(matches!(reason, ExecBoundaryError::Guard(_)), "{reason}");
    assert!(!inspected);
    assert!(
        plan.descriptors()
            .iter()
            .all(|d| close_on_exec(d.descriptor()))
    );
    assert_eq!(sealed_size(manifests[0].as_fd()), (0, false));
    assert_eq!(sealed_size(manifests[1].as_fd()), (0, false));
    drop((owners, plan));
    let runtime = daemon_runtime();
    let adopter = FakeAdopter::default();
    let (fleet, incarnations, _) = resume(&runtime, stopped, &adopter, &budgets);
    assert_resumed(&runtime, &fleet, &incarnations);
    runtime.block_on(async {
        assert_echoes(&fleet.resumed[0].runtime.handle(), "echo-after-guard").await;
    });
    assert_eq!(adopter.adopted.lock().unwrap().len(), 1);
    shut_down(&runtime, fleet);
    let _ = fs::remove_dir_all(directory);
}

/// A failure injected at each stage resumes without killing, with
/// close-on-exec restored on every descriptor.
fn faults_stage() {
    assert_single_threaded();
    let budgets = budgets();
    for (fault, sealed) in [
        (ExecBoundaryStage::Guard, [false, false]),
        (ExecBoundaryStage::SealForwardManifest, [false, false]),
        (ExecBoundaryStage::SealRollbackManifest, [true, false]),
        (ExecBoundaryStage::Inheritance, [true, true]),
        (ExecBoundaryStage::Exec, [true, true]),
        (ExecBoundaryStage::Restore, [true, true]),
    ] {
        let (listener, _, directory) = bound_listener("fault");
        let stopped = hand_off_and_stop(&[(CAT, 0)], false, &budgets);
        let manifests = manifest_storage();
        let owners = owners(&listener, &manifests, &stopped.handed_off);
        let plan = plan(&owners, &manifests);
        let (reason, _) = cross(&plan, Some(fault));
        match fault {
            ExecBoundaryStage::Restore => assert!(
                matches!(
                    &reason,
                    ExecBoundaryError::Restore { cause, restore }
                        if matches!(**cause, ExecBoundaryError::ExecNotWired)
                            && matches!(**restore, ExecBoundaryError::Injected(ExecBoundaryStage::Restore))
                ),
                "{reason}"
            ),
            _ => assert!(
                matches!(reason, ExecBoundaryError::Injected(stage) if stage == fault),
                "{fault:?}: {reason}"
            ),
        }
        // Even an injected restore failure leaves every descriptor
        // close-on-exec: the unrestored inheritance restored on drop.
        assert!(
            plan.descriptors()
                .iter()
                .all(|d| close_on_exec(d.descriptor())),
            "{fault:?}"
        );
        for (storage, sealed) in manifests.iter().zip(sealed) {
            assert_eq!(sealed_size(storage.as_fd()).1, sealed, "{fault:?}");
        }
        assert_eq!(
            fs::read_dir("/proc/self/task").unwrap().count(),
            1,
            "{fault:?}"
        );
        drop((owners, plan));
        let runtime = daemon_runtime();
        let (fleet, incarnations, _) = resume(&runtime, stopped, &FakeAdopter::default(), &budgets);
        assert_resumed(&runtime, &fleet, &incarnations);
        runtime.block_on(async {
            assert_echoes(&fleet.resumed[0].runtime.handle(), "echo-after-fault").await;
        });
        shut_down(&runtime, fleet);
        drop(runtime);
        let _ = fs::remove_dir_all(directory);
        assert!(
            events_for("old")
                .iter()
                .all(|(_, event)| *event == "retained")
        );
    }
}
