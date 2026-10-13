//! Process-wide descriptor audit coverage.
//!
//! The audit inspects every descriptor in the process, so this file holds
//! exactly one test: a sibling test thread that briefly makes a descriptor
//! inheritable would be reported instead of the one under test.

use std::{
    env,
    fs::File,
    os::fd::{AsFd, AsRawFd},
    process::Command,
};

use rustix::io::{FdFlags, fcntl_getfd, fcntl_setfd};
use splinterd::handoff_descriptors::{
    DescriptorHandoffError, HandoffDescriptor, HandoffDescriptorSlot, PreparedDescriptorInheritance,
};

const CLEAN_STAGE: &str = "SPLINTERD_HANDOFF_AUDIT_CLEAN_STAGE";
const TEST_NAME: &str = "unexpected_inheritable_descriptor_blocks_preparation";

fn null_descriptor() -> File {
    File::open("/dev/null").expect("open null descriptor")
}

/// Re-runs this test without descriptors inherited from the invoking harness.
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
        .expect("launch clean descriptor audit generation");
    assert!(status.success());
}

#[test]
fn unexpected_inheritable_descriptor_blocks_preparation() {
    if env::var_os(CLEAN_STAGE).is_none() {
        clean_test_generation();
        return;
    }

    let allowed = null_descriptor();
    let unexpected = null_descriptor();
    PreparedDescriptorInheritance::prepare([HandoffDescriptor::new(
        HandoffDescriptorSlot::Listener,
        allowed.as_fd(),
    )])
    .expect("the clean generation starts without inheritable descriptors")
    .restore()
    .expect("restore the allowlisted descriptor");

    let unexpected_flags = fcntl_getfd(&unexpected).unwrap();
    fcntl_setfd(&unexpected, unexpected_flags - FdFlags::CLOEXEC).unwrap();

    let result = PreparedDescriptorInheritance::prepare([HandoffDescriptor::new(
        HandoffDescriptorSlot::Listener,
        allowed.as_fd(),
    )]);

    assert!(matches!(
        result,
        Err(DescriptorHandoffError::UnexpectedInheritableDescriptor(fd))
            if fd == unexpected.as_raw_fd()
    ));
    assert!(fcntl_getfd(&allowed).unwrap().contains(FdFlags::CLOEXEC));
    fcntl_setfd(&unexpected, unexpected_flags).unwrap();
}
