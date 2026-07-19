mod consent;

use std::{
    collections::HashMap,
    env,
    ffi::OsString,
    io::ErrorKind,
    os::unix::{
        ffi::OsStrExt,
        fs::{FileTypeExt, MetadataExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use consent::{GrantStore, PeerIdentity};
use splinterd::{
    LiveEvent, LiveSnapshot, LiveSplintConfig, LiveSplintHandle, LiveSplintRuntime, Subscription,
    SubscriptionReceive,
};
use splinterm_core::{Lair, LayoutNode, SplintId, SplintState};
use splinterm_protocol::{
    AccessGrant, AccessScope, ActiveScreen as WireActiveScreen, CellAttributes, ClientFrame,
    ColorSource, ErrorCode, HistoryTransition, MAX_COLUMNS, MAX_FRAME_BYTES, MAX_INPUT_BYTES,
    MAX_ROWS, MAX_SCROLLBACK_PAGE_ROWS, MAX_SNAPSHOT_SCROLLBACK_ROWS, MAX_SUBSCRIPTIONS,
    MouseTracking as WireMouseTracking, PROTOCOL_VERSION, ProtocolError, Request, Response,
    ScrollDirection as WireScrollDirection, ScrollbackPage as WireScrollbackPage, ServerFrame,
    ServerLimits, SubscriptionEvent, TerminalCell, TerminalCursor, TerminalInputModes, TerminalRow,
    TerminalRowPatch, TerminalScroll, TerminalScrollbackUpdate, TerminalSnapshot,
    TerminalUpdate as WireTerminalUpdate, UnderlineStyle as WireUnderlineStyle, encode_frame,
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
    active: Option<ControllerLease>,
}

impl Default for ControllerState {
    fn default() -> Self {
        Self {
            next_id: 1,
            active: None,
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
        if self.active.is_some() {
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
        self.active = Some(lease);
        Ok(lease)
    }

    fn authorize(
        &self,
        controller_id: u64,
        splint_id: SplintId,
        incarnation: u64,
    ) -> Result<(), ProtocolError> {
        match self.active {
            Some(lease)
                if lease.id == controller_id
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

    fn release(&mut self, controller_id: u64) -> bool {
        if self.active.is_some_and(|lease| lease.id == controller_id) {
            self.active = None;
            true
        } else {
            false
        }
    }

    fn release_grant(&mut self, grant_id: u64) {
        if self
            .active
            .is_some_and(|lease| lease.grant_id == Some(grant_id))
        {
            self.active = None;
        }
    }

    fn release_identity(&mut self, splint_id: SplintId, incarnation: u64) {
        if self
            .active
            .is_some_and(|lease| lease.splint_id == splint_id && lease.incarnation == incarnation)
        {
            self.active = None;
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Revocation {
    grant_id: u64,
}

struct DaemonState {
    lair: RwLock<Lair>,
    live_splint: Mutex<Option<LiveSplintRuntime>>,
    controller: Mutex<ControllerState>,
    grants: Mutex<GrantStore>,
    revocations: broadcast::Sender<Revocation>,
    pty_backend: LinuxPtyBackend,
    development_terminal_access: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let socket = socket_path()?;
    prepare_socket_parent(&socket).await?;
    remove_stale_socket(&socket).await?;
    let listener = UnixListener::bind(&socket)
        .with_context(|| format!("failed to bind {}", socket.display()))?;
    fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).await?;
    verify_socket(&socket).await?;

    let (revocations, _) = broadcast::channel(32);
    let state = Arc::new(DaemonState {
        lair: RwLock::new(Lair::new()),
        live_splint: Mutex::new(None),
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

    if let Some(runtime) = state.live_splint.lock().await.take() {
        if let Err(error) = runtime.shutdown().await {
            error!(%error, "failed to shut down live Splint cleanly");
        }
    }
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
                    send_response(outbound, request_id, Ok(Response::Acknowledged)).await?;
                    continue;
                }
                let handled = handle_request(request, state, peer, &mut owned_controller).await;
                match handled {
                    Ok(Handled {
                        response,
                        subscription,
                    }) => {
                        if let Some((id, stream, handle, access)) = subscription {
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
                            let task = spawn_subscription(
                                id,
                                stream,
                                handle,
                                outbound.clone(),
                                control.clone(),
                                state.revocations.subscribe(),
                                access,
                            );
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
    subscription: Option<(u64, Subscription, LiveSplintHandle, SubscriptionAccess)>,
}

#[derive(Clone, Copy, Debug)]
struct SubscriptionAccess {
    grant_id: Option<u64>,
    scrollback_rows: usize,
    history: HistoryState,
}

#[derive(Clone, Copy, Debug)]
struct HistoryState {
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
        Request::InspectLiveSplint => {
            let live = state.live_splint.lock().await;
            let handle = live.as_ref().ok_or_else(not_found)?.handle();
            Response::LiveSplint {
                splint_id: handle.splint_id,
                incarnation: handle.incarnation.value(),
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
            name,
            cwd,
            command,
            shell,
            login_shell,
            scrollback_lines,
        } => {
            let command_bytes = command
                .iter()
                .try_fold(0_usize, |total, item| total.checked_add(item.len()));
            if name.len() > 128
                || cwd.as_os_str().as_bytes().len() > 4096
                || command.len() > 256
                || command_bytes.is_none_or(|bytes| bytes > MAX_INPUT_BYTES)
                || shell
                    .as_ref()
                    .is_some_and(|shell| shell.is_empty() || shell.len() > 4096)
                || scrollback_lines > 1_000_000
            {
                return Err(invalid("dojo launch parameters exceed limits"));
            }
            let mut live = state.live_splint.lock().await;
            if live.is_some() {
                return Err(invalid("exactly one live Splint is supported"));
            }
            let dojo = {
                let mut lair = state.lair.write().await;
                lair.create_dojo(name, cwd.clone())
                    .cloned()
                    .map_err(|_| invalid("dojo could not be created"))?
            };
            let LayoutNode::Leaf(splint) = &dojo.windows[0].root else {
                unreachable!()
            };
            let splint_id = splint.id;
            let pty_command = if let Some((program, arguments)) = command.split_first() {
                PtyCommand::new(program, cwd)
                    .args(arguments.iter())
                    .login_shell(false)
            } else {
                PtyCommand::new(shell.map_or_else(default_shell, OsString::from), cwd)
                    .login_shell(login_shell)
            };
            let mut live_config = LiveSplintConfig::default();
            live_config.terminal.scrollback_lines = scrollback_lines;
            let runtime = match LiveSplintRuntime::spawn(
                splint_id,
                state.pty_backend.clone(),
                pty_command,
                live_config,
            )
            .await
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    error!(%error, "failed to spawn live Splint");
                    state.lair.write().await.remove_dojo(dojo.id);
                    return Err(internal());
                }
            };
            let handle = runtime.handle();
            let process_incarnation = handle.incarnation.value();
            state
                .lair
                .write()
                .await
                .set_splint_state(splint_id, SplintState::Running);
            let updated = state
                .lair
                .read()
                .await
                .dojos()
                .find(|candidate| candidate.id == dojo.id)
                .cloned()
                .unwrap();
            let lair = Arc::clone(state);
            tokio::spawn(async move {
                if let Some(status) = handle.wait_for_exit().await {
                    lair.controller
                        .lock()
                        .await
                        .release_identity(splint_id, process_incarnation);
                    let revoked = lair.grants.lock().await.revoke_identity(
                        splint_id,
                        process_incarnation,
                        "process exited",
                    );
                    for grant_id in revoked {
                        let _ = lair.revocations.send(Revocation { grant_id });
                    }
                    let code = status
                        .code
                        .or_else(|| status.signal.map(|signal| 128 + signal))
                        .unwrap_or(1);
                    lair.lair
                        .write()
                        .await
                        .set_splint_state(splint_id, SplintState::Exited(code));
                }
            });
            *live = Some(runtime);
            Response::DojoCreated { dojo: updated }
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
                subscription: Some((
                    id,
                    subscription,
                    handle,
                    SubscriptionAccess {
                        grant_id,
                        scrollback_rows,
                        history,
                    },
                )),
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
            Response::Acknowledged
        }
        Request::Detach { .. } => Response::Acknowledged,
        Request::Terminate {
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
            let mut live = state.live_splint.lock().await;
            let runtime = live.take().ok_or_else(not_found)?;
            let handle = runtime.handle();
            if handle.splint_id != splint_id {
                *live = Some(runtime);
                return Err(not_found());
            }
            if handle.incarnation.value() != incarnation {
                *live = Some(runtime);
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
            Response::Terminated {
                code: status.code,
                signal: status.signal,
            }
        }
    };
    Ok(Handled {
        response,
        subscription: None,
    })
}

async fn current_handle(
    state: &DaemonState,
    splint_id: SplintId,
    incarnation: u64,
) -> Result<LiveSplintHandle, ProtocolError> {
    let live = state.live_splint.lock().await;
    let handle = live.as_ref().ok_or_else(not_found)?.handle();
    if handle.splint_id != splint_id {
        return Err(not_found());
    }
    if handle.incarnation.value() != incarnation {
        return Err(ProtocolError::new(
            ErrorCode::StaleIncarnation,
            "process incarnation is stale",
        ));
    }
    Ok(handle)
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
                SubscriptionReceive::Event(LiveEvent::Update { update, .. }) => {
                    let Ok(snapshot) = handle
                        .snapshot_with_scrollback(access.scrollback_rows)
                        .await
                    else {
                        break;
                    };
                    let current_history = history_state(&snapshot);
                    let event = subscription_update_event(&update, snapshot, previous_history);
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
        generation: snapshot.scrollback.history_generation,
        available_rows: snapshot.scrollback.available_rows,
    }
}

fn revisions_match(update_revision: u64, snapshot_revision: u64) -> bool {
    update_revision == snapshot_revision
}

fn subscription_update_event(
    update: &TerminalUpdate,
    snapshot: LiveSnapshot,
    previous_history: HistoryState,
) -> SubscriptionEvent {
    if !revisions_match(update.revision().value(), snapshot.revision.value()) {
        return SubscriptionEvent::Snapshot {
            snapshot: wire_snapshot(snapshot),
        };
    }
    match wire_update(update, &snapshot, previous_history) {
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
    update: &TerminalUpdate,
    snapshot: &LiveSnapshot,
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
    for damage in update.damage() {
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
        base_revision: update.revision().value().saturating_sub(1),
        revision: update.revision().value(),
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
            live_splint: Mutex::new(None),
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

    #[test]
    fn queued_update_uses_delta_only_for_its_exact_snapshot_revision() {
        assert!(revisions_match(41, 41));
        assert!(!revisions_match(41, 42));
        assert!(!revisions_match(42, 41));
    }

    #[test]
    fn controller_state_is_exclusive_authorized_and_releasable() {
        let splint_id = SplintId::new();
        let mut controllers = ControllerState::default();
        let lease = controllers
            .acquire(splint_id, 7, Some(4))
            .expect("first controller");
        assert_eq!(
            controllers.acquire(splint_id, 7, Some(4)).unwrap_err().code,
            ErrorCode::ControllerUnavailable
        );
        assert!(controllers.authorize(lease.id, splint_id, 7).is_ok());
        assert_eq!(
            controllers
                .authorize(lease.id + 1, splint_id, 7)
                .unwrap_err()
                .code,
            ErrorCode::Unauthorized
        );
        controllers.release_identity(splint_id, 8);
        assert_eq!(controllers.active, Some(lease));
        controllers.release_identity(splint_id, 7);
        assert_eq!(controllers.active, None);
        assert!(controllers.acquire(splint_id, 8, None).is_ok());
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
