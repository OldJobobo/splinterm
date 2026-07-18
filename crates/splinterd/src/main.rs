use std::{
    collections::HashMap,
    env,
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
    time::Duration,
};

use anyhow::{Context, Result, bail};
use splinterd::{
    LiveEvent, LiveSnapshot, LiveSplintConfig, LiveSplintHandle, LiveSplintRuntime, Subscription,
    SubscriptionReceive,
};
use splinterm_core::{Lair, LayoutNode, SplintId, SplintState};
use splinterm_protocol::{
    ActiveScreen as WireActiveScreen, CellAttributes, ClientFrame, ColorSource, ErrorCode,
    MAX_COLUMNS, MAX_FRAME_BYTES, MAX_INPUT_BYTES, MAX_ROWS, MAX_SNAPSHOT_SCROLLBACK_ROWS,
    MAX_SUBSCRIPTIONS, MouseTracking as WireMouseTracking, PROTOCOL_VERSION, ProtocolError,
    Request, Response, ScrollDirection as WireScrollDirection, ServerFrame, ServerLimits,
    SubscriptionEvent, TerminalCell, TerminalCursor, TerminalInputModes, TerminalRow,
    TerminalRowPatch, TerminalScroll, TerminalSnapshot, TerminalUpdate as WireTerminalUpdate,
    encode_frame,
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
    sync::{Mutex, RwLock, Semaphore, mpsc},
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

    fn release_identity(&mut self, splint_id: SplintId, incarnation: u64) {
        if self
            .active
            .is_some_and(|lease| lease.splint_id == splint_id && lease.incarnation == incarnation)
        {
            self.active = None;
        }
    }
}

struct DaemonState {
    lair: RwLock<Lair>,
    live_splint: Mutex<Option<LiveSplintRuntime>>,
    controller: Mutex<ControllerState>,
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

