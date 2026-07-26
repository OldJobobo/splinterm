mod audit;
mod consent;
mod persistence;

use std::{
    collections::HashMap,
    env,
    ffi::OsString,
    io::{ErrorKind, IoSlice},
    os::{
        fd::AsFd,
        unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use consent::{GrantStore, PeerIdentity};
use persistence::MetadataStore;
use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags, sendmsg};
use splinterd::{
    LiveError, LiveEvent, LiveScrollbackPage, LiveSearchPage, LiveSnapshot, LiveSplintConfig,
    LiveSplintHandle, LiveSplintRuntime, ProcessExit, ProcessIncarnation, Subscription,
    SubscriptionReceive, authorization, executable_identity,
    image_transport::{TransferAdmission, TransferAdmissionError, sealed_image_memfd},
    policy,
};
use splinterm_core::{
    Dojo, DojoId, Lair, LairDocument, LairError, LayoutNode, Splint, SplintId,
    SplintLaunchMetadata, SplintState, TopologyRevision, Window, WindowId,
};
use splinterm_protocol::{
    AccessGrant, AccessScope, ActiveScreen as WireActiveScreen, CellAttributes, ClientFrame,
    ClientRole, ColorSource, ControlStatus, ControlTransferDecision, ControlTransferOutcome,
    ErrorCode, HistoryTransition, ImageTransferMode, MAX_COLUMNS, MAX_FRAME_BYTES,
    MAX_IMAGE_BYTES_PER_DAEMON, MAX_INPUT_BYTES, MAX_ROWS, MAX_SCROLLBACK_PAGE_ROWS,
    MAX_SEARCH_CURSOR_BYTES, MAX_SEARCH_QUERY_BYTES, MAX_SEARCH_RESULTS,
    MAX_SNAPSHOT_SCROLLBACK_ROWS, MAX_SUBSCRIPTIONS, MouseTracking as WireMouseTracking,
    PROTOCOL_VERSION, ProcessExitStatus, ProtocolError, Request, Response, RestoreLeafResult,
    ScrollDirection as WireScrollDirection, ScrollbackPage as WireScrollbackPage,
    SearchMatch as WireSearchMatch, SearchPage as WireSearchPage, ServerFrame, ServerLimits,
    SplintLifecycle, SplintRuntimeSummary, SubscriptionEvent, TerminalCell, TerminalCursor,
    TerminalInputModes, TerminalProvenance, TerminalRow, TerminalRowPatch, TerminalScroll,
    TerminalScrollbackUpdate, TerminalSnapshot, TerminalUpdate as WireTerminalUpdate,
    TopologyChange, TopologyChangeKind, TopologySnapshot, UnderlineStyle as WireUnderlineStyle,
    encode_frame, image_content_socket_path,
};
use splinterm_pty::{LinuxPtyBackend, PtyCommand, PtySize, default_shell};
use splinterm_terminal::{
    ActiveScreen, ColorSource as TerminalColorSource, DEFAULT_KITTY_UPLOAD_BYTES_PER_DAEMON,
    ScrollDirection, SharedImageBudget, SharedKittyUploadBudget, TerminalDamage, TerminalUpdate,
};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        UnixListener, UnixStream,
        unix::{OwnedReadHalf, OwnedWriteHalf},
    },
    signal,
    sync::{Mutex, Notify, RwLock, Semaphore, broadcast, mpsc},
    task::JoinHandle,
    time,
};
use tokio_util::task::TaskTracker;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const CONNECTION_LIMIT: usize = 32;
const IMAGE_CONTENT_CONNECTION_LIMIT: usize = 8;
const IMAGE_CONTENT_IO_TIMEOUT: Duration = Duration::from_secs(5);
const IMAGE_CONTENT_HEADER_BYTES: usize = 53;
const IMAGE_MEMFD_HEADER_BYTES: usize = 45;
const MAX_LIVE_SPLINTS: usize = 256;
const TOPOLOGY_QUEUE: usize = 16;
const OUTBOUND_QUEUE: usize = 32;
const CONTROL_QUEUE: usize = 4;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const CONTROL_TRANSFER_TIMEOUT: Duration = Duration::from_secs(15);
const EXIT_OBSERVER_TIMEOUT: Duration = Duration::from_secs(10);
const CONTROL_EVENT_QUEUE: usize = 32;
const SEARCH_DEADLINE: Duration = Duration::from_millis(10);
static NEXT_SUBSCRIPTION: AtomicU64 = AtomicU64::new(1);
static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(1);

struct AbortOnDrop {
    task: Option<JoinHandle<()>>,
}

impl AbortOnDrop {
    fn new(task: JoinHandle<()>) -> Self {
        Self { task: Some(task) }
    }

    fn abort(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }

    async fn join(mut self) {
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

struct ActiveImageTransferGuard {
    state: Arc<DaemonState>,
    transfer_id: Option<u64>,
}

impl ActiveImageTransferGuard {
    fn new(state: Arc<DaemonState>, transfer_id: u64) -> Self {
        Self {
            state,
            transfer_id: Some(transfer_id),
        }
    }

    async fn finish(mut self) -> Result<(), TransferAdmissionError> {
        let transfer_id = self
            .transfer_id
            .expect("active image transfer guard finishes once");
        let result = self.state.image_transfers.lock().await.finish(transfer_id);
        self.transfer_id = None;
        result
    }
}

impl Drop for ActiveImageTransferGuard {
    fn drop(&mut self) {
        let Some(transfer_id) = self.transfer_id.take() else {
            return;
        };
        let state = Arc::clone(&self.state);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = state.image_transfers.lock().await.finish(transfer_id);
            });
        }
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.abort();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ControllerLease {
    id: u64,
    connection_id: u64,
    splint_id: SplintId,
    incarnation: u64,
    grant_id: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingControlTransfer {
    id: u64,
    owner_connection_id: u64,
    requester_connection_id: u64,
    splint_id: SplintId,
    incarnation: u64,
}

#[derive(Clone, Copy, Debug)]
enum ControlNotice {
    Status {
        splint_id: SplintId,
        incarnation: u64,
        owner_connection_id: Option<u64>,
    },
    TransferRequested(PendingControlTransfer),
    TransferResolved {
        transfer: PendingControlTransfer,
        outcome: ControlTransferOutcome,
        controller_id: Option<u64>,
    },
}

#[derive(Debug)]
struct ControllerState {
    next_id: u64,
    next_transfer_id: u64,
    by_id: HashMap<u64, ControllerLease>,
    by_splint: HashMap<SplintId, u64>,
    by_connection: HashMap<u64, u64>,
    transfers: HashMap<u64, PendingControlTransfer>,
    transfer_by_splint: HashMap<SplintId, u64>,
}

impl Default for ControllerState {
    fn default() -> Self {
        Self {
            next_id: 1,
            next_transfer_id: 1,
            by_id: HashMap::new(),
            by_splint: HashMap::new(),
            by_connection: HashMap::new(),
            transfers: HashMap::new(),
            transfer_by_splint: HashMap::new(),
        }
    }
}

impl ControllerState {
    fn acquire(
        &mut self,
        connection_id: u64,
        splint_id: SplintId,
        incarnation: u64,
        grant_id: Option<u64>,
    ) -> Result<ControllerLease, ProtocolError> {
        if self.by_connection.contains_key(&connection_id) {
            return Err(ProtocolError::new(
                ErrorCode::ControllerUnavailable,
                "connection already owns a controller lease",
            ));
        }
        if self.by_splint.contains_key(&splint_id) {
            return Err(ProtocolError::new(
                ErrorCode::ControllerUnavailable,
                "live Splint already has a controller",
            ));
        }
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).ok_or_else(|| {
            ProtocolError::new(ErrorCode::ResourceLimit, "controller ID space exhausted")
        })?;
        let lease = ControllerLease {
            id,
            connection_id,
            splint_id,
            incarnation,
            grant_id,
        };
        self.by_id.insert(id, lease);
        self.by_splint.insert(splint_id, id);
        self.by_connection.insert(connection_id, id);
        Ok(lease)
    }

    fn authorize(
        &self,
        connection_id: u64,
        controller_id: u64,
        splint_id: SplintId,
        incarnation: u64,
    ) -> Result<(), ProtocolError> {
        match self.by_id.get(&controller_id) {
            Some(lease)
                if lease.connection_id == connection_id
                    && lease.splint_id == splint_id
                    && lease.incarnation == incarnation =>
            {
                Ok(())
            }
            _ => Err(ProtocolError::new(
                ErrorCode::Unauthorized,
                "controller lease is not owned by this connection",
            )),
        }
    }

    fn status(&self, connection_id: u64, splint_id: SplintId, incarnation: u64) -> ControlStatus {
        let lease = self
            .by_splint
            .get(&splint_id)
            .and_then(|id| self.by_id.get(id))
            .filter(|lease| lease.incarnation == incarnation);
        ControlStatus {
            splint_id,
            incarnation,
            controlled: lease.is_some(),
            locally_owned: lease.is_some_and(|lease| lease.connection_id == connection_id),
        }
    }

    fn release(&mut self, controller_id: u64) -> Option<ControllerLease> {
        let lease = self.by_id.remove(&controller_id)?;
        self.by_splint.remove(&lease.splint_id);
        self.by_connection.remove(&lease.connection_id);
        Some(lease)
    }

    fn release_owned(&mut self, connection_id: u64, controller_id: u64) -> Option<ControllerLease> {
        (self.by_connection.get(&connection_id) == Some(&controller_id))
            .then(|| self.release(controller_id))
            .flatten()
    }

    fn release_connection(&mut self, connection_id: u64) -> Option<ControllerLease> {
        let controller_id = self.by_connection.get(&connection_id).copied()?;
        self.release(controller_id)
    }

    fn request_transfer(
        &mut self,
        requester_connection_id: u64,
        splint_id: SplintId,
        incarnation: u64,
    ) -> Result<PendingControlTransfer, ProtocolError> {
        if self.by_connection.contains_key(&requester_connection_id)
            || self.transfer_by_splint.contains_key(&splint_id)
        {
            return Err(ProtocolError::new(
                ErrorCode::ControlTransferUnavailable,
                "control transfer is unavailable",
            ));
        }
        let owner = self
            .by_splint
            .get(&splint_id)
            .and_then(|id| self.by_id.get(id))
            .filter(|lease| lease.incarnation == incarnation)
            .ok_or_else(|| {
                ProtocolError::new(
                    ErrorCode::ControlTransferUnavailable,
                    "live Splint has no matching controller",
                )
            })?;
        let id = self.next_transfer_id;
        self.next_transfer_id = self.next_transfer_id.checked_add(1).ok_or_else(|| {
            ProtocolError::new(
                ErrorCode::ResourceLimit,
                "control transfer ID space exhausted",
            )
        })?;
        let transfer = PendingControlTransfer {
            id,
            owner_connection_id: owner.connection_id,
            requester_connection_id,
            splint_id,
            incarnation,
        };
        self.transfers.insert(id, transfer);
        self.transfer_by_splint.insert(splint_id, id);
        Ok(transfer)
    }

    fn take_transfer(
        &mut self,
        connection_id: u64,
        transfer_id: u64,
    ) -> Result<PendingControlTransfer, ProtocolError> {
        let transfer = self.transfers.get(&transfer_id).copied().ok_or_else(|| {
            ProtocolError::new(ErrorCode::RequestNotFound, "control transfer not found")
        })?;
        if transfer.owner_connection_id != connection_id {
            return Err(ProtocolError::new(
                ErrorCode::Unauthorized,
                "only the current controller may decide a transfer",
            ));
        }
        self.transfers.remove(&transfer_id);
        self.transfer_by_splint.remove(&transfer.splint_id);
        Ok(transfer)
    }

    fn expire_transfer(&mut self, transfer_id: u64) -> Option<PendingControlTransfer> {
        let transfer = self.transfers.remove(&transfer_id)?;
        self.transfer_by_splint.remove(&transfer.splint_id);
        Some(transfer)
    }

    fn decide_transfer(
        &mut self,
        connection_id: u64,
        transfer_id: u64,
        decision: ControlTransferDecision,
    ) -> Result<
        (
            PendingControlTransfer,
            ControlTransferOutcome,
            Option<ControllerLease>,
        ),
        ProtocolError,
    > {
        let transfer = self.take_transfer(connection_id, transfer_id)?;
        if decision == ControlTransferDecision::Deny {
            return Ok((transfer, ControlTransferOutcome::Denied, None));
        }
        let current = self
            .by_splint
            .get(&transfer.splint_id)
            .and_then(|id| self.by_id.get(id))
            .copied();
        if current.is_none_or(|lease| {
            lease.connection_id != transfer.owner_connection_id
                || lease.incarnation != transfer.incarnation
        }) || self
            .by_connection
            .contains_key(&transfer.requester_connection_id)
        {
            return Ok((transfer, ControlTransferOutcome::Cancelled, None));
        }
        self.next_id.checked_add(1).ok_or_else(|| {
            ProtocolError::new(ErrorCode::ResourceLimit, "controller ID space exhausted")
        })?;
        self.release(current.expect("matching controller checked").id);
        let lease = self.acquire(
            transfer.requester_connection_id,
            transfer.splint_id,
            transfer.incarnation,
            None,
        )?;
        Ok((transfer, ControlTransferOutcome::Granted, Some(lease)))
    }

    fn force_transfer(
        &mut self,
        requester_connection_id: u64,
        splint_id: SplintId,
        incarnation: u64,
    ) -> Result<ControllerLease, ProtocolError> {
        if self.by_connection.contains_key(&requester_connection_id) {
            return Err(ProtocolError::new(
                ErrorCode::ControllerUnavailable,
                "connection already owns a controller lease",
            ));
        }
        let current = self
            .by_splint
            .get(&splint_id)
            .and_then(|id| self.by_id.get(id))
            .copied()
            .filter(|lease| lease.incarnation == incarnation)
            .ok_or_else(|| {
                ProtocolError::new(
                    ErrorCode::ControlTransferUnavailable,
                    "live Splint has no matching controller",
                )
            })?;
        self.next_id.checked_add(1).ok_or_else(|| {
            ProtocolError::new(ErrorCode::ResourceLimit, "controller ID space exhausted")
        })?;
        self.release(current.id);
        self.acquire(requester_connection_id, splint_id, incarnation, None)
    }

    fn cancel_connection_transfers(&mut self, connection_id: u64) -> Vec<PendingControlTransfer> {
        let ids: Vec<_> = self
            .transfers
            .values()
            .filter(|transfer| {
                transfer.owner_connection_id == connection_id
                    || transfer.requester_connection_id == connection_id
            })
            .map(|transfer| transfer.id)
            .collect();
        ids.into_iter()
            .filter_map(|id| self.expire_transfer(id))
            .collect()
    }

    fn release_grant(&mut self, grant_id: u64) -> Vec<ControllerLease> {
        let ids: Vec<_> = self
            .by_id
            .values()
            .filter(|lease| lease.grant_id == Some(grant_id))
            .map(|lease| lease.id)
            .collect();
        ids.into_iter().filter_map(|id| self.release(id)).collect()
    }

    fn reset_connections(&mut self) -> (Vec<ControllerLease>, Vec<PendingControlTransfer>) {
        let leases = self.by_id.drain().map(|(_, lease)| lease).collect();
        let transfers = self
            .transfers
            .drain()
            .map(|(_, transfer)| transfer)
            .collect();
        self.by_splint.clear();
        self.by_connection.clear();
        self.transfer_by_splint.clear();
        (leases, transfers)
    }

    fn release_identity(
        &mut self,
        splint_id: SplintId,
        incarnation: u64,
    ) -> Option<ControllerLease> {
        let id = self
            .by_splint
            .get(&splint_id)
            .and_then(|id| self.by_id.get(id))
            .filter(|lease| lease.incarnation == incarnation)
            .map(|lease| lease.id)?;
        self.release(id)
    }
}

#[derive(Clone, Copy, Debug)]
struct Revocation {
    grant_id: u64,
}

#[derive(Default)]
struct RuntimeRegistry {
    entries: HashMap<SplintId, LiveSplintRuntime>,
}

impl RuntimeRegistry {
    fn insert(&mut self, runtime: LiveSplintRuntime) -> Result<(), LiveSplintRuntime> {
        let id = runtime.handle().splint_id;
        if self.entries.len() >= MAX_LIVE_SPLINTS || self.entries.contains_key(&id) {
            return Err(runtime);
        }
        self.entries.insert(id, runtime);
        Ok(())
    }

    fn handle(&self, id: SplintId) -> Option<LiveSplintHandle> {
        self.entries.get(&id).map(LiveSplintRuntime::handle)
    }

    fn handles(&self) -> HashMap<SplintId, LiveSplintHandle> {
        self.entries
            .iter()
            .map(|(id, runtime)| (*id, runtime.handle()))
            .collect()
    }

    fn remove(&mut self, id: SplintId) -> Option<LiveSplintRuntime> {
        self.entries.remove(&id)
    }

    fn drain(&mut self) -> Vec<LiveSplintRuntime> {
        self.entries.drain().map(|(_, runtime)| runtime).collect()
    }
}

struct TopologySubscriber {
    changes: mpsc::Sender<Arc<TopologyChange>>,
    resync: tokio::sync::watch::Sender<Option<splinterm_core::TopologyRevision>>,
}

#[derive(Default)]
struct TopologyHub {
    subscribers: HashMap<u64, TopologySubscriber>,
}

#[derive(Debug)]
struct TopologySubscription {
    changes: mpsc::Receiver<Arc<TopologyChange>>,
    resync: tokio::sync::watch::Receiver<Option<splinterm_core::TopologyRevision>>,
}

impl TopologyHub {
    fn subscribe(&mut self, id: u64) -> TopologySubscription {
        let (changes, receiver) = mpsc::channel(TOPOLOGY_QUEUE);
        let (resync, resync_receiver) = tokio::sync::watch::channel(None);
        self.subscribers
            .insert(id, TopologySubscriber { changes, resync });
        TopologySubscription {
            changes: receiver,
            resync: resync_receiver,
        }
    }

    fn remove(&mut self, id: u64) {
        self.subscribers.remove(&id);
    }

    fn clear(&mut self) {
        self.subscribers.clear();
    }

    fn publish(&mut self, change: &TopologyChange) {
        let change = Arc::new(change.clone());
        self.subscribers.retain(|_, subscriber| {
            match subscriber.changes.try_send(Arc::clone(&change)) {
                Ok(()) => true,
                Err(mpsc::error::TrySendError::Full(_)) => {
                    subscriber.resync.send_replace(Some(change.revision));
                    true
                }
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            }
        });
    }
}

struct DaemonState {
    lair: RwLock<Lair>,
    runtimes: Mutex<RuntimeRegistry>,
    topology: Mutex<TopologyHub>,
    topology_transactions: Semaphore,
    exit_observers: TaskTracker,
    metadata: Option<MetadataStore>,
    policy: Mutex<policy::PolicyStore>,
    audit: Mutex<audit::AuditStore>,
    daemon_audit_peer: splinterm_protocol::AuditPeer,
    policy_reloads: broadcast::Sender<u64>,
    controller: Mutex<ControllerState>,
    control_events: broadcast::Sender<ControlNotice>,
    connection_revocations: broadcast::Sender<u64>,
    grants: Mutex<GrantStore>,
    revocations: broadcast::Sender<Revocation>,
    image_transfers: Mutex<TransferAdmission>,
    image_transfer_expiry_changed: Notify,
    shared_image_budget: SharedImageBudget,
    shared_kitty_upload_budget: SharedKittyUploadBudget,
    pty_backend: LinuxPtyBackend,
    owner_home: Option<PathBuf>,
    development_terminal_access: bool,
}

async fn image_transfer_expiry_deadline(state: &DaemonState, expire: bool) -> time::Instant {
    let mut transfers = state.image_transfers.lock().await;
    if expire {
        transfers.expire(Instant::now());
    }
    transfers.next_expiry().map_or_else(
        || time::Instant::now() + Duration::from_secs(365 * 24 * 60 * 60),
        time::Instant::from_std,
    )
}

// Local PTY and Unix-socket work is asynchronous; bounding workers also bounds
// glibc per-thread allocator arenas after sustained terminal output.
#[allow(
    clippy::too_many_lines,
    reason = "startup, owned connection lifetime, and ordered shutdown remain one daemon boundary"
)]
#[tokio::main(worker_threads = 2)]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let metadata = MetadataStore::discover()?;
    let loaded = tokio::task::spawn_blocking({
        let metadata = metadata.clone();
        move || metadata.load()
    })
    .await
    .context("metadata load task failed")?;
    let lair = match loaded {
        Ok(Some(document)) => document.into_lair().context("invalid restored Lair")?,
        Ok(None) => Lair::new(),
        Err(error) => {
            warn!(%error, "metadata recovery failed; starting with an empty Lair");
            Lair::new()
        }
    };
    for dojo in lair.dojos() {
        for window in &dojo.windows {
            for splint_id in layout_splint_ids(&window.root) {
                if let Some(incarnation) = lair
                    .find_splint(splint_id)
                    .and_then(|splint| splint.last_incarnation)
                {
                    ProcessIncarnation::reserve_after(incarnation);
                }
            }
        }
    }

    let socket = socket_path()?;
    prepare_socket_parent(&socket).await?;
    remove_stale_socket(&socket).await?;
    let listener = UnixListener::bind(&socket)
        .with_context(|| format!("failed to bind {}", socket.display()))?;
    fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).await?;
    verify_socket(&socket).await?;
    let image_socket = image_content_socket_path(&socket);
    remove_stale_socket(&image_socket).await?;
    let image_listener = UnixListener::bind(&image_socket)
        .with_context(|| format!("failed to bind {}", image_socket.display()))?;
    fs::set_permissions(&image_socket, std::fs::Permissions::from_mode(0o600)).await?;
    verify_socket(&image_socket).await?;

    let daemon_identity = tokio::task::spawn_blocking(|| {
        executable_identity::ExecutableIdentity::from_pid(std::process::id())
    })
    .await
    .context("daemon executable identity task failed")??;
    let daemon_audit_peer = splinterm_protocol::AuditPeer {
        uid: rustix::process::geteuid().as_raw(),
        executable_path: daemon_identity
            .path
            .to_string_lossy()
            .chars()
            .take(4096)
            .collect(),
        executable_sha256: daemon_identity.sha256,
        device: Some(daemon_identity.device),
        inode: Some(daemon_identity.inode),
    };
    let (revocations, _) = broadcast::channel(32);
    let (control_events, _) = broadcast::channel(CONTROL_EVENT_QUEUE);
    let (connection_revocations, _) = broadcast::channel(CONNECTION_LIMIT);
    let (policy_reloads, _) = broadcast::channel(1);
    let mut policy = policy::PolicyStore::default();
    let policy_generation = policy.reload(policy::configured_path().as_deref(), &lair);
    if let Some(diagnostic) = &policy_generation.diagnostic {
        warn!(generation = policy_generation.id, %diagnostic, "persistent policy rejected; installed deny-all generation");
    }
    let state = Arc::new(DaemonState {
        lair: RwLock::new(lair),
        runtimes: Mutex::new(RuntimeRegistry::default()),
        topology: Mutex::new(TopologyHub::default()),
        topology_transactions: Semaphore::new(1),
        exit_observers: TaskTracker::new(),
        metadata: Some(metadata),
        policy: Mutex::new(policy),
        audit: Mutex::new(audit::AuditStore::default()),
        daemon_audit_peer,
        policy_reloads,
        controller: Mutex::new(ControllerState::default()),
        control_events,
        connection_revocations,
        grants: Mutex::new(GrantStore::default()),
        revocations,
        image_transfers: Mutex::new(TransferAdmission::default()),
        image_transfer_expiry_changed: Notify::new(),
        shared_image_budget: SharedImageBudget::new(MAX_IMAGE_BYTES_PER_DAEMON),
        shared_kitty_upload_budget: SharedKittyUploadBudget::new(
            DEFAULT_KITTY_UPLOAD_BYTES_PER_DAEMON,
        ),
        pty_backend: LinuxPtyBackend::installed()?,
        owner_home: env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute() && !path.as_os_str().is_empty()),
        development_terminal_access: env::var_os("SPLINTERM_ENABLE_DEV_ATTACH").as_deref()
            == Some(std::ffi::OsStr::new("1")),
    });
    record_policy_reload(&state, &policy_generation).await;
    let connections = Arc::new(Semaphore::new(CONNECTION_LIMIT));
    let image_connections = Arc::new(Semaphore::new(IMAGE_CONTENT_CONNECTION_LIMIT));
    let mut connection_tasks = tokio::task::JoinSet::new();
    info!(socket = %socket.display(), image_socket = %image_socket.display(), development_terminal_access = state.development_terminal_access, "splinterd ready");
    let shutdown_signal = signal::ctrl_c();
    tokio::pin!(shutdown_signal);
    let mut reload_signal = signal::unix::signal(signal::unix::SignalKind::hangup())
        .context("failed to listen for policy reload signal")?;
    let image_transfer_expiry =
        time::sleep_until(image_transfer_expiry_deadline(&state, false).await);
    tokio::pin!(image_transfer_expiry);

    loop {
        tokio::select! {
            biased;
            result = &mut shutdown_signal => {
                result.context("failed to listen for shutdown signal")?;
                break;
            }
            _ = &mut image_transfer_expiry => {
                image_transfer_expiry
                    .as_mut()
                    .reset(image_transfer_expiry_deadline(&state, true).await);
            }
            () = state.image_transfer_expiry_changed.notified() => {
                image_transfer_expiry
                    .as_mut()
                    .reset(image_transfer_expiry_deadline(&state, false).await);
            }
            received = reload_signal.recv() => {
                if received.is_none() {
                    bail!("policy reload signal stream closed");
                }
                let policy_path = policy::configured_path();
                let candidate = tokio::task::spawn_blocking(move || policy::prepare(policy_path))
                    .await
                    .context("policy reload task failed")?;
                let _transaction = state
                    .topology_transactions
                    .acquire()
                    .await
                    .context("topology transaction barrier closed during policy reload")?;
                let topology_snapshot = state.lair.read().await.clone();
                let generation = state
                    .policy
                    .lock()
                    .await
                    .publish(candidate, &topology_snapshot);
                record_policy_reload(&state, &generation).await;
                let _ = state.policy_reloads.send(generation.id);
                state.topology.lock().await.clear();
                let (leases, transfers) = state.controller.lock().await.reset_connections();
                for lease in &leases {
                    append_daemon_splint_audit(
                        &state,
                        splinterm_protocol::AuditOperation::AcquireControl,
                        lease.splint_id,
                        lease.incarnation,
                        splinterm_protocol::AuditDecision::Revoked,
                        "policy_reload_revoked",
                        splinterm_protocol::AuditOutcome::Cancelled,
                    )
                    .await;
                }
                for transfer in &transfers {
                    append_daemon_splint_audit(
                        &state,
                        splinterm_protocol::AuditOperation::RequestControlTransfer,
                        transfer.splint_id,
                        transfer.incarnation,
                        splinterm_protocol::AuditDecision::Revoked,
                        "policy_reload_revoked",
                        splinterm_protocol::AuditOutcome::Cancelled,
                    )
                    .await;
                }
                if let Some(diagnostic) = &generation.diagnostic {
                    warn!(generation = generation.id, %diagnostic, released_controllers = leases.len(), cancelled_transfers = transfers.len(), "persistent policy reload rejected; installed deny-all generation and disconnected clients");
                } else {
                    info!(generation = generation.id, rules = generation.document.rule_count(), released_controllers = leases.len(), cancelled_transfers = transfers.len(), "persistent policy reloaded atomically; disconnected clients");
                }
            }
            accepted = image_listener.accept() => {
                let (stream, _) = accepted.context("failed to accept image content client")?;
                let Ok(permit) = Arc::clone(&image_connections).try_acquire_owned() else {
                    warn!("image content connection limit reached");
                    continue;
                };
                let state = Arc::clone(&state);
                connection_tasks.spawn(async move {
                    let _permit = permit;
                    if let Err(error) = serve_image_content_client(stream, state).await {
                        warn!(%error, "image content connection closed");
                    }
                });
            }
            accepted = listener.accept() => {
                let (stream, _) = accepted.context("failed to accept client")?;
                let Ok(permit) = Arc::clone(&connections).try_acquire_owned() else {
                    warn!("connection limit reached");
                    continue;
                };
                let state = Arc::clone(&state);
                connection_tasks.spawn(async move {
                    let _permit = permit;
                    if let Err(error) = serve_client(stream, state).await {
                        warn!(%error, "client connection closed");
                    }
                });
            }
            completed = connection_tasks.join_next(), if !connection_tasks.is_empty() => {
                if let Some(Err(error)) = completed {
                    warn!(%error, "client connection task failed");
                }
            }
        }
    }

    connection_tasks.abort_all();
    while connection_tasks.join_next().await.is_some() {}

    state.exit_observers.close();
    let runtimes = state.runtimes.lock().await.drain();
    let shutdown_result = async {
        let mut tasks = tokio::task::JoinSet::new();
        for runtime in runtimes {
            tasks.spawn(runtime.shutdown());
        }
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => error!(%error, "failed to shut down live Splint cleanly"),
                Err(error) => error!(%error, "live Splint shutdown task failed"),
            }
        }
        time::timeout(EXIT_OBSERVER_TIMEOUT, state.exit_observers.wait())
            .await
            .context("timed out while reconciling process exits during shutdown")?;
        let _transaction = state
            .topology_transactions
            .acquire()
            .await
            .context("topology transaction barrier closed during shutdown")?;
        let final_lair = state.lair.read().await.clone();
        persist_lair(&state, &final_lair)
            .await
            .map_err(|_| anyhow::anyhow!("failed to persist final Lair metadata"))
    }
    .await;
    let socket_removal = fs::remove_file(&socket).await;
    let image_socket_removal = fs::remove_file(&image_socket).await;
    shutdown_result?;
    socket_removal?;
    image_socket_removal?;
    Ok(())
}

