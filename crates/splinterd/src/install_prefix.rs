//! Installed-prefix authority: the one directory the daemon trusts to hold its
//! installed `splinterd`, `splinterm`, and helper executables.
//!
//! Nothing after startup re-resolves the running executable's path. Once a
//! daemon or client runs from a sealed memfd (Plan 0061 blocker 3),
//! `current_exe()` and `/proc/<pid>/exe` name `/memfd:… (deleted)`, not a file,
//! and after a package upgrade they name a replaced inode. Every consumer that
//! used to derive a path from the running image (the PTY helper, the consent
//! launcher, trusted-client matching, package provenance, and the daemon's
//! audit identity) uses this prefix instead.
//!
//! A fresh daemon computes the prefix exactly once, from its canonical running
//! path, while that path still names the running image
//! ([`InstallPrefix::of_running_daemon`]). A daemon adopted from a handoff
//! (M3.8/M3.10) will instead take it from the authenticated manifest's
//! `install_prefix`, bound to the sealed executable pair's recorded source
//! paths; that startup source does not exist yet.

use std::{
    fs, io,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Component, Path, PathBuf},
};

use thiserror::Error;

use crate::executable_snapshot::SealedExecutablePair;

/// The installed daemon's file name.
pub const DAEMON_EXECUTABLE: &str = "splinterd";
/// The installed trusted client's file name.
pub const CLIENT_EXECUTABLE: &str = "splinterm";
/// The installed graphical relay's file name.
pub const GRAPHICAL_RELAY_EXECUTABLE: &str = "splinterm-relay";
/// The kernel's `PATH_MAX`, which counts the terminating NUL.
const PATH_MAX: usize = 4096;
/// The longest prefix: `<prefix>/<longest executable>` plus its NUL still fits
/// `PATH_MAX`.
pub const MAX_INSTALL_PREFIX_BYTES: usize =
    PATH_MAX - 1 - 1 - splinterm_pty::HELPER_EXECUTABLE.len();
const _: () =
    assert!(MAX_INSTALL_PREFIX_BYTES + 1 + splinterm_pty::HELPER_EXECUTABLE.len() < PATH_MAX);
const _: () = assert!(splinterm_pty::HELPER_EXECUTABLE.len() >= GRAPHICAL_RELAY_EXECUTABLE.len());
const _: () = assert!(splinterm_pty::HELPER_EXECUTABLE.len() >= DAEMON_EXECUTABLE.len());
const _: () = assert!(splinterm_pty::HELPER_EXECUTABLE.len() >= CLIENT_EXECUTABLE.len());

/// The running image, reached through the kernel's executable link rather than
/// through the path text it would print.
const RUNNING_EXECUTABLE: &str = "/proc/self/exe";