    let state = Arc::new(DaemonState {
        lair: RwLock::new(Lair::new()),
        live_splint: Mutex::new(None),
        controller: Mutex::new(ControllerState::default()),
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
    verify_peer(&stream)?;
    let (reader, writer) = stream.into_split();
    let (outbound_tx, outbound_rx) = mpsc::channel(OUTBOUND_QUEUE);
    let (control_tx, control_rx) = mpsc::channel(CONTROL_QUEUE);
    let writer_task = tokio::spawn(write_frames(writer, outbound_rx, control_rx));
    let result = serve_authenticated(reader, &state, &outbound_tx, &control_tx).await;
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
                let handled = handle_request(request, state, &mut owned_controller).await;
                match handled {
                    Ok(Handled {
                        response,
                        subscription,
                    }) => {
                        if let Some((id, stream, handle)) = subscription {
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
    subscription: Option<(u64, Subscription, LiveSplintHandle)>,
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

#[allow(
    clippy::too_many_lines,
    reason = "authorization remains adjacent to every development operation"
)]
async fn handle_request(
    request: Request,
    state: &Arc<DaemonState>,
    owned_controller: &mut Option<u64>,
) -> Result<Handled, ProtocolError> {
    let response = match request {
        Request::Ping => Response::Pong,
        Request::ListDojos => Response::Dojos {
            dojos: state.lair.read().await.dojos().cloned().collect(),
        },
        Request::InspectLiveSplint => {
            require_dev(state)?;
            let live = state.live_splint.lock().await;
            let handle = live.as_ref().ok_or_else(not_found)?.handle();
            Response::LiveSplint {
                splint_id: handle.splint_id,
                incarnation: handle.incarnation.value(),
            }
        }
        Request::CreateDojo { name, cwd } => {
            require_dev(state)?;
            if name.len() > 128 || cwd.as_os_str().as_bytes().len() > 4096 {
                return Err(invalid("dojo name or working directory exceeds limit"));
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
            let command = PtyCommand::new(default_shell(), cwd).login_shell(true);
            let Ok(runtime) = LiveSplintRuntime::spawn(
                splint_id,
                state.pty_backend.clone(),
                command,
                LiveSplintConfig::default(),
            )
            .await
            else {
                state.lair.write().await.remove_dojo(dojo.id);
                return Err(internal());
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
            require_dev(state)?;
            let handle = current_handle(state, splint_id, incarnation).await?;
            let (snapshot, subscription) = handle
                .attach_with_scrollback(scrollback_rows.min(MAX_SNAPSHOT_SCROLLBACK_ROWS))
                .await
                .map_err(|_| internal())?;
            let id = NEXT_SUBSCRIPTION.fetch_add(1, Ordering::Relaxed);
            return Ok(Handled {
                response: Response::Attached {
                    subscription_id: id,
                    snapshot: wire_snapshot(snapshot),
                },
                subscription: Some((id, subscription, handle)),
            });
        }
        Request::AcquireControl {
            splint_id,
            incarnation,
        } => {
            require_dev(state)?;
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
                .acquire(splint_id, incarnation)?;
            *owned_controller = Some(lease.id);
            Response::ControlGranted {
                controller_id: lease.id,
            }
        }
        Request::ReleaseControl { controller_id } => {
            require_dev(state)?;
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
            require_dev(state)?;
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
            require_dev(state)?;
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
            require_dev(state)?;
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
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut sequence = 1_u64;
        loop {
            match subscription.recv().await {
                SubscriptionReceive::ResnapshotRequired => {
                    let revision = handle
                        .snapshot()
                        .await
                        .map_or(0, |snapshot| snapshot.revision.value());
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
                    let Ok(snapshot) = handle.snapshot().await else {
                        break;
                    };
                    let event = subscription_update_event(&update, snapshot);

                    if outbound
                        .try_send(ServerFrame::Event {
                            subscription_id: id,
                            sequence,
                            event,
                        })
                        .is_err()
                    {
                        let revision = handle
                            .snapshot()
                            .await
                            .map_or(0, |snapshot| snapshot.revision.value());
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

fn revisions_match(update_revision: u64, snapshot_revision: u64) -> bool {
    update_revision == snapshot_revision
}

fn subscription_update_event(update: &TerminalUpdate, snapshot: LiveSnapshot) -> SubscriptionEvent {
    if !revisions_match(update.revision().value(), snapshot.revision.value()) {
        return SubscriptionEvent::Snapshot {
            snapshot: wire_snapshot(snapshot),
        };
    }
    match wire_update(update, &snapshot) {
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
fn require_dev(state: &DaemonState) -> Result<(), ProtocolError> {
    if state.development_terminal_access {
        Ok(())
    } else {
        Err(ProtocolError::new(
            ErrorCode::DevelopmentFeatureDisabled,
            "development terminal access is disabled",
        ))
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

fn wire_update(
    update: &TerminalUpdate,
    snapshot: &LiveSnapshot,
) -> Result<WireTerminalUpdate, ProtocolError> {
    let mut damaged = vec![false; snapshot.visible_rows.len()];
    let mut scrolls = Vec::new();
    let mut cursor = false;
    let mut title = false;
    let mut modes = false;
    let mut palette = false;
    let mut dimensions = false;
    for damage in update.damage() {
        match damage {
            TerminalDamage::FullSnapshot => {
                damaged.fill(true);
                cursor = true;
                title = true;
                modes = true;
                palette = true;
                dimensions = true;
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
                damaged.fill(true);
            }
            TerminalDamage::Title => title = true,
            TerminalDamage::Palette { .. } => palette = true,
            TerminalDamage::Scrollback => {}
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
                    underline: cell.attributes.underline,
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

fn verify_peer(stream: &UnixStream) -> Result<()> {
    let credentials = stream.peer_cred().context("cannot read peer credentials")?;
    if credentials.uid() != rustix::process::geteuid().as_raw() {
        bail!("peer uid mismatch");
    }
    Ok(())
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
        let state = Arc::new(DaemonState {
            lair: RwLock::new(Lair::new()),
            live_splint: Mutex::new(None),
            controller: Mutex::new(ControllerState::default()),
            pty_backend: LinuxPtyBackend::new("/missing/helper"),
            development_terminal_access: false,
        });
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
    async fn development_terminal_operations_are_disabled_by_default() {
        let state = Arc::new(DaemonState {
            lair: RwLock::new(Lair::new()),
            live_splint: Mutex::new(None),
            controller: Mutex::new(ControllerState::default()),
            pty_backend: LinuxPtyBackend::new("/missing/helper"),
            development_terminal_access: false,
        });
        let mut owned_controller = None;
        let error = handle_request(
            Request::Attach {
                splint_id: SplintId::new(),
                incarnation: 1,
                scrollback_rows: 0,
            },
            &state,
            &mut owned_controller,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::DevelopmentFeatureDisabled);
    }

    #[tokio::test]
    async fn resize_limits_are_checked_before_runtime_access() {
        let state = Arc::new(DaemonState {
            lair: RwLock::new(Lair::new()),
            live_splint: Mutex::new(None),
            controller: Mutex::new(ControllerState::default()),
            pty_backend: LinuxPtyBackend::new("/missing/helper"),
            development_terminal_access: true,
        });
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
        let lease = controllers.acquire(splint_id, 7).expect("first controller");
        assert_eq!(
            controllers.acquire(splint_id, 7).unwrap_err().code,
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
        assert!(controllers.acquire(splint_id, 8).is_ok());
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