async fn serve_image_content_client(mut stream: UnixStream, state: Arc<DaemonState>) -> Result<()> {
    let (peer, peer_monitor) = verify_peer(&stream).await?;
    let transfer_peer = peer
        .transfer_peer()
        .context("persistent image content peer identity is unavailable")?;
    let transfer = async {
        let mut token = [0_u8; splinterm_protocol::IMAGE_TRANSFER_TOKEN_BYTES];
        time::timeout(IMAGE_CONTENT_IO_TIMEOUT, stream.read_exact(&mut token))
            .await
            .context("image content handshake timed out")??;
        let claimed = state
            .image_transfers
            .lock()
            .await
            .claim(token, &transfer_peer, Instant::now())
            .map_err(|error| anyhow::anyhow!(error))?;
        let guard = ActiveImageTransferGuard::new(Arc::clone(&state), claimed.transfer_id);
        let result = match claimed.mode {
            ImageTransferMode::BinaryChunks => {
                send_image_content_chunks(&mut stream, &claimed).await
            }
            ImageTransferMode::SealedMemfd => send_image_content_memfd(&mut stream, &claimed).await,
        };
        let finish = guard.finish().await;
        result?;
        finish.map_err(|error| anyhow::anyhow!(error))
    };
    let _peer_monitor = peer_monitor;
    transfer.await
}

async fn send_image_content_memfd(
    stream: &mut UnixStream,
    claimed: &splinterd::image_transport::ClaimedTransfer,
) -> Result<()> {
    let pixels = claimed.content.pixels();
    let metadata = claimed.content.metadata();
    let fd = sealed_image_memfd(pixels).map_err(|error| anyhow::anyhow!(error))?;
    let descriptors = [fd.as_fd()];
    loop {
        time::timeout(IMAGE_CONTENT_IO_TIMEOUT, stream.writable())
            .await
            .context("image descriptor send timed out")??;
        let mut ancillary_space =
            [std::mem::MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = SendAncillaryBuffer::new(&mut ancillary_space);
        if !ancillary.push(SendAncillaryMessage::ScmRights(&descriptors)) {
            bail!("image descriptor ancillary buffer is too small");
        }
        match sendmsg(
            stream.as_fd(),
            &[IoSlice::new(b"F")],
            &mut ancillary,
            SendFlags::empty(),
        ) {
            Ok(1) => break,
            Ok(_) => bail!("image descriptor marker write was incomplete"),
            Err(error) if error == rustix::io::Errno::WOULDBLOCK => continue,
            Err(error) => return Err(error).context("image descriptor send failed"),
        }
    }
    let mut header = [0_u8; IMAGE_MEMFD_HEADER_BYTES];
    header[0..5].copy_from_slice(b"SPIF\x01");
    header[5..13].copy_from_slice(
        &u64::try_from(pixels.len())
            .context("image content length exceeds u64")?
            .to_be_bytes(),
    );
    header[13..45].copy_from_slice(&metadata.digest);
    time::timeout(IMAGE_CONTENT_IO_TIMEOUT, stream.write_all(&header))
        .await
        .context("image descriptor header write timed out")??;
    let mut acknowledgement = [0_u8; 1];
    time::timeout(
        IMAGE_CONTENT_IO_TIMEOUT,
        stream.read_exact(&mut acknowledgement),
    )
    .await
    .context("image descriptor acknowledgement timed out")??;
    if acknowledgement != [1] {
        bail!("image descriptor transfer was cancelled or rejected");
    }
    Ok(())
}

async fn send_image_content_chunks(
    stream: &mut UnixStream,
    claimed: &splinterd::image_transport::ClaimedTransfer,
) -> Result<()> {
    let pixels = claimed.content.pixels();
    let metadata = claimed.content.metadata();
    let mut header = [0_u8; IMAGE_CONTENT_HEADER_BYTES];
    header[0..4].copy_from_slice(b"SPIM");
    header[4] = 1;
    header[5..13].copy_from_slice(
        &u64::try_from(pixels.len())
            .context("image content length exceeds u64")?
            .to_be_bytes(),
    );
    header[13..45].copy_from_slice(&metadata.digest);
    header[45..49].copy_from_slice(
        &u32::try_from(splinterm_protocol::MAX_IMAGE_CHUNK_BYTES)
            .expect("image chunk bound fits u32")
            .to_be_bytes(),
    );
    header[49..53].copy_from_slice(
        &u32::try_from(splinterm_protocol::MAX_IMAGE_CHUNK_WINDOW)
            .expect("image chunk window fits u32")
            .to_be_bytes(),
    );
    time::timeout(IMAGE_CONTENT_IO_TIMEOUT, stream.write_all(&header))
        .await
        .context("image content header write timed out")??;

    let mut offset = 0_usize;
    while offset < pixels.len() {
        let window_end = offset
            .saturating_add(
                splinterm_protocol::MAX_IMAGE_CHUNK_BYTES
                    * splinterm_protocol::MAX_IMAGE_CHUNK_WINDOW,
            )
            .min(pixels.len());
        while offset < window_end {
            let end = offset
                .saturating_add(splinterm_protocol::MAX_IMAGE_CHUNK_BYTES)
                .min(window_end);
            let mut chunk_header = [0_u8; 12];
            chunk_header[0..8].copy_from_slice(
                &u64::try_from(offset)
                    .context("image content offset exceeds u64")?
                    .to_be_bytes(),
            );
            chunk_header[8..12].copy_from_slice(
                &u32::try_from(end - offset)
                    .expect("bounded image chunk fits u32")
                    .to_be_bytes(),
            );
            time::timeout(IMAGE_CONTENT_IO_TIMEOUT, stream.write_all(&chunk_header))
                .await
                .context("image chunk header write timed out")??;
            time::timeout(
                IMAGE_CONTENT_IO_TIMEOUT,
                stream.write_all(&pixels[offset..end]),
            )
            .await
            .context("image chunk write timed out")??;
            offset = end;
        }
        let mut acknowledgement = [0_u8; 9];
        time::timeout(
            IMAGE_CONTENT_IO_TIMEOUT,
            stream.read_exact(&mut acknowledgement),
        )
        .await
        .context("image acknowledgement timed out")??;
        if acknowledgement[0] == 2 {
            bail!("image content transfer cancelled");
        }
        let acknowledged = usize::try_from(u64::from_be_bytes(
            acknowledgement[1..9]
                .try_into()
                .expect("acknowledgement offset has fixed width"),
        ))
        .context("image acknowledgement offset exceeds usize")?;
        if acknowledgement[0] != 1 || acknowledged != offset {
            bail!("image acknowledgement is out of window");
        }
    }
    Ok(())
}

async fn serve_client(stream: UnixStream, state: Arc<DaemonState>) -> Result<()> {
    let (peer, peer_monitor) = verify_peer(&stream).await?;
    let connection_id = NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed);
    let (reader, writer) = stream.into_split();
    let (outbound_tx, outbound_rx) = mpsc::channel(OUTBOUND_QUEUE);
    let (control_tx, control_rx) = mpsc::channel(CONTROL_QUEUE);
    let writer_task = AbortOnDrop::new(tokio::spawn(write_frames(writer, outbound_rx, control_rx)));
    let authenticated = serve_authenticated(
        reader,
        &state,
        &peer,
        connection_id,
        &outbound_tx,
        &control_tx,
    );
    let result = if let Some(peer_monitor) = peer_monitor {
        tokio::select! {
            result = authenticated => result,
            exited = peer_monitor.exited() => {
                exited?;
                Err(anyhow::anyhow!("socket peer exited"))
            }
        }
    } else {
        authenticated.await
    };
    cleanup_connection(&state, connection_id).await;
    drop(outbound_tx);
    drop(control_tx);
    writer_task.join().await;
    result
}

async fn record_policy_reload(state: &DaemonState, generation: &policy::PolicyGeneration) {
    let rejected = generation.diagnostic.is_some();
    state.audit.lock().await.record(audit::AuditDraft {
        unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        policy_generation: Some(generation.id),
        policy_rule_id: None,
        peer: state.daemon_audit_peer.clone(),
        operation: splinterm_protocol::AuditOperation::PolicyReload,
        resource: None,
        requested_scopes: Vec::new(),
        decision: if rejected {
            splinterm_protocol::AuditDecision::Rejected
        } else {
            splinterm_protocol::AuditDecision::Allowed
        },
        reason: if rejected {
            "policy_reload_rejected"
        } else {
            "policy_reload_accepted"
        },
        outcome: Some(splinterm_protocol::AuditOutcome::Succeeded),
        argument_count: None,
        executable_basename: None,
    });
}

async fn append_daemon_splint_audit(
    state: &DaemonState,
    operation: splinterm_protocol::AuditOperation,
    splint_id: SplintId,
    incarnation: u64,
    decision: splinterm_protocol::AuditDecision,
    reason: &'static str,
    outcome: splinterm_protocol::AuditOutcome,
) {
    let resource = {
        let lair = state.lair.read().await;
        lair.dojos().find_map(|dojo| {
            dojo.windows.iter().find_map(|window| {
                window
                    .root
                    .find_splint(splint_id)
                    .map(|_| splinterm_protocol::AuditResource {
                        dojo_id: Some(dojo.id),
                        window_id: Some(window.id),
                        splint_id: Some(splint_id),
                        incarnation: Some(incarnation),
                    })
            })
        })
    };
    state.audit.lock().await.record(audit::AuditDraft {
        unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        policy_generation: None,
        policy_rule_id: None,
        peer: state.daemon_audit_peer.clone(),
        operation,
        resource,
        requested_scopes: Vec::new(),
        decision,
        reason,
        outcome: Some(outcome),
        argument_count: None,
        executable_basename: None,
    });
}

async fn cleanup_connection(state: &DaemonState, connection_id: u64) {
    let (released, cancelled) = {
        let mut controllers = state.controller.lock().await;
        (
            controllers.release_connection(connection_id),
            controllers.cancel_connection_transfers(connection_id),
        )
    };
    if let Some(lease) = released {
        publish_control_status(state, lease.splint_id, lease.incarnation).await;
    }
    for transfer in cancelled {
        let _ = state
            .connection_revocations
            .send(transfer.requester_connection_id);
        publish_control_notice(
            state,
            ControlNotice::TransferResolved {
                transfer,
                outcome: ControlTransferOutcome::Cancelled,
                controller_id: None,
            },
        );
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the connection state machine keeps handshake and request-id enforcement together"
)]
async fn serve_authenticated(
    mut reader: OwnedReadHalf,
    state: &Arc<DaemonState>,
    peer: &PeerIdentity,
    connection_id: u64,
    outbound: &mpsc::Sender<ServerFrame>,
    control: &mpsc::Sender<ServerFrame>,
) -> Result<()> {
    let hello = time::timeout(HANDSHAKE_TIMEOUT, read_frame(&mut reader))
        .await
        .context("handshake timed out")??;
    let ClientFrame::Hello {
        minimum_version,
        maximum_version,
        role,
    } = hello
    else {
        send_control(
            control,
            protocol_error(None, ErrorCode::HandshakeRequired, "hello required"),
        )
        .await?;
        bail!("client did not send hello");
    };
    if minimum_version > PROTOCOL_VERSION || maximum_version < PROTOCOL_VERSION {
        send_control(
            control,
            protocol_error(
                None,
                ErrorCode::IncompatibleVersion,
                "no compatible protocol version",
            ),
        )
        .await?;
        bail!("incompatible protocol version");
    }
    send_control(
        control,
        ServerFrame::Hello {
            version: PROTOCOL_VERSION,
            limits: ServerLimits::default(),
            development_terminal_access: state.development_terminal_access,
        },
    )
    .await?;

    let mut subscriptions = HashMap::<u64, AbortOnDrop>::new();
    let mut policy_reloads = state.policy_reloads.subscribe();
    let mut connection_revocations = state.connection_revocations.subscribe();
    let mut last_request_id = 0_u64;
    loop {
        let frame = tokio::select! {
            biased;
            _ = policy_reloads.recv() => break,
            revoked = connection_revocations.recv() => match revoked {
                Ok(revoked) if revoked == connection_id => break,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            },
            frame = read_optional_frame(&mut reader) => frame?,
        };
        let Some(frame) = frame else { break };
        match frame {
            ClientFrame::Hello { .. } => {
                send_control(
                    control,
                    protocol_error(None, ErrorCode::InvalidFrame, "hello already completed"),
                )
                .await?;
                break;
            }
            ClientFrame::Cancel { request_id } => {
                send_response(
                    outbound,
                    request_id,
                    Err(ProtocolError::new(
                        ErrorCode::RequestNotFound,
                        "request is no longer outstanding",
                    )),
                )
                .await?;
            }
            ClientFrame::Request {
                request_id,
                request,
            } => {
                if request_id == 0 {
                    send_response(
                        outbound,
                        request_id,
                        Err(ProtocolError::new(
                            ErrorCode::InvalidRequestId,
                            "request id must be nonzero",
                        )),
                    )
                    .await?;
                    continue;
                }
                if request_id <= last_request_id {
                    send_response(
                        outbound,
                        request_id,
                        Err(ProtocolError::new(
                            ErrorCode::DuplicateRequestId,
                            "request ids must increase monotonically",
                        )),
                    )
                    .await?;
                    continue;
                }
                last_request_id = request_id;
                if let Request::Detach { subscription_id } = &request {
                    let authorization = RequestAuthorizationContext::default();
                    let Some(task) = subscriptions.remove(subscription_id) else {
                        append_request_audit(
                            state,
                            peer,
                            &request,
                            Some(&authorization),
                            RequestAuditDisposition {
                                resource: None,
                                decision: splinterm_protocol::AuditDecision::Denied,
                                reason: "subscription_not_owned",
                                outcome: splinterm_protocol::AuditOutcome::Failed,
                            },
                        )
                        .await;
                        send_response(
                            outbound,
                            request_id,
                            Err(ProtocolError::new(
                                ErrorCode::Unauthorized,
                                "subscription is not owned by this connection",
                            )),
                        )
                        .await?;
                        continue;
                    };
                    drop(task);
                    state.topology.lock().await.remove(*subscription_id);
                    append_request_audit(
                        state,
                        peer,
                        &request,
                        Some(&authorization),
                        RequestAuditDisposition {
                            resource: None,
                            decision: splinterm_protocol::AuditDecision::Allowed,
                            reason: "owned_subscription",
                            outcome: splinterm_protocol::AuditOutcome::Succeeded,
                        },
                    )
                    .await;
                    send_response(outbound, request_id, Ok(Response::Acknowledged)).await?;
                    continue;
                }
                let handled = handle_request(
                    request,
                    state,
                    peer,
                    connection_id,
                    subscriptions.len(),
                    role == ClientRole::TrustedUi,
                )
                .await;
                match handled {
                    Ok(Handled {
                        response,
                        subscription,
                    }) => {
                        if let Some(subscription) = subscription {
                            if subscriptions.len() >= MAX_SUBSCRIPTIONS {
                                send_response(
                                    outbound,
                                    request_id,
                                    Err(ProtocolError::new(
                                        ErrorCode::ResourceLimit,
                                        "subscription limit reached",
                                    )),
                                )
                                .await?;
                                continue;
                            }
                            send_response(outbound, request_id, Ok(response)).await?;
                            let (id, task) = match subscription {
                                PendingSubscription::Terminal {
                                    id,
                                    stream,
                                    handle,
                                    access,
                                } => (
                                    id,
                                    spawn_subscription(
                                        id,
                                        stream,
                                        handle,
                                        SubscriptionOutputs {
                                            outbound: outbound.clone(),
                                            control: control.clone(),
                                        },
                                        state.revocations.subscribe(),
                                        access,
                                        SubscriptionAudit {
                                            state: Arc::clone(state),
                                            peer: peer.audit_peer(),
                                        },
                                    ),
                                ),
                                PendingSubscription::Topology {
                                    id,
                                    stream,
                                    maximum_returned_bytes,
                                } => (
                                    id,
                                    spawn_topology_subscription(
                                        id,
                                        stream,
                                        outbound.clone(),
                                        maximum_returned_bytes,
                                    ),
                                ),
                                PendingSubscription::Control {
                                    id,
                                    stream,
                                    connection_id,
                                    splint_id,
                                    incarnation,
                                    maximum_returned_bytes,
                                } => (
                                    id,
                                    spawn_control_subscription(
                                        id,
                                        stream,
                                        outbound.clone(),
                                        ControlSubscriptionContext {
                                            state: Arc::clone(state),
                                            connection_id,
                                            splint_id,
                                            incarnation,
                                            maximum_returned_bytes,
                                        },
                                    ),
                                ),
                            };
                            subscriptions.insert(id, AbortOnDrop::new(task));
                        } else {
                            send_response(outbound, request_id, Ok(response)).await?;
                        }
                    }
                    Err(error) => send_response(outbound, request_id, Err(error)).await?,
                }
            }
        }
    }
    drop(subscriptions);
    Ok(())
}

#[derive(Debug)]
struct Handled {
    response: Response,
    subscription: Option<PendingSubscription>,
}

#[derive(Debug)]
enum PendingSubscription {
    Terminal {
        id: u64,
        stream: Subscription,
        handle: LiveSplintHandle,
        access: SubscriptionAccess,
    },
    Topology {
        id: u64,
        stream: TopologySubscription,
        maximum_returned_bytes: Option<usize>,
    },
    Control {
        id: u64,
        stream: broadcast::Receiver<ControlNotice>,
        connection_id: u64,
        splint_id: SplintId,
        incarnation: u64,
        maximum_returned_bytes: Option<usize>,
    },
}

#[derive(Clone, Debug)]
struct SubscriptionAccess {
    grant_id: Option<u64>,
    maximum_returned_bytes: Option<usize>,
    scrollback_rows: usize,
    include_images: bool,
    history: HistoryState,
    visible_rows: Vec<splinterd::LiveRow>,
}

#[derive(Clone, Copy, Debug)]
struct HistoryState {
    revision: u64,
    generation: u64,
    available_rows: usize,
}

fn publish_control_notice(state: &DaemonState, notice: ControlNotice) {
    let _ = state.control_events.send(notice);
}

async fn publish_control_status(state: &DaemonState, splint_id: SplintId, incarnation: u64) {
    let owner_connection_id = {
        let controllers = state.controller.lock().await;
        controllers
            .by_splint
            .get(&splint_id)
            .and_then(|id| controllers.by_id.get(id))
            .filter(|lease| lease.incarnation == incarnation)
            .map(|lease| lease.connection_id)
    };
    publish_control_notice(
        state,
        ControlNotice::Status {
            splint_id,
            incarnation,
            owner_connection_id,
        },
    );
}

fn publish_transfer_timeout(state: &DaemonState, transfer: PendingControlTransfer) {
    // Closing only the requester connection invalidates its pending adapter
    // handle immediately. The current owner remains connected and keeps its
    // controller lease.
    let _ = state
        .connection_revocations
        .send(transfer.requester_connection_id);
    publish_control_notice(
        state,
        ControlNotice::TransferResolved {
            transfer,
            outcome: ControlTransferOutcome::TimedOut,
            controller_id: None,
        },
    );
}

fn schedule_transfer_timeout(state: Arc<DaemonState>, transfer: PendingControlTransfer) {
    tokio::spawn(async move {
        time::sleep(CONTROL_TRANSFER_TIMEOUT).await;
        let expired = state.controller.lock().await.expire_transfer(transfer.id);
        if let Some(transfer) = expired {
            append_daemon_splint_audit(
                &state,
                splinterm_protocol::AuditOperation::RequestControlTransfer,
                transfer.splint_id,
                transfer.incarnation,
                splinterm_protocol::AuditDecision::Expired,
                "control_transfer_expired",
                splinterm_protocol::AuditOutcome::Cancelled,
            )
            .await;
            publish_transfer_timeout(&state, transfer);
        }
    });
}

async fn revoke_grant_controllers(state: &DaemonState, grant_id: u64) {
    let released = state.controller.lock().await.release_grant(grant_id);
    for lease in released {
        // Revocation removes both daemon and connection-owned adapter
        // authority. The connection close makes stale handles and resource
        // overlays disappear without waiting for reuse.
        let _ = state.connection_revocations.send(lease.connection_id);
        publish_control_status(state, lease.splint_id, lease.incarnation).await;
    }
}

async fn controlled_handle(
    state: &Arc<DaemonState>,
    connection_id: u64,
    controller_id: u64,
    splint_id: SplintId,
    incarnation: u64,
) -> Result<LiveSplintHandle, ProtocolError> {
    let handle = current_handle(state, splint_id, incarnation).await?;
    state.controller.lock().await.authorize(
        connection_id,
        controller_id,
        splint_id,
        incarnation,
    )?;
    Ok(handle)
}

async fn terminal_action_acknowledgement(
    state: &DaemonState,
    handle: &LiveSplintHandle,
) -> Result<Response, ProtocolError> {
    let snapshot = handle.snapshot().await.map_err(|_| internal())?;
    let splint_id = handle.splint_id;
    let (dojo_id, window_id) = state
        .lair
        .read()
        .await
        .dojos()
        .find_map(|dojo| {
            dojo.windows
                .iter()
                .find(|window| window.root.find_splint(splint_id).is_some())
                .map(|window| (dojo.id, window.id))
        })
        .ok_or_else(not_found)?;
    Ok(Response::TerminalActionAcknowledged {
        dojo_id,
        window_id,
        splint_id,
        incarnation: handle.incarnation.value(),
        terminal_revision: snapshot.revision.value(),
        history_generation: snapshot.scrollback.history_generation,
    })
}

fn first_party_ui_scopes(scopes: &[AccessScope]) -> bool {
    scopes.iter().all(|scope| {
        matches!(
            scope,
            AccessScope::Observe
                | AccessScope::Scrollback
                | AccessScope::Input
                | AccessScope::Resize
        )
    })
}

fn trusted_first_party_ui(
    trusted_ui_client: bool,
    peer: &PeerIdentity,
    scopes: &[AccessScope],
) -> bool {
    trusted_ui_client && peer.is_matching_splinterm() && first_party_ui_scopes(scopes)
}

async fn authorize_scope(
    state: &DaemonState,
    peer: &PeerIdentity,
    trusted_ui_client: bool,
    splint_id: SplintId,
    incarnation: u64,
    scopes: &[AccessScope],
) -> Result<Option<u64>, ProtocolError> {
    if state.development_terminal_access || trusted_first_party_ui(trusted_ui_client, peer, scopes)
    {
        return Ok(None);
    }
    state
        .grants
        .lock()
        .await
        .authorize(peer, splint_id, incarnation, scopes)
        .map(Some)
        .ok_or_else(|| ProtocolError::new(ErrorCode::Unauthorized, "trusted consent is required"))
}

fn first_party_grant(
    splint_id: SplintId,
    incarnation: u64,
    scopes: Vec<AccessScope>,
) -> AccessGrant {
    AccessGrant {
        grant_id: 0,
        splint_id,
        incarnation,
        scopes,
        requester: "TRUSTED FIRST-PARTY SPLINTERM UI".to_owned(),
        expires_at_unix_seconds: u64::MAX,
    }
}

fn development_grant(
    peer: &PeerIdentity,
    splint_id: SplintId,
    incarnation: u64,
    scopes: Vec<AccessScope>,
) -> AccessGrant {
    AccessGrant {
        grant_id: 0,
        splint_id,
        incarnation,
        scopes,
        requester: format!("DEVELOPMENT BYPASS — {}", peer.requester_label()),
        expires_at_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    }
}

