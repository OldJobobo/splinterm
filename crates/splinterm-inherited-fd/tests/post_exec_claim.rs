//! Post-exec claim contract, exercised in freshly exec'd single-threaded
//! processes.
//!
//! This target uses `harness = false`: libtest runs every test on a spawned
//! thread, which would violate the claim's single-thread precondition. Harness
//! arguments such as `--test-threads=1` are accepted and ignored.

#![allow(unsafe_code)]

use std::{
    env,
    fs::{self, File},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use rustix::{
    fs::{MemfdFlags, OFlags, fcntl_add_seals, memfd_create},
    io::{FdFlags, fcntl_getfd, fcntl_setfd, write},
};
use splinterm_inherited_fd::{
    InheritedFdError, REQUIRED_SEALED_BODY_SEALS, claim_post_exec_inherited,
    read_post_exec_sealed_descriptor,
};

const STAGE: &str = "SPLINTERM_INHERITED_FD_STAGE";
const EXPECTED: &str = "SPLINTERM_INHERITED_FD_EXPECTED";
const UNKNOWN: &str = "SPLINTERM_INHERITED_FD_UNKNOWN";
const SEALED: &str = "SPLINTERM_INHERITED_FD_SEALED";
const UNSEALED: &str = "SPLINTERM_INHERITED_FD_UNSEALED";
const SEALED_BODY: &[u8] = b"sealed post-exec manifest body";
const STAGES: [&str; 5] = [
    "missing",
    "not-inherited",
    "multi-threaded",
    "sealed-read",
    "success",
];
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "splinterm-inherited-fd-{}-{unique}-{sequence}",
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

fn fixture(path: &Path) -> File {
    fs::write(path, path.as_os_str().as_encoded_bytes()).unwrap();
    File::open(path).unwrap()
}

fn is_open(fd: i32) -> bool {
    fs::symlink_metadata(format!("/proc/self/fd/{fd}")).is_ok()
}

fn is_close_on_exec(fd: i32) -> bool {
    let fdinfo = fs::read_to_string(format!("/proc/self/fdinfo/{fd}")).unwrap();
    let flags = fdinfo
        .lines()
        .find_map(|line| line.strip_prefix("flags:\t"))
        .map(|flags| u64::from_str_radix(flags, 8).unwrap())
        .unwrap();
    flags & u64::from(OFlags::CLOEXEC.bits()) != 0
}

/// Every open non-standard descriptor that would cross another exec.
fn inheritable_descriptors() -> Vec<i32> {
    let entries = fs::read_dir("/proc/self/fd")
        .unwrap()
        .map(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_str()
                .unwrap()
                .parse::<i32>()
                .unwrap()
        })
        .filter(|fd| *fd >= 3)
        .collect::<Vec<_>>();
    entries
        .into_iter()
        .filter(|fd| is_open(*fd) && !is_close_on_exec(*fd))
        .collect()
}

fn assert_untouched(fds: &[i32]) {
    for fd in fds {
        assert!(is_open(*fd), "descriptor {fd} was closed");
        assert!(!is_close_on_exec(*fd), "descriptor {fd} lost inheritance");
    }
}

fn file_position(fd: i32) -> u64 {
    fs::read_to_string(format!("/proc/self/fdinfo/{fd}"))
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("pos:\t"))
        .map(|position| position.parse().unwrap())
        .unwrap()
}

fn sealed_memfd(bytes: &[u8], sealed: bool) -> File {
    let fd = memfd_create(
        "post-exec-sealed-body",
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )
    .unwrap();
    let mut written = 0;
    while written < bytes.len() {
        written += write(&fd, &bytes[written..]).unwrap();
    }
    if sealed {
        fcntl_add_seals(&fd, REQUIRED_SEALED_BODY_SEALS).unwrap();
    }
    File::from(fd)
}

fn env_fd(name: &str) -> i32 {
    env::var(name).unwrap().parse().unwrap()
}

fn sealed_read_generation(fixtures: &[i32]) {
    let sealed = env_fd(SEALED);
    let unsealed = env_fd(UNSEALED);
    let position = file_position(sealed);
    let read = |fd, maximum| {
        // SAFETY: this process has just exec'd, runs only its main thread, and
        // holds no Rust owner for any inherited fixture. The read only borrows.
        unsafe { read_post_exec_sealed_descriptor(fd, maximum) }
    };
    assert_eq!(read(sealed, SEALED_BODY.len()).unwrap(), SEALED_BODY);
    assert_eq!(file_position(sealed), position);
    assert!(matches!(
        read(sealed, SEALED_BODY.len() - 1),
        Err(InheritedFdError::InvalidSealedDescriptor { fd }) if fd == sealed
    ));
    assert!(matches!(
        read(unsealed, 1024),
        Err(InheritedFdError::InvalidSealedDescriptor { fd }) if fd == unsealed
    ));
    // An ordinary file is unsealable or lacks the required seals.
    assert!(matches!(
        read(fixtures[0], 1024),
        Err(InheritedFdError::InvalidSealedDescriptor { .. })
    ));
    assert!(matches!(
        read(i32::MAX, 1024),
        Err(InheritedFdError::MissingDescriptor(i32::MAX))
    ));
    let local = File::open("/dev/null").unwrap();
    assert!(matches!(
        read(local.as_raw_fd(), 1024),
        Err(InheritedFdError::DescriptorWasNotInherited(fd)) if fd == local.as_raw_fd()
    ));

    let (release, parked) = mpsc::channel::<()>();
    let helper = thread::spawn(move || parked.recv());
    let result = read(sealed, SEALED_BODY.len());
    release.send(()).unwrap();
    helper.join().unwrap().unwrap();
    assert!(matches!(result, Err(InheritedFdError::MultiThreaded(2))));

    assert_untouched(fixtures);
    assert_untouched(&[sealed, unsealed]);
}

