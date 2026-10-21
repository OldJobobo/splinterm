//! The easy-agent-access grant engine, daemon side (ADR 0014, Plan 0053 PR 2).
//!
//! # What this adds
//!
//! Beside today's peer-bound "just this once" Lair grants (in
//! [`crate::consent::GrantStore`]) and the policy file, two more independent
//! authority sources:
//!
//! * **While this workspace is open** ([`GrantDuration::WorkspaceSession`]):
//!   bound to the verified executable identity of ADR 0007 (uid, canonical
//!   path, SHA-256) plus the Lair, held in daemon memory only, so it ends with
//!   the Lair or with this daemon generation.
//! * **Always for this workspace** ([`GrantDuration::Remembered`]): bound to
//!   a durable identity ([`DurableIdentity`]) plus the Lair, kept in
//!   `$XDG_STATE_HOME/splinterm/agent-access.json`, loaded fresh by every
//!   generation, and checked against each new connection's own evidence.
//!
//! A request is authorized when one source covers every scope it needs;
//! scopes are never combined across sources. Operation preconditions,
//! controller ownership, resource checks, and limits stay with the caller.
//!
//! # Reachability
//!
//! The consent flow (`RequestLairAccess`, Plan 0053 PR 3) commits answers
//! through [`crate::commit_agent_grant`] and answers covered requests from
//! [`AgentAccess::authorize`]. The access inventory (`ListAccess`), per-entry
//! revocation (`RevokeAccessEntry`), and **Revoke all access**
//! (`RevokeAllAccess`, Plan 0053 PR 4) read and revoke these definitions.
//!
//! # Locks
//!
//! [`AgentAccess::file`] is the writer lock. It is held across every
//! read-modify-write of the store and the matching memory change, and is
//! always taken before [`AgentAccess::memory`], never while holding it.
//! Authorization only reads memory, never the file, so a slow `fsync` never
//! stalls a request. Both are synchronous locks; store I/O runs on blocking
//! threads.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
    ffi::{OsStr, OsString},
    fmt,
    fs::File,
    io::{Read, Write},
    os::{fd::OwnedFd, unix::ffi::OsStrExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use rustix::{
    fs::{AtFlags, FileType, Mode, OFlags, Stat},
    io::Errno,
};
use serde::{Deserialize, Serialize};
use splinterm_core::{LairId, Topology};
use splinterm_policy::durable_identity::{
    self, DurableIdentity, FileId, IdentityRoot, is_known_interpreter,
};
use splinterm_protocol::AccessScope;

use crate::{consent::PeerIdentity, executable_identity::ExecutableIdentity};

/// The store's file name, inside the session-state directory.
pub const STORE_FILE: &str = "agent-access.json";
const STATE_DIRECTORY: &str = "splinterm";
/// The most remembered entries the store holds.
pub const MAX_REMEMBERED_ENTRIES: usize = 256;
/// The largest store the daemon reads or writes, in bytes.
pub const MAX_STORE_BYTES: u64 = 256 * 1024;
const STORE_FORMAT: &str = "splinterm.agent-access";
/// The format version this daemon writes into a new store.
pub const CURRENT_STORE_VERSION: u32 = 1;
const SUPPORTED_STORE_VERSIONS: &[u32] = &[1];
const MAX_DISPLAY_NAME_BYTES: usize = 256;
const ENTRY_ID_PREFIX: &str = "ra_";
const ENTRY_ID_HEX: usize = 32;
const MAX_AGENT_AUDIT_RECORDS: usize = 256;
const MAX_FAULT_BYTES: usize = 256;
/// Last-used time is written at most this often per entry because of use.
pub const LAST_USED_WRITE_INTERVAL_SECONDS: u64 = 60 * 60;
/// Names a uid that stands in for root in durable identity checks. Read only
/// by debug builds, so an end-to-end test running as an ordinary user can
/// build a protected tree; a release daemon never compiles it.
#[cfg(debug_assertions)]
pub const TEST_TRUSTED_OWNER_ENV: &str = "SPLINTERM_TEST_DURABLE_IDENTITY_OWNER_UID";

// ---------------------------------------------------------------------------
// Access levels
// ---------------------------------------------------------------------------

// The three levels (`AccessLevel`), the durations (`GrantDuration`), and why
// a duration is withheld (`Withheld`) are protocol types: the trusted prompt
// shows them, and the store serializes levels by the same names.
pub use splinterm_protocol::{AccessLevel, DurationWithheld as Withheld, GrantDuration};

/// What a grant allows: exactly one level, or an exact Custom scope set that
/// equals no level. Never rounded up.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AccessSet {
    Level(AccessLevel),
    Custom(BTreeSet<AccessScope>),
}

/// Why a scope set cannot be granted at all.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvalidAccess {
    Empty,
    /// Clipboard scopes stay invalid in a workspace request (never Custom).
    Clipboard,
    TooMany,
}

impl AccessSet {
    /// Classifies requested consent scopes: an exact bundle is that level,
    /// anything else is Custom.
    ///
    /// # Errors
    /// An empty set, a clipboard scope, or more scopes than the protocol
    /// bound.
    pub fn from_scopes(
        scopes: impl IntoIterator<Item = AccessScope>,
    ) -> Result<Self, InvalidAccess> {
        let scopes: BTreeSet<_> = scopes.into_iter().collect();
        if scopes.is_empty() {
            return Err(InvalidAccess::Empty);
        }
        if scopes.len() > splinterm_protocol::MAX_ACCESS_SCOPES {
            return Err(InvalidAccess::TooMany);
        }
        if scopes.contains(&AccessScope::ClipboardRead)
            || scopes.contains(&AccessScope::ClipboardWrite)
        {
            return Err(InvalidAccess::Clipboard);
        }
        Ok(AccessLevel::ALL
            .into_iter()
            .find(|level| level.scopes().iter().copied().collect::<BTreeSet<_>>() == scopes)
            .map_or(Self::Custom(scopes), Self::Level))
    }

    /// Every consent scope this set allows.
    #[must_use]
    pub fn scopes(&self) -> BTreeSet<AccessScope> {
        match self {
            Self::Level(level) => level.scopes().iter().copied().collect(),
            Self::Custom(scopes) => scopes.clone(),
        }
    }

    /// Whether this set alone covers every required consent scope.
    #[must_use]
    pub fn covers(&self, required: &[AccessScope]) -> bool {
        match self {
            Self::Level(level) => required.iter().all(|scope| level.scopes().contains(scope)),
            Self::Custom(scopes) => required.iter().all(|scope| scopes.contains(scope)),
        }
    }

    /// The level this set is exactly, or `None` for Custom.
    #[must_use]
    pub const fn level(&self) -> Option<AccessLevel> {
        match self {
            Self::Level(level) => Some(*level),
            Self::Custom(_) => None,
        }
    }

    /// A Custom set containing takeover is offered just this once only.
    #[must_use]
    pub fn is_custom_takeover(&self) -> bool {
        matches!(self, Self::Custom(scopes) if scopes.contains(&AccessScope::ControlTakeover))
    }
}

// ---------------------------------------------------------------------------
// Durations and which are offered
// ---------------------------------------------------------------------------

/// The durations the daemon offers one requester for one request.
/// "Just this once" is always offered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurationOffer {
    pub workspace_session: Result<(), Withheld>,
    /// The durable identity "Always" would store.
    pub remembered: Result<DurableIdentity, Withheld>,
}

impl DurationOffer {
    /// Whether `duration` is offered.
    ///
    /// # Errors
    /// The reason it is withheld.
    pub fn allows(&self, duration: GrantDuration) -> Result<(), Withheld> {
        match duration {
            GrantDuration::Once => Ok(()),
            GrantDuration::WorkspaceSession => self.workspace_session,
            GrantDuration::Remembered => self.remembered.as_ref().map(|_| ()).map_err(|why| *why),
        }
    }
}

/// The ADR 0014 duration-eligibility table, in its order: no pinned identity,
/// a Custom request with takeover, and the installed relay offer only once;
/// a known interpreter, an executable without a durable identity (a
/// development build or any user-writable file), and an unavailable store
/// withhold only "Always".
#[must_use]
pub fn offered_durations(
    peer: &PeerIdentity,
    access: &AccessSet,
    store_available: bool,
) -> DurationOffer {
    let both = |why| DurationOffer {
        workspace_session: Err(why),
        remembered: Err(why),
    };
    let (Some(_), Some(evidence)) = (peer.persistent_executable(), peer.durable_evidence()) else {
        return both(Withheld::NoPinnedIdentity);
    };
    if access.is_custom_takeover() {
        return both(Withheld::CustomTakeover);
    }
    if evidence.installed_relay {
        return both(Withheld::Relay);
    }
    let remembered = if evidence.interpreter {
        Err(Withheld::Interpreter)
    } else if let Some(identity) = evidence.preferred() {
        if store_available {
            Ok(identity)
        } else {
            Err(Withheld::StoreUnavailable)
        }
    } else {
        Err(Withheld::NoDurableIdentity)
    };
    DurationOffer {
        workspace_session: Ok(()),
        remembered,
    }
}

// ---------------------------------------------------------------------------
// Connection evidence
// ---------------------------------------------------------------------------

/// What one verified connection established about durable identity, read
/// once at connection time while the peer's pidfd pins it, and never reused
/// for another connection.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DurableEvidence {
    /// The executing path, when it is a protected install now.
    protected_install: Option<PathBuf>,
    /// Profile entries named after the executing file that resolve to it,
    /// in priority order.
    nix_profiles: Vec<PathBuf>,
    /// The executing file's basename names a known interpreter.
    interpreter: bool,
    /// The peer executes the installed relay (by device and inode).
    pub installed_relay: bool,
}

impl DurableEvidence {
    /// The identity "Always" would store: the protected install first, then
    /// the first matching profile entry.
    #[must_use]
    pub fn preferred(&self) -> Option<DurableIdentity> {
        self.protected_install
            .clone()
            .map(DurableIdentity::ProtectedInstall)
            .or_else(|| {
                self.nix_profiles
                    .first()
                    .cloned()
                    .map(DurableIdentity::NixProfile)
            })
    }

    /// Whether a stored identity is one this connection qualifies for now.
    #[must_use]
    pub fn matches(&self, identity: &DurableIdentity) -> bool {
        match identity {
            DurableIdentity::ProtectedInstall(path) => {
                self.protected_install.as_ref() == Some(path)
            }
            DurableIdentity::NixProfile(path) => self.nix_profiles.contains(path),
        }
    }

    #[cfg(test)]
    pub fn for_test(
        protected_install: Option<&str>,
        nix_profiles: &[&str],
        interpreter: bool,
    ) -> Self {
        Self {
            protected_install: protected_install.map(PathBuf::from),
            nix_profiles: nix_profiles.iter().map(PathBuf::from).collect(),
            interpreter,
            installed_relay: false,
        }
    }
}

/// The account and root durable identity checks run against.
#[derive(Clone, Debug)]
pub struct IdentityContext {
    root: IdentityRoot,
    /// `HOME` and `USER` from the password database entry for the daemon's
    /// uid, never from the environment; `None` disables Nix profile entries.
    account: Option<(PathBuf, OsString)>,
}

impl IdentityContext {
    /// The daemon's context: `/`, root as the only trusted owner, and the
    /// password database entry for its effective uid.
    pub fn for_daemon() -> Self {
        let account = nix::unistd::User::from_uid(nix::unistd::Uid::effective())
            .ok()
            .flatten()
            .map(|user| (user.dir, OsString::from(user.name)));
        Self {
            root: daemon_identity_root(),
            account,
        }
    }

    /// A private tree for tests.
    #[cfg(test)]
    pub fn at(root: IdentityRoot, account: Option<(PathBuf, OsString)>) -> Self {
        Self { root, account }
    }

    /// The account the store's Nix profile entries must belong to.
    pub fn account(&self) -> Option<(&Path, &OsStr)> {
        self.account
            .as_ref()
            .map(|(home, user)| (home.as_path(), user.as_os_str()))
    }

    /// Evaluates one connection's executing file.
    pub fn evaluate(&self, executable: &ExecutableIdentity) -> DurableEvidence {
        let executing = (executable.device, executable.inode);
        let name = executable.path.file_name().unwrap_or_default();
        let protected_install =
            durable_identity::check_protected_install(&self.root, &executable.path, executing)
                .ok()
                .map(|()| executable.path.clone());
        let nix_profiles = self
            .account()
            .map(|(home, user)| {
                durable_identity::matching_nix_profile_entries(
                    &self.root, home, user, name, executing,
                )
            })
            .unwrap_or_default();
        DurableEvidence {
            protected_install,
            nix_profiles,
            interpreter: is_known_interpreter(name),
            installed_relay: false,
        }
    }

    /// The canonical file a stored identity names now: the path itself for a
    /// protected install, and for a Nix profile entry of this account the
    /// physical store path its full walk ends at. Display data for the
    /// access list (to name the policy rule for the same program), never an
    /// authorization. Blocking.
    pub fn canonical_target(&self, identity: &DurableIdentity) -> Option<PathBuf> {
        match identity {
            DurableIdentity::ProtectedInstall(path) => Some(path.clone()),
            DurableIdentity::NixProfile(path) => {
                let (home, user) = self.account()?;
                if !durable_identity::is_nix_profile_entry(path, home, user) {
                    return None;
                }
                durable_identity::nix_profile_target(&self.root, path)
                    .ok()
                    .map(|(resolved, _)| resolved)
            }
        }
    }

    /// Whether `identity` still names the executing file now, checked in
    /// full again.
    pub fn still_qualifies(&self, identity: &DurableIdentity, executing: FileId) -> bool {
        match identity {
            DurableIdentity::ProtectedInstall(path) => {
                durable_identity::check_protected_install(&self.root, path, executing).is_ok()
            }
            DurableIdentity::NixProfile(path) => {
                self.account().is_some_and(|(home, user)| {
                    durable_identity::is_nix_profile_entry(path, home, user)
                }) && durable_identity::resolve_nix_profile_entry(&self.root, path, executing)
                    .is_ok()
            }
        }
    }
}

fn daemon_identity_root() -> IdentityRoot {
    #[cfg(debug_assertions)]
    if let Some(owner) = std::env::var(TEST_TRUSTED_OWNER_ENV)
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
    {
        tracing::warn!(
            owner,
            "debug build: durable identity checks also trust this owner"
        );
        return IdentityRoot::system_also_trusting(owner);
    }
    IdentityRoot::system()
}

// ---------------------------------------------------------------------------
// Grant identifiers
// ---------------------------------------------------------------------------

/// The one runtime grant ID space every source shares, so a revocation or a
/// controller lease names exactly one grant whatever its duration, and the
/// one daemon-wide authorization revision every source advances on each
/// grant or pending-request change (ADR 0014's inventory reports it).
#[derive(Debug, Default)]
pub struct GrantIds {
    next: AtomicU64,
    revision: AtomicU64,
}

impl GrantIds {
    pub fn next(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed).saturating_add(1)
    }

    /// Advances the authorization revision and returns the new value. It
    /// saturates rather than wrapping, so it never moves backwards.
    pub fn advance_revision(&self) -> u64 {
        let mut current = self.revision.load(Ordering::Acquire);
        loop {
            let next = current.saturating_add(1);
            match self.revision.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return next,
                Err(actual) => current = actual,
            }
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }
}