async fn require_current_topology_revision(
    state: &DaemonState,
    expected: TopologyRevision,
) -> Result<(), ProtocolError> {
    let current = state.lair.read().await.revision();
    if current == expected {
        return Ok(());
    }
    Err(ProtocolError {
        code: ErrorCode::StaleTopology,
        message: "topology revision is stale".into(),
        current_topology_revision: Some(current),
    })
}

fn model_error(error: LairError) -> ProtocolError {
    match error {
        LairError::StaleTopology { current, .. } => ProtocolError {
            code: ErrorCode::StaleTopology,
            message: "topology revision is stale".into(),
            current_topology_revision: Some(current),
        },
        LairError::SplintNotFound(_)
        | LairError::DojoNotFound(_)
        | LairError::WindowNotFound(_) => not_found(),
        other => invalid(&other.to_string()),
    }
}

async fn persist_lair(state: &DaemonState, lair: &Lair) -> Result<(), ProtocolError> {
    if let Some(metadata) = &state.metadata {
        let document = LairDocument::from_lair(lair).map_err(|error| {
            error!(%error, "refusing invalid durable Lair candidate");
            internal()
        })?;
        let metadata = metadata.clone();
        tokio::task::spawn_blocking(move || metadata.save(&document))
            .await
            .map_err(|error| {
                error!(%error, "metadata save task failed");
                internal()
            })?
            .map_err(|error| {
                error!(%error, "durable Lair commit failed");
                internal()
            })?;
    }
    Ok(())
}

async fn durable_lair_candidate<T>(
    state: &DaemonState,
    mutate: impl FnOnce(&mut Lair) -> Result<T, LairError>,
) -> Result<(Lair, T), ProtocolError> {
    let mut candidate = state.lair.read().await.clone();
    let result = mutate(&mut candidate).map_err(model_error)?;
    persist_lair(state, &candidate).await?;
    Ok((candidate, result))
}

async fn install_lair(state: &DaemonState, candidate: Lair) {
    *state.lair.write().await = candidate;
}

async fn publish_topology(
    state: &DaemonState,
    revision: splinterm_core::TopologyRevision,
    kind: TopologyChangeKind,
) {
    let snapshot = topology_snapshot(state).await;
    debug_assert_eq!(snapshot.revision, revision);
    state.topology.lock().await.publish(&TopologyChange {
        revision,
        kind,
        snapshot,
    });
}

#[derive(Clone, Copy)]
struct SplintLaunchContext {
    dojo: DojoId,
    window: WindowId,
    splint: SplintId,
}

fn splint_launch_context(lair: &Lair, splint_id: SplintId) -> Option<SplintLaunchContext> {
    for dojo in lair.dojos() {
        for window in &dojo.windows {
            if window.root.find_splint(splint_id).is_some() {
                return Some(SplintLaunchContext {
                    dojo: dojo.id,
                    window: window.id,
                    splint: splint_id,
                });
            }
        }
    }
    None
}

async fn spawn_runtime(
    state: &DaemonState,
    context: SplintLaunchContext,
    launch: &splinterm_protocol::LaunchParameters,
) -> Result<LiveSplintRuntime, ProtocolError> {
    launch.validate()?;
    let pty_command = if let Some((program, arguments)) = launch.command.split_first() {
        PtyCommand::new(program, launch.cwd.clone())
            .args(arguments.iter())
            .login_shell(false)
    } else {
        PtyCommand::new(
            launch
                .shell
                .as_ref()
                .map_or_else(default_shell, OsString::from),
            launch.cwd.clone(),
        )
        .login_shell(launch.login_shell)
    }
    .env("SPLINTERM_DOJO_ID", context.dojo.to_string())
    .env("SPLINTERM_WINDOW_ID", context.window.to_string())
    .env("SPLINTERM_SPLINT_ID", context.splint.to_string());
    let mut config = LiveSplintConfig::default();
    config.terminal.scrollback_lines = launch.scrollback_lines;
    config.terminal.shared_image_budget = Some(state.shared_image_budget.clone());
    config.terminal.shared_kitty_upload_budget = Some(state.shared_kitty_upload_budget.clone());
    config.incarnation_environment = Some(OsString::from("SPLINTERM_SPLINT_INCARNATION"));
    LiveSplintRuntime::spawn(
        context.splint,
        state.pty_backend.clone(),
        pty_command,
        config,
    )
    .await
    .map_err(|error| {
        error!(%error, splint_id = ?context.splint, "failed to spawn live Splint");
        internal()
    })
}

fn durable_launch(launch: &splinterm_protocol::LaunchParameters) -> SplintLaunchMetadata {
    SplintLaunchMetadata {
        shell: launch.shell.clone(),
        login_shell: launch.login_shell,
        scrollback_lines: launch.scrollback_lines,
        ..SplintLaunchMetadata::default()
    }
}

fn saved_launch(splint: &Splint) -> splinterm_protocol::LaunchParameters {
    splinterm_protocol::LaunchParameters {
        cwd: splint.cwd.clone(),
        command: splint.command.clone(),
        shell: splint.launch.shell.clone(),
        login_shell: splint.launch.login_shell,
        scrollback_lines: splint.launch.scrollback_lines,
    }
}

async fn start_exited_splint(
    state: &Arc<DaemonState>,
    splint_id: SplintId,
    launch: &splinterm_protocol::LaunchParameters,
) -> Result<(u64, splinterm_core::TopologyRevision), ProtocolError> {
    launch.validate()?;
    {
        let lair = state.lair.read().await;
        let splint = lair.find_splint(splint_id).ok_or_else(not_found)?;
        if !matches!(splint.state, SplintState::Exited(_)) {
            return Err(invalid("only an exited Splint can be restored"));
        }
    }
    if let Some(runtime) = state.runtimes.lock().await.remove(splint_id) {
        let handle = runtime.handle();
        let incarnation = handle.incarnation.value();
        state
            .controller
            .lock()
            .await
            .release_identity(splint_id, incarnation);
        publish_control_status(state, splint_id, incarnation).await;
        let revoked =
            state
                .grants
                .lock()
                .await
                .revoke_identity(splint_id, incarnation, "process restored");
        for grant_id in revoked {
            let _ = state.revocations.send(Revocation { grant_id });
        }
        runtime.shutdown().await.map_err(|_| internal())?;
    }

    let context =
        splint_launch_context(&*state.lair.read().await, splint_id).ok_or_else(not_found)?;
    let runtime = spawn_runtime(state, context, launch).await?;
    let handle = runtime.handle();
    let incarnation = handle.incarnation.value();
    let previous_lair = state.lair.read().await.clone();
    let prepared = durable_lair_candidate(state, |lair| {
        lair.commit_relaunch(splint_id, launch.cwd.clone(), launch.command.clone())?;
        if !lair.set_splint_launch_metadata(splint_id, durable_launch(launch))
            || !lair.set_splint_last_incarnation(splint_id, incarnation)
        {
            return Err(LairError::SplintNotFound(splint_id));
        }
        Ok(lair.revision())
    })
    .await;
    let (candidate, topology_revision) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            let _ = runtime.shutdown().await;
            return Err(error);
        }
    };
    let rejected = {
        let mut lair = state.lair.write().await;
        let mut runtimes = state.runtimes.lock().await;
        match runtimes.insert(runtime) {
            Ok(()) => {
                *lair = candidate;
                None
            }
            Err(runtime) => Some(runtime),
        }
    };
    if let Some(runtime) = rejected {
        if persist_lair(state, &previous_lair).await.is_err() {
            error!("failed to roll back durable restore after registry rejection");
        }
        let _ = runtime.shutdown().await;
        return Err(ProtocolError::new(
            ErrorCode::ControllerUnavailable,
            "another restore won the Splint runtime race",
        ));
    }
    observe_process_exit(state, handle);
    Ok((incarnation, topology_revision))
}

async fn restore_targets(
    state: &Arc<DaemonState>,
    splint_ids: Vec<SplintId>,
) -> (splinterm_core::TopologyRevision, Vec<RestoreLeafResult>) {
    let mut results = Vec::with_capacity(splint_ids.len());
    for splint_id in splint_ids {
        let launch = state
            .lair
            .read()
            .await
            .find_splint(splint_id)
            .map(|splint| {
                (
                    saved_launch(splint),
                    splint.launch.columns,
                    splint.launch.rows,
                )
            });
        let result = match launch {
            Some((launch, columns, rows)) => {
                match start_exited_splint(state, splint_id, &launch).await {
                    Ok((incarnation, revision)) => {
                        let handle = state.runtimes.lock().await.handle(splint_id);
                        if let Some(handle) = handle {
                            let _ = handle
                                .resize(PtySize {
                                    columns,
                                    rows,
                                    pixel_width: 0,
                                    pixel_height: 0,
                                })
                                .await;
                        }
                        publish_topology(state, revision, TopologyChangeKind::RuntimeChanged).await;
                        RestoreLeafResult {
                            splint_id,
                            incarnation: Some(incarnation),
                            error: None,
                        }
                    }
                    Err(error) => RestoreLeafResult {
                        splint_id,
                        incarnation: None,
                        error: Some(error),
                    },
                }
            }

            None => RestoreLeafResult {
                splint_id,
                incarnation: None,
                error: Some(not_found()),
            },
        };
        results.push(result);
    }
    (state.lair.read().await.revision(), results)
}

async fn finalize_exit_if_current(
    state: &DaemonState,
    splint_id: SplintId,
    incarnation: u64,
    code: i32,
) -> bool {
    let Ok(_transaction) = state.topology_transactions.acquire().await else {
        return false;
    };
    let current = state
        .runtimes
        .lock()
        .await
        .handle(splint_id)
        .map(|handle| handle.incarnation.value());
    if current.is_some_and(|current| current != incarnation) {
        return false;
    }
    let released = state
        .controller
        .lock()
        .await
        .release_identity(splint_id, incarnation);
    if let Some(lease) = released {
        let _ = state.connection_revocations.send(lease.connection_id);
    }
    publish_control_status(state, splint_id, incarnation).await;
    let revoked =
        state
            .grants
            .lock()
            .await
            .revoke_identity(splint_id, incarnation, "process exited");
    for grant_id in revoked {
        let _ = state.revocations.send(Revocation { grant_id });
    }
    let mut candidate = state.lair.read().await.clone();
    if !candidate.set_splint_state(splint_id, SplintState::Exited(code)) {
        return false;
    }
    let revision = candidate.revision();
    if persist_lair(state, &candidate).await.is_err() {
        error!(
            ?splint_id,
            incarnation, "failed to persist process exit state"
        );
        return false;
    }
    install_lair(state, candidate).await;
    publish_topology(state, revision, TopologyChangeKind::RuntimeChanged).await;
    append_daemon_splint_audit(
        state,
        splinterm_protocol::AuditOperation::ProcessExit,
        splint_id,
        incarnation,
        splinterm_protocol::AuditDecision::Allowed,
        "process_exit_reconciled",
        splinterm_protocol::AuditOutcome::Succeeded,
    )
    .await;
    true
}

fn observe_process_exit(state: &Arc<DaemonState>, handle: LiveSplintHandle) {
    let exit_observers = state.exit_observers.clone();
    let state = Arc::clone(state);
    exit_observers.spawn(async move {
        let splint_id = handle.splint_id;
        let incarnation = handle.incarnation.value();
        if let Some(status) = handle.wait_for_exit().await {
            let code = status
                .code
                .or_else(|| status.signal.map(|signal| 128 + signal))
                .unwrap_or(1);
            finalize_exit_if_current(&state, splint_id, incarnation, code).await;
        }
    });
}

#[derive(Debug, Default)]
struct RequestAuthorizationContext {
    policy_match: Option<policy::PolicyMatch>,
}

impl RequestAuthorizationContext {
    fn policy_authorized(&self) -> bool {
        self.policy_match
            .as_ref()
            .is_some_and(|matched| !matched.rule_id.is_empty())
    }

    fn maximum_returned_bytes(&self) -> Option<usize> {
        self.policy_match
            .as_ref()
            .and_then(|matched| matched.max_returned_bytes)
    }
}

fn trusted_ui_request(request: &Request) -> bool {
    matches!(
        request,
        Request::Ping
            | Request::ListDojos
            | Request::InspectTopology
            | Request::SubscribeTopology
            | Request::InspectSplint { .. }
            | Request::RequestAccess { .. }
            | Request::AuthorizationStatus { .. }
            | Request::RevokeAccess { .. }
            | Request::PrepareMutation { .. }
            | Request::CreateDojo { .. }
            | Request::CreateDojoAutomation { .. }
            | Request::SplitSplint { .. }
            | Request::SplitSplintAutomation { .. }
            | Request::RelaunchSplint { .. }
            | Request::RelaunchSplintAutomation { .. }
            | Request::RestoreSplint { .. }
            | Request::RestoreWindow { .. }
            | Request::RestoreDojo { .. }
            | Request::CloseSplint { .. }
            | Request::SetSplitRatio { .. }
            | Request::NewWindow { .. }
            | Request::NewWindowAutomation { .. }
            | Request::CloseWindow { .. }
            | Request::RenameDojo { .. }
            | Request::RenameWindow { .. }
            | Request::SetWindowDefaultFocus { .. }
            | Request::RenameSplint { .. }
            | Request::Attach { .. }
            | Request::RequestImageContent { .. }
            | Request::StartScrollbackPage { .. }
            | Request::ScrollbackPage { .. }
            | Request::StartSearchScrollback { .. }
            | Request::SearchScrollback { .. }
            | Request::AcquireControl { .. }
            | Request::SubscribeControl { .. }
            | Request::RequestControlTransfer { .. }
            | Request::DecideControlTransfer { .. }
            | Request::ForceControlTransfer { .. }
            | Request::ReleaseControl { .. }
            | Request::Input { .. }
            | Request::Resize { .. }
            | Request::Detach { .. }
            | Request::KillSplint { .. }
    )
}

fn trusted_ui_bypass(
    trusted_ui_client: bool,
    matching_splinterm_executable: bool,
    request: &Request,
) -> bool {
    trusted_ui_client && matching_splinterm_executable && trusted_ui_request(request)
}

const fn include_image_metadata(
    trusted_ui_client: bool,
    matching_splinterm_executable: bool,
) -> bool {
    trusted_ui_client && matching_splinterm_executable
}

fn consent_capable_request(request: &Request) -> bool {
    matches!(
        request,
        Request::RequestAccess { .. }
            | Request::Attach { .. }
            | Request::StartScrollbackPage { .. }
            | Request::ScrollbackPage { .. }
            | Request::StartSearchScrollback { .. }
            | Request::SearchScrollback { .. }
            | Request::AcquireControl { .. }
            | Request::Input { .. }
            | Request::Resize { .. }
            | Request::KillSplint { .. }
    )
}

fn requested_operation_scopes(request: &Request) -> Option<Vec<authorization::OperationScope>> {
    use authorization::{ConditionalRequirement, OperationScope as Scope, RequestAuthorization};

    let plan = authorization::for_request(request);
    let mut scopes = match plan {
        RequestAuthorization::Authenticated
        | RequestAuthorization::Owned(_)
        | RequestAuthorization::TrustedUiConsent => return Some(Vec::new()),
        RequestAuthorization::Policy { required, .. }
        | RequestAuthorization::PolicyAndOwned { required, .. } => required.to_vec(),
        RequestAuthorization::Conditional { base, requirement } => {
            let mut scopes = base.to_vec();
            match requirement {
                ConditionalRequirement::RequestedAccessScopes => {
                    let Request::RequestAccess {
                        scopes: requested, ..
                    } = request
                    else {
                        return None;
                    };
                    for scope in requested {
                        scopes.push(match scope {
                            AccessScope::Observe => Scope::TerminalVisibleRead,
                            AccessScope::Scrollback => Scope::ScrollbackRead,
                            AccessScope::Input => Scope::Input,
                            AccessScope::Resize => Scope::Resize,
                            AccessScope::Terminate => Scope::ProcessTerminate,
                            AccessScope::ClipboardRead
                            | AccessScope::ClipboardWrite
                            | AccessScope::ControlTakeover => return None,
                        });
                    }
                }
                ConditionalRequirement::RequestedControlModes => {
                    let (Request::AcquireControl { modes, .. }
                    | Request::RequestControlTransfer { modes, .. }) = request
                    else {
                        return None;
                    };
                    if splinterm_protocol::validate_control_modes(modes).is_err() {
                        return None;
                    }
                    for mode in modes {
                        scopes.push(match mode {
                            splinterm_protocol::ControlMode::Input => Scope::Input,
                            splinterm_protocol::ControlMode::Resize => Scope::Resize,
                        });
                    }
                }
                ConditionalRequirement::AttachScrollback => {
                    if matches!(request, Request::Attach { scrollback_rows, .. } if *scrollback_rows > 0)
                    {
                        scopes.push(Scope::ScrollbackRead);
                    }
                }
                ConditionalRequirement::LiveProcessTermination
                | ConditionalRequirement::ExpandedLiveProcessTermination => {}
            }
            scopes
        }
    };
    scopes.sort_unstable();
    scopes.dedup();
    Some(scopes)
}

fn requested_limits(request: &Request, active_subscriptions: usize) -> policy::RequestedLimits {
    let mut limits = policy::RequestedLimits::default();
    match request {
        Request::SubscribeTopology | Request::Attach { .. } | Request::SubscribeControl { .. } => {
            limits.live_subscriptions = Some(active_subscriptions.saturating_add(1));
        }
        _ => {}
    }
    match request {
        Request::Attach {
            scrollback_rows, ..
        } if *scrollback_rows > 0 => limits.returned_rows = Some(*scrollback_rows),
        Request::StartScrollbackPage { max_rows, .. }
        | Request::ScrollbackPage { max_rows, .. } => limits.returned_rows = Some(*max_rows),
        Request::StartSearchScrollback { max_results, .. }
        | Request::SearchScrollback { max_results, .. } => {
            limits.results = Some(*max_results);
            limits.deadline_ms =
                Some(u64::try_from(SEARCH_DEADLINE.as_millis()).unwrap_or(u64::MAX));
        }
        Request::AuditInspect { max_records, .. } => limits.results = Some(*max_records),
        Request::CreateDojo { .. }
        | Request::CreateDojoAutomation { .. }
        | Request::SplitSplint { .. }
        | Request::SplitSplintAutomation { .. }
        | Request::NewWindow { .. }
        | Request::NewWindowAutomation { .. } => {
            limits.spawn_count = Some(1);
        }
        _ => {}
    }
    limits
}

fn splint_containment(
    lair: &Lair,
    splint_id: SplintId,
) -> Option<(splinterm_core::DojoId, splinterm_core::WindowId, String)> {
    for dojo in lair.dojos() {
        for window in &dojo.windows {
            if let Some(splint) = window.root.find_splint(splint_id) {
                return Some((dojo.id, window.id, splint.title.clone()));
            }
        }
    }
    None
}

async fn terminal_provenance(
    state: &DaemonState,
    splint_id: SplintId,
    incarnation: u64,
    terminal_revision: u64,
    history_generation: u64,
    title: String,
) -> Result<TerminalProvenance, ProtocolError> {
    let lair = state.lair.read().await;
    let topology_revision = lair.revision();
    let (dojo_id, window_id, _) = splint_containment(&lair, splint_id).ok_or_else(not_found)?;
    Ok(TerminalProvenance {
        dojo_id,
        window_id,
        splint_id,
        incarnation,
        topology_revision,
        terminal_revision,
        history_generation,
        title,
    })
}

async fn scrollback_response(
    state: &DaemonState,
    splint_id: SplintId,
    incarnation: u64,
    page: LiveScrollbackPage,
) -> Result<Response, ProtocolError> {
    let terminal_revision = page.terminal_revision.value();
    let history_generation = page.history_generation;
    let provenance = terminal_provenance(
        state,
        splint_id,
        incarnation,
        terminal_revision,
        history_generation,
        page.title,
    )
    .await?;
    Ok(Response::ScrollbackPage {
        provenance,
        page: WireScrollbackPage {
            splint_id,
            incarnation,
            terminal_revision,
            history_generation,
            oldest_available_row_id: page.oldest_available_row_id,
            newest_available_row_id: page.newest_available_row_id,
            rows: page.rows.into_iter().map(wire_row).collect(),
            has_older: page.has_older,
        },
    })
}

async fn scrollback_resync_response(
    state: &DaemonState,
    splint_id: SplintId,
    incarnation: u64,
    page: LiveScrollbackPage,
) -> Result<Response, ProtocolError> {
    let current_revision = page.terminal_revision.value();
    let history_generation = page.history_generation;
    Ok(Response::ScrollbackResyncRequired {
        provenance: terminal_provenance(
            state,
            splint_id,
            incarnation,
            current_revision,
            history_generation,
            page.title,
        )
        .await?,
        current_revision,
        history_generation,
    })
}

async fn search_response(
    state: &DaemonState,
    splint_id: SplintId,
    incarnation: u64,
    search: LiveSearchPage,
) -> Result<Response, ProtocolError> {
    let terminal_revision = search.terminal_revision.value();
    let history_generation = search.history_generation;
    let provenance = terminal_provenance(
        state,
        splint_id,
        incarnation,
        terminal_revision,
        history_generation,
        search.title,
    )
    .await?;
    let page = WireSearchPage {
        splint_id,
        incarnation,
        terminal_revision,
        history_generation,
        matches: search
            .page
            .matches
            .into_iter()
            .map(|item| WireSearchMatch {
                row_id: item.row_id,
                start_column: item.start_column,
                end_column: item.end_column,
                preview: item.preview,
            })
            .collect(),
        next_cursor: search.page.next_offset.map(encode_search_cursor),
        timed_out: search.page.timed_out,
    };
    page.validate()?;
    Ok(Response::SearchResults { provenance, page })
}

async fn search_resync_response(
    state: &DaemonState,
    splint_id: SplintId,
    incarnation: u64,
    search: LiveSearchPage,
) -> Result<Response, ProtocolError> {
    let current_revision = search.terminal_revision.value();
    let history_generation = search.history_generation;
    Ok(Response::SearchResyncRequired {
        provenance: terminal_provenance(
            state,
            splint_id,
            incarnation,
            current_revision,
            history_generation,
            search.title,
        )
        .await?,
        current_revision,
        history_generation,
    })
}

fn access_granted_response(
    containment: &(DojoId, WindowId, String),
    mutation: consent::AuthorizationMutation,
) -> Response {
    Response::AccessGranted {
        dojo_id: containment.0,
        window_id: containment.1,
        authorization_revision: mutation.authorization_revision,
        grant: mutation.grant,
    }
}

async fn access_containment(
    state: &DaemonState,
    splint_id: SplintId,
    incarnation: u64,
) -> Result<(DojoId, WindowId, String), ProtocolError> {
    let lair = state.lair.read().await;
    let splint = lair.find_splint(splint_id).ok_or_else(not_found)?;
    if matches!(splint.state, SplintState::Exited(_)) {
        return Err(ProtocolError::new(
            ErrorCode::StaleIncarnation,
            "requested Splint has no current incarnation",
        ));
    }
    let containment = splint_containment(&lair, splint_id).ok_or_else(not_found)?;
    drop(lair);
    let current = state
        .runtimes
        .lock()
        .await
        .handle(splint_id)
        .map(|handle| handle.incarnation.value());
    if current != Some(incarnation) {
        return Err(ProtocolError::new(
            ErrorCode::StaleIncarnation,
            "requested incarnation is not current",
        ));
    }
    Ok(containment)
}

async fn grant_access_response(
    state: &DaemonState,
    peer: &PeerIdentity,
    splint_id: SplintId,
    incarnation: u64,
    scopes: Vec<AccessScope>,
) -> Result<Response, ProtocolError> {
    let _transaction = state
        .topology_transactions
        .acquire()
        .await
        .map_err(|_| internal())?;
    let containment = access_containment(state, splint_id, incarnation).await?;
    let mutation = state
        .grants
        .lock()
        .await
        .grant(peer, splint_id, incarnation, scopes);
    Ok(access_granted_response(&containment, mutation))
}

async fn existing_access_response(
    state: &DaemonState,
    grant_id: u64,
    splint_id: SplintId,
    incarnation: u64,
) -> Result<Response, ProtocolError> {
    let _transaction = state
        .topology_transactions
        .acquire()
        .await
        .map_err(|_| internal())?;
    let containment = access_containment(state, splint_id, incarnation).await?;
    let mutation = state
        .grants
        .lock()
        .await
        .grant_with_revision(grant_id)
        .filter(|mutation| {
            mutation.grant.splint_id == splint_id && mutation.grant.incarnation == incarnation
        })
        .ok_or_else(not_found)?;
    Ok(access_granted_response(&containment, mutation))
}

async fn nonstored_access_response(
    state: &DaemonState,
    grant: AccessGrant,
) -> Result<Response, ProtocolError> {
    let _transaction = state
        .topology_transactions
        .acquire()
        .await
        .map_err(|_| internal())?;
    let containment = access_containment(state, grant.splint_id, grant.incarnation).await?;
    let authorization_revision = state.grants.lock().await.authorization_revision();
    Ok(access_granted_response(
        &containment,
        consent::AuthorizationMutation {
            grant,
            authorization_revision,
        },
    ))
}

fn splint_policy_resource(
    lair: &Lair,
    handles: &HashMap<SplintId, LiveSplintHandle>,
    splint_id: SplintId,
    requested_incarnation: Option<u64>,
) -> Option<policy::PolicyResource> {
    for dojo in lair.dojos() {
        for window in &dojo.windows {
            if let Some(splint) = window.root.find_splint(splint_id) {
                let current = if matches!(splint.state, SplintState::Exited(_)) {
                    None
                } else {
                    handles
                        .get(&splint_id)
                        .map(|handle| handle.incarnation.value())
                };
                if requested_incarnation.is_some() && requested_incarnation != current {
                    return None;
                }
                return Some(policy::PolicyResource::Splint {
                    dojo_id: dojo.id,
                    window_id: window.id,
                    splint_id,
                    incarnation: requested_incarnation.or(current),
                });
            }
        }
    }
    None
}

fn window_policy_resources(
    lair: &Lair,
    handles: &HashMap<SplintId, LiveSplintHandle>,
    window_id: WindowId,
    expanded: bool,
) -> Option<Vec<policy::PolicyResource>> {
    for dojo in lair.dojos() {
        if let Some(window) = dojo.windows.iter().find(|window| window.id == window_id) {
            let mut resources = vec![policy::PolicyResource::Window {
                dojo_id: dojo.id,
                window_id,
            }];
            if expanded {
                resources.extend(
                    layout_splint_ids(&window.root)
                        .into_iter()
                        .filter_map(|id| splint_policy_resource(lair, handles, id, None)),
                );
            }
            return Some(resources);
        }
    }
    None
}

