//! Durable executable identities for remembered consent grants (ADR 0014).
//!
//! A remembered ("Always for this workspace") grant names an executable by a
//! path that survives package upgrades instead of by a digest. ADR 0014
//! accepts exactly two kinds, and both are checked again, in full, on every
//! new connection; nothing here caches a result:
//!
//! * A **protected install**: every component of an absolute canonical path,
//!   from `/` to the file, opens without following a symbolic link; every
//!   directory and the file are owned by root, are neither group- nor
//!   world-writable, and carry no access ACL granting anyone else write; the
//!   file is a regular executable; and it has the executing file's device and
//!   inode.
//! * A **Nix profile entry**: one of six fixed profile paths, walked one
//!   component at a time (links followed, at most 40, relative targets against
//!   the link's directory, `..` physically), ending at a regular executable
//!   file under a well-formed, root-owned, unwritable `/nix/store` object,
//!   with the executing file's device and inode.
//!
//! This module never grants anything by itself. It also never decides which
//! durations a prompt offers; it only answers whether a path qualifies now.
//! The known-interpreter list is a user-experience rule, not a security
//! boundary: it can only withhold "Always".
//!
//! Every check runs against an [`IdentityRoot`]: `/` with root as the only
//! trusted owner in production. Tests substitute a private tree and their own
//! uid, so no test touches the real `/nix`.

use std::{
    collections::VecDeque,
    ffi::{OsStr, OsString},
    fmt,
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
    path::{Component, Path, PathBuf},
};

use rustix::{
    fs::{FileType, Mode, OFlags, Stat},
    io::Errno,
};
use serde::{Deserialize, Serialize};

/// The most symbolic links one Nix profile walk follows.
pub const MAX_LINKS: usize = 40;
/// The longest path, or link text, a check accepts, in bytes.
pub const MAX_PATH_BYTES: usize = 4096;
const MAX_NAME_BYTES: usize = 255;
const NIX_BASE32: &[u8; 32] = b"0123456789abcdfghijklmnpqrsvwxyz";
const NIX_HASH_CHARS: usize = 32;
const ACL_ACCESS_XATTR: &str = "system.posix_acl_access";
/// Enough for 1023 ACL entries; a larger ACL fails closed.
const ACL_BUFFER_BYTES: usize = 4 + 8 * 1023;

/// Interpreters, by the basename of the executing file with any trailing
/// version suffix of digits and dots removed (ADR 0014 "Known interpreters").
///
/// ADR 0014 names the list's minimum. The dynamic loaders are added because a
/// program started as `ld.so ./program` executes the loader, so remembering
/// the loader would cover every program it loads, exactly as for a script
/// interpreter. The other additions (awk, csh, nu, julia, dotnet) also run
/// whatever script or assembly they are given.
const KNOWN_INTERPRETERS: &[&[u8]] = &[
    b"python",
    b"node",
    b"nodejs",
    b"deno",
    b"bun",
    b"bash",
    b"sh",
    b"dash",
    b"zsh",
    b"fish",
    b"ksh",
    b"mksh",
    b"busybox",
    b"perl",
    b"ruby",
    b"php",
    b"lua",
    b"luajit",
    b"tclsh",
    b"java",
    b"pwsh",
    b"Rscript",
    b"awk",
    b"gawk",
    b"mawk",
    b"tcsh",
    b"csh",
    b"nu",
    b"julia",
    b"dotnet",
    b"ld.so",
    b"ld-linux.so",
    b"ld-linux-x86-64.so",
    b"ld-linux-aarch64.so",
    b"ld-linux-armhf.so",
    b"ld-musl-x86_64.so",
    b"ld-musl-aarch64.so",
];

/// A file object, by device and inode.
pub type FileId = (u64, u64);

/// Where durable identity checks start, and which owners stand for root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentityRoot {
    root: PathBuf,
    trusted_owners: Vec<u32>,
}

impl IdentityRoot {
    /// The production root: `/`, with only root trusted.
    #[must_use]
    pub fn system() -> Self {
        Self {
            root: PathBuf::from("/"),
            trusted_owners: vec![0],
        }
    }

    /// `/`, trusting `owner` beside root. Only a debug-build test seam may use
    /// this, so a test running as an ordinary user can build a protected
    /// tree.
    #[must_use]
    pub fn system_also_trusting(owner: u32) -> Self {
        Self {
            root: PathBuf::from("/"),
            trusted_owners: vec![0, owner],
        }
    }

    /// A private tree standing in for `/`, for tests.
    #[must_use]
    pub fn at(root: PathBuf, trusted_owners: Vec<u32>) -> Self {
        Self {
            root,
            trusted_owners,
        }
    }

    fn trusts(&self, owner: u32) -> bool {
        self.trusted_owners.contains(&owner)
    }

    fn open(&self) -> Result<OwnedFd, IdentityRefusal> {
        rustix::fs::open(
            &self.root,
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(open_refusal)
    }
}

/// A durable identity a remembered grant may name.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "path", rename_all = "snake_case")]
pub enum DurableIdentity {
    /// An absolute canonical path whose every component is protected.
    ProtectedInstall(PathBuf),
    /// One of the six fixed Nix profile entries.
    NixProfile(PathBuf),
}

impl DurableIdentity {
    /// The stored path.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::ProtectedInstall(path) | Self::NixProfile(path) => path,
        }
    }
}

/// Why a path is not a durable identity for the executing file now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentityRefusal {
    /// Not a bounded, absolute, canonical, UTF-8 path, or not a single name.
    InvalidPath,
    /// A component does not exist.
    Missing,
    /// A component could not be opened or examined.
    Unreadable,
    /// A protected-install component is a symbolic link.
    SymbolicLink,
    /// A component that must be a directory is not one.
    NotDirectory,
    /// A component is not owned by root.
    UntrustedOwner,
    /// A component is group- or world-writable.
    Writable,
    /// A component's access ACL grants write to someone other than its owner.
    WriteAcl,
    /// The file is not a regular executable file.
    NotExecutable,
    /// The file is not the executing file (device or inode differ).
    DifferentFile,
    /// The walk followed more than [`MAX_LINKS`] links.
    TooManyLinks,
    /// A path formed during the walk exceeded [`MAX_PATH_BYTES`].
    PathTooLong,
    /// A link's text exceeded [`MAX_PATH_BYTES`].
    LinkTextTooLong,
    /// The walk did not end inside a `/nix/store` object.
    OutsideStore,
    /// The store object's name is not `HASH-NAME`.
    MalformedStoreName,
    /// The path is not one of the six fixed Nix profile entries.
    NotProfileEntry,
}