/// A remembered entry's stable ID, distinct from every runtime grant ID.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntryId(String);

impl EntryId {
    fn generate() -> std::io::Result<Self> {
        let mut bytes = [0_u8; ENTRY_ID_HEX / 2];
        let mut filled = 0;
        while filled < bytes.len() {
            filled += rustix::rand::getrandom(
                &mut bytes[filled..],
                rustix::rand::GetRandomFlags::empty(),
            )?;
        }
        let mut id = String::from(ENTRY_ID_PREFIX);
        for byte in bytes {
            for nibble in [byte >> 4, byte & 0x0f] {
                id.push(char::from(b"0123456789abcdef"[usize::from(nibble)]));
            }
        }
        Ok(Self(id))
    }

    fn is_valid(&self) -> bool {
        self.0.strip_prefix(ENTRY_ID_PREFIX).is_some_and(|hex| {
            hex.len() == ENTRY_ID_HEX
                && hex
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    }

    /// An entry ID named by a client, when it is well formed.
    pub fn parse(id: &str) -> Option<Self> {
        let parsed = Self(id.to_owned());
        parsed.is_valid().then_some(parsed)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// ---------------------------------------------------------------------------
// The remembered-grant store
// ---------------------------------------------------------------------------

/// One remembered ("Always for this workspace") definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RememberedEntry {
    pub id: EntryId,
    pub identity: DurableIdentity,
    pub lair_id: LairId,
    pub access: AccessSet,
    pub created_unix_seconds: u64,
    /// Display data: never extends or ends a grant.
    pub last_used_unix_seconds: Option<u64>,
    /// The host-reported agent name, display data only.
    pub display_name: Option<String>,
}

/// Where the store lives: `<state base>/splinterm/agent-access.json`. The
/// state base is used as given and may pass through links; nothing below it
/// may.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoreLocation {
    base: PathBuf,
}

impl StoreLocation {
    pub fn new(base: PathBuf) -> Self {
        Self { base }
    }

    pub fn file_path(&self) -> PathBuf {
        self.base.join(STATE_DIRECTORY).join(STORE_FILE)
    }
}

/// Why the remembered source denies everything in this generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreFault {
    /// No state directory is configured (unit tests).
    NotConfigured,
    /// A later round of this process owns the store; this handle never
    /// writes again.
    Retired,
    /// A link, a wrong owner or mode, or a wrong file type.
    Unsafe(&'static str),
    Corrupt(String),
    Oversized,
    /// A version this generation does not support; never rewritten.
    UnsupportedVersion(u32),
    Unreadable(String),
}

impl fmt::Display for StoreFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConfigured => formatter.write_str("no state directory is configured"),
            Self::Retired => formatter.write_str("a later daemon round owns the store"),
            Self::Unsafe(why) => write!(formatter, "unsafe store: {why}"),
            Self::Corrupt(why) => write!(formatter, "corrupt store: {why}"),
            Self::Oversized => write!(formatter, "store exceeds {MAX_STORE_BYTES} bytes"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported store version {version}")
            }
            Self::Unreadable(why) => write!(formatter, "unreadable store: {why}"),
        }
    }
}

/// Why a store write did not happen. Nothing is granted, and memory keeps
/// its revocations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreWriteError {
    Unavailable(StoreFault),
    /// Something else changed the file since this daemon loaded or wrote it.
    Conflict,
    /// The new document would exceed the entry or byte bound.
    Bounds,
    Io {
        stage: &'static str,
        error: String,
    },
    /// The new store was renamed into place but could not be made durable:
    /// the file may hold the new contents. A commit that ends here grants
    /// nothing and owes the entry's removal.
    NotDurable {
        stage: &'static str,
        error: String,
    },
}

impl fmt::Display for StoreWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(fault) => write!(formatter, "remembered access is unavailable: {fault}"),
            Self::Conflict => formatter.write_str(
                "the remembered-grant store was changed by something else; refusing to overwrite it",
            ),
            Self::Bounds => write!(
                formatter,
                "the remembered-grant store would exceed {MAX_REMEMBERED_ENTRIES} entries or {MAX_STORE_BYTES} bytes"
            ),
            Self::Io { stage, error } => {
                write!(formatter, "cannot write the remembered-grant store ({stage}): {error}")
            }
            Self::NotDurable { stage, error } => write!(
                formatter,
                "the remembered-grant store was replaced but not made durable ({stage}): {error}"
            ),
        }
    }
}

impl std::error::Error for StoreWriteError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Fingerprint {
    device: u64,
    inode: u64,
    size: i64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
}

impl Fingerprint {
    #[allow(
        clippy::useless_conversion,
        clippy::unnecessary_cast,
        clippy::cast_possible_wrap,
        reason = "the stat field widths differ between architectures"
    )]
    fn of(status: &Stat) -> Self {
        Self {
            device: u64::from(status.st_dev),
            inode: u64::from(status.st_ino),
            size: status.st_size as i64,
            modified_seconds: status.st_mtime as i64,
            modified_nanoseconds: status.st_mtime_nsec as i64,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum FileState {
    /// No store file yet: no remembered grants; a write creates one at the
    /// current version.
    Absent,
    /// Loaded at this version; writes keep it.
    Loaded { version: u32 },
    /// The remembered source denies; never written.
    Unavailable(StoreFault),
}

/// The store file and what this daemon last knew of it. Held under the
/// writer lock.
#[derive(Debug)]
struct DurableFile {
    location: Option<StoreLocation>,
    state: FileState,
    /// The file as last loaded or written; `None` while it is absent.
    fingerprint: Option<Fingerprint>,
    #[cfg(test)]
    injected_failures: usize,
    #[cfg(test)]
    fail_after_rename: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreDocumentV1 {
    format: String,
    version: u32,
    entries: Vec<StoredEntryV1>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredEntryV1 {
    id: EntryId,
    identity: DurableIdentity,
    lair_id: LairId,
    access: StoredAccess,
    created_unix_seconds: u64,
    last_used_unix_seconds: Option<u64>,
    display_name: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredAccess {
    Level(AccessLevel),
    Custom(Vec<AccessScope>),
}

#[derive(Deserialize)]
struct VersionProbe {
    format: String,
    version: u32,
}

struct Loaded {
    state: FileState,
    fingerprint: Option<Fingerprint>,
    entries: Vec<RememberedEntry>,
}

impl Loaded {
    fn unavailable(fault: StoreFault) -> Self {
        Self {
            state: FileState::Unavailable(fault),
            fingerprint: None,
            entries: Vec::new(),
        }
    }
}

fn bounded(text: &str) -> String {
    let mut text = text.to_owned();
    if text.len() > MAX_FAULT_BYTES {
        let mut end = MAX_FAULT_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

fn daemon_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// Opens `<base>/splinterm`, never through a link, and requires it owned by
/// the daemon with mode 0700. `Ok(None)` when it, or the base, is absent.
fn open_state_directory(location: &StoreLocation) -> Result<Option<OwnedFd>, StoreFault> {
    let base = match rustix::fs::open(
        &location.base,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(base) => base,
        Err(Errno::NOENT) => return Ok(None),
        Err(error) => return Err(StoreFault::Unreadable(error.to_string())),
    };
    let directory = match rustix::fs::openat(
        &base,
        STATE_DIRECTORY,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(directory) => directory,
        Err(Errno::NOENT) => return Ok(None),
        Err(Errno::LOOP | Errno::NOTDIR) => {
            return Err(StoreFault::Unsafe(
                "the state directory is a link or not a directory",
            ));
        }
        Err(error) => return Err(StoreFault::Unreadable(error.to_string())),
    };
    validate_state_directory(&directory)?;
    Ok(Some(directory))
}

fn validate_state_directory(directory: &OwnedFd) -> Result<(), StoreFault> {
    let status =
        rustix::fs::fstat(directory).map_err(|error| StoreFault::Unreadable(error.to_string()))?;
    if status.st_uid != daemon_uid() || status.st_mode & 0o7777 != 0o700 {
        return Err(StoreFault::Unsafe(
            "the state directory is not owned by the daemon with mode 0700",
        ));
    }
    Ok(())
}

fn validate_store_status(status: &Stat) -> Result<(), StoreFault> {
    if FileType::from_raw_mode(status.st_mode) != FileType::RegularFile {
        return Err(StoreFault::Unsafe("the store is not a regular file"));
    }
    if status.st_nlink != 1 {
        return Err(StoreFault::Unsafe("the store has more than one hard link"));
    }
    if status.st_uid != daemon_uid() || status.st_mode & 0o7777 != 0o600 {
        return Err(StoreFault::Unsafe(
            "the store is not owned by the daemon with mode 0600",
        ));
    }
    if u64::try_from(status.st_size).unwrap_or(u64::MAX) > MAX_STORE_BYTES {
        return Err(StoreFault::Oversized);
    }
    if status.st_size == 0 {
        return Err(StoreFault::Corrupt("the store is empty".into()));
    }
    Ok(())
}

/// The most leftover temporary stores one load removes.
const MAX_STALE_TEMPORARIES: usize = 64;

/// Removes temporary stores a write interrupted by a crash left behind. Only
/// this daemon's own name pattern, only non-directories, and bounded; a
/// failure leaves them for the next load.
fn remove_stale_temporaries(directory: &OwnedFd) {
    let prefix = format!(".{STORE_FILE}.tmp-");
    let Ok(entries) = rustix::fs::Dir::read_from(directory) else {
        return;
    };
    let stale: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type() != FileType::Directory
                && entry.file_name().to_bytes().starts_with(prefix.as_bytes())
        })
        .map(|entry| entry.file_name().to_owned())
        .take(MAX_STALE_TEMPORARIES)
        .collect();
    for name in stale {
        let _ = rustix::fs::unlinkat(directory, name.as_c_str(), AtFlags::empty());
    }
}

/// Loads the store with the policy-style checks. A missing store is no
/// grants; anything unsafe, corrupt, oversized, or of an unsupported version
/// makes the remembered source deny everything.
fn load_store(location: &StoreLocation, account: Option<(&Path, &OsStr)>) -> Loaded {
    let directory = match open_state_directory(location) {
        Ok(Some(directory)) => directory,
        Ok(None) => {
            return Loaded {
                state: FileState::Absent,
                fingerprint: None,
                entries: Vec::new(),
            };
        }
        Err(fault) => return Loaded::unavailable(fault),
    };
    remove_stale_temporaries(&directory);
    let descriptor = match rustix::fs::openat(
        &directory,
        STORE_FILE,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(descriptor) => descriptor,
        Err(Errno::NOENT) => {
            return Loaded {
                state: FileState::Absent,
                fingerprint: None,
                entries: Vec::new(),
            };
        }
        Err(Errno::LOOP) => return Loaded::unavailable(StoreFault::Unsafe("the store is a link")),
        Err(error) => return Loaded::unavailable(StoreFault::Unreadable(error.to_string())),
    };
    let status = match rustix::fs::fstat(&descriptor) {
        Ok(status) => status,
        Err(error) => return Loaded::unavailable(StoreFault::Unreadable(error.to_string())),
    };
    if let Err(fault) = validate_store_status(&status) {
        return Loaded::unavailable(fault);
    }
    let mut bytes = Vec::new();
    if let Err(error) = File::from(descriptor)
        .take(MAX_STORE_BYTES + 1)
        .read_to_end(&mut bytes)
    {
        return Loaded::unavailable(StoreFault::Unreadable(error.to_string()));
    }
    if u64::try_from(bytes.len()).ok() != u64::try_from(status.st_size).ok() {
        return Loaded::unavailable(StoreFault::Unreadable(
            "the store changed while it was read".into(),
        ));
    }
    match decode_store(&bytes, account) {
        Ok((version, entries)) => Loaded {
            state: FileState::Loaded { version },
            fingerprint: Some(Fingerprint::of(&status)),
            entries,
        },
        Err(fault) => Loaded::unavailable(fault),
    }
}

fn decode_store(
    bytes: &[u8],
    account: Option<(&Path, &OsStr)>,
) -> Result<(u32, Vec<RememberedEntry>), StoreFault> {
    let probe: VersionProbe = serde_json::from_slice(bytes)
        .map_err(|error| StoreFault::Corrupt(bounded(&error.to_string())))?;
    if probe.format != STORE_FORMAT {
        return Err(StoreFault::Corrupt("unknown store format".into()));
    }
    if !SUPPORTED_STORE_VERSIONS.contains(&probe.version) {
        return Err(StoreFault::UnsupportedVersion(probe.version));
    }
    let document: StoreDocumentV1 = serde_json::from_slice(bytes)
        .map_err(|error| StoreFault::Corrupt(bounded(&error.to_string())))?;
    if document.entries.len() > MAX_REMEMBERED_ENTRIES {
        return Err(StoreFault::Corrupt("too many entries".into()));
    }
    let mut ids = HashSet::new();
    let mut definitions = HashSet::new();
    let mut entries = Vec::with_capacity(document.entries.len());
    for stored in document.entries {
        let entry = decode_entry(stored, account)?;
        if !ids.insert(entry.id.clone())
            || !definitions.insert((entry.identity.clone(), entry.lair_id))
        {
            return Err(StoreFault::Corrupt(
                "duplicate entry ID or identity and workspace".into(),
            ));
        }
        entries.push(entry);
    }
    Ok((probe.version, entries))
}

fn decode_entry(
    stored: StoredEntryV1,
    account: Option<(&Path, &OsStr)>,
) -> Result<RememberedEntry, StoreFault> {
    let corrupt = |why: &str| StoreFault::Corrupt(why.into());
    if !stored.id.is_valid() {
        return Err(corrupt("invalid entry ID"));
    }
    match &stored.identity {
        DurableIdentity::ProtectedInstall(path) => {
            let bytes = path.as_os_str().as_bytes();
            if !path.is_absolute()
                || bytes.len() > durable_identity::MAX_PATH_BYTES
                || path.to_str().is_none()
                || path.components().any(|part| {
                    !matches!(
                        part,
                        std::path::Component::RootDir | std::path::Component::Normal(_)
                    )
                })
                || bytes.ends_with(b"/")
                || bytes[1..]
                    .split(|byte| *byte == b'/')
                    .any(|part| part.is_empty() || part == b"." || part == b"..")
            {
                return Err(corrupt("invalid protected install path"));
            }
        }
        DurableIdentity::NixProfile(path) => {
            if !account.is_some_and(|(home, user)| {
                durable_identity::is_nix_profile_entry(path, home, user)
            }) {
                return Err(corrupt("not a supported Nix profile entry"));
            }
        }
    }
    let access = match stored.access {
        StoredAccess::Level(level) => AccessSet::Level(level),
        StoredAccess::Custom(scopes) => {
            let count = scopes.len();
            let access = AccessSet::from_scopes(scopes).map_err(|_| corrupt("invalid scopes"))?;
            if let AccessSet::Custom(set) = &access
                && set.len() != count
            {
                return Err(corrupt("duplicate scopes"));
            }
            access
        }
    };
    if stored.display_name.as_ref().is_some_and(|name| {
        name.len() > MAX_DISPLAY_NAME_BYTES || name.chars().any(char::is_control)
    }) {
        return Err(corrupt("invalid display name"));
    }
    Ok(RememberedEntry {
        id: stored.id,
        identity: stored.identity,
        lair_id: stored.lair_id,
        access,
        created_unix_seconds: stored.created_unix_seconds,
        last_used_unix_seconds: stored.last_used_unix_seconds,
        display_name: stored.display_name,
    })
}

fn encode_store(version: u32, entries: &[RememberedEntry]) -> Result<Vec<u8>, StoreWriteError> {
    debug_assert!(SUPPORTED_STORE_VERSIONS.contains(&version));
    if entries.len() > MAX_REMEMBERED_ENTRIES {
        return Err(StoreWriteError::Bounds);
    }
    let document = StoreDocumentV1 {
        format: STORE_FORMAT.to_owned(),
        version,
        entries: entries
            .iter()
            .map(|entry| StoredEntryV1 {
                id: entry.id.clone(),
                identity: entry.identity.clone(),
                lair_id: entry.lair_id,
                access: match &entry.access {
                    AccessSet::Level(level) => StoredAccess::Level(*level),
                    AccessSet::Custom(scopes) => {
                        StoredAccess::Custom(scopes.iter().copied().collect())
                    }
                },
                created_unix_seconds: entry.created_unix_seconds,
                last_used_unix_seconds: entry.last_used_unix_seconds,
                display_name: entry.display_name.clone(),
            })
            .collect(),
    };
    let mut bytes = serde_json::to_vec_pretty(&document).map_err(|error| StoreWriteError::Io {
        stage: "encode",
        error: error.to_string(),
    })?;
    bytes.push(b'\n');
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_STORE_BYTES {
        return Err(StoreWriteError::Bounds);
    }
    Ok(bytes)
}

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(1);

fn io_error(stage: &'static str) -> impl Fn(Errno) -> StoreWriteError {
    move |error| StoreWriteError::Io {
        stage,
        error: error.to_string(),
    }
}

/// Opens the state directory for a write, creating it 0700 when it is
/// absent below an existing state base.
fn open_or_create_state_directory(location: &StoreLocation) -> Result<OwnedFd, StoreWriteError> {
    if let Some(directory) = open_state_directory(location).map_err(StoreWriteError::Unavailable)? {
        return Ok(directory);
    }
    let base = rustix::fs::open(
        &location.base,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io_error("open state base"))?;
    match rustix::fs::mkdirat(&base, STATE_DIRECTORY, Mode::from_raw_mode(0o700)) {
        Ok(()) | Err(Errno::EXIST) => {}
        Err(error) => return Err(io_error("create state directory")(error)),
    }
    open_state_directory(location)
        .map_err(StoreWriteError::Unavailable)?
        .ok_or(StoreWriteError::Io {
            stage: "open state directory",
            error: "missing".into(),
        })
}

/// Writes `bytes` to a new 0600 file `name` in `directory` and syncs it.
fn write_synced_temporary(
    directory: &OwnedFd,
    name: &str,
    bytes: &[u8],
) -> Result<File, StoreWriteError> {
    let descriptor = rustix::fs::openat(
        directory,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )
    .map_err(io_error("create temporary store"))?;
    rustix::fs::fchmod(&descriptor, Mode::from_raw_mode(0o600))
        .map_err(io_error("protect temporary store"))?;
    let mut file = File::from(descriptor);
    let failed = |stage: &'static str| {
        move |error: std::io::Error| StoreWriteError::Io {
            stage,
            error: error.to_string(),
        }
    };
    file.write_all(bytes)
        .map_err(failed("write temporary store"))?;
    file.sync_all().map_err(failed("sync temporary store"))?;
    Ok(file)
}

impl DurableFile {
    fn unconfigured() -> Self {
        Self {
            location: None,
            state: FileState::Unavailable(StoreFault::NotConfigured),
            fingerprint: None,
            #[cfg(test)]
            injected_failures: 0,
            #[cfg(test)]
            fail_after_rename: false,
        }
    }

    fn version_to_write(&self) -> Result<u32, StoreWriteError> {
        match &self.state {
            FileState::Absent => Ok(CURRENT_STORE_VERSION),
            FileState::Loaded { version } => Ok(*version),
            FileState::Unavailable(fault) => Err(StoreWriteError::Unavailable(fault.clone())),
        }
    }

    /// Replaces the store with exactly `entries`: refuses an outside change,
    /// writes a new 0600 file beside it, syncs it, renames it over the store,
    /// and syncs the directory. Only then is the write durable.
    fn write(&mut self, entries: &[RememberedEntry]) -> Result<(), StoreWriteError> {
        let version = self.version_to_write()?;
        let location = self
            .location
            .clone()
            .ok_or(StoreWriteError::Unavailable(StoreFault::NotConfigured))?;
        let encoded = encode_store(version, entries)?;
        #[cfg(test)]
        if self.injected_failures > 0 {
            self.injected_failures -= 1;
            return Err(StoreWriteError::Io {
                stage: "injected",
                error: "injected test failure".into(),
            });
        }
        let directory = open_or_create_state_directory(&location)?;
        let current = match rustix::fs::statat(&directory, STORE_FILE, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(status) => Some(Fingerprint::of(&status)),
            Err(Errno::NOENT) => None,
            Err(error) => return Err(io_error("examine store")(error)),
        };
        if current != self.fingerprint {
            return Err(StoreWriteError::Conflict);
        }
        let temporary = format!(
            ".{STORE_FILE}.tmp-{}-{}",
            std::process::id(),
            NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
        );
        let file = match write_synced_temporary(&directory, &temporary, &encoded) {
            Ok(file) => file,
            Err(error) => {
                let _ = rustix::fs::unlinkat(&directory, temporary.as_str(), AtFlags::empty());
                return Err(error);
            }
        };
        if let Err(error) =
            rustix::fs::renameat(&directory, temporary.as_str(), &directory, STORE_FILE)
        {
            let _ = rustix::fs::unlinkat(&directory, temporary.as_str(), AtFlags::empty());
            return Err(io_error("replace store")(error));
        }
        // The rename happened: the store may now hold the new contents even
        // if what follows fails. Record the new file before the directory
        // sync, so a failed sync does not make our own next write look like
        // an outside change; the write is reported as not durable all the
        // same.
        self.state = FileState::Loaded { version };
        let not_durable = |stage: &'static str| {
            move |error: Errno| StoreWriteError::NotDurable {
                stage,
                error: error.to_string(),
            }
        };
        let status = rustix::fs::fstat(&file).map_err(not_durable("examine new store"));
        if let Ok(status) = &status {
            self.fingerprint = Some(Fingerprint::of(status));
        }
        #[cfg(test)]
        if std::mem::take(&mut self.fail_after_rename) {
            return Err(not_durable("injected")(Errno::IO));
        }
        rustix::fs::fsync(&directory).map_err(not_durable("sync state directory"))?;
        status?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// In-memory definitions and coordination
// ---------------------------------------------------------------------------

/// Which source authorized a request, for audit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentAuthorization {
    pub grant_id: u64,
    pub duration: GrantDuration,
    /// A last-used write is due; the caller runs [`AgentAccess::flush`].
    pub flush_due: bool,
}

/// A committed definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Committed {
    pub grant_id: u64,
    /// The runtime grant the new definition replaced, whose controllers and
    /// subscriptions the caller must end.
    pub replaced: Option<u64>,
    pub entry_id: Option<EntryId>,
}

/// One "while this workspace is open" definition, for the inventory.
#[derive(Clone, Debug)]
pub struct SessionView {
    pub grant_id: u64,
    pub uid: u32,
    pub path: PathBuf,
    pub sha256: String,
    pub lair_id: LairId,
    pub access: AccessSet,
    pub display_name: Option<String>,
    pub created_unix_seconds: u64,
}

/// One remembered definition, for the inventory: listed (with its runtime
/// grant ID) or revoked with its durable removal still owed (`None`).
#[derive(Clone, Debug)]
pub struct RememberedView {
    pub grant_id: Option<u64>,
    pub entry: RememberedEntry,
    pub workspace_missing: bool,
}

/// Whether the remembered source can be used.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreStatus {
    Available,
    Unavailable(String),
    NotConfigured,
}

/// Every definition the engine holds.
#[derive(Clone, Debug)]
pub struct AgentInventory {
    pub sessions: Vec<SessionView>,
    pub remembered: Vec<RememberedView>,
    pub store: StoreStatus,
}

/// What **Revoke all access** revoked in the engine.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct RevokedAll {
    pub session_grant_ids: Vec<u64>,
    pub remembered_grant_ids: Vec<u64>,
}

/// What ending a Lair removed.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct LairEnd {
    pub grant_ids: Vec<u64>,
    /// Remembered entries were removed; the caller runs a flush.
    pub flush_needed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentAuditEvent {
    Granted,
    Replaced,
    Revoked,
    StoreError,
}

/// Body-free metadata about one grant-definition event.
#[allow(
    dead_code,
    reason = "bounded audit metadata is retained in memory for PR 4's inventory and trusted diagnostics"
)]
#[derive(Clone, Debug)]
pub struct AgentAuditRecord {
    pub order: u64,
    pub unix_seconds: u64,
    pub event: AgentAuditEvent,
    pub duration: GrantDuration,
    pub lair_id: Option<LairId>,
    pub identity: Option<String>,
    pub entry_id: Option<EntryId>,
    pub grant_id: Option<u64>,
    pub scopes: Vec<AccessScope>,
    pub reason: String,
}