fn dojo_policy_resources(
    lair: &Lair,
    handles: &HashMap<SplintId, LiveSplintHandle>,
    dojo_id: DojoId,
    expanded: bool,
) -> Option<Vec<policy::PolicyResource>> {
    let dojo = lair.dojos().find(|dojo| dojo.id == dojo_id)?;
    let mut resources = vec![policy::PolicyResource::Dojo { dojo_id }];
    if expanded {
        resources.extend(
            dojo.windows
                .iter()
                .flat_map(|window| layout_splint_ids(&window.root))
                .filter_map(|id| splint_policy_resource(lair, handles, id, None)),
        );
    }
    Some(resources)
}

#[allow(
    clippy::too_many_lines,
    reason = "the exhaustive request-to-resource security table remains reviewable in one match"
)]
async fn request_policy_resources(
    request: &Request,
    state: &DaemonState,
) -> Option<Vec<policy::PolicyResource>> {
    let lair = state.lair.read().await.clone();
    let handles = state.runtimes.lock().await.handles();
    let splint = |id, incarnation| splint_policy_resource(&lair, &handles, id, incarnation);
    let window = |id, expanded| window_policy_resources(&lair, &handles, id, expanded);
    let dojo = |id, expanded| dojo_policy_resources(&lair, &handles, id, expanded);

    Some(match request {
        Request::Ping
        | Request::DecideControlTransfer { .. }
        | Request::ReleaseControl { .. }
        | Request::Detach { .. }
        | Request::ForceControlTransfer { .. } => Vec::new(),
        Request::ListDojos
        | Request::InspectTopology
        | Request::SubscribeTopology
        | Request::CreateDojo { .. }
        | Request::CreateDojoAutomation { .. }
        | Request::PrepareMutation {
            mutation: splinterm_protocol::MutationPreflight::CreateDojo,
        }
        | Request::AuditInspect { .. } => vec![policy::PolicyResource::Lair],
        Request::RevokeAccess { grant_id } => {
            let (splint_id, incarnation) = state.grants.lock().await.grant_resource(*grant_id)?;
            vec![splint(splint_id, Some(incarnation))?]
        }
        Request::InspectSplint { splint_id }
        | Request::RelaunchSplint { splint_id, .. }
        | Request::RelaunchSplintAutomation { splint_id, .. }
        | Request::RestoreSplint { splint_id, .. }
        | Request::CloseSplint { splint_id, .. }
        | Request::RenameSplint { splint_id, .. }
        | Request::SetSplitRatio {
            target_splint_id: splint_id,
            ..
        } => vec![splint(*splint_id, None)?],
        Request::RequestAccess {
            splint_id,
            incarnation,
            ..
        }
        | Request::RequestImageContent {
            request:
                splinterm_protocol::ImageContentRequest {
                    splint_id,
                    incarnation,
                    ..
                },
        }
        | Request::ScrollbackPage {
            splint_id,
            incarnation,
            ..
        }
        | Request::SearchScrollback {
            splint_id,
            incarnation,
            ..
        }
        | Request::AcquireControl {
            splint_id,
            incarnation,
            ..
        }
        | Request::SubscribeControl {
            splint_id,
            incarnation,
        }
        | Request::RequestControlTransfer {
            splint_id,
            incarnation,
            ..
        }
        | Request::Input {
            splint_id,
            incarnation,
            ..
        }
        | Request::Resize {
            splint_id,
            incarnation,
            ..
        }
        | Request::KillSplint {
            splint_id,
            incarnation,
        } => vec![splint(*splint_id, Some(*incarnation))?],
        Request::Attach {
            splint_id,
            incarnation,
            ..
        }
        | Request::StartScrollbackPage {
            splint_id,
            incarnation,
            ..
        }
        | Request::StartSearchScrollback {
            splint_id,
            incarnation,
            ..
        }
        | Request::AuthorizationStatus {
            splint_id,
            incarnation,
        } => vec![splint(*splint_id, *incarnation)?],
        Request::SplitSplint {
            target_splint_id, ..
        }
        | Request::SplitSplintAutomation {
            target_splint_id, ..
        } => vec![splint(*target_splint_id, None)?],
        Request::RestoreWindow { window_id, .. } | Request::CloseWindow { window_id, .. } => {
            window(*window_id, true)?
        }
        Request::RestoreDojo { dojo_id, .. } => dojo(*dojo_id, true)?,
        Request::NewWindow { dojo_id, .. }
        | Request::NewWindowAutomation { dojo_id, .. }
        | Request::RenameDojo { dojo_id, .. } => dojo(*dojo_id, false)?,
        Request::RenameWindow { window_id, .. } => window(*window_id, false)?,
        Request::SetWindowDefaultFocus {
            window_id,
            splint_id,
            ..
        } => {
            let mut resources = window(*window_id, false)?;
            resources.push(splint(*splint_id, None)?);
            resources
        }
        Request::PrepareMutation { mutation } => match mutation {
            splinterm_protocol::MutationPreflight::CreateDojo => unreachable!(),
            splinterm_protocol::MutationPreflight::SplitSplint { splint_id }
            | splinterm_protocol::MutationPreflight::RelaunchSplint { splint_id }
            | splinterm_protocol::MutationPreflight::RestoreSplint { splint_id }
            | splinterm_protocol::MutationPreflight::CloseSplint { splint_id }
            | splinterm_protocol::MutationPreflight::SetSplitRatio { splint_id }
            | splinterm_protocol::MutationPreflight::RenameSplint { splint_id } => {
                vec![splint(*splint_id, None)?]
            }
            splinterm_protocol::MutationPreflight::KillSplint {
                splint_id,
                incarnation,
            } => {
                vec![splint(*splint_id, Some(*incarnation))?]
            }
            splinterm_protocol::MutationPreflight::RestoreWindow { window_id }
            | splinterm_protocol::MutationPreflight::CloseWindow { window_id } => {
                window(*window_id, true)?
            }
            splinterm_protocol::MutationPreflight::RestoreDojo { dojo_id } => dojo(*dojo_id, true)?,
            splinterm_protocol::MutationPreflight::NewWindow { dojo_id }
            | splinterm_protocol::MutationPreflight::RenameDojo { dojo_id } => {
                dojo(*dojo_id, false)?
            }
            splinterm_protocol::MutationPreflight::RenameWindow { window_id } => {
                window(*window_id, false)?
            }
            splinterm_protocol::MutationPreflight::SetWindowDefaultFocus {
                window_id,
                splint_id,
            } => {
                let mut resources = window(*window_id, false)?;
                resources.push(splint(*splint_id, None)?);
                resources
            }
        },
    })
}

async fn authorize_request(
    request: &Request,
    state: &DaemonState,
    peer: &PeerIdentity,
    active_subscriptions: usize,
    trusted_ui_client: bool,
) -> Result<RequestAuthorizationContext, ProtocolError> {
    use authorization::RequestAuthorization;

    let plan = authorization::for_request(request);
    if matches!(
        plan,
        RequestAuthorization::Authenticated
            | RequestAuthorization::Owned(_)
            | RequestAuthorization::TrustedUiConsent
    ) || state.development_terminal_access
        || trusted_ui_bypass(trusted_ui_client, peer.is_matching_splinterm(), request)
    {
        return Ok(RequestAuthorizationContext::default());
    }

    let Some(required_scopes) = requested_operation_scopes(request) else {
        if consent_capable_request(request) {
            return Ok(RequestAuthorizationContext::default());
        }
        return Err(ProtocolError::new(
            ErrorCode::Unauthorized,
            "request cannot be authorized by persistent policy",
        ));
    };
    let any_scope = match plan {
        RequestAuthorization::Policy { any_of, .. } => any_of,
        _ => &[],
    };
    let mut required_scopes = required_scopes;
    if matches!(
        plan,
        RequestAuthorization::Conditional {
            requirement: authorization::ConditionalRequirement::LiveProcessTermination
                | authorization::ConditionalRequirement::ExpandedLiveProcessTermination,
            ..
        }
    ) {
        let resources = request_policy_resources(request, state).await;
        if resources.as_ref().is_some_and(|resources| {
            resources.iter().any(|resource| {
                matches!(
                    resource,
                    policy::PolicyResource::Splint {
                        incarnation: Some(_),
                        ..
                    }
                )
            })
        }) {
            required_scopes.push(authorization::OperationScope::ProcessTerminate);
        }
    }
    if required_scopes.is_empty() {
        if consent_capable_request(request) {
            return Ok(RequestAuthorizationContext::default());
        }
        return Err(ProtocolError::new(
            ErrorCode::Unauthorized,
            "policy-authorized request has no operation scopes",
        ));
    }
    let resources = request_policy_resources(request, state)
        .await
        .filter(|resources| !resources.is_empty())
        .ok_or_else(|| {
            ProtocolError::new(ErrorCode::Unauthorized, "policy resource is unavailable")
        })?;
    let policy_request = policy::PolicyRequest {
        required_scopes: &required_scopes,
        any_scope,
        resources: &resources,
        limits: requested_limits(request, active_subscriptions),
    };
    let matched = if let Some(executable) = peer.persistent_executable() {
        state.policy.lock().await.authorize(
            executable,
            &policy_request,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        )
    } else {
        None
    };
    if let Some(policy_match) = matched {
        return Ok(RequestAuthorizationContext {
            policy_match: Some(policy_match),
        });
    }
    if consent_capable_request(request) {
        Ok(RequestAuthorizationContext::default())
    } else {
        Err(ProtocolError::new(
            ErrorCode::Unauthorized,
            "no exact persistent policy rule authorizes this request",
        ))
    }
}

fn audit_resource(resource: policy::PolicyResource) -> Option<splinterm_protocol::AuditResource> {
    match resource {
        policy::PolicyResource::Lair => None,
        policy::PolicyResource::Dojo { dojo_id } => Some(splinterm_protocol::AuditResource {
            dojo_id: Some(dojo_id),
            window_id: None,
            splint_id: None,
            incarnation: None,
        }),
        policy::PolicyResource::Window { dojo_id, window_id } => {
            Some(splinterm_protocol::AuditResource {
                dojo_id: Some(dojo_id),
                window_id: Some(window_id),
                splint_id: None,
                incarnation: None,
            })
        }
        policy::PolicyResource::Splint {
            dojo_id,
            window_id,
            splint_id,
            incarnation,
        } => Some(splinterm_protocol::AuditResource {
            dojo_id: Some(dojo_id),
            window_id: Some(window_id),
            splint_id: Some(splint_id),
            incarnation,
        }),
    }
}

fn spawn_audit_metadata(request: &Request) -> (Option<usize>, Option<String>) {
    if let Request::CreateDojoAutomation { launch, .. }
    | Request::SplitSplintAutomation { launch, .. }
    | Request::RelaunchSplintAutomation { launch, .. }
    | Request::NewWindowAutomation { launch, .. } = request
    {
        return (Some(launch.argv.len().saturating_sub(1)), None);
    }
    let (Request::CreateDojo { launch, .. }
    | Request::SplitSplint { launch, .. }
    | Request::RelaunchSplint { launch, .. }
    | Request::NewWindow { launch, .. }) = request
    else {
        return (None, None);
    };
    let basename = launch
        .command
        .first()
        .map(PathBuf::from)
        .or_else(|| launch.shell.as_ref().map(PathBuf::from))
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        });
    (Some(launch.command.len().saturating_sub(1)), basename)
}

struct RequestAuditDisposition {
    resource: Option<splinterm_protocol::AuditResource>,
    decision: splinterm_protocol::AuditDecision,
    reason: &'static str,
    outcome: splinterm_protocol::AuditOutcome,
}

fn should_append_request_audit(request: &Request, result: &Result<Handled, ProtocolError>) -> bool {
    !matches!(request, Request::PrepareMutation { .. }) || result.is_err()
}

async fn append_request_audit(
    state: &DaemonState,
    peer: &PeerIdentity,
    request: &Request,
    authorization: Option<&RequestAuthorizationContext>,
    disposition: RequestAuditDisposition,
) {
    let Some(audit_peer) = peer.audit_peer() else {
        warn!(operation = ?audit::operation_for_request(request), "audit record omitted because stable peer identity is unavailable");
        return;
    };
    let scopes = requested_operation_scopes(request).unwrap_or_default();
    let (argument_count, executable_basename) = spawn_audit_metadata(request);
    let policy_generation =
        if authorization.is_some_and(RequestAuthorizationContext::policy_authorized) {
            Some(state.policy.lock().await.snapshot().id)
        } else {
            None
        };
    let policy_rule_id = authorization
        .and_then(|context| context.policy_match.as_ref())
        .map(|matched| matched.rule_id.clone());
    state.audit.lock().await.record(audit::AuditDraft {
        unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        policy_generation,
        policy_rule_id,
        peer: audit_peer,
        operation: audit::operation_for_request(request),
        resource: disposition.resource,
        requested_scopes: scopes,
        decision: disposition.decision,
        reason: disposition.reason,
        outcome: Some(disposition.outcome),
        argument_count,
        executable_basename,
    });
}

async fn append_request_result_audit(
    state: &DaemonState,
    peer: &PeerIdentity,
    request: &Request,
    authorization: &RequestAuthorizationContext,
    resource: Option<splinterm_protocol::AuditResource>,
    result: &Result<Handled, ProtocolError>,
) {
    let denied = result.as_ref().err().is_some_and(|error| {
        matches!(
            error.code,
            ErrorCode::Unauthorized | ErrorCode::ConsentDenied | ErrorCode::ConsentUnavailable
        )
    });
    let revoked = result.is_ok() && matches!(request, Request::RevokeAccess { .. });
    let decision = if denied {
        splinterm_protocol::AuditDecision::Denied
    } else if revoked {
        splinterm_protocol::AuditDecision::Revoked
    } else if authorization.policy_authorized() {
        splinterm_protocol::AuditDecision::Matched
    } else {
        splinterm_protocol::AuditDecision::Allowed
    };
    let reason = if denied {
        "authorization_denied"
    } else if revoked {
        "grant_revoked"
    } else if authorization.policy_authorized() {
        "policy_match"
    } else {
        "trusted_or_owned_authority"
    };
    let outcome = if result.is_ok() {
        splinterm_protocol::AuditOutcome::Succeeded
    } else {
        splinterm_protocol::AuditOutcome::Failed
    };
    // A successful scoped preflight establishes no mutation commit. Denials and
    // failed preflights remain auditable; only the final mutation records success.
    if should_append_request_audit(request, result) {
        append_request_audit(
            state,
            peer,
            request,
            Some(authorization),
            RequestAuditDisposition {
                resource,
                decision,
                reason,
                outcome,
            },
        )
        .await;
    }
}

async fn bind_current_terminal_incarnation(
    mut request: Request,
    state: &DaemonState,
    peer: &PeerIdentity,
) -> Result<Request, ProtocolError> {
    let current = match &request {
        Request::Attach {
            splint_id,
            incarnation: None,
            ..
        }
        | Request::StartScrollbackPage {
            splint_id,
            incarnation: None,
            ..
        }
        | Request::StartSearchScrollback {
            splint_id,
            incarnation: None,
            ..
        } => state
            .runtimes
            .lock()
            .await
            .handle(*splint_id)
            .map(|handle| handle.incarnation.value()),
        _ => return Ok(request),
    };
    let Some(current) = current else {
        append_request_audit(
            state,
            peer,
            &request,
            None,
            RequestAuditDisposition {
                resource: None,
                decision: splinterm_protocol::AuditDecision::Denied,
                reason: "resource_unavailable",
                outcome: splinterm_protocol::AuditOutcome::Failed,
            },
        )
        .await;
        return Err(not_found());
    };
    match &mut request {
        Request::Attach { incarnation, .. }
        | Request::StartScrollbackPage { incarnation, .. }
        | Request::StartSearchScrollback { incarnation, .. } => {
            *incarnation = Some(current);
        }
        _ => unreachable!("only current terminal requests are bound"),
    }
    Ok(request)
}

fn preparation_target(
    snapshot: &TopologySnapshot,
    splint_id: SplintId,
) -> Result<splinterm_protocol::MutationTarget, ProtocolError> {
    let (dojo_id, window_id, _) =
        splint_containment(&snapshot.lair, splint_id).ok_or_else(not_found)?;
    let runtime = snapshot
        .runtimes
        .iter()
        .find(|runtime| runtime.splint_id == splint_id)
        .ok_or_else(not_found)?;
    let incarnation = runtime
        .live_incarnation
        .or(runtime.last_incarnation)
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            ProtocolError::new(ErrorCode::StaleIncarnation, "Splint has no incarnation")
        })?;
    Ok(splinterm_protocol::MutationTarget {
        dojo_id,
        window_id,
        splint_id,
        incarnation,
    })
}

fn collect_preparation_targets(
    snapshot: &TopologySnapshot,
    node: &LayoutNode,
    output: &mut Vec<splinterm_protocol::MutationTarget>,
) -> Result<(), ProtocolError> {
    match node {
        LayoutNode::Leaf(splint) => output.push(preparation_target(snapshot, splint.id)?),
        LayoutNode::Branch { first, second, .. } => {
            collect_preparation_targets(snapshot, first, output)?;
            collect_preparation_targets(snapshot, second, output)?;
        }
    }
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "the closed preflight operation-to-provenance table stays contiguous for review"
)]
fn prepare_mutation(
    snapshot: &TopologySnapshot,
    mutation: splinterm_protocol::MutationPreflight,
) -> Result<splinterm_protocol::MutationPreparation, ProtocolError> {
    use splinterm_protocol::MutationPreflight as Preflight;
    let mut preparation = splinterm_protocol::MutationPreparation {
        topology_revision: snapshot.revision,
        dojo_id: None,
        window_id: None,
        splint_id: None,
        incarnation: None,
        targets: Vec::new(),
    };
    match mutation {
        Preflight::CreateDojo => {}
        Preflight::SplitSplint { splint_id }
        | Preflight::RelaunchSplint { splint_id }
        | Preflight::RestoreSplint { splint_id }
        | Preflight::CloseSplint { splint_id }
        | Preflight::SetSplitRatio { splint_id }
        | Preflight::RenameSplint { splint_id } => {
            let target = preparation_target(snapshot, splint_id)?;
            preparation.dojo_id = Some(target.dojo_id);
            preparation.window_id = Some(target.window_id);
            preparation.splint_id = Some(target.splint_id);
            preparation.incarnation = Some(target.incarnation);
        }
        Preflight::KillSplint {
            splint_id,
            incarnation,
        } => {
            let target = preparation_target(snapshot, splint_id)?;
            let current = snapshot
                .runtimes
                .iter()
                .find(|runtime| runtime.splint_id == splint_id)
                .and_then(|runtime| runtime.live_incarnation);
            if current != Some(incarnation) || target.incarnation != incarnation {
                return Err(ProtocolError::new(
                    ErrorCode::StaleIncarnation,
                    "requested incarnation is not current",
                ));
            }
            preparation.dojo_id = Some(target.dojo_id);
            preparation.window_id = Some(target.window_id);
            preparation.splint_id = Some(target.splint_id);
            preparation.incarnation = Some(incarnation);
        }
        Preflight::NewWindow { dojo_id } | Preflight::RenameDojo { dojo_id } => {
            if !snapshot.lair.dojos().any(|dojo| dojo.id == dojo_id) {
                return Err(not_found());
            }
            preparation.dojo_id = Some(dojo_id);
        }
        Preflight::RenameWindow { window_id } | Preflight::CloseWindow { window_id } => {
            let dojo_id = snapshot
                .lair
                .dojos()
                .find(|dojo| dojo.windows.iter().any(|window| window.id == window_id))
                .map(|dojo| dojo.id)
                .ok_or_else(not_found)?;
            preparation.dojo_id = Some(dojo_id);
            preparation.window_id = Some(window_id);
        }
        Preflight::RestoreWindow { window_id } => {
            let (dojo_id, window) = snapshot
                .lair
                .dojos()
                .find_map(|dojo| {
                    dojo.windows
                        .iter()
                        .find(|window| window.id == window_id)
                        .map(|window| (dojo.id, window))
                })
                .ok_or_else(not_found)?;
            preparation.dojo_id = Some(dojo_id);
            preparation.window_id = Some(window_id);
            collect_preparation_targets(snapshot, &window.root, &mut preparation.targets)?;
        }
        Preflight::RestoreDojo { dojo_id } => {
            let dojo = snapshot
                .lair
                .dojos()
                .find(|dojo| dojo.id == dojo_id)
                .ok_or_else(not_found)?;
            preparation.dojo_id = Some(dojo_id);
            for window in &dojo.windows {
                collect_preparation_targets(snapshot, &window.root, &mut preparation.targets)?;
            }
        }
        Preflight::SetWindowDefaultFocus {
            window_id,
            splint_id,
        } => {
            let target = preparation_target(snapshot, splint_id)?;
            if target.window_id != window_id {
                return Err(invalid(
                    "selected Splint does not belong to selected window",
                ));
            }
            preparation.dojo_id = Some(target.dojo_id);
            preparation.window_id = Some(window_id);
            preparation.splint_id = Some(splint_id);
            preparation.incarnation = Some(target.incarnation);
        }
    }
    Ok(preparation)
}

fn splint_durable_cwd(lair: &Lair, splint_id: SplintId) -> Result<PathBuf, ProtocolError> {
    lair.find_splint(splint_id)
        .map(|splint| splint.cwd.clone())
        .ok_or_else(not_found)
}

fn dojo_default_cwd(lair: &Lair, dojo_id: DojoId) -> Result<PathBuf, ProtocolError> {
    let dojo = lair
        .dojos()
        .find(|dojo| dojo.id == dojo_id)
        .ok_or_else(not_found)?;
    let window = dojo.windows.first().ok_or_else(not_found)?;
    splint_durable_cwd(lair, window.default_focus)
}

fn resolved_automation_launch(
    launch: splinterm_protocol::AutomationLaunch,
    default_cwd: PathBuf,
) -> Result<splinterm_protocol::LaunchParameters, ProtocolError> {
    launch.validate()?;
    let launch = splinterm_protocol::LaunchParameters {
        cwd: launch.cwd.unwrap_or(default_cwd),
        command: launch.argv,
        shell: None,
        login_shell: false,
        scrollback_lines: splinterm_terminal::TerminalConfig::default().scrollback_lines,
    };
    launch.validate()?;
    Ok(launch)
}

async fn resolve_automation_mutation(
    request: Request,
    state: &DaemonState,
) -> Result<Request, ProtocolError> {
    Ok(match request {
        Request::CreateDojoAutomation {
            expected_topology_revision,
            name,
            launch,
        } => Request::CreateDojo {
            expected_topology_revision,
            name,
            launch: resolved_automation_launch(
                launch,
                state.owner_home.clone().ok_or_else(|| {
                    ProtocolError::new(ErrorCode::InvalidArgument, "owner home is unavailable")
                })?,
            )?,
        },
        Request::SplitSplintAutomation {
            expected_topology_revision,
            target_splint_id,
            axis,
            side,
            ratio,
            launch,
        } => {
            let cwd = splint_durable_cwd(&*state.lair.read().await, target_splint_id)?;
            Request::SplitSplint {
                expected_topology_revision,
                target_splint_id,
                axis,
                side,
                ratio,
                launch: resolved_automation_launch(launch, cwd)?,
            }
        }
        Request::RelaunchSplintAutomation {
            expected_topology_revision,
            splint_id,
            launch,
        } => {
            let cwd = splint_durable_cwd(&*state.lair.read().await, splint_id)?;
            Request::RelaunchSplint {
                expected_topology_revision,
                splint_id,
                launch: resolved_automation_launch(launch, cwd)?,
            }
        }
        Request::NewWindowAutomation {
            expected_topology_revision,
            dojo_id,
            title,
            launch,
        } => {
            let cwd = dojo_default_cwd(&*state.lair.read().await, dojo_id)?;
            Request::NewWindow {
                expected_topology_revision,
                dojo_id,
                title,
                launch: resolved_automation_launch(launch, cwd)?,
            }
        }
        request => request,
    })
}

async fn handle_request(
    request: Request,
    state: &Arc<DaemonState>,
    peer: &PeerIdentity,
    connection_id: u64,
    active_subscriptions: usize,
    trusted_ui_client: bool,
) -> Result<Handled, ProtocolError> {
    let request = bind_current_terminal_incarnation(request, state, peer).await?;
    let audit_resource = request_policy_resources(&request, state)
        .await
        .unwrap_or_default()
        .first()
        .copied()
        .and_then(audit_resource);
    let authorization = match authorize_request(
        &request,
        state,
        peer,
        active_subscriptions,
        trusted_ui_client,
    )
    .await
    {
        Ok(authorization) => authorization,
        Err(error) => {
            append_request_audit(
                state,
                peer,
                &request,
                None,
                RequestAuditDisposition {
                    resource: audit_resource.clone(),
                    decision: splinterm_protocol::AuditDecision::Denied,
                    reason: "policy_rejected",
                    outcome: splinterm_protocol::AuditOutcome::Failed,
                },
            )
            .await;
            return Err(error);
        }
    };
    let mut result = handle_authorized_request(
        request.clone(),
        state,
        peer,
        connection_id,
        trusted_ui_client,
        &authorization,
    )
    .await;
    if let (Ok(handled), Some(maximum)) = (&result, authorization.maximum_returned_bytes()) {
        let encoded_bytes = serde_json::to_vec(&handled.response)
            .map_err(|_| internal())?
            .len();
        if encoded_bytes > maximum {
            result = Err(ProtocolError::new(
                ErrorCode::ResourceLimit,
                "response exceeds persistent policy byte limit",
            ));
        }
    }
    append_request_result_audit(
        state,
        peer,
        &request,
        &authorization,
        audit_resource,
        &result,
    )
    .await;
    result
}

