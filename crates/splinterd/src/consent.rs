//! Trusted, daemon-owned authorization state and the private consent-client broker.
//!
//! The daemon never creates graphical objects. A one-use random capability and
//! bounded prompt cross only a private inherited socket connected to
//! `splinterm consent`; neither is exposed through argv, environment, or logs.

use std::{
    collections::{BTreeSet, VecDeque},
    fs,
    io::{Read, Write},
    os::{
        fd::{AsFd, AsRawFd, OwnedFd},
        unix::{fs::MetadataExt, net::UnixStream as StdUnixStream},
    },
    path::Path,
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use nix::{
    poll::{PollFd, PollFlags, PollTimeout, poll},
    sys::socket::{getsockopt, sockopt::PeerPidfd},
};
use splinterd::install_prefix::InstallPrefix;
use splinterm_core::{LairId, SplintId};
use splinterm_protocol::{
    AccessGrant, AccessGrantSource, AccessScope, AuditPeer, CONSENT_CAPABILITY_BYTES,
    ConsentPrompt, ConsentReply, ConsentTarget, LairAccessGrant, MAX_ACCESS_SCOPES,
    MAX_CONSENT_FRAME_BYTES,
};
use tokio::{io::unix::AsyncFd, net::UnixStream};

use crate::{
    agent_access::{DurableEvidence, GrantIds, IdentityContext},
    executable_identity::ExecutableIdentity,
};

pub const GRANT_LIFETIME: Duration = Duration::from_secs(5 * 60);
const CONSENT_DEADLINE: Duration = Duration::from_secs(20);
const MAX_AUDIT_RECORDS: usize = 256;

/// The sealed forward client snapshot a daemon adopted from a handoff
/// verified against its package record, held open and pinned for the
/// daemon's lifetime (AGENTS.md §4.4, the operator exception of 2026-10-18;
/// Plan 0061 M3.14).
///
/// A Window relaunched across the adoption executes exactly this file, so
/// its process's executable is this snapshot's device and inode. Holding the
/// descriptor keeps the inode alive, so no other file can take its identity
/// while the daemon runs. Matching is by device and inode only, never by
/// content hash: a different file with the same bytes is not this snapshot.
/// A fresh daemon and a rollback daemon pin nothing.
#[derive(Debug)]
pub struct PinnedClientSnapshot {
    _snapshot: OwnedFd,
    device: u64,
    inode: u64,
}

impl PinnedClientSnapshot {
    /// Pins `snapshot`, which must be a regular file sealed immutable
    /// ([`splinterm_protocol::handoff_prepare::REQUIRED_CLIENT_SNAPSHOT_SEALS`]:
    /// write, grow, shrink, and the seal set itself), so the sealed half of
    /// the §4.4 exception holds at the pin itself.
    ///
    /// # Errors
    /// A descriptor that cannot be examined, is not a regular file, or lacks
    /// a required seal.
    pub fn pin(snapshot: OwnedFd) -> std::io::Result<Self> {
        let status = rustix::fs::fstat(&snapshot)?;
        if rustix::fs::FileType::from_raw_mode(status.st_mode) != rustix::fs::FileType::RegularFile
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the forward client snapshot is not a regular file",
            ));
        }
        // A regular file without sealing support reports EINVAL: not sealed.
        let sealed = rustix::fs::fcntl_get_seals(&snapshot).is_ok_and(|seals| {
            seals.contains(splinterm_protocol::handoff_prepare::REQUIRED_CLIENT_SNAPSHOT_SEALS)
        });
        if !sealed {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the forward client snapshot is not sealed immutable",
            ));
        }
        Ok(Self {
            _snapshot: snapshot,
            device: status.st_dev,
            inode: status.st_ino,
        })
    }

    /// The pinned file's device and inode.
    pub const fn identity(&self) -> (u64, u64) {
        (self.device, self.inode)
    }

    /// This test process's own executable, held open as if it were the
    /// pinned snapshot, so a peer that is this process matches it.
    #[cfg(test)]
    pub fn own_executable_for_test() -> Self {
        let executable = fs::File::open("/proc/self/exe").expect("open the test executable");
        let status = rustix::fs::fstat(&executable).expect("identify the test executable");
        Self {
            _snapshot: OwnedFd::from(executable),
            device: status.st_dev,
            inode: status.st_ino,
        }
    }
}

impl PartialEq for PinnedClientSnapshot {
    fn eq(&self, other: &Self) -> bool {
        self.identity() == other.identity()
    }
}

impl Eq for PinnedClientSnapshot {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerIdentity {
    pub uid: u32,
    pub pid: u32,
    /// The requester label shown in consent dialogs: the kernel's
    /// `/proc/<pid>/exe` link text, marked unless it resolves here to the
    /// executing file. Display only; never an identity.
    executable_label: String,
    /// Device and inode of the file object the peer executes, from the opened
    /// executable link itself.
    executable_device: u64,
    executable_inode: u64,
    /// The daemon's installed-prefix authority, attached at verification.
    /// It is not a property of the peer: adjacency is matched against it.
    install_prefix: Arc<InstallPrefix>,
    /// The forward client snapshot an adopted daemon pinned, attached at
    /// verification; `None` in every other daemon.
    pinned_client: Option<Arc<PinnedClientSnapshot>>,
    persistent_executable: Option<ExecutableIdentity>,
    /// The peer's start time in clock ticks since boot, recorded when the
    /// connection is verified, so a reused PID is never mistaken for it.
    start_time: Option<u64>,
    /// What this connection established about durable identity (ADR 0014),
    /// read while its pidfd pinned it; `None` without a pinned executable.
    durable: Option<DurableEvidence>,
}

impl PeerIdentity {
    pub fn from_stream(
        stream: &UnixStream,
        install_prefix: Arc<InstallPrefix>,
        pinned_client: Option<Arc<PinnedClientSnapshot>>,
    ) -> Result<Self> {
        let credentials = stream.peer_cred().context("cannot read peer credentials")?;
        let pid = credentials.pid().context("peer credentials omit pid")?;
        let pid = u32::try_from(pid).context("peer pid is negative")?;
        let (executable_device, executable_inode) = executable_file_identity(pid)?;
        let link_text =
            fs::read_link(format!("/proc/{pid}/exe")).context("cannot label peer executable")?;
        Ok(Self {
            uid: credentials.uid(),
            pid,
            executable_label: executable_label(&link_text, executable_device, executable_inode),
            executable_device,
            executable_inode,
            install_prefix,
            pinned_client,
            persistent_executable: None,
            start_time: None,
            durable: None,
        })
    }

    pub fn requester_label(&self) -> String {
        self.executable_label.clone()
    }

    #[cfg(test)]
    pub fn for_test() -> Self {
        let executable = std::env::current_exe().expect("test executable");
        let metadata = fs::metadata(&executable).expect("test executable metadata");
        Self {
            uid: rustix::process::geteuid().as_raw(),
            pid: std::process::id(),
            executable_label: executable_label(&executable, metadata.dev(), metadata.ino()),
            executable_device: metadata.dev(),
            executable_inode: metadata.ino(),
            install_prefix: Arc::new(test_install_prefix()),
            pinned_client: None,
            persistent_executable: None,
            start_time: None,
            durable: None,
        }
    }

