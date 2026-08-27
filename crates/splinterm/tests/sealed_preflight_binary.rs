use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use splinterd::{
    executable_snapshot::{
        ExecutableSnapshotPolicy, ExecutableSourcePair, HandoffExecutableSnapshots,
        RetainedRollbackExecutables,
    },
    handoff_preflight::{PreflightRole, preflight_sealed_snapshot_for_integration_test},
};

struct Fixture {
    directory: PathBuf,
    snapshots: HandoffExecutableSnapshots,
}

impl Fixture {
    fn new(executable: &Path) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "splinterm-production-preflight-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let forward = pair(&directory, "forward", executable);
        let rollback = pair(&directory, "rollback", executable);
        let policy = ExecutableSnapshotPolicy {
            expected_owner_uid: rustix::process::geteuid().as_raw(),
        };
        let rollback =
            RetainedRollbackExecutables::capture_declared_for_test(&rollback, policy).unwrap();
        let snapshots =
            HandoffExecutableSnapshots::materialize(&forward, rollback, policy).unwrap();
        Self {
            directory,
            snapshots,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn pair(root: &Path, generation: &str, executable: &Path) -> ExecutableSourcePair {
    let directory = root.join(generation);
    fs::create_dir(&directory).unwrap();
    let daemon = directory.join("splinterd");
    let client = directory.join("splinterm");
    for path in [&daemon, &client] {
        fs::copy(executable, path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    ExecutableSourcePair::new(daemon, client).unwrap()
}

#[test]
fn actual_splinterm_entrypoint_runs_sealed_preflight_before_runtime() {
    let executable = Path::new(env!("CARGO_BIN_EXE_splinterm"));
    let fixture = Fixture::new(executable);
    preflight_sealed_snapshot_for_integration_test(
        fixture.snapshots.forward().client(),
        PreflightRole::Client,
        executable,
    )
    .unwrap();
}