impl fmt::Display for IdentityRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidPath => "path is not a bounded absolute canonical path",
            Self::Missing => "a path component does not exist",
            Self::Unreadable => "a path component cannot be examined",
            Self::SymbolicLink => "a path component is a symbolic link",
            Self::NotDirectory => "a path component is not a directory",
            Self::UntrustedOwner => "a path component is not owned by root",
            Self::Writable => "a path component is group- or world-writable",
            Self::WriteAcl => "a path component's ACL grants write to another user",
            Self::NotExecutable => "the path does not name a regular executable file",
            Self::DifferentFile => "the path does not name the executing file",
            Self::TooManyLinks => "the path follows too many symbolic links",
            Self::PathTooLong => "the path grows beyond its bound while resolving",
            Self::LinkTextTooLong => "a symbolic link's text exceeds its bound",
            Self::OutsideStore => "the path does not resolve into a Nix store object",
            Self::MalformedStoreName => "the Nix store object name is malformed",
            Self::NotProfileEntry => "the path is not a supported Nix profile entry",
        })
    }
}

impl std::error::Error for IdentityRefusal {}

/// Whether `path`, in the identity root, is a protected install of the
/// executing file `executing`.
///
/// # Errors
/// The first rule `path` breaks, as an [`IdentityRefusal`].
pub fn check_protected_install(
    root: &IdentityRoot,
    path: &Path,
    executing: FileId,
) -> Result<(), IdentityRefusal> {
    let components = canonical_components(path)?;
    let (file_name, directories) = components
        .split_last()
        .ok_or(IdentityRefusal::InvalidPath)?;
    let mut directory = root.open()?;
    require_protected(root, &directory, &stat(&directory)?)?;
    for name in directories {
        let next = open_component(&directory, name)?;
        let status = stat(&next)?;
        match FileType::from_raw_mode(status.st_mode) {
            FileType::Directory => {}
            FileType::Symlink => return Err(IdentityRefusal::SymbolicLink),
            _ => return Err(IdentityRefusal::NotDirectory),
        }
        require_protected(root, &next, &status)?;
        directory = next;
    }
    let file = open_component(&directory, file_name)?;
    let status = stat(&file)?;
    match FileType::from_raw_mode(status.st_mode) {
        FileType::Symlink => return Err(IdentityRefusal::SymbolicLink),
        FileType::RegularFile if status.st_mode & 0o111 != 0 => {}
        _ => return Err(IdentityRefusal::NotExecutable),
    }
    require_protected(root, &file, &status)?;
    if file_id(&status) != executing {
        return Err(IdentityRefusal::DifferentFile);
    }
    Ok(())
}

/// The six fixed Nix profile entries for `name`, in priority order (system
/// profiles first). `home` and `user` must come from the password database
/// entry for the daemon's uid, never from the environment.
///
/// `None` when `name` or `user` is not a single path component, or `home` is
/// not an absolute canonical path.
#[must_use]
pub fn nix_profile_entries(home: &Path, user: &OsStr, name: &OsStr) -> Option<[PathBuf; 6]> {
    if !single_component(name) || !single_component(user) || canonical_components(home).is_err() {
        return None;
    }
    let user = Path::new(user);
    Some([
        Path::new("/run/current-system/sw/bin").join(name),
        Path::new("/etc/profiles/per-user")
            .join(user)
            .join("bin")
            .join(name),
        Path::new("/nix/var/nix/profiles/default/bin").join(name),
        Path::new("/nix/var/nix/profiles/per-user")
            .join(user)
            .join("profile/bin")
            .join(name),
        home.join(".nix-profile/bin").join(name),
        home.join(".local/state/nix/profile/bin").join(name),
    ])
}

/// Whether `path` is exactly one of the six fixed Nix profile entries for
/// this account.
#[must_use]
pub fn is_nix_profile_entry(path: &Path, home: &Path, user: &OsStr) -> bool {
    path.file_name()
        .and_then(|name| nix_profile_entries(home, user, name))
        .is_some_and(|entries| entries.iter().any(|entry| entry == path))
}

