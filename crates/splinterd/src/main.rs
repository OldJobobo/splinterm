mod consent;
mod persistence;

use std::{
    collections::HashMap,
    env,
    ffi::OsString,
    io::ErrorKind,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use consent::{GrantStore, PeerIdentity};
use persistence::MetadataStore;
use splinterd::{
    LiveEvent, LiveSnapshot, LiveSplintConfig, LiveSplintHandle, LiveSplintRuntime, Subscription,
    SubscriptionReceive,
};
use splinterm_core::{
    Dojo, Lair, LairDocument, LairError, LayoutNode, Splint, SplintId, SplintLaunchMetadata,
    SplintState, Window,
};
use splinterm_protocol::{
    AccessGrant, AccessScope, ActiveScreen as WireActiveScreen, CellAttributes, ClientFrame,
    ColorSource, ErrorCode, HistoryTransition, MAX_COLUMNS, MAX_FRAME_BYTES, MAX_INPUT_BYTES,
    MAX_ROWS, MAX_SCROLLBACK_PAGE_ROWS, MAX_SNAPSHOT_SCROLLBACK_ROWS, MAX_SUBSCRIPTIONS,
    MouseTracking as WireMouseTracking, PROTOCOL_VERSION, ProcessExitStatus, ProtocolError,
    Request, Response, RestoreLeafResult, ScrollDirection as WireScrollDirection,
    ScrollbackPage as WireScrollbackPage, ServerFrame, ServerLimits, SplintLifecycle,
    SplintRuntimeSummary, SubscriptionEvent, TerminalCell, TerminalCursor, TerminalInputModes,
    TerminalRow, TerminalRowPatch, TerminalScroll, TerminalScrollbackUpdate, TerminalSnapshot,
    TerminalUpdate as WireTerminalUpdate, TopologyChange, TopologyChangeKind, TopologySnapshot,
    UnderlineStyle as WireUnderlineStyle, encode_frame,
};
use splinterm_pty::{LinuxPtyBackend, PtyCommand, PtySize, default_shell};
use splinterm_terminal::{
    ActiveScreen, ColorSource as TerminalColorSource, ScrollDirection, TerminalDamage,
    TerminalUpdate,
};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        UnixListener, UnixStream,
        unix::{OwnedReadHalf, OwnedWriteHalf},
    },
    signal,
    sync::{Mutex, RwLock, Semaphore, broadcast, mpsc},
    task::JoinHandle,
    time,
};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const CONNECTION_LIMIT: usize = 32;
const MAX_LIVE_SPLINTS: usize = 256;
const TOPOLOGY_QUEUE: usize = 16;
const OUTBOUND_QUEUE: usize = 32;
const CONTROL_QUEUE: usize = 4;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
static NEXT_SUBSCRIPTION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ControllerLease {
    id: u64,
    splint_id: SplintId,
    incarnation: u64,
    grant_id: Option<u64>,
}

#[derive(Debug)]
struct ControllerState {
    next_id: u64,
    by_id: HashMap<u64, ControllerLease>,
    by_splint: HashMap<SplintId, u64>,
}

impl Default for ControllerState {
    fn default() -> Self {
        Self {
            next_id: 1,
            by_id: HashMap::new(),
            by_splint: HashMap::new(),
        }
    }
}

impl ControllerState {
    fn acquire(
        &mut self,
        splint_id: SplintId,
        incarnation: u64,
        grant_id: Option<u64>,
    ) -> Result<ControllerLease, ProtocolError> {
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
            splint_id,
            incarnation,
            grant_id,
        };
        self.by_id.insert(id, lease);
        self.by_splint.insert(splint_id, id);
        Ok(lease)
    }

    fn authorize(
        &self,
        controller_id: u64,
        splint_id: SplintId,
        incarnation: u64,
    ) -> Result<(), ProtocolError> {
        match self.by_id.get(&controller_id) {
            Some(lease) if lease.splint_id == splint_id && lease.incarnation == incarnation => {
                Ok(())
            }
            _ => Err(ProtocolError::new(
                ErrorCode::Unauthorized,
                "controller lease is not owned by this connection",
            )),
        }
    }

    fn release(&mut self, controller_id: u64) -> bool {
        let Some(lease) = self.by_id.remove(&controller_id) else {
            return false;
        };
        self.by_splint.remove(&lease.splint_id);
        true
    }

    fn release_grant(&mut self, grant_id: u64) {
        let ids: Vec<_> = self
            .by_id
            .values()
            .filter(|lease| lease.grant_id == Some(grant_id))
            .map(|lease| lease.id)
            .collect();
        for id in ids {
            self.release(id);
        }
    }

    fn release_identity(&mut self, splint_id: SplintId, incarnation: u64) {
        let id = self
            .by_splint
            .get(&splint_id)
            .and_then(|id| self.by_id.get(id))
            .filter(|lease| lease.incarnation == incarnation)
            .map(|lease| lease.id);
        if let Some(id) = id {
            self.release(id);
        }
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
    metadata: Option<MetadataStore>,
    controller: Mutex<ControllerState>,
    grants: Mutex<GrantStore>,
    revocations: broadcast::Sender<Revocation>,
    pty_backend: LinuxPtyBackend,
    development_terminal_access: bool,
}