#[derive(Clone, Debug)]
struct SessionGrant {
    grant_id: u64,
    uid: u32,
    path: PathBuf,
    sha256: String,
    lair_id: LairId,
    access: AccessSet,
    display_name: Option<String>,
    created_unix_seconds: u64,
}

#[derive(Clone, Debug)]
struct RememberedGrant {
    grant_id: u64,
    entry: RememberedEntry,
    /// The last-used time the store holds for this entry.
    persisted_last_used: Option<u64>,
    /// Its Lair was absent when the store was loaded: it authorizes nothing
    /// and is listed as "workspace missing" until revoked.
    workspace_missing: bool,
}

#[derive(Debug, Default)]
struct Memory {
    sessions: Vec<SessionGrant>,
    remembered: Vec<RememberedGrant>,
    /// Revoked in memory, still in the file until a write succeeds; the
    /// list says these return after a restart.
    pending_removals: BTreeMap<EntryId, RememberedEntry>,
    /// Lairs this generation saw end; nothing new attaches to them.
    ended_lairs: HashSet<LairId>,
    /// The remembered source is usable (the store is absent or loaded).
    available: bool,
    /// Why it is not, for the inventory.
    fault: Option<StoreFault>,
    flush_in_flight: bool,
    next_last_used_attempt: u64,
    next_audit_order: u64,
    audit: VecDeque<AgentAuditRecord>,
}

impl Memory {
    fn record(
        &mut self,
        event: AgentAuditEvent,
        duration: GrantDuration,
        subject: AuditSubject<'_>,
        reason: &str,
    ) {
        self.next_audit_order = self.next_audit_order.saturating_add(1);
        if self.audit.len() == MAX_AGENT_AUDIT_RECORDS {
            self.audit.pop_front();
        }
        self.audit.push_back(AgentAuditRecord {
            order: self.next_audit_order,
            unix_seconds: unix_seconds(),
            event,
            duration,
            lair_id: subject.lair_id,
            identity: subject
                .identity
                .map(|path| bounded(&path.to_string_lossy())),
            entry_id: subject.entry_id.cloned(),
            grant_id: subject.grant_id,
            scopes: subject
                .access
                .map(|access| access.scopes().into_iter().collect())
                .unwrap_or_default(),
            reason: bounded(reason),
        });
    }

    fn remembered_entries(&self) -> Vec<RememberedEntry> {
        self.remembered
            .iter()
            .map(|grant| grant.entry.clone())
            .collect()
    }

    fn flush_needed(&self) -> bool {
        !self.pending_removals.is_empty()
            || self
                .remembered
                .iter()
                .any(|grant| grant.entry.last_used_unix_seconds != grant.persisted_last_used)
    }

    /// After a durable write of exactly `written`: those entries' last-used
    /// times are persisted, and every pending removal absent from it is done.
    /// Returns whether a pending removal completed.
    fn written(&mut self, written: &[RememberedEntry]) -> bool {
        let by_id: BTreeMap<_, _> = written
            .iter()
            .map(|entry| (&entry.id, entry.last_used_unix_seconds))
            .collect();
        for grant in &mut self.remembered {
            if let Some(last_used) = by_id.get(&grant.entry.id) {
                grant.persisted_last_used = *last_used;
            }
        }
        let before = self.pending_removals.len();
        self.pending_removals.retain(|id, _| by_id.contains_key(id));
        self.pending_removals.len() != before
    }
}

#[derive(Clone, Copy, Default)]
struct AuditSubject<'a> {
    lair_id: Option<LairId>,
    identity: Option<&'a Path>,
    entry_id: Option<&'a EntryId>,
    grant_id: Option<u64>,
    access: Option<&'a AccessSet>,
}

/// A test hook run once inside a remembered commit, after the durable write
/// and before the memory update, with the writer lock held.
#[cfg(test)]
#[derive(Default)]
struct CommitHook(Mutex<Option<Box<dyn FnOnce() + Send>>>);

#[cfg(test)]
impl CommitHook {
    fn run(&self) {
        let hook = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(hook) = hook {
            hook();
        }
    }
}

#[cfg(test)]
impl fmt::Debug for CommitHook {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CommitHook")
    }
}

/// Workspace-session and remembered definitions, and the remembered store.
#[derive(Debug)]
pub struct AgentAccess {
    ids: Arc<GrantIds>,
    file: Mutex<DurableFile>,
    memory: Mutex<Memory>,
    #[cfg(test)]
    after_commit_write: CommitHook,
}

impl AgentAccess {
    /// Loads the store fresh, as every generation does at startup, adoption,
    /// or rollback. Entries whose Lair is absent authorize nothing. Entries in
    /// `carried_removals` (revoked in a previous round of this same process
    /// whose durable removal had not succeeded) stay revoked.
    pub fn load(
        ids: Arc<GrantIds>,
        location: Option<StoreLocation>,
        context: &IdentityContext,
        topology: &Topology,
        carried_removals: &BTreeSet<EntryId>,
    ) -> Self {
        let Some(location) = location else {
            return Self::unconfigured(ids);
        };
        let loaded = load_store(&location, context.account());
        let mut memory = Memory {
            available: !matches!(loaded.state, FileState::Unavailable(_)),
            fault: match &loaded.state {
                FileState::Unavailable(fault) => Some(fault.clone()),
                _ => None,
            },
            ..Memory::default()
        };
        if let FileState::Unavailable(fault) = &loaded.state {
            tracing::error!(%fault, path = %location.file_path().display(), "remembered agent access is unavailable; only that source denies");
            memory.record(
                AgentAuditEvent::StoreError,
                GrantDuration::Remembered,
                AuditSubject::default(),
                &fault.to_string(),
            );
        }
        for entry in loaded.entries {
            if carried_removals.contains(&entry.id) {
                memory.pending_removals.insert(entry.id.clone(), entry);
                continue;
            }
            memory.remembered.push(RememberedGrant {
                grant_id: ids.next(),
                persisted_last_used: entry.last_used_unix_seconds,
                workspace_missing: topology.find_lair(entry.lair_id).is_none(),
                entry,
            });
        }
        Self {
            ids,
            file: Mutex::new(DurableFile {
                location: Some(location),
                state: loaded.state,
                fingerprint: loaded.fingerprint,
                #[cfg(test)]
                injected_failures: 0,
                #[cfg(test)]
                fail_after_rename: false,
            }),
            memory: Mutex::new(memory),
            #[cfg(test)]
            after_commit_write: CommitHook::default(),
        }
    }