#[allow(
    clippy::too_many_lines,
    reason = "authorization remains adjacent to every sensitive operation"
)]
async fn handle_authorized_request(
    request: Request,
    state: &Arc<DaemonState>,
    peer: &PeerIdentity,
    connection_id: u64,
    trusted_ui_client: bool,
    authorization: &RequestAuthorizationContext,
) -> Result<Handled, ProtocolError> {
    let request = resolve_automation_mutation(request, state).await?;
    let response = match request {
        Request::Ping => Response::Pong,
        Request::RequestImageContent { request } => {
            request.validate()?;
            if !trusted_ui_client || !peer.is_matching_splinterm() {
                return Err(ProtocolError::new(
                    ErrorCode::Unauthorized,
                    "image content is available only to the trusted local UI",
                ));
            }
            let content_id = splinterm_terminal::ImageContentId::new(request.content_id)
                .ok_or_else(|| invalid("image content identity must be nonzero"))?;
            let handle = current_handle(state, request.splint_id, request.incarnation).await?;
            let content = handle
                .image_content(content_id, request.generation, request.digest)
                .await
                .map_err(|error| image_content_error(&error))?;
            let transfer_peer = peer.transfer_peer().ok_or_else(|| {
                ProtocolError::new(
                    ErrorCode::AuthenticationFailed,
                    "persistent peer identity is required for image transfer",
                )
            })?;
            let transfer = state
                .image_transfers
                .lock()
                .await
                .mint(transfer_peer, &request, content, Instant::now())
                .map_err(|error| image_transfer_error(&error))?;
            state.image_transfer_expiry_changed.notify_one();
            Response::ImageContentReady { transfer }
        }
        Request::PrepareMutation { mutation } => Response::MutationPrepared {
            preparation: prepare_mutation(&topology_snapshot(state).await, mutation)?,
        },
        Request::ListDojos => {
            let lair = state.lair.read().await;
            Response::Dojos {
                dojos: lair.dojos().cloned().collect(),
                topology_revision: lair.revision(),
            }
        }
        Request::InspectTopology => Response::Topology {
            snapshot: topology_snapshot(state).await,
        },
        Request::SubscribeTopology => {
            let (id, snapshot, stream) = subscribe_topology(state).await;
            return Ok(Handled {
                response: Response::TopologySubscribed {
                    subscription_id: id,
                    snapshot,
                },
                subscription: Some(PendingSubscription::Topology {
                    id,
                    stream,
                    maximum_returned_bytes: authorization.maximum_returned_bytes(),
                }),
            });
        }
        Request::InspectSplint { splint_id } => {
            let snapshot = topology_snapshot(state).await;
            let runtime = snapshot
                .runtimes
                .iter()
                .find(|runtime| runtime.splint_id == splint_id)
                .cloned()
                .ok_or_else(not_found)?;
            let (dojo_id, window_id, title) =
                splint_containment(&snapshot.lair, splint_id).ok_or_else(not_found)?;
            Response::Splint {
                dojo_id,
                window_id,
                title,
                topology_revision: snapshot.revision,
                runtime,
            }
        }
        Request::RequestAccess {
            splint_id,
            incarnation,
            scopes,
        } => {
            let _ = current_handle(state, splint_id, incarnation).await?;
            let canonical: std::collections::BTreeSet<_> = scopes.into_iter().collect();
            if canonical.is_empty() || canonical.len() > splinterm_protocol::MAX_ACCESS_SCOPES {
                return Err(invalid("access scopes are empty or exceed limits"));
            }
            let scopes: Vec<_> = canonical.into_iter().collect();
            if authorization.policy_authorized() {
                grant_access_response(state, peer, splint_id, incarnation, scopes).await?
            } else if state.development_terminal_access {
                nonstored_access_response(
                    state,
                    development_grant(peer, splint_id, incarnation, scopes),
                )
                .await?
            } else if trusted_first_party_ui(trusted_ui_client, peer, &scopes) {
                nonstored_access_response(state, first_party_grant(splint_id, incarnation, scopes))
                    .await?
            } else if let Some(grant_id) =
                state
                    .grants
                    .lock()
                    .await
                    .authorize(peer, splint_id, incarnation, &scopes)
            {
                existing_access_response(state, grant_id, splint_id, incarnation).await?
            } else {
                let granted =
                    match consent::prompt(peer, splint_id, incarnation, scopes.clone()).await {
                        Ok(granted) => granted,
                        Err(error) => {
                            warn!(%error, "trusted consent client failed closed");
                            false
                        }
                    };
                if !granted {
                    state.grants.lock().await.deny(
                        peer,
                        splint_id,
                        incarnation,
                        &scopes,
                        "denied or consent client unavailable",
                    );
                    return Err(ProtocolError::new(
                        ErrorCode::ConsentDenied,
                        "access was denied",
                    ));
                }
                grant_access_response(state, peer, splint_id, incarnation, scopes).await?
            }
        }
        Request::AuthorizationStatus {
            splint_id,
            incarnation: requested_incarnation,
        } => {
            let handle = state
                .runtimes
                .lock()
                .await
                .handle(splint_id)
                .ok_or_else(not_found)?;
            let incarnation = handle.incarnation.value();
            if requested_incarnation.is_some_and(|requested| requested != incarnation) {
                return Err(ProtocolError::new(
                    ErrorCode::StaleIncarnation,
                    "requested incarnation is not current",
                ));
            }
            if !authorization.policy_authorized()
                && !state.development_terminal_access
                && !peer.is_matching_splinterm()
            {
                return Err(ProtocolError::new(
                    ErrorCode::Unauthorized,
                    "authorization status requires trusted UI or exact policy",
                ));
            }
            let lair = state.lair.read().await;
            let topology_revision = lair.revision();
            let (dojo_id, window_id, _) =
                splint_containment(&lair, splint_id).ok_or_else(not_found)?;
            drop(lair);
            let grants = state.grants.lock().await.status(splint_id, incarnation);
            let resource = request_policy_resources(
                &Request::AuthorizationStatus {
                    splint_id,
                    incarnation: Some(incarnation),
                },
                state,
            )
            .await
            .and_then(|resources| resources.into_iter().next());
            let policy = state.policy.lock().await;
            let policy_generation = policy.snapshot().id;
            let persistent = match (peer.persistent_executable(), resource) {
                (Some(executable), Some(resource)) => policy.status(
                    executable,
                    resource,
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                ),
                _ => Vec::new(),
            };
            Response::AuthorizationStatus {
                dojo_id,
                window_id,
                incarnation,
                topology_revision,
                policy_generation,
                grants,
                persistent,
                development_bypass: state.development_terminal_access,
            }
        }
        Request::RevokeAccess { grant_id } => {
            if !authorization.policy_authorized()
                && !state.development_terminal_access
                && !peer.is_matching_splinterm()
            {
                return Err(ProtocolError::new(
                    ErrorCode::Unauthorized,
                    "revocation requires trusted UI or exact policy",
                ));
            }
            let transaction = state
                .topology_transactions
                .acquire()
                .await
                .map_err(|_| internal())?;
            let (splint_id, _incarnation) = state
                .grants
                .lock()
                .await
                .grant_resource(grant_id)
                .ok_or_else(not_found)?;
            let containment =
                splint_containment(&*state.lair.read().await, splint_id).ok_or_else(not_found)?;
            let mutation = state
                .grants
                .lock()
                .await
                .revoke(grant_id)
                .ok_or_else(not_found)?;
            drop(transaction);
            revoke_grant_controllers(state, grant_id).await;
            let _ = state.revocations.send(Revocation { grant_id });
            info!(
                grant_id,
                splint_id = ?mutation.grant.splint_id,
                incarnation = mutation.grant.incarnation,
                "terminal access grant revoked"
            );
            Response::AccessRevoked {
                dojo_id: containment.0,
                window_id: containment.1,
                authorization_revision: mutation.authorization_revision,
                grant: mutation.grant,
            }
        }
        Request::CreateDojo {
            expected_topology_revision,
            name,
            launch,
        } => {
            let _transaction = state
                .topology_transactions
                .acquire()
                .await
                .map_err(|_| internal())?;
            launch.validate()?;
            let current = state.lair.read().await.revision();
            if current != expected_topology_revision {
                return Err(model_error(LairError::StaleTopology {
                    expected: expected_topology_revision,
                    current,
                }));
            }
            if name.len() > 128 {
                return Err(invalid("dojo name exceeds protocol limits"));
            }
            let mut dojo = Dojo::new(name, launch.cwd.clone());
            let dojo_id = dojo.id;
            let window_id = dojo.windows[0].id;
            let LayoutNode::Leaf(splint) = &mut dojo.windows[0].root else {
                unreachable!()
            };
            splint.command.clone_from(&launch.command);
            splint.launch = Box::new(durable_launch(&launch));
            let splint_id = splint.id;
            let context = SplintLaunchContext {
                dojo: dojo_id,
                window: window_id,
                splint: splint_id,
            };
            let runtime = spawn_runtime(state, context, &launch).await?;
            let handle = runtime.handle();
            let incarnation = handle.incarnation.value();
            splint.last_incarnation = Some(incarnation);
            splint.state = SplintState::Running;
            let previous = state.lair.read().await.clone();
            let prepared = durable_lair_candidate(state, |lair| {
                lair.insert_dojo_at(expected_topology_revision, dojo.clone())
            })
            .await;
            let (candidate, topology_revision) = match prepared {
                Ok(prepared) => prepared,
                Err(error) => {
                    let _ = runtime.shutdown().await;
                    return Err(error);
                }
            };
            let rejected = {
                let mut lair = state.lair.write().await;
                let mut runtimes = state.runtimes.lock().await;
                match runtimes.insert(runtime) {
                    Ok(()) => {
                        *lair = candidate;
                        None
                    }
                    Err(runtime) => Some(runtime),
                }
            };
            if let Some(runtime) = rejected {
                if persist_lair(state, &previous).await.is_err() {
                    error!("failed to roll back durable create after runtime registry rejection");
                }
                let _ = runtime.shutdown().await;
                return Err(ProtocolError::new(
                    ErrorCode::ResourceLimit,
                    "live Splint registry rejected the process",
                ));
            }
            observe_process_exit(state, handle);
            publish_topology(state, topology_revision, TopologyChangeKind::DojoCreated).await;
            Response::DojoCreated {
                dojo,
                incarnation,
                topology_revision,
            }
        }
        Request::SplitSplint {
            expected_topology_revision,
            target_splint_id,
            axis,
            side,
            ratio,
            launch,
        } => {
            let _transaction = state
                .topology_transactions
                .acquire()
                .await
                .map_err(|_| internal())?;
            launch.validate()?;
            {
                let lair = state.lair.read().await;
                if lair.revision() != expected_topology_revision {
                    return Err(model_error(LairError::StaleTopology {
                        expected: expected_topology_revision,
                        current: lair.revision(),
                    }));
                }
                if lair.find_splint(target_splint_id).is_none() {
                    return Err(not_found());
                }
            }
            if state.runtimes.lock().await.entries.len() >= MAX_LIVE_SPLINTS {
                return Err(ProtocolError::new(
                    ErrorCode::ResourceLimit,
                    "live Splint registry is full",
                ));
            }

            let mut splint = Splint::shell(launch.cwd.clone());
            splint.command.clone_from(&launch.command);
            splint.launch = Box::new(durable_launch(&launch));
            let splint_id = splint.id;
            let parent_context = splint_launch_context(&*state.lair.read().await, target_splint_id)
                .ok_or_else(not_found)?;
            let context = SplintLaunchContext {
                splint: splint_id,
                ..parent_context
            };
            let runtime = spawn_runtime(state, context, &launch).await?;
            let handle = runtime.handle();
            let incarnation = handle.incarnation.value();

            splint.last_incarnation = Some(incarnation);
            splint.state = SplintState::Running;
            let previous = state.lair.read().await.clone();
            let prepared = durable_lair_candidate(state, |lair| {
                lair.split_splint_at(
                    expected_topology_revision,
                    target_splint_id,
                    splint,
                    axis,
                    side,
                    ratio,
                )
            })
            .await;
            let (candidate, topology_revision) = match prepared {
                Ok(prepared) => prepared,
                Err(error) => {
                    let _ = runtime.shutdown().await;
                    return Err(error);
                }
            };
            let rejected = {
                let mut lair = state.lair.write().await;
                let mut runtimes = state.runtimes.lock().await;
                match runtimes.insert(runtime) {
                    Ok(()) => {
                        *lair = candidate;
                        None
                    }
                    Err(runtime) => Some(runtime),
                }
            };
            if let Some(runtime) = rejected {
                if persist_lair(state, &previous).await.is_err() {
                    error!("failed to roll back durable split after runtime registry rejection");
                }
                let _ = runtime.shutdown().await;
                return Err(ProtocolError::new(
                    ErrorCode::ResourceLimit,
                    "live Splint registry rejected the process",
                ));
            }
            observe_process_exit(state, handle);
            publish_topology(state, topology_revision, TopologyChangeKind::SplintSplit).await;
            Response::SplintStarted {
                splint_id,
                incarnation,
                topology_revision,
            }
        }
        Request::RelaunchSplint {
            expected_topology_revision,
            splint_id,
            launch,
        } => {
            let _transaction = state
                .topology_transactions
                .acquire()
                .await
                .map_err(|_| internal())?;
            require_current_topology_revision(state, expected_topology_revision).await?;
            let (incarnation, topology_revision) =
                start_exited_splint(state, splint_id, &launch).await?;
            publish_topology(state, topology_revision, TopologyChangeKind::RuntimeChanged).await;
            Response::SplintStarted {
                splint_id,
                incarnation,
                topology_revision,
            }
        }
        Request::RestoreSplint {
            expected_topology_revision,
            splint_id,
        } => {
            let _transaction = state
                .topology_transactions
                .acquire()
                .await
                .map_err(|_| internal())?;
            require_current_topology_revision(state, expected_topology_revision).await?;
            if state.lair.read().await.find_splint(splint_id).is_none() {
                return Err(not_found());
            }
            let (topology_revision, results) = restore_targets(state, vec![splint_id]).await;
            Response::RestoreCompleted {
                topology_revision,
                results,
            }
        }
        Request::RestoreWindow {
            expected_topology_revision,
            window_id,
        } => {
            let _transaction = state
                .topology_transactions
                .acquire()
                .await
                .map_err(|_| internal())?;
            require_current_topology_revision(state, expected_topology_revision).await?;
            let splint_ids = state
                .lair
                .read()
                .await
                .find_window(window_id)
                .map(|window| layout_splint_ids(&window.root))
                .ok_or_else(not_found)?;
            let (topology_revision, results) = restore_targets(state, splint_ids).await;
            Response::RestoreCompleted {
                topology_revision,
                results,
            }
        }
        Request::RestoreDojo {
            expected_topology_revision,
            dojo_id,
        } => {
            let _transaction = state
                .topology_transactions
                .acquire()
                .await
                .map_err(|_| internal())?;
            require_current_topology_revision(state, expected_topology_revision).await?;
            let splint_ids = state
                .lair
                .read()
                .await
                .dojos()
                .find(|dojo| dojo.id == dojo_id)
                .map(|dojo| {
                    dojo.windows
                        .iter()
                        .flat_map(|window| layout_splint_ids(&window.root))
                        .collect::<Vec<_>>()
                })
                .ok_or_else(not_found)?;
            let (topology_revision, results) = restore_targets(state, splint_ids).await;
            Response::RestoreCompleted {
                topology_revision,
                results,
            }
        }
        Request::CloseSplint {
            expected_topology_revision,
            splint_id,
        } => {
            let _transaction = state
                .topology_transactions
                .acquire()
                .await
                .map_err(|_| internal())?;
            let (candidate, topology_revision) = durable_lair_candidate(state, |lair| {
                lair.close_splint_at(expected_topology_revision, splint_id)
            })
            .await?;
            let runtime = {
                let mut lair = state.lair.write().await;
                let mut runtimes = state.runtimes.lock().await;
                *lair = candidate;
                runtimes.remove(splint_id)
            };
            if let Some(runtime) = runtime {
                runtime.shutdown().await.map_err(|_| internal())?;
            }
            publish_topology(state, topology_revision, TopologyChangeKind::SplintClosed).await;
            Response::TopologyCommitted { topology_revision }
        }
        Request::SetSplitRatio {
            expected_topology_revision,
            target_splint_id,
            ratio,
        } => {
            let _transaction = state
                .topology_transactions
                .acquire()
                .await
                .map_err(|_| internal())?;
            let (candidate, topology_revision) = durable_lair_candidate(state, |lair| {
                lair.set_split_ratio_at(expected_topology_revision, target_splint_id, ratio)
            })
            .await?;
            install_lair(state, candidate).await;
            publish_topology(
                state,
                topology_revision,
                TopologyChangeKind::SplitRatioChanged,
            )
            .await;
            Response::TopologyCommitted { topology_revision }
        }
        Request::NewWindow {
            expected_topology_revision,
            dojo_id,
            title,
            launch,
        } => {
            let _transaction = state
                .topology_transactions
                .acquire()
                .await
                .map_err(|_| internal())?;
            launch.validate()?;
            let current = state.lair.read().await.revision();
            if current != expected_topology_revision {
                return Err(model_error(LairError::StaleTopology {
                    expected: expected_topology_revision,
                    current,
                }));
            }
            let mut window = Window::with_shell(launch.cwd.clone());
            window.title = title;
            let LayoutNode::Leaf(splint) = &mut window.root else {
                unreachable!()
            };
            splint.command.clone_from(&launch.command);
            splint.launch = Box::new(durable_launch(&launch));
            let splint_id = splint.id;
            let window_id = window.id;
            let context = SplintLaunchContext {
                dojo: dojo_id,
                window: window_id,
                splint: splint_id,
            };
            let runtime = spawn_runtime(state, context, &launch).await?;
            let handle = runtime.handle();
            let incarnation = handle.incarnation.value();
            splint.last_incarnation = Some(incarnation);
            splint.state = SplintState::Running;
            let previous = state.lair.read().await.clone();
            let prepared = durable_lair_candidate(state, |lair| {
                lair.new_window_at(expected_topology_revision, dojo_id, window)
            })
            .await;
            let (candidate, topology_revision) = match prepared {
                Ok(prepared) => prepared,
                Err(error) => {
                    let _ = runtime.shutdown().await;
                    return Err(error);
                }
            };
            let rejected = {
                let mut lair = state.lair.write().await;
                let mut runtimes = state.runtimes.lock().await;
                match runtimes.insert(runtime) {
                    Ok(()) => {
                        *lair = candidate;
                        None
                    }
                    Err(runtime) => Some(runtime),
                }
            };
            if let Some(runtime) = rejected {
                if persist_lair(state, &previous).await.is_err() {
                    error!("failed to roll back durable window creation after registry rejection");
                }
                let _ = runtime.shutdown().await;
                return Err(ProtocolError::new(
                    ErrorCode::ResourceLimit,
                    "live Splint registry rejected the process",
                ));
            }
            observe_process_exit(state, handle);
            publish_topology(state, topology_revision, TopologyChangeKind::WindowCreated).await;
            Response::WindowStarted {
                window_id,
                splint_id,
                incarnation,
                topology_revision,
            }
        }
        Request::CloseWindow {
            expected_topology_revision,
            window_id,
        } => {
            let _transaction = state
                .topology_transactions
                .acquire()
                .await
                .map_err(|_| internal())?;
            let (candidate, (topology_revision, ids)) = durable_lair_candidate(state, |lair| {
                let ids = lair
                    .find_window(window_id)
                    .map(|window| layout_splint_ids(&window.root))
                    .ok_or(LairError::WindowNotFound(window_id))?;
                let revision = lair.close_window_at(expected_topology_revision, window_id)?;
                Ok((revision, ids))
            })
            .await?;
            let runtimes = {
                let mut lair = state.lair.write().await;
                let mut registry = state.runtimes.lock().await;
                *lair = candidate;
                ids.into_iter()
                    .filter_map(|id| registry.remove(id))
                    .collect::<Vec<_>>()
            };
            for runtime in runtimes {
                runtime.shutdown().await.map_err(|_| internal())?;
            }
            publish_topology(state, topology_revision, TopologyChangeKind::WindowClosed).await;
            Response::TopologyCommitted { topology_revision }
        }
        Request::RenameDojo {
            expected_topology_revision,
            dojo_id,
            name,
        } => {
            let _transaction = state
                .topology_transactions
                .acquire()
                .await
                .map_err(|_| internal())?;
            let (candidate, topology_revision) = durable_lair_candidate(state, |lair| {
                lair.rename_dojo_at(expected_topology_revision, dojo_id, name)
            })
            .await?;
            install_lair(state, candidate).await;
            publish_topology(state, topology_revision, TopologyChangeKind::DojoRenamed).await;
            Response::TopologyCommitted { topology_revision }
        }
        Request::RenameWindow {
            expected_topology_revision,
            window_id,
            title,
        } => {
            let _transaction = state
                .topology_transactions
                .acquire()
                .await
                .map_err(|_| internal())?;
            let (candidate, topology_revision) = durable_lair_candidate(state, |lair| {
                lair.rename_window_at(expected_topology_revision, window_id, title)
            })
            .await?;
            install_lair(state, candidate).await;
            publish_topology(state, topology_revision, TopologyChangeKind::WindowRenamed).await;
            Response::TopologyCommitted { topology_revision }
        }
        Request::SetWindowDefaultFocus {
            expected_topology_revision,
            window_id,
            splint_id,
        } => {
            let _transaction = state
                .topology_transactions
                .acquire()
                .await
                .map_err(|_| internal())?;
            let (candidate, topology_revision) = durable_lair_candidate(state, |lair| {
                lair.set_window_default_focus_at(expected_topology_revision, window_id, splint_id)
            })
            .await?;
            install_lair(state, candidate).await;
            publish_topology(
                state,
                topology_revision,
                TopologyChangeKind::WindowDefaultFocusChanged,
            )
            .await;
            Response::TopologyCommitted { topology_revision }
        }
        Request::RenameSplint {
            expected_topology_revision,
            splint_id,
            title,
        } => {
            let _transaction = state
                .topology_transactions
                .acquire()
                .await
                .map_err(|_| internal())?;
            let (candidate, topology_revision) = durable_lair_candidate(state, |lair| {
                lair.rename_splint_at(expected_topology_revision, splint_id, title)
            })
            .await?;
            install_lair(state, candidate).await;
            publish_topology(state, topology_revision, TopologyChangeKind::SplintRenamed).await;
            Response::TopologyCommitted { topology_revision }
        }
        Request::Attach {
            splint_id,
            incarnation,
            scrollback_rows,
        } => {
            let incarnation = incarnation.ok_or_else(internal)?;
            let include_images =
                include_image_metadata(trusted_ui_client, peer.is_matching_splinterm());
            let required = if scrollback_rows == 0 {
                vec![AccessScope::Observe]
            } else {
                vec![AccessScope::Observe, AccessScope::Scrollback]
            };
            let grant_id = if authorization.policy_authorized() {
                None
            } else {
                authorize_scope(
                    state,
                    peer,
                    trusted_ui_client,
                    splint_id,
                    incarnation,
                    &required,
                )
                .await?
            };
            let handle = current_handle(state, splint_id, incarnation).await?;
            let scrollback_rows = scrollback_rows.min(MAX_SNAPSHOT_SCROLLBACK_ROWS);
            let (snapshot, subscription) = handle
                .attach_with_scrollback(scrollback_rows)
                .await
                .map_err(|_| internal())?;
            let id = NEXT_SUBSCRIPTION.fetch_add(1, Ordering::Relaxed);
            let history = history_state(&snapshot);
            let visible_rows = snapshot.visible_rows.clone();
            let provenance = terminal_provenance(
                state,
                splint_id,
                incarnation,
                snapshot.revision.value(),
                snapshot.scrollback.history_generation,
                snapshot.title.clone(),
            )
            .await?;
            return Ok(Handled {
                response: Response::Attached {
                    subscription_id: id,
                    provenance,
                    snapshot: wire_snapshot(snapshot, include_images),
                },
                subscription: Some(PendingSubscription::Terminal {
                    id,
                    stream: subscription,
                    handle,
                    access: SubscriptionAccess {
                        grant_id,
                        maximum_returned_bytes: authorization.maximum_returned_bytes(),
                        scrollback_rows,
                        include_images,
                        history,
                        visible_rows,
                    },
                }),
            });
        }
        Request::StartScrollbackPage {
            splint_id,
            incarnation,
            max_rows,
        } => {
            let incarnation = incarnation.ok_or_else(internal)?;
            if max_rows == 0 || max_rows > MAX_SCROLLBACK_PAGE_ROWS {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidArgument,
                    "scrollback page request exceeds protocol bounds",
                ));
            }
            if !authorization.policy_authorized() {
                let _ = authorize_scope(
                    state,
                    peer,
                    trusted_ui_client,
                    splint_id,
                    incarnation,
                    &[AccessScope::Observe, AccessScope::Scrollback],
                )
                .await?;
            }
            let page = current_handle(state, splint_id, incarnation)
                .await?
                .start_scrollback_page(max_rows)
                .await
                .map_err(|_| internal())?;
            scrollback_response(state, splint_id, incarnation, page).await?
        }
        Request::ScrollbackPage {
            splint_id,
            incarnation,
            terminal_revision,
            history_generation,
            before_row_id,
            max_rows,
        } => {
            if before_row_id == 0 || max_rows == 0 || max_rows > MAX_SCROLLBACK_PAGE_ROWS {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidArgument,
                    "scrollback page request exceeds protocol bounds",
                ));
            }
            if !authorization.policy_authorized() {
                let _ = authorize_scope(
                    state,
                    peer,
                    trusted_ui_client,
                    splint_id,
                    incarnation,
                    &[AccessScope::Observe, AccessScope::Scrollback],
                )
                .await?;
            }
            let page = current_handle(state, splint_id, incarnation)
                .await?
                .scrollback_page(before_row_id, max_rows)
                .await
                .map_err(|_| internal())?;
            if page.terminal_revision.value() != terminal_revision
                || page.history_generation != history_generation
            {
                return Ok(Handled {
                    response: scrollback_resync_response(state, splint_id, incarnation, page)
                        .await?,
                    subscription: None,
                });
            }
            scrollback_response(state, splint_id, incarnation, page).await?
        }
        Request::StartSearchScrollback {
            splint_id,
            incarnation,
            query,
            case_sensitive,
            max_results,
        } => {
            let incarnation = incarnation.ok_or_else(internal)?;
            if query.is_empty()
                || query.len() > MAX_SEARCH_QUERY_BYTES
                || max_results == 0
                || max_results > MAX_SEARCH_RESULTS
            {
                return Err(invalid("search request exceeds protocol bounds"));
            }
            if !authorization.policy_authorized() {
                let _ = authorize_scope(
                    state,
                    peer,
                    trusted_ui_client,
                    splint_id,
                    incarnation,
                    &[AccessScope::Observe, AccessScope::Scrollback],
                )
                .await?;
            }
            let search = current_handle(state, splint_id, incarnation)
                .await?
                .search(query, case_sensitive, 0, max_results, SEARCH_DEADLINE)
                .await
                .map_err(|_| internal())?;
            search_response(state, splint_id, incarnation, search).await?
        }
        Request::SearchScrollback {
            splint_id,
            incarnation,
            terminal_revision,
            history_generation,
            query,
            case_sensitive,
            cursor,
            max_results,
        } => {
            if query.is_empty()
                || query.len() > MAX_SEARCH_QUERY_BYTES
                || max_results == 0
                || max_results > MAX_SEARCH_RESULTS
            {
                return Err(invalid("search request exceeds protocol bounds"));
            }
            let skip_rows = decode_search_cursor(cursor.as_deref())?;
            if !authorization.policy_authorized() {
                let _ = authorize_scope(
                    state,
                    peer,
                    trusted_ui_client,
                    splint_id,
                    incarnation,
                    &[AccessScope::Observe, AccessScope::Scrollback],
                )
                .await?;
            }
            let search = current_handle(state, splint_id, incarnation)
                .await?
                .search(
                    query,
                    case_sensitive,
                    skip_rows,
                    max_results,
                    SEARCH_DEADLINE,
                )
                .await
                .map_err(|_| internal())?;
            if search.terminal_revision.value() != terminal_revision
                || search.history_generation != history_generation
            {
                return Ok(Handled {
                    response: search_resync_response(state, splint_id, incarnation, search).await?,
                    subscription: None,
                });
            }
            search_response(state, splint_id, incarnation, search).await?
        }
        Request::AcquireControl {
            splint_id,
            incarnation,
            modes,
        } => {
            splinterm_protocol::validate_control_modes(&modes)?;
            let grant_id = if authorization.policy_authorized()
                || state.development_terminal_access
                || trusted_first_party_ui(
                    trusted_ui_client,
                    peer,
                    &[AccessScope::Input, AccessScope::Resize],
                ) {
                None
            } else {
                let required = modes
                    .iter()
                    .map(|mode| match mode {
                        splinterm_protocol::ControlMode::Input => AccessScope::Input,
                        splinterm_protocol::ControlMode::Resize => AccessScope::Resize,
                    })
                    .collect::<Vec<_>>();
                state
                    .grants
                    .lock()
                    .await
                    .authorize(peer, splint_id, incarnation, &required)
                    .map(Some)
                    .ok_or_else(|| {
                        ProtocolError::new(
                            ErrorCode::Unauthorized,
                            "requested controller modes require consent",
                        )
                    })?
            };
            let _ = current_handle(state, splint_id, incarnation).await?;
            let lease = state.controller.lock().await.acquire(
                connection_id,
                splint_id,
                incarnation,
                grant_id,
            )?;
            publish_control_status(state, splint_id, incarnation).await;
            let (dojo_id, window_id, _) =
                splint_containment(&*state.lair.read().await, splint_id).ok_or_else(not_found)?;
            Response::ControlGranted {
                controller_id: lease.id,
                dojo_id,
                window_id,
            }
        }
        Request::SubscribeControl {
            splint_id,
            incarnation,
        } => {
            if !authorization.policy_authorized()
                && !state.development_terminal_access
                && !trusted_first_party_ui(trusted_ui_client, peer, &[AccessScope::Observe])
            {
                return Err(ProtocolError::new(
                    ErrorCode::Unauthorized,
                    "control status is restricted to the trusted first-party UI",
                ));
            }
            let _ = current_handle(state, splint_id, incarnation).await?;
            let status =
                state
                    .controller
                    .lock()
                    .await
                    .status(connection_id, splint_id, incarnation);
            let id = NEXT_SUBSCRIPTION.fetch_add(1, Ordering::Relaxed);
            return Ok(Handled {
                response: Response::ControlSubscribed {
                    subscription_id: id,
                    status,
                },
                subscription: Some(PendingSubscription::Control {
                    id,
                    stream: state.control_events.subscribe(),
                    connection_id,
                    splint_id,
                    incarnation,
                    maximum_returned_bytes: authorization.maximum_returned_bytes(),
                }),
            });
        }
        Request::RequestControlTransfer {
            splint_id,
            incarnation,
            modes,
        } => {
            splinterm_protocol::validate_control_modes(&modes)?;
            if !authorization.policy_authorized()
                && !state.development_terminal_access
                && !trusted_first_party_ui(
                    trusted_ui_client,
                    peer,
                    &[AccessScope::Input, AccessScope::Resize],
                )
            {
                return Err(ProtocolError::new(
                    ErrorCode::Unauthorized,
                    "control transfer is restricted to the trusted first-party UI",
                ));
            }
            let _ = current_handle(state, splint_id, incarnation).await?;
            let transfer = state.controller.lock().await.request_transfer(
                connection_id,
                splint_id,
                incarnation,
            )?;
            publish_control_notice(state, ControlNotice::TransferRequested(transfer));
            schedule_transfer_timeout(Arc::clone(state), transfer);
            let (dojo_id, window_id, _) =
                splint_containment(&*state.lair.read().await, splint_id).ok_or_else(not_found)?;
            Response::ControlTransferPending {
                transfer_id: transfer.id,
                dojo_id,
                window_id,
            }
        }
        Request::DecideControlTransfer {
            transfer_id,
            decision,
        } => {
            let (transfer, outcome, lease) = state.controller.lock().await.decide_transfer(
                connection_id,
                transfer_id,
                decision,
            )?;
            publish_control_notice(
                state,
                ControlNotice::TransferResolved {
                    transfer,
                    outcome,
                    controller_id: lease.map(|lease| lease.id),
                },
            );
            if outcome == ControlTransferOutcome::Granted {
                publish_control_status(state, transfer.splint_id, transfer.incarnation).await;
            }
            Response::ControlTransferDecided {
                outcome,
                controller_id: lease.map(|lease| lease.id),
            }
        }
        Request::ForceControlTransfer {
            splint_id,
            incarnation,
        } => {
            if !trusted_first_party_ui(
                trusted_ui_client,
                peer,
                &[AccessScope::Input, AccessScope::Resize],
            ) {
                return Err(ProtocolError::new(
                    ErrorCode::Unauthorized,
                    "forced transfer is restricted to the trusted first-party UI",
                ));
            }
            let _ = current_handle(state, splint_id, incarnation).await?;
            let confirmed = consent::prompt(
                peer,
                splint_id,
                incarnation,
                vec![AccessScope::ControlTakeover],
            )
            .await
            .map_err(|error| {
                warn!(%error, "trusted forced-control confirmation failed closed");
                ProtocolError::new(
                    ErrorCode::ConsentUnavailable,
                    "trusted confirmation unavailable",
                )
            })?;
            if !confirmed {
                return Err(ProtocolError::new(
                    ErrorCode::ConsentDenied,
                    "forced control transfer denied",
                ));
            }
            let lease = state.controller.lock().await.force_transfer(
                connection_id,
                splint_id,
                incarnation,
            )?;
            publish_control_status(state, splint_id, incarnation).await;
            let (dojo_id, window_id, _) =
                splint_containment(&*state.lair.read().await, splint_id).ok_or_else(not_found)?;
            Response::ControlGranted {
                controller_id: lease.id,
                dojo_id,
                window_id,
            }
        }
        Request::ReleaseControl { controller_id } => {
            let lease = state
                .controller
                .lock()
                .await
                .release_owned(connection_id, controller_id)
                .ok_or_else(|| {
                    ProtocolError::new(
                        ErrorCode::Unauthorized,
                        "controller lease is not owned by this connection",
                    )
                })?;
            publish_control_status(state, lease.splint_id, lease.incarnation).await;
            Response::Acknowledged
        }
        Request::Input {
            controller_id,
            splint_id,
            incarnation,
            bytes,
        } => {
            if !authorization.policy_authorized() {
                let _ = authorize_scope(
                    state,
                    peer,
                    trusted_ui_client,
                    splint_id,
                    incarnation,
                    &[AccessScope::Input],
                )
                .await?;
            }
            if bytes.len() > MAX_INPUT_BYTES {
                return Err(invalid("input exceeds limit"));
            }
            let handle =
                controlled_handle(state, connection_id, controller_id, splint_id, incarnation)
                    .await?;
            handle.input(bytes).await.map_err(|_| internal())?;
            terminal_action_acknowledgement(state, &handle).await?
        }
        Request::Resize {
            controller_id,
            splint_id,
            incarnation,
            columns,
            rows,
            pixel_width,
            pixel_height,
        } => {
            if !authorization.policy_authorized() {
                let _ = authorize_scope(
                    state,
                    peer,
                    trusted_ui_client,
                    splint_id,
                    incarnation,
                    &[AccessScope::Resize],
                )
                .await?;
            }
            if columns == 0 || rows == 0 || columns > MAX_COLUMNS || rows > MAX_ROWS {
                return Err(invalid("terminal dimensions exceed limits"));
            }
            let handle =
                controlled_handle(state, connection_id, controller_id, splint_id, incarnation)
                    .await?;
            handle
                .resize(PtySize {
                    columns,
                    rows,
                    pixel_width,
                    pixel_height,
                })
                .await
                .map_err(|_| invalid("resize rejected"))?;
            let _transaction = state
                .topology_transactions
                .acquire()
                .await
                .map_err(|_| internal())?;
            let mut candidate = state.lair.read().await.clone();
            if candidate.set_splint_dimensions(splint_id, columns, rows) {
                persist_lair(state, &candidate).await?;
                install_lair(state, candidate).await;
            }
            terminal_action_acknowledgement(state, &handle).await?
        }
        Request::Detach { .. } => Response::Acknowledged,
        Request::KillSplint {
            splint_id,
            incarnation,
        } => {
            if !authorization.policy_authorized() {
                let _ = authorize_scope(
                    state,
                    peer,
                    trusted_ui_client,
                    splint_id,
                    incarnation,
                    &[AccessScope::Terminate],
                )
                .await?;
            }
            let runtime = state
                .runtimes
                .lock()
                .await
                .remove(splint_id)
                .ok_or_else(not_found)?;
            let handle = runtime.handle();
            if handle.incarnation.value() != incarnation {
                let _ = state.runtimes.lock().await.insert(runtime);
                return Err(ProtocolError::new(
                    ErrorCode::StaleIncarnation,
                    "process incarnation is stale",
                ));
            }
            state
                .controller
                .lock()
                .await
                .release_identity(splint_id, incarnation);
            publish_control_status(state, splint_id, incarnation).await;
            let revoked = state.grants.lock().await.revoke_identity(
                splint_id,
                incarnation,
                "process terminated",
            );
            for grant_id in revoked {
                let _ = state.revocations.send(Revocation { grant_id });
            }
            let status = runtime.shutdown().await.map_err(|_| internal())?;
            let code = status
                .code
                .or_else(|| status.signal.map(|signal| 128 + signal))
                .unwrap_or(1);
            finalize_exit_if_current(state, splint_id, incarnation, code).await;
            Response::SplintKilled {
                splint_id,
                incarnation,
                exit_status: ProcessExitStatus {
                    code: status.code,
                    signal: status.signal,
                },
            }
        }
        Request::CreateDojoAutomation { .. }
        | Request::SplitSplintAutomation { .. }
        | Request::RelaunchSplintAutomation { .. }
        | Request::NewWindowAutomation { .. } => {
            unreachable!("automation launch requests are resolved before dispatch")
        }
        Request::AuditInspect {
            after_audit_id,
            max_records,
        } => {
            if max_records == 0 || max_records > splinterm_protocol::MAX_AUDIT_PAGE_RECORDS {
                return Err(invalid("audit page request exceeds protocol bounds"));
            }
            Response::AuditPage {
                page: state.audit.lock().await.page(after_audit_id, max_records),
            }
        }
    };
    Ok(Handled {
        response,
        subscription: None,
    })
}