    /// This test process as a peer executing `executable` (its file by
    /// device and inode) with `durable` evidence, as a pinned connection
    /// verification records it.
    #[cfg(test)]
    pub fn for_test_executing(executable: ExecutableIdentity, durable: DurableEvidence) -> Self {
        let mut peer = Self::for_test();
        peer.executable_device = executable.device;
        peer.executable_inode = executable.inode;
        peer.persistent_executable = Some(executable);
        peer.durable = Some(durable);
        peer
    }

    /// This test process as a peer in a daemon that pinned `pinned`, with
    /// its start time recorded as connection verification records it.
    #[cfg(test)]
    pub fn for_test_pinned(pinned: Arc<PinnedClientSnapshot>) -> Self {
        let mut peer = Self::for_test();
        peer.pinned_client = Some(pinned);
        peer.start_time = splinterd::handoff_window_carry::process_start_time(std::process::id());
        peer
    }

    pub fn install_persistent_executable(&mut self, executable: ExecutableIdentity) {
        self.persistent_executable = Some(executable);
    }

    pub fn persistent_executable(&self) -> Option<&ExecutableIdentity> {
        self.persistent_executable.as_ref()
    }

    /// Records the durable-identity evidence its pinned verification read,
    /// marking whether the peer executes the installed relay.
    pub fn install_durable_evidence(&mut self, mut durable: DurableEvidence) {
        durable.installed_relay = self.is_installed_relay();
        self.durable = Some(durable);
    }

    /// The durable-identity evidence of this connection; `None` without a
    /// pidfd-pinned executable.
    pub fn durable_evidence(&self) -> Option<&DurableEvidence> {
        self.durable.as_ref()
    }

    /// Whether the peer executes the installed `splinterm-relay`, by device
    /// and inode, in any mode. Never eligible for the longer durations.
    pub fn is_installed_relay(&self) -> bool {
        self.matches_installed_executable(&self.install_prefix.graphical_relay())
    }

    pub fn install_start_time(&mut self, start_time: u64) {
        self.start_time = Some(start_time);
    }

    pub const fn start_time(&self) -> Option<u64> {
        self.start_time
    }

    /// Device and inode of the file the peer executed when its connection
    /// was verified. A package upgrade leaves a running client on this inode,
    /// so it keeps naming the image the peer actually runs.
    pub const fn executable_file(&self) -> (u64, u64) {
        (self.executable_device, self.executable_inode)
    }

    pub fn audit_peer(&self) -> Option<AuditPeer> {
        let executable = self.persistent_executable.as_ref()?;
        Some(AuditPeer {
            uid: self.uid,
            executable_path: executable
                .path
                .to_string_lossy()
                .chars()
                .take(4096)
                .collect(),
            executable_sha256: executable.sha256.clone(),
            device: Some(executable.device),
            inode: Some(executable.inode),
        })
    }

    pub fn transfer_peer(&self) -> Option<splinterd::image_transport::TransferPeer> {
        self.persistent_executable.as_ref().map(|executable| {
            splinterd::image_transport::TransferPeer {
                uid: self.uid,
                pid: self.pid,
                executable_device: executable.device,
                executable_inode: executable.inode,
                executable_sha256: executable.sha256.clone(),
            }
        })
    }

    /// Whether the peer is a trusted graphical client under AGENTS.md §4.4:
    /// it executes exactly the file now installed as `<prefix>/splinterm`
    /// (same device and inode), or, in a daemon adopted through a verified
    /// handoff, exactly the sealed forward client snapshot that daemon
    /// verified against its package record and pinned for its lifetime
    /// (same device and inode; the operator exception of 2026-10-18).
    ///
    /// Matching is never by content hash. A fresh or rollback daemon pins no
    /// snapshot, so only the installed rule applies there. A client still
    /// running a replaced or unlinked image, a stale pre-upgrade inode among
    /// them, keeps its old inode and matches neither.
    pub fn is_matching_splinterm(&self) -> bool {
        self.matches_installed_executable(&self.install_prefix.client())
            || self.matches_pinned_client()
    }

    /// Whether the peer executes the pinned forward client snapshot.
    fn matches_pinned_client(&self) -> bool {
        self.pinned_client.as_ref().is_some_and(|pinned| {
            pinned.identity() == (self.executable_device, self.executable_inode)
        })
    }

    pub fn is_matching_graphical_relay(&self) -> bool {
        let executable_matches =
            self.matches_installed_executable(&self.install_prefix.graphical_relay());
        let command_line = fs::read(format!("/proc/{}/cmdline", self.pid));
        let (argument_count, graphical_mode) = command_line.as_ref().map_or((0, false), |value| {
            let arguments = value
                .split(|byte| *byte == 0)
                .filter(|argument| !argument.is_empty())
                .collect::<Vec<_>>();
            (arguments.len(), graphical_relay_arguments(&arguments))
        });
        tracing::debug!(
            peer_pid = self.pid,
            executable_matches,
            command_line_available = command_line.is_ok(),
            argument_count,
            graphical_mode,
            "graphical relay identity evaluated"
        );
        executable_matches && graphical_mode
    }

