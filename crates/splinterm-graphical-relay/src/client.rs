use std::{
    collections::HashMap,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context as TaskContext, Poll},
};

use anyhow::{Result, bail};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf},
    sync::{mpsc, oneshot},
};
use tokio_util::sync::CancellationToken;

use crate::{
    ChannelIdAllocator, FairDataChannel, FairDataSender, Frame, MAX_CHANNEL_QUEUED_BYTES,
    MAX_DATA_BYTES, MAX_LOGICAL_CHANNELS, fair_data_queue, read_frame, write_data_frame,
    write_frame,
};

const CHANNEL_QUEUE_FRAMES: usize = MAX_CHANNEL_QUEUED_BYTES / MAX_DATA_BYTES - 1;
const CONTROL_QUEUE_FRAMES: usize = MAX_LOGICAL_CHANNELS * 2 + 16;

#[derive(Debug)]
enum IncomingCommand {
    Data(Vec<u8>),
    HalfClose,
    Close,
}

#[derive(Debug)]
struct Route {
    opened: Option<oneshot::Sender<Result<(), String>>>,
    incoming: mpsc::Sender<IncomingCommand>,
}

#[derive(Debug)]
struct ClientState {
    control_outbound: mpsc::Sender<Frame>,
    data_outbound: FairDataSender,
    routes: Mutex<HashMap<u32, Route>>,
    allocator: Mutex<ChannelIdAllocator>,
    failure: Mutex<Option<String>>,
    cancellation: CancellationToken,
}

impl ClientState {
    fn failure(&self) -> Option<String> {
        self.failure.lock().ok().and_then(|failure| failure.clone())
    }

    fn fail(&self, reason: impl Into<String>) {
        let reason = reason.into();
        let Ok(mut failure) = self.failure.lock() else {
            self.cancellation.cancel();
            return;
        };
        if failure.is_some() {
            return;
        }
        *failure = Some(reason.clone());
        self.cancellation.cancel();
        drop(failure);
        if let Ok(mut routes) = self.routes.lock() {
            for (_, mut route) in routes.drain() {
                if let Some(opened) = route.opened.take() {
                    let _ = opened.send(Err(reason.clone()));
                }
                let _ = route.incoming.try_send(IncomingCommand::Close);
            }
        }
    }

    fn close_channel(&self, channel_id: u32) {
        if let Ok(mut routes) = self.routes.lock()
            && let Some(mut route) = routes.remove(&channel_id)
        {
            if let Some(opened) = route.opened.take() {
                let _ = opened.send(Err(
                    "graphical relay channel closed while opening".to_owned()
                ));
            }
            let _ = route.incoming.try_send(IncomingCommand::Close);
        }
    }
}

#[derive(Debug)]
struct OpeningGuard {
    state: Arc<ClientState>,
    channel_id: u32,
    armed: bool,
}

impl OpeningGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for OpeningGuard {
    fn drop(&mut self) {
        if self.armed {
            self.state.fail(format!(
                "graphical relay channel {} admission was cancelled",
                self.channel_id
            ));
        }
    }
}

#[derive(Debug)]
struct ClientHandle {
    state: Arc<ClientState>,
}

impl Drop for ClientHandle {
    fn drop(&mut self) {
        self.state
            .fail("graphical relay client lifetime ended".to_owned());
    }
}

/// A negotiated client-side graphical relay multiplexer.
#[derive(Clone, Debug)]
pub struct ClientMultiplexer {
    handle: Arc<ClientHandle>,
}

impl ClientMultiplexer {
    /// Negotiates the exact outer protocol and starts bounded routing tasks.
    ///
    /// # Errors
    ///
    /// Returns an error for transport failure or an invalid/incompatible relay
    /// handshake.
    pub async fn negotiate<R, W>(mut reader: R, mut writer: W) -> Result<Self>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        write_frame(&mut writer, &Frame::Hello).await?;
        match read_frame(&mut reader).await? {
            Some(Frame::HelloAck) => {}
            Some(Frame::SessionError { reason }) => bail!("remote graphical relay: {reason}"),
            Some(_) => bail!("remote graphical relay sent an invalid handshake"),
            None => bail!("remote graphical relay closed before acknowledging the handshake"),
        }