async fn subscribe_topology(state: &DaemonState) -> (u64, TopologySnapshot, TopologySubscription) {
    let lair_guard = state.lair.read().await;
    let lair = lair_guard.clone();
    let live = state.runtimes.lock().await.handles();
    let snapshot = topology_snapshot_from(lair, &live);
    let id = NEXT_SUBSCRIPTION.fetch_add(1, Ordering::Relaxed);
    let subscription = state.topology.lock().await.subscribe(id);
    drop(lair_guard);
    (id, snapshot, subscription)
}

async fn topology_snapshot(state: &DaemonState) -> TopologySnapshot {
    let lair = state.lair.read().await.clone();
    let live = state.runtimes.lock().await.handles();
    topology_snapshot_from(lair, &live)
}

fn topology_snapshot_from(
    lair: Lair,
    live: &HashMap<SplintId, LiveSplintHandle>,
) -> TopologySnapshot {
    let mut runtimes = Vec::new();
    for dojo in lair.dojos() {
        for window in &dojo.windows {
            collect_runtime_summaries(&window.root, live, &mut runtimes);
        }
    }
    TopologySnapshot {
        revision: lair.revision(),
        lair,
        runtimes,
    }
}

fn layout_splint_ids(node: &LayoutNode) -> Vec<SplintId> {
    match node {
        LayoutNode::Leaf(splint) => vec![splint.id],
        LayoutNode::Branch { first, second, .. } => {
            let mut ids = layout_splint_ids(first);
            ids.extend(layout_splint_ids(second));
            ids
        }
    }
}

fn collect_runtime_summaries(
    node: &LayoutNode,
    live: &HashMap<SplintId, LiveSplintHandle>,
    summaries: &mut Vec<SplintRuntimeSummary>,
) {
    match node {
        LayoutNode::Leaf(splint) => {
            let matching_live = live.get(&splint.id);
            let (lifecycle, exit_status) = match splint.state {
                SplintState::Starting => (SplintLifecycle::Starting, None),
                SplintState::Running => (SplintLifecycle::Running, None),
                SplintState::Exited(code) => (
                    SplintLifecycle::Exited,
                    Some(ProcessExitStatus {
                        code: Some(code),
                        signal: None,
                    }),
                ),
            };
            summaries.push(SplintRuntimeSummary {
                splint_id: splint.id,
                live_incarnation: if matches!(lifecycle, SplintLifecycle::Exited) {
                    None
                } else {
                    matching_live.map(|handle| handle.incarnation.value())
                },
                last_incarnation: splint.last_incarnation,
                restorable: matches!(splint.state, SplintState::Exited(_)),
                lifecycle,
                exit_status,
            });
        }
        LayoutNode::Branch { first, second, .. } => {
            collect_runtime_summaries(first, live, summaries);
            collect_runtime_summaries(second, live, summaries);
        }
    }
}

async fn current_handle(
    state: &DaemonState,
    splint_id: SplintId,
    incarnation: u64,
) -> Result<LiveSplintHandle, ProtocolError> {
    let handle = state
        .runtimes
        .lock()
        .await
        .handle(splint_id)
        .ok_or_else(not_found)?;
    if handle.incarnation.value() != incarnation {
        return Err(ProtocolError::new(
            ErrorCode::StaleIncarnation,
            "process incarnation is stale",
        ));
    }
    Ok(handle)
}

struct ControlSubscriptionContext {
    state: Arc<DaemonState>,
    connection_id: u64,
    splint_id: SplintId,
    incarnation: u64,
    maximum_returned_bytes: Option<usize>,
}

fn spawn_control_subscription(
    id: u64,
    mut stream: broadcast::Receiver<ControlNotice>,
    outbound: mpsc::Sender<ServerFrame>,
    context: ControlSubscriptionContext,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let ControlSubscriptionContext {
            state,
            connection_id,
            splint_id,
            incarnation,
            maximum_returned_bytes,
        } = context;
        let mut sequence = 1_u64;
        loop {
            let event = match stream.recv().await {
                Ok(ControlNotice::Status {
                    splint_id: event_splint,
                    incarnation: event_incarnation,
                    owner_connection_id,
                }) if event_splint == splint_id && event_incarnation == incarnation => {
                    Some(SubscriptionEvent::ControlStatusChanged {
                        status: ControlStatus {
                            splint_id,
                            incarnation,
                            controlled: owner_connection_id.is_some(),
                            locally_owned: owner_connection_id == Some(connection_id),
                        },
                    })
                }
                Ok(ControlNotice::TransferRequested(transfer))
                    if transfer.splint_id == splint_id
                        && transfer.incarnation == incarnation
                        && transfer.owner_connection_id == connection_id =>
                {
                    Some(SubscriptionEvent::ControlTransferRequested {
                        transfer_id: transfer.id,
                    })
                }
                Ok(ControlNotice::TransferResolved {
                    transfer,
                    outcome,
                    controller_id,
                }) if transfer.splint_id == splint_id
                    && transfer.incarnation == incarnation
                    && (transfer.owner_connection_id == connection_id
                        || transfer.requester_connection_id == connection_id) =>
                {
                    Some(SubscriptionEvent::ControlTransferResolved {
                        transfer_id: transfer.id,
                        outcome,
                        controller_id: (transfer.requester_connection_id == connection_id)
                            .then_some(controller_id)
                            .flatten(),
                    })
                }
                Ok(_) => None,
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let status =
                        state
                            .controller
                            .lock()
                            .await
                            .status(connection_id, splint_id, incarnation);
                    Some(SubscriptionEvent::ControlStatusChanged { status })
                }
                Err(broadcast::error::RecvError::Closed) => break,
            };
            let Some(event) = event else { continue };
            let frame = ServerFrame::Event {
                subscription_id: id,
                sequence,
                event,
            };
            if !frame_within_policy_limit(&frame, maximum_returned_bytes)
                || outbound.send(frame).await.is_err()
            {
                break;
            }
            sequence = sequence.saturating_add(1);
        }
    })
}

fn spawn_topology_subscription(
    id: u64,
    mut subscription: TopologySubscription,
    outbound: mpsc::Sender<ServerFrame>,
    maximum_returned_bytes: Option<usize>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut sequence = 1_u64;
        loop {
            let (event, resync_required) = tokio::select! {
                biased;
                changed = subscription.resync.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let Some(current_revision) = *subscription.resync.borrow_and_update() else {
                        continue;
                    };
                    (
                        SubscriptionEvent::TopologyResyncRequired { current_revision },
                        true,
                    )
                }
                change = subscription.changes.recv() => {
                    let Some(change) = change else { break; };
                    (
                        SubscriptionEvent::TopologyChanged {
                            change: (*change).clone(),
                        },
                        false,
                    )
                }
            };
            let frame = ServerFrame::Event {
                subscription_id: id,
                sequence,
                event,
            };
            if !frame_within_policy_limit(&frame, maximum_returned_bytes)
                || outbound.send(frame).await.is_err()
            {
                break;
            }
            if resync_required {
                break;
            }
            sequence = sequence.saturating_add(1);
        }
    })
}

struct SubscriptionOutputs {
    outbound: mpsc::Sender<ServerFrame>,
    control: mpsc::Sender<ServerFrame>,
}

struct SubscriptionAudit {
    state: Arc<DaemonState>,
    peer: Option<splinterm_protocol::AuditPeer>,
}

async fn record_subscription_expiry(audit: &SubscriptionAudit, handle: &LiveSplintHandle) {
    let Some(peer) = audit.peer.clone() else {
        return;
    };
    audit.state.audit.lock().await.record(audit::AuditDraft {
        unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        policy_generation: None,
        policy_rule_id: None,
        peer,
        operation: splinterm_protocol::AuditOperation::RequestAccess,
        resource: Some(splinterm_protocol::AuditResource {
            dojo_id: None,
            window_id: None,
            splint_id: Some(handle.splint_id),
            incarnation: Some(handle.incarnation.value()),
        }),
        requested_scopes: Vec::new(),
        decision: splinterm_protocol::AuditDecision::Expired,
        reason: "grant_expired",
        outcome: Some(splinterm_protocol::AuditOutcome::Cancelled),
        argument_count: None,
        executable_basename: None,
    });
}

async fn send_access_revoked(
    control: &mpsc::Sender<ServerFrame>,
    subscription_id: u64,
    sequence: u64,
    grant_id: u64,
) {
    let _ = control
        .send(ServerFrame::Event {
            subscription_id,
            sequence,
            event: SubscriptionEvent::AccessRevoked { grant_id },
        })
        .await;
}

fn frame_within_policy_limit(frame: &ServerFrame, maximum: Option<usize>) -> bool {
    maximum.is_none_or(|maximum| {
        serde_json::to_vec(frame).is_ok_and(|encoded| encoded.len() <= maximum)
    })
}

enum DrainedSubscription {
    Open,
    Exited(ProcessExit),
    Closed,
    ResnapshotRequired,
}

fn drain_pending_subscription(
    subscription: &mut Subscription,
    updates: &mut Vec<TerminalUpdate>,
    snapshot: &mut Box<LiveSnapshot>,
) -> DrainedSubscription {
    loop {
        if subscription.resnapshot_required() {
            return DrainedSubscription::ResnapshotRequired;
        }
        match subscription.events.try_recv() {
            Ok(LiveEvent::Update {
                updates: pending,
                snapshot: pending_snapshot,
                ..
            }) => {
                updates.extend(pending);
                *snapshot = pending_snapshot;
            }
            Ok(LiveEvent::Exited { status, .. }) => {
                return DrainedSubscription::Exited(status);
            }
            Err(mpsc::error::TryRecvError::Disconnected) => {
                return DrainedSubscription::Closed;
            }
            Err(mpsc::error::TryRecvError::Empty) => {
                return if subscription.resnapshot_required() {
                    DrainedSubscription::ResnapshotRequired
                } else {
                    DrainedSubscription::Open
                };
            }
        }
    }
}

fn spawn_subscription(
    id: u64,
    mut subscription: Subscription,
    handle: LiveSplintHandle,
    outputs: SubscriptionOutputs,
    mut revocations: broadcast::Receiver<Revocation>,
    access: SubscriptionAccess,
    audit: SubscriptionAudit,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut sequence = 1_u64;
        let mut previous_history = access.history;
        let mut previous_visible_rows = access.visible_rows.clone();
        let expiry = time::sleep(consent::GRANT_LIFETIME);
        tokio::pin!(expiry);
        loop {
            let received = tokio::select! {
                value = subscription.recv() => value,
                revoked = revocations.recv(), if access.grant_id.is_some() => {
                    match revoked {
                        Ok(revocation) if Some(revocation.grant_id) == access.grant_id => {
                            send_access_revoked(
                                &outputs.control,
                                id,
                                sequence,
                                revocation.grant_id,
                            )
                            .await;
                            break;
                        }
                        Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                () = &mut expiry, if access.grant_id.is_some() => {
                    if let Some(grant_id) = access.grant_id {
                        send_access_revoked(&outputs.control, id, sequence, grant_id).await;
                        record_subscription_expiry(&audit, &handle).await;
                    }
                    break;
                }
            };
            match received {
                SubscriptionReceive::ResnapshotRequired => {
                    let revision = current_revision(&handle, access.scrollback_rows).await;
                    let _ = outputs
                        .control
                        .send(ServerFrame::Event {
                            subscription_id: id,
                            sequence,
                            event: SubscriptionEvent::ResyncRequired {
                                current_revision: revision,
                            },
                        })
                        .await;
                    break;
                }
                SubscriptionReceive::Event(LiveEvent::Exited { status, .. }) => {
                    let _ = outputs.outbound.try_send(ServerFrame::Event {
                        subscription_id: id,
                        sequence,
                        event: SubscriptionEvent::Exited {
                            code: status.code,
                            signal: status.signal,
                        },
                    });
                    break;
                }
                SubscriptionReceive::Event(LiveEvent::Update {
                    mut updates,
                    mut snapshot,
                    ..
                }) => {
                    let drained =
                        drain_pending_subscription(&mut subscription, &mut updates, &mut snapshot);
                    if matches!(drained, DrainedSubscription::ResnapshotRequired) {
                        let revision = current_revision(&handle, access.scrollback_rows).await;
                        let _ = outputs
                            .control
                            .send(ServerFrame::Event {
                                subscription_id: id,
                                sequence,
                                event: SubscriptionEvent::ResyncRequired {
                                    current_revision: revision,
                                },
                            })
                            .await;
                        break;
                    }
                    let current_history = history_state(&snapshot);
                    if revision_advances(previous_history.revision, current_history.revision) {
                        let next_visible_rows = snapshot.visible_rows.clone();
                        let event = subscription_update_event(
                            &updates,
                            *snapshot,
                            previous_history,
                            &previous_visible_rows,
                            access.include_images,
                        );
                        previous_history = current_history;
                        previous_visible_rows = next_visible_rows;
                        let frame = ServerFrame::Event {
                            subscription_id: id,
                            sequence,
                            event,
                        };
                        if !frame_within_policy_limit(&frame, access.maximum_returned_bytes)
                            || outputs.outbound.try_send(frame).is_err()
                        {
                            let revision = current_revision(&handle, access.scrollback_rows).await;
                            let _ = outputs
                                .control
                                .send(ServerFrame::Event {
                                    subscription_id: id,
                                    sequence,
                                    event: SubscriptionEvent::ResyncRequired {
                                        current_revision: revision,
                                    },
                                })
                                .await;
                            break;
                        }
                        sequence += 1;
                    }
                    match drained {
                        DrainedSubscription::Exited(status) => {
                            let _ = outputs.outbound.try_send(ServerFrame::Event {
                                subscription_id: id,
                                sequence,
                                event: SubscriptionEvent::Exited {
                                    code: status.code,
                                    signal: status.signal,
                                },
                            });
                            break;
                        }
                        DrainedSubscription::Closed => break,
                        DrainedSubscription::Open | DrainedSubscription::ResnapshotRequired => {}
                    }
                }
                SubscriptionReceive::Closed => break,
            }
        }
    })
}

async fn current_revision(handle: &LiveSplintHandle, scrollback_rows: usize) -> u64 {
    handle
        .snapshot_with_scrollback(scrollback_rows)
        .await
        .map_or(0, |snapshot| snapshot.revision.value())
}

fn history_state(snapshot: &LiveSnapshot) -> HistoryState {
    HistoryState {
        revision: snapshot.revision.value(),
        generation: snapshot.scrollback.history_generation,
        available_rows: snapshot.scrollback.available_rows,
    }
}

fn revisions_match(updates: &[TerminalUpdate], snapshot_revision: u64) -> bool {
    updates
        .last()
        .is_some_and(|update| update.revision().value() == snapshot_revision)
}

fn revision_advances(previous_revision: u64, current_revision: u64) -> bool {
    current_revision > previous_revision
}

fn subscription_update_event(
    updates: &[TerminalUpdate],
    snapshot: LiveSnapshot,
    previous_history: HistoryState,
    previous_visible_rows: &[splinterd::LiveRow],
    include_images: bool,
) -> SubscriptionEvent {
    if !revisions_match(updates, snapshot.revision.value()) {
        return SubscriptionEvent::Snapshot {
            snapshot: wire_snapshot(snapshot, include_images),
        };
    }
    match wire_update(
        updates,
        &snapshot,
        previous_history.revision,
        previous_history,
        previous_visible_rows,
        include_images,
    ) {
        Ok(update) => SubscriptionEvent::Update { update },
        Err(_) => SubscriptionEvent::ResyncRequired {
            current_revision: snapshot.revision.value(),
        },
    }
}