// Local PTY and Unix-socket work is asynchronous; bounding workers also bounds
// glibc per-thread allocator arenas after sustained terminal output.
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

    let socket = socket_path()?;
    prepare_socket_parent(&socket).await?;
    remove_stale_socket(&socket).await?;
    let listener = UnixListener::bind(&socket)
        .with_context(|| format!("failed to bind {}", socket.display()))?;
    fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).await?;
    verify_socket(&socket).await?;

    let (revocations, _) = broadcast::channel(32);
    let state = Arc::new(DaemonState {
        lair: RwLock::new(lair),
        runtimes: Mutex::new(RuntimeRegistry::default()),
        topology: Mutex::new(TopologyHub::default()),
        topology_transactions: Semaphore::new(1),
        metadata: Some(metadata),
        controller: Mutex::new(ControllerState::default()),
        grants: Mutex::new(GrantStore::default()),
        revocations,
        pty_backend: LinuxPtyBackend::installed()?,
        development_terminal_access: env::var_os("SPLINTERM_ENABLE_DEV_ATTACH").as_deref()
            == Some(std::ffi::OsStr::new("1")),
    });
    let connections = Arc::new(Semaphore::new(CONNECTION_LIMIT));
    info!(socket = %socket.display(), development_terminal_access = state.development_terminal_access, "splinterd ready");

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted.context("failed to accept client")?;
                let Ok(permit) = Arc::clone(&connections).try_acquire_owned() else {
                    warn!("connection limit reached");
                    continue;
                };
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Err(error) = serve_client(stream, state).await {
                        warn!(%error, "client connection closed");
                    }
                });
            }
            result = signal::ctrl_c() => {
                result.context("failed to listen for shutdown signal")?;
                break;
            }
        }
    }

    let runtimes = state.runtimes.lock().await.drain();
    let shutdown = async {
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
    };
    if time::timeout(Duration::from_secs(10), shutdown)
        .await
        .is_err()
    {
        error!("timed out while shutting down live Splints");
    }
    let final_lair = state.lair.read().await.clone();
    persist_lair(&state, &final_lair)
        .await
        .map_err(|_| anyhow::anyhow!("failed to persist final Lair metadata"))?;
    fs::remove_file(&socket).await?;
    Ok(())
}

async fn serve_client(stream: UnixStream, state: Arc<DaemonState>) -> Result<()> {
    let peer = verify_peer(&stream)?;
    let (reader, writer) = stream.into_split();
    let (outbound_tx, outbound_rx) = mpsc::channel(OUTBOUND_QUEUE);
    let (control_tx, control_rx) = mpsc::channel(CONTROL_QUEUE);
    let writer_task = tokio::spawn(write_frames(writer, outbound_rx, control_rx));
    let result = serve_authenticated(reader, &state, &peer, &outbound_tx, &control_tx).await;
    drop(outbound_tx);
    drop(control_tx);
    let _ = writer_task.await;
    result
}