        let (control_outbound, mut control_rx) = mpsc::channel::<Frame>(CONTROL_QUEUE_FRAMES);
        let (data_outbound, mut data_rx) = fair_data_queue();
        let state = Arc::new(ClientState {
            control_outbound,
            data_outbound,
            routes: Mutex::new(HashMap::new()),
            allocator: Mutex::new(ChannelIdAllocator::default()),
            failure: Mutex::new(None),
            cancellation: CancellationToken::new(),
        });

        let writer_state = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    () = writer_state.cancellation.cancelled() => {
                        let _ = writer.shutdown().await;
                        return;
                    },
                    frame = control_rx.recv() => if let Some(frame) = frame
                        && let Err(error) = write_frame(&mut writer, &frame).await
                    {
                        writer_state.fail(format!("graphical relay write failed: {error}"));
                        return;
                    },
                    data = data_rx.recv() => if let Some(data) = data
                        && let Err(error) = write_data_frame(
                            &mut writer,
                            data.channel_id(),
                            data.bytes(),
                        ).await
                    {
                        writer_state.fail(format!("graphical relay write failed: {error}"));
                        return;
                    },
                }
            }
        });

        let reader_state = state.clone();
        tokio::spawn(async move {
            loop {
                let frame = tokio::select! {
                    () = reader_state.cancellation.cancelled() => return,
                    frame = read_frame(&mut reader) => frame,
                };
                let frame = match frame {
                    Ok(Some(frame)) => frame,
                    Ok(None) => {
                        reader_state.fail("remote graphical relay closed the session");
                        return;
                    }
                    Err(error) => {
                        reader_state.fail(format!("graphical relay read failed: {error}"));
                        return;
                    }
                };
                if let Err(reason) = dispatch_frame(&reader_state, frame) {
                    reader_state.fail(reason);
                    return;
                }
            }
        });

        Ok(Self {
            handle: Arc::new(ClientHandle { state }),
        })
    }

    /// Opens one independently bounded logical byte channel.
    ///
    /// # Errors
    ///
    /// Returns an error when the session has failed, the identity space is
    /// exhausted, or the remote relay rejects the channel.
    pub async fn open_channel(&self) -> Result<LogicalChannel> {
        let state = &self.handle.state;
        if let Some(reason) = state.failure() {
            bail!(reason);
        }
        let channel_id = state
            .allocator
            .lock()
            .map_err(|_| anyhow::anyhow!("graphical relay channel allocator is poisoned"))?
            .allocate()?;
        let (application, bridge) = tokio::io::duplex(MAX_DATA_BYTES);
        let (bridge_reader, bridge_writer) = tokio::io::split(bridge);
        let (incoming, incoming_rx) = mpsc::channel(CHANNEL_QUEUE_FRAMES);
        let (opened_tx, opened_rx) = oneshot::channel();
        state
            .routes
            .lock()
            .map_err(|_| anyhow::anyhow!("graphical relay route table is poisoned"))?
            .insert(
                channel_id,
                Route {
                    opened: Some(opened_tx),
                    incoming,
                },
            );
        let mut opening = OpeningGuard {
            state: state.clone(),
            channel_id,
            armed: true,
        };
        tokio::spawn(run_incoming_channel(
            channel_id,
            bridge_writer,
            incoming_rx,
            state.clone(),
        ));
        if state
            .control_outbound
            .send(Frame::OpenChannel { channel_id })
            .await
            .is_err()
        {
            state.close_channel(channel_id);
            opening.disarm();
            bail!("graphical relay writer is unavailable");
        }
        let admission = opened_rx.await;
        opening.disarm();
        match admission {
            Ok(Ok(())) => {}
            Ok(Err(reason)) => bail!(reason),
            Err(_) => bail!(
                "{}",
                state
                    .failure()
                    .unwrap_or_else(|| "graphical relay channel admission was cancelled".to_owned())
            ),
        }
        let data_outbound = state.data_outbound.channel(channel_id);
        let outgoing_data = data_outbound.clone();
        let outgoing_state = state.clone();
        let (outgoing_finished, outgoing_completion) = oneshot::channel();
        tokio::spawn(async move {
            run_outgoing_channel(channel_id, bridge_reader, outgoing_data, outgoing_state).await;
            let _ = outgoing_finished.send(());
        });
        Ok(LogicalChannel {
            stream: application,
            guard: Arc::new(ChannelGuard {
                channel_id,
                handle: self.handle.clone(),
                data_outbound,
                outgoing_completion: Some(outgoing_completion),
            }),
        })
    }

    /// Returns the first terminal session failure, if any.
    #[must_use]
    pub fn terminal_failure(&self) -> Option<String> {
        self.handle.state.failure()
    }
}

