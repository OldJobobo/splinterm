//! Race-resistant snapshots of a local peer's executable.

use std::{
    fs::{self, File, Metadata},
    io::Read,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use rustix::fs::{Mode, OFlags};
use sha2::{Digest, Sha256};

const MAX_EXECUTABLE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_EXECUTABLE_PATH_BYTES: usize = 4096;
const HASH_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutableIdentity {
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
    pub owner_uid: u32,
    pub mode: u32,
    pub size: u64,
    pub sha256: String,
}

impl ExecutableIdentity {
    pub fn from_pid(pid: u32) -> Result<Self> {
        snapshot_link(Path::new(&format!("/proc/{pid}/exe")))
            .with_context(|| format!("cannot snapshot peer executable for pid {pid}"))
    }
}

fn snapshot_link(link: &Path) -> Result<ExecutableIdentity> {
    snapshot_link_with(link, || Ok(()))
}

fn snapshot_link_with(
    link: &Path,
    after_open: impl FnOnce() -> Result<()>,
) -> Result<ExecutableIdentity> {
    let target_before = fs::read_link(link).context("cannot resolve executable link")?;
    validate_canonical_path(&target_before)?;

    let descriptor = rustix::fs::open(link, OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())
        .context("cannot open executable link")?;
    let mut file = File::from(descriptor);
    let metadata_before = file.metadata().context("cannot stat opened executable")?;
    validate_metadata(&metadata_before)?;

    after_open()?;
    let sha256 = hash_bounded(&mut file, metadata_before.len())?;
    let metadata_after = file.metadata().context("cannot restat opened executable")?;
    if metadata_fingerprint(&metadata_before) != metadata_fingerprint(&metadata_after) {
        bail!("executable changed while hashing");
    }

    let target_after = fs::read_link(link).context("cannot re-resolve executable link")?;
    if target_after != target_before {
        bail!("executable link changed while hashing");
    }
    let path_metadata =
        fs::metadata(&target_after).context("executable path no longer resolves")?;
    if path_metadata.dev() != metadata_after.dev() || path_metadata.ino() != metadata_after.ino() {
        bail!("executable path no longer names the opened file");
    }

    Ok(ExecutableIdentity {
        path: target_after,
        device: metadata_after.dev(),
        inode: metadata_after.ino(),
        owner_uid: metadata_after.uid(),
        mode: metadata_after.mode(),
        size: metadata_after.len(),
        sha256,
    })
}

fn validate_canonical_path(path: &Path) -> Result<()> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_EXECUTABLE_PATH_BYTES || !path.is_absolute() {
        bail!("executable path is not a bounded absolute path");
    }
    if bytes.ends_with(b" (deleted)") {
        bail!("executable has been unlinked");
    }
    if path
        .components()
        .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        bail!("executable path is not canonical");
    }
    Ok(())
}

fn validate_metadata(metadata: &Metadata) -> Result<()> {
    if !metadata.is_file() {
        bail!("executable is not a regular file");
    }
    if metadata.len() == 0 || metadata.len() > MAX_EXECUTABLE_BYTES {
        bail!("executable size is outside the supported bound");
    }
    Ok(())
}

fn hash_bounded(file: &mut File, expected_size: u64) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES].into_boxed_slice();
    let mut total = 0_u64;
    loop {
        let count = file.read(&mut buffer).context("cannot read executable")?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(u64::try_from(count).expect("buffer length fits u64"))
            .context("executable byte count overflow")?;
        if total > MAX_EXECUTABLE_BYTES {
            bail!("executable exceeded the supported bound while hashing");
        }
        hasher.update(&buffer[..count]);
    }
    if total != expected_size {
        bail!("executable size changed while hashing");
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn metadata_fingerprint(metadata: &Metadata) -> (u64, u64, u64, u32, u32, i64, i64, i64, i64) {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.uid(),
        metadata.mode(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, OpenOptions},
        io::Write,
        os::unix::fs::symlink,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos();
            let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "splinterm-executable-identity-{}-{unique}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("create test directory");
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn snapshots_current_process_through_proc() {
        let identity = ExecutableIdentity::from_pid(std::process::id())
            .expect("snapshot current process executable");

        assert!(identity.path.is_absolute());
        assert!(identity.size > 0);
        assert_eq!(identity.sha256.len(), 64);
    }

    #[test]
    fn snapshots_exact_opened_regular_file() {
        let directory = TestDirectory::new();
        let executable = directory.0.join("client");
        let link = directory.0.join("exe");
        fs::write(&executable, b"abc").expect("write executable");
        symlink(&executable, &link).expect("create executable link");

        let identity = snapshot_link(&link).expect("snapshot executable");

        assert_eq!(identity.path, executable);
        assert_eq!(identity.size, 3);
        assert_eq!(
            identity.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn rejects_path_replacement_after_descriptor_open() {
        let directory = TestDirectory::new();
        let executable = directory.0.join("client");
        let displaced = directory.0.join("client.old");
        let link = directory.0.join("exe");
        fs::write(&executable, b"authorized").expect("write executable");
        symlink(&executable, &link).expect("create executable link");

        let result = snapshot_link_with(&link, || {
            fs::rename(&executable, &displaced).context("displace executable")?;
            fs::write(&executable, b"replacement").context("write replacement")?;
            Ok(())
        });

        assert!(result.is_err());
    }

    #[test]
    fn rejects_in_place_mutation_after_descriptor_open() {
        let directory = TestDirectory::new();
        let executable = directory.0.join("client");
        let link = directory.0.join("exe");
        fs::write(&executable, b"authorized").expect("write executable");
        symlink(&executable, &link).expect("create executable link");

        let result = snapshot_link_with(&link, || {
            let mut file = OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&executable)
                .context("open executable for mutation")?;
            file.write_all(b"changed").context("mutate executable")?;
            Ok(())
        });

        assert!(result.is_err());
    }

    #[test]
    fn rejects_relative_non_regular_and_oversized_targets() {
        let directory = TestDirectory::new();
        let relative_link = directory.0.join("relative");
        symlink("client", &relative_link).expect("create relative link");
        assert!(snapshot_link(&relative_link).is_err());

        let directory_link = directory.0.join("directory");
        symlink(&directory.0, &directory_link).expect("create directory link");
        assert!(snapshot_link(&directory_link).is_err());

        let oversized = directory.0.join("oversized");
        let oversized_link = directory.0.join("oversized-link");
        let file = File::create(&oversized).expect("create sparse file");
        file.set_len(MAX_EXECUTABLE_BYTES + 1)
            .expect("size sparse file");
        symlink(&oversized, &oversized_link).expect("create oversized link");
        assert!(snapshot_link(&oversized_link).is_err());
    }
}