/// The profile entries whose name equals the executing file's name and that
/// resolve, now, to the executing file, in priority order. The name only
/// selects which entries are tested; the walk and the device and inode check
/// decide the match.
#[must_use]
pub fn matching_nix_profile_entries(
    root: &IdentityRoot,
    home: &Path,
    user: &OsStr,
    name: &OsStr,
    executing: FileId,
) -> Vec<PathBuf> {
    nix_profile_entries(home, user, name)
        .map(|entries| {
            entries
                .into_iter()
                .filter(|entry| resolve_nix_profile_entry(root, entry, executing).is_ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Walks the profile entry `entry` in the identity root and requires it to
/// end at the executing file inside a protected Nix store object.
///
/// Returns the physical path of the resolved file.
///
/// # Errors
/// The first rule the walk or its result breaks, as an [`IdentityRefusal`].
pub fn resolve_nix_profile_entry(
    root: &IdentityRoot,
    entry: &Path,
    executing: FileId,
) -> Result<PathBuf, IdentityRefusal> {
    let bytes = entry.as_os_str().as_bytes();
    if !entry.is_absolute() || bytes.len() > MAX_PATH_BYTES || entry.to_str().is_none() {
        return Err(IdentityRefusal::InvalidPath);
    }
    let root_directory = root.open()?;
    let (physical, status) = walk(&root_directory, bytes)?;
    if FileType::from_raw_mode(status.st_mode) != FileType::RegularFile
        || status.st_mode & 0o111 == 0
    {
        return Err(IdentityRefusal::NotExecutable);
    }
    let names: Vec<&OsStr> = physical.iter().map(|(name, _)| name.as_os_str()).collect();
    if names.len() < 4 || names[0] != OsStr::new("nix") || names[1] != OsStr::new("store") {
        return Err(IdentityRefusal::OutsideStore);
    }
    if !valid_store_object_name(names[2].as_bytes()) {
        return Err(IdentityRefusal::MalformedStoreName);
    }
    // `/` and `/nix`: root-owned and unwritable.
    require_unwritable_owner(root, &stat(&root_directory)?)?;
    require_unwritable_owner(root, &stat(&physical[0].1)?)?;
    // `/nix/store`: root-owned; the usual sticky group-writable store
    // directory is accepted, a world-writable one is not.
    let store = stat(&physical[1].1)?;
    if !root.trusts(store.st_uid) {
        return Err(IdentityRefusal::UntrustedOwner);
    }
    if store.st_mode & 0o002 != 0 || (store.st_mode & 0o020 != 0 && store.st_mode & 0o1000 == 0) {
        return Err(IdentityRefusal::Writable);
    }
    // The store object and everything beneath it on the physical path.
    for (_, descriptor) in &physical[2..] {
        require_unwritable_owner(root, &stat(descriptor)?)?;
    }
    if file_id(&status) != executing {
        return Err(IdentityRefusal::DifferentFile);
    }
    let mut resolved = PathBuf::from("/");
    for name in names {
        resolved.push(name);
    }
    Ok(resolved)
}

/// One walk step's physical path so far: each component below the root with
/// its open descriptor.
type PhysicalPath = Vec<(OsString, OwnedFd)>;

/// Walks `path` from `root_directory`, one component at a time, following
/// links as ADR 0014 specifies, and returns the physical path of the file it
/// ends at with that file's status.
fn walk(root_directory: &OwnedFd, path: &[u8]) -> Result<(PhysicalPath, Stat), IdentityRefusal> {
    let mut physical: PhysicalPath = Vec::new();
    let mut pending = split_components(path);
    let mut links = 0_usize;
    let mut file: Option<Stat> = None;
    while let Some(name) = pending.pop_front() {
        if file.is_some() {
            return Err(IdentityRefusal::NotDirectory);
        }
        if name == "." {
            continue;
        }
        if name == ".." {
            // Physically: the actual parent of the directory reached so far.
            physical.pop();
            continue;
        }
        let parent = physical.last().map_or(root_directory, |(_, fd)| fd);
        let descriptor = open_component(parent, &name)?;
        let status = stat(&descriptor)?;
        match FileType::from_raw_mode(status.st_mode) {
            FileType::Symlink => {
                links += 1;
                if links > MAX_LINKS {
                    return Err(IdentityRefusal::TooManyLinks);
                }
                let target = rustix::fs::readlinkat(parent, name.as_os_str(), Vec::new())
                    .map_err(open_refusal)?;
                let target = target.as_bytes();
                check_link_text(target)?;
                if target.first() == Some(&b'/') {
                    physical.clear();
                }
                let mut next = split_components(target);
                next.extend(pending.drain(..));
                pending = next;
                if joined_length(pending.iter()) > MAX_PATH_BYTES {
                    return Err(IdentityRefusal::PathTooLong);
                }
            }
            kind => {
                physical.push((name, descriptor));
                if joined_length(physical.iter().map(|(name, _)| name)) > MAX_PATH_BYTES {
                    return Err(IdentityRefusal::PathTooLong);
                }
                if kind != FileType::Directory {
                    file = Some(status);
                }
            }
        }
    }
    file.map(|status| (physical, status))
        .ok_or(IdentityRefusal::NotExecutable)
}

fn check_link_text(target: &[u8]) -> Result<(), IdentityRefusal> {
    if target.is_empty() {
        return Err(IdentityRefusal::Missing);
    }
    if target.len() > MAX_PATH_BYTES {
        return Err(IdentityRefusal::LinkTextTooLong);
    }
    Ok(())
}

fn split_components(path: &[u8]) -> VecDeque<OsString> {
    path.split(|byte| *byte == b'/')
        .filter(|component| !component.is_empty())
        .map(|component| OsStr::from_bytes(component).to_os_string())
        .collect()
}

/// The length of `/a/b/c` for these components.
fn joined_length<'a>(components: impl Iterator<Item = &'a OsString>) -> usize {
    components.map(|component| component.len() + 1).sum()
}

/// Whether `name` is `HASH-NAME`: 32 characters of the Nix base-32 alphabet,
/// `-`, then a store name of letters, digits, and `+-._?=` that does not start
/// with `.`.
#[must_use]
pub fn valid_store_object_name(name: &[u8]) -> bool {
    let Some((hash, rest)) = name.split_at_checked(NIX_HASH_CHARS) else {
        return false;
    };
    let Some((b'-', store_name)) = rest.split_first() else {
        return false;
    };
    hash.iter().all(|byte| NIX_BASE32.contains(byte))
        && !store_name.is_empty()
        && store_name[0] != b'.'
        && store_name
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"+-._?=".contains(byte))
}

/// Whether the executing file's basename names a known interpreter, ignoring
/// a trailing version suffix of digits and dots (`python3.12`, `perl5.38.2`).
///
/// A user-experience rule, not a security boundary: a renamed or copied
/// interpreter evades it, and it can only withhold "Always".
#[must_use]
pub fn is_known_interpreter(basename: &OsStr) -> bool {
    let bytes = basename.as_bytes();
    let stem_length = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_digit() && *byte != b'.')
        .map_or(0, |index| index + 1);
    let stem = &bytes[..stem_length];
    !stem.is_empty() && KNOWN_INTERPRETERS.contains(&stem)
}

fn single_component(name: &OsStr) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_NAME_BYTES
        && bytes != b"."
        && bytes != b".."
        && !bytes.contains(&b'/')
        && !bytes.contains(&0)
        && name.to_str().is_some()
}

/// The normal components of a bounded, absolute, canonical, UTF-8 path.
fn canonical_components(path: &Path) -> Result<Vec<&OsStr>, IdentityRefusal> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty()
        || bytes.len() > MAX_PATH_BYTES
        || !path.is_absolute()
        || path.to_str().is_none()
        || bytes.ends_with(b"/")
        // `Path::components` drops `.` and empty components, so the text
        // itself must hold none of them.
        || bytes[1..]
            .split(|byte| *byte == b'/')
            .any(|component| component.is_empty() || component == b"." || component == b"..")
    {
        return Err(IdentityRefusal::InvalidPath);
    }
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => components.push(name),
            _ => return Err(IdentityRefusal::InvalidPath),
        }
    }
    if components.is_empty() {
        return Err(IdentityRefusal::InvalidPath);
    }
    Ok(components)
}

fn open_component(directory: &OwnedFd, name: &OsStr) -> Result<OwnedFd, IdentityRefusal> {
    rustix::fs::openat(
        directory,
        name,
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(open_refusal)
}

fn open_refusal(error: Errno) -> IdentityRefusal {
    if error == Errno::NOENT {
        IdentityRefusal::Missing
    } else if error == Errno::NOTDIR {
        IdentityRefusal::NotDirectory
    } else if error == Errno::NAMETOOLONG {
        IdentityRefusal::PathTooLong
    } else {
        IdentityRefusal::Unreadable
    }
}

fn stat(descriptor: &OwnedFd) -> Result<Stat, IdentityRefusal> {
    rustix::fs::fstat(descriptor).map_err(|_| IdentityRefusal::Unreadable)
}

#[allow(
    clippy::useless_conversion,
    reason = "the stat field widths differ between architectures"
)]
fn file_id(status: &Stat) -> FileId {
    (u64::from(status.st_dev), u64::from(status.st_ino))
}

