#![forbid(unsafe_code)]

//! Byte-transparent, bounded stdio transport for one local Splinterm socket.

use std::{
    collections::HashMap,
    env,
    fs::File,
    future::Future,
    net::Shutdown,
    os::{
        fd::{AsFd, OwnedFd},
        unix::{
            fs::{FileTypeExt, MetadataExt, PermissionsExt},
            net::UnixStream as StdUnixStream,
        },
    },
    path::{Component, Path, PathBuf},
    sync::mpsc as std_mpsc,
    thread,
};

use anyhow::{Context, Result, bail};
use splinterm_graphical_relay::{
    FairData, FairDataChannel, Frame as GraphicalFrame, MAX_CHANNEL_QUEUED_BYTES, MAX_DATA_BYTES,
    MAX_LOGICAL_CHANNELS, fair_data_queue, read_frame as read_graphical_frame,
    write_data_frame as write_graphical_data_frame, write_frame as write_graphical_frame,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{UnixStream, unix::OwnedReadHalf, unix::OwnedWriteHalf},
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;

const COPY_BUFFER_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SocketIdentity {
    device: u64,
    inode: u64,
}

/// Resolves exactly one configured daemon socket.
///
/// # Errors
///
/// Returns an error when neither `SPLINTERM_SOCKET` nor `XDG_RUNTIME_DIR` is set.
pub fn socket_path() -> Result<PathBuf> {
    if let Some(path) = env::var_os("SPLINTERM_SOCKET") {
        return Ok(path.into());
    }
    let runtime = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .context("XDG_RUNTIME_DIR is unset; set SPLINTERM_SOCKET explicitly")?;
    Ok(runtime.join("splinterm/splinterd.sock"))
}

fn validate_socket_path(path: &Path) -> Result<SocketIdentity> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        bail!("daemon socket path must be absolute and normalized");
    }
    let parent = path
        .parent()
        .context("daemon socket path does not have a parent directory")?;
    let canonical_parent = parent
        .canonicalize()
        .context("cannot canonicalize daemon socket directory")?;
    if canonical_parent != parent {
        bail!("daemon socket directory must not contain symlinks");
    }
    let effective_uid = rustix::process::geteuid().as_raw();
    let parent_metadata = parent
        .symlink_metadata()
        .context("cannot inspect daemon socket directory")?;
    if !parent_metadata.file_type().is_dir()
        || parent_metadata.uid() != effective_uid
        || parent_metadata.permissions().mode() & 0o077 != 0
    {
        bail!("daemon socket directory must be owner-only and owned by the current user");
    }
    let metadata = path
        .symlink_metadata()
        .context("cannot inspect daemon socket endpoint")?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != effective_uid
        || metadata.permissions().mode() & 0o777 != 0o600
    {
        bail!("daemon endpoint must be an owner-only Unix socket owned by the current user");
    }
    Ok(SocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[derive(Debug)]
struct ValidatedConnection {
    stream: UnixStream,
    peer_pidfd: OwnedFd,
}

fn peer_exited(pidfd: &OwnedFd) -> Result<bool> {
    let mut descriptor = [nix::poll::PollFd::new(
        pidfd.as_fd(),
        nix::poll::PollFlags::POLLIN,
    )];
    nix::poll::poll(&mut descriptor, nix::poll::PollTimeout::ZERO)
        .context("cannot inspect daemon peer lifetime")?;
    Ok(descriptor[0].any().unwrap_or(true))
}

async fn connect_validated_for(path: &Path, expected_peer: &Path) -> Result<ValidatedConnection> {
    let before = validate_socket_path(path)?;
    let stream = UnixStream::connect(path)
        .await
        .context("cannot connect to the validated daemon socket")?;
    let effective_uid = rustix::process::geteuid().as_raw();
    let peer = stream
        .peer_cred()
        .context("cannot inspect daemon socket peer credentials")?;
    if peer.uid() != effective_uid {
        bail!("daemon socket peer is not owned by the current user");
    }
    let peer_pidfd = nix::sys::socket::getsockopt(&stream, nix::sys::socket::sockopt::PeerPidfd)
        .context("cannot bind daemon socket peer to a pidfd")?;
    if peer_exited(&peer_pidfd)? {
        bail!("daemon socket peer exited during validation");
    }
    let peer_pid = peer
        .pid()
        .context("daemon socket peer PID is unavailable")?;
    let peer_executable = File::open(format!("/proc/{peer_pid}/exe"))
        .context("cannot open daemon socket peer executable")?;
    let expected_executable = File::open(expected_peer)
        .context("cannot open the expected adjacent splinterd executable")?;
    let peer_metadata = peer_executable
        .metadata()
        .context("cannot inspect daemon socket peer executable")?;
    let expected_metadata = expected_executable
        .metadata()
        .context("cannot inspect the expected splinterd executable")?;
    if !peer_metadata.is_file()
        || !expected_metadata.is_file()
        || peer_metadata.dev() != expected_metadata.dev()
        || peer_metadata.ino() != expected_metadata.ino()
    {
        bail!("daemon socket peer is not the exact adjacent splinterd executable");
    }
    if peer_exited(&peer_pidfd)? {
        bail!("daemon socket peer exited during executable validation");
    }
    if validate_socket_path(path)? != before {
        bail!("daemon socket endpoint changed while connecting");
    }
    Ok(ValidatedConnection { stream, peer_pidfd })
}

/// Connects once to a strictly validated adjacent `splinterd` process.
///
/// # Errors
///
/// Returns an error when path metadata, connected peer credentials/executable,
/// or the post-connect endpoint identity does not match the owner-only contract.
async fn connect_validated(path: &Path) -> Result<ValidatedConnection> {
    let current = env::current_exe().context("cannot resolve the relay executable")?;
    connect_validated_for(path, &current.with_file_name("splinterd")).await
}

async fn copy_bounded<R, W>(reader: &mut R, writer: &mut W) -> std::io::Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buffer = Box::new([0_u8; COPY_BUFFER_BYTES]);
    let mut copied = 0_u64;
    loop {
        let count = reader.read(buffer.as_mut_slice()).await?;
        if count == 0 {
            writer.shutdown().await?;
            return Ok(copied);
        }
        writer.write_all(&buffer[..count]).await?;
        copied = copied.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
    }
}