#[allow(
    clippy::too_many_lines,
    reason = "the connection state machine keeps handshake and request-id enforcement together"
)]
async fn serve_authenticated(
    mut reader: OwnedReadHalf,
    state: &Arc<DaemonState>,
    peer: &PeerIdentity,
    outbound: &mpsc::Sender<ServerFrame>,
    control: &mpsc::Sender<ServerFrame>,
) -> Result<()> {
    let hello = time::timeout(HANDSHAKE_TIMEOUT, read_frame(&mut reader))
        .await
        .context("handshake timed out")??;
    let ClientFrame::Hello {
        minimum_version,
        maximum_version,
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

    let mut subscriptions = HashMap::<u64, JoinHandle<()>>::new();
    let mut owned_controller = None;
    let mut last_request_id = 0_u64;
    while let Some(frame) = read_optional_frame(&mut reader).await? {
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
                    if let Some(task) = subscriptions.remove(subscription_id) {
                        task.abort();
                    }
                    state.topology.lock().await.remove(*subscription_id);
                    send_response(outbound, request_id, Ok(Response::Acknowledged)).await?;
                    continue;
                }
                let handled = handle_request(request, state, peer, &mut owned_controller).await;
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
                                        outbound.clone(),
                                        control.clone(),
                                        state.revocations.subscribe(),
                                        access,
                                    ),
                                ),
                                PendingSubscription::Topology { id, stream } => (
                                    id,
                                    spawn_topology_subscription(id, stream, outbound.clone()),
                                ),
                            };
                            subscriptions.insert(id, task);
                        } else {
                            send_response(outbound, request_id, Ok(response)).await?;
                        }
                    }
                    Err(error) => send_response(outbound, request_id, Err(error)).await?,
                }
            }
        }
    }
    for (_, task) in subscriptions {
        task.abort();
    }
    if let Some(controller_id) = owned_controller {
        state.controller.lock().await.release(controller_id);
    }
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
    },
}

#[derive(Clone, Copy, Debug)]
struct SubscriptionAccess {
    grant_id: Option<u64>,
    scrollback_rows: usize,
    history: HistoryState,
}

#[derive(Clone, Copy, Debug)]
struct HistoryState {
    revision: u64,
    generation: u64,
    available_rows: usize,
}