    /// No store: the remembered source denies (unit tests that persist
    /// nothing).
    pub fn unconfigured(ids: Arc<GrantIds>) -> Self {
        Self {
            ids,
            file: Mutex::new(DurableFile::unconfigured()),
            memory: Mutex::new(Memory {
                fault: Some(StoreFault::NotConfigured),
                ..Memory::default()
            }),
            #[cfg(test)]
            after_commit_write: CommitHook::default(),
        }
    }

    fn memory(&self) -> std::sync::MutexGuard<'_, Memory> {
        self.memory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn file(&self) -> std::sync::MutexGuard<'_, DurableFile> {
        self.file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether "Always" may be offered and committed.
    pub fn store_available(&self) -> bool {
        self.memory().available
    }

    /// The "while this workspace is open" definition a new answer from this
    /// peer for this Lair would replace, so the prompt can say so.
    pub fn session_definition(&self, peer: &PeerIdentity, lair_id: LairId) -> Option<AccessSet> {
        let executable = peer.persistent_executable()?;
        self.memory()
            .sessions
            .iter()
            .find(|grant| {
                grant.lair_id == lair_id
                    && grant.uid == peer.uid
                    && grant.path == executable.path
                    && grant.sha256 == executable.sha256
            })
            .map(|grant| grant.access.clone())
    }

    /// The remembered definition an "Always" answer for this identity and
    /// Lair would replace, so the prompt can say so.
    pub fn remembered_definition(
        &self,
        identity: &DurableIdentity,
        lair_id: LairId,
    ) -> Option<AccessSet> {
        self.memory()
            .remembered
            .iter()
            .find(|grant| grant.entry.identity == *identity && grant.entry.lair_id == lair_id)
            .map(|grant| grant.entry.access.clone())
    }

    /// The first session or remembered definition that alone covers every
    /// required consent scope for this peer and Lair. A match updates the
    /// remembered entry's last-used time in memory.
    pub fn authorize(
        &self,
        peer: &PeerIdentity,
        lair_id: LairId,
        required: &[AccessScope],
        now: u64,
    ) -> Option<AgentAuthorization> {
        let evidence = peer.durable_evidence()?;
        // The relay is never eligible, whatever a hand-edited store says.
        if evidence.installed_relay {
            return None;
        }
        let mut memory = self.memory();
        if let Some(executable) = peer.persistent_executable()
            && let Some(grant) = memory.sessions.iter().find(|grant| {
                grant.lair_id == lair_id
                    && grant.uid == peer.uid
                    && grant.path == executable.path
                    && grant.sha256 == executable.sha256
                    && grant.access.covers(required)
            })
        {
            return Some(AgentAuthorization {
                grant_id: grant.grant_id,
                duration: GrantDuration::WorkspaceSession,
                flush_due: false,
            });
        }
        if !memory.available {
            return None;
        }
        let memory = &mut *memory;
        let grant = memory.remembered.iter_mut().find(|grant| {
            grant.entry.lair_id == lair_id
                && !grant.workspace_missing
                && evidence.matches(&grant.entry.identity)
                && grant.entry.access.covers(required)
        })?;
        grant.entry.last_used_unix_seconds = Some(now);
        let flush_due = !memory.flush_in_flight
            && now >= memory.next_last_used_attempt
            && now.saturating_sub(grant.persisted_last_used.unwrap_or(0))
                >= LAST_USED_WRITE_INTERVAL_SECONDS;
        let grant_id = grant.grant_id;
        if flush_due {
            memory.flush_in_flight = true;
        }
        Some(AgentAuthorization {
            grant_id,
            duration: GrantDuration::Remembered,
            flush_due,
        })
    }

    #[cfg(test)]
    /// Commits a "while this workspace is open" definition for the peer's
    /// verified executable identity, replacing an earlier one for the same
    /// identity and Lair with exactly `access`.
    ///
    /// # Errors
    /// The peer has no verified executable identity, or the Lair ended.
    #[cfg(test)]
    pub fn commit_session(
        &self,
        peer: &PeerIdentity,
        lair_id: LairId,
        access: AccessSet,
        display_name: Option<String>,
    ) -> Result<Committed, CommitError> {
        self.commit_session_gated(peer, lair_id, access, display_name, || true)
    }

    /// [`Self::commit_session`], committing only when `gate` agrees. The gate
    /// runs under this engine's memory lock, just before the definition
    /// becomes visible, so whatever it checks cannot change in between.
    ///
    /// # Errors
    /// As [`Self::commit_session`], or [`CommitError::Ended`] when the gate
    /// refused.
    pub fn commit_session_gated(
        &self,
        peer: &PeerIdentity,
        lair_id: LairId,
        access: AccessSet,
        display_name: Option<String>,
        gate: impl FnOnce() -> bool,
    ) -> Result<Committed, CommitError> {
        let executable = peer
            .persistent_executable()
            .ok_or(CommitError::NotOffered(Withheld::NoPinnedIdentity))?;
        let mut memory = self.memory();
        if memory.ended_lairs.contains(&lair_id) {
            return Err(CommitError::WorkspaceEnded);
        }
        if !gate() {
            return Err(CommitError::Ended);
        }
        let replaced = memory
            .sessions
            .iter()
            .position(|grant| {
                grant.lair_id == lair_id
                    && grant.uid == peer.uid
                    && grant.path == executable.path
                    && grant.sha256 == executable.sha256
            })
            .map(|index| memory.sessions.remove(index));
        let grant = SessionGrant {
            grant_id: self.ids.next(),
            uid: peer.uid,
            path: executable.path.clone(),
            sha256: executable.sha256.clone(),
            lair_id,
            access,
            display_name: display_name.map(|name| bounded_display_name(&name)),
            created_unix_seconds: unix_seconds(),
        };
        let committed = Committed {
            grant_id: grant.grant_id,
            replaced: replaced.as_ref().map(|old| old.grant_id),
            entry_id: None,
        };
        if let Some(old) = &replaced {
            memory.record(
                AgentAuditEvent::Replaced,
                GrantDuration::WorkspaceSession,
                AuditSubject {
                    lair_id: Some(lair_id),
                    identity: Some(&old.path),
                    grant_id: Some(old.grant_id),
                    access: Some(&old.access),
                    ..AuditSubject::default()
                },
                "replaced by a new answer",
            );
        }
        memory.record(
            AgentAuditEvent::Granted,
            GrantDuration::WorkspaceSession,
            AuditSubject {
                lair_id: Some(lair_id),
                identity: Some(&grant.path),
                grant_id: Some(grant.grant_id),
                access: Some(&grant.access),
                ..AuditSubject::default()
            },
            "while this workspace is open",
        );
        memory.sessions.push(grant);
        self.ids.advance_revision();
        Ok(committed)
    }

    /// Commits an "Always" definition durably, then in memory, replacing an
    /// earlier one for the same identity and Lair with exactly `access`.
    /// Blocking: run it on a blocking thread.
    ///
    /// # Errors
    /// The store is unavailable, changed outside the daemon, would exceed its
    /// bounds, or cannot be written; or the Lair ended. Nothing is granted.
    pub fn commit_remembered(
        &self,
        identity: DurableIdentity,
        lair_id: LairId,
        access: AccessSet,
        display_name: Option<String>,
    ) -> Result<Committed, CommitError> {
        let entry = RememberedEntry {
            id: EntryId::generate().map_err(|error| {
                CommitError::Store(StoreWriteError::Io {
                    stage: "generate entry ID",
                    error: error.to_string(),
                })
            })?,
            identity,
            lair_id,
            access,
            created_unix_seconds: unix_seconds(),
            last_used_unix_seconds: None,
            display_name: display_name.map(|name| bounded_display_name(&name)),
        };
        let mut file = self.file();
        let proposal = {
            let memory = self.memory();
            if memory.ended_lairs.contains(&lair_id) {
                return Err(CommitError::WorkspaceEnded);
            }
            let mut proposal: Vec<_> = memory
                .remembered
                .iter()
                .filter(|grant| {
                    !(grant.entry.identity == entry.identity && grant.entry.lair_id == lair_id)
                })
                .map(|grant| grant.entry.clone())
                .collect();
            proposal.push(entry.clone());
            proposal
        };
        if let Err(error) = file.write(&proposal) {
            if matches!(error, StoreWriteError::NotDurable { .. }) {
                // The file may already hold the entry. It grants nothing, and
                // its removal is owed to the next write and to shutdown, so
                // a restart does not bring back an answer that failed.
                self.memory()
                    .pending_removals
                    .insert(entry.id.clone(), entry.clone());
            }
            self.record_store_error(&error, Some(&entry));
            return Err(CommitError::Store(error));
        }
        // The writer lock stays held until memory matches the file (ADR 0014:
        // one lock over the read-modify-write and the memory change), so no
        // flush can snapshot memory without the new entry and rewrite it away.
        #[cfg(test)]
        self.after_commit_write.run();
        let mut memory = self.memory();
        if memory.written(&proposal) {
            self.ids.advance_revision();
        }
        // The Lair ended while the write ran (which also removed any earlier
        // definition): the new entry never authorizes and leaves the file at
        // the next write.
        if memory.ended_lairs.contains(&lair_id) {
            memory.pending_removals.insert(entry.id.clone(), entry);
            return Err(CommitError::WorkspaceEnded);
        }
        let replaced = memory
            .remembered
            .iter()
            .position(|grant| {
                grant.entry.identity == entry.identity && grant.entry.lair_id == lair_id
            })
            .map(|index| memory.remembered.remove(index));
        let grant_id = self.ids.next();
        if let Some(old) = &replaced {
            memory.record(
                AgentAuditEvent::Replaced,
                GrantDuration::Remembered,
                AuditSubject {
                    lair_id: Some(lair_id),
                    identity: Some(old.entry.identity.path()),
                    entry_id: Some(&old.entry.id),
                    grant_id: Some(old.grant_id),
                    access: Some(&old.entry.access),
                },
                "replaced by a new answer",
            );
        }
        memory.record(
            AgentAuditEvent::Granted,
            GrantDuration::Remembered,
            AuditSubject {
                lair_id: Some(lair_id),
                identity: Some(entry.identity.path()),
                entry_id: Some(&entry.id),
                grant_id: Some(grant_id),
                access: Some(&entry.access),
            },
            "always for this workspace",
        );
        let entry_id = entry.id.clone();
        memory.remembered.push(RememberedGrant {
            grant_id,
            entry,
            persisted_last_used: None,
            workspace_missing: false,
        });
        self.ids.advance_revision();
        Ok(Committed {
            grant_id,
            replaced: replaced.map(|old| old.grant_id),
            entry_id: Some(entry_id),
        })
    }

    /// Revokes a "while open" definition by its runtime grant ID.
    pub fn revoke_session(&self, grant_id: u64) -> bool {
        let mut memory = self.memory();
        let Some(index) = memory
            .sessions
            .iter()
            .position(|grant| grant.grant_id == grant_id)
        else {
            return false;
        };
        let grant = memory.sessions.remove(index);
        memory.record(
            AgentAuditEvent::Revoked,
            GrantDuration::WorkspaceSession,
            AuditSubject {
                lair_id: Some(grant.lair_id),
                identity: Some(&grant.path),
                grant_id: Some(grant.grant_id),
                access: Some(&grant.access),
                ..AuditSubject::default()
            },
            "explicit local revocation",
        );
        self.ids.advance_revision();
        true
    }

    /// Revokes a remembered definition in memory at once and returns its
    /// runtime grant ID; the caller then runs [`Self::flush`] to remove it
    /// durably. Until that succeeds it stays revoked here and is retried by
    /// the next write and at shutdown.
    pub fn revoke_remembered(&self, entry_id: &EntryId) -> Option<u64> {
        let mut memory = self.memory();
        let index = memory
            .remembered
            .iter()
            .position(|grant| &grant.entry.id == entry_id)?;
        let grant = memory.remembered.remove(index);
        memory.record(
            AgentAuditEvent::Revoked,
            GrantDuration::Remembered,
            AuditSubject {
                lair_id: Some(grant.entry.lair_id),
                identity: Some(grant.entry.identity.path()),
                entry_id: Some(&grant.entry.id),
                grant_id: Some(grant.grant_id),
                access: Some(&grant.entry.access),
            },
            "explicit local revocation",
        );
        memory
            .pending_removals
            .insert(grant.entry.id.clone(), grant.entry);
        self.ids.advance_revision();
        Some(grant.grant_id)
    }

    /// Removes every session and remembered definition for an ended Lair.
    pub fn end_lair(&self, lair_id: LairId, reason: &'static str) -> LairEnd {
        let mut memory = self.memory();
        memory.ended_lairs.insert(lair_id);
        let mut end = LairEnd::default();
        let (ended, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut memory.sessions)
            .into_iter()
            .partition(|grant| grant.lair_id == lair_id);
        memory.sessions = kept;
        for grant in ended {
            end.grant_ids.push(grant.grant_id);
            memory.record(
                AgentAuditEvent::Revoked,
                GrantDuration::WorkspaceSession,
                AuditSubject {
                    lair_id: Some(lair_id),
                    identity: Some(&grant.path),
                    grant_id: Some(grant.grant_id),
                    access: Some(&grant.access),
                    ..AuditSubject::default()
                },
                reason,
            );
        }
        let (ended, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut memory.remembered)
            .into_iter()
            .partition(|grant| grant.entry.lair_id == lair_id);
        memory.remembered = kept;
        for grant in ended {
            end.grant_ids.push(grant.grant_id);
            end.flush_needed = true;
            memory.record(
                AgentAuditEvent::Revoked,
                GrantDuration::Remembered,
                AuditSubject {
                    lair_id: Some(lair_id),
                    identity: Some(grant.entry.identity.path()),
                    entry_id: Some(&grant.entry.id),
                    grant_id: Some(grant.grant_id),
                    access: Some(&grant.entry.access),
                },
                reason,
            );
            memory
                .pending_removals
                .insert(grant.entry.id.clone(), grant.entry);
        }
        if !end.grant_ids.is_empty() {
            self.ids.advance_revision();
        }
        end
    }

    /// Whether a write is owed: a pending removal or an unwritten last-used
    /// time.
    pub fn flush_needed(&self) -> bool {
        self.memory().flush_needed()
    }

