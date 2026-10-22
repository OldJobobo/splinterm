//! Reading and atomically replacing one MCP host configuration file.
//!
//! The file must be a regular file with one link, owned by this user, and not
//! writable by anyone else; setup never follows a symbolic link to it, so a
//! file managed elsewhere (a dotfile link, a Nix store path) is left alone.
//! Its directory may be reached through links, as `$HOME` often is, but must
//! belong to this user and not be world-writable.
//!
//! A write goes to a new sibling file with the old file's mode, is synced, and
//! is renamed over the target only if the target still has the identity, size,
//! and modification time it had when it was read; the directory is then
//! synced. An interrupted write leaves the old file intact.

use std::{
    fs::{self, File},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use rustix::fs::{Mode, OFlags};

use super::{FailureKind, failure};

/// Larger files are refused; Claude Code's own file stays far below this.
const MAX_CONFIG_BYTES: u64 = 64 * 1024 * 1024;

/// A host configuration file as it was read.
pub(super) struct Existing {
    pub(super) text: String,
    dev: u64,
    ino: u64,
    size: u64,
    mtime: (i64, i64),
    mode: u32,
}

fn uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

fn invalid(path: &Path, problem: &str) -> anyhow::Error {
    failure(
        FailureKind::Invalid,
        format!("{} {problem}; nothing was written", path.display()),
    )
}

/// Checks the directory that holds the configuration file. `Ok(false)` means
/// it does not exist yet.
pub(super) fn check_directory(directory: &Path) -> Result<bool> {
    let metadata = match fs::metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("inspect {}", directory.display()));
        }
    };
    if !metadata.is_dir() {
        return Err(invalid(directory, "is not a directory"));
    }
    if metadata.uid() != uid() {
        return Err(invalid(directory, "belongs to another user"));
    }
    if metadata.mode() & 0o002 != 0 {
        return Err(invalid(directory, "is writable by every user"));
    }
    Ok(true)
}

/// Reads the configuration file, or `None` when it does not exist.
pub(super) fn read(path: &Path) -> Result<Option<Existing>> {
    let directory = path
        .parent()
        .context("configuration path has no directory")?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            check_directory(directory)?;
            return Ok(None);
        }
        Err(error) => return Err(error).with_context(|| format!("inspect {}", path.display())),
    };
    if metadata.file_type().is_symlink() {
        return Err(invalid(
            path,
            "is a symbolic link, and setup does not edit through links. Edit the file it points to by hand, or pass --config with that file's own path",
        ));
    }
    if !metadata.file_type().is_file() {
        return Err(invalid(path, "is not a regular file"));
    }
    if metadata.nlink() != 1 {
        return Err(invalid(path, "has more than one hard link"));
    }
    if metadata.uid() != uid() {
        return Err(invalid(path, "belongs to another user"));
    }
    if metadata.mode() & 0o022 != 0 {
        return Err(invalid(
            path,
            "is writable by other users; fix its permissions first",
        ));
    }
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(invalid(path, "is larger than 64 MiB"));
    }
    check_directory(directory)?;
    let descriptor = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NOCTTY,
        Mode::empty(),
    )
    .with_context(|| format!("open {}", path.display()))?;
    let mut file = File::from(descriptor);
    let opened = file.metadata()?;
    if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
        return Err(changed(path));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {}", path.display()))?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(invalid(path, "is larger than 64 MiB"));
    }
    let text = String::from_utf8(bytes).map_err(|_| invalid(path, "is not UTF-8 text"))?;
    Ok(Some(Existing {
        text,
        dev: opened.dev(),
        ino: opened.ino(),
        size: opened.len(),
        mtime: (opened.mtime(), opened.mtime_nsec()),
        mode: opened.mode() & 0o7777,
    }))
}

fn changed(path: &Path) -> anyhow::Error {
    failure(
        FailureKind::Changed,
        format!(
            "{} changed while setup was editing it; nothing was written. Run the command again",
            path.display()
        ),
    )
}

fn still_as_read(path: &Path, existing: &Existing) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.file_type().is_file()
            && metadata.dev() == existing.dev
            && metadata.ino() == existing.ino
            && metadata.len() == existing.size
            && (metadata.mtime(), metadata.mtime_nsec()) == existing.mtime
    })
}

/// Atomically replaces (or creates) `path` with `contents`. `create_directory`
/// allows creating a missing directory, mode `0700`, one level deep.
pub(super) fn write(
    path: &Path,
    contents: &str,
    existing: Option<&Existing>,
    create_directory: bool,
) -> Result<()> {
    let directory = path
        .parent()
        .context("configuration path has no directory")?;
    if !check_directory(directory)? {
        if !create_directory {
            return Err(failure(
                FailureKind::NotFound,
                format!(
                    "directory {} does not exist; nothing was written",
                    directory.display()
                ),
            ));
        }
        fs::DirBuilder::new()
            .mode(0o700)
            .create(directory)
            .with_context(|| format!("create {}", directory.display()))?;
        check_directory(directory)?;
    }
    let name = path
        .file_name()
        .context("configuration path has no file name")?
        .to_string_lossy();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    let temporary = directory.join(format!(
        ".{name}.splinterm-{}-{nonce}.tmp",
        std::process::id()
    ));
    let mode = existing.map_or(0o600, |existing| existing.mode);
    let result = (|| {
        let descriptor = rustix::fs::open(
            &temporary,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(mode),
        )
        .with_context(|| format!("create {}", temporary.display()))?;
        let mut file = File::from(descriptor);
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        drop(file);
        match existing {
            Some(existing) => {
                if !still_as_read(path, existing) {
                    return Err(changed(path));
                }
                fs::rename(&temporary, path)
                    .with_context(|| format!("replace {}", path.display()))?;
            }
            None => match rustix::fs::renameat_with(
                rustix::fs::CWD,
                &temporary,
                rustix::fs::CWD,
                path,
                rustix::fs::RenameFlags::NOREPLACE,
            ) {
                Ok(()) => {}
                Err(rustix::io::Errno::EXIST) => return Err(changed(path)),
                Err(error) => {
                    return Err(error).with_context(|| format!("create {}", path.display()));
                }
            },
        }
        File::open(directory)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