#[derive(Debug, Error)]
pub enum InstallPrefixError {
    #[error("cannot identify the running daemon executable: {0}")]
    RunningExecutable(#[source] io::Error),
    #[error("the running daemon is not an installed splinterd executable")]
    NotDaemon,
    #[error("installed prefix is not a bounded, canonical, absolute directory path")]
    InvalidPath,
    #[error("cannot resolve the installed prefix: {0}")]
    Resolve(#[source] io::Error),
    #[error("installed prefix is not a canonical directory")]
    NotCanonicalDirectory,
    #[error("installed splinterd is not the running daemon image")]
    NotRunningImage,
}

/// A validated installed prefix.
///
/// Invariants: the directory path is absolute, at most
/// [`MAX_INSTALL_PREFIX_BYTES`], contains no NUL byte, is in canonical form
/// (no `.`, `..`, repeated, or trailing separators), is not `/`, and resolved
/// to itself as an existing directory when constructed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallPrefix {
    directory: PathBuf,
}

impl InstallPrefix {
    /// Records the prefix of the daemon running this process, at fresh startup.
    ///
    /// The canonical running path must be a file named `splinterd`, and the
    /// `splinterd` inside its directory must be the running image itself
    /// (same device and inode as the kernel's executable link), so the prefix
    /// provably names this daemon's installation at the moment it is recorded.
    /// A deleted, replaced, or memfd-backed running image fails closed: its
    /// link text resolves to no file or to a different one. In development, a
    /// daemon started through an explicit `ld.so` likewise refuses to start
    /// (its running image is the loader), and valgrind or qemu-user may too,
    /// depending on how they present `/proc/self/exe`.
    ///
    /// The other executables are not required to exist here: a missing
    /// `splinterm` or helper fails closed where it is used (no trusted client
    /// matches, consent cannot launch, a spawn fails), exactly as before.
    ///
    /// # Errors
    /// Fails when the running image cannot be identified, is not an installed
    /// `splinterd`, or its directory is not a valid prefix.
    pub fn of_running_daemon() -> Result<Self, InstallPrefixError> {
        let running =
            fs::metadata(RUNNING_EXECUTABLE).map_err(InstallPrefixError::RunningExecutable)?;
        let daemon = std::env::current_exe().map_err(InstallPrefixError::RunningExecutable)?;
        let daemon = fs::canonicalize(daemon).map_err(InstallPrefixError::RunningExecutable)?;
        if daemon.file_name().and_then(|name| name.to_str()) != Some(DAEMON_EXECUTABLE) {
            return Err(InstallPrefixError::NotDaemon);
        }
        let prefix = Self::from_directory(daemon.parent().ok_or(InstallPrefixError::NotDaemon)?)?;
        let installed = fs::metadata(prefix.daemon()).map_err(InstallPrefixError::Resolve)?;
        if !installed.is_file()
            || installed.dev() != running.dev()
            || installed.ino() != running.ino()
        {
            return Err(InstallPrefixError::NotRunningImage);
        }
        Ok(prefix)
    }

    /// Accepts an existing directory already in canonical form.
    ///
    /// This validates the path's form and on-disk canonicality only. It is
    /// not an authority check by itself: only [`Self::of_running_daemon`]
    /// (and, from M3.8/M3.10, the authenticated sealed manifest bound to its
    /// executable pairs) may supply the daemon's prefix.
    ///
    /// # Errors
    /// Rejects an invalid path form, or a path that does not resolve to itself
    /// as an existing directory.
    pub fn from_directory(directory: &Path) -> Result<Self, InstallPrefixError> {
        validate_install_prefix_path(directory)?;
        let resolved = fs::canonicalize(directory).map_err(InstallPrefixError::Resolve)?;
        let metadata = fs::metadata(&resolved).map_err(InstallPrefixError::Resolve)?;
        if resolved != directory || !metadata.is_dir() {
            return Err(InstallPrefixError::NotCanonicalDirectory);
        }
        Ok(Self {
            directory: resolved,
        })
    }

    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// `<prefix>/splinterd`.
    #[must_use]
    pub fn daemon(&self) -> PathBuf {
        self.directory.join(DAEMON_EXECUTABLE)
    }

    /// `<prefix>/splinterm`: the trusted client, and the consent client.
    #[must_use]
    pub fn client(&self) -> PathBuf {
        self.directory.join(CLIENT_EXECUTABLE)
    }

    /// `<prefix>/splinterm-relay`.
    #[must_use]
    pub fn graphical_relay(&self) -> PathBuf {
        self.directory.join(GRAPHICAL_RELAY_EXECUTABLE)
    }