async fn write_frames(
    mut writer: OwnedWriteHalf,
    mut normal: mpsc::Receiver<ServerFrame>,
    mut control: mpsc::Receiver<ServerFrame>,
) {
    loop {
        let frame = tokio::select! { biased; frame = control.recv() => frame, frame = normal.recv() => frame };
        let Some(frame) = frame else { break };
        let Ok(encoded) = encode_frame(&frame) else {
            break;
        };
        if time::timeout(WRITE_TIMEOUT, writer.write_all(&encoded))
            .await
            .is_err()
        {
            break;
        }
    }
}

async fn read_frame(reader: &mut OwnedReadHalf) -> Result<ClientFrame> {
    read_optional_frame(reader)
        .await?
        .context("connection closed")
}

async fn read_optional_frame(reader: &mut OwnedReadHalf) -> Result<Option<ClientFrame>> {
    let mut length = [0_u8; 4];
    match reader.read_exact(&mut length).await {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        bail!("invalid frame length");
    }
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body).await?;
    serde_json::from_slice(&body).context("invalid client frame")
}

async fn send_response(
    sender: &mpsc::Sender<ServerFrame>,
    request_id: u64,
    result: Result<Response, ProtocolError>,
) -> Result<()> {
    let frame = match result {
        Ok(result) => ServerFrame::Response { request_id, result },
        Err(error) => ServerFrame::Error {
            request_id: Some(request_id),
            error,
        },
    };
    sender.send(frame).await.context("writer closed")
}
async fn send_control(sender: &mpsc::Sender<ServerFrame>, frame: ServerFrame) -> Result<()> {
    sender.send(frame).await.context("writer closed")
}
fn protocol_error(request_id: Option<u64>, code: ErrorCode, message: &str) -> ServerFrame {
    ServerFrame::Error {
        request_id,
        error: ProtocolError::new(code, message),
    }
}
fn invalid(message: &str) -> ProtocolError {
    ProtocolError::new(ErrorCode::InvalidArgument, message)
}

fn decode_search_cursor(cursor: Option<&str>) -> Result<usize, ProtocolError> {
    let Some(cursor) = cursor else { return Ok(0) };
    if cursor.is_empty() || cursor.len() > MAX_SEARCH_CURSOR_BYTES {
        return Err(invalid("search cursor exceeds limit"));
    }
    usize::from_str_radix(cursor, 16).map_err(|_| invalid("search cursor is invalid"))
}

fn encode_search_cursor(offset: usize) -> String {
    format!("{offset:016x}")
}
fn internal() -> ProtocolError {
    ProtocolError::new(ErrorCode::Internal, "operation failed")
}
fn not_found() -> ProtocolError {
    ProtocolError::new(ErrorCode::NotFound, "resource not found")
}

fn image_transfer_error(error: &TransferAdmissionError) -> ProtocolError {
    match error {
        TransferAdmissionError::Identity | TransferAdmissionError::Token => ProtocolError::new(
            ErrorCode::StaleImageContent,
            "image transfer identity is stale or mismatched",
        ),
        TransferAdmissionError::Capacity => ProtocolError::new(
            ErrorCode::ResourceLimit,
            "image transfer capacity is exhausted",
        ),
        TransferAdmissionError::Random | TransferAdmissionError::Descriptor => internal(),
    }
}

fn image_content_error(error: &LiveError) -> ProtocolError {
    match error {
        LiveError::ImageContentNotFound => ProtocolError::new(
            ErrorCode::ImageContentNotFound,
            "image content does not exist on the active screen",
        ),
        LiveError::StaleImageContent => ProtocolError::new(
            ErrorCode::StaleImageContent,
            "image content generation or digest is stale",
        ),
        _ => internal(),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "wire conversion keeps one revision's bounded semantic damage atomic"
)]
fn visible_row_changed(
    previous: &[splinterd::LiveRow],
    current: &[splinterd::LiveRow],
    index: usize,
) -> bool {
    previous.get(index) != current.get(index)
}

fn wire_update(
    updates: &[TerminalUpdate],
    snapshot: &LiveSnapshot,
    previous_revision: u64,
    previous_history: HistoryState,
    previous_visible_rows: &[splinterd::LiveRow],
    include_images: bool,
) -> Result<WireTerminalUpdate, ProtocolError> {
    let mut damaged = vec![false; snapshot.visible_rows.len()];
    let mut scrolls = Vec::new();
    let mut cursor = false;
    let mut title = false;
    let mut modes = false;
    let mut palette = false;
    let mut dimensions = false;
    let mut scrollback = false;
    let mut images = false;
    let mut reflow = false;
    let mut appended_rows = 0_usize;
    for damage in updates.iter().flat_map(TerminalUpdate::damage) {
        match damage {
            TerminalDamage::FullSnapshot => {
                damaged.fill(true);
                cursor = true;
                title = true;
                modes = true;
                palette = true;
                dimensions = true;
                scrollback = true;
                images = true;
            }
            TerminalDamage::Viewport => damaged.fill(true),
            TerminalDamage::Rows { start, end } => {
                for item in damaged.iter_mut().take(*end).skip(*start) {
                    *item = true;
                }
            }
            TerminalDamage::Scroll {
                direction,
                region,
                rows,
            } => {
                let start = usize::try_from(region.start()).map_err(|_| internal())?;
                let end = usize::try_from(region.end()).map_err(|_| internal())?;
                if *direction == ScrollDirection::Forward
                    && start == 0
                    && end == snapshot.dimensions.rows
                    && snapshot.active_screen == ActiveScreen::Normal
                {
                    appended_rows = appended_rows.saturating_add(*rows);
                }
                for item in damaged.iter_mut().take(end).skip(start) {
                    *item = true;
                }
                scrolls.push(TerminalScroll {
                    direction: match direction {
                        ScrollDirection::Forward => WireScrollDirection::Forward,
                        ScrollDirection::Reverse => WireScrollDirection::Reverse,
                    },
                    start_row: start,
                    end_row: end,
                    rows: *rows,
                });
            }
            TerminalDamage::Cursor { .. } => cursor = true,
            TerminalDamage::Modes => modes = true,
            TerminalDamage::Dimensions => {
                dimensions = true;
                reflow = true;
                damaged.fill(true);
            }
            TerminalDamage::Title => title = true,
            TerminalDamage::Palette { .. } => palette = true,
            TerminalDamage::Scrollback => scrollback = true,
            TerminalDamage::Images { .. } => {
                damaged.fill(true);
                images = true;
            }
        }
    }
    let position = snapshot.cursor.cursor.position();
    let rows = damaged
        .into_iter()
        .enumerate()
        .filter(|(index, changed)| {
            *changed && visible_row_changed(previous_visible_rows, &snapshot.visible_rows, *index)
        })
        .map(|(index, _)| TerminalRowPatch {
            index,
            row: wire_row(snapshot.visible_rows[index].clone()),
        })
        .collect();
    Ok(WireTerminalUpdate {
        base_revision: previous_revision,
        revision: updates.last().ok_or_else(internal)?.revision().value(),
        rows,
        scrolls,
        cursor: cursor.then_some(TerminalCursor {
            column: position.column,
            row: position.row,
            deferred_wrap: snapshot.cursor.cursor.deferred_wrap(),
        }),
        title: title.then(|| snapshot.title.clone()),
        input_modes: modes.then_some(wire_modes(snapshot.modes)),
        active_screen: modes.then_some(wire_active_screen(snapshot.active_screen)),
        palette: palette.then(|| snapshot.palette.to_vec()),
        default_colors: palette.then_some(snapshot.default_colors),
        columns: dimensions.then_some(snapshot.dimensions.columns),
        row_count: dimensions.then_some(snapshot.dimensions.rows),
        scrollback: scrollback.then(|| {
            let first = snapshot
                .scrollback_rows
                .len()
                .saturating_sub(MAX_SNAPSHOT_SCROLLBACK_ROWS);
            let rows: Vec<_> = snapshot.scrollback_rows[first..]
                .iter()
                .cloned()
                .map(wire_row)
                .collect();
            let transition =
                if snapshot.scrollback.history_generation != previous_history.generation {
                    if reflow {
                        HistoryTransition::Reflow
                    } else if snapshot.scrollback.available_rows == 0 {
                        HistoryTransition::Clear
                    } else {
                        HistoryTransition::Replace
                    }
                } else if appended_rows > 0 {
                    HistoryTransition::Append {
                        appended_rows,
                        trimmed_rows: previous_history
                            .available_rows
                            .saturating_add(appended_rows)
                            .saturating_sub(snapshot.scrollback.available_rows),
                    }
                } else {
                    HistoryTransition::Replace
                };
            TerminalScrollbackUpdate {
                transition,
                history_generation: snapshot.scrollback.history_generation,
                oldest_available_row_id: snapshot.scrollback.oldest_available_row_id,
                newest_available_row_id: snapshot.scrollback.newest_available_row_id,
                omitted_oldest_rows: snapshot
                    .scrollback
                    .available_rows
                    .saturating_sub(rows.len()),
                available_rows: snapshot.scrollback.available_rows,
                rows,
            }
        }),
        images: (include_images && images).then(|| Box::new(wire_image_plane(snapshot))),
    })
}

fn wire_snapshot(snapshot: LiveSnapshot, include_images: bool) -> TerminalSnapshot {
    let position = snapshot.cursor.cursor.position();
    let exited_code = snapshot.exited.and_then(|status| status.code);
    let images = include_images.then(|| Box::new(wire_image_plane(&snapshot)));
    let exited_signal = snapshot.exited.and_then(|status| status.signal);
    TerminalSnapshot {
        splint_id: snapshot.splint_id,
        incarnation: snapshot.incarnation.value(),
        revision: snapshot.revision.value(),
        columns: snapshot.dimensions.columns,
        rows: snapshot.dimensions.rows,
        cursor_column: position.column,
        cursor_row: position.row,
        cursor_deferred_wrap: snapshot.cursor.cursor.deferred_wrap(),
        active_screen: wire_active_screen(snapshot.active_screen),
        input_modes: wire_modes(snapshot.modes),
        palette: snapshot.palette.to_vec(),
        default_colors: snapshot.default_colors,
        title: snapshot.title,
        visible_rows: snapshot.visible_rows.into_iter().map(wire_row).collect(),
        history_generation: snapshot.scrollback.history_generation,
        oldest_available_scrollback_row_id: snapshot.scrollback.oldest_available_row_id,
        newest_available_scrollback_row_id: snapshot.scrollback.newest_available_row_id,
        scrollback_rows: snapshot.scrollback_rows.into_iter().map(wire_row).collect(),
        available_scrollback_rows: snapshot.scrollback.available_rows,
        omitted_oldest_scrollback_rows: snapshot.scrollback.omitted_oldest_rows,
        images,
        exited_code,
        exited_signal,
    }
}

fn wire_image_plane(snapshot: &LiveSnapshot) -> splinterm_protocol::TerminalImagePlane {
    use splinterm_protocol::{
        ImageAlphaMode as WireAlphaMode, ImageContentMetadata as WireContent,
        ImageErasePolicy as WireErasePolicy, ImagePixelRect, ImagePixelSize,
        ImagePlacement as WirePlacement, ImageRetention as WireRetention,
        ImageSourceFormat as WireSourceFormat,
    };

    let contents = snapshot
        .image_contents
        .iter()
        .map(|content| WireContent {
            content_id: content.id.value(),
            generation: content.generation,
            width: content.width,
            height: content.height,
            source_format: match content.source_format {
                splinterm_terminal::ImageSourceFormat::Sixel => WireSourceFormat::Sixel,
                splinterm_terminal::ImageSourceFormat::KittyRgb => WireSourceFormat::KittyRgb,
                splinterm_terminal::ImageSourceFormat::KittyRgba => WireSourceFormat::KittyRgba,
                splinterm_terminal::ImageSourceFormat::KittyPng => WireSourceFormat::KittyPng,
                splinterm_terminal::ImageSourceFormat::Iterm2 => WireSourceFormat::Iterm2,
            },
            alpha_mode: match content.alpha_mode {
                splinterm_terminal::ImageAlphaMode::Opaque => WireAlphaMode::Opaque,
                splinterm_terminal::ImageAlphaMode::Premultiplied => WireAlphaMode::Premultiplied,
            },
            digest: content.digest,
            byte_length: content.byte_charge,
            retention: match content.retention {
                splinterm_terminal::ImageRetention::WhilePlaced => WireRetention::WhilePlaced,
                splinterm_terminal::ImageRetention::ExplicitDelete => WireRetention::ExplicitDelete,
            },
        })
        .collect();
    let placements = snapshot
        .image_placements
        .iter()
        .map(|placement| WirePlacement {
            placement_id: placement.id.value(),
            content_id: placement.content_id.value(),
            row_id: placement.row_id,
            column: placement.column,
            source: ImagePixelRect {
                x: placement.source.x,
                y: placement.source.y,
                width: placement.source.width,
                height: placement.source.height,
            },
            destination_columns: placement.destination.columns,
            destination_rows: placement.destination.rows,
            source_cell_size: placement.source_cell_size.map(|size| ImagePixelSize {
                width: size.width,
                height: size.height,
            }),
            x_offset: placement.x_offset,
            y_offset: placement.y_offset,
            z_index: placement.z_index,
            application_image_id: placement.application_image_id,
            application_placement_id: placement.application_placement_id,
            creation_order: placement.creation_order,
            erase_policy: match placement.erase_policy {
                splinterm_terminal::ImageErasePolicy::TextOverwrite => {
                    WireErasePolicy::TextOverwrite
                }
                splinterm_terminal::ImageErasePolicy::ExplicitDelete => {
                    WireErasePolicy::ExplicitDelete
                }
            },
        })
        .collect();
    splinterm_protocol::TerminalImagePlane {
        screen: wire_active_screen(snapshot.active_screen),
        contents,
        placements,
    }
}

fn wire_active_screen(screen: ActiveScreen) -> WireActiveScreen {
    match screen {
        ActiveScreen::Normal => WireActiveScreen::Normal,
        ActiveScreen::Alternate => WireActiveScreen::Alternate,
    }
}

fn wire_modes(modes: splinterm_terminal::TerminalModes) -> TerminalInputModes {
    TerminalInputModes {
        application_cursor: modes.application_cursor,
        application_keypad: modes.application_keypad,
        focus_reporting: modes.focus_reporting,
        bracketed_paste: modes.bracketed_paste,
        cursor_visible: modes.cursor_visible,
        cursor_blink: modes.cursor_blink,
        mouse_tracking: match modes.mouse_tracking {
            splinterm_terminal::MouseTracking::None => WireMouseTracking::None,
            splinterm_terminal::MouseTracking::Normal => WireMouseTracking::Normal,
            splinterm_terminal::MouseTracking::Button => WireMouseTracking::Button,
            splinterm_terminal::MouseTracking::Any => WireMouseTracking::Any,
        },
        sgr_mouse: modes.sgr_mouse,
    }
}

fn wire_row(row: splinterd::LiveRow) -> TerminalRow {
    TerminalRow {
        row_id: row.row_id,
        linebreak: row.linebreak,
        cells: row
            .cells
            .into_iter()
            .map(|cell| TerminalCell {
                content: cell.content,
                spacer_remaining: cell.spacer_remaining,
                attributes: CellAttributes {
                    bold: cell.attributes.bold,
                    dim: cell.attributes.dim,
                    italic: cell.attributes.italic,
                    underline: match cell.attributes.underline {
                        splinterm_terminal::UnderlineStyle::None => WireUnderlineStyle::None,
                        splinterm_terminal::UnderlineStyle::Single => WireUnderlineStyle::Single,
                        splinterm_terminal::UnderlineStyle::Double => WireUnderlineStyle::Double,
                        splinterm_terminal::UnderlineStyle::Curly => WireUnderlineStyle::Curly,
                        splinterm_terminal::UnderlineStyle::Dotted => WireUnderlineStyle::Dotted,
                        splinterm_terminal::UnderlineStyle::Dashed => WireUnderlineStyle::Dashed,
                    },
                    underline_color_source: wire_color_source(
                        cell.attributes.underline_color.source(),
                    ),
                    underline_color: cell.attributes.underline_color.value(),
                    strikethrough: cell.attributes.strikethrough,
                    blink: cell.attributes.blink,
                    conceal: cell.attributes.conceal,
                    reverse: cell.attributes.reverse,
                    foreground_source: wire_color_source(cell.attributes.foreground.source()),
                    foreground: cell.attributes.foreground.value(),
                    background_source: wire_color_source(cell.attributes.background.source()),
                    background: cell.attributes.background.value(),
                },
            })
            .collect(),
    }
}
fn wire_color_source(source: TerminalColorSource) -> ColorSource {
    match source {
        TerminalColorSource::Default => ColorSource::Default,
        TerminalColorSource::Base16 => ColorSource::Base16,
        TerminalColorSource::Base256 => ColorSource::Base256,
        TerminalColorSource::Rgb => ColorSource::Rgb,
    }
}

async fn verify_peer(stream: &UnixStream) -> Result<(PeerIdentity, Option<consent::PeerMonitor>)> {
    let mut identity = PeerIdentity::from_stream(stream)?;
    if identity.uid != rustix::process::geteuid().as_raw() {
        bail!("peer uid mismatch");
    }
    let monitor = match consent::PeerMonitor::initialize(stream, identity.pid).await {
        Ok((monitor, executable)) => {
            identity.install_persistent_executable(executable);
            Some(monitor)
        }
        Err(error) => {
            warn!(%error, "persistent peer identity unavailable; policy authorization disabled");
            None
        }
    };
    Ok((identity, monitor))
}