fn require_unwritable_owner(root: &IdentityRoot, status: &Stat) -> Result<(), IdentityRefusal> {
    if !root.trusts(status.st_uid) {
        return Err(IdentityRefusal::UntrustedOwner);
    }
    if status.st_mode & 0o022 != 0 {
        return Err(IdentityRefusal::Writable);
    }
    Ok(())
}

fn require_protected(
    root: &IdentityRoot,
    descriptor: &OwnedFd,
    status: &Stat,
) -> Result<(), IdentityRefusal> {
    require_unwritable_owner(root, status)?;
    if access_acl_grants_foreign_write(descriptor, status.st_uid)? {
        return Err(IdentityRefusal::WriteAcl);
    }
    Ok(())
}

/// Reads the component's access ACL through its descriptor's procfs link
/// (an `O_PATH` descriptor cannot carry `fgetxattr`). No ACL, or a file system
/// without ACL support, grants nothing; any other failure fails closed.
fn access_acl_grants_foreign_write(
    descriptor: &OwnedFd,
    owner: u32,
) -> Result<bool, IdentityRefusal> {
    let mut buffer = vec![0_u8; ACL_BUFFER_BYTES];
    let link = format!("/proc/self/fd/{}", descriptor.as_raw_fd());
    match rustix::fs::getxattr(link.as_str(), ACL_ACCESS_XATTR, &mut buffer[..]) {
        Ok(length) => Ok(acl_grants_foreign_write(&buffer[..length], owner).unwrap_or(true)),
        Err(error) if error == Errno::NODATA || error == Errno::NOTSUP => Ok(false),
        Err(_) => Err(IdentityRefusal::Unreadable),
    }
}

const ACL_VERSION: u32 = 2;
const ACL_USER_OBJ: u16 = 0x01;
const ACL_USER: u16 = 0x02;
const ACL_GROUP_OBJ: u16 = 0x04;
const ACL_GROUP: u16 = 0x08;
const ACL_MASK: u16 = 0x10;
const ACL_OTHER: u16 = 0x20;
const ACL_WRITE: u16 = 0x02;