fn dispatch_frame(state: &Arc<ClientState>, frame: Frame) -> std::result::Result<(), String> {
    match frame {
        Frame::ChannelOpened { channel_id } => {
            let mut routes = state
                .routes
                .lock()
                .map_err(|_| "graphical relay route table is poisoned".to_owned())?;
            let route = routes
                .get_mut(&channel_id)
                .ok_or_else(|| "channel acknowledgement targeted an unknown channel".to_owned())?;
            let opened = route
                .opened
                .take()
                .ok_or_else(|| "channel was acknowledged more than once".to_owned())?;
            let _ = opened.send(Ok(()));
        }
        Frame::ChannelRejected { channel_id, reason } => {
            let mut route = state
                .routes
                .lock()
                .map_err(|_| "graphical relay route table is poisoned".to_owned())?
                .remove(&channel_id)
                .ok_or_else(|| "channel rejection targeted an unknown channel".to_owned())?;
            let opened = route
                .opened
                .take()
                .ok_or_else(|| "opened channel was rejected".to_owned())?;
            let _ = opened.send(Err(format!(
                "remote graphical relay rejected channel: {reason}"
            )));
            let _ = route.incoming.try_send(IncomingCommand::Close);
        }
        Frame::Data { channel_id, bytes } => {
            send_incoming(state, channel_id, IncomingCommand::Data(bytes), "data")?;
        }
        Frame::HalfClose { channel_id } => {
            send_shutdown_incoming(state, channel_id, IncomingCommand::HalfClose, "half-close")?;
        }
        Frame::CloseChannel { channel_id } => {
            close_remote_channel(state, channel_id)?;
        }
        Frame::SessionError { reason } => return Err(format!("remote graphical relay: {reason}")),
        Frame::Hello | Frame::HelloAck | Frame::OpenChannel { .. } => {
            return Err("remote graphical relay sent an invalid client-side frame".to_owned());
        }
    }
    Ok(())
}

fn send_incoming(
    state: &ClientState,
    channel_id: u32,
    command: IncomingCommand,
    label: &str,
) -> std::result::Result<(), String> {
    let routes = state
        .routes
        .lock()
        .map_err(|_| "graphical relay route table is poisoned".to_owned())?;
    let route = routes
        .get(&channel_id)
        .ok_or_else(|| format!("{label} targeted an unknown graphical relay channel"))?;
    if route.opened.is_some() {
        return Err(format!(
            "{label} preceded graphical relay channel admission"
        ));
    }
    route
        .incoming
        .try_send(command)
        .map_err(|_| "graphical relay channel queue exceeded its byte bound".to_owned())
}

fn send_shutdown_incoming(
    state: &ClientState,
    channel_id: u32,
    command: IncomingCommand,
    label: &str,
) -> std::result::Result<(), String> {
    let routes = state
        .routes
        .lock()
        .map_err(|_| "graphical relay route table is poisoned".to_owned())?;
    let Some(route) = routes.get(&channel_id) else {
        let last_issued = state
            .allocator
            .lock()
            .map_err(|_| "graphical relay channel allocator is poisoned".to_owned())?
            .last_issued();
        return if channel_id <= last_issued {
            Ok(())
        } else {
            Err(format!(
                "{label} targeted a never-issued graphical relay channel"
            ))
        };
    };
    if route.opened.is_some() {
        return Err(format!(
            "{label} preceded graphical relay channel admission"
        ));
    }
    route
        .incoming
        .try_send(command)
        .map_err(|_| "graphical relay channel queue exceeded its byte bound".to_owned())
}

fn close_remote_channel(state: &ClientState, channel_id: u32) -> std::result::Result<(), String> {
    let route = state
        .routes
        .lock()
        .map_err(|_| "graphical relay route table is poisoned".to_owned())?
        .remove(&channel_id);
    let Some(mut route) = route else {
        let last_issued = state
            .allocator
            .lock()
            .map_err(|_| "graphical relay channel allocator is poisoned".to_owned())?
            .last_issued();
        return if channel_id <= last_issued {
            Ok(())
        } else {
            Err("close targeted a never-issued graphical relay channel".to_owned())
        };
    };
    if let Some(opened) = route.opened.take() {
        let _ = opened.send(Err(
            "graphical relay channel closed while opening".to_owned()
        ));
    }
    let _ = route.incoming.try_send(IncomingCommand::Close);
    Ok(())
}