async fn controlled_handle(
    state: &Arc<DaemonState>,
    owned_controller: &mut Option<u64>,
    controller_id: u64,
    splint_id: SplintId,
    incarnation: u64,
) -> Result<LiveSplintHandle, ProtocolError> {
    if *owned_controller != Some(controller_id) {
        return Err(ProtocolError::new(
            ErrorCode::Unauthorized,
            "controller lease is not owned by this connection",
        ));
    }
    let handle = match current_handle(state, splint_id, incarnation).await {
        Ok(handle) => handle,
        Err(error) => {
            state.controller.lock().await.release(controller_id);
            *owned_controller = None;
            return Err(error);
        }
    };
    state
        .controller
        .lock()
        .await
        .authorize(controller_id, splint_id, incarnation)?;
    Ok(handle)
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

fn trusted_first_party_ui(peer: &PeerIdentity, scopes: &[AccessScope]) -> bool {
    peer.is_matching_splinterm() && first_party_ui_scopes(scopes)
}

async fn authorize_scope(
    state: &DaemonState,
    peer: &PeerIdentity,
    splint_id: SplintId,
    incarnation: u64,
    scopes: &[AccessScope],
) -> Result<Option<u64>, ProtocolError> {
    if state.development_terminal_access || trusted_first_party_ui(peer, scopes) {
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

async fn spawn_runtime(
    state: &DaemonState,
    splint_id: SplintId,
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
    };
    let mut config = LiveSplintConfig::default();
    config.terminal.scrollback_lines = launch.scrollback_lines;
    LiveSplintRuntime::spawn(splint_id, state.pty_backend.clone(), pty_command, config)
        .await
        .map_err(|error| {
            error!(%error, ?splint_id, "failed to spawn live Splint");
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

    let runtime = spawn_runtime(state, splint_id, launch).await?;
    let handle = runtime.handle();
    let incarnation = handle.incarnation.value();
    let previous_lair = state.lair.read().await.clone();
    let prepared = durable_lair_candidate(state, |lair| {
        lair.commit_relaunch(splint_id, launch.cwd.clone(), launch.command.clone())?;
        if !lair.set_splint_launch_metadata(splint_id, durable_launch(launch)) {
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
    state
        .controller
        .lock()
        .await
        .release_identity(splint_id, incarnation);
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
    true
}

fn observe_process_exit(state: &Arc<DaemonState>, handle: LiveSplintHandle) {
    let state = Arc::clone(state);
    tokio::spawn(async move {
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

#[allow(
    clippy::too_many_lines,
    reason = "authorization remains adjacent to every sensitive operation"
)]
async fn handle_request(
    request: Request,
    state: &Arc<DaemonState>,
    peer: &PeerIdentity,
    owned_controller: &mut Option<u64>,
) -> Result<Handled, ProtocolError> {
    let response = match request {
        Request::Ping => Response::Pong,
        Request::ListDojos => Response::Dojos {
            dojos: state.lair.read().await.dojos().cloned().collect(),
        },
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
                subscription: Some(PendingSubscription::Topology { id, stream }),
            });
        }
        Request::InspectSplint { splint_id } => {
            let snapshot = topology_snapshot(state).await;
            let runtime = snapshot
                .runtimes
                .into_iter()
                .find(|runtime| runtime.splint_id == splint_id)
                .ok_or_else(not_found)?;
            Response::Splint { runtime }
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
            if state.development_terminal_access {
                Response::AccessGranted {
                    grant: development_grant(peer, splint_id, incarnation, scopes),
                }
            } else if trusted_first_party_ui(peer, &scopes) {
                Response::AccessGranted {
                    grant: first_party_grant(splint_id, incarnation, scopes),
                }
            } else if let Some(grant_id) =
                state
                    .grants
                    .lock()
                    .await
                    .authorize(peer, splint_id, incarnation, &scopes)
            {
                let grant = state
                    .grants
                    .lock()
                    .await
                    .status(splint_id, incarnation)
                    .into_iter()
                    .find(|grant| grant.grant_id == grant_id)
                    .ok_or_else(internal)?;
                Response::AccessGranted { grant }
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
                let grant = state
                    .grants
                    .lock()
                    .await
                    .grant(peer, splint_id, incarnation, scopes);
                Response::AccessGranted { grant }
            }
        }
        Request::AuthorizationStatus {
            splint_id,
            incarnation,
        } => {
            if !state.development_terminal_access && !peer.is_matching_splinterm() {
                return Err(ProtocolError::new(
                    ErrorCode::Unauthorized,
                    "authorization status is available only to trusted Splinterm UI",
                ));
            }
            let grants = state.grants.lock().await.status(splint_id, incarnation);
            Response::AuthorizationStatus {
                grants,
                development_bypass: state.development_terminal_access,
            }
        }
        Request::RevokeAccess { grant_id } => {
            if !state.development_terminal_access && !peer.is_matching_splinterm() {
                return Err(ProtocolError::new(
                    ErrorCode::Unauthorized,
                    "revocation is available only to trusted Splinterm UI",
                ));
            }
            let revoked = state
                .grants
                .lock()
                .await
                .revoke(grant_id)
                .ok_or_else(not_found)?;
            state.controller.lock().await.release_grant(grant_id);
            let _ = state.revocations.send(Revocation { grant_id });
            info!(
                grant_id,
                splint_id = ?revoked.splint_id,
                incarnation = revoked.incarnation,
                "terminal access grant revoked"
            );
            Response::Acknowledged
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
            let LayoutNode::Leaf(splint) = &mut dojo.windows[0].root else {
                unreachable!()
            };
            splint.command.clone_from(&launch.command);
            splint.launch = Box::new(durable_launch(&launch));
            let splint_id = splint.id;
            let runtime = spawn_runtime(state, splint_id, &launch).await?;
            let handle = runtime.handle();
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
            Response::DojoCreated { dojo }
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
            let runtime = spawn_runtime(state, splint_id, &launch).await?;
            let handle = runtime.handle();
            let incarnation = handle.incarnation.value();

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
        Request::RelaunchSplint { splint_id, launch } => {
            let _transaction = state
                .topology_transactions
                .acquire()
                .await
                .map_err(|_| internal())?;
            let (incarnation, topology_revision) =
                start_exited_splint(state, splint_id, &launch).await?;
            publish_topology(state, topology_revision, TopologyChangeKind::RuntimeChanged).await;
            Response::SplintStarted {
                splint_id,
                incarnation,
                topology_revision,
            }
        }
        Request::RestoreSplint { splint_id } => {
            let _transaction = state
                .topology_transactions
                .acquire()
                .await
                .map_err(|_| internal())?;
            if state.lair.read().await.find_splint(splint_id).is_none() {
                return Err(not_found());
            }
            let (topology_revision, results) = restore_targets(state, vec![splint_id]).await;
            Response::RestoreCompleted {
                topology_revision,
                results,
            }
        }
        Request::RestoreWindow { window_id } => {
            let _transaction = state
                .topology_transactions
                .acquire()
                .await
                .map_err(|_| internal())?;
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
        Request::RestoreDojo { dojo_id } => {
            let _transaction = state
                .topology_transactions
                .acquire()
                .await
                .map_err(|_| internal())?;
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
            let runtime = spawn_runtime(state, splint_id, &launch).await?;
            let handle = runtime.handle();
            let incarnation = handle.incarnation.value();
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
            let required = if scrollback_rows == 0 {
                vec![AccessScope::Observe]
            } else {
                vec![AccessScope::Observe, AccessScope::Scrollback]
            };
            let grant_id = authorize_scope(state, peer, splint_id, incarnation, &required).await?;
            let handle = current_handle(state, splint_id, incarnation).await?;
            let scrollback_rows = scrollback_rows.min(MAX_SNAPSHOT_SCROLLBACK_ROWS);
            let (snapshot, subscription) = handle
                .attach_with_scrollback(scrollback_rows)
                .await
                .map_err(|_| internal())?;
            let id = NEXT_SUBSCRIPTION.fetch_add(1, Ordering::Relaxed);
            let history = history_state(&snapshot);
            return Ok(Handled {
                response: Response::Attached {
                    subscription_id: id,
                    snapshot: wire_snapshot(snapshot),
                },
                subscription: Some(PendingSubscription::Terminal {
                    id,
                    stream: subscription,
                    handle,
                    access: SubscriptionAccess {
                        grant_id,
                        scrollback_rows,
                        history,
                    },
                }),
            });
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
            let _ = authorize_scope(
                state,
                peer,
                splint_id,
                incarnation,
                &[AccessScope::Observe, AccessScope::Scrollback],
            )
            .await?;
            let handle = current_handle(state, splint_id, incarnation).await?;
            let current = handle
                .snapshot_with_scrollback(1)
                .await
                .map_err(|_| internal())?;
            if current.revision.value() != terminal_revision
                || current.scrollback.history_generation != history_generation
            {
                return Ok(Handled {
                    response: Response::ScrollbackResyncRequired {
                        current_revision: current.revision.value(),
                        history_generation: current.scrollback.history_generation,
                    },
                    subscription: None,
                });
            }
            let page = handle
                .scrollback_page(before_row_id, max_rows)
                .await
                .map_err(|_| internal())?;
            if page.terminal_revision.value() != terminal_revision
                || page.history_generation != history_generation
            {
                return Ok(Handled {
                    response: Response::ScrollbackResyncRequired {
                        current_revision: page.terminal_revision.value(),
                        history_generation: page.history_generation,
                    },
                    subscription: None,
                });
            }
            Response::ScrollbackPage {
                page: WireScrollbackPage {
                    splint_id,
                    incarnation,
                    terminal_revision,
                    history_generation,
                    oldest_available_row_id: current.scrollback.oldest_available_row_id,
                    newest_available_row_id: current.scrollback.newest_available_row_id,
                    rows: page.rows.into_iter().map(wire_row).collect(),
                    has_older: page.has_older,
                },
            }
        }
        Request::AcquireControl {
            splint_id,
            incarnation,
        } => {
            let grant_id = if state.development_terminal_access
                || trusted_first_party_ui(peer, &[AccessScope::Input, AccessScope::Resize])
            {
                None
            } else {
                let mut grants = state.grants.lock().await;
                grants
                    .authorize(peer, splint_id, incarnation, &[AccessScope::Input])
                    .or_else(|| {
                        grants.authorize(peer, splint_id, incarnation, &[AccessScope::Resize])
                    })
                    .map(Some)
                    .ok_or_else(|| {
                        ProtocolError::new(
                            ErrorCode::Unauthorized,
                            "input or resize consent is required",
                        )
                    })?
            };
            let _ = current_handle(state, splint_id, incarnation).await?;
            if owned_controller.is_some() {
                return Err(ProtocolError::new(
                    ErrorCode::ControllerUnavailable,
                    "connection already owns a controller lease",
                ));
            }
            let lease = state
                .controller
                .lock()
                .await
                .acquire(splint_id, incarnation, grant_id)?;
            *owned_controller = Some(lease.id);
            Response::ControlGranted {
                controller_id: lease.id,
            }
        }
        Request::ReleaseControl { controller_id } => {
            if *owned_controller != Some(controller_id)
                || !state.controller.lock().await.release(controller_id)
            {
                return Err(ProtocolError::new(
                    ErrorCode::Unauthorized,
                    "controller lease is not owned by this connection",
                ));
            }
            *owned_controller = None;
            Response::Acknowledged
        }
        Request::Input {
            controller_id,
            splint_id,
            incarnation,
            bytes,
        } => {
            let _ =
                authorize_scope(state, peer, splint_id, incarnation, &[AccessScope::Input]).await?;
            if bytes.len() > MAX_INPUT_BYTES {
                return Err(invalid("input exceeds limit"));
            }
            controlled_handle(
                state,
                owned_controller,
                controller_id,
                splint_id,
                incarnation,
            )
            .await?
            .input(bytes)
            .await
            .map_err(|_| internal())?;
            Response::Acknowledged
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
            let _ = authorize_scope(state, peer, splint_id, incarnation, &[AccessScope::Resize])
                .await?;
            if columns == 0 || rows == 0 || columns > MAX_COLUMNS || rows > MAX_ROWS {
                return Err(invalid("terminal dimensions exceed limits"));
            }
            controlled_handle(
                state,
                owned_controller,
                controller_id,
                splint_id,
                incarnation,
            )
            .await?
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
            Response::Acknowledged
        }
        Request::Detach { .. } => Response::Acknowledged,
        Request::KillSplint {
            splint_id,
            incarnation,
        } => {
            let _ = authorize_scope(
                state,
                peer,
                splint_id,
                incarnation,
                &[AccessScope::Terminate],
            )
            .await?;
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
            let revoked = state.grants.lock().await.revoke_identity(
                splint_id,
                incarnation,
                "process terminated",
            );
            for grant_id in revoked {
                let _ = state.revocations.send(Revocation { grant_id });
            }
            *owned_controller = None;
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

fn spawn_topology_subscription(
    id: u64,
    mut subscription: TopologySubscription,
    outbound: mpsc::Sender<ServerFrame>,
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
            if outbound
                .send(ServerFrame::Event {
                    subscription_id: id,
                    sequence,
                    event,
                })
                .await
                .is_err()
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

fn spawn_subscription(
    id: u64,
    mut subscription: Subscription,
    handle: LiveSplintHandle,
    outbound: mpsc::Sender<ServerFrame>,
    control: mpsc::Sender<ServerFrame>,
    mut revocations: broadcast::Receiver<Revocation>,
    access: SubscriptionAccess,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut sequence = 1_u64;
        let mut previous_history = access.history;
        let expiry = time::sleep(consent::GRANT_LIFETIME);
        tokio::pin!(expiry);
        loop {
            let received = tokio::select! {
                value = subscription.recv() => value,
                revoked = revocations.recv(), if access.grant_id.is_some() => {
                    match revoked {
                        Ok(revocation) if Some(revocation.grant_id) == access.grant_id => {
                            let _ = control.send(ServerFrame::Event {
                                subscription_id: id,
                                sequence,
                                event: SubscriptionEvent::AccessRevoked {
                                    grant_id: revocation.grant_id,
                                },
                            }).await;
                            break;
                        }
                        Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                () = &mut expiry, if access.grant_id.is_some() => {
                    if let Some(grant_id) = access.grant_id {
                        let _ = control.send(ServerFrame::Event {
                            subscription_id: id,
                            sequence,
                            event: SubscriptionEvent::AccessRevoked { grant_id },
                        }).await;
                    }
                    break;
                }
            };
            match received {
                SubscriptionReceive::ResnapshotRequired => {
                    let revision = current_revision(&handle, access.scrollback_rows).await;
                    let _ = control
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
                    let _ = outbound.try_send(ServerFrame::Event {
                        subscription_id: id,
                        sequence,
                        event: SubscriptionEvent::Exited {
                            code: status.code,
                            signal: status.signal,
                        },
                    });
                    break;
                }
                SubscriptionReceive::Event(LiveEvent::Update { updates, .. }) => {
                    let Ok(snapshot) = handle
                        .snapshot_with_scrollback(access.scrollback_rows)
                        .await
                    else {
                        break;
                    };
                    let current_history = history_state(&snapshot);
                    if !revision_advances(previous_history.revision, current_history.revision) {
                        continue;
                    }
                    let event = subscription_update_event(&updates, snapshot, previous_history);
                    previous_history = current_history;

                    if outbound
                        .try_send(ServerFrame::Event {
                            subscription_id: id,
                            sequence,
                            event,
                        })
                        .is_err()
                    {
                        let revision = current_revision(&handle, access.scrollback_rows).await;
                        let _ = control
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
) -> SubscriptionEvent {
    if !revisions_match(updates, snapshot.revision.value()) {
        return SubscriptionEvent::Snapshot {
            snapshot: wire_snapshot(snapshot),
        };
    }
    match wire_update(
        updates,
        &snapshot,
        previous_history.revision,
        previous_history,
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
fn internal() -> ProtocolError {
    ProtocolError::new(ErrorCode::Internal, "operation failed")
}
fn not_found() -> ProtocolError {
    ProtocolError::new(ErrorCode::NotFound, "resource not found")
}

#[allow(
    clippy::too_many_lines,
    reason = "wire conversion keeps one revision's bounded semantic damage atomic"
)]
fn wire_update(
    updates: &[TerminalUpdate],
    snapshot: &LiveSnapshot,
    previous_revision: u64,
    previous_history: HistoryState,
) -> Result<WireTerminalUpdate, ProtocolError> {
    let mut damaged = vec![false; snapshot.visible_rows.len()];
    let mut scrolls = Vec::new();
    let mut cursor = false;
    let mut title = false;
    let mut modes = false;
    let mut palette = false;
    let mut dimensions = false;
    let mut scrollback = false;
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
        }
    }
    let position = snapshot.cursor.cursor.position();
    let rows = damaged
        .into_iter()
        .enumerate()
        .filter(|(_, changed)| *changed)
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
    })
}

fn wire_snapshot(snapshot: LiveSnapshot) -> TerminalSnapshot {
    let position = snapshot.cursor.cursor.position();
    let exited_code = snapshot.exited.and_then(|status| status.code);
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
        exited_code,
        exited_signal,
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

fn verify_peer(stream: &UnixStream) -> Result<PeerIdentity> {
    let identity = PeerIdentity::from_stream(stream)?;
    if identity.uid != rustix::process::geteuid().as_raw() {
        bail!("peer uid mismatch");
    }
    Ok(identity)
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

    fn test_state(development_terminal_access: bool) -> Arc<DaemonState> {
        let (revocations, _) = broadcast::channel(32);
        Arc::new(DaemonState {
            lair: RwLock::new(Lair::new()),
            runtimes: Mutex::new(RuntimeRegistry::default()),
            topology: Mutex::new(TopologyHub::default()),
            topology_transactions: Semaphore::new(1),
            metadata: None,
            controller: Mutex::new(ControllerState::default()),
            grants: Mutex::new(GrantStore::default()),
            revocations,
            pty_backend: LinuxPtyBackend::new("/missing/helper"),
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
        let mut owned_controller = None;
        let error = handle_request(
            Request::Attach {
                splint_id: SplintId::new(),
                incarnation: 1,
                scrollback_rows: 0,
            },
            &state,
            &peer,
            &mut owned_controller,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::Unauthorized);
    }

    #[tokio::test]
    async fn resize_limits_are_checked_before_runtime_access() {
        let state = test_state(true);
        let peer = PeerIdentity::for_test();
        let mut owned_controller = None;
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
            &mut owned_controller,
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
            .acquire(first_id, 7, Some(4))
            .expect("first controller");
        let second = controllers
            .acquire(second_id, 3, Some(5))
            .expect("different Splint controller");
        assert_eq!(
            controllers.acquire(first_id, 7, Some(4)).unwrap_err().code,
            ErrorCode::ControllerUnavailable
        );
        assert!(controllers.authorize(first.id, first_id, 7).is_ok());
        assert!(controllers.authorize(second.id, second_id, 3).is_ok());
        assert_eq!(
            controllers
                .authorize(first.id, second_id, 3)
                .unwrap_err()
                .code,
            ErrorCode::Unauthorized
        );
        controllers.release_identity(first_id, 8);
        assert!(controllers.authorize(first.id, first_id, 7).is_ok());
        controllers.release_identity(first_id, 7);
        assert!(controllers.authorize(first.id, first_id, 7).is_err());
        assert!(controllers.authorize(second.id, second_id, 3).is_ok());
        controllers.release_grant(5);
        assert!(controllers.authorize(second.id, second_id, 3).is_err());
        assert!(controllers.acquire(first_id, 8, None).is_ok());
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