    /// Whether a sealed pair was captured from exactly `<prefix>/splinterd`
    /// and `<prefix>/splinterm`.
    #[must_use]
    pub fn names_pair(&self, pair: &SealedExecutablePair) -> bool {
        names_pair_in(&self.directory, pair)
    }
}

/// Whether both sealed images' recorded source paths are the installed
/// executables inside `directory`.
pub(crate) fn names_pair_in(directory: &Path, pair: &SealedExecutablePair) -> bool {
    pair.daemon().source().path == directory.join(DAEMON_EXECUTABLE)
        && pair.client().source().path == directory.join(CLIENT_EXECUTABLE)
}

/// Validates the form of an installed prefix path without touching the
/// filesystem; a decoded manifest's prefix is checked with this.
///
/// # Errors
/// Rejects an empty, relative, oversized, NUL-bearing, non-canonical, or root
/// path.
pub fn validate_install_prefix_path(path: &Path) -> Result<(), InstallPrefixError> {
    let bytes = path.as_os_str().as_bytes();
    let canonical_form = path
        .components()
        .all(|component| matches!(component, Component::RootDir | Component::Normal(_)))
        // `components` silently drops `.`, repeated, and trailing separators;
        // rejoining exposes them.
        && path.components().collect::<PathBuf>().as_os_str() == path.as_os_str();
    if bytes.is_empty()
        || bytes.len() > MAX_INSTALL_PREFIX_BYTES
        || bytes.contains(&0)
        || !path.is_absolute()
        || !canonical_form
        || path.parent().is_none()
    {
        return Err(InstallPrefixError::InvalidPath);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    #[test]
    fn prefix_paths_must_be_bounded_canonical_absolute_and_not_root() {
        for valid in ["/usr/bin", "/nix/store/abc-splinterm-0.2.0/bin", "/a"] {
            validate_install_prefix_path(Path::new(valid)).unwrap();
        }
        let oversized = format!("/{}", "a".repeat(MAX_INSTALL_PREFIX_BYTES));
        let nul = Path::new(OsStr::from_bytes(b"/usr/b\0in"));
        for invalid in [
            Path::new(""),
            Path::new("/"),
            Path::new("usr/bin"),
            Path::new("/usr/bin/"),
            Path::new("//usr/bin"),
            Path::new("/usr//bin"),
            Path::new("/usr/./bin"),
            Path::new("/usr/../bin"),
            Path::new(&oversized),
            nul,
        ] {
            assert!(
                validate_install_prefix_path(invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
        let longest = format!("/{}", "a".repeat(MAX_INSTALL_PREFIX_BYTES - 1));
        validate_install_prefix_path(Path::new(&longest)).unwrap();
        let helper = Path::new(&longest).join(splinterm_pty::HELPER_EXECUTABLE);
        // The longest derived path plus its NUL is exactly `PATH_MAX`.
        assert_eq!(helper.as_os_str().len(), PATH_MAX - 1);
    }

    #[test]
    fn a_prefix_directory_must_resolve_to_itself() {
        let test_binary = std::env::current_exe().unwrap();
        let directory = fs::canonicalize(test_binary.parent().unwrap()).unwrap();
        let prefix = InstallPrefix::from_directory(&directory).unwrap();
        assert_eq!(prefix.directory(), directory);
        assert_eq!(prefix.daemon(), directory.join("splinterd"));
        assert_eq!(prefix.client(), directory.join("splinterm"));
        assert_eq!(prefix.graphical_relay(), directory.join("splinterm-relay"));

        let scratch = std::env::temp_dir().join(format!(
            "splinterd-install-prefix-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&scratch).unwrap();
        let scratch = fs::canonicalize(&scratch).unwrap();
        let link = scratch.join("link");
        std::os::unix::fs::symlink(&directory, &link).unwrap();
        let file = scratch.join("file");
        fs::write(&file, b"").unwrap();
        let missing = scratch.join("missing");
        for rejected in [&link, &file, &missing] {
            assert!(
                InstallPrefix::from_directory(rejected).is_err(),
                "accepted {rejected:?}"
            );
        }
        fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn a_test_binary_is_not_a_running_installed_daemon() {
        // The unit-test executable is not named splinterd.
        assert!(matches!(
            InstallPrefix::of_running_daemon(),
            Err(InstallPrefixError::NotDaemon)
        ));
    }
}
