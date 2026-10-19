//! The old generation's forward exec call site in a build without the
//! `integration-test` feature (Plan 0061 M3.11b). Production builds now
//! contain the exec. What keeps a running daemon from reaching it is the
//! binary's live handoff decision, not compilation.
//!
//! * `exec`: an armed plan really executes through
//!   [`cross_exec_boundary`]. The image is this test binary itself, executed
//!   from its descriptor. The `post-exec` stage then runs in the same
//!   process and checks what crossed: exactly the recorded descriptors are
//!   inheritable, each manifest is sealed with exactly its encoding, the
//!   image descriptor did not cross, and SIGINT, SIGTERM, and SIGHUP arrived
//!   blocked.
//! * `exec-failure`: an armed plan whose image cannot execute returns
//!   [`ExecBoundaryError::Exec`]. An unarmed plan returns
//!   [`ExecBoundaryError::ExecNotWired`]. Both restore close-on-exec and the
//!   signal mask and release the guard.
//!
//! Like `handoff_exec_resume`, each stage runs in a fresh child copy of this
//! binary with only its main thread and no inherited descriptors, because
//! the process guard needs one thread, which libtest never provides.
//! Harness arguments are accepted and ignored; `--list` prints the stages
//! the way libtest lists tests.

use std::{
    env,
    ffi::CString,
    fs,
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd, RawFd},
        unix::{ffi::OsStrExt, net::UnixListener},
    },
    path::PathBuf,
    process::{Command, ExitCode},
    time::{Duration, Instant},
};

use nix::sys::signal::{SigSet, SigmaskHow, Signal, pthread_sigmask};
use rustix::{
    fs::{MemfdFlags, fcntl_get_seals, fstat, memfd_create},
    io::{FdFlags, fcntl_getfd, fcntl_setfd, pread, write},
};
use splinterd::{
    handoff_coordinator::HandoffProcessGuard,
    handoff_descriptors::{HandoffDescriptor, HandoffDescriptorSlot, inheritable_descriptors},
    handoff_exec::{ExecBoundaryError, ExecInheritancePlan, ManifestSeal, cross_exec_boundary},
};

const STAGE: &str = "SPLINTERD_EXEC_PRODUCTION_STAGE";
/// `listener,forward,rollback,image` descriptor numbers for `post-exec`.
const DESCRIPTORS: &str = "SPLINTERD_EXEC_PRODUCTION_DESCRIPTORS";
const STAGES: [&str; 2] = ["exec", "exec-failure"];
const POST_EXEC_MARKER: &str = "handoff_exec_production: post-exec checks passed";
const POST_EXEC_STATUS: i32 = 42;
const FORWARD_BYTES: &[u8] = b"forward manifest encoding";
const ROLLBACK_BYTES: &[u8] = b"rollback manifest encoding";
const GUARD_WAIT: Duration = Duration::from_secs(2);

fn main() -> ExitCode {
    if env::args().skip(1).any(|argument| argument == "--list") {
        for stage in STAGES {
            println!("handoff_exec_production::{stage}: test");
        }
        return ExitCode::SUCCESS;
    }
    match env::var(STAGE).ok().as_deref() {
        Some("exec") => exec_stage(),
        Some("post-exec") => return post_exec_stage(),
        Some("exec-failure") => exec_failure_stage(),
        Some(stage) => panic!("unknown stage {stage:?}"),
        None => run_stages(),
    }
    ExitCode::SUCCESS
}