async fn prepare_socket_parent(path: &Path) -> Result<()> {
    let parent = path.parent().context("socket path has no parent")?;
    fs::create_dir_all(parent).await?;
    let metadata = fs::symlink_metadata(parent).await?;
    if !metadata.is_dir() || metadata.uid() != rustix::process::geteuid().as_raw() {
        bail!("unsafe socket directory owner or type");
    }
    fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).await?;
    Ok(())
}
async fn verify_socket(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).await?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o777 != 0o600
    {
        bail!("unsafe socket metadata");
    }
    Ok(())
}
async fn remove_stale_socket(path: &Path) -> Result<()> {
    match UnixStream::connect(path).await {
        Ok(_) => bail!("splinterd is already running at {}", path.display()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) if error.kind() == ErrorKind::ConnectionRefused => {
            let metadata = fs::symlink_metadata(path).await?;
            if !metadata.file_type().is_socket()
                || metadata.uid() != rustix::process::geteuid().as_raw()
            {
                bail!("refusing to remove unsafe stale endpoint");
            }
            fs::remove_file(path).await?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}
fn socket_path() -> Result<PathBuf> {
    if let Some(path) = env::var_os("SPLINTERM_SOCKET") {
        return Ok(path.into());
    }
    let runtime = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .context("XDG_RUNTIME_DIR is unset; set SPLINTERM_SOCKET explicitly")?;
    Ok(runtime.join("splinterm/splinterd.sock"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[tokio::test]
    async fn binary_image_content_channel_is_raw_windowed_and_acknowledged() {
        use splinterm_terminal::{
            ImageAlphaMode, ImagePlane, ImageRetention, ImageSourceFormat, NewImageContent,
        };

        let mut plane = ImagePlane::default();
        let content_id = plane
            .insert_content(
                ActiveScreen::Normal,
                NewImageContent {
                    width: 1,
                    height: 1,
                    source_format: ImageSourceFormat::Sixel,
                    alpha_mode: ImageAlphaMode::Opaque,
                    pixels: &[1, 2, 3, 255],
                    retention: ImageRetention::ExplicitDelete,
                },
            )
            .unwrap();
        let content = plane
            .content(ActiveScreen::Normal, content_id)
            .unwrap()
            .clone();
        let metadata = content.metadata();
        let claimed = splinterd::image_transport::ClaimedTransfer {
            transfer_id: 1,
            request: splinterm_protocol::ImageContentRequest {
                splint_id: SplintId::new(),
                incarnation: 2,
                content_id: content_id.value(),
                generation: metadata.generation,
                digest: metadata.digest,
                accepted_transfers: vec![splinterm_protocol::ImageTransferMode::BinaryChunks],
            },
            content,
            mode: ImageTransferMode::BinaryChunks,
        };
        let (mut server, mut client) = UnixStream::pair().unwrap();
        let sender =
            tokio::spawn(async move { send_image_content_chunks(&mut server, &claimed).await });
        let mut header = [0_u8; IMAGE_CONTENT_HEADER_BYTES];
        client.read_exact(&mut header).await.unwrap();
        assert_eq!(&header[0..5], b"SPIM\x01");
        assert_eq!(u64::from_be_bytes(header[5..13].try_into().unwrap()), 4);
        assert_eq!(&header[13..45], &metadata.digest);
        let mut chunk_header = [0_u8; 12];
        client.read_exact(&mut chunk_header).await.unwrap();
        assert_eq!(
            u64::from_be_bytes(chunk_header[0..8].try_into().unwrap()),
            0
        );
        assert_eq!(
            u32::from_be_bytes(chunk_header[8..12].try_into().unwrap()),
            4
        );
        let mut pixels = [0_u8; 4];
        client.read_exact(&mut pixels).await.unwrap();
        assert_eq!(pixels, [1, 2, 3, 255]);
        let mut acknowledgement = [0_u8; 9];
        acknowledgement[0] = 1;
        acknowledgement[1..9].copy_from_slice(&4_u64.to_be_bytes());
        client.write_all(&acknowledgement).await.unwrap();
        sender.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn sealed_image_content_channel_passes_one_exact_immutable_descriptor() {
        use std::io::{IoSliceMut, Read as _, Seek as _, SeekFrom};

        use rustix::{
            fs::{SealFlags, fcntl_get_seals, fstat},
            net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags, recvmsg},
        };
        use splinterm_terminal::{
            ImageAlphaMode, ImagePlane, ImageRetention, ImageSourceFormat, NewImageContent,
        };

        let mut plane = ImagePlane::default();
        let content_id = plane
            .insert_content(
                ActiveScreen::Normal,
                NewImageContent {
                    width: 1,
                    height: 1,
                    source_format: ImageSourceFormat::Sixel,
                    alpha_mode: ImageAlphaMode::Opaque,
                    pixels: &[1, 2, 3, 255],
                    retention: ImageRetention::ExplicitDelete,
                },
            )
            .unwrap();
        let content = plane
            .content(ActiveScreen::Normal, content_id)
            .unwrap()
            .clone();
        let metadata = content.metadata();
        let claimed = splinterd::image_transport::ClaimedTransfer {
            transfer_id: 1,
            request: splinterm_protocol::ImageContentRequest {
                splint_id: SplintId::new(),
                incarnation: 2,
                content_id: content_id.value(),
                generation: metadata.generation,
                digest: metadata.digest,
                accepted_transfers: vec![ImageTransferMode::SealedMemfd],
            },
            content,
            mode: ImageTransferMode::SealedMemfd,
        };
        let (mut server, mut client) = UnixStream::pair().unwrap();
        let sender =
            tokio::spawn(async move { send_image_content_memfd(&mut server, &claimed).await });
        client.readable().await.unwrap();
        let mut marker = [0_u8; 1];
        let mut iov = [IoSliceMut::new(&mut marker)];
        let mut space = [std::mem::MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = RecvAncillaryBuffer::new(&mut space);
        let message = recvmsg(
            client.as_fd(),
            &mut iov,
            &mut ancillary,
            RecvFlags::CMSG_CLOEXEC,
        )
        .unwrap();
        assert_eq!(message.bytes, 1);
        assert_eq!(marker, [b'F']);
        assert!(!message.flags.contains(ReturnFlags::CTRUNC));
        let mut descriptor = None;
        for item in ancillary.drain() {
            if let RecvAncillaryMessage::ScmRights(mut descriptors) = item {
                assert!(descriptor.is_none());
                descriptor = descriptors.next();
                assert!(descriptors.next().is_none());
            }
        }
        let descriptor = descriptor.unwrap();
        assert_eq!(
            usize::try_from(fstat(&descriptor).unwrap().st_size).unwrap(),
            4
        );
        assert!(
            fcntl_get_seals(&descriptor)
                .unwrap()
                .contains(SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL)
        );
        let mut file = std::fs::File::from(descriptor);
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut pixels = [0_u8; 4];
        file.read_exact(&mut pixels).unwrap();
        assert_eq!(pixels, [1, 2, 3, 255]);
        let mut header = [0_u8; IMAGE_MEMFD_HEADER_BYTES];
        client.read_exact(&mut header).await.unwrap();
        assert_eq!(&header[0..5], b"SPIF\x01");
        assert_eq!(u64::from_be_bytes(header[5..13].try_into().unwrap()), 4);
        assert_eq!(&header[13..45], &metadata.digest);
        client.write_all(&[1]).await.unwrap();
        sender.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn dropped_content_sender_releases_active_transfer_capacity() {
        use splinterm_terminal::{
            ImageAlphaMode, ImagePlane, ImageRetention, ImageSourceFormat, NewImageContent,
        };

        let state = test_state(false);
        let mut plane = ImagePlane::default();
        let content_id = plane
            .insert_content(
                ActiveScreen::Normal,
                NewImageContent {
                    width: 1,
                    height: 1,
                    source_format: ImageSourceFormat::Sixel,
                    alpha_mode: ImageAlphaMode::Opaque,
                    pixels: &[1, 2, 3, 255],
                    retention: ImageRetention::ExplicitDelete,
                },
            )
            .unwrap();
        let content = plane
            .content(ActiveScreen::Normal, content_id)
            .unwrap()
            .clone();
        let metadata = content.metadata();
        let peer = splinterd::image_transport::TransferPeer {
            uid: 1000,
            pid: 2,
            executable_device: 3,
            executable_inode: 4,
            executable_sha256: "5".repeat(64),
        };
        let request = splinterm_protocol::ImageContentRequest {
            splint_id: SplintId::new(),
            incarnation: 2,
            content_id: content_id.value(),
            generation: metadata.generation,
            digest: metadata.digest,
            accepted_transfers: vec![ImageTransferMode::BinaryChunks],
        };
        let claimed = {
            let mut admission = state.image_transfers.lock().await;
            let grant = admission
                .mint(peer.clone(), &request, content.clone(), Instant::now())
                .unwrap();
            admission.claim(grant.token, &peer, Instant::now()).unwrap()
        };
        let guard = ActiveImageTransferGuard::new(Arc::clone(&state), claimed.transfer_id);
        assert_eq!(
            state
                .image_transfers
                .lock()
                .await
                .metrics()
                .active_transfers,
            1
        );
        drop(guard);
        for _ in 0..8 {
            if state
                .image_transfers
                .lock()
                .await
                .metrics()
                .active_transfers
                == 0
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            state
                .image_transfers
                .lock()
                .await
                .metrics()
                .active_transfers,
            0
        );

        let claimed = {
            let mut admission = state.image_transfers.lock().await;
            let grant = admission
                .mint(peer.clone(), &request, content, Instant::now())
                .unwrap();
            admission.claim(grant.token, &peer, Instant::now()).unwrap()
        };
        let admission_lock = state.image_transfers.lock().await;
        let guard = ActiveImageTransferGuard::new(Arc::clone(&state), claimed.transfer_id);
        let finishing = tokio::spawn(guard.finish());
        tokio::task::yield_now().await;
        finishing.abort();
        assert!(finishing.await.unwrap_err().is_cancelled());
        drop(admission_lock);
        for _ in 0..8 {
            if state
                .image_transfers
                .lock()
                .await
                .metrics()
                .active_transfers
                == 0
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            state
                .image_transfers
                .lock()
                .await
                .metrics()
                .active_transfers,
            0
        );
    }

    #[test]
    fn image_metadata_and_retrieval_require_the_matching_trusted_ui() {
        assert!(include_image_metadata(true, true));
        assert!(!include_image_metadata(true, false));
        assert!(!include_image_metadata(false, true));
        assert!(trusted_ui_request(&Request::RequestImageContent {
            request: splinterm_protocol::ImageContentRequest {
                splint_id: SplintId::new(),
                incarnation: 1,
                content_id: 1,
                generation: 1,
                digest: [1; 32],
                accepted_transfers: vec![splinterm_protocol::ImageTransferMode::BinaryChunks],
            },
        }));
    }

    fn test_state(development_terminal_access: bool) -> Arc<DaemonState> {
        let (revocations, _) = broadcast::channel(32);
        let (control_events, _) = broadcast::channel(CONTROL_EVENT_QUEUE);
        Arc::new(DaemonState {
            lair: RwLock::new(Lair::new()),
            runtimes: Mutex::new(RuntimeRegistry::default()),
            topology: Mutex::new(TopologyHub::default()),
            topology_transactions: Semaphore::new(1),
            exit_observers: TaskTracker::new(),
            metadata: None,
            policy: Mutex::new(policy::PolicyStore::default()),
            audit: Mutex::new(audit::AuditStore::default()),
            daemon_audit_peer: splinterm_protocol::AuditPeer {
                uid: rustix::process::geteuid().as_raw(),
                executable_path: "/test/splinterd".into(),
                executable_sha256: "0".repeat(64),
                device: Some(1),
                inode: Some(1),
            },
            policy_reloads: broadcast::channel(1).0,
            controller: Mutex::new(ControllerState::default()),
            control_events,
            connection_revocations: broadcast::channel(CONNECTION_LIMIT).0,
            grants: Mutex::new(GrantStore::default()),
            revocations,
            image_transfers: Mutex::new(TransferAdmission::default()),
            image_transfer_expiry_changed: Notify::new(),
            shared_image_budget: SharedImageBudget::new(MAX_IMAGE_BYTES_PER_DAEMON),
            shared_kitty_upload_budget: SharedKittyUploadBudget::new(
                DEFAULT_KITTY_UPLOAD_BYTES_PER_DAEMON,
            ),
            pty_backend: LinuxPtyBackend::new("/missing/helper"),
            owner_home: Some(PathBuf::from("/home/test")),
            development_terminal_access,
        })
    }

    fn temp_dir() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("splinterd-test-{}-{nonce}", std::process::id()))
    }
    #[test]
    fn automation_role_never_receives_trusted_ui_bypass() {
        assert!(!trusted_ui_bypass(false, true, &Request::ListDojos));
        assert!(trusted_ui_bypass(true, true, &Request::ListDojos));
        assert!(!trusted_ui_bypass(true, false, &Request::ListDojos));
        assert!(!trusted_ui_bypass(
            true,
            true,
            &Request::AuditInspect {
                after_audit_id: None,
                max_records: 1,
            }
        ));
        let peer = PeerIdentity::for_test();
        assert!(!trusted_first_party_ui(false, &peer, &[AccessScope::Input]));
    }

    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "one test covers every daemon-owned automation launch default"
    )]
    async fn scoped_mutation_preflight_and_daemon_launch_defaults_are_exact() {
        let state = test_state(false);
        let dojo = state
            .lair
            .write()
            .await
            .create_dojo("test", PathBuf::from("/target"))
            .unwrap()
            .clone();
        let dojo_id = dojo.id;
        let window_id = dojo.windows[0].id;
        let splint_id = dojo.windows[0].default_focus;
        let snapshot = TopologySnapshot {
            revision: state.lair.read().await.revision(),
            lair: state.lair.read().await.clone(),
            runtimes: vec![SplintRuntimeSummary {
                splint_id,
                live_incarnation: None,
                last_incarnation: Some(2),
                restorable: true,
                lifecycle: SplintLifecycle::Exited,
                exit_status: None,
            }],
        };
        let rename = prepare_mutation(
            &snapshot,
            splinterm_protocol::MutationPreflight::RenameSplint { splint_id },
        )
        .unwrap();
        assert_eq!(rename.dojo_id, Some(dojo_id));
        assert_eq!(rename.window_id, Some(window_id));
        assert_eq!(rename.incarnation, Some(2));
        let restore = prepare_mutation(
            &snapshot,
            splinterm_protocol::MutationPreflight::RestoreWindow { window_id },
        )
        .unwrap();
        assert_eq!(restore.targets.len(), 1);
        assert_eq!(restore.targets[0].splint_id, splint_id);

        for request in [
            Request::CreateDojoAutomation {
                expected_topology_revision: snapshot.revision,
                name: "new".to_owned(),
                launch: splinterm_protocol::AutomationLaunch {
                    cwd: None,
                    argv: Vec::new(),
                },
            },
            Request::SplitSplintAutomation {
                expected_topology_revision: snapshot.revision,
                target_splint_id: splint_id,
                axis: splinterm_core::Axis::Horizontal,
                side: splinterm_core::SplitSide::Second,
                ratio: splinterm_core::SplitRatio::new(500).unwrap(),
                launch: splinterm_protocol::AutomationLaunch {
                    cwd: None,
                    argv: Vec::new(),
                },
            },
            Request::RelaunchSplintAutomation {
                expected_topology_revision: snapshot.revision,
                splint_id,
                launch: splinterm_protocol::AutomationLaunch {
                    cwd: None,
                    argv: Vec::new(),
                },
            },
            Request::NewWindowAutomation {
                expected_topology_revision: snapshot.revision,
                dojo_id,
                title: "window".to_owned(),
                launch: splinterm_protocol::AutomationLaunch {
                    cwd: None,
                    argv: Vec::new(),
                },
            },
        ] {
            let resolved = resolve_automation_mutation(request, &state).await.unwrap();
            let is_create = matches!(resolved, Request::CreateDojo { .. });
            let (Request::CreateDojo { launch, .. }
            | Request::SplitSplint { launch, .. }
            | Request::RelaunchSplint { launch, .. }
            | Request::NewWindow { launch, .. }) = resolved
            else {
                panic!("automation request was not resolved")
            };
            assert_eq!(
                launch.cwd,
                if is_create {
                    PathBuf::from("/home/test")
                } else {
                    PathBuf::from("/target")
                }
            );
            assert!(launch.command.is_empty());
            assert!(launch.shell.is_none());
            assert!(!launch.login_shell);
        }

        let explicit = resolve_automation_mutation(
            Request::RelaunchSplintAutomation {
                expected_topology_revision: snapshot.revision,
                splint_id,
                launch: splinterm_protocol::AutomationLaunch {
                    cwd: Some(PathBuf::from("/override")),
                    argv: vec!["sh".to_owned(), String::new()],
                },
            },
            &state,
        )
        .await
        .unwrap();
        let Request::RelaunchSplint { launch, .. } = explicit else {
            panic!("explicit relaunch was not resolved")
        };
        assert_eq!(launch.cwd, PathBuf::from("/override"));
        assert_eq!(launch.command, ["sh", ""]);
    }

    #[tokio::test]
    async fn exact_policy_authorizes_only_its_declared_request_scope() {
        let state = test_state(false);
        let mut peer = PeerIdentity::for_test();
        let executable = executable_identity::ExecutableIdentity::from_pid(std::process::id())
            .expect("snapshot test executable");
        peer.install_persistent_executable(executable.clone());

        let denied = authorize_request(&Request::ListDojos, &state, &peer, 0, false)
            .await
            .unwrap_err();
        assert_eq!(denied.code, ErrorCode::Unauthorized);

        let directory = temp_dir();
        fs::create_dir(&directory).await.unwrap();
        let policy_path = directory.join("policy.json");
        let policy = serde_json::json!({
            "schema": "splinterm.policy.v1",
            "rules": [{
                "id": "topology-reader",
                "executable": {
                    "path": executable.path,
                    "sha256": executable.sha256,
                },
                "scopes": ["topology_metadata_read"],
                "resources": [{"kind": "lair"}],
                "limits": {"deadline_ms": 1000, "max_returned_bytes": 1},
            }],
        });
        fs::write(&policy_path, serde_json::to_vec(&policy).unwrap())
            .await
            .unwrap();
        fs::set_permissions(&policy_path, std::fs::Permissions::from_mode(0o600))
            .await
            .unwrap();
        let topology_snapshot = state.lair.read().await.clone();
        state
            .policy
            .lock()
            .await
            .reload(Some(policy_path.as_path()), &topology_snapshot);

        let allowed = authorize_request(&Request::ListDojos, &state, &peer, 0, false)
            .await
            .unwrap();
        assert!(allowed.policy_authorized());
        let oversized = handle_request(Request::ListDojos, &state, &peer, 1, 0, false)
            .await
            .unwrap_err();
        assert_eq!(oversized.code, ErrorCode::ResourceLimit);
        let wrong_scope = authorize_request(&Request::SubscribeTopology, &state, &peer, 0, false)
            .await
            .unwrap_err();
        assert_eq!(wrong_scope.code, ErrorCode::Unauthorized);
        fs::remove_dir_all(directory).await.unwrap();
    }

    #[test]
    fn requested_controller_modes_expand_to_exact_operation_scopes() {
        let splint_id = SplintId::new();
        assert_eq!(
            requested_operation_scopes(&Request::AcquireControl {
                splint_id,
                incarnation: 1,
                modes: vec![splinterm_protocol::ControlMode::Input],
            }),
            Some(vec![
                splinterm_protocol::AutomationScope::ControllerAcquire,
                splinterm_protocol::AutomationScope::Input,
            ])
        );
        assert_eq!(
            requested_operation_scopes(&Request::RequestControlTransfer {
                splint_id,
                incarnation: 1,
                modes: vec![
                    splinterm_protocol::ControlMode::Input,
                    splinterm_protocol::ControlMode::Resize,
                ],
            }),
            Some(vec![
                splinterm_protocol::AutomationScope::ControllerTransfer,
                splinterm_protocol::AutomationScope::Input,
                splinterm_protocol::AutomationScope::Resize,
            ])
        );
        assert!(
            requested_operation_scopes(&Request::AcquireControl {
                splint_id,
                incarnation: 1,
                modes: Vec::new(),
            })
            .is_none()
        );
    }

    #[tokio::test]
    async fn socket_directory_and_endpoint_are_private() {
        let dir = temp_dir();
        let path = dir.join("endpoint");
        prepare_socket_parent(&path).await.unwrap();
        let listener = UnixListener::bind(&path).unwrap();
        fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .await
            .unwrap();
        verify_socket(&path).await.unwrap();
        assert_eq!(fs::metadata(&dir).await.unwrap().mode() & 0o777, 0o700);
        drop(listener);
        fs::remove_dir_all(dir).await.unwrap();
    }
    #[tokio::test]
    async fn refuses_to_replace_a_regular_file() {
        let dir = temp_dir();
        fs::create_dir(&dir).await.unwrap();
        let path = dir.join("endpoint");
        fs::write(&path, "keep").await.unwrap();
        assert!(remove_stale_socket(&path).await.is_err());
        fs::remove_dir_all(dir).await.unwrap();
    }
    #[tokio::test]
    async fn oversized_frame_is_rejected_before_body_allocation() {
        let (mut client, server) = UnixStream::pair().unwrap();
        let (mut reader, _) = server.into_split();
        let oversized = u32::try_from(MAX_FRAME_BYTES).unwrap() + 1;
        client.write_all(&oversized.to_be_bytes()).await.unwrap();
        assert!(read_optional_frame(&mut reader).await.is_err());
    }

    #[tokio::test]
    async fn request_before_hello_is_rejected_without_side_effects() {
        let state = test_state(false);
        let (mut client, server) = UnixStream::pair().unwrap();
        let task = tokio::spawn(serve_client(server, state));
        client
            .write_all(
                &encode_frame(&ClientFrame::Request {
                    request_id: 1,
                    request: Request::Ping,
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let mut length = [0_u8; 4];
        client.read_exact(&mut length).await.unwrap();
        let mut body = vec![0_u8; u32::from_be_bytes(length) as usize];
        client.read_exact(&mut body).await.unwrap();
        let frame: ServerFrame = serde_json::from_slice(&body).unwrap();
        assert!(matches!(
            frame,
            ServerFrame::Error {
                error: ProtocolError {
                    code: ErrorCode::HandshakeRequired,
                    ..
                },
                ..
            }
        ));
        drop(client);
        let _ = task.await;
    }

    #[tokio::test]
    async fn terminal_operations_require_consent_by_default() {
        let state = test_state(false);
        let peer = PeerIdentity::for_test();
        let error = handle_request(
            Request::Attach {
                splint_id: SplintId::new(),
                incarnation: Some(1),
                scrollback_rows: 0,
            },
            &state,
            &peer,
            1,
            0,
            false,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::Unauthorized);
    }

    #[tokio::test]
    async fn revoke_access_returns_exact_removed_grant() {
        let state = test_state(true);
        let peer = PeerIdentity::for_test();
        let dojo = state
            .lair
            .write()
            .await
            .create_dojo("test", PathBuf::from("/tmp"))
            .unwrap()
            .clone();
        let dojo_id = dojo.id;
        let window_id = dojo.windows[0].id;
        let LayoutNode::Leaf(splint) = &dojo.windows[0].root else {
            unreachable!()
        };
        let splint_id = splint.id;
        let grant =
            state
                .grants
                .lock()
                .await
                .grant(&peer, splint_id, 2, vec![AccessScope::Observe]);

        let response = handle_request(
            Request::RevokeAccess {
                grant_id: grant.grant.grant_id,
            },
            &state,
            &peer,
            1,
            0,
            false,
        )
        .await
        .unwrap();

        assert_eq!(
            response.response,
            Response::AccessRevoked {
                dojo_id,
                window_id,
                authorization_revision: 2,
                grant: grant.grant.clone(),
            }
        );
        assert!(state.grants.lock().await.status(splint_id, 2).is_empty());
    }

    #[tokio::test]
    async fn revoke_metadata_commit_is_atomic_with_a_queued_close() {
        let state = test_state(true);
        let peer = PeerIdentity::for_test();
        let dojo = state
            .lair
            .write()
            .await
            .create_dojo("race", PathBuf::from("/tmp"))
            .unwrap()
            .clone();
        let dojo_id = dojo.id;
        let window_id = dojo.windows[0].id;
        let LayoutNode::Leaf(splint) = &dojo.windows[0].root else {
            unreachable!()
        };
        let splint_id = splint.id;
        assert!(
            state
                .lair
                .write()
                .await
                .set_splint_state(splint_id, SplintState::Exited(0))
        );
        let grant = state
            .grants
            .lock()
            .await
            .grant(&peer, splint_id, 2, vec![AccessScope::Observe])
            .grant;
        let expected_topology_revision = state.lair.read().await.revision();

        let barrier = state.topology_transactions.acquire().await.unwrap();
        let revoke_state = Arc::clone(&state);
        let revoke_peer = peer.clone();
        let revoke = tokio::spawn(async move {
            handle_request(
                Request::RevokeAccess {
                    grant_id: grant.grant_id,
                },
                &revoke_state,
                &revoke_peer,
                1,
                0,
                false,
            )
            .await
        });
        tokio::task::yield_now().await;
        let close_state = Arc::clone(&state);
        let close_peer = peer.clone();
        let close = tokio::spawn(async move {
            handle_request(
                Request::CloseSplint {
                    expected_topology_revision,
                    splint_id,
                },
                &close_state,
                &close_peer,
                2,
                0,
                false,
            )
            .await
        });
        drop(barrier);

        let revoked = revoke.await.unwrap().unwrap().response;
        assert_eq!(
            revoked,
            Response::AccessRevoked {
                dojo_id,
                window_id,
                authorization_revision: 2,
                grant,
            }
        );
        assert!(matches!(
            close.await.unwrap().unwrap().response,
            Response::TopologyCommitted { .. }
        ));
        assert!(state.grants.lock().await.status(splint_id, 2).is_empty());
    }

    #[tokio::test]
    async fn restore_rejects_stale_topology_before_resource_lookup() {
        let state = test_state(true);
        let peer = PeerIdentity::for_test();
        let error = handle_request(
            Request::RestoreDojo {
                expected_topology_revision: TopologyRevision::new(1),
                dojo_id: DojoId::new(),
            },
            &state,
            &peer,
            1,
            0,
            false,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::StaleTopology);
        assert_eq!(
            error.current_topology_revision,
            Some(TopologyRevision::new(0))
        );
    }

    #[tokio::test]
    async fn resize_limits_are_checked_before_runtime_access() {
        let state = test_state(true);
        let peer = PeerIdentity::for_test();
        let error = handle_request(
            Request::Resize {
                controller_id: 1,
                splint_id: SplintId::new(),
                incarnation: 1,
                columns: MAX_COLUMNS + 1,
                rows: 24,
                pixel_width: 0,
                pixel_height: 0,
            },
            &state,
            &peer,
            1,
            0,
            false,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[tokio::test]
    async fn failed_durable_write_does_not_install_topology_edit() {
        let mut state = test_state(false);
        let base = std::env::temp_dir().join(format!(
            "splinterd-durable-failure-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let target = base.join("target");
        std::fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, base.join("splinterm")).unwrap();
        Arc::get_mut(&mut state).unwrap().metadata = Some(MetadataStore::from_base(&base));
        let dojo_id = state
            .lair
            .write()
            .await
            .create_dojo("main", PathBuf::from("/tmp"))
            .unwrap()
            .id;
        let before = state.lair.read().await.clone();
        assert!(
            durable_lair_candidate(&state, |lair| {
                lair.rename_dojo_at(lair.revision(), dojo_id, "renamed")
            })
            .await
            .is_err()
        );
        assert_eq!(*state.lair.read().await, before);
        std::fs::remove_file(base.join("splinterm")).unwrap();
        std::fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn topology_subscription_overflow_requires_resync() {
        let mut hub = TopologyHub::default();
        let subscription = hub.subscribe(1);
        for _ in 1..=TOPOLOGY_QUEUE + 2 {
            hub.publish(&TopologyChange {
                revision: splinterm_core::TopologyRevision::default(),
                kind: TopologyChangeKind::SplintRenamed,
                snapshot: TopologySnapshot {
                    revision: splinterm_core::TopologyRevision::default(),
                    lair: Lair::new(),
                    runtimes: Vec::new(),
                },
            });
        }
        assert_eq!(
            *subscription.resync.borrow(),
            Some(splinterm_core::TopologyRevision::default())
        );
    }

    #[test]
    fn semantic_row_diff_suppresses_identical_redraws() {
        let row = splinterd::LiveRow {
            row_id: Some(7),
            linebreak: false,
            cells: Vec::new(),
        };
        let mut changed = row.clone();
        changed.linebreak = true;
        assert!(!visible_row_changed(
            std::slice::from_ref(&row),
            std::slice::from_ref(&row),
            0,
        ));
        assert!(visible_row_changed(&[row], &[changed.clone()], 0));
        assert!(visible_row_changed(&[], &[changed], 0));
        assert!(!visible_row_changed(&[], &[], 0));
    }

    #[test]
    fn queued_update_uses_delta_only_for_its_exact_snapshot_revision() {
        let terminal = splinterm_terminal::Terminal::new(
            80,
            24,
            splinterm_terminal::TerminalConfig::default(),
        );
        let empty = terminal
            .updates_since(terminal.revision())
            .unwrap()
            .updates()
            .cloned()
            .collect::<Vec<_>>();
        assert!(!revisions_match(&empty, terminal.revision().value()));
        assert!(revision_advances(41, 52));
        assert!(!revision_advances(41, 41));
        assert!(!revision_advances(41, 40));
    }

    #[test]
    fn controller_state_is_per_splint_authorized_and_releasable() {
        let first_id = SplintId::new();
        let second_id = SplintId::new();
        let mut controllers = ControllerState::default();
        let first = controllers
            .acquire(101, first_id, 7, Some(4))
            .expect("first controller");
        let second = controllers
            .acquire(102, second_id, 3, Some(5))
            .expect("different Splint controller");
        assert_eq!(
            controllers
                .acquire(103, first_id, 7, Some(4))
                .unwrap_err()
                .code,
            ErrorCode::ControllerUnavailable
        );
        assert!(controllers.authorize(101, first.id, first_id, 7).is_ok());
        assert!(controllers.authorize(102, second.id, second_id, 3).is_ok());
        assert_eq!(
            controllers
                .authorize(101, first.id, second_id, 3)
                .unwrap_err()
                .code,
            ErrorCode::Unauthorized
        );
        controllers.release_identity(first_id, 8);
        assert!(controllers.authorize(101, first.id, first_id, 7).is_ok());
        controllers.release_identity(first_id, 7);
        assert!(controllers.authorize(101, first.id, first_id, 7).is_err());
        assert!(controllers.authorize(102, second.id, second_id, 3).is_ok());
        controllers.release_grant(5);
        assert!(controllers.authorize(102, second.id, second_id, 3).is_err());
        assert!(controllers.acquire(101, first_id, 8, None).is_ok());
    }

    #[test]
    fn policy_reload_reset_revokes_all_connection_owned_control() {
        let mut controllers = ControllerState::default();
        let splint_id = SplintId::new();
        let lease = controllers.acquire(10, splint_id, 4, None).unwrap();
        let transfer = controllers.request_transfer(20, splint_id, 4).unwrap();

        let (leases, transfers) = controllers.reset_connections();

        assert_eq!(leases, vec![lease]);
        assert_eq!(transfers, vec![transfer]);
        assert!(controllers.by_id.is_empty());
        assert!(controllers.by_splint.is_empty());
        assert!(controllers.by_connection.is_empty());
        assert!(controllers.transfers.is_empty());
        assert!(controllers.transfer_by_splint.is_empty());
    }

    #[tokio::test]
    async fn grant_revocation_and_transfer_timeout_signal_only_affected_connections() {
        let state = test_state(false);
        let mut revocations = state.connection_revocations.subscribe();
        let splint_id = SplintId::new();
        let lease = state
            .controller
            .lock()
            .await
            .acquire(42, splint_id, 3, Some(9))
            .unwrap();

        revoke_grant_controllers(&state, 9).await;
        assert_eq!(revocations.recv().await.unwrap(), 42);
        assert!(
            state
                .controller
                .lock()
                .await
                .authorize(42, lease.id, splint_id, 3)
                .is_err()
        );

        let owner = state
            .controller
            .lock()
            .await
            .acquire(10, splint_id, 3, None)
            .unwrap();
        let transfer = state
            .controller
            .lock()
            .await
            .request_transfer(20, splint_id, 3)
            .unwrap();
        let expired = state
            .controller
            .lock()
            .await
            .expire_transfer(transfer.id)
            .unwrap();
        publish_transfer_timeout(&state, expired);
        assert_eq!(revocations.recv().await.unwrap(), 20);
        assert!(
            state
                .controller
                .lock()
                .await
                .authorize(10, owner.id, splint_id, 3)
                .is_ok()
        );
    }

    #[test]
    fn controller_transfer_is_explicit_atomic_and_disconnect_bounded() {
        let splint_id = SplintId::new();
        let mut controllers = ControllerState::default();
        let owner = controllers.acquire(10, splint_id, 4, None).unwrap();
        let denied = controllers.request_transfer(20, splint_id, 4).unwrap();
        let (_, outcome, lease) = controllers
            .decide_transfer(10, denied.id, ControlTransferDecision::Deny)
            .unwrap();
        assert_eq!(outcome, ControlTransferOutcome::Denied);
        assert!(lease.is_none());
        assert!(controllers.authorize(10, owner.id, splint_id, 4).is_ok());

        let accepted = controllers.request_transfer(20, splint_id, 4).unwrap();
        let (_, outcome, lease) = controllers
            .decide_transfer(10, accepted.id, ControlTransferDecision::Accept)
            .unwrap();
        let lease = lease.unwrap();
        assert_eq!(outcome, ControlTransferOutcome::Granted);
        assert!(controllers.authorize(10, owner.id, splint_id, 4).is_err());
        assert!(controllers.authorize(20, lease.id, splint_id, 4).is_ok());

        controllers.release_connection(20);
        let owner = controllers.acquire(10, splint_id, 4, None).unwrap();
        let pending = controllers.request_transfer(30, splint_id, 4).unwrap();
        assert_eq!(controllers.cancel_connection_transfers(30), vec![pending]);
        assert!(controllers.authorize(10, owner.id, splint_id, 4).is_ok());

        let timed_out = controllers.request_transfer(31, splint_id, 4).unwrap();
        assert_eq!(controllers.expire_transfer(timed_out.id), Some(timed_out));
        assert_eq!(
            controllers
                .decide_transfer(10, timed_out.id, ControlTransferDecision::Accept)
                .unwrap_err()
                .code,
            ErrorCode::RequestNotFound
        );
        assert!(controllers.authorize(10, owner.id, splint_id, 4).is_ok());
    }

    #[test]
    fn first_party_ui_scope_policy_allows_terminal_history_but_excludes_external_authority() {
        assert!(first_party_ui_scopes(&[
            AccessScope::Observe,
            AccessScope::Input,
            AccessScope::Resize,
        ]));
        assert!(first_party_ui_scopes(&[
            AccessScope::Observe,
            AccessScope::Scrollback,
        ]));
        assert!(!first_party_ui_scopes(&[AccessScope::ClipboardRead]));
        assert!(!first_party_ui_scopes(&[AccessScope::ClipboardWrite]));
        assert!(!first_party_ui_scopes(&[AccessScope::Terminate]));
        assert!(!trusted_ui_request(&Request::AuditInspect {
            after_audit_id: None,
            max_records: 1,
        }));
    }

    #[test]
    fn development_access_is_explicit() {
        let error = ProtocolError::new(
            ErrorCode::DevelopmentFeatureDisabled,
            "development terminal access is disabled",
        );
        assert_eq!(error.code, ErrorCode::DevelopmentFeatureDisabled);
    }
}