/// Relays opaque bytes between separate input/output streams and one socket.
///
/// Input EOF half-closes only the socket write side and continues draining
/// daemon output. Daemon EOF cancels any blocked input read by dropping the
/// upstream future. No frame bytes are parsed or rewritten.
///
/// # Errors
///
/// Returns the first transport error from either direction.
pub async fn relay_streams<R, W>(input: R, output: W, stream: UnixStream) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let (mut daemon_reader, mut daemon_writer) = stream.into_split();
    let mut input = input;
    let mut output = output;
    let upstream = copy_bounded(&mut input, &mut daemon_writer);
    let downstream = copy_bounded(&mut daemon_reader, &mut output);
    tokio::pin!(upstream);
    tokio::pin!(downstream);

    tokio::select! {
        result = &mut upstream => {
            result.context("stdin-to-daemon relay failed")?;
            downstream.await.context("daemon-to-stdout relay failed")?;
        }
        result = &mut downstream => {
            result.context("daemon-to-stdout relay failed")?;
        }
    }
    Ok(())
}

fn copy_bounded_blocking<R, W>(reader: &mut R, writer: &mut W) -> std::io::Result<u64>
where
    R: std::io::Read,
    W: std::io::Write,
{
    let mut buffer = Box::new([0_u8; COPY_BUFFER_BYTES]);
    let mut copied = 0_u64;
    loop {
        let count = reader.read(buffer.as_mut_slice())?;
        if count == 0 {
            writer.flush()?;
            return Ok(copied);
        }
        writer.write_all(&buffer[..count])?;
        writer.flush()?;
        copied = copied.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
    }
}

#[derive(Clone, Copy)]
enum CompletedDirection {
    Upstream,
    Downstream,
    Peer,
}

fn relay_process_stdio(stream: StdUnixStream, peer_pidfd: OwnedFd) -> Result<()> {
    let mut upstream_socket = stream
        .try_clone()
        .context("cannot duplicate relay socket")?;
    let (completed_tx, completed_rx) = std_mpsc::channel();
    let upstream_tx = completed_tx.clone();
    thread::spawn(move || {
        let result = copy_bounded_blocking(&mut std::io::stdin().lock(), &mut upstream_socket)
            .and_then(|copied| {
                upstream_socket.shutdown(Shutdown::Write)?;
                Ok(copied)
            });
        let _ = upstream_tx.send((CompletedDirection::Upstream, result));
    });
    let downstream_tx = completed_tx.clone();
    thread::spawn(move || {
        let result = copy_bounded_blocking(&mut &stream, &mut std::io::stdout().lock());
        let _ = downstream_tx.send((CompletedDirection::Downstream, result));
    });
    thread::spawn(move || {
        let mut descriptor = [nix::poll::PollFd::new(
            peer_pidfd.as_fd(),
            nix::poll::PollFlags::POLLIN,
        )];
        let result = nix::poll::poll(&mut descriptor, nix::poll::PollTimeout::NONE)
            .map(|_| 0_u64)
            .map_err(std::io::Error::from);
        let _ = completed_tx.send((CompletedDirection::Peer, result));
    });

    let (direction, result) = completed_rx
        .recv()
        .context("relay worker exited without reporting completion")?;
    result.context(match direction {
        CompletedDirection::Upstream => "stdin-to-daemon relay failed",
        CompletedDirection::Downstream => "daemon-to-stdout relay failed",
        CompletedDirection::Peer => "daemon peer monitor failed",
    })?;
    if matches!(direction, CompletedDirection::Upstream) {
        let (_, downstream) = completed_rx
            .recv()
            .context("daemon-to-stdout relay worker did not complete")?;
        downstream.context("daemon-to-stdout relay failed")?;
    }
    Ok(())
}

/// Connects to the configured socket and relays process stdin/stdout.
///
/// # Errors
///
/// Returns an error when socket validation, connection, or transport fails.
pub async fn run_stdio() -> Result<()> {
    let path = socket_path()?;
    let connection = connect_validated(&path).await?;
    let stream = connection
        .stream
        .into_std()
        .context("cannot convert the validated daemon socket")?;
    stream
        .set_nonblocking(false)
        .context("cannot configure the relay socket")?;
    tokio::task::spawn_blocking(move || relay_process_stdio(stream, connection.peer_pidfd))
        .await
        .context("relay coordinator task failed")?
}

const CHANNEL_QUEUE_FRAMES: usize = MAX_CHANNEL_QUEUED_BYTES / MAX_DATA_BYTES;
const CONTROL_QUEUE_FRAMES: usize = MAX_LOGICAL_CHANNELS * 2 + 16;

#[derive(Debug)]
enum ChannelCommand {
    Data(Vec<u8>),
    HalfClose,
    Close,
}

#[derive(Debug)]
enum GraphicalEvent {
    ChannelFinished(u32),
    DaemonExited,
    SessionFailure(String),
}

#[derive(Debug)]
enum ScheduledOutput {
    Control(GraphicalFrame),
    Data(FairData),
}

#[derive(Debug)]
struct ActiveChannel {
    commands: mpsc::Sender<ChannelCommand>,
    cancellation: CancellationToken,
}