fn replacement_generation(stage: &str) {
    let expected = env::var(EXPECTED)
        .unwrap()
        .split(',')
        .map(|fd| fd.parse::<i32>().unwrap())
        .collect::<Vec<_>>();
    let unknown = env::var(UNKNOWN).unwrap().parse::<i32>().unwrap();
    let fixtures = expected
        .iter()
        .copied()
        .chain([unknown])
        .collect::<Vec<_>>();
    match stage {
        "success" => {
            // SAFETY: this process has just exec'd with the expected and unknown
            // fixture descriptors, runs only its main thread, and has created
            // no Rust owner for any non-standard inheritable descriptor.
            let claimed = unsafe { claim_post_exec_inherited(&expected) }.unwrap();
            assert!(claimed.matches_raw_fds(&expected));
            assert!(claimed.closed_unexpected() >= 1);
            assert!(!is_open(unknown));
            // Standard descriptors are never claimed or closed.
            for fd in 0..3 {
                assert!(is_open(fd), "standard descriptor {fd} was closed");
            }
            assert_eq!(inheritable_descriptors(), Vec::<i32>::new());
            let mut owners = claimed.into_descriptors();
            for (owner, expected_fd) in owners.iter().zip(&expected) {
                assert_eq!(owner.as_raw_fd(), *expected_fd);
                assert_eq!(fcntl_getfd(owner).unwrap(), FdFlags::CLOEXEC);
            }
            let retained = owners.remove(0);
            drop(owners);
            assert!(is_open(retained.as_raw_fd()));
            for fd in &expected[1..] {
                assert!(!is_open(*fd));
            }
        }
        "missing" => {
            let mut missing = expected.clone();
            missing[0] = i32::MAX;
            // SAFETY: every non-standard inheritable descriptor is an ownerless
            // inherited fixture and only the main thread runs. The absent
            // number is a supported mutation-free rejection.
            let error = unsafe { claim_post_exec_inherited(&missing) }.unwrap_err();
            assert!(matches!(
                error,
                InheritedFdError::MissingDescriptor(i32::MAX)
            ));
            assert_untouched(&fixtures);
        }
        "not-inherited" => {
            let local = File::open("/dev/null").unwrap();
            let mut with_local = expected.clone();
            with_local.push(local.as_raw_fd());
            // SAFETY: as above; the descriptor this generation opened itself is
            // close-on-exec and owned, and is rejected before any claim.
            let error = unsafe { claim_post_exec_inherited(&with_local) }.unwrap_err();
            assert!(matches!(
                error,
                InheritedFdError::DescriptorWasNotInherited(fd) if fd == local.as_raw_fd()
            ));
            assert_untouched(&fixtures);
            assert!(is_close_on_exec(local.as_raw_fd()));
        }
        "multi-threaded" => {
            let (release, parked) = mpsc::channel::<()>();
            let helper = thread::spawn(move || parked.recv());
            // SAFETY: the claim rejects the second thread before it inspects
            // or mutates the descriptor table.
            let result = unsafe { claim_post_exec_inherited(&expected) };
            release.send(()).unwrap();
            helper.join().unwrap().unwrap();
            assert!(matches!(result, Err(InheritedFdError::MultiThreaded(2))));
            assert_untouched(&fixtures);
        }
        "sealed-read" => sealed_read_generation(&fixtures),
        stage => panic!("unexpected stage {stage}"),
    }
}

/// Every stage inherits the sealed-read fixtures too; claiming stages close
/// them as unexpected descriptors.
fn run_replacement(stage: &str, expected: &[File], unknown: &File, sealed: [&File; 2]) {
    let expected = expected
        .iter()
        .map(|file| file.as_raw_fd().to_string())
        .collect::<Vec<_>>()
        .join(",");
    let status = Command::new(env::current_exe().unwrap())
        .env(STAGE, stage)
        .env(EXPECTED, expected)
        .env(UNKNOWN, unknown.as_raw_fd().to_string())
        .env(SEALED, sealed[0].as_raw_fd().to_string())
        .env(UNSEALED, sealed[1].as_raw_fd().to_string())
        .status()
        .unwrap();
    assert!(
        status.success(),
        "replacement stage {stage} failed: {status}"
    );
}

fn main() {
    if let Some(stage) = env::var_os(STAGE) {
        replacement_generation(stage.to_str().unwrap());
        return;
    }

    let directory = TestDirectory::new();
    let expected = (0..3)
        .map(|index| fixture(&directory.0.join(format!("expected-{index}"))))
        .collect::<Vec<_>>();
    let unknown = fixture(&directory.0.join("unknown"));
    let sealed = sealed_memfd(SEALED_BODY, true);
    let unsealed = sealed_memfd(SEALED_BODY, false);
    let inherited = || expected.iter().chain([&unknown, &sealed, &unsealed]);
    let originals = inherited()
        .map(|file| fcntl_getfd(file).unwrap())
        .collect::<Vec<_>>();
    for file in inherited() {
        let flags = fcntl_getfd(file).unwrap();
        fcntl_setfd(file, flags - FdFlags::CLOEXEC).unwrap();
    }

    for stage in STAGES {
        run_replacement(stage, &expected, &unknown, [&sealed, &unsealed]);
        println!("test post_exec_claim::{stage} ... ok");
    }

    for (file, flags) in inherited().zip(originals) {
        fcntl_setfd(file, flags).unwrap();
    }
    println!("\ntest result: ok. {} passed; 0 failed", STAGES.len());
}