/// Whether the encoded POSIX access ACL lets anyone but `owner` write,
/// honoring the mask for named users and groups. `None` when the encoding is
/// malformed (the caller fails closed).
fn acl_grants_foreign_write(encoded: &[u8], owner: u32) -> Option<bool> {
    let (header, entries) = encoded.split_first_chunk::<4>()?;
    if u32::from_le_bytes(*header) != ACL_VERSION || entries.len() % 8 != 0 {
        return None;
    }
    let entries: Vec<(u16, u16, u32)> = entries
        .chunks_exact(8)
        .map(|entry| {
            (
                u16::from_le_bytes([entry[0], entry[1]]),
                u16::from_le_bytes([entry[2], entry[3]]),
                u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]),
            )
        })
        .collect();
    let mask = entries
        .iter()
        .find(|(tag, _, _)| *tag == ACL_MASK)
        .map_or(u16::MAX, |(_, permissions, _)| *permissions);
    for (tag, permissions, id) in &entries {
        let writes = match *tag {
            ACL_USER_OBJ | ACL_MASK => false,
            ACL_USER => *id != owner && permissions & mask & ACL_WRITE != 0,
            ACL_GROUP_OBJ | ACL_GROUP => permissions & mask & ACL_WRITE != 0,
            ACL_OTHER => permissions & ACL_WRITE != 0,
            _ => return None,
        };
        if writes {
            return Some(true);
        }
    }
    Some(false)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::{MetadataExt, PermissionsExt, symlink},
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static NEXT_TREE: AtomicU64 = AtomicU64::new(0);

    const HASH: &str = "0123456789abcdfghijklmnpqrsvwxyz";
    const OTHER_HASH: &str = "zyxwvsrqpnmlkjihgfdcba9876543210";

    /// A private tree standing in for `/`, removed on drop.
    struct Tree {
        root: PathBuf,
    }

    impl Tree {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "splinterm-durable-identity-{}-{}",
                std::process::id(),
                NEXT_TREE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
            Self { root }
        }

        fn identity_root(&self) -> IdentityRoot {
            IdentityRoot::at(self.root.clone(), vec![own_uid()])
        }

        fn path(&self, path: &str) -> PathBuf {
            self.root.join(path.trim_start_matches('/'))
        }

        fn directory(&self, path: &str) -> PathBuf {
            let path = self.path(path);
            fs::create_dir_all(&path).unwrap();
            let mut ancestor = path.as_path();
            while ancestor != self.root {
                fs::set_permissions(ancestor, fs::Permissions::from_mode(0o755)).unwrap();
                ancestor = ancestor.parent().unwrap();
            }
            path
        }

        fn executable(&self, path: &str) -> FileId {
            let path = self.path(path);
            let parent = path.parent().unwrap().strip_prefix(&self.root).unwrap();
            self.directory(&format!("/{}", parent.display()));
            fs::write(&path, format!("#!/bin/false {}\n", path.display())).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            id_of(&path)
        }

        fn link(&self, path: &str, target: &str) {
            let path = self.path(path);
            let parent = path.parent().unwrap().strip_prefix(&self.root).unwrap();
            self.directory(&format!("/{}", parent.display()));
            symlink(target, path).unwrap();
        }

        fn chmod(&self, path: &str, mode: u32) {
            fs::set_permissions(self.path(path), fs::Permissions::from_mode(mode)).unwrap();
        }

        /// A multi-user store: `/nix` 0755, `/nix/store` sticky 1775.
        fn store(&self) {
            self.directory("/nix/store");
            self.chmod("/nix/store", 0o1775);
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            // Restore directory permissions a test narrowed, then remove.
            // Links are never followed: some point at absolute paths.
            for entry in walkdir(&self.root) {
                if fs::symlink_metadata(&entry).is_ok_and(|metadata| metadata.is_dir()) {
                    let _ = fs::set_permissions(&entry, fs::Permissions::from_mode(0o755));
                }
            }
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn walkdir(root: &Path) -> Vec<PathBuf> {
        let mut found = vec![root.to_path_buf()];
        let mut index = 0;
        while index < found.len() {
            let current = found[index].clone();
            index += 1;
            if fs::symlink_metadata(&current).is_ok_and(|metadata| metadata.is_dir()) {
                for entry in fs::read_dir(&current).into_iter().flatten().flatten() {
                    found.push(entry.path());
                }
            }
        }
        found
    }

    fn own_uid() -> u32 {
        rustix::process::geteuid().as_raw()
    }

    fn id_of(path: &Path) -> FileId {
        let metadata = fs::metadata(path).unwrap();
        (metadata.dev(), metadata.ino())
    }

    // ---- Protected install ----

    #[test]
    fn a_protected_install_matches_the_executing_file() {
        let tree = Tree::new();
        let executing = tree.executable("/usr/bin/agent");
        assert_eq!(
            check_protected_install(
                &tree.identity_root(),
                Path::new("/usr/bin/agent"),
                executing
            ),
            Ok(())
        );
    }

    #[test]
    fn a_writable_directory_or_file_is_not_protected() {
        for (path, mode) in [
            ("/usr/bin", 0o775),
            ("/usr/bin", 0o757),
            ("/usr", 0o775),
            ("/", 0o777),
            ("/usr/bin/agent", 0o775),
            ("/usr/bin/agent", 0o757),
        ] {
            let tree = Tree::new();
            let executing = tree.executable("/usr/bin/agent");
            tree.chmod(path, mode);
            assert_eq!(
                check_protected_install(
                    &tree.identity_root(),
                    Path::new("/usr/bin/agent"),
                    executing
                ),
                Err(IdentityRefusal::Writable),
                "{path} {mode:o}"
            );
        }
    }

    #[test]
    fn a_symbolic_link_anywhere_on_the_path_is_not_protected() {
        let tree = Tree::new();
        let executing = tree.executable("/usr/bin/agent");
        tree.link("/bin", "usr/bin");
        tree.link("/usr/bin/alias", "agent");
        let root = tree.identity_root();
        // A directory component that is a link, even to the right place.
        assert_eq!(
            check_protected_install(&root, Path::new("/bin/agent"), executing),
            Err(IdentityRefusal::SymbolicLink)
        );
        // The final component.
        assert_eq!(
            check_protected_install(&root, Path::new("/usr/bin/alias"), executing),
            Err(IdentityRefusal::SymbolicLink)
        );
    }

    #[test]
    fn a_component_not_owned_by_a_trusted_owner_is_not_protected() {
        if own_uid() == 0 {
            return;
        }
        let tree = Tree::new();
        let executing = tree.executable("/usr/bin/agent");
        let root_only = IdentityRoot::at(tree.root.clone(), vec![0]);
        assert_eq!(
            check_protected_install(&root_only, Path::new("/usr/bin/agent"), executing),
            Err(IdentityRefusal::UntrustedOwner)
        );
    }

    #[test]
    fn another_or_replaced_file_does_not_match() {
        let tree = Tree::new();
        let executing = tree.executable("/usr/bin/agent");
        let other = tree.executable("/usr/bin/other");
        let root = tree.identity_root();
        assert_eq!(
            check_protected_install(&root, Path::new("/usr/bin/agent"), other),
            Err(IdentityRefusal::DifferentFile)
        );
        // An upgrade replaces the file by rename: a running process keeps the
        // old inode and no longer matches; a new process would.
        let staged = tree.path("/usr/bin/agent.new");
        fs::write(&staged, b"#!/bin/false new\n").unwrap();
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755)).unwrap();
        fs::rename(&staged, tree.path("/usr/bin/agent")).unwrap();
        assert_eq!(
            check_protected_install(&root, Path::new("/usr/bin/agent"), executing),
            Err(IdentityRefusal::DifferentFile)
        );
        let upgraded = id_of(&tree.path("/usr/bin/agent"));
        assert_eq!(
            check_protected_install(&root, Path::new("/usr/bin/agent"), upgraded),
            Ok(())
        );
        // A deleted file matches nothing.
        fs::remove_file(tree.path("/usr/bin/agent")).unwrap();
        assert_eq!(
            check_protected_install(&root, Path::new("/usr/bin/agent"), upgraded),
            Err(IdentityRefusal::Missing)
        );
    }

    #[test]
    fn only_a_regular_executable_file_is_a_protected_install() {
        let tree = Tree::new();
        let executing = tree.executable("/usr/bin/agent");
        let root = tree.identity_root();
        tree.chmod("/usr/bin/agent", 0o644);
        assert_eq!(
            check_protected_install(&root, Path::new("/usr/bin/agent"), executing),
            Err(IdentityRefusal::NotExecutable)
        );
        assert_eq!(
            check_protected_install(&root, Path::new("/usr/bin"), executing),
            Err(IdentityRefusal::NotExecutable)
        );
        assert_eq!(
            check_protected_install(&root, Path::new("/usr/bin/agent/more"), executing),
            Err(IdentityRefusal::NotDirectory)
        );
    }

    #[test]
    fn a_protected_install_path_must_be_canonical_and_bounded() {
        let tree = Tree::new();
        let executing = tree.executable("/usr/bin/agent");
        let root = tree.identity_root();
        for path in [
            "usr/bin/agent",
            "/usr/../usr/bin/agent",
            "/usr/./bin/agent",
            "/usr//bin/agent",
            "/usr/bin/agent/",
            "/",
            "",
        ] {
            assert_eq!(
                check_protected_install(&root, Path::new(path), executing),
                Err(IdentityRefusal::InvalidPath),
                "{path:?}"
            );
        }
        let long = format!("/{}", "a/".repeat(MAX_PATH_BYTES / 2));
        assert_eq!(
            check_protected_install(&root, Path::new(&long), executing),
            Err(IdentityRefusal::InvalidPath)
        );
    }

    #[test]
    fn a_write_acl_is_not_protected() {
        let tree = Tree::new();
        let executing = tree.executable("/usr/bin/agent");
        // user::rwx, user:65534:rw-, group::r-x, mask::rwx, other::r-x
        let mut acl = ACL_VERSION.to_le_bytes().to_vec();
        for (tag, permissions, id) in [
            (ACL_USER_OBJ, 7_u16, u32::MAX),
            (ACL_USER, 6, 65_534),
            (ACL_GROUP_OBJ, 5, u32::MAX),
            (ACL_MASK, 7, u32::MAX),
            (ACL_OTHER, 5, u32::MAX),
        ] {
            acl.extend(tag.to_le_bytes());
            acl.extend(permissions.to_le_bytes());
            acl.extend(id.to_le_bytes());
        }
        let file = tree.path("/usr/bin/agent");
        match rustix::fs::setxattr(
            &file,
            ACL_ACCESS_XATTR,
            &acl,
            rustix::fs::XattrFlags::empty(),
        ) {
            Ok(()) => {}
            // No ACL support on this file system: the encoded rule is still
            // proven below.
            Err(error) if error == Errno::NOTSUP || error == Errno::PERM => return,
            Err(error) => panic!("set ACL: {error}"),
        }
        // A named user's effective write also shows as the mask in the group
        // mode bits, so either rule refuses it.
        let refused = check_protected_install(
            &tree.identity_root(),
            Path::new("/usr/bin/agent"),
            executing,
        );
        assert!(
            matches!(
                refused,
                Err(IdentityRefusal::WriteAcl | IdentityRefusal::Writable)
            ),
            "{refused:?}"
        );
    }

    fn encoded_acl(entries: &[(u16, u16, u32)]) -> Vec<u8> {
        let mut acl = ACL_VERSION.to_le_bytes().to_vec();
        for (tag, permissions, id) in entries {
            acl.extend(tag.to_le_bytes());
            acl.extend(permissions.to_le_bytes());
            acl.extend(id.to_le_bytes());
        }
        acl
    }

    #[test]
    fn access_acls_grant_foreign_write_only_through_unmasked_entries() {
        let base = [
            (ACL_USER_OBJ, 7, u32::MAX),
            (ACL_GROUP_OBJ, 5, u32::MAX),
            (ACL_OTHER, 5, u32::MAX),
        ];
        assert_eq!(
            acl_grants_foreign_write(&encoded_acl(&base), 0),
            Some(false)
        );
        // A named user with write, masked to read-only: no effective write.
        let masked = [
            (ACL_USER_OBJ, 7, u32::MAX),
            (ACL_USER, 7, 1000),
            (ACL_GROUP_OBJ, 5, u32::MAX),
            (ACL_MASK, 5, u32::MAX),
            (ACL_OTHER, 5, u32::MAX),
        ];
        assert_eq!(
            acl_grants_foreign_write(&encoded_acl(&masked), 0),
            Some(false)
        );
        // Unmasked named user, named group, owning group, and other.
        for writer in [
            (ACL_USER, 2, 1000),
            (ACL_GROUP, 2, 1000),
            (ACL_GROUP_OBJ, 7, u32::MAX),
            (ACL_OTHER, 2, u32::MAX),
        ] {
            let mut entries = base.to_vec();
            entries.push(writer);
            assert_eq!(
                acl_grants_foreign_write(&encoded_acl(&entries), 0),
                Some(true),
                "{writer:?}"
            );
        }
        // The owner naming itself is not someone else.
        let mut entries = base.to_vec();
        entries.push((ACL_USER, 7, 0));
        assert_eq!(
            acl_grants_foreign_write(&encoded_acl(&entries), 0),
            Some(false)
        );
        // Malformed encodings fail closed.
        assert_eq!(acl_grants_foreign_write(&[], 0), None);
        assert_eq!(acl_grants_foreign_write(&[1, 0, 0, 0], 0), None);
        let mut truncated = encoded_acl(&base);
        truncated.pop();
        assert_eq!(acl_grants_foreign_write(&truncated, 0), None);
        assert_eq!(
            acl_grants_foreign_write(&encoded_acl(&[(0x40, 7, 0)]), 0),
            None
        );
    }

    // ---- Nix profile ----

    /// A store object with `bin/agent`, a user environment linking to it, and
    /// the per-user profile generation chain a Nix profile uses.
    fn nix_profile(tree: &Tree) -> FileId {
        tree.store();
        let executing = tree.executable(&format!("/nix/store/{HASH}-agent-1.0/bin/agent"));
        tree.link(
            &format!("/nix/store/{OTHER_HASH}-user-environment/bin/agent"),
            &format!("/nix/store/{HASH}-agent-1.0/bin/agent"),
        );
        tree.link(
            "/nix/var/nix/profiles/per-user/u/profile-3-link",
            &format!("/nix/store/{OTHER_HASH}-user-environment"),
        );
        // Relative: against the link's own directory.
        tree.link("/nix/var/nix/profiles/per-user/u/profile", "profile-3-link");
        tree.link(
            "/home/u/.nix-profile",
            "/nix/var/nix/profiles/per-user/u/profile",
        );
        executing
    }

    #[test]
    fn a_nix_profile_entry_resolves_link_by_link_into_the_store() {
        let tree = Tree::new();
        let executing = nix_profile(&tree);
        let root = tree.identity_root();
        assert_eq!(
            resolve_nix_profile_entry(
                &root,
                Path::new("/home/u/.nix-profile/bin/agent"),
                executing
            ),
            Ok(PathBuf::from(format!(
                "/nix/store/{HASH}-agent-1.0/bin/agent"
            )))
        );
        assert_eq!(
            matching_nix_profile_entries(
                &root,
                Path::new("/home/u"),
                OsStr::new("u"),
                OsStr::new("agent"),
                executing
            ),
            // The per-user profile the home link points at is itself entry 4.
            vec![
                PathBuf::from("/nix/var/nix/profiles/per-user/u/profile/bin/agent"),
                PathBuf::from("/home/u/.nix-profile/bin/agent"),
            ]
        );
    }

    #[test]
    fn dot_dot_resolves_physically_against_the_directory_reached() {
        let tree = Tree::new();
        tree.store();
        let real = tree.executable(&format!("/nix/store/{HASH}-real/libexec/agent"));
        let decoy = tree.executable(&format!("/nix/store/{OTHER_HASH}-env/libexec/agent"));
        tree.directory(&format!("/nix/store/{HASH}-real/bin"));
        // env/bin is a link to real/bin; real/bin/agent is `../libexec/agent`.
        tree.link(
            &format!("/nix/store/{OTHER_HASH}-env/bin"),
            &format!("/nix/store/{HASH}-real/bin"),
        );
        tree.link(
            &format!("/nix/store/{HASH}-real/bin/agent"),
            "../libexec/agent",
        );
        tree.link(
            "/home/u/.nix-profile",
            &format!("/nix/store/{OTHER_HASH}-env"),
        );
        let root = tree.identity_root();
        let entry = Path::new("/home/u/.nix-profile/bin/agent");
        // Textually `env/bin/../libexec/agent` would be the decoy; physically
        // `..` is the parent of `real/bin`.
        assert_eq!(
            resolve_nix_profile_entry(&root, entry, real),
            Ok(PathBuf::from(format!(
                "/nix/store/{HASH}-real/libexec/agent"
            )))
        );
        assert_eq!(
            resolve_nix_profile_entry(&root, entry, decoy),
            Err(IdentityRefusal::DifferentFile)
        );
        // `..` never climbs above the root.
        tree.link("/home/u/up", "../../../../../nix");
        assert_eq!(
            resolve_nix_profile_entry(
                &root,
                Path::new(&format!("/home/u/up/store/{HASH}-real/libexec/agent")),
                real
            ),
            Ok(PathBuf::from(format!(
                "/nix/store/{HASH}-real/libexec/agent"
            )))
        );
    }

    #[test]
    fn a_walk_follows_at_most_forty_links() {
        for (links, expected_ok) in [(MAX_LINKS, true), (MAX_LINKS + 1, false)] {
            let tree = Tree::new();
            tree.store();
            let executing = tree.executable(&format!("/nix/store/{HASH}-agent/bin/agent"));
            // A chain of exactly `links` links: hop-1 .. hop-`links`, the
            // last pointing into the store.
            tree.link(
                &format!("/home/u/hops/hop-{links}"),
                &format!("/nix/store/{HASH}-agent/bin/agent"),
            );
            for hop in (1..links).rev() {
                tree.link(
                    &format!("/home/u/hops/hop-{hop}"),
                    &format!("hop-{}", hop + 1),
                );
            }
            let result = resolve_nix_profile_entry(
                &tree.identity_root(),
                Path::new("/home/u/hops/hop-1"),
                executing,
            );
            if expected_ok {
                assert!(result.is_ok(), "{links}: {result:?}");
            } else {
                assert_eq!(result, Err(IdentityRefusal::TooManyLinks), "{links}");
            }
        }
    }

    #[test]
    fn walk_paths_and_link_text_are_bounded() {
        let tree = Tree::new();
        tree.store();
        let executing = tree.executable(&format!("/nix/store/{HASH}-agent/bin/agent"));
        // A legal link whose text, joined with the remaining components,
        // exceeds the path bound while resolving.
        let padding = "./".repeat((MAX_PATH_BYTES - 8) / 2);
        tree.link("/home/u/long", &format!("{padding}x"));
        assert_eq!(
            resolve_nix_profile_entry(
                &tree.identity_root(),
                Path::new("/home/u/long/bin/agent"),
                executing
            ),
            Err(IdentityRefusal::PathTooLong)
        );
        // The kernel stores at most 4095 bytes of link text, so the text
        // bound itself is checked directly.
        assert_eq!(check_link_text(&[b'a'; MAX_PATH_BYTES]), Ok(()));
        assert_eq!(
            check_link_text(&[b'a'; MAX_PATH_BYTES + 1]),
            Err(IdentityRefusal::LinkTextTooLong)
        );
        assert_eq!(check_link_text(b""), Err(IdentityRefusal::Missing));
        // An entry longer than the bound is refused before any walk.
        let long = format!("/{}", "a".repeat(MAX_PATH_BYTES));
        assert_eq!(
            resolve_nix_profile_entry(&tree.identity_root(), Path::new(&long), executing),
            Err(IdentityRefusal::InvalidPath)
        );
    }

    #[test]
    fn a_target_outside_the_store_or_with_a_malformed_name_is_refused() {
        let tree = Tree::new();
        tree.store();
        let outside = tree.executable("/opt/agent/bin/agent");
        tree.link("/home/u/.nix-profile", "/opt/agent");
        let root = tree.identity_root();
        let entry = Path::new("/home/u/.nix-profile/bin/agent");
        assert_eq!(
            resolve_nix_profile_entry(&root, entry, outside),
            Err(IdentityRefusal::OutsideStore)
        );
        for name in [
            "agent-1.0".to_owned(),
            format!("{}-agent", &HASH[..31]),
            format!("{}e-agent", &HASH[..31]),
            format!("{HASH}-.hidden"),
            format!("{HASH}_agent"),
        ] {
            let tree = Tree::new();
            tree.store();
            let executing = tree.executable(&format!("/nix/store/{name}/bin/agent"));
            tree.link("/home/u/.nix-profile", &format!("/nix/store/{name}"));
            assert_eq!(
                resolve_nix_profile_entry(&tree.identity_root(), entry, executing),
                Err(IdentityRefusal::MalformedStoreName),
                "{name}"
            );
        }
        // A single-file store object is not the documented `HASH-NAME/...`.
        let tree = Tree::new();
        tree.store();
        let single = tree.executable(&format!("/nix/store/{HASH}-script"));
        tree.link(
            "/home/u/.nix-profile/bin/agent",
            &format!("/nix/store/{HASH}-script"),
        );
        assert_eq!(
            resolve_nix_profile_entry(&tree.identity_root(), entry, single),
            Err(IdentityRefusal::OutsideStore)
        );
    }

    #[test]
    fn store_object_names_follow_the_nix_grammar() {
        assert!(valid_store_object_name(
            format!("{HASH}-agent-1.0").as_bytes()
        ));
        assert!(valid_store_object_name(
            format!("{HASH}-a+b-c._?=d").as_bytes()
        ));
        for invalid in [
            format!("{HASH}-"),
            format!("{HASH}agent"),
            format!("{HASH}-.agent"),
            format!("{HASH}-agent/x"),
            format!("{HASH}-agent space"),
            format!("{}-agent", HASH.to_uppercase()),
            format!("{}t-agent", &HASH[..31]),
            String::new(),
        ] {
            assert!(!valid_store_object_name(invalid.as_bytes()), "{invalid:?}");
        }
    }

    #[test]
    fn root_nix_and_store_ownership_and_modes_are_enforced() {
        for (path, mode, refusal) in [
            ("/", 0o775, IdentityRefusal::Writable),
            ("/nix", 0o775, IdentityRefusal::Writable),
            ("/nix/store", 0o1777, IdentityRefusal::Writable),
            ("/nix/store", 0o775, IdentityRefusal::Writable),
            (
                &format!("/nix/store/{HASH}-agent-1.0") as &str,
                0o775,
                IdentityRefusal::Writable,
            ),
            (
                &format!("/nix/store/{HASH}-agent-1.0/bin/agent") as &str,
                0o757,
                IdentityRefusal::Writable,
            ),
        ] {
            let tree = Tree::new();
            let executing = nix_profile(&tree);
            tree.chmod(path, mode);
            assert_eq!(
                resolve_nix_profile_entry(
                    &tree.identity_root(),
                    Path::new("/home/u/.nix-profile/bin/agent"),
                    executing
                ),
                Err(refusal),
                "{path} {mode:o}"
            );
        }
        // The usual sticky group-writable store, and a plain 0755 one, pass.
        for mode in [0o1775, 0o755, 0o1755] {
            let tree = Tree::new();
            let executing = nix_profile(&tree);
            tree.chmod("/nix/store", mode);
            assert!(
                resolve_nix_profile_entry(
                    &tree.identity_root(),
                    Path::new("/home/u/.nix-profile/bin/agent"),
                    executing
                )
                .is_ok(),
                "{mode:o}"
            );
        }
        // User-owned intermediate links and directories are accepted; a
        // store not owned by root is not.
        if own_uid() != 0 {
            let tree = Tree::new();
            let executing = nix_profile(&tree);
            assert_eq!(
                resolve_nix_profile_entry(
                    &IdentityRoot::at(tree.root.clone(), vec![0]),
                    Path::new("/home/u/.nix-profile/bin/agent"),
                    executing
                ),
                Err(IdentityRefusal::UntrustedOwner)
            );
        }
    }

    #[test]
    fn a_wrapper_or_another_file_does_not_match() {
        let tree = Tree::new();
        tree.store();
        // The profile names a wrapper script; the process runs the wrapped
        // binary, a different file.
        tree.executable(&format!("/nix/store/{HASH}-agent/bin/agent"));
        let wrapped = tree.executable(&format!("/nix/store/{HASH}-agent/bin/.agent-wrapped"));
        tree.link("/home/u/.nix-profile", &format!("/nix/store/{HASH}-agent"));
        let root = tree.identity_root();
        let entry = Path::new("/home/u/.nix-profile/bin/agent");
        assert_eq!(
            resolve_nix_profile_entry(&root, entry, wrapped),
            Err(IdentityRefusal::DifferentFile)
        );
        // Selecting the profile entries named after the wrapper finds none
        // that resolve to the wrapped binary.
        assert!(
            matching_nix_profile_entries(
                &root,
                Path::new("/home/u"),
                OsStr::new("u"),
                OsStr::new("agent"),
                wrapped
            )
            .is_empty()
        );
    }

    #[test]
    fn earlier_profile_entries_take_priority_and_a_repointed_profile_follows() {
        let tree = Tree::new();
        let executing = nix_profile(&tree);
        // The system profile also provides the same file.
        tree.link(
            "/run/current-system/sw/bin/agent",
            &format!("/nix/store/{HASH}-agent-1.0/bin/agent"),
        );
        let root = tree.identity_root();
        let home = Path::new("/home/u");
        let user = OsStr::new("u");
        let name = OsStr::new("agent");
        assert_eq!(
            matching_nix_profile_entries(&root, home, user, name, executing),
            vec![
                PathBuf::from("/run/current-system/sw/bin/agent"),
                PathBuf::from("/nix/var/nix/profiles/per-user/u/profile/bin/agent"),
                PathBuf::from("/home/u/.nix-profile/bin/agent"),
            ]
        );
        // Repoint the user profile at a new generation with a new build.
        let upgraded = tree.executable(&format!("/nix/store/{OTHER_HASH}-agent-2.0/bin/agent"));
        tree.link(
            &format!("/nix/store/{HASH}-user-environment-2/bin/agent"),
            &format!("/nix/store/{OTHER_HASH}-agent-2.0/bin/agent"),
        );
        tree.link(
            "/nix/var/nix/profiles/per-user/u/profile-4-link",
            &format!("/nix/store/{HASH}-user-environment-2"),
        );
        fs::remove_file(tree.path("/nix/var/nix/profiles/per-user/u/profile")).unwrap();
        tree.link("/nix/var/nix/profiles/per-user/u/profile", "profile-4-link");
        // The old process now matches only the system profile; a process of
        // the new build matches the user profile.
        assert_eq!(
            matching_nix_profile_entries(&root, home, user, name, executing),
            vec![PathBuf::from("/run/current-system/sw/bin/agent")]
        );
        assert_eq!(
            matching_nix_profile_entries(&root, home, user, name, upgraded),
            vec![
                PathBuf::from("/nix/var/nix/profiles/per-user/u/profile/bin/agent"),
                PathBuf::from("/home/u/.nix-profile/bin/agent"),
            ]
        );
    }

    #[test]
    fn only_the_six_fixed_profile_entries_are_nix_identities() {
        let home = Path::new("/home/u");
        let user = OsStr::new("u");
        let entries = nix_profile_entries(home, user, OsStr::new("agent")).unwrap();
        assert_eq!(
            entries.clone().map(|entry| entry.display().to_string()),
            [
                "/run/current-system/sw/bin/agent",
                "/etc/profiles/per-user/u/bin/agent",
                "/nix/var/nix/profiles/default/bin/agent",
                "/nix/var/nix/profiles/per-user/u/profile/bin/agent",
                "/home/u/.nix-profile/bin/agent",
                "/home/u/.local/state/nix/profile/bin/agent",
            ]
        );
        for entry in &entries {
            assert!(
                is_nix_profile_entry(entry, home, user),
                "{}",
                entry.display()
            );
        }
        for outside in [
            "/home/u/bin/agent",
            "/home/v/.nix-profile/bin/agent",
            "/etc/profiles/per-user/v/bin/agent",
            "/nix/store/x/bin/agent",
            "/run/current-system/sw/bin/sub/agent",
            "/usr/bin/agent",
        ] {
            assert!(
                !is_nix_profile_entry(Path::new(outside), home, user),
                "{outside}"
            );
        }
        for name in ["", ".", "..", "a/b"] {
            assert!(
                nix_profile_entries(home, user, OsStr::new(name)).is_none(),
                "{name:?}"
            );
        }
        assert!(nix_profile_entries(Path::new("home/u"), user, OsStr::new("agent")).is_none());
        assert!(nix_profile_entries(home, OsStr::new("a/b"), OsStr::new("agent")).is_none());
    }

    #[test]
    fn known_interpreters_are_named_by_basename_without_a_version_suffix() {
        for interpreter in [
            "python",
            "python3",
            "python3.12",
            "perl5.38.2",
            "node",
            "nodejs",
            "bash",
            "sh",
            "busybox",
            "Rscript",
            "lua5.4",
            "ld-linux-x86-64.so.2",
            "gawk",
            "tcsh",
            "nu",
            "julia1.10",
            "dotnet",
        ] {
            assert!(
                is_known_interpreter(OsStr::new(interpreter)),
                "{interpreter}"
            );
        }
        for compiled in [
            "splinterm-mcp",
            "python-agent",
            "pythonx",
            "rscript",
            "3.12",
            "",
            "claude",
            "codex",
        ] {
            assert!(!is_known_interpreter(OsStr::new(compiled)), "{compiled}");
        }
    }
}