#[derive(Debug)]
struct ChannelOutput {
    control: mpsc::Sender<GraphicalFrame>,
    data: FairDataChannel,
    events: mpsc::Sender<GraphicalEvent>,
}

fn bounded_reason(error: &anyhow::Error) -> String {
    let reason = format!("{error:#}");
    let reason: String = reason
        .chars()
        .filter(|character| character.is_ascii() && !character.is_control())
        .take(1024)
        .collect();
    if reason.is_empty() {
        "graphical relay channel failed".to_owned()
    } else {
        reason
    }
}

async fn monitor_daemon_peer(
    peer_pidfd: OwnedFd,
    cancellation: CancellationToken,
    events: mpsc::Sender<GraphicalEvent>,
) {
    let descriptor = match tokio::io::unix::AsyncFd::new(peer_pidfd) {
        Ok(descriptor) => descriptor,
        Err(error) => {
            let _ = events
                .send(GraphicalEvent::SessionFailure(format!(
                    "cannot monitor daemon peer: {error}"
                )))
                .await;
            return;
        }
    };
    tokio::select! {
        () = cancellation.cancelled() => {}
        result = descriptor.readable() => {
            match result {
                Ok(_) => { let _ = events.send(GraphicalEvent::DaemonExited).await; }
                Err(error) => {
                    let _ = events.send(GraphicalEvent::SessionFailure(format!(
                        "daemon peer monitor failed: {error}"
                    ))).await;
                }
            }
        }
    }
}

async fn finish_channel(
    channel_id: u32,
    control_outbound: &mpsc::Sender<GraphicalFrame>,
    events: &mpsc::Sender<GraphicalEvent>,
) {
    let _ = control_outbound
        .send(GraphicalFrame::CloseChannel { channel_id })
        .await;
    let _ = events
        .send(GraphicalEvent::ChannelFinished(channel_id))
        .await;
}

async fn apply_channel_command(
    command: Option<ChannelCommand>,
    upstream_open: &mut bool,
    daemon_writer: &mut OwnedWriteHalf,
    events: &mpsc::Sender<GraphicalEvent>,
) -> bool {
    match command {
        Some(ChannelCommand::Data(bytes)) if *upstream_open => {
            daemon_writer.write_all(&bytes).await.is_ok()
        }
        Some(ChannelCommand::Data(_)) => {
            let _ = events
                .send(GraphicalEvent::SessionFailure(
                    "data followed a logical channel half-close".to_owned(),
                ))
                .await;
            false
        }
        Some(ChannelCommand::HalfClose) if *upstream_open => {
            *upstream_open = false;
            daemon_writer.shutdown().await.is_ok()
        }
        Some(ChannelCommand::HalfClose) => {
            let _ = events
                .send(GraphicalEvent::SessionFailure(
                    "logical channel was half-closed twice".to_owned(),
                ))
                .await;
            false
        }
        Some(ChannelCommand::Close) | None => false,
    }
}

async fn run_graphical_channel(
    channel_id: u32,
    daemon_reader: OwnedReadHalf,
    daemon_writer: OwnedWriteHalf,
    mut commands: mpsc::Receiver<ChannelCommand>,
    output: ChannelOutput,
    cancellation: CancellationToken,
) {
    let mut daemon_reader = daemon_reader;
    let mut daemon_writer = daemon_writer;
    let mut read_buffer = vec![0_u8; MAX_DATA_BYTES];
    let mut upstream_open = true;
    'channel: loop {
        let permit = tokio::select! {
            () = cancellation.cancelled() => break,
            command = commands.recv() => {
                if !apply_channel_command(
                    command,
                    &mut upstream_open,
                    &mut daemon_writer,
                    &output.events,
                ).await {
                    break;
                }
                continue 'channel;
            }
            permit = output.data.reserve() => match permit {
                Ok(permit) => permit,
                Err(_) => break,
            },
        };
        let read = tokio::select! {
            () = cancellation.cancelled() => break,
            command = commands.recv() => {
                drop(permit);
                if !apply_channel_command(
                    command,
                    &mut upstream_open,
                    &mut daemon_writer,
                    &output.events,
                ).await {
                    break;
                }
                continue 'channel;
            }
            read = daemon_reader.read(&mut read_buffer) => read,
        };
        match read {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                if permit.send(read_buffer[..count].to_vec()).is_err() {
                    break;
                }
            }
        }
    }
    cancellation.cancel();
    let _ = output.data.drain().await;
    finish_channel(channel_id, &output.control, &output.events).await;
}