    /// Writes memory's remembered definitions when a write is owed: pending
    /// removals and last-used times. Blocking.
    ///
    /// # Errors
    /// The write failed; memory keeps its revocations, and a later write or
    /// shutdown retries.
    pub fn flush(&self) -> Result<(), StoreWriteError> {
        let mut file = self.file();
        let proposal = {
            let mut memory = self.memory();
            if !memory.flush_needed() {
                memory.flush_in_flight = false;
                return Ok(());
            }
            memory.remembered_entries()
        };
        let result = file.write(&proposal);
        match &result {
            Ok(()) => {
                let mut memory = self.memory();
                if memory.written(&proposal) {
                    self.ids.advance_revision();
                }
                memory.flush_in_flight = false;
            }
            Err(error) => {
                {
                    let mut memory = self.memory();
                    memory.flush_in_flight = false;
                    memory.next_last_used_attempt =
                        unix_seconds().saturating_add(LAST_USED_WRITE_INTERVAL_SECONDS);
                }
                self.record_store_error(error, None);
            }
        }
        result
    }

    /// Retires this round's store handle and returns the removals it still
    /// owes. Under the writer lock, so no write of this handle can follow:
    /// a resumed round's fresh handle is then the store's only writer.
    pub fn retire(&self) -> BTreeSet<EntryId> {
        let mut file = self.file();
        file.state = FileState::Unavailable(StoreFault::Retired);
        let mut memory = self.memory();
        memory.available = false;
        memory.fault = Some(StoreFault::Retired);
        memory.pending_removals.keys().cloned().collect()
    }

    /// Entries revoked in memory whose durable removal is still owed.
    #[cfg(test)]
    pub fn pending_removal_ids(&self) -> BTreeSet<EntryId> {
        self.memory().pending_removals.keys().cloned().collect()
    }

    /// Whether a runtime grant ID is a session or remembered grant still in
    /// force. Controller and subscription registration check this under
    /// their own lock, so a revoke that lands after authorization wins.
    pub fn is_live(&self, grant_id: u64) -> bool {
        let memory = self.memory();
        memory
            .sessions
            .iter()
            .any(|grant| grant.grant_id == grant_id)
            || (memory.available
                && memory
                    .remembered
                    .iter()
                    .any(|grant| grant.grant_id == grant_id && !grant.workspace_missing))
    }

    /// Whether a runtime grant ID belongs to a listed remembered entry, which
    /// is revoked by its durable entry ID instead.
    pub fn is_remembered_grant(&self, grant_id: u64) -> bool {
        self.memory()
            .remembered
            .iter()
            .any(|grant| grant.grant_id == grant_id)
    }

    /// The Lair of a session grant, by its runtime ID.
    pub fn session_lair(&self, grant_id: u64) -> Option<LairId> {
        self.memory()
            .sessions
            .iter()
            .find(|grant| grant.grant_id == grant_id)
            .map(|grant| grant.lair_id)
    }

    /// The Lair of a listed remembered entry.
    pub fn remembered_lair(&self, entry_id: &EntryId) -> Option<LairId> {
        self.memory()
            .remembered
            .iter()
            .find(|grant| &grant.entry.id == entry_id)
            .map(|grant| grant.entry.lair_id)
    }

    /// Every definition this engine holds, for the access inventory.
    pub fn inventory(&self) -> AgentInventory {
        let memory = self.memory();
        AgentInventory {
            sessions: memory
                .sessions
                .iter()
                .map(|grant| SessionView {
                    grant_id: grant.grant_id,
                    uid: grant.uid,
                    path: grant.path.clone(),
                    sha256: grant.sha256.clone(),
                    lair_id: grant.lair_id,
                    access: grant.access.clone(),
                    display_name: grant.display_name.clone(),
                    created_unix_seconds: grant.created_unix_seconds,
                })
                .collect(),
            remembered: memory
                .remembered
                .iter()
                .map(|grant| RememberedView {
                    grant_id: Some(grant.grant_id),
                    entry: grant.entry.clone(),
                    workspace_missing: grant.workspace_missing,
                })
                .chain(
                    memory
                        .pending_removals
                        .values()
                        .map(|entry| RememberedView {
                            grant_id: None,
                            entry: entry.clone(),
                            workspace_missing: false,
                        }),
                )
                .collect(),
            store: if memory.available {
                StoreStatus::Available
            } else {
                match &memory.fault {
                    None | Some(StoreFault::NotConfigured) => StoreStatus::NotConfigured,
                    Some(fault) => StoreStatus::Unavailable(fault.to_string()),
                }
            },
        }
    }

    /// How many session and remembered definitions **Revoke all access**
    /// would revoke now.
    pub fn revocable_counts(&self) -> (usize, usize) {
        let memory = self.memory();
        (memory.sessions.len(), memory.remembered.len())
    }

    /// **Revoke all access**: every session and remembered definition, in
    /// memory at once. The caller ends their controllers and subscriptions
    /// and runs [`Self::flush`] to remove the remembered ones durably; until
    /// that succeeds they stay revoked here and are retried.
    pub fn revoke_all(&self, reason: &'static str) -> RevokedAll {
        let mut memory = self.memory();
        let sessions = std::mem::take(&mut memory.sessions);
        let remembered = std::mem::take(&mut memory.remembered);
        let mut revoked = RevokedAll::default();
        for grant in sessions {
            revoked.session_grant_ids.push(grant.grant_id);
            memory.record(
                AgentAuditEvent::Revoked,
                GrantDuration::WorkspaceSession,
                AuditSubject {
                    lair_id: Some(grant.lair_id),
                    identity: Some(&grant.path),
                    grant_id: Some(grant.grant_id),
                    access: Some(&grant.access),
                    ..AuditSubject::default()
                },
                reason,
            );
        }
        for grant in remembered {
            revoked.remembered_grant_ids.push(grant.grant_id);
            memory.record(
                AgentAuditEvent::Revoked,
                GrantDuration::Remembered,
                AuditSubject {
                    lair_id: Some(grant.entry.lair_id),
                    identity: Some(grant.entry.identity.path()),
                    entry_id: Some(&grant.entry.id),
                    grant_id: Some(grant.grant_id),
                    access: Some(&grant.entry.access),
                },
                reason,
            );
            memory
                .pending_removals
                .insert(grant.entry.id.clone(), grant.entry);
        }
        if !revoked.session_grant_ids.is_empty() || !revoked.remembered_grant_ids.is_empty() {
            self.ids.advance_revision();
        }
        revoked
    }

    fn record_store_error(&self, error: &StoreWriteError, entry: Option<&RememberedEntry>) {
        tracing::error!(%error, "remembered agent access store write failed");
        self.memory().record(
            AgentAuditEvent::StoreError,
            GrantDuration::Remembered,
            AuditSubject {
                lair_id: entry.map(|entry| entry.lair_id),
                identity: entry.map(|entry| entry.identity.path()),
                entry_id: entry.map(|entry| &entry.id),
                access: entry.map(|entry| &entry.access),
                ..AuditSubject::default()
            },
            &error.to_string(),
        );
    }

    #[cfg(test)]
    pub fn remembered_for_test(&self) -> Vec<(u64, RememberedEntry)> {
        self.memory()
            .remembered
            .iter()
            .map(|grant| (grant.grant_id, grant.entry.clone()))
            .collect()
    }

    #[cfg(test)]
    pub fn session_count_for_test(&self) -> usize {
        self.memory().sessions.len()
    }

    #[cfg(test)]
    pub fn audit_for_test(&self) -> Vec<AgentAuditRecord> {
        self.memory().audit.iter().cloned().collect()
    }

    #[cfg(test)]
    pub fn inject_write_failures(&self, count: usize) {
        self.file().injected_failures = count;
    }

    #[cfg(test)]
    pub fn after_commit_write_for_test(&self, hook: impl FnOnce() + Send + 'static) {
        *self
            .after_commit_write
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Box::new(hook));
    }

    #[cfg(test)]
    pub fn inject_failure_after_rename(&self) {
        self.file().fail_after_rename = true;
    }

    #[cfg(test)]
    pub fn workspace_missing_for_test(&self, entry_id: &EntryId) -> Option<bool> {
        self.memory()
            .remembered
            .iter()
            .find(|grant| &grant.entry.id == entry_id)
            .map(|grant| grant.workspace_missing)
    }
}

/// Why a commit granted nothing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommitError {
    NotOffered(Withheld),
    WorkspaceEnded,
    Store(StoreWriteError),
    /// The answer's pending request ended before the grant became visible.
    Ended,
}

impl fmt::Display for CommitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotOffered(why) => write!(formatter, "duration not offered: {why:?}"),
            Self::WorkspaceEnded => formatter.write_str("the workspace has ended"),
            Self::Ended => formatter.write_str("the access request ended"),
            Self::Store(error) => error.fmt(formatter),
        }
    }
}