fn run_stages() {
    // As in `handoff_exec_resume`: each stage starts without descriptors
    // inherited from here, and this deliberately inheritable one proves it.
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
        let stdout = String::from_utf8_lossy(&output.stdout);
        let expected = if stage == "exec" {
            Some(POST_EXEC_STATUS)
        } else {
            Some(0)
        };
        assert!(
            output.status.code() == expected
                && (stage != "exec" || stdout.contains(POST_EXEC_MARKER)),
            "{stage} stage failed with {}\nstdout:\n{stdout}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        println!("handoff_exec_production: {stage} stage passed");
    }
    println!("handoff_exec_production: ok");
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

fn bound_listener(name: &str) -> (UnixListener, PathBuf) {
    let directory = env::temp_dir().join(format!("sd-exec-prod-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir(&directory).unwrap();
    let listener = UnixListener::bind(directory.join("socket")).unwrap();
    (listener, directory)
}

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

fn owners<'a>(
    listener: &'a UnixListener,
    manifests: &'a [OwnedFd; 2],
) -> Vec<HandoffDescriptor<'a>> {
    vec![
        HandoffDescriptor::new(HandoffDescriptorSlot::Listener, listener.as_fd()),
        HandoffDescriptor::new(
            HandoffDescriptorSlot::CheckpointManifest,
            manifests[0].as_fd(),
        ),
        HandoffDescriptor::new(
            HandoffDescriptorSlot::RollbackManifest,
            manifests[1].as_fd(),
        ),
    ]
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

fn sealed_with(descriptor: BorrowedFd<'_>, expected: &[u8]) -> bool {
    let sealed = fcntl_get_seals(descriptor)
        .unwrap()
        .contains(splinterm_filemap::REQUIRED_IMMUTABLE_SEALS);
    let mut bytes = vec![0; expected.len() + 1];
    let read = pread(descriptor, &mut bytes, 0).unwrap();
    sealed
        && u64::try_from(fstat(descriptor).unwrap().st_size).ok()
            == u64::try_from(expected.len()).ok()
        && &bytes[..read] == expected
}

fn handoff_signals_blocked() -> bool {
    let mut mask = SigSet::empty();
    pthread_sigmask(SigmaskHow::SIG_BLOCK, None, Some(&mut mask)).unwrap();
    [Signal::SIGINT, Signal::SIGTERM, Signal::SIGHUP]
        .iter()
        .all(|signal| mask.contains(*signal))
}

fn handoff_signals_unblocked() -> bool {
    let mut mask = SigSet::empty();
    pthread_sigmask(SigmaskHow::SIG_BLOCK, None, Some(&mut mask)).unwrap();
    [Signal::SIGINT, Signal::SIGTERM, Signal::SIGHUP]
        .iter()
        .all(|signal| !mask.contains(*signal))
}

/// Arms a plan with this binary's own image and crosses the boundary, which
/// must not return.
fn exec_stage() {
    assert_single_threaded();
    assert!(handoff_signals_unblocked());
    // The socket directory is the post-exec stage's to remove.
    let (listener, _directory) = bound_listener("exec");
    let manifests = manifest_storage();
    let owners = owners(&listener, &manifests);
    let plan = plan(&owners, &manifests);
    let executable = env::current_exe().unwrap();
    // Opened close-on-exec, as a sealed snapshot descriptor is held.
    let image = fs::File::open(&executable).unwrap();
    assert!(close_on_exec(image.as_fd()));
    let numbers = [
        listener.as_raw_fd(),
        manifests[0].as_raw_fd(),
        manifests[1].as_raw_fd(),
        image.as_raw_fd(),
    ];
    let mut environment = env::vars_os()
        .filter(|(key, _)| key != STAGE && key != DESCRIPTORS)
        .map(|(key, value)| {
            let mut entry = key.as_bytes().to_vec();
            entry.push(b'=');
            entry.extend_from_slice(value.as_bytes());
            CString::new(entry).unwrap()
        })
        .collect::<Vec<_>>();
    environment.push(CString::new(format!("{STAGE}=post-exec")).unwrap());
    environment.push(
        CString::new(format!(
            "{DESCRIPTORS}={}",
            numbers.map(|fd| fd.to_string()).join(",")
        ))
        .unwrap(),
    );
    let arguments = vec![CString::new(executable.as_os_str().as_bytes()).unwrap()];
    let plan = plan.with_forward_daemon(image.as_fd(), arguments, environment);
    let reason = cross_exec_boundary(&plan, Instant::now() + GUARD_WAIT);
    panic!("the armed exec boundary returned instead of executing: {reason}");
}

/// Runs in the process `exec_stage` executed into.
fn post_exec_stage() -> ExitCode {
    let numbers = env::var(DESCRIPTORS)
        .unwrap()
        .split(',')
        .map(|number| number.parse::<RawFd>().unwrap())
        .collect::<Vec<_>>();
    let [listener, forward, rollback, image] = numbers[..] else {
        panic!("four descriptor numbers expected: {numbers:?}");
    };
    assert_eq!(
        fs::read_dir("/proc/self/task").unwrap().count(),
        1,
        "the executed image starts single-threaded"
    );
    // Checked before this stage opens anything that could take its number.
    assert!(
        fs::symlink_metadata(format!("/proc/self/fd/{image}")).is_err(),
        "the close-on-exec image descriptor did not cross exec"
    );
    let mut inheritable = inheritable_descriptors(4096)
        .unwrap()
        .into_iter()
        .map(|(fd, _)| fd)
        .collect::<Vec<_>>();
    inheritable.sort_unstable();
    let mut recorded = vec![listener, forward, rollback];
    recorded.sort_unstable();
    assert_eq!(
        inheritable, recorded,
        "exactly the recorded descriptors crossed exec, inheritable"
    );
    // The package denies `unsafe`, so the inherited memfds are read through
    // a fresh open of the same inode, which carries the same seals.
    for (fd, expected) in [(forward, FORWARD_BYTES), (rollback, ROLLBACK_BYTES)] {
        let reopened = fs::File::open(format!("/proc/self/fd/{fd}")).unwrap();
        assert!(sealed_with(reopened.as_fd(), expected), "manifest {fd}");
    }
    assert!(
        handoff_signals_blocked(),
        "SIGINT, SIGTERM, and SIGHUP cross exec blocked"
    );
    let directory = env::temp_dir().join(format!("sd-exec-prod-{}-exec", std::process::id()));
    let _ = fs::remove_dir_all(directory);
    println!("{POST_EXEC_MARKER}");
    ExitCode::from(u8::try_from(POST_EXEC_STATUS).unwrap())
}

/// An armed plan whose image cannot execute, then an unarmed plan: both
/// return, restore every descriptor and the signal mask, and release the
/// guard.
fn exec_failure_stage() {
    assert_single_threaded();
    let before = {
        let mut mask = SigSet::empty();
        pthread_sigmask(SigmaskHow::SIG_BLOCK, None, Some(&mut mask)).unwrap();
        mask
    };

    let (listener, directory) = bound_listener("exec-failure");
    let manifests = manifest_storage();
    let owners_list = owners(&listener, &manifests);
    let armed = plan(&owners_list, &manifests);
    // Not an executable image: `execveat` refuses it before the point of
    // no return.
    let image = memfd_create("not-an-executable", MemfdFlags::CLOEXEC).unwrap();
    write(&image, b"this is not an executable image\n").unwrap();
    let armed = armed.with_forward_daemon(
        image.as_fd(),
        vec![CString::new("not-an-executable").unwrap()],
        Vec::new(),
    );
    let reason = cross_exec_boundary(&armed, Instant::now() + GUARD_WAIT);
    assert!(matches!(reason, ExecBoundaryError::Exec(_)), "{reason}");
    assert_restored(&armed, &manifests, &before);

    let unarmed_manifests = manifest_storage();
    let unarmed_owners = owners(&listener, &unarmed_manifests);
    let unarmed = plan(&unarmed_owners, &unarmed_manifests);
    let reason = cross_exec_boundary(&unarmed, Instant::now() + GUARD_WAIT);
    assert!(
        matches!(reason, ExecBoundaryError::ExecNotWired),
        "{reason}"
    );
    assert_restored(&unarmed, &unarmed_manifests, &before);
    fs::remove_dir_all(directory).unwrap();
}

fn assert_restored(plan: &ExecInheritancePlan<'_>, manifests: &[OwnedFd; 2], before: &SigSet) {
    assert!(
        plan.descriptors()
            .iter()
            .all(|descriptor| close_on_exec(descriptor.descriptor())),
        "close-on-exec is restored on every recorded descriptor"
    );
    assert_eq!(inheritable_descriptors(4096).unwrap(), []);
    // Sealing is permanent, with exactly the encodings.
    assert!(sealed_with(manifests[0].as_fd(), FORWARD_BYTES));
    assert!(sealed_with(manifests[1].as_fd(), ROLLBACK_BYTES));
    let mut after = SigSet::empty();
    pthread_sigmask(SigmaskHow::SIG_BLOCK, None, Some(&mut after)).unwrap();
    assert_eq!(&after, before, "the signal mask is restored");
    // The guard was released: it can be taken again at once.
    drop(HandoffProcessGuard::acquire_until(Instant::now() + GUARD_WAIT).unwrap());
}