async fn run_graphical_input_reader<R>(
    mut input: R,
    input_tx: mpsc::Sender<Result<Option<GraphicalFrame>>>,
    cancellation: CancellationToken,
) where
    R: AsyncRead + Unpin,
{
    loop {
        let frame = tokio::select! {
            () = cancellation.cancelled() => return,
            frame = read_graphical_frame(&mut input) => frame,
        };
        let terminal = !matches!(frame, Ok(Some(_)));
        let sent = tokio::select! {
            () = cancellation.cancelled() => return,
            sent = input_tx.send(frame) => sent,
        };
        if sent.is_err() || terminal {
            return;
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the bounded coordinator keeps one explicit session state machine"
)]
async fn run_graphical_streams_with_connector<R, W, C, F>(
    mut input: R,
    mut output: W,
    connector: C,
) -> Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
    C: Fn() -> F,
    F: Future<Output = Result<ValidatedConnection>>,
{
    match read_graphical_frame(&mut input).await? {
        Some(GraphicalFrame::Hello) => {}
        Some(_) => bail!("graphical relay handshake must begin with Hello"),
        None => bail!("graphical relay input closed before Hello"),
    }
    write_graphical_frame(&mut output, &GraphicalFrame::HelloAck).await?;

    let cancellation = CancellationToken::new();
    let input_cancellation = cancellation.clone();
    let (input_tx, mut input_rx) = mpsc::channel(1);
    let input_reader = tokio::spawn(run_graphical_input_reader(
        input,
        input_tx,
        input_cancellation,
    ));
    let (control_tx, mut control_rx) = mpsc::channel::<GraphicalFrame>(CONTROL_QUEUE_FRAMES);
    let (data_tx, mut data_rx) = fair_data_queue();
    let (event_tx, mut event_rx) = mpsc::channel::<GraphicalEvent>(MAX_LOGICAL_CHANNELS * 2 + 1);
    let writer_events = event_tx.clone();
    let writer = tokio::spawn(async move {
        let mut control_open = true;
        let mut data_open = true;
        while control_open || data_open {
            let scheduled = tokio::select! {
                biased;
                frame = control_rx.recv(), if control_open => if let Some(frame) = frame {
                    Some(ScheduledOutput::Control(frame))
                } else {
                    control_open = false;
                    None
                },
                data = data_rx.recv(), if data_open => if let Some(data) = data {
                    Some(ScheduledOutput::Data(data))
                } else {
                    data_open = false;
                    None
                },
            };
            let Some(scheduled) = scheduled else {
                continue;
            };
            let result = match &scheduled {
                ScheduledOutput::Control(frame) => write_graphical_frame(&mut output, frame).await,
                ScheduledOutput::Data(data) => {
                    write_graphical_data_frame(&mut output, data.channel_id(), data.bytes()).await
                }
            };
            if let Err(error) = result {
                let _ = writer_events
                    .send(GraphicalEvent::SessionFailure(format!(
                        "graphical relay output failed: {error}"
                    )))
                    .await;
                return;
            }
        }
        let _ = output.shutdown().await;
    });

    let mut channels = HashMap::<u32, ActiveChannel>::new();
    let mut last_channel_id = 0_u32;
    let mut terminal_error = None;
    loop {
        tokio::select! {
            event = event_rx.recv() => match event {
                Some(GraphicalEvent::ChannelFinished(channel_id)) => {
                    channels.remove(&channel_id);
                }
                Some(GraphicalEvent::DaemonExited) => {
                    terminal_error = Some("validated splinterd process exited".to_owned());
                    break;
                }
                Some(GraphicalEvent::SessionFailure(reason)) => {
                    terminal_error = Some(reason);
                    break;
                }
                None => {
                    terminal_error = Some("graphical relay coordinator stopped".to_owned());
                    break;
                }
            },
            frame = input_rx.recv() => {
                let frame = match frame {
                    Some(Ok(Some(frame))) => frame,
                    Some(Ok(None)) => break,
                    Some(Err(error)) => {
                        terminal_error = Some(bounded_reason(&error));
                        break;
                    }
                    None => {
                        terminal_error = Some("graphical relay input reader stopped".to_owned());
                        break;
                    }
                };
                match frame {
                    GraphicalFrame::OpenChannel { channel_id } => {
                        if channel_id <= last_channel_id {
                            terminal_error = Some(
                                "graphical relay channel IDs must increase without reuse".to_owned(),
                            );
                            break;
                        }
                        last_channel_id = channel_id;
                        if channels.len() >= MAX_LOGICAL_CHANNELS {
                            if control_tx.send(GraphicalFrame::ChannelRejected {
                                channel_id,
                                reason: "graphical relay logical channel limit reached".to_owned(),
                            }).await.is_err() {
                                terminal_error = Some("graphical relay output closed".to_owned());
                                break;
                            }
                            continue;
                        }
                        match connector().await {
                            Ok(connection) => {
                                let channel_cancellation = cancellation.child_token();
                                let (commands, command_rx) =
                                    mpsc::channel(CHANNEL_QUEUE_FRAMES);
                                let (daemon_reader, daemon_writer) = connection.stream.into_split();
                                channels.insert(channel_id, ActiveChannel {
                                    commands,
                                    cancellation: channel_cancellation.clone(),
                                });
                                // Queue the admission acknowledgement before either channel task
                                // can emit data or a daemon-lifetime failure.
                                if control_tx
                                    .send(GraphicalFrame::ChannelOpened { channel_id })
                                    .await
                                    .is_err()
                                {
                                    terminal_error = Some("graphical relay output closed".to_owned());
                                    break;
                                }
                                tokio::spawn(monitor_daemon_peer(
                                    connection.peer_pidfd,
                                    channel_cancellation.clone(),
                                    event_tx.clone(),
                                ));
                                tokio::spawn(run_graphical_channel(
                                    channel_id,
                                    daemon_reader,
                                    daemon_writer,
                                    command_rx,
                                    ChannelOutput {
                                        control: control_tx.clone(),
                                        data: data_tx.channel(channel_id),
                                        events: event_tx.clone(),
                                    },
                                    channel_cancellation,
                                ));
                            }
                            Err(error) => {
                                if control_tx.send(GraphicalFrame::ChannelRejected {
                                    channel_id,
                                    reason: bounded_reason(&error),
                                }).await.is_err() {
                                    terminal_error = Some("graphical relay output closed".to_owned());
                                    break;
                                }
                            }
                        }
                    }
                    GraphicalFrame::Data { channel_id, bytes } => {
                        let Some(channel) = channels.get(&channel_id) else {
                            terminal_error = Some("data targeted an unknown logical channel".to_owned());
                            break;
                        };
                        if channel.commands.try_send(ChannelCommand::Data(bytes)).is_err() {
                            terminal_error = Some(
                                "logical channel input queue exceeded its byte bound".to_owned(),
                            );
                            break;
                        }
                    }
                    GraphicalFrame::HalfClose { channel_id } => {
                        if let Some(channel) = channels.get(&channel_id) {
                            if channel.commands.try_send(ChannelCommand::HalfClose).is_err() {
                                terminal_error = Some(
                                    "logical channel input queue exceeded its byte bound".to_owned(),
                                );
                                break;
                            }
                        } else if channel_id > last_channel_id {
                            terminal_error = Some("half-close targeted an unknown logical channel".to_owned());
                            break;
                        }
                    }
                    GraphicalFrame::CloseChannel { channel_id } => {
                        if let Some(channel) = channels.remove(&channel_id) {
                            let _ = channel.commands.try_send(ChannelCommand::Close);
                            channel.cancellation.cancel();
                        } else if channel_id > last_channel_id {
                            terminal_error = Some("close targeted an unknown logical channel".to_owned());
                            break;
                        }
                        // A local close can cross channel-local daemon EOF after the
                        // relay has already retired the same monotonically issued ID.
                        // Treat that close as idempotent without accepting data or a
                        // half-close for any retired or unknown channel.
                    }
                    GraphicalFrame::Hello
                    | GraphicalFrame::HelloAck
                    | GraphicalFrame::ChannelOpened { .. }
                    | GraphicalFrame::ChannelRejected { .. }
                    | GraphicalFrame::SessionError { .. } => {
                        terminal_error = Some("client sent an invalid graphical relay frame".to_owned());
                        break;
                    }
                }
            }
        }
    }

    if let Some(reason) = terminal_error {
        let _ = control_tx.try_send(GraphicalFrame::SessionError { reason });
    }
    cancellation.cancel();
    drop(input_rx);
    let _ = input_reader.await;
    for channel in channels.into_values() {
        channel.cancellation.cancel();
    }
    drop(control_tx);
    drop(data_tx);
    let mut writer = writer;
    tokio::select! {
        result = &mut writer => result.context("graphical relay writer task failed")?,
        () = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
            writer.abort();
            let _ = writer.await;
        }
    }
    Ok(())
}