    // The file installed at `expected` now, by device and inode. The pinned
    // forward client snapshot is matched separately and only for the
    // graphical client ([`Self::matches_pinned_client`]); it never widens
    // the graphical relay's rule.
    fn matches_installed_executable(&self, expected: &Path) -> bool {
        fs::metadata(expected).is_ok_and(|metadata| {
            metadata.dev() == self.executable_device && metadata.ino() == self.executable_inode
        })
    }
}

/// Identifies the file object process `pid` executes: its device and inode.
///
/// The executable link is opened (`O_PATH`) and the descriptor itself is
/// examined, so the kernel resolves the link to the exact executing file even
/// when it is a memfd, unlinked, or replaced on disk. The link's path text is
/// never resolved: it may name nothing, or a different file now installed at
/// that path, or a file in another mount namespace. `O_PATH` needs no read
/// permission on the file, as the `stat` this replaces did not.
///
/// Fails closed when the link cannot be opened or examined, or the executing
/// object is not a regular file.
fn executable_file_identity(pid: u32) -> Result<(u64, u64)> {
    let link = format!("/proc/{pid}/exe");
    let executable = rustix::fs::open(
        link.as_str(),
        rustix::fs::OFlags::PATH | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .context("cannot open peer executable")?;
    let status = rustix::fs::fstat(&executable).context("cannot identify peer executable")?;
    if rustix::fs::FileType::from_raw_mode(status.st_mode) != rustix::fs::FileType::RegularFile {
        bail!("peer executable is not a regular file");
    }
    Ok((status.st_dev, status.st_ino))
}

/// The longest requester label, in characters, markers included.
const MAX_REQUESTER_LABEL_CHARS: usize = 1024;

/// Labels a peer executable for a human from its link text, never presenting
/// an unverified path as fact.
///
/// The text is shown plain only when it resolves, in the daemon's mount
/// namespace, to the executing file's device and inode. Otherwise (a memfd
/// image, a deleted or replaced file, or a path that names another file here,
/// such as a binary in another mount namespace) it carries a marker naming
/// the executing file by device and inode. The marker always survives the
/// length bound.
fn executable_label(link_text: &Path, device: u64, inode: u64) -> String {
    let verified = link_text.is_absolute()
        && fs::metadata(link_text)
            .is_ok_and(|metadata| metadata.dev() == device && metadata.ino() == inode);
    let text = link_text.to_string_lossy();
    let marker = if verified {
        String::new()
    } else {
        let kind = if text.starts_with("/memfd:") {
            "memory image, "
        } else if text.ends_with(" (deleted)") {
            "deleted or replaced file, "
        } else {
            ""
        };
        format!(" [{kind}path unverified; executable device {device} inode {inode}]")
    };
    let budget = MAX_REQUESTER_LABEL_CHARS.saturating_sub(marker.chars().count());
    let mut label = text.chars().take(budget).collect::<String>();
    label.push_str(&marker);
    label
}

/// A prefix for unit tests: the test binary's own directory, which holds no
/// `splinterm`, so no test peer is a trusted client.
#[cfg(test)]
pub fn test_install_prefix() -> InstallPrefix {
    let test_binary = std::env::current_exe().expect("test executable");
    let directory = fs::canonicalize(test_binary.parent().expect("test binary directory"))
        .expect("canonical test binary directory");
    InstallPrefix::from_directory(&directory).expect("test install prefix")
}

fn graphical_relay_arguments(arguments: &[&[u8]]) -> bool {
    arguments.len() == 2 && arguments[1] == b"--graphical-stdio"
}

#[derive(Debug)]
pub struct PeerMonitor {
    pidfd: AsyncFd<OwnedFd>,
}

/// What a pidfd-pinned peer verification established.
#[derive(Debug)]
pub enum PeerPinning {
    /// The kernel predates `SO_PEERPIDFD` (Linux 6.5): nothing pins the peer,
    /// so persistent identity, and with it policy authorization, is
    /// unavailable.
    Unsupported,
    /// The peer was live at a point after every identity read, so its PID was
    /// not reused meanwhile. The persistent snapshot is `Err` when the
    /// executable has no bindable path (a memfd, deleted, or replaced image),
    /// which disables only policy authorization.
    Pinned {
        monitor: PeerMonitor,
        executable: Result<ExecutableIdentity>,
        /// Durable-identity evidence for the executable, read under the same
        /// pin; `None` when the executable has no bindable path.
        durable: Option<DurableEvidence>,
    },
}

impl PeerMonitor {
    /// Pins the connected peer with its `SO_PEERPIDFD` and snapshots its
    /// persistent executable identity.
    ///
    /// Every identity read for this connection, including the earlier
    /// [`PeerIdentity::from_stream`], happened before the final liveness
    /// check, so a live pidfd proves those reads described this peer.
    ///
    /// # Errors
    /// Fails closed, and the caller rejects the connection, when the pidfd
    /// names a different process than `SO_PEERCRED`, cannot be inspected, or
    /// shows the peer has exited (its PID may already be reused), and when
    /// `SO_PEERPIDFD` fails for any reason but being unsupported.
    pub async fn initialize(
        stream: &UnixStream,
        pid: u32,
        identity_context: Arc<IdentityContext>,
    ) -> Result<PeerPinning> {
        let Some(pidfd) = supported_peer_pidfd(getsockopt(stream, PeerPidfd))? else {
            return Ok(PeerPinning::Unsupported);
        };
        verify_pidfd_pid(&pidfd, pid)?;
        require_live_pidfd(&pidfd)?;
        // Durable identity is evaluated afresh for every connection, inside
        // the pin, and never cached for another (ADR 0014).
        let (executable, durable) = tokio::task::spawn_blocking(move || {
            let executable = ExecutableIdentity::from_pid(pid);
            let durable = executable
                .as_ref()
                .ok()
                .map(|executable| identity_context.evaluate(executable));
            (executable, durable)
        })
        .await
        .context("peer executable snapshot task failed")?;
        require_live_pidfd(&pidfd)?;
        Ok(PeerPinning::Pinned {
            monitor: Self {
                pidfd: AsyncFd::new(pidfd).context("cannot monitor peer pidfd")?,
            },
            executable,
            durable,
        })
    }

    pub async fn exited(&self) -> Result<()> {
        let _guard = self
            .pidfd
            .readable()
            .await
            .context("peer pidfd monitor failed")?;
        Ok(())
    }

    /// A second close-on-exec descriptor for the pinned peer process, so a
    /// handoff can keep the binding after this connection's monitor ends
    /// (Plan 0061 M3.13b1). Both name the one process the connection was
    /// verified against; a reused PID matches neither.
    ///
    /// # Errors
    /// Fails when the descriptor cannot be duplicated.
    pub fn duplicate_pidfd(&self) -> Result<OwnedFd> {
        rustix::io::fcntl_dupfd_cloexec(self.pidfd.get_ref(), 0)
            .context("cannot duplicate the peer pidfd")
    }
}

/// Tolerates only an unsupported `SO_PEERPIDFD`: an older kernel answers an
/// unknown socket option with `ENOPROTOOPT`. Any other failure fails closed;
/// on Linux 6.5+ `EINVAL` or `ESRCH` means the peer has no live process left.
fn supported_peer_pidfd(result: nix::Result<OwnedFd>) -> Result<Option<OwnedFd>> {
    match result {
        Ok(pidfd) => Ok(Some(pidfd)),
        Err(nix::errno::Errno::ENOPROTOOPT) => Ok(None),
        Err(error) => Err(error).context("cannot pin the socket peer with SO_PEERPIDFD"),
    }
}

fn verify_pidfd_pid(pidfd: &OwnedFd, expected_pid: u32) -> Result<()> {
    let path = format!("/proc/self/fdinfo/{}", pidfd.as_raw_fd());
    let contents = fs::read_to_string(path).context("cannot inspect peer pidfd")?;
    let actual = contents
        .lines()
        .find_map(|line| line.strip_prefix("Pid:\t"))
        .context("peer pidfd omits process identity")?
        .parse::<u32>()
        .context("peer pidfd process identity is invalid")?;
    if actual != expected_pid {
        bail!("SO_PEERCRED and SO_PEERPIDFD identify different processes");
    }
    Ok(())
}

fn require_live_pidfd(pidfd: &OwnedFd) -> Result<()> {
    let mut descriptors = [PollFd::new(pidfd.as_fd(), PollFlags::POLLIN)];
    let ready = poll(&mut descriptors, PollTimeout::ZERO).context("cannot poll peer pidfd")?;
    if ready != 0
        || descriptors[0]
            .revents()
            .is_some_and(|events| !events.is_empty())
    {
        bail!("peer exited during persistent identity snapshot");
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct Grant {
    id: u64,
    peer: PeerIdentity,
    splint_id: SplintId,
    incarnation: u64,
    scopes: BTreeSet<AccessScope>,
    expires: Instant,
    expires_at_unix_seconds: u64,
}

impl Grant {
    fn wire(&self) -> AccessGrant {
        AccessGrant {
            grant_id: self.id,
            splint_id: self.splint_id,
            incarnation: self.incarnation,
            scopes: self.scopes.iter().copied().collect(),
            requester: self.peer.requester_label(),
            expires_at_unix_seconds: self.expires_at_unix_seconds,
        }
    }
}

#[derive(Clone, Debug)]
struct LairGrant {
    id: u64,
    peer: PeerIdentity,
    lair_id: LairId,
    scopes: BTreeSet<AccessScope>,
    expires: Instant,
    expires_at_unix_seconds: u64,
}

impl LairGrant {
    fn wire(&self) -> LairAccessGrant {
        LairAccessGrant {
            grant_id: self.id,
            source: AccessGrantSource::Ephemeral,
            lair_id: self.lair_id,
            scopes: self.scopes.iter().copied().collect(),
            requester: self.peer.requester_label(),
            expires_at_unix_seconds: self.expires_at_unix_seconds,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GrantResource {
    Splint {
        splint_id: SplintId,
        incarnation: u64,
    },
    Lair {
        lair_id: LairId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditDecision {
    Granted,
    Denied,
    Revoked,
}

#[allow(
    dead_code,
    reason = "bounded audit metadata is retained in memory for future trusted diagnostics"
)]
#[derive(Clone, Debug)]
pub struct AuditRecord {
    pub order: u64,
    pub unix_seconds: u64,
    pub peer_uid: u32,
    pub peer_pid: u32,
    pub requester: String,
    pub lair_id: Option<LairId>,
    pub splint_id: Option<SplintId>,
    pub incarnation: Option<u64>,
    pub scopes: Vec<AccessScope>,
    pub decision: AuditDecision,
    pub reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationMutation {
    pub grant: AccessGrant,
    pub authorization_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LairAuthorizationMutation {
    pub grant: LairAccessGrant,
    pub authorization_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevokedAuthorization {
    Splint(AuthorizationMutation),
    Lair(LairAuthorizationMutation),
}

#[derive(Debug, Default)]
pub struct GrantStore {
    ids: Arc<GrantIds>,
    next_audit_order: u64,
    authorization_revision: u64,
    grants: Vec<Grant>,
    lair_grants: Vec<LairGrant>,
    audit: VecDeque<AuditRecord>,
}

impl GrantStore {
    /// A store allocating runtime grant IDs from the space every authority
    /// source shares.
    pub fn with_ids(ids: Arc<GrantIds>) -> Self {
        Self {
            ids,
            ..Self::default()
        }
    }

    #[must_use]
    pub const fn authorization_revision(&self) -> u64 {
        self.authorization_revision
    }

    fn advance_authorization_revision(&mut self) {
        self.authorization_revision = self.authorization_revision.saturating_add(1);
    }

    pub fn authorize(
        &mut self,
        peer: &PeerIdentity,
        splint_id: SplintId,
        incarnation: u64,
        required: &[AccessScope],
    ) -> Option<u64> {
        self.remove_expired();
        let required: BTreeSet<_> = required.iter().copied().collect();
        self.grants.iter().find_map(|grant| {
            (grant.peer == *peer
                && grant.splint_id == splint_id
                && grant.incarnation == incarnation
                && required.is_subset(&grant.scopes))
            .then_some(grant.id)
        })
    }

    pub fn authorize_lair(
        &mut self,
        peer: &PeerIdentity,
        lair_id: LairId,
        required: &[AccessScope],
    ) -> Option<u64> {
        self.remove_expired();
        let required: BTreeSet<_> = required.iter().copied().collect();
        self.lair_grants.iter().find_map(|grant| {
            (grant.peer == *peer
                && grant.lair_id == lair_id
                && grant.expires > Instant::now()
                && required.is_subset(&grant.scopes))
            .then_some(grant.id)
        })
    }

    pub fn status(&mut self, splint_id: SplintId, incarnation: u64) -> Vec<AccessGrant> {
        self.remove_expired();
        self.grants
            .iter()
            .filter(|grant| grant.splint_id == splint_id && grant.incarnation == incarnation)
            .map(Grant::wire)
            .collect()
    }

    pub fn lair_status(&mut self, lair_id: LairId) -> Vec<LairAccessGrant> {
        self.remove_expired();
        self.lair_grants
            .iter()
            .filter(|grant| grant.lair_id == lair_id && grant.expires > Instant::now())
            .map(LairGrant::wire)
            .collect()
    }

    pub fn grant(
        &mut self,
        peer: &PeerIdentity,
        splint_id: SplintId,
        incarnation: u64,
        scopes: Vec<AccessScope>,
    ) -> AuthorizationMutation {
        self.remove_expired();
        let scopes: BTreeSet<_> = scopes.into_iter().collect();
        if let Some(index) = self.grants.iter().position(|grant| {
            grant.peer == *peer && grant.splint_id == splint_id && grant.incarnation == incarnation
        }) {
            let wire = {
                let grant = &mut self.grants[index];
                grant.scopes.extend(scopes.iter().copied());
                grant.expires = Instant::now() + GRANT_LIFETIME;
                grant.expires_at_unix_seconds = unix_seconds() + GRANT_LIFETIME.as_secs();
                grant.wire()
            };
            self.advance_authorization_revision();
            self.record(
                peer,
                splint_id,
                incarnation,
                scopes,
                AuditDecision::Granted,
                "grant once",
            );
            return AuthorizationMutation {
                grant: wire,
                authorization_revision: self.authorization_revision,
            };
        }
        let grant = Grant {
            id: self.ids.next(),
            peer: peer.clone(),
            splint_id,
            incarnation,
            scopes: scopes.clone(),
            expires: Instant::now() + GRANT_LIFETIME,
            expires_at_unix_seconds: unix_seconds() + GRANT_LIFETIME.as_secs(),
        };
        let wire = grant.wire();
        self.grants.push(grant);
        self.advance_authorization_revision();
        self.record(
            peer,
            splint_id,
            incarnation,
            scopes,
            AuditDecision::Granted,
            "grant once",
        );
        AuthorizationMutation {
            grant: wire,
            authorization_revision: self.authorization_revision,
        }
    }

    pub fn grant_lair(
        &mut self,
        peer: &PeerIdentity,
        lair_id: LairId,
        scopes: Vec<AccessScope>,
    ) -> LairAuthorizationMutation {
        self.remove_expired();
        let scopes: BTreeSet<_> = scopes.into_iter().collect();
        if let Some(index) = self
            .lair_grants
            .iter()
            .position(|grant| grant.peer == *peer && grant.lair_id == lair_id)
        {
            let wire = {
                let grant = &mut self.lair_grants[index];
                grant.scopes.extend(scopes.iter().copied());
                grant.expires = Instant::now() + GRANT_LIFETIME;
                grant.expires_at_unix_seconds = unix_seconds() + GRANT_LIFETIME.as_secs();
                grant.wire()
            };
            self.advance_authorization_revision();
            self.record_lair(
                peer,
                lair_id,
                scopes,
                AuditDecision::Granted,
                "grant Lair once",
            );
            return LairAuthorizationMutation {
                grant: wire,
                authorization_revision: self.authorization_revision,
            };
        }
        let grant = LairGrant {
            id: self.ids.next(),
            peer: peer.clone(),
            lair_id,
            scopes: scopes.clone(),
            expires: Instant::now() + GRANT_LIFETIME,
            expires_at_unix_seconds: unix_seconds() + GRANT_LIFETIME.as_secs(),
        };
        let wire = grant.wire();
        self.lair_grants.push(grant);
        self.advance_authorization_revision();
        self.record_lair(
            peer,
            lair_id,
            scopes,
            AuditDecision::Granted,
            "grant Lair once",
        );
        LairAuthorizationMutation {
            grant: wire,
            authorization_revision: self.authorization_revision,
        }
    }

    pub fn deny_lair(
        &mut self,
        peer: &PeerIdentity,
        lair_id: LairId,
        scopes: &[AccessScope],
        reason: &'static str,
    ) {
        self.record_lair(
            peer,
            lair_id,
            scopes.iter().copied().collect(),
            AuditDecision::Denied,
            reason,
        );
    }

    pub fn deny(
        &mut self,
        peer: &PeerIdentity,
        splint_id: SplintId,
        incarnation: u64,
        scopes: &[AccessScope],
        reason: &'static str,
    ) {
        self.record(
            peer,
            splint_id,
            incarnation,
            scopes.iter().copied().collect(),
            AuditDecision::Denied,
            reason,
        );
    }

    pub fn owns_lair_grant(&self, peer: &PeerIdentity, grant_id: u64) -> bool {
        self.lair_grants.iter().any(|grant| {
            grant.id == grant_id && grant.peer == *peer && grant.expires > Instant::now()
        })
    }

    pub fn grant_resource(&self, grant_id: u64) -> Option<GrantResource> {
        self.grants
            .iter()
            .find(|grant| grant.id == grant_id)
            .map(|grant| GrantResource::Splint {
                splint_id: grant.splint_id,
                incarnation: grant.incarnation,
            })
            .or_else(|| {
                self.lair_grants
                    .iter()
                    .find(|grant| grant.id == grant_id)
                    .map(|grant| GrantResource::Lair {
                        lair_id: grant.lair_id,
                    })
            })
    }

    pub fn revoke(&mut self, grant_id: u64) -> Option<AuthorizationMutation> {
        let index = self.grants.iter().position(|grant| grant.id == grant_id)?;
        let grant = self.grants.remove(index);
        let wire = grant.wire();
        self.advance_authorization_revision();
        self.record(
            &grant.peer,
            grant.splint_id,
            grant.incarnation,
            grant.scopes,
            AuditDecision::Revoked,
            "explicit local revocation",
        );
        Some(AuthorizationMutation {
            grant: wire,
            authorization_revision: self.authorization_revision,
        })
    }

    pub fn revoke_any(&mut self, grant_id: u64) -> Option<RevokedAuthorization> {
        if let Some(revoked) = self.revoke(grant_id) {
            return Some(RevokedAuthorization::Splint(revoked));
        }
        let index = self
            .lair_grants
            .iter()
            .position(|grant| grant.id == grant_id)?;
        let grant = self.lair_grants.remove(index);
        let wire = grant.wire();
        self.advance_authorization_revision();
        self.record_lair(
            &grant.peer,
            grant.lair_id,
            grant.scopes,
            AuditDecision::Revoked,
            "explicit local revocation",
        );
        Some(RevokedAuthorization::Lair(LairAuthorizationMutation {
            grant: wire,
            authorization_revision: self.authorization_revision,
        }))
    }

    pub fn revoke_lair_if_expired(&mut self, grant_id: u64) -> Option<LairAuthorizationMutation> {
        let index = self
            .lair_grants
            .iter()
            .position(|grant| grant.id == grant_id && grant.expires <= Instant::now())?;
        let grant = self.lair_grants.remove(index);
        let wire = grant.wire();
        self.advance_authorization_revision();
        self.record_lair(
            &grant.peer,
            grant.lair_id,
            grant.scopes,
            AuditDecision::Revoked,
            "grant expired",
        );
        Some(LairAuthorizationMutation {
            grant: wire,
            authorization_revision: self.authorization_revision,
        })
    }

    pub fn lair_grant_with_revision(&mut self, grant_id: u64) -> Option<LairAuthorizationMutation> {
        self.remove_expired();
        self.lair_grants
            .iter()
            .find(|grant| grant.id == grant_id)
            .map(|grant| LairAuthorizationMutation {
                grant: grant.wire(),
                authorization_revision: self.authorization_revision,
            })
    }

    pub fn grant_with_revision(&mut self, grant_id: u64) -> Option<AuthorizationMutation> {
        self.remove_expired();
        self.grants
            .iter()
            .find(|grant| grant.id == grant_id)
            .map(|grant| AuthorizationMutation {
                grant: grant.wire(),
                authorization_revision: self.authorization_revision,
            })
    }

    pub fn revoke_lair(&mut self, lair_id: LairId, reason: &'static str) -> Vec<u64> {
        let mut removed = Vec::new();
        self.lair_grants.retain(|grant| {
            if grant.lair_id == lair_id {
                removed.push(grant.clone());
                false
            } else {
                true
            }
        });
        let ids = removed.iter().map(|grant| grant.id).collect();
        if !removed.is_empty() {
            self.advance_authorization_revision();
        }
        for grant in removed {
            self.record_lair(
                &grant.peer,
                lair_id,
                grant.scopes,
                AuditDecision::Revoked,
                reason,
            );
        }
        ids
    }

    pub fn revoke_identity(
        &mut self,
        splint_id: SplintId,
        incarnation: u64,
        reason: &'static str,
    ) -> Vec<u64> {
        let mut removed = Vec::new();
        self.grants.retain(|grant| {
            if grant.splint_id == splint_id && grant.incarnation == incarnation {
                removed.push(grant.clone());
                false
            } else {
                true
            }
        });
        let ids = removed.iter().map(|grant| grant.id).collect();
        if !removed.is_empty() {
            self.advance_authorization_revision();
        }
        for grant in removed {
            self.record(
                &grant.peer,
                splint_id,
                incarnation,
                grant.scopes,
                AuditDecision::Revoked,
                reason,
            );
        }
        ids
    }

    fn remove_expired(&mut self) {
        let previous = self.grants.len();
        self.grants.retain(|grant| grant.expires > Instant::now());
        if self.grants.len() != previous {
            self.advance_authorization_revision();
        }
    }

    fn record(
        &mut self,
        peer: &PeerIdentity,
        splint_id: SplintId,
        incarnation: u64,
        scopes: BTreeSet<AccessScope>,
        decision: AuditDecision,
        reason: &'static str,
    ) {
        self.next_audit_order = self.next_audit_order.saturating_add(1);
        if self.audit.len() == MAX_AUDIT_RECORDS {
            self.audit.pop_front();
        }
        self.audit.push_back(AuditRecord {
            order: self.next_audit_order,
            unix_seconds: unix_seconds(),
            peer_uid: peer.uid,
            peer_pid: peer.pid,
            requester: peer.requester_label(),
            lair_id: None,
            splint_id: Some(splint_id),
            incarnation: Some(incarnation),
            scopes: scopes.into_iter().collect(),
            decision,
            reason,
        });
    }

    fn record_lair(
        &mut self,
        peer: &PeerIdentity,
        lair_id: LairId,
        scopes: BTreeSet<AccessScope>,
        decision: AuditDecision,
        reason: &'static str,
    ) {
        self.next_audit_order = self.next_audit_order.saturating_add(1);
        if self.audit.len() == MAX_AUDIT_RECORDS {
            self.audit.pop_front();
        }
        self.audit.push_back(AuditRecord {
            order: self.next_audit_order,
            unix_seconds: unix_seconds(),
            peer_uid: peer.uid,
            peer_pid: peer.pid,
            requester: peer.requester_label(),
            lair_id: Some(lair_id),
            splint_id: None,
            incarnation: None,
            scopes: scopes.into_iter().collect(),
            decision,
            reason,
        });
    }
}

pub async fn prompt(
    install_prefix: &InstallPrefix,
    peer: &PeerIdentity,
    splint_id: SplintId,
    incarnation: u64,
    scopes: Vec<AccessScope>,
) -> Result<bool> {
    prompt_target(
        install_prefix,
        peer,
        ConsentTarget::Splint {
            splint_id,
            incarnation,
        },
        scopes,
    )
    .await
}

pub async fn prompt_lair(
    install_prefix: &InstallPrefix,
    peer: &PeerIdentity,
    lair_id: LairId,
    lair_name: String,
    scopes: Vec<AccessScope>,
) -> Result<bool> {
    prompt_target(
        install_prefix,
        peer,
        ConsentTarget::Lair { lair_id, lair_name },
        scopes,
    )
    .await
}

async fn prompt_target(
    install_prefix: &InstallPrefix,
    peer: &PeerIdentity,
    target: ConsentTarget,
    scopes: Vec<AccessScope>,
) -> Result<bool> {
    if scopes.is_empty() || scopes.len() > MAX_ACCESS_SCOPES {
        bail!("invalid consent scope count");
    }
    let peer = peer.clone();
    let client = install_prefix.client();
    tokio::task::spawn_blocking(move || prompt_blocking(&client, &peer, target, scopes))
        .await
        .context("consent broker task failed")?
}

/// Launches the trusted consent client, `<prefix>/splinterm`, from the
/// daemon's installed-prefix authority, never from a path derived from the
/// running daemon image.
fn prompt_blocking(
    executable: &Path,
    peer: &PeerIdentity,
    target: ConsentTarget,
    scopes: Vec<AccessScope>,
) -> Result<bool> {
    let mut capability = vec![0_u8; CONSENT_CAPABILITY_BYTES];
    let mut offset = 0;
    while offset < capability.len() {
        offset += rustix::rand::getrandom(
            &mut capability[offset..],
            rustix::rand::GetRandomFlags::empty(),
        )
        .context("OS randomness unavailable")?;
    }
    let (mut daemon, child) = StdUnixStream::pair().context("create private consent socket")?;
    daemon.set_read_timeout(Some(CONSENT_DEADLINE))?;
    daemon.set_write_timeout(Some(CONSENT_DEADLINE))?;
    let child_read = child
        .try_clone()
        .context("duplicate private consent descriptor")?;
    // The child inherits this thread's signal mask (`std::process::Command`
    // passes it on). No daemon thread that can reach here holds a blocked
    // mask: a handoff candidate releases the signals its exec carried before
    // it builds its runtime (`handoff_exec`, "Signals across exec").
    let mut process = Command::new(executable)
        .arg("consent")
        .stdin(Stdio::from(OwnedFd::from(child_read)))
        .stdout(Stdio::from(OwnedFd::from(child)))
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("launch trusted consent client {}", executable.display()))?;
    let prompt = ConsentPrompt {
        capability: capability.clone(),
        requester: peer.requester_label(),
        requester_pid: peer.pid,
        requester_uid: peer.uid,
        target,
        scopes,
    };
    write_bounded(&mut daemon, &prompt)?;
    let reply: ConsentReply = match read_bounded(&mut daemon) {
        Ok(reply) => reply,
        Err(error) => {
            let _ = process.kill();
            let _ = process.wait();
            return Err(error);
        }
    };
    let status = process.wait().context("wait for consent client")?;
    if !status.success() || reply.capability != capability {
        bail!("consent client authentication failed");
    }
    Ok(reply.granted)
}

pub fn read_bounded<T: serde::de::DeserializeOwned>(reader: &mut impl Read) -> Result<T> {
    let mut length = [0_u8; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_CONSENT_FRAME_BYTES {
        bail!("invalid consent frame length");
    }
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).context("invalid consent frame")
}

pub fn write_bounded<T: serde::Serialize>(writer: &mut impl Write, value: &T) -> Result<()> {
    let body = serde_json::to_vec(value).context("encode consent frame")?;
    if body.is_empty() || body.len() > MAX_CONSENT_FRAME_BYTES {
        bail!("consent frame exceeds bound");
    }
    writer.write_all(&u32::try_from(body.len())?.to_be_bytes())?;
    writer.write_all(&body)?;
    writer.flush()?;
    Ok(())
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[tokio::test]
    async fn peer_pidfd_matches_credentials_and_guards_executable_snapshot() {
        let (first, _second) = UnixStream::pair().unwrap();
        let credentials = first.peer_cred().unwrap();
        let pid = u32::try_from(credentials.pid().unwrap()).unwrap();

        let PeerPinning::Pinned {
            monitor: _monitor,
            executable,
            durable,
        } = PeerMonitor::initialize(&first, pid, Arc::new(IdentityContext::for_daemon()))
            .await
            .unwrap()
        else {
            panic!("this kernel supports SO_PEERPIDFD");
        };
        let executable = executable.unwrap();

        assert_eq!(pid, std::process::id());
        assert!(executable.path.is_absolute());
        assert_eq!(executable.sha256.len(), 64);
        // Durable evidence is read under the same pin. The test binary is a
        // build output, never root-owned, so it has no durable identity.
        let durable = durable.expect("evidence for a bindable executable");
        if rustix::process::geteuid().as_raw() != 0 {
            assert_eq!(durable.preferred(), None);
        }
    }

    /// A child process the test started and owns, killed and reaped on drop.
    struct OwnedChild(std::process::Child);

    impl OwnedChild {
        fn pid(&self) -> u32 {
            self.0.id()
        }
    }

    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn spawn_sleep(program: &Path) -> OwnedChild {
        // A sibling test thread's fork can briefly inherit the descriptor
        // that wrote a fresh copy, so its exec may see ETXTBSY.
        for _ in 0..100 {
            match Command::new(program).arg("30").spawn() {
                Ok(child) => return OwnedChild(child),
                Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("spawn sleeping child: {error}"),
            }
        }
        panic!("spawn sleeping child: executable stayed busy");
    }

    /// A sealed memfd holding a copy of `program`, as a handoff seals the
    /// forward client snapshot.
    fn sealed_copy(program: &Path) -> OwnedFd {
        let memfd = rustix::fs::memfd_create(
            "splinterm",
            rustix::fs::MemfdFlags::CLOEXEC
                | rustix::fs::MemfdFlags::ALLOW_SEALING
                | rustix::fs::MemfdFlags::EXEC,
        )
        .unwrap();
        let mut image = fs::File::from(memfd);
        image.write_all(&fs::read(program).unwrap()).unwrap();
        let memfd = OwnedFd::from(image);
        rustix::fs::fcntl_add_seals(
            &memfd,
            rustix::fs::SealFlags::WRITE
                | rustix::fs::SealFlags::GROW
                | rustix::fs::SealFlags::SHRINK
                | rustix::fs::SealFlags::SEAL,
        )
        .unwrap();
        memfd
    }

    /// AGENTS.md §4.4, the 2026-10-18 exception: an adopted daemon trusts a
    /// client executing exactly the snapshot it pinned, by device and inode;
    /// a daemon that pinned nothing does not, and neither does a different
    /// file with the same bytes or a stale installed inode.
    #[test]
    fn only_the_exact_pinned_snapshot_inode_is_trusted() {
        let snapshot = sealed_copy(system_sleep());
        let child = spawn_sleep(Path::new(&format!(
            "/proc/self/fd/{}",
            snapshot.as_raw_fd()
        )));
        let pinned = Arc::new(PinnedClientSnapshot::pin(snapshot.try_clone().unwrap()).unwrap());
        let stat = rustix::fs::fstat(&snapshot).unwrap();
        assert_eq!(pinned.identity(), (stat.st_dev, stat.st_ino));

        let prefix = test_install_prefix();
        // A fresh or rollback daemon pins nothing: the relaunched client is
        // not trusted there.
        let fresh = peer_for(child.pid(), &prefix);
        assert!(!fresh.is_matching_splinterm());
        // The adopted daemon that pinned this snapshot trusts it.
        let adopted = PeerIdentity {
            pinned_client: Some(Arc::clone(&pinned)),
            ..fresh.clone()
        };
        assert!(adopted.is_matching_splinterm());
        // The pin never widens the graphical relay's rule.
        assert!(!adopted.is_matching_graphical_relay());
        // Same bytes, another file: never matched by content.
        let twin = Arc::new(PinnedClientSnapshot::pin(sealed_copy(system_sleep())).unwrap());
        assert_ne!(twin.identity(), pinned.identity());
        let other = PeerIdentity {
            pinned_client: Some(twin),
            ..fresh.clone()
        };
        assert!(!other.is_matching_splinterm());

        // A client still running a stale installed inode matches neither
        // rule, even in a daemon that pinned a snapshot.
        let scratch = scratch_prefix();
        let client = scratch.client();
        install_copy(system_sleep(), &client);
        let stale_child = spawn_sleep(&client);
        install_copy(system_sleep(), &client);
        let stale = PeerIdentity {
            pinned_client: Some(Arc::clone(&pinned)),
            ..peer_for(stale_child.pid(), &scratch)
        };
        assert!(!stale.is_matching_splinterm());
        drop(stale_child);
        fs::remove_dir_all(scratch.directory()).unwrap();

        // Only a sealed regular file can be pinned.
        let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
        assert!(PinnedClientSnapshot::pin(OwnedFd::from(socket)).is_err());
        let unsealed = rustix::fs::memfd_create(
            "splinterm",
            rustix::fs::MemfdFlags::CLOEXEC | rustix::fs::MemfdFlags::ALLOW_SEALING,
        )
        .unwrap();
        rustix::io::write(&unsealed, b"\x7fELF").unwrap();
        assert!(PinnedClientSnapshot::pin(unsealed).is_err());
        // Sealed against writes only: the extent and seal set may change.
        let partial = rustix::fs::memfd_create(
            "splinterm",
            rustix::fs::MemfdFlags::CLOEXEC | rustix::fs::MemfdFlags::ALLOW_SEALING,
        )
        .unwrap();
        rustix::io::write(&partial, b"\x7fELF").unwrap();
        rustix::fs::fcntl_add_seals(&partial, rustix::fs::SealFlags::WRITE).unwrap();
        assert!(PinnedClientSnapshot::pin(partial).is_err());
        // An ordinary file cannot carry seals at all.
        let file = fs::File::open(system_sleep()).unwrap();
        assert!(PinnedClientSnapshot::pin(OwnedFd::from(file)).is_err());
        drop(child);
    }

    fn system_sleep() -> &'static Path {
        ["/usr/bin/sleep", "/bin/sleep"]
            .into_iter()
            .map(Path::new)
            .find(|path| path.is_file())
            .expect("a sleep executable")
    }

    fn scratch_prefix() -> InstallPrefix {
        let directory = std::env::temp_dir().join(format!(
            "splinterd-peer-identity-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        InstallPrefix::from_directory(&fs::canonicalize(&directory).unwrap()).unwrap()
    }

    fn install_copy(source: &Path, destination: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let staged = destination.with_extension("staged");
        fs::copy(source, &staged).unwrap();
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755)).unwrap();
        // Replace by rename, as a package upgrade does: a new inode.
        fs::rename(&staged, destination).unwrap();
    }

    fn peer_for(pid: u32, install_prefix: &InstallPrefix) -> PeerIdentity {
        let (executable_device, executable_inode) = executable_file_identity(pid).unwrap();
        PeerIdentity {
            pid,
            executable_label: executable_label(
                &fs::read_link(format!("/proc/{pid}/exe")).unwrap(),
                executable_device,
                executable_inode,
            ),
            executable_device,
            executable_inode,
            install_prefix: Arc::new(install_prefix.clone()),
            ..PeerIdentity::for_test()
        }
    }

    #[test]
    fn trusted_client_identity_is_the_executing_inode_not_its_path() {
        let prefix = scratch_prefix();
        let client = prefix.client();
        install_copy(system_sleep(), &client);
        let original = fs::metadata(&client).unwrap();
        let child = spawn_sleep(&client);

        let peer = peer_for(child.pid(), &prefix);
        assert_eq!(
            (peer.executable_device, peer.executable_inode),
            (original.dev(), original.ino())
        );
        assert!(peer.is_matching_splinterm());
        assert!(!peer.is_matching_graphical_relay());
        assert_eq!(peer.requester_label(), client.to_string_lossy());

        // An upgrade replaces the installed client: the running client keeps
        // its old inode, is still identified by it, and no longer matches.
        install_copy(system_sleep(), &client);
        let replaced = fs::metadata(&client).unwrap();
        assert_ne!(replaced.ino(), original.ino());
        let stale = peer_for(child.pid(), &prefix);
        assert_eq!(
            stale.requester_label(),
            format!(
                "{} (deleted) [deleted or replaced file, path unverified; executable device {} inode {}]",
                client.display(),
                original.dev(),
                original.ino()
            )
        );
        assert_eq!(
            (stale.executable_device, stale.executable_inode),
            (original.dev(), original.ino())
        );
        assert!(!stale.is_matching_splinterm());
        assert!(!peer.is_matching_splinterm());

        // With nothing installed at the path, nothing matches.
        fs::remove_file(&client).unwrap();
        assert!(!peer_for(child.pid(), &prefix).is_matching_splinterm());
        drop(child);
        fs::remove_dir_all(prefix.directory()).unwrap();
    }

    #[test]
    fn a_memfd_executable_is_identified_through_its_descriptor() {
        let memfd = rustix::fs::memfd_create("splinterm", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
        let mut image = fs::File::from(memfd);
        image.write_all(&fs::read(system_sleep()).unwrap()).unwrap();
        let expected = image.metadata().unwrap();
        // exec opens the image before close-on-exec descriptors are closed.
        let child = spawn_sleep(Path::new(&format!("/proc/self/fd/{}", image.as_raw_fd())));

        let label = fs::read_link(format!("/proc/{}/exe", child.pid())).unwrap();
        assert!(label.to_string_lossy().starts_with("/memfd:splinterm"));
        assert_eq!(
            executable_file_identity(child.pid()).unwrap(),
            (expected.dev(), expected.ino())
        );
        let peer = peer_for(child.pid(), &test_install_prefix());
        assert!(!peer.is_matching_splinterm());
        assert!(
            peer.requester_label().ends_with(&format!(
                " (deleted) [memory image, path unverified; executable device {} inode {}]",
                expected.dev(),
                expected.ino()
            )),
            "{}",
            peer.requester_label()
        );
    }

    #[test]
    fn peer_identity_fails_closed_without_an_executing_process() {
        // Above every Linux pid_max: no such process.
        assert!(executable_file_identity(u32::MAX).is_err());
    }

    #[test]
    fn consent_labels_mark_every_path_that_does_not_name_the_executing_file() {
        let running = fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
        let identity = fs::metadata(&running).unwrap();
        let (device, inode) = (identity.dev(), identity.ino());
        let marker = format!(" [path unverified; executable device {device} inode {inode}]");

        // Resolves here to the executing file: shown plain.
        assert_eq!(
            executable_label(&running, device, inode),
            running.to_string_lossy()
        );
        // Resolves here to a different file, as a binary at the same path in
        // another mount namespace would.
        let elsewhere = running.parent().unwrap();
        assert_eq!(
            executable_label(elsewhere, device, inode),
            format!("{}{marker}", elsewhere.display())
        );
        // Resolves nowhere, or relative.
        assert_eq!(
            executable_label(Path::new("/nonexistent/splinterm"), device, inode),
            format!("/nonexistent/splinterm{marker}")
        );
        assert_eq!(
            executable_label(Path::new("splinterm"), device, inode),
            format!("splinterm{marker}")
        );
        // The marker survives the length bound.
        let long = PathBuf::from(format!("/{}", "a".repeat(2 * MAX_REQUESTER_LABEL_CHARS)));
        let label = executable_label(&long, device, inode);
        assert_eq!(label.chars().count(), MAX_REQUESTER_LABEL_CHARS);
        assert!(label.ends_with(&marker));
    }

    #[test]
    fn pidfd_pinning_tolerates_only_an_unsupported_socket_option() {
        use nix::errno::Errno;

        assert!(matches!(
            supported_peer_pidfd(Err(Errno::ENOPROTOOPT)),
            Ok(None)
        ));
        for refused in [Errno::EINVAL, Errno::ESRCH, Errno::ENODATA, Errno::EMFILE] {
            assert!(supported_peer_pidfd(Err(refused)).is_err(), "{refused}");
        }
    }

    #[test]
    fn a_pidfd_for_another_or_exited_process_fails_closed() {
        let mut child = Command::new(system_sleep()).arg("30").spawn().unwrap();
        let child_pid = child.id();
        let pidfd = rustix::process::pidfd_open(
            rustix::process::Pid::from_raw(i32::try_from(child_pid).unwrap()).unwrap(),
            rustix::process::PidfdFlags::empty(),
        )
        .unwrap();

        verify_pidfd_pid(&pidfd, child_pid).unwrap();
        require_live_pidfd(&pidfd).unwrap();
        // SO_PEERCRED naming another process than the pidfd.
        assert!(verify_pidfd_pid(&pidfd, std::process::id()).is_err());

        child.kill().unwrap();
        child.wait().unwrap();
        // Exited and reaped: its PID may already belong to another process.
        assert!(require_live_pidfd(&pidfd).is_err());
        assert!(verify_pidfd_pid(&pidfd, child_pid).is_err());
    }

    #[test]
    fn only_exact_graphical_relay_mode_can_carry_the_human_role() {
        assert!(graphical_relay_arguments(&[
            b"/usr/bin/splinterm-relay",
            b"--graphical-stdio",
        ]));
        assert!(!graphical_relay_arguments(&[
            b"/usr/bin/splinterm-relay",
            b"--stdio",
        ]));
        assert!(!graphical_relay_arguments(&[
            b"/usr/bin/splinterm-relay",
            b"--graphical-stdio",
            b"extra",
        ]));
    }

    #[test]
    fn grants_are_bound_to_peer_splint_incarnation_and_scope() {
        let peer = PeerIdentity::for_test();
        let other_peer = PeerIdentity {
            pid: peer.pid.saturating_add(1),
            ..peer.clone()
        };
        let splint = SplintId::new();
        let other_splint = SplintId::new();
        let mut store = GrantStore::default();
        let grant = store.grant(
            &peer,
            splint,
            7,
            vec![AccessScope::Observe, AccessScope::Input],
        );

        assert_eq!(
            store.authorize(&peer, splint, 7, &[AccessScope::Observe]),
            Some(grant.grant.grant_id)
        );
        assert_eq!(
            store.authorize(&peer, splint, 7, &[AccessScope::Resize]),
            None
        );
        assert_eq!(
            store.authorize(&other_peer, splint, 7, &[AccessScope::Observe]),
            None
        );
        assert_eq!(
            store.authorize(&peer, other_splint, 7, &[AccessScope::Observe]),
            None
        );
        assert_eq!(
            store.authorize(&peer, splint, 8, &[AccessScope::Observe]),
            None
        );
    }

    #[test]
    fn lair_grants_are_peer_bound_dynamic_and_unrelated_lairs_fail_closed() {
        let peer = PeerIdentity::for_test();
        let other_peer = PeerIdentity {
            pid: peer.pid.saturating_add(1),
            ..peer.clone()
        };
        let lair_id = LairId::new();
        let other_lair = LairId::new();
        let mut store = GrantStore::default();
        let grant = store.grant_lair(
            &peer,
            lair_id,
            vec![AccessScope::Input, AccessScope::TopologyLayout],
        );

        assert_eq!(
            store.authorize_lair(&peer, lair_id, &[AccessScope::Input]),
            Some(grant.grant.grant_id)
        );
        assert_eq!(
            store.authorize_lair(&peer, lair_id, &[AccessScope::Resize]),
            None
        );
        assert_eq!(
            store.authorize_lair(&other_peer, lair_id, &[AccessScope::Input]),
            None
        );
        assert_eq!(
            store.authorize_lair(&peer, other_lair, &[AccessScope::Input]),
            None
        );

        let revoked = store.revoke_any(grant.grant.grant_id).unwrap();
        assert!(matches!(revoked, RevokedAuthorization::Lair(_)));
        assert_eq!(
            store.authorize_lair(&peer, lair_id, &[AccessScope::Input]),
            None
        );
    }

    #[test]
    fn expired_lair_grant_is_revoked_with_its_exact_identity() {
        let peer = PeerIdentity::for_test();
        let lair_id = LairId::new();
        let mut store = GrantStore::default();
        let grant = store.grant_lair(&peer, lair_id, vec![AccessScope::Input]);
        store.lair_grants[0].expires = Instant::now();

        let revoked = store.revoke_lair_if_expired(grant.grant.grant_id).unwrap();
        assert_eq!(revoked.grant, grant.grant);
        assert_eq!(revoked.authorization_revision, 2);
        assert!(store.lair_grants.is_empty());
    }

    #[test]
    fn grant_and_revoke_return_the_exact_atomic_authorization_revision() {
        let peer = PeerIdentity::for_test();
        let splint = SplintId::new();
        let mut store = GrantStore::default();
        let first = store.grant(&peer, splint, 1, vec![AccessScope::Observe]);
        assert_eq!(first.authorization_revision, 1);
        let second = store.grant(&peer, splint, 1, vec![AccessScope::Resize]);
        assert_eq!(second.authorization_revision, 2);
        assert_eq!(second.grant.grant_id, first.grant.grant_id);
        let revoked = store.revoke(first.grant.grant_id).unwrap();
        assert_eq!(revoked.authorization_revision, 3);
        assert_eq!(revoked.grant, second.grant);
    }

    #[test]
    fn revocation_removes_authority_and_records_bounded_metadata() {
        let peer = PeerIdentity::for_test();
        let splint = SplintId::new();
        let mut store = GrantStore::default();
        let grant = store.grant(&peer, splint, 1, vec![AccessScope::Observe]);
        assert!(store.revoke(grant.grant.grant_id).is_some());
        assert_eq!(
            store.authorize(&peer, splint, 1, &[AccessScope::Observe]),
            None
        );
        assert_eq!(store.audit.len(), 2);
        assert!(
            store
                .audit
                .iter()
                .all(|record| record.requester.len() <= 1024)
        );
        assert!(
            store
                .audit
                .iter()
                .all(|record| record.scopes.len() <= MAX_ACCESS_SCOPES)
        );
    }

    #[test]
    fn private_frames_reject_zero_and_oversized_lengths() {
        let mut zero = &0_u32.to_be_bytes()[..];
        assert!(read_bounded::<ConsentReply>(&mut zero).is_err());
        let oversized = u32::try_from(MAX_CONSENT_FRAME_BYTES + 1)
            .unwrap()
            .to_be_bytes();
        assert!(read_bounded::<ConsentReply>(&mut &oversized[..]).is_err());
    }
}