async fn run_incoming_channel(
    channel_id: u32,
    mut writer: tokio::io::WriteHalf<DuplexStream>,
    mut incoming: mpsc::Receiver<IncomingCommand>,
    state: Arc<ClientState>,
) {
    let mut half_closed = false;
    loop {
        let command = tokio::select! {
            () = state.cancellation.cancelled() => break,
            command = incoming.recv() => command,
        };
        match command {
            Some(IncomingCommand::Data(bytes)) if !half_closed => {
                if writer.write_all(&bytes).await.is_err() {
                    let _ = state
                        .control_outbound
                        .try_send(Frame::CloseChannel { channel_id });
                    state.close_channel(channel_id);
                    break;
                }
            }
            Some(IncomingCommand::HalfClose) if !half_closed => {
                half_closed = true;
                let _ = writer.shutdown().await;
            }
            Some(IncomingCommand::Close) | None => {
                let _ = writer.shutdown().await;
                break;
            }
            Some(IncomingCommand::Data(_) | IncomingCommand::HalfClose) => {
                state.fail("remote graphical relay sent data after channel half-close");
                break;
            }
        }
    }
}

async fn run_outgoing_channel(
    channel_id: u32,
    mut reader: tokio::io::ReadHalf<DuplexStream>,
    data_outbound: FairDataChannel,
    state: Arc<ClientState>,
) {
    let mut buffer = vec![0_u8; MAX_DATA_BYTES];
    loop {
        let permit = tokio::select! {
            () = state.cancellation.cancelled() => return,
            permit = data_outbound.reserve() => match permit {
                Ok(permit) => permit,
                Err(error) => {
                    state.fail(format!("graphical relay byte reservation failed: {error}"));
                    return;
                }
            },
        };
        let read = tokio::select! {
            () = state.cancellation.cancelled() => return,
            read = reader.read(&mut buffer) => read,
        };
        match read {
            Ok(0) => {
                drop(permit);
                let _ = data_outbound.drain().await;
                let _ = state
                    .control_outbound
                    .send(Frame::HalfClose { channel_id })
                    .await;
                return;
            }
            Ok(count) => {
                if let Err(error) = permit.send(buffer[..count].to_vec()) {
                    state.fail(format!("graphical relay data queue failed: {error}"));
                    return;
                }
            }
            Err(error) => {
                drop(permit);
                state.fail(format!(
                    "logical graphical relay channel read failed: {error}"
                ));
                return;
            }
        }
    }
}

#[derive(Debug)]
struct ChannelGuard {
    channel_id: u32,
    handle: Arc<ClientHandle>,
    data_outbound: FairDataChannel,
    outgoing_completion: Option<oneshot::Receiver<()>>,
}

impl Drop for ChannelGuard {
    fn drop(&mut self) {
        let state = &self.handle.state;
        state.close_channel(self.channel_id);
        let channel_id = self.channel_id;
        let handle = self.handle.clone();
        let data_outbound = self.data_outbound.clone();
        let outgoing_completion = self.outgoing_completion.take();
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            state.fail("graphical relay channel closed outside an async runtime");
            return;
        };
        runtime.spawn(async move {
            if let Some(completion) = outgoing_completion {
                let _ = completion.await;
            }
            let _ = data_outbound.drain().await;
            if handle
                .state
                .control_outbound
                .send(Frame::CloseChannel { channel_id })
                .await
                .is_err()
            {
                handle
                    .state
                    .fail("graphical relay control output queue closed");
            }
        });
    }
}

/// One logical full-duplex byte channel over a graphical relay session.
#[derive(Debug)]
pub struct LogicalChannel {
    stream: DuplexStream,
    #[allow(dead_code, reason = "the guard owns channel-close signaling")]
    guard: Arc<ChannelGuard>,
}

impl LogicalChannel {
    /// Returns the monotonically allocated session-local channel identity.
    #[must_use]
    pub fn channel_id(&self) -> u32 {
        self.guard.channel_id
    }
}

impl AsyncRead for LogicalChannel {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(context, buffer)
    }
}

impl AsyncWrite for LogicalChannel {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(context, buffer)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(context)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::{io::AsyncReadExt as _, time};