/// Runs the bounded graphical multiplexer over process stdin/stdout.
///
/// # Errors
///
/// Returns an error for invalid outer framing, socket validation failure, daemon
/// death, or stdio transport failure.
pub async fn run_graphical_stdio() -> Result<()> {
    let path = socket_path()?;
    run_graphical_streams_with_connector(tokio::io::stdin(), tokio::io::stdout(), || {
        connect_validated(&path)
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        fs,
        os::unix::fs::PermissionsExt,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::UnixListener,
        time,
    };

    use super::*;

    fn validated_pair() -> (ValidatedConnection, UnixStream) {
        let (stream, peer) = UnixStream::pair().unwrap();
        let peer_pidfd =
            nix::sys::socket::getsockopt(&stream, nix::sys::socket::sockopt::PeerPidfd).unwrap();
        (ValidatedConnection { stream, peer_pidfd }, peer)
    }

    fn queued_connector(
        connections: Arc<Mutex<VecDeque<ValidatedConnection>>>,
    ) -> impl Fn() -> std::future::Ready<Result<ValidatedConnection>> {
        move || {
            std::future::ready(
                connections
                    .lock()
                    .unwrap()
                    .pop_front()
                    .context("test connector exhausted"),
            )
        }
    }

    fn test_directory(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "splinterm-relay-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[tokio::test]
    async fn validated_connect_requires_owner_only_stable_socket() {
        let directory = test_directory("socket");
        let path = directory.join("daemon.sock");
        let listener = UnixListener::bind(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let accepted = tokio::spawn(async move { listener.accept().await.unwrap().0 });

        let expected_peer = env::current_exe().unwrap();
        let connection = connect_validated_for(&path, &expected_peer).await.unwrap();
        assert!(
            rustix::io::fcntl_getfd(&connection.stream)
                .unwrap()
                .contains(rustix::io::FdFlags::CLOEXEC)
        );
        let peer = accepted.await.unwrap();
        drop(connection);
        drop(peer);
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(connect_validated_for(&path, &expected_peer).await.is_err());

        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[tokio::test]
    async fn same_uid_non_daemon_peer_is_rejected() {
        let directory = test_directory("peer");
        let path = directory.join("daemon.sock");
        let listener = UnixListener::bind(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let accepted = tokio::spawn(async move { listener.accept().await.unwrap().0 });

        let error = connect_validated_for(&path, Path::new("/bin/false"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("adjacent splinterd"));
        drop(accepted.await.unwrap());
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[tokio::test]
    async fn relay_is_byte_transparent_and_preserves_input_half_close() {
        let (mut input_writer, input_reader) = tokio::io::duplex(32 * 1024);
        let (output_writer, mut output_reader) = tokio::io::duplex(32 * 1024);
        let (relay_socket, mut daemon_socket) = UnixStream::pair().unwrap();
        let request = vec![0xa5; 2 * 1024 * 1024];
        let response = vec![0x5a; 2 * 1024 * 1024];
        let expected_request = request.clone();
        let expected_response = response.clone();

        let relay = tokio::spawn(relay_streams(input_reader, output_writer, relay_socket));
        let input = tokio::spawn(async move {
            input_writer.write_all(&request).await.unwrap();
            input_writer.shutdown().await.unwrap();
        });
        let daemon = tokio::spawn(async move {
            let mut received = Vec::new();
            daemon_socket.read_to_end(&mut received).await.unwrap();
            assert_eq!(received, expected_request);
            daemon_socket.write_all(&response).await.unwrap();
            daemon_socket.shutdown().await.unwrap();
        });
        let output = tokio::spawn(async move {
            let mut received = Vec::new();
            output_reader.read_to_end(&mut received).await.unwrap();
            assert_eq!(received, expected_response);
        });

        time::timeout(Duration::from_secs(10), async {
            input.await.unwrap();
            daemon.await.unwrap();
            relay.await.unwrap().unwrap();
            output.await.unwrap();
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn upstream_backpressure_is_bounded_until_daemon_reads() {
        let (mut input_writer, input_reader) = tokio::io::duplex(1024);
        let (output_writer, _output_reader) = tokio::io::duplex(1024);
        let (relay_socket, mut daemon_socket) = UnixStream::pair().unwrap();
        let relay = tokio::spawn(relay_streams(input_reader, output_writer, relay_socket));
        let input = tokio::spawn(async move {
            input_writer
                .write_all(&vec![0x3c; 1024 * 1024])
                .await
                .unwrap();
            input_writer.shutdown().await.unwrap();
        });

        time::sleep(Duration::from_millis(100)).await;
        assert!(!input.is_finished(), "upstream ignored daemon backpressure");
        let daemon = tokio::spawn(async move {
            let mut received = Vec::new();
            daemon_socket.read_to_end(&mut received).await.unwrap();
            assert_eq!(received, vec![0x3c; 1024 * 1024]);
        });
        time::timeout(Duration::from_secs(10), async {
            input.await.unwrap();
            daemon.await.unwrap();
            relay.await.unwrap().unwrap();
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn daemon_eof_cancels_a_blocked_input_direction() {
        let (_input_writer, input_reader) = tokio::io::duplex(1024);
        let (output_writer, mut output_reader) = tokio::io::duplex(1024);
        let (relay_socket, daemon_socket) = UnixStream::pair().unwrap();
        drop(daemon_socket);

        time::timeout(
            Duration::from_secs(2),
            relay_streams(input_reader, output_writer, relay_socket),
        )
        .await
        .unwrap()
        .unwrap();
        let mut output = Vec::new();
        output_reader.read_to_end(&mut output).await.unwrap();
        assert!(output.is_empty());
    }

    #[test]
    fn graphical_queue_cap_rejects_before_exceeding_byte_bound() {
        let (commands, _receiver) = mpsc::channel(CHANNEL_QUEUE_FRAMES);
        for _ in 0..CHANNEL_QUEUE_FRAMES {
            commands
                .try_send(ChannelCommand::Data(vec![0; MAX_DATA_BYTES]))
                .unwrap();
        }
        assert!(
            commands
                .try_send(ChannelCommand::Data(vec![0; MAX_DATA_BYTES]))
                .is_err()
        );
        assert_eq!(
            CHANNEL_QUEUE_FRAMES * MAX_DATA_BYTES,
            MAX_CHANNEL_QUEUED_BYTES
        );
    }

    #[tokio::test]
    async fn input_reader_cancellation_unblocks_a_full_frame_queue() {
        let (mut input_writer, input_reader) = tokio::io::duplex(64);
        let (input_tx, mut input_rx) = mpsc::channel(1);
        let cancellation = CancellationToken::new();
        let reader = tokio::spawn(run_graphical_input_reader(
            input_reader,
            input_tx,
            cancellation.clone(),
        ));

        write_graphical_frame(
            &mut input_writer,
            &GraphicalFrame::OpenChannel { channel_id: 1 },
        )
        .await
        .unwrap();
        time::timeout(Duration::from_secs(1), async {
            while input_rx.len() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        write_graphical_frame(
            &mut input_writer,
            &GraphicalFrame::OpenChannel { channel_id: 2 },
        )
        .await
        .unwrap();
        tokio::task::yield_now().await;
        cancellation.cancel();
        time::timeout(Duration::from_secs(1), reader)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            input_rx.recv().await.unwrap().unwrap(),
            Some(GraphicalFrame::OpenChannel { channel_id: 1 })
        );
    }

    #[tokio::test]
    async fn channel_event_does_not_cancel_fragmented_outer_frame_read() {
        let (connection, daemon) = validated_pair();
        let (later_connection, _later_daemon) = validated_pair();
        let connections = Arc::new(Mutex::new(VecDeque::from([connection, later_connection])));
        let (mut client_input, relay_input) = tokio::io::duplex(1);
        let (relay_output, mut client_output) = tokio::io::duplex(32 * 1024);
        let relay = tokio::spawn(run_graphical_streams_with_connector(
            relay_input,
            relay_output,
            queued_connector(connections),
        ));

        write_graphical_frame(&mut client_input, &GraphicalFrame::Hello)
            .await
            .unwrap();
        assert_eq!(
            read_graphical_frame(&mut client_output).await.unwrap(),
            Some(GraphicalFrame::HelloAck)
        );
        write_graphical_frame(
            &mut client_input,
            &GraphicalFrame::OpenChannel { channel_id: 1 },
        )
        .await
        .unwrap();
        assert_eq!(
            read_graphical_frame(&mut client_output).await.unwrap(),
            Some(GraphicalFrame::ChannelOpened { channel_id: 1 })
        );

        let (mut encoded_writer, mut encoded_reader) = tokio::io::duplex(64);
        write_graphical_frame(
            &mut encoded_writer,
            &GraphicalFrame::OpenChannel { channel_id: 2 },
        )
        .await
        .unwrap();
        let mut encoded = [0_u8; 16];
        encoded_reader.read_exact(&mut encoded).await.unwrap();
        client_input.write_all(&encoded[..1]).await.unwrap();
        // Capacity one makes completion of the second write proof that the
        // reader consumed at least the first byte of this frame.
        client_input.write_all(&encoded[1..2]).await.unwrap();

        drop(daemon);
        assert_eq!(
            read_graphical_frame(&mut client_output).await.unwrap(),
            Some(GraphicalFrame::CloseChannel { channel_id: 1 })
        );
        client_input.write_all(&encoded[2..]).await.unwrap();
        assert_eq!(
            read_graphical_frame(&mut client_output).await.unwrap(),
            Some(GraphicalFrame::ChannelOpened { channel_id: 2 })
        );

        client_input.shutdown().await.unwrap();
        time::timeout(Duration::from_secs(2), relay)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn crossed_close_after_channel_local_eof_preserves_later_admission() {
        let (connection, mut daemon) = validated_pair();
        let (later_connection, _later_daemon) = validated_pair();
        let connections = Arc::new(Mutex::new(VecDeque::from([connection, later_connection])));
        let (mut client_input, relay_input) = tokio::io::duplex(32 * 1024);
        let (relay_output, mut client_output) = tokio::io::duplex(32 * 1024);
        let relay = tokio::spawn(run_graphical_streams_with_connector(
            relay_input,
            relay_output,
            queued_connector(connections),
        ));

        write_graphical_frame(&mut client_input, &GraphicalFrame::Hello)
            .await
            .unwrap();
        assert_eq!(
            read_graphical_frame(&mut client_output).await.unwrap(),
            Some(GraphicalFrame::HelloAck)
        );
        write_graphical_frame(
            &mut client_input,
            &GraphicalFrame::OpenChannel { channel_id: 1 },
        )
        .await
        .unwrap();
        assert_eq!(
            read_graphical_frame(&mut client_output).await.unwrap(),
            Some(GraphicalFrame::ChannelOpened { channel_id: 1 })
        );
        write_graphical_frame(
            &mut client_input,
            &GraphicalFrame::Data {
                channel_id: 1,
                bytes: b"request".to_vec(),
            },
        )
        .await
        .unwrap();
        let daemon_task = tokio::spawn(async move {
            let mut request = [0_u8; 7];
            daemon.read_exact(&mut request).await.unwrap();
            assert_eq!(&request, b"request");
            daemon.write_all(b"response").await.unwrap();
            daemon.shutdown().await.unwrap();
        });
        assert_eq!(
            read_graphical_frame(&mut client_output).await.unwrap(),
            Some(GraphicalFrame::Data {
                channel_id: 1,
                bytes: b"response".to_vec(),
            })
        );
        assert_eq!(
            read_graphical_frame(&mut client_output).await.unwrap(),
            Some(GraphicalFrame::CloseChannel { channel_id: 1 })
        );
        daemon_task.await.unwrap();

        // The local connection can finish concurrently with daemon EOF. Its
        // ordered half-close and close may therefore cross the relay's
        // channel-local close.
        write_graphical_frame(
            &mut client_input,
            &GraphicalFrame::HalfClose { channel_id: 1 },
        )
        .await
        .unwrap();
        write_graphical_frame(
            &mut client_input,
            &GraphicalFrame::CloseChannel { channel_id: 1 },
        )
        .await
        .unwrap();
        write_graphical_frame(
            &mut client_input,
            &GraphicalFrame::OpenChannel { channel_id: 2 },
        )
        .await
        .unwrap();
        assert_eq!(
            read_graphical_frame(&mut client_output).await.unwrap(),
            Some(GraphicalFrame::ChannelOpened { channel_id: 2 })
        );

        write_graphical_frame(
            &mut client_input,
            &GraphicalFrame::CloseChannel { channel_id: 3 },
        )
        .await
        .unwrap();
        assert_eq!(
            read_graphical_frame(&mut client_output).await.unwrap(),
            Some(GraphicalFrame::SessionError {
                reason: "close targeted an unknown logical channel".to_owned(),
            })
        );
        client_input.shutdown().await.unwrap();
        time::timeout(Duration::from_secs(2), relay)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn graphical_channel_limit_rejects_before_another_socket_open() {
        let opened = Arc::new(AtomicUsize::new(0));
        let peers = Arc::new(Mutex::new(Vec::new()));
        let connector = {
            let opened = opened.clone();
            let peers = peers.clone();
            move || {
                let (connection, peer) = validated_pair();
                peers.lock().unwrap().push(peer);
                opened.fetch_add(1, Ordering::SeqCst);
                std::future::ready(Ok(connection))
            }
        };
        let (mut client_input, relay_input) = tokio::io::duplex(64 * 1024);
        let (relay_output, mut client_output) = tokio::io::duplex(64 * 1024);
        let relay = tokio::spawn(run_graphical_streams_with_connector(
            relay_input,
            relay_output,
            connector,
        ));
        write_graphical_frame(&mut client_input, &GraphicalFrame::Hello)
            .await
            .unwrap();
        assert_eq!(
            read_graphical_frame(&mut client_output).await.unwrap(),
            Some(GraphicalFrame::HelloAck)
        );
        for channel_id in 1..=u32::try_from(MAX_LOGICAL_CHANNELS + 1).unwrap() {
            write_graphical_frame(
                &mut client_input,
                &GraphicalFrame::OpenChannel { channel_id },
            )
            .await
            .unwrap();
            let response = read_graphical_frame(&mut client_output).await.unwrap();
            if usize::try_from(channel_id).unwrap() <= MAX_LOGICAL_CHANNELS {
                assert_eq!(response, Some(GraphicalFrame::ChannelOpened { channel_id }));
            } else {
                assert!(matches!(
                    response,
                    Some(GraphicalFrame::ChannelRejected { channel_id: rejected, .. })
                        if rejected == channel_id
                ));
            }
        }
        assert_eq!(opened.load(Ordering::SeqCst), MAX_LOGICAL_CHANNELS);
        client_input.shutdown().await.unwrap();
        time::timeout(Duration::from_secs(2), relay)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn corrupt_outer_framing_closes_every_active_channel() {
        let (connection, mut daemon) = validated_pair();
        let connections = Arc::new(Mutex::new(VecDeque::from([connection])));
        let (mut client_input, relay_input) = tokio::io::duplex(32 * 1024);
        let (relay_output, mut client_output) = tokio::io::duplex(32 * 1024);
        let relay = tokio::spawn(run_graphical_streams_with_connector(
            relay_input,
            relay_output,
            queued_connector(connections),
        ));
        write_graphical_frame(&mut client_input, &GraphicalFrame::Hello)
            .await
            .unwrap();
        assert_eq!(
            read_graphical_frame(&mut client_output).await.unwrap(),
            Some(GraphicalFrame::HelloAck)
        );
        write_graphical_frame(
            &mut client_input,
            &GraphicalFrame::OpenChannel { channel_id: 1 },
        )
        .await
        .unwrap();
        assert_eq!(
            read_graphical_frame(&mut client_output).await.unwrap(),
            Some(GraphicalFrame::ChannelOpened { channel_id: 1 })
        );
        client_input.write_all(&[0_u8; 16]).await.unwrap();
        assert!(matches!(
            read_graphical_frame(&mut client_output).await.unwrap(),
            Some(GraphicalFrame::SessionError { reason }) if reason.contains("magic")
        ));
        time::timeout(Duration::from_secs(2), relay)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let mut byte = [0_u8; 1];
        assert_eq!(daemon.read(&mut byte).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn validated_daemon_death_fails_the_graphical_session() {
        let mut child = std::process::Command::new("/usr/bin/sleep")
            .arg("0.1")
            .spawn()
            .unwrap();
        let pid = rustix::process::Pid::from_raw(i32::try_from(child.id()).unwrap()).unwrap();
        let peer_pidfd =
            rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty()).unwrap();
        let (stream, _daemon) = UnixStream::pair().unwrap();
        let connection = ValidatedConnection { stream, peer_pidfd };
        let connections = Arc::new(Mutex::new(VecDeque::from([connection])));
        let (mut client_input, relay_input) = tokio::io::duplex(32 * 1024);
        let (relay_output, mut client_output) = tokio::io::duplex(32 * 1024);
        let relay = tokio::spawn(run_graphical_streams_with_connector(
            relay_input,
            relay_output,
            queued_connector(connections),
        ));
        write_graphical_frame(&mut client_input, &GraphicalFrame::Hello)
            .await
            .unwrap();
        assert_eq!(
            read_graphical_frame(&mut client_output).await.unwrap(),
            Some(GraphicalFrame::HelloAck)
        );
        write_graphical_frame(
            &mut client_input,
            &GraphicalFrame::OpenChannel { channel_id: 1 },
        )
        .await
        .unwrap();
        assert_eq!(
            read_graphical_frame(&mut client_output).await.unwrap(),
            Some(GraphicalFrame::ChannelOpened { channel_id: 1 })
        );
        assert!(matches!(
            time::timeout(Duration::from_secs(2), read_graphical_frame(&mut client_output))
                .await
                .unwrap()
                .unwrap(),
            Some(GraphicalFrame::SessionError { reason }) if reason.contains("splinterd process exited")
        ));
        time::timeout(Duration::from_secs(2), relay)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        child.wait().unwrap();
    }

    #[tokio::test]
    async fn output_heavy_channel_does_not_starve_small_channel() {
        let (first, mut first_daemon) = validated_pair();
        let (second, mut second_daemon) = validated_pair();
        let connections = Arc::new(Mutex::new(VecDeque::from([first, second])));
        let (mut client_input, relay_input) = tokio::io::duplex(64 * 1024);
        let (relay_output, mut client_output) = tokio::io::duplex(64 * 1024);
        let relay = tokio::spawn(run_graphical_streams_with_connector(
            relay_input,
            relay_output,
            queued_connector(connections),
        ));
        write_graphical_frame(&mut client_input, &GraphicalFrame::Hello)
            .await
            .unwrap();
        assert_eq!(
            read_graphical_frame(&mut client_output).await.unwrap(),
            Some(GraphicalFrame::HelloAck)
        );
        for channel_id in [1, 2] {
            write_graphical_frame(
                &mut client_input,
                &GraphicalFrame::OpenChannel { channel_id },
            )
            .await
            .unwrap();
            assert_eq!(
                read_graphical_frame(&mut client_output).await.unwrap(),
                Some(GraphicalFrame::ChannelOpened { channel_id })
            );
        }
        let heavy = tokio::spawn(async move {
            for _ in 0..(CHANNEL_QUEUE_FRAMES * 2) {
                if first_daemon
                    .write_all(&vec![0x55; MAX_DATA_BYTES])
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        second_daemon.write_all(b"small").await.unwrap();
        let mut preceding = 0_usize;
        loop {
            match read_graphical_frame(&mut client_output).await.unwrap() {
                Some(GraphicalFrame::Data {
                    channel_id: 2,
                    bytes,
                }) => {
                    assert_eq!(bytes, b"small");
                    break;
                }
                Some(GraphicalFrame::Data { channel_id: 1, .. }) => preceding += 1,
                other => panic!("unexpected fairness frame: {other:?}"),
            }
            assert!(
                preceding <= CHANNEL_QUEUE_FRAMES + 1,
                "small channel was starved beyond the aggregate queue bound"
            );
        }
        client_input.shutdown().await.unwrap();
        time::timeout(Duration::from_secs(2), relay)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let mut closed = [0_u8; 1];
        assert_eq!(
            time::timeout(Duration::from_secs(1), second_daemon.read(&mut closed))
                .await
                .unwrap()
                .unwrap(),
            0,
            "session EOF did not close every active daemon channel"
        );
        let _ = heavy.await;
    }
}