fn bounded_display_name(name: &str) -> String {
    let mut cleaned: String = name
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    if cleaned.len() > MAX_DISPLAY_NAME_BYTES {
        let mut end = MAX_DISPLAY_NAME_BYTES;
        while !cleaned.is_char_boundary(end) {
            end -= 1;
        }
        cleaned.truncate(end);
    }
    cleaned
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::{MetadataExt, PermissionsExt, symlink},
        sync::atomic::AtomicU64,
    };

    use super::*;

    static NEXT_BASE: AtomicU64 = AtomicU64::new(0);

    /// A private state base with a 0700 `splinterm` directory, removed on
    /// drop.
    struct Base {
        path: PathBuf,
    }

    impl Base {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "splinterd-agent-access-{}-{}",
                std::process::id(),
                NEXT_BASE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(path.join(STATE_DIRECTORY)).unwrap();
            fs::set_permissions(
                path.join(STATE_DIRECTORY),
                fs::Permissions::from_mode(0o700),
            )
            .unwrap();
            Self { path }
        }

        fn without_state_directory() -> Self {
            let base = Self::new();
            fs::remove_dir(base.path.join(STATE_DIRECTORY)).unwrap();
            base
        }

        fn location(&self) -> StoreLocation {
            StoreLocation::new(self.path.clone())
        }

        fn store(&self) -> PathBuf {
            self.location().file_path()
        }

        fn write(&self, bytes: &[u8]) {
            fs::write(self.store(), bytes).unwrap();
            fs::set_permissions(self.store(), fs::Permissions::from_mode(0o600)).unwrap();
        }

        fn read(&self) -> Vec<u8> {
            fs::read(self.store()).unwrap()
        }

        fn document(&self) -> serde_json::Value {
            serde_json::from_slice(&self.read()).unwrap()
        }

        fn load(&self) -> AgentAccess {
            self.load_with(&Topology::new(), &BTreeSet::new())
        }

        fn load_with(&self, topology: &Topology, carried: &BTreeSet<EntryId>) -> AgentAccess {
            AgentAccess::load(
                Arc::new(GrantIds::default()),
                Some(self.location()),
                &context(),
                topology,
                carried,
            )
        }
    }

    impl Drop for Base {
        fn drop(&mut self) {
            let _ = fs::set_permissions(
                self.path.join(STATE_DIRECTORY),
                fs::Permissions::from_mode(0o700),
            );
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn context() -> IdentityContext {
        IdentityContext::at(
            IdentityRoot::system(),
            Some((PathBuf::from("/home/u"), OsString::from("u"))),
        )
    }

    fn executable(path: &str, sha: char) -> ExecutableIdentity {
        ExecutableIdentity {
            path: PathBuf::from(path),
            device: 1,
            inode: 2,
            owner_uid: 0,
            mode: 0o100_755,
            size: 1,
            sha256: sha.to_string().repeat(64),
        }
    }

    /// A pinned agent whose executable is a protected install at `path`.
    fn agent(path: &str) -> PeerIdentity {
        PeerIdentity::for_test_executing(
            executable(path, 'a'),
            DurableEvidence::for_test(Some(path), &[], false),
        )
    }

    fn protected(path: &str) -> DurableIdentity {
        DurableIdentity::ProtectedInstall(PathBuf::from(path))
    }

    fn look() -> AccessSet {
        AccessSet::Level(AccessLevel::Look)
    }

    fn scopes(set: &[AccessScope]) -> BTreeSet<AccessScope> {
        set.iter().copied().collect()
    }

    const NOW: u64 = 1_800_000_000;

    /// A topology holding a Lair for each ID, so entries for them are not
    /// "workspace missing".
    fn topology_with(lairs: &[LairId]) -> Topology {
        let mut topology = Topology::new();
        for (index, lair_id) in lairs.iter().enumerate() {
            let mut lair =
                splinterm_core::Lair::new(format!("present-{index}"), PathBuf::from("/tmp"));
            lair.id = *lair_id;
            let revision = topology.revision();
            topology.insert_lair_at(revision, lair).unwrap();
        }
        topology
    }

    // ---- Levels and offers ----

    #[test]
    fn the_authorization_revision_saturates_instead_of_wrapping() {
        let ids = GrantIds {
            next: AtomicU64::new(0),
            revision: AtomicU64::new(u64::MAX - 1),
        };
        assert_eq!(ids.advance_revision(), u64::MAX);
        assert_eq!(ids.advance_revision(), u64::MAX);
        assert_eq!(ids.revision(), u64::MAX);
    }

    #[test]
    fn levels_expand_to_exactly_the_adr_bundles() {
        use AccessScope as S;
        let look = scopes(&[
            S::TopologyObserve,
            S::Observe,
            S::Scrollback,
            S::AgentStatusReport,
        ]);
        let mut look_and_type = look.clone();
        look_and_type.extend([S::Input, S::Resize, S::ControlTakeover]);
        let mut manage = look_and_type.clone();
        manage.extend([
            S::ProcessSpawn,
            S::TopologyLayout,
            S::TopologyName,
            S::Terminate,
            S::ProcessRestore,
        ]);
        assert_eq!(AccessSet::Level(AccessLevel::Look).scopes(), look);
        assert_eq!(
            AccessSet::Level(AccessLevel::LookAndType).scopes(),
            look_and_type
        );
        assert_eq!(AccessSet::Level(AccessLevel::Manage).scopes(), manage);
        // An exact bundle is named; anything else is Custom, never rounded up.
        for level in AccessLevel::ALL {
            assert_eq!(
                AccessSet::from_scopes(level.scopes().iter().copied()),
                Ok(AccessSet::Level(level))
            );
        }
        let mut almost = look.clone();
        almost.remove(&S::AgentStatusReport);
        assert_eq!(
            AccessSet::from_scopes(almost.clone()),
            Ok(AccessSet::Custom(almost))
        );
        let mut more = look.clone();
        more.insert(S::Input);
        assert_eq!(
            AccessSet::from_scopes(more.clone()),
            Ok(AccessSet::Custom(more))
        );
        // Clipboard scopes are invalid, never Custom.
        for clipboard in [S::ClipboardRead, S::ClipboardWrite] {
            let mut with_clipboard = look.clone();
            with_clipboard.insert(clipboard);
            assert_eq!(
                AccessSet::from_scopes(with_clipboard),
                Err(InvalidAccess::Clipboard)
            );
        }
        assert_eq!(AccessSet::from_scopes([]), Err(InvalidAccess::Empty));
        // Takeover is durable only inside a level.
        assert!(!AccessSet::Level(AccessLevel::LookAndType).is_custom_takeover());
        assert!(AccessSet::Custom(scopes(&[S::Observe, S::ControlTakeover])).is_custom_takeover());
        assert!(!AccessSet::Custom(scopes(&[S::Observe, S::Input])).is_custom_takeover());
    }

    #[test]
    fn each_set_covers_exactly_its_own_scopes() {
        let every = [
            AccessScope::Observe,
            AccessScope::Scrollback,
            AccessScope::Input,
            AccessScope::Resize,
            AccessScope::Terminate,
            AccessScope::ControlTakeover,
            AccessScope::TopologyObserve,
            AccessScope::TopologyLayout,
            AccessScope::TopologyName,
            AccessScope::ProcessSpawn,
            AccessScope::ProcessRestore,
            AccessScope::AgentStatusReport,
            AccessScope::ClipboardRead,
            AccessScope::ClipboardWrite,
        ];
        let custom = AccessSet::Custom(scopes(&[AccessScope::Observe, AccessScope::Input]));
        for scope in every {
            for level in AccessLevel::ALL {
                assert_eq!(
                    AccessSet::Level(level).covers(&[scope]),
                    level.scopes().contains(&scope),
                    "{level:?} {scope:?}"
                );
            }
            assert_eq!(
                custom.covers(&[scope]),
                matches!(scope, AccessScope::Observe | AccessScope::Input),
                "{scope:?}"
            );
        }
        assert!(custom.covers(&[AccessScope::Observe, AccessScope::Input]));
        assert!(!custom.covers(&[AccessScope::Observe, AccessScope::Resize]));
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one walk through the ADR 0014 eligibility table"
    )]
    fn offered_durations_follow_the_eligibility_table() {
        let manage = AccessSet::Level(AccessLevel::Manage);
        let offer = |peer: &PeerIdentity, access: &AccessSet| offered_durations(peer, access, true);
        // Without a pinned executable: just this once.
        let unpinned = PeerIdentity::for_test();
        assert_eq!(
            offer(&unpinned, &manage),
            DurationOffer {
                workspace_session: Err(Withheld::NoPinnedIdentity),
                remembered: Err(Withheld::NoPinnedIdentity),
            }
        );
        assert_eq!(
            offer(&unpinned, &manage).allows(GrantDuration::Once),
            Ok(())
        );
        // A protected install gets all three.
        let installed = agent("/usr/bin/agent");
        assert_eq!(
            offer(&installed, &manage),
            DurationOffer {
                workspace_session: Ok(()),
                remembered: Ok(protected("/usr/bin/agent")),
            }
        );
        // Custom with takeover: just this once, even for a protected install.
        let takeover = AccessSet::Custom(scopes(&[
            AccessScope::Observe,
            AccessScope::ControlTakeover,
        ]));
        assert_eq!(
            offer(&installed, &takeover),
            DurationOffer {
                workspace_session: Err(Withheld::CustomTakeover),
                remembered: Err(Withheld::CustomTakeover),
            }
        );
        // Takeover inside a level stays durable.
        assert!(
            offer(&installed, &AccessSet::Level(AccessLevel::LookAndType))
                .remembered
                .is_ok()
        );
        // The installed relay: just this once.
        let mut evidence = DurableEvidence::for_test(Some("/usr/bin/splinterm-relay"), &[], false);
        evidence.installed_relay = true;
        let relay =
            PeerIdentity::for_test_executing(executable("/usr/bin/splinterm-relay", 'b'), evidence);
        assert_eq!(
            offer(&relay, &manage),
            DurationOffer {
                workspace_session: Err(Withheld::Relay),
                remembered: Err(Withheld::Relay),
            }
        );
        // A known interpreter: no "Always", even when protected.
        let interpreter = PeerIdentity::for_test_executing(
            executable("/usr/bin/python3.12", 'c'),
            DurableEvidence::for_test(Some("/usr/bin/python3.12"), &[], true),
        );
        assert_eq!(
            offer(&interpreter, &manage),
            DurationOffer {
                workspace_session: Ok(()),
                remembered: Err(Withheld::Interpreter),
            }
        );
        // A development build (no durable identity): no "Always".
        let development = PeerIdentity::for_test_executing(
            executable("/home/u/src/target/debug/agent", 'd'),
            DurableEvidence::for_test(None, &[], false),
        );
        assert_eq!(
            offer(&development, &manage),
            DurationOffer {
                workspace_session: Ok(()),
                remembered: Err(Withheld::NoDurableIdentity),
            }
        );
        // A Nix profile entry: the first matching entry is stored.
        let nix = PeerIdentity::for_test_executing(
            executable("/nix/store/x-agent/bin/agent", 'e'),
            DurableEvidence::for_test(
                None,
                &[
                    "/run/current-system/sw/bin/agent",
                    "/home/u/.nix-profile/bin/agent",
                ],
                false,
            ),
        );
        assert_eq!(
            offer(&nix, &manage).remembered,
            Ok(DurableIdentity::NixProfile(PathBuf::from(
                "/run/current-system/sw/bin/agent"
            )))
        );
        // An unavailable store offers no "Always".
        assert_eq!(
            offered_durations(&installed, &manage, false).remembered,
            Err(Withheld::StoreUnavailable)
        );
        for why in [
            Withheld::NoPinnedIdentity,
            Withheld::CustomTakeover,
            Withheld::Relay,
            Withheld::Interpreter,
            Withheld::NoDurableIdentity,
            Withheld::StoreUnavailable,
        ] {
            assert!(!why.reason().is_empty());
        }
    }

    // ---- Store ----

    #[test]
    fn a_missing_store_grants_nothing_and_the_first_commit_creates_it_privately() {
        let base = Base::without_state_directory();
        let access = base.load();
        assert!(access.store_available());
        let lair = LairId::new();
        let peer = agent("/usr/bin/agent");
        assert_eq!(
            access.authorize(&peer, lair, &[AccessScope::Observe], NOW),
            None
        );
        let committed = access
            .commit_remembered(
                protected("/usr/bin/agent"),
                lair,
                look(),
                Some("Agent".into()),
            )
            .unwrap();
        let entry_id = committed.entry_id.clone().unwrap();
        assert!(entry_id.is_valid());
        let directory = fs::metadata(base.path.join(STATE_DIRECTORY)).unwrap();
        assert_eq!(directory.mode() & 0o7777, 0o700);
        let file = fs::symlink_metadata(base.store()).unwrap();
        assert!(file.is_file());
        assert_eq!(file.mode() & 0o7777, 0o600);
        assert_eq!(file.nlink(), 1);
        let document = base.document();
        assert_eq!(document["format"], STORE_FORMAT);
        assert_eq!(document["version"], CURRENT_STORE_VERSION);
        assert_eq!(document["entries"][0]["id"], entry_id.0);
        assert_eq!(
            document["entries"][0]["access"],
            serde_json::json!({"level": "look"})
        );
        assert_eq!(
            document["entries"][0]["identity"],
            serde_json::json!({"kind": "protected_install", "path": "/usr/bin/agent"})
        );
        // No temporary file is left behind.
        assert_eq!(
            fs::read_dir(base.path.join(STATE_DIRECTORY))
                .unwrap()
                .count(),
            1
        );
        let authorized = access
            .authorize(&peer, lair, &[AccessScope::Observe], NOW)
            .unwrap();
        assert_eq!(authorized.grant_id, committed.grant_id);
        assert_eq!(authorized.duration, GrantDuration::Remembered);
    }

    fn valid_entry(id: &str, lair: LairId) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "identity": {"kind": "protected_install", "path": "/usr/bin/agent"},
            "lair_id": lair,
            "access": {"level": "look"},
            "created_unix_seconds": 1,
            "last_used_unix_seconds": null,
            "display_name": "Agent",
        })
    }

    fn valid_document(entries: &[serde_json::Value]) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "format": STORE_FORMAT,
            "version": 1,
            "entries": entries,
        }))
        .unwrap()
    }

    fn entry_id(index: usize) -> String {
        format!("{ENTRY_ID_PREFIX}{index:032x}")
    }

    /// Every rejected store denies only the remembered source, is never
    /// rewritten, and leaves workspace-session grants working.
    fn assert_rejected(base: &Base, describe: &str) {
        let before = fs::symlink_metadata(base.store())
            .ok()
            .map(|_| fs::read(base.store()).ok());
        let access = base.load();
        assert!(!access.store_available(), "{describe}");
        let lair = LairId::new();
        let peer = agent("/usr/bin/agent");
        assert_eq!(
            access.authorize(&peer, lair, &[AccessScope::Observe], NOW),
            None,
            "{describe}"
        );
        assert!(
            matches!(
                access.commit_remembered(protected("/usr/bin/agent"), lair, look(), None),
                Err(CommitError::Store(StoreWriteError::Unavailable(_)))
            ),
            "{describe}"
        );
        assert!(
            access.flush().is_ok() || !access.flush_needed(),
            "{describe}"
        );
        let after = fs::symlink_metadata(base.store())
            .ok()
            .map(|_| fs::read(base.store()).ok());
        assert_eq!(
            before, after,
            "{describe}: the rejected store was rewritten"
        );
        // Another source still works.
        access.commit_session(&peer, lair, look(), None).unwrap();
        assert!(
            access
                .authorize(&peer, lair, &[AccessScope::Observe], NOW)
                .is_some()
        );
        assert_eq!(
            offered_durations(&peer, &look(), access.store_available()).remembered,
            Err(Withheld::StoreUnavailable),
            "{describe}"
        );
        assert!(
            access
                .audit_for_test()
                .iter()
                .any(|record| record.event == AgentAuditEvent::StoreError),
            "{describe}"
        );
    }

    #[test]
    fn unsafe_files_and_directories_deny_only_the_remembered_source() {
        let lair = LairId::new();
        let good = valid_document(&[valid_entry(&entry_id(1), lair)]);

        let base = Base::new();
        base.write(&good);
        fs::set_permissions(base.store(), fs::Permissions::from_mode(0o644)).unwrap();
        assert_rejected(&base, "mode 0644");

        let base = Base::new();
        base.write(&good);
        fs::set_permissions(base.store(), fs::Permissions::from_mode(0o400)).unwrap();
        assert_rejected(&base, "mode 0400");

        let base = Base::new();
        let elsewhere = base.path.join("elsewhere.json");
        fs::write(&elsewhere, &good).unwrap();
        fs::set_permissions(&elsewhere, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&elsewhere, base.store()).unwrap();
        assert_rejected(&base, "store symlink");
        assert_eq!(fs::read(&elsewhere).unwrap(), good);

        let base = Base::new();
        base.write(&good);
        fs::hard_link(base.store(), base.path.join("second-link")).unwrap();
        assert_rejected(&base, "hard link");

        let base = Base::new();
        fs::set_permissions(
            base.path.join(STATE_DIRECTORY),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        base.write(&good);
        assert_rejected(&base, "directory mode 0755");

        let base = Base::new();
        let real = base.path.join("real-state");
        fs::rename(base.path.join(STATE_DIRECTORY), &real).unwrap();
        symlink(&real, base.path.join(STATE_DIRECTORY)).unwrap();
        fs::write(real.join(STORE_FILE), &good).unwrap();
        fs::set_permissions(real.join(STORE_FILE), fs::Permissions::from_mode(0o600)).unwrap();
        assert_rejected(&base, "state directory symlink");

        let base = Base::new();
        fs::create_dir(base.store()).unwrap();
        assert_rejected(&base, "store is a directory");
        fs::remove_dir(base.store()).unwrap();
    }

    #[test]
    fn the_state_base_itself_may_pass_through_a_link() {
        let base = Base::new();
        let lair = LairId::new();
        base.write(&valid_document(&[valid_entry(&entry_id(1), lair)]));
        let linked = base.path.with_extension("link");
        symlink(&base.path, &linked).unwrap();
        let access = AgentAccess::load(
            Arc::new(GrantIds::default()),
            Some(StoreLocation::new(linked.clone())),
            &context(),
            &topology_with(&[lair]),
            &BTreeSet::new(),
        );
        assert!(access.store_available());
        assert!(
            access
                .authorize(&agent("/usr/bin/agent"), lair, &[AccessScope::Observe], NOW)
                .is_some()
        );
        fs::remove_file(linked).unwrap();
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one table of rejected store contents"
    )]
    fn corrupt_oversized_and_invalid_contents_deny_only_the_remembered_source() {
        let lair = LairId::new();
        let mut cases: Vec<(&str, Vec<u8>)> = vec![
            ("truncated JSON", b"{\"format\": ".to_vec()),
            ("empty", Vec::new()),
            (
                "unknown format",
                serde_json::to_vec(
                    &serde_json::json!({"format": "other", "version": 1, "entries": []}),
                )
                .unwrap(),
            ),
            (
                "unknown field",
                serde_json::to_vec(&serde_json::json!({
                    "format": STORE_FORMAT, "version": 1, "entries": [], "extra": true
                }))
                .unwrap(),
            ),
            (
                "duplicate entry ID",
                valid_document(&[valid_entry(&entry_id(1), lair), {
                    let mut other = valid_entry(&entry_id(1), LairId::new());
                    other["identity"]["path"] = "/usr/bin/other".into();
                    other
                }]),
            ),
            (
                "duplicate identity and workspace",
                valid_document(&[
                    valid_entry(&entry_id(1), lair),
                    valid_entry(&entry_id(2), lair),
                ]),
            ),
            (
                "invalid entry ID",
                valid_document(&[valid_entry("ra_xyz", lair)]),
            ),
            (
                "clipboard scope",
                valid_document(&[{
                    let mut entry = valid_entry(&entry_id(1), lair);
                    entry["access"] = serde_json::json!({"custom": ["observe", "clipboard_read"]});
                    entry
                }]),
            ),
            (
                "relative path",
                valid_document(&[{
                    let mut entry = valid_entry(&entry_id(1), lair);
                    entry["identity"]["path"] = "usr/bin/agent".into();
                    entry
                }]),
            ),
            (
                "non-canonical path",
                valid_document(&[{
                    let mut entry = valid_entry(&entry_id(1), lair);
                    entry["identity"]["path"] = "/usr/bin/../bin/agent".into();
                    entry
                }]),
            ),
            (
                "Nix path outside the list",
                valid_document(&[{
                    let mut entry = valid_entry(&entry_id(1), lair);
                    entry["identity"] =
                        serde_json::json!({"kind": "nix_profile", "path": "/home/u/bin/agent"});
                    entry
                }]),
            ),
            (
                "control character in name",
                valid_document(&[{
                    let mut entry = valid_entry(&entry_id(1), lair);
                    entry["display_name"] = "bad\u{1b}[2J".into();
                    entry
                }]),
            ),
            (
                "too many entries",
                valid_document(
                    &(0..=MAX_REMEMBERED_ENTRIES)
                        .map(|index| valid_entry(&entry_id(index), LairId::new()))
                        .collect::<Vec<_>>(),
                ),
            ),
        ];
        let mut oversized = valid_document(&[valid_entry(&entry_id(1), lair)]);
        oversized.extend(std::iter::repeat_n(
            b' ',
            usize::try_from(MAX_STORE_BYTES).unwrap(),
        ));
        cases.push(("oversized", oversized));
        for (describe, bytes) in cases {
            let base = Base::new();
            base.write(&bytes);
            assert_rejected(&base, describe);
        }
        // A Nix profile entry from the list for this account loads.
        let base = Base::new();
        base.write(&valid_document(&[{
            let mut entry = valid_entry(&entry_id(1), lair);
            entry["identity"] =
                serde_json::json!({"kind": "nix_profile", "path": "/home/u/.nix-profile/bin/agent"});
            entry
        }]));
        assert!(base.load().store_available());
    }

    #[test]
    fn a_newer_version_is_unavailable_and_never_rewritten() {
        let base = Base::new();
        let lair = LairId::new();
        let newer = serde_json::to_vec(&serde_json::json!({
            "format": STORE_FORMAT,
            "version": 2,
            "entries": [valid_entry(&entry_id(1), lair)],
            "future_field": {"anything": true},
        }))
        .unwrap();
        base.write(&newer);
        let access = base.load();
        assert!(!access.store_available());
        assert!(matches!(
            access.audit_for_test().last().map(|record| record.reason.clone()),
            Some(reason) if reason.contains("unsupported store version 2")
        ));
        assert_rejected(&base, "newer version");
        assert_eq!(base.read(), newer);
    }

    #[test]
    fn a_generation_writes_only_the_version_it_loaded() {
        let file = DurableFile {
            location: None,
            state: FileState::Loaded { version: 1 },
            fingerprint: None,
            injected_failures: 0,
            fail_after_rename: false,
        };
        assert_eq!(file.version_to_write(), Ok(1));
        let absent = DurableFile {
            state: FileState::Absent,
            ..DurableFile::unconfigured()
        };
        assert_eq!(absent.version_to_write(), Ok(CURRENT_STORE_VERSION));
        assert!(matches!(
            DurableFile {
                state: FileState::Unavailable(StoreFault::UnsupportedVersion(9)),
                ..DurableFile::unconfigured()
            }
            .version_to_write(),
            Err(StoreWriteError::Unavailable(
                StoreFault::UnsupportedVersion(9)
            ))
        ));
        // Loaded at version 1, a write keeps version 1.
        let base = Base::new();
        base.write(&valid_document(&[valid_entry(&entry_id(1), LairId::new())]));
        let access = base.load();
        access
            .commit_remembered(protected("/usr/bin/other"), LairId::new(), look(), None)
            .unwrap();
        assert_eq!(base.document()["version"], 1);
        assert_eq!(base.document()["entries"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn an_outside_change_before_a_write_is_refused_and_reported() {
        let base = Base::new();
        let access = base.load();
        let lair = LairId::new();
        let first = access
            .commit_remembered(protected("/usr/bin/agent"), lair, look(), None)
            .unwrap();
        // Something else rewrites the store (a new inode, same bytes).
        let outside = base.read();
        let staged = base.path.join(STATE_DIRECTORY).join("outside");
        fs::write(&staged, &outside).unwrap();
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o600)).unwrap();
        fs::rename(&staged, base.store()).unwrap();
        let second = access.commit_remembered(protected("/usr/bin/other"), lair, look(), None);
        assert_eq!(second, Err(CommitError::Store(StoreWriteError::Conflict)));
        // Nothing was granted, the outside file stands, and it is reported.
        assert_eq!(base.read(), outside);
        assert_eq!(
            access.authorize(&agent("/usr/bin/other"), lair, &[AccessScope::Observe], NOW),
            None
        );
        assert!(
            access
                .authorize(&agent("/usr/bin/agent"), lair, &[AccessScope::Observe], NOW)
                .is_some_and(|authorized| authorized.grant_id == first.grant_id)
        );
        assert!(
            access
                .audit_for_test()
                .iter()
                .any(|record| record.event == AgentAuditEvent::StoreError
                    && record.reason.contains("changed by something else"))
        );
        // An in-place edit (same inode, new size and time) is refused too.
        let base = Base::new();
        let access = base.load();
        access
            .commit_remembered(protected("/usr/bin/agent"), lair, look(), None)
            .unwrap();
        let mut edited = base.read();
        edited.extend(b"\n\n");
        fs::write(base.store(), &edited).unwrap();
        assert_eq!(
            access.commit_remembered(protected("/usr/bin/other"), lair, look(), None),
            Err(CommitError::Store(StoreWriteError::Conflict))
        );
        // A store created by something else after a missing-store load.
        let base = Base::new();
        let access = base.load();
        base.write(&valid_document(&[]));
        assert_eq!(
            access.commit_remembered(protected("/usr/bin/agent"), lair, look(), None),
            Err(CommitError::Store(StoreWriteError::Conflict))
        );
    }

    #[test]
    fn a_failed_write_grants_nothing() {
        let base = Base::new();
        let access = base.load();
        let lair = LairId::new();
        access.inject_write_failures(1);
        let result = access.commit_remembered(protected("/usr/bin/agent"), lair, look(), None);
        assert!(matches!(
            result,
            Err(CommitError::Store(StoreWriteError::Io {
                stage: "injected",
                ..
            }))
        ));
        assert!(access.remembered_for_test().is_empty());
        assert_eq!(
            access.authorize(&agent("/usr/bin/agent"), lair, &[AccessScope::Observe], NOW),
            None
        );
        assert!(!base.store().exists());
        assert!(
            access
                .audit_for_test()
                .iter()
                .any(|record| record.event == AgentAuditEvent::StoreError)
        );
        // Bounds: a store already at its entry limit refuses one more.
        let base = Base::new();
        base.write(&valid_document(
            &(0..MAX_REMEMBERED_ENTRIES)
                .map(|index| {
                    let mut entry = valid_entry(&entry_id(index), LairId::new());
                    entry["display_name"] = serde_json::Value::Null;
                    entry
                })
                .collect::<Vec<_>>(),
        ));
        let full = base.load();
        assert!(full.store_available());
        let before = base.read();
        assert_eq!(
            full.commit_remembered(protected("/usr/bin/agent"), lair, look(), None),
            Err(CommitError::Store(StoreWriteError::Bounds))
        );
        assert_eq!(base.read(), before);
    }

    #[test]
    fn a_commit_that_is_not_durable_grants_nothing_and_owes_its_removal() {
        let base = Base::new();
        let access = base.load();
        let lair = LairId::new();
        access.inject_failure_after_rename();
        let failed = access.commit_remembered(protected("/usr/bin/agent"), lair, look(), None);
        assert!(matches!(
            failed,
            Err(CommitError::Store(StoreWriteError::NotDurable { .. }))
        ));
        assert_eq!(
            access.authorize(&agent("/usr/bin/agent"), lair, &[AccessScope::Observe], NOW),
            None
        );
        // The renamed file holds the entry, so its removal is owed.
        assert_eq!(base.document()["entries"].as_array().unwrap().len(), 1);
        assert_eq!(access.pending_removal_ids().len(), 1);
        access.flush().unwrap();
        assert_eq!(base.document()["entries"], serde_json::json!([]));
        assert!(base.load().remembered_for_test().is_empty());
    }

    #[test]
    fn a_failed_removal_stays_revoked_and_is_retried() {
        let base = Base::new();
        let access = base.load();
        let lair = LairId::new();
        let peer = agent("/usr/bin/agent");
        let committed = access
            .commit_remembered(protected("/usr/bin/agent"), lair, look(), None)
            .unwrap();
        let entry_id = committed.entry_id.unwrap();
        access.inject_write_failures(1);
        assert_eq!(
            access.revoke_remembered(&entry_id),
            Some(committed.grant_id)
        );
        assert!(access.flush().is_err());
        // Revoked in memory at once, still in the file, and owed.
        assert_eq!(
            access.authorize(&peer, lair, &[AccessScope::Observe], NOW),
            None
        );
        assert_eq!(base.document()["entries"][0]["id"], entry_id.0);
        assert_eq!(
            access.pending_removal_ids(),
            BTreeSet::from([entry_id.clone()])
        );
        assert!(access.flush_needed());
        // The next write of any kind carries the removal.
        access
            .commit_remembered(protected("/usr/bin/other"), lair, look(), None)
            .unwrap();
        let ids: Vec<_> = base.document()["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["id"].as_str().unwrap().to_owned())
            .collect();
        assert!(!ids.contains(&entry_id.0));
        assert!(access.pending_removal_ids().is_empty());
        // And a shutdown flush retries on its own.
        let other = access.remembered_for_test()[0].1.id.clone();
        access.inject_write_failures(1);
        access.revoke_remembered(&other).unwrap();
        assert!(access.flush().is_err());
        assert!(access.flush().is_ok());
        assert_eq!(base.document()["entries"], serde_json::json!([]));
        assert!(!access.flush_needed());
        // A reload sees neither.
        assert!(base.load().remembered_for_test().is_empty());
    }

    #[test]
    fn concurrent_commits_serialize_into_one_consistent_store() {
        let base = Base::new();
        let access = Arc::new(base.load());
        let lairs: Vec<_> = (0..16).map(|_| LairId::new()).collect();
        let threads: Vec<_> = lairs
            .iter()
            .map(|lair| {
                let access = Arc::clone(&access);
                let lair = *lair;
                std::thread::spawn(move || {
                    access.commit_remembered(protected("/usr/bin/agent"), lair, look(), None)
                })
            })
            .collect();
        let committed: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap().unwrap())
            .collect();
        let ids: HashSet<_> = committed.iter().map(|commit| commit.grant_id).collect();
        assert_eq!(ids.len(), 16);
        let reloaded = base.load_with(&topology_with(&lairs), &BTreeSet::new());
        assert_eq!(reloaded.remembered_for_test().len(), 16);
        let peer = agent("/usr/bin/agent");
        for lair in lairs {
            assert!(
                reloaded
                    .authorize(&peer, lair, &[AccessScope::Observe], NOW)
                    .is_some()
            );
        }
    }

    #[test]
    fn always_replaces_its_earlier_definition_exactly() {
        let base = Base::new();
        let access = base.load();
        let lair = LairId::new();
        let peer = agent("/usr/bin/agent");
        let manage = access
            .commit_remembered(
                protected("/usr/bin/agent"),
                lair,
                AccessSet::Level(AccessLevel::Manage),
                None,
            )
            .unwrap();
        assert!(
            access
                .authorize(&peer, lair, &[AccessScope::Terminate], NOW)
                .is_some()
        );
        // A narrower answer replaces it; it never widens or merges.
        let narrowed = access
            .commit_remembered(protected("/usr/bin/agent"), lair, look(), None)
            .unwrap();
        assert_eq!(narrowed.replaced, Some(manage.grant_id));
        assert_eq!(access.remembered_for_test().len(), 1);
        assert_eq!(
            access.authorize(&peer, lair, &[AccessScope::Terminate], NOW),
            None
        );
        assert!(
            access
                .authorize(&peer, lair, &[AccessScope::Observe], NOW)
                .is_some()
        );
        assert_eq!(base.document()["entries"].as_array().unwrap().len(), 1);
        // Another workspace is a separate definition.
        let other = access
            .commit_remembered(protected("/usr/bin/agent"), LairId::new(), look(), None)
            .unwrap();
        assert_eq!(other.replaced, None);
        assert_eq!(access.remembered_for_test().len(), 2);
    }

    #[test]
    fn remembered_definitions_reload_fresh_with_new_runtime_ids() {
        let base = Base::new();
        let lair = LairId::new();
        let first = base.load();
        let committed = first
            .commit_remembered(
                protected("/usr/bin/agent"),
                lair,
                look(),
                Some("Agent".into()),
            )
            .unwrap();
        first
            .commit_session(
                &agent("/usr/bin/agent"),
                lair,
                AccessSet::Level(AccessLevel::Manage),
                None,
            )
            .unwrap();
        drop(first);
        // A new generation: the remembered entry, by its stable ID, under a
        // new runtime ID space; the session grant did not survive.
        let second = base.load_with(&topology_with(&[lair]), &BTreeSet::new());
        let reloaded = second.remembered_for_test();
        assert_eq!(reloaded.len(), 1);
        assert_eq!(Some(&reloaded[0].1.id), committed.entry_id.as_ref());
        assert_eq!(reloaded[0].1.display_name.as_deref(), Some("Agent"));
        assert_eq!(second.session_count_for_test(), 0);
        let peer = agent("/usr/bin/agent");
        assert_eq!(
            second.authorize(&peer, lair, &[AccessScope::Terminate], NOW),
            None
        );
        assert_eq!(
            second
                .authorize(&peer, lair, &[AccessScope::Observe], NOW)
                .map(|authorized| authorized.duration),
            Some(GrantDuration::Remembered)
        );
    }

    #[test]
    fn an_entry_for_an_absent_workspace_is_marked_missing() {
        let base = Base::new();
        let mut topology = Topology::new();
        topology
            .create_lair("present", PathBuf::from("/tmp"))
            .unwrap();
        let present = topology.lairs().next().unwrap().id;
        let absent = LairId::new();
        base.write(&valid_document(&[valid_entry(&entry_id(1), present), {
            let mut entry = valid_entry(&entry_id(2), absent);
            entry["identity"]["path"] = "/usr/bin/other".into();
            entry
        }]));
        let access = base.load_with(&topology, &BTreeSet::new());
        assert_eq!(
            access.workspace_missing_for_test(&EntryId(entry_id(1))),
            Some(false)
        );
        assert_eq!(
            access.workspace_missing_for_test(&EntryId(entry_id(2))),
            Some(true)
        );
        // A missing workspace's entry authorizes nothing, even for its own
        // Lair ID.
        assert!(
            access
                .authorize(
                    &agent("/usr/bin/agent"),
                    present,
                    &[AccessScope::Observe],
                    NOW
                )
                .is_some()
        );
        assert_eq!(
            access.authorize(
                &agent("/usr/bin/other"),
                absent,
                &[AccessScope::Observe],
                NOW
            ),
            None
        );
    }

    /// Review F1: the writer lock covers the durable write and the memory
    /// update, so a flush queued between them cannot snapshot memory without
    /// the new entry and rewrite it away.
    #[test]
    fn a_flush_queued_inside_a_commit_cannot_rewrite_the_commit_away() {
        use std::{sync::mpsc, time::Duration};

        let base = Base::new();
        let access = Arc::new(base.load());
        let lair = LairId::new();
        // An existing entry with an owed last-used write gives the flush work.
        access
            .commit_remembered(protected("/usr/bin/old"), lair, look(), None)
            .unwrap();
        assert!(
            access
                .authorize(&agent("/usr/bin/old"), lair, &[AccessScope::Observe], NOW)
                .unwrap()
                .flush_due
        );
        let (written_tx, written_rx) = mpsc::channel::<()>();
        let (flushed_tx, flushed_rx) = mpsc::channel::<()>();
        access.after_commit_write_for_test(move || {
            written_tx.send(()).unwrap();
            // Without the lock the flush completes here; with it, the flush
            // waits for this commit, and this wait times out.
            let _ = flushed_rx.recv_timeout(Duration::from_millis(500));
        });
        let flusher = {
            let access = Arc::clone(&access);
            std::thread::spawn(move || {
                written_rx.recv().unwrap();
                let flushed = access.flush();
                let _ = flushed_tx.send(());
                flushed
            })
        };
        let committed = access
            .commit_remembered(protected("/usr/bin/new"), lair, look(), None)
            .unwrap();
        flusher.join().unwrap().unwrap();
        let new_id = committed.entry_id.unwrap();
        let ids: Vec<_> = base.document()["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["id"].as_str().unwrap().to_owned())
            .collect();
        assert!(ids.contains(&new_id.0), "{ids:?}");
        assert!(
            base.load()
                .remembered_for_test()
                .iter()
                .any(|(_, entry)| entry.id == new_id)
        );
    }

    /// Review F3: a resumed round retires the previous round's handle, which
    /// then never writes again; its owed removals carry over.
    #[test]
    fn a_retired_handle_never_writes_again() {
        let base = Base::new();
        let access = base.load();
        let lair = LairId::new();
        let committed = access
            .commit_remembered(protected("/usr/bin/agent"), lair, look(), None)
            .unwrap();
        let entry_id = committed.entry_id.unwrap();
        access.inject_write_failures(1);
        access.revoke_remembered(&entry_id).unwrap();
        assert!(access.flush().is_err());
        let owed = access.retire();
        assert_eq!(owed, BTreeSet::from([entry_id]));
        assert!(!access.store_available());
        let before = base.read();
        assert_eq!(
            access.flush(),
            Err(StoreWriteError::Unavailable(StoreFault::Retired))
        );
        assert_eq!(
            access.commit_remembered(protected("/usr/bin/other"), lair, look(), None),
            Err(CommitError::Store(StoreWriteError::Unavailable(
                StoreFault::Retired
            )))
        );
        assert_eq!(base.read(), before);
        // The next round keeps the removal owed and performs it.
        let next = base.load_with(&topology_with(&[lair]), &owed);
        assert_eq!(
            next.authorize(&agent("/usr/bin/agent"), lair, &[AccessScope::Observe], NOW),
            None
        );
        next.flush().unwrap();
        assert_eq!(base.document()["entries"], serde_json::json!([]));
    }

    #[test]
    fn a_load_removes_leftover_temporary_stores_only() {
        let base = Base::new();
        let directory = base.path.join(STATE_DIRECTORY);
        let leftover = directory.join(format!(".{STORE_FILE}.tmp-1-1"));
        let unrelated = directory.join("topology.json");
        let lookalike_directory = directory.join(format!(".{STORE_FILE}.tmp-dir"));
        fs::write(&leftover, b"{").unwrap();
        fs::write(&unrelated, b"{}").unwrap();
        fs::create_dir(&lookalike_directory).unwrap();
        assert!(base.load().store_available());
        assert!(!leftover.exists());
        assert!(unrelated.exists());
        assert!(lookalike_directory.exists());
    }

    #[test]
    fn last_used_is_written_at_most_hourly_because_of_use() {
        let base = Base::new();
        let access = base.load();
        let lair = LairId::new();
        let peer = agent("/usr/bin/agent");
        access
            .commit_remembered(protected("/usr/bin/agent"), lair, look(), None)
            .unwrap();
        let first = access
            .authorize(&peer, lair, &[AccessScope::Observe], NOW)
            .unwrap();
        assert!(first.flush_due);
        // While that write is in flight, no second one is scheduled.
        assert!(
            !access
                .authorize(&peer, lair, &[AccessScope::Observe], NOW + 1)
                .unwrap()
                .flush_due
        );
        access.flush().unwrap();
        assert_eq!(
            base.document()["entries"][0]["last_used_unix_seconds"],
            NOW + 1
        );
        assert!(
            !access
                .authorize(
                    &peer,
                    lair,
                    &[AccessScope::Observe],
                    NOW + LAST_USED_WRITE_INTERVAL_SECONDS
                )
                .unwrap()
                .flush_due
        );
        assert!(
            access
                .authorize(
                    &peer,
                    lair,
                    &[AccessScope::Observe],
                    NOW + 1 + LAST_USED_WRITE_INTERVAL_SECONDS
                )
                .unwrap()
                .flush_due
        );
        // A failed write backs off for an hour of wall time.
        access.inject_write_failures(1);
        assert!(access.flush().is_err());
        let far_future = unix_seconds() + LAST_USED_WRITE_INTERVAL_SECONDS / 2;
        assert!(
            !access
                .authorize(&peer, lair, &[AccessScope::Observe], far_future)
                .unwrap()
                .flush_due
        );
    }

    #[test]
    fn ending_a_workspace_removes_its_grants_of_every_duration() {
        let base = Base::new();
        let access = base.load();
        let lair = LairId::new();
        let other = LairId::new();
        let peer = agent("/usr/bin/agent");
        let remembered = access
            .commit_remembered(protected("/usr/bin/agent"), lair, look(), None)
            .unwrap();
        let kept = access
            .commit_remembered(protected("/usr/bin/agent"), other, look(), None)
            .unwrap();
        let session = access.commit_session(&peer, lair, look(), None).unwrap();
        let ended = access.end_lair(lair, "Lair terminated");
        assert_eq!(
            ended.grant_ids.iter().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from([remembered.grant_id, session.grant_id])
        );
        assert!(ended.flush_needed);
        assert_eq!(
            access.authorize(&peer, lair, &[AccessScope::Observe], NOW),
            None
        );
        access.flush().unwrap();
        let entries = base.document()["entries"].clone();
        assert_eq!(entries.as_array().unwrap().len(), 1);
        assert_eq!(entries[0]["id"], kept.entry_id.unwrap().0);
        // Nothing new attaches to an ended workspace.
        assert_eq!(
            access.commit_remembered(protected("/usr/bin/agent"), lair, look(), None),
            Err(CommitError::WorkspaceEnded)
        );
        assert_eq!(
            access.commit_session(&peer, lair, look(), None),
            Err(CommitError::WorkspaceEnded)
        );
        assert!(
            access
                .audit_for_test()
                .iter()
                .any(|record| record.event == AgentAuditEvent::Revoked
                    && record.reason == "Lair terminated")
        );
    }

    #[test]
    fn carried_removals_stay_revoked_across_a_resumed_round() {
        let base = Base::new();
        let lair = LairId::new();
        base.write(&valid_document(&[valid_entry(&entry_id(1), lair)]));
        let carried = BTreeSet::from([EntryId(entry_id(1))]);
        let access = base.load_with(&topology_with(&[lair]), &carried);
        assert_eq!(
            access.authorize(&agent("/usr/bin/agent"), lair, &[AccessScope::Observe], NOW),
            None
        );
        assert_eq!(access.pending_removal_ids(), carried);
        access.flush().unwrap();
        assert_eq!(base.document()["entries"], serde_json::json!([]));
    }

    #[test]
    fn workspace_session_grants_bind_path_digest_uid_and_lair() {
        let access = AgentAccess::unconfigured(Arc::new(GrantIds::default()));
        let lair = LairId::new();
        let peer = PeerIdentity::for_test_executing(
            executable("/home/u/target/debug/agent", 'a'),
            DurableEvidence::for_test(None, &[], false),
        );
        let committed = access
            .commit_session(&peer, lair, look(), Some("Agent".into()))
            .unwrap();
        // A reconnect or restarted process of the same build matches.
        let mut restarted = peer.clone();
        restarted.pid = peer.pid.saturating_add(7);
        assert_eq!(
            access
                .authorize(&restarted, lair, &[AccessScope::Observe], NOW)
                .map(|authorized| (authorized.grant_id, authorized.duration)),
            Some((committed.grant_id, GrantDuration::WorkspaceSession))
        );
        // A rebuild (new digest), another path, another workspace, or
        // another scope does not.
        let rebuilt = PeerIdentity::for_test_executing(
            executable("/home/u/target/debug/agent", 'b'),
            DurableEvidence::for_test(None, &[], false),
        );
        let moved = PeerIdentity::for_test_executing(
            executable("/home/u/target/release/agent", 'a'),
            DurableEvidence::for_test(None, &[], false),
        );
        assert_eq!(
            access.authorize(&rebuilt, lair, &[AccessScope::Observe], NOW),
            None
        );
        assert_eq!(
            access.authorize(&moved, lair, &[AccessScope::Observe], NOW),
            None
        );
        assert_eq!(
            access.authorize(&peer, LairId::new(), &[AccessScope::Observe], NOW),
            None
        );
        assert_eq!(
            access.authorize(&peer, lair, &[AccessScope::Input], NOW),
            None
        );
        // Without a pinned identity, no session grant can be committed or
        // used.
        let unpinned = PeerIdentity::for_test();
        assert_eq!(
            access.authorize(&unpinned, lair, &[AccessScope::Observe], NOW),
            None
        );
        assert_eq!(
            access.commit_session(&unpinned, lair, look(), None),
            Err(CommitError::NotOffered(Withheld::NoPinnedIdentity))
        );
        // Repeating replaces it exactly; revoking ends it.
        let custom = AccessSet::Custom(scopes(&[AccessScope::Input]));
        let replaced = access.commit_session(&peer, lair, custom, None).unwrap();
        assert_eq!(replaced.replaced, Some(committed.grant_id));
        assert_eq!(
            access.authorize(&peer, lair, &[AccessScope::Observe], NOW),
            None
        );
        assert!(
            access
                .authorize(&peer, lair, &[AccessScope::Input], NOW)
                .is_some()
        );
        assert!(access.revoke_session(replaced.grant_id));
        assert!(!access.revoke_session(replaced.grant_id));
        assert_eq!(
            access.authorize(&peer, lair, &[AccessScope::Input], NOW),
            None
        );
        // No store: "Always" is never possible here.
        assert!(!access.store_available());
    }

    #[test]
    fn sources_are_independent_and_never_combine_scopes() {
        let base = Base::new();
        let access = base.load();
        let lair = LairId::new();
        let peer = agent("/usr/bin/agent");
        access.commit_session(&peer, lair, look(), None).unwrap();
        access
            .commit_remembered(
                protected("/usr/bin/agent"),
                lair,
                AccessSet::Custom(scopes(&[AccessScope::Input])),
                None,
            )
            .unwrap();
        assert_eq!(
            access
                .authorize(&peer, lair, &[AccessScope::Observe], NOW)
                .map(|a| a.duration),
            Some(GrantDuration::WorkspaceSession)
        );
        assert_eq!(
            access
                .authorize(&peer, lair, &[AccessScope::Input], NOW)
                .map(|a| a.duration),
            Some(GrantDuration::Remembered)
        );
        assert_eq!(
            access.authorize(
                &peer,
                lair,
                &[AccessScope::Observe, AccessScope::Input],
                NOW
            ),
            None
        );
    }

    #[test]
    fn a_remembered_identity_matches_only_connections_that_qualify_now() {
        let base = Base::new();
        let access = base.load();
        let lair = LairId::new();
        access
            .commit_remembered(protected("/usr/bin/agent"), lair, look(), None)
            .unwrap();
        access
            .commit_remembered(
                DurableIdentity::NixProfile(PathBuf::from("/home/u/.nix-profile/bin/tool")),
                lair,
                look(),
                None,
            )
            .unwrap();
        // The same path, but this connection's file no longer qualifies
        // (replaced, or now writable): no match.
        let stale = PeerIdentity::for_test_executing(
            executable("/usr/bin/agent", 'a'),
            DurableEvidence::for_test(None, &[], false),
        );
        assert_eq!(
            access.authorize(&stale, lair, &[AccessScope::Observe], NOW),
            None
        );
        // A Nix connection matches through any qualifying entry it resolved.
        let nix = PeerIdentity::for_test_executing(
            executable("/nix/store/x-tool/bin/tool", 'f'),
            DurableEvidence::for_test(
                None,
                &[
                    "/run/current-system/sw/bin/tool",
                    "/home/u/.nix-profile/bin/tool",
                ],
                false,
            ),
        );
        assert!(
            access
                .authorize(&nix, lair, &[AccessScope::Observe], NOW)
                .is_some()
        );
        // The relay never matches, even an entry naming it.
        access
            .commit_remembered(protected("/usr/bin/splinterm-relay"), lair, look(), None)
            .unwrap();
        let mut evidence = DurableEvidence::for_test(Some("/usr/bin/splinterm-relay"), &[], false);
        evidence.installed_relay = true;
        let relay =
            PeerIdentity::for_test_executing(executable("/usr/bin/splinterm-relay", 'r'), evidence);
        assert_eq!(
            access.authorize(&relay, lair, &[AccessScope::Observe], NOW),
            None
        );
    }

    #[test]
    fn revocation_wins_every_race_with_authorization() {
        let base = Base::new();
        let access = Arc::new(base.load());
        let lair = LairId::new();
        let committed = access
            .commit_remembered(protected("/usr/bin/agent"), lair, look(), None)
            .unwrap();
        let entry_id = committed.entry_id.unwrap();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let readers: Vec<_> = (0..4)
            .map(|_| {
                let access = Arc::clone(&access);
                let stop = Arc::clone(&stop);
                std::thread::spawn(move || {
                    let peer = agent("/usr/bin/agent");
                    while !stop.load(Ordering::Relaxed) {
                        let _ = access.authorize(&peer, lair, &[AccessScope::Observe], NOW);
                    }
                })
            })
            .collect();
        // Two revokers race: exactly one revokes.
        let revokers: Vec<_> = (0..2)
            .map(|_| {
                let access = Arc::clone(&access);
                let entry_id = entry_id.clone();
                std::thread::spawn(move || access.revoke_remembered(&entry_id))
            })
            .collect();
        let revoked: Vec<_> = revokers
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(
            revoked.iter().filter(|outcome| outcome.is_some()).count(),
            1
        );
        // Once revoke returned, nothing authorizes again.
        let peer = agent("/usr/bin/agent");
        for _ in 0..100 {
            assert_eq!(
                access.authorize(&peer, lair, &[AccessScope::Observe], NOW),
                None
            );
        }
        stop.store(true, Ordering::Relaxed);
        for reader in readers {
            reader.join().unwrap();
        }
        // A commit racing a revoke of another entry never resurrects it.
        let other = access
            .commit_remembered(protected("/usr/bin/other"), lair, look(), None)
            .unwrap();
        let racing_commit = {
            let access = Arc::clone(&access);
            std::thread::spawn(move || {
                access.commit_remembered(protected("/usr/bin/third"), lair, look(), None)
            })
        };
        access
            .revoke_remembered(other.entry_id.as_ref().unwrap())
            .unwrap();
        racing_commit.join().unwrap().unwrap();
        access.flush().unwrap();
        let reloaded = base.load();
        let ids: Vec<_> = reloaded
            .remembered_for_test()
            .into_iter()
            .map(|(_, entry)| entry.identity)
            .collect();
        assert_eq!(ids, vec![protected("/usr/bin/third")]);
    }

    #[test]
    fn display_names_are_bounded_display_data() {
        let base = Base::new();
        let access = base.load();
        let lair = LairId::new();
        let long = format!("Agent\u{7}{}", "é".repeat(MAX_DISPLAY_NAME_BYTES));
        access
            .commit_remembered(protected("/usr/bin/agent"), lair, look(), Some(long))
            .unwrap();
        let stored = access.remembered_for_test()[0]
            .1
            .display_name
            .clone()
            .unwrap();
        assert!(stored.len() <= MAX_DISPLAY_NAME_BYTES);
        assert!(!stored.chars().any(char::is_control));
        assert!(stored.starts_with("Agent"));
        // The reloaded store accepts what the daemon wrote.
        assert!(base.load().store_available());
    }
}