    use super::*;

    #[tokio::test]
    async fn one_negotiation_routes_multiple_independent_channels() {
        let (client, server) = tokio::io::duplex(128 * 1024);
        let (client_reader, client_writer) = tokio::io::split(client);
        let (mut server_reader, mut server_writer) = tokio::io::split(server);
        let (done_tx, done_rx) = oneshot::channel();
        let server_task = tokio::spawn(async move {
            assert_eq!(
                read_frame(&mut server_reader).await.unwrap(),
                Some(Frame::Hello)
            );
            write_frame(&mut server_writer, &Frame::HelloAck)
                .await
                .unwrap();
            for expected_id in [1, 2] {
                assert_eq!(
                    read_frame(&mut server_reader).await.unwrap(),
                    Some(Frame::OpenChannel {
                        channel_id: expected_id
                    })
                );
                write_frame(
                    &mut server_writer,
                    &Frame::ChannelOpened {
                        channel_id: expected_id,
                    },
                )
                .await
                .unwrap();
            }
            let mut received = HashMap::new();
            while received.len() < 2 {
                let Some(Frame::Data { channel_id, bytes }) =
                    read_frame(&mut server_reader).await.unwrap()
                else {
                    panic!("expected channel data");
                };
                received.insert(channel_id, bytes.clone());
                write_frame(&mut server_writer, &Frame::Data { channel_id, bytes })
                    .await
                    .unwrap();
            }
            assert_eq!(received.get(&1).map(Vec::as_slice), Some(&b"first"[..]));
            assert_eq!(received.get(&2).map(Vec::as_slice), Some(&b"second"[..]));
            let _ = done_rx.await;
        });

        let client = ClientMultiplexer::negotiate(client_reader, client_writer)
            .await
            .unwrap();
        let mut first = client.open_channel().await.unwrap();
        let mut second = client.open_channel().await.unwrap();
        first.write_all(b"first").await.unwrap();
        second.write_all(b"second").await.unwrap();
        let mut first_echo = [0_u8; 5];
        let mut second_echo = [0_u8; 6];
        first.read_exact(&mut first_echo).await.unwrap();
        second.read_exact(&mut second_echo).await.unwrap();
        assert_eq!(&first_echo, b"first");
        assert_eq!(&second_echo, b"second");
        done_tx.send(()).unwrap();
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn crossed_server_shutdown_after_drop_preserves_later_admission() {
        let (client, server) = tokio::io::duplex(32 * 1024);
        let (client_reader, client_writer) = tokio::io::split(client);
        let (mut server_reader, mut server_writer) = tokio::io::split(server);
        let server_task = tokio::spawn(async move {
            assert_eq!(
                read_frame(&mut server_reader).await.unwrap(),
                Some(Frame::Hello)
            );
            write_frame(&mut server_writer, &Frame::HelloAck)
                .await
                .unwrap();
            for expected_id in [1, 2] {
                loop {
                    match read_frame(&mut server_reader).await.unwrap() {
                        Some(Frame::OpenChannel { channel_id }) => {
                            assert_eq!(channel_id, expected_id);
                            write_frame(&mut server_writer, &Frame::ChannelOpened { channel_id })
                                .await
                                .unwrap();
                            break;
                        }
                        Some(
                            Frame::HalfClose { channel_id } | Frame::CloseChannel { channel_id },
                        ) => {
                            assert_eq!(channel_id, 1);
                        }
                        frame => panic!("unexpected client frame: {frame:?}"),
                    }
                }
            }
        });

        let client = ClientMultiplexer::negotiate(client_reader, client_writer)
            .await
            .unwrap();
        drop(client.open_channel().await.unwrap());
        assert!(dispatch_frame(&client.handle.state, Frame::HalfClose { channel_id: 1 }).is_ok());
        assert!(
            dispatch_frame(&client.handle.state, Frame::CloseChannel { channel_id: 1 }).is_ok()
        );
        let second = client.open_channel().await.unwrap();
        assert_eq!(second.channel_id(), 2);
        assert!(dispatch_frame(&client.handle.state, Frame::HalfClose { channel_id: 3 }).is_err());
        assert!(
            dispatch_frame(&client.handle.state, Frame::CloseChannel { channel_id: 3 }).is_err()
        );
        time::timeout(Duration::from_secs(1), server_task)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn saturated_heavy_channel_cannot_starve_later_small_channel() {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (client_reader, client_writer) = tokio::io::split(client);
        let (mut server_reader, mut server_writer) = tokio::io::split(server);
        let (start_tx, start_rx) = oneshot::channel();
        let server_task = tokio::spawn(async move {
            assert_eq!(
                read_frame(&mut server_reader).await.unwrap(),
                Some(Frame::Hello)
            );
            write_frame(&mut server_writer, &Frame::HelloAck)
                .await
                .unwrap();
            for expected_id in [1, 2] {
                assert_eq!(
                    read_frame(&mut server_reader).await.unwrap(),
                    Some(Frame::OpenChannel {
                        channel_id: expected_id
                    })
                );
                write_frame(
                    &mut server_writer,
                    &Frame::ChannelOpened {
                        channel_id: expected_id,
                    },
                )
                .await
                .unwrap();
            }
            start_rx.await.unwrap();
            let mut heavy_before_small = 0_usize;
            loop {
                match read_frame(&mut server_reader).await.unwrap() {
                    Some(Frame::Data {
                        channel_id: 2,
                        bytes,
                    }) => {
                        assert_eq!(bytes, b"small");
                        break;
                    }
                    Some(Frame::Data { channel_id: 1, .. }) => heavy_before_small += 1,
                    other => panic!("unexpected fairness frame: {other:?}"),
                }
                assert!(
                    heavy_before_small <= 10,
                    "small channel was starved behind unbounded heavy data"
                );
            }
        });

        let client = ClientMultiplexer::negotiate(client_reader, client_writer)
            .await
            .unwrap();
        let mut heavy = client.open_channel().await.unwrap();
        let mut small = client.open_channel().await.unwrap();
        let heavy_task = tokio::spawn(async move {
            let _ = heavy.write_all(&vec![0x55; MAX_DATA_BYTES * 12]).await;
        });
        small.write_all(b"small").await.unwrap();
        time::sleep(Duration::from_millis(20)).await;
        start_tx.send(()).unwrap();
        server_task.await.unwrap();
        drop(client);
        heavy_task.abort();
        let _ = heavy_task.await;
    }

    #[tokio::test]
    async fn cancelled_channel_admission_fails_the_session_and_closes_transport() {
        let (client, server) = tokio::io::duplex(32 * 1024);
        let (client_reader, client_writer) = tokio::io::split(client);
        let (mut server_reader, mut server_writer) = tokio::io::split(server);
        let server_task = tokio::spawn(async move {
            assert_eq!(
                read_frame(&mut server_reader).await.unwrap(),
                Some(Frame::Hello)
            );
            write_frame(&mut server_writer, &Frame::HelloAck)
                .await
                .unwrap();
            assert_eq!(
                read_frame(&mut server_reader).await.unwrap(),
                Some(Frame::OpenChannel { channel_id: 1 })
            );
            assert_eq!(read_frame(&mut server_reader).await.unwrap(), None);
        });
        let client = ClientMultiplexer::negotiate(client_reader, client_writer)
            .await
            .unwrap();

        assert!(
            time::timeout(Duration::from_millis(20), client.open_channel())
                .await
                .is_err()
        );
        assert_eq!(
            client.terminal_failure().as_deref(),
            Some("graphical relay channel 1 admission was cancelled")
        );
        assert!(client.open_channel().await.is_err());
        time::timeout(Duration::from_secs(1), server_task)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn unknown_channel_frame_fails_session_and_future_admission() {
        let (client, server) = tokio::io::duplex(32 * 1024);
        let (client_reader, client_writer) = tokio::io::split(client);
        let (mut server_reader, mut server_writer) = tokio::io::split(server);
        let server_task = tokio::spawn(async move {
            assert_eq!(
                read_frame(&mut server_reader).await.unwrap(),
                Some(Frame::Hello)
            );
            write_frame(&mut server_writer, &Frame::HelloAck)
                .await
                .unwrap();
            write_frame(
                &mut server_writer,
                &Frame::Data {
                    channel_id: 99,
                    bytes: b"invalid".to_vec(),
                },
            )
            .await
            .unwrap();
        });
        let client = ClientMultiplexer::negotiate(client_reader, client_writer)
            .await
            .unwrap();
        time::timeout(Duration::from_secs(1), async {
            while client.terminal_failure().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(client.open_channel().await.is_err());
        server_task.await.unwrap();
    }
}
