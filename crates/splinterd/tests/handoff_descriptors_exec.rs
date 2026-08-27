use std::{
    env,
    fs::{self, File},
    os::fd::{AsFd, AsRawFd},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use rustix::io::{FdFlags, fcntl_getfd};
use splinterd::handoff_descriptors::{
    HandoffDescriptor, HandoffDescriptorSlot, PreparedDescriptorInheritance,
};

const CLEAN_STAGE: &str = "SPLINTERD_HANDOFF_DESCRIPTOR_CLEAN_STAGE";
const STAGE: &str = "SPLINTERD_HANDOFF_DESCRIPTOR_STAGE";
const ALLOWED_FD: &str = "SPLINTERD_HANDOFF_ALLOWED_FD";
const ALLOWED_PATH: &str = "SPLINTERD_HANDOFF_ALLOWED_PATH";
const UNRELATED_PATH: &str = "SPLINTERD_HANDOFF_UNRELATED_PATH";
const TEST_NAME: &str = "only_the_typed_allowlist_survives_exec";
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "splinterd-handoff-descriptors-{}-{unique}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create descriptor test directory");
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn open_fixture(path: &Path) -> File {
    fs::write(path, path.as_os_str().as_encoded_bytes()).expect("write descriptor fixture");
    File::open(path).expect("open descriptor fixture")
}

fn inherited_descriptors() -> Vec<(i32, PathBuf)> {
    let scan_target = PathBuf::from(format!("/proc/{}/fd", std::process::id()));
    let directory = fs::read_dir("/proc/self/fd").expect("read descriptor table");
    let mut inherited = Vec::new();
    let mut scanner_descriptors = 0;
    for entry in directory {
        let entry = entry.expect("read every descriptor table entry");
        let fd = entry
            .file_name()
            .to_str()
            .expect("numeric descriptor entry")
            .parse::<i32>()
            .expect("numeric descriptor entry");
        if fd < 3 {
            continue;
        }
        match fs::read_link(entry.path()) {
            Ok(target) if target == scan_target => scanner_descriptors += 1,
            Ok(target) => inherited.push((fd, target)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("cannot inspect inherited descriptor {fd}: {error}"),
        }
    }
    assert_eq!(
        scanner_descriptors, 1,
        "exclude only the descriptor opened by this exact table scan"
    );
    inherited.sort();
    inherited
}

fn clean_test_generation() {
    let status = Command::new("python3")
        .arg("-c")
        .arg(
            "import subprocess, sys; raise SystemExit(subprocess.run(sys.argv[1:], close_fds=True).returncode)",
        )
        .arg(env::current_exe().expect("current test executable"))
        .arg(TEST_NAME)
        .arg("--exact")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env(CLEAN_STAGE, "clean")
        .status()
        .expect("launch clean descriptor test generation");
    assert!(status.success());
}

fn replacement_generation() {
    let allowed_fd = env::var(ALLOWED_FD)
        .expect("allowed descriptor slot")
        .parse::<i32>()
        .expect("numeric allowed descriptor slot");
    let allowed = PathBuf::from(env::var_os(ALLOWED_PATH).expect("allowed fixture path"));
    let unrelated = PathBuf::from(env::var_os(UNRELATED_PATH).expect("unrelated fixture path"));
    let inherited = inherited_descriptors();
    assert_eq!(inherited, vec![(allowed_fd, allowed)]);
    assert!(!inherited.iter().any(|(_, target)| target == &unrelated));
}

#[test]
fn only_the_typed_allowlist_survives_exec() {
    if env::var_os(STAGE).is_some() {
        replacement_generation();
        return;
    }
    if env::var_os(CLEAN_STAGE).is_none() {
        clean_test_generation();
        return;
    }

    let directory = TestDirectory::new();
    let allowed_path = directory.0.join("allowed");
    let unrelated_path = directory.0.join("unrelated");
    let allowed = open_fixture(&allowed_path);
    let unrelated = open_fixture(&unrelated_path);
    let original_allowed_flags = fcntl_getfd(&allowed).expect("read allowed flags");
    assert!(original_allowed_flags.contains(FdFlags::CLOEXEC));
    assert!(fcntl_getfd(&unrelated).unwrap().contains(FdFlags::CLOEXEC));

    let prepared = PreparedDescriptorInheritance::prepare([HandoffDescriptor::new(
        HandoffDescriptorSlot::Listener,
        allowed.as_fd(),
    )])
    .expect("prepare typed descriptor inheritance");
    assert!(!fcntl_getfd(&allowed).unwrap().contains(FdFlags::CLOEXEC));
    assert_eq!(prepared.descriptors()[0].raw_fd(), allowed.as_raw_fd());

    let status = Command::new(env::current_exe().expect("current test executable"))
        .arg(TEST_NAME)
        .arg("--exact")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env(STAGE, "replacement")
        .env(ALLOWED_FD, allowed.as_raw_fd().to_string())
        .env(ALLOWED_PATH, &allowed_path)
        .env(UNRELATED_PATH, &unrelated_path)
        .status()
        .expect("execute replacement test generation");
    assert!(status.success());

    prepared.restore().expect("restore descriptor inheritance");
    assert_eq!(fcntl_getfd(&allowed).unwrap(), original_allowed_flags);
}
