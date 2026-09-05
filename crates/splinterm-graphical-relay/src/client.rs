use std::{
    collections::HashMap,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context as TaskContext, Poll},
};

use anyhow::{Result, bail};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf},
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot},
};
use tokio_util::sync::CancellationToken;

use crate::{
    ChannelIdAllocator, FairDataChannel, FairDataSender, Frame, MAX_DATA_BYTES,
    MAX_INCOMING_CHANNEL_QUEUED_BYTES, MAX_LOGICAL_CHANNELS, MAX_SESSION_QUEUED_BYTES,
    fair_data_queue, read_frame, write_data_frame, write_frame,
};

const CHANNEL_QUEUE_FRAMES: usize = MAX_INCOMING_CHANNEL_QUEUED_BYTES / MAX_DATA_BYTES + 2;
const CONTROL_QUEUE_FRAMES: usize = MAX_LOGICAL_CHANNELS * 2 + 16;

#[derive(Debug)]
struct IncomingData {
    bytes: Vec<u8>,
    _channel_bytes: OwnedSemaphorePermit,
    _session_bytes: OwnedSemaphorePermit,
}

#[derive(Debug)]
enum IncomingCommand {
    Data(IncomingData),
    HalfClose,
}

#[derive(Debug)]
struct Route {
    opened: Option<oneshot::Sender<Result<(), String>>>,
    incoming: mpsc::Sender<IncomingCommand>,
    incoming_bytes: Arc<Semaphore>,
    outgoing_cancellation: CancellationToken,
    incoming_cancellation: CancellationToken,
    data_outbound: FairDataChannel,
}

impl Drop for Route {
    fn drop(&mut self) {
        // Removing a remote route is ordered incoming EOF, not cancellation:
        // dropping its sender drains queued bytes under the retained channel slot.
        self.outgoing_cancellation.cancel();
        self.data_outbound.discard();
    }
}

#[derive(Debug)]
struct ClientState {
    control_outbound: mpsc::Sender<Frame>,
    data_outbound: FairDataSender,
    incoming_session_bytes: Arc<Semaphore>,
    channel_slots: Arc<Semaphore>,
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
        self.channel_slots.close();
        drop(failure);
        if let Ok(mut routes) = self.routes.lock() {
            for (_, mut route) in routes.drain() {
                if let Some(opened) = route.opened.take() {
                    let _ = opened.send(Err(reason.clone()));
                }
            }
        }
    }

    fn close_channel(&self, channel_id: u32) {
        if let Ok(mut routes) = self.routes.lock()
            && let Some(mut route) = routes.remove(&channel_id)
        {
            route.incoming_cancellation.cancel();
            if let Some(opened) = route.opened.take() {
                let _ = opened.send(Err(
                    "graphical relay channel closed while opening".to_owned()
                ));
            }
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
            incoming_session_bytes: Arc::new(Semaphore::new(MAX_SESSION_QUEUED_BYTES)),
            channel_slots: Arc::new(Semaphore::new(MAX_LOGICAL_CHANNELS)),
            routes: Mutex::new(HashMap::new()),
            allocator: Mutex::new(ChannelIdAllocator::default()),
            failure: Mutex::new(None),
            cancellation: CancellationToken::new(),
        });

        let writer_state = state.clone();
        tokio::spawn(async move {
            // Cancellation covers physical writes too, not only queue selection.
            tokio::select! {
                biased;
                () = writer_state.cancellation.cancelled() => {}
                () = async {
                    loop {
                        let result = tokio::select! {
                            biased;
                            frame = control_rx.recv() => match frame {
                                Some(frame) => write_frame(&mut writer, &frame).await,
                                None => return,
                            },
                            data = data_rx.recv() => match data {
                                Some(data) => write_data_frame(
                                    &mut writer, data.channel_id(), data.bytes(),
                                ).await,
                                None => return,
                            },
                        };
                        if let Err(error) = result {
                            writer_state.fail(format!("graphical relay write failed: {error}"));
                            return;
                        }
                    }
                } => {}
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
    /// exhausted, the local channel limit is reached (including pending incoming
    /// drains), or the remote relay rejects the channel.
    pub async fn open_channel(&self) -> Result<LogicalChannel> {
        let state = &self.handle.state;
        if let Some(reason) = state.failure() {
            bail!(reason);
        }
        // Remote EOF may leave ordered incoming data waiting on a slow consumer.
        // Admission remains charged until both that pump and the application end.
        let channel_slot = Arc::new(
            state
                .channel_slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| anyhow::anyhow!("graphical relay logical channel limit reached"))?,
        );
        let channel_id = state
            .allocator
            .lock()
            .map_err(|_| anyhow::anyhow!("graphical relay channel allocator is poisoned"))?
            .allocate()?;
        let (application, bridge) = tokio::io::duplex(MAX_DATA_BYTES);
        let (bridge_reader, bridge_writer) = tokio::io::split(bridge);
        let (incoming, incoming_rx) = mpsc::channel(CHANNEL_QUEUE_FRAMES);
        let (opened_tx, opened_rx) = oneshot::channel();
        let outgoing_cancellation = state.cancellation.child_token();
        let incoming_cancellation = state.cancellation.child_token();
        let data_outbound = state.data_outbound.channel(channel_id);
        state
            .routes
            .lock()
            .map_err(|_| anyhow::anyhow!("graphical relay route table is poisoned"))?
            .insert(
                channel_id,
                Route {
                    opened: Some(opened_tx),
                    incoming,
                    incoming_bytes: Arc::new(Semaphore::new(MAX_INCOMING_CHANNEL_QUEUED_BYTES)),
                    outgoing_cancellation: outgoing_cancellation.clone(),
                    incoming_cancellation: incoming_cancellation.clone(),
                    data_outbound: data_outbound.clone(),
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
            incoming_cancellation.clone(),
            channel_slot.clone(),
        ));
        let admission = tokio::select! {
            biased;
            // Honor an acknowledgement already delivered before transport EOF.
            admission = async {
                if state.control_outbound.send(Frame::OpenChannel { channel_id }).await.is_err() {
                    state.close_channel(channel_id);
                }
                opened_rx.await
            } => admission,
            () = state.cancellation.cancelled() => {
                state.close_channel(channel_id);
                opening.disarm();
                bail!(state.failure().unwrap_or_else(|| "graphical relay session cancelled".to_owned()));
            }
        };
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
        tokio::spawn(run_outgoing_channel(
            channel_id,
            bridge_reader,
            data_outbound.clone(),
            state.clone(),
            outgoing_cancellation,
            channel_slot.clone(),
        ));
        Ok(LogicalChannel {
            stream: application,
            guard: Arc::new(ChannelGuard {
                channel_id,
                handle: self.handle.clone(),
                data_outbound,
                incoming_cancellation,
                _channel_slot: channel_slot,
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
        }
        Frame::Data { channel_id, bytes } => {
            send_incoming_data(state, channel_id, bytes)?;
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

fn send_incoming_data(
    state: &ClientState,
    channel_id: u32,
    bytes: Vec<u8>,
) -> std::result::Result<(), String> {
    let routes = state
        .routes
        .lock()
        .map_err(|_| "graphical relay route table is poisoned".to_owned())?;
    let route = routes
        .get(&channel_id)
        .ok_or_else(|| "data targeted an unknown graphical relay channel".to_owned())?;
    if route.opened.is_some() {
        return Err("data preceded graphical relay channel admission".to_owned());
    }
    let amount = u32::try_from(bytes.len())
        .map_err(|_| "graphical relay data length exceeded its byte bound".to_owned())?;
    let channel_bytes = route
        .incoming_bytes
        .clone()
        .try_acquire_many_owned(amount)
        .map_err(|_| "graphical relay channel queue exceeded its byte bound".to_owned())?;
    let session_bytes = state
        .incoming_session_bytes
        .clone()
        .try_acquire_many_owned(amount)
        .map_err(|_| "graphical relay session queue exceeded its byte bound".to_owned())?;
    route
        .incoming
        .try_send(IncomingCommand::Data(IncomingData {
            bytes,
            _channel_bytes: channel_bytes,
            _session_bytes: session_bytes,
        }))
        .map_err(|_| "graphical relay channel queue exceeded its frame bound".to_owned())
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
    Ok(())
}

async fn run_incoming_channel(
    channel_id: u32,
    mut writer: tokio::io::WriteHalf<DuplexStream>,
    mut incoming: mpsc::Receiver<IncomingCommand>,
    state: Arc<ClientState>,
    cancellation: CancellationToken,
    _channel_slot: Arc<OwnedSemaphorePermit>,
) {
    let pump = async {
        let mut half_closed = false;
        loop {
            match incoming.recv().await {
                Some(IncomingCommand::Data(data)) if !half_closed => {
                    if writer.write_all(&data.bytes).await.is_err() {
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
                None => {
                    let _ = writer.shutdown().await;
                    break;
                }
                Some(IncomingCommand::Data(_) | IncomingCommand::HalfClose) => {
                    state.fail("remote graphical relay sent data after channel half-close");
                    break;
                }
            }
        }
    };
    tokio::select! {
        biased;
        () = cancellation.cancelled() => {}
        () = pump => {}
    }
    // The outgoing read half can still own the duplex stream. Explicit shutdown
    // exposes EOF to the application without waiting for that half to disappear.
    let _ = writer.shutdown().await;
}

async fn run_outgoing_channel(
    channel_id: u32,
    mut reader: tokio::io::ReadHalf<DuplexStream>,
    data_outbound: FairDataChannel,
    state: Arc<ClientState>,
    cancellation: CancellationToken,
    _channel_slot: Arc<OwnedSemaphorePermit>,
) {
    let pump = async {
        let mut buffer = vec![0_u8; MAX_DATA_BYTES];
        loop {
            let permit = match data_outbound.reserve().await {
                Ok(permit) => permit,
                Err(error) => {
                    if !cancellation.is_cancelled() {
                        state.fail(format!("graphical relay byte reservation failed: {error}"));
                    }
                    return;
                }
            };
            let read = reader.read(&mut buffer).await;
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
                        if !cancellation.is_cancelled() {
                            state.fail(format!("graphical relay data queue failed: {error}"));
                        }
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
    };
    tokio::select! {
        biased;
        () = cancellation.cancelled() => {}
        () = pump => {}
    }
}

#[derive(Debug)]
struct ChannelGuard {
    channel_id: u32,
    handle: Arc<ClientHandle>,
    data_outbound: FairDataChannel,
    incoming_cancellation: CancellationToken,
    _channel_slot: Arc<OwnedSemaphorePermit>,
}

impl Drop for ChannelGuard {
    fn drop(&mut self) {
        let state = &self.handle.state;
        state.close_channel(self.channel_id);
        self.incoming_cancellation.cancel();
        self.data_outbound.discard();
        if state
            .control_outbound
            .try_send(Frame::CloseChannel {
                channel_id: self.channel_id,
            })
            .is_err()
        {
            state.fail("graphical relay control output queue closed or full");
        }
    }
}

/// One logical full-duplex byte channel over a graphical relay session.
///
/// Dropping the channel performs a full close and can discard buffered writes.
/// Shut down its write side and drain its response first when ordered completion
/// is required.
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

    async fn test_client(capacity: usize) -> (ClientMultiplexer, DuplexStream) {
        let (transport, mut server) = tokio::io::duplex(capacity);
        let (reader, writer) = tokio::io::split(transport);
        let negotiation = tokio::spawn(ClientMultiplexer::negotiate(reader, writer));
        assert_eq!(read_frame(&mut server).await.unwrap(), Some(Frame::Hello));
        write_frame(&mut server, &Frame::HelloAck).await.unwrap();
        (negotiation.await.unwrap().unwrap(), server)
    }

    async fn test_open(client: &ClientMultiplexer, server: &mut DuplexStream) -> LogicalChannel {
        let opening_client = client.clone();
        let opening = tokio::spawn(async move { opening_client.open_channel().await.unwrap() });
        let channel_id = loop {
            match read_frame(server).await.unwrap() {
                Some(Frame::OpenChannel { channel_id }) => break channel_id,
                Some(Frame::CloseChannel { .. } | Frame::HalfClose { .. }) => {}
                other => panic!("unexpected admission frame: {other:?}"),
            }
        };
        write_frame(server, &Frame::ChannelOpened { channel_id })
            .await
            .unwrap();
        opening.await.unwrap()
    }

    #[tokio::test]
    async fn outgoing_half_close_follows_all_data_and_keeps_incoming_reply_open() {
        time::timeout(Duration::from_secs(2), async {
            let (client, mut server) = test_client(4 * MAX_DATA_BYTES).await;
            let mut channel = test_open(&client, &mut server).await;
            let expected = vec![0xaa; 2 * MAX_DATA_BYTES + 7];
            channel.write_all(&expected).await.unwrap();
            channel.shutdown().await.unwrap();
            let channel_id = channel.channel_id();
            let mut received = Vec::new();
            loop {
                match read_frame(&mut server).await.unwrap() {
                    Some(Frame::Data {
                        channel_id: id,
                        bytes,
                    }) => {
                        assert_eq!(id, channel_id);
                        received.extend(bytes);
                    }
                    Some(Frame::HalfClose { channel_id: id }) => {
                        assert_eq!(id, channel_id);
                        break;
                    }
                    other => panic!("unexpected half-close frame: {other:?}"),
                }
            }
            assert_eq!(received, expected);
            write_frame(
                &mut server,
                &Frame::Data {
                    channel_id,
                    bytes: b"reply".to_vec(),
                },
            )
            .await
            .unwrap();
            write_frame(&mut server, &Frame::CloseChannel { channel_id })
                .await
                .unwrap();
            let mut reply = Vec::new();
            channel.read_to_end(&mut reply).await.unwrap();
            assert_eq!(reply, b"reply");
            assert_eq!(client.terminal_failure(), None);
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn remote_close_preserves_tail_for_consumer_delayed_beyond_teardown_grace() {
        time::timeout(Duration::from_secs(2), async {
            let (client, mut server) = test_client(4 * MAX_DATA_BYTES).await;
            let mut channel = test_open(&client, &mut server).await;
            let channel_id = channel.channel_id();
            for byte in [0xaa, 0xbb] {
                write_frame(
                    &mut server,
                    &Frame::Data {
                        channel_id,
                        bytes: vec![byte; MAX_DATA_BYTES],
                    },
                )
                .await
                .unwrap();
            }
            write_frame(&mut server, &Frame::CloseChannel { channel_id })
                .await
                .unwrap();
            while client
                .handle
                .state
                .routes
                .lock()
                .unwrap()
                .contains_key(&channel_id)
            {
                tokio::task::yield_now().await;
            }
            time::sleep(Duration::from_millis(150)).await;
            assert_eq!(
                client.handle.state.channel_slots.available_permits(),
                MAX_LOGICAL_CHANNELS - 1
            );
            let mut received = Vec::new();
            channel.read_to_end(&mut received).await.unwrap();
            assert_eq!(
                received,
                [vec![0xaa; MAX_DATA_BYTES], vec![0xbb; MAX_DATA_BYTES]].concat()
            );
            assert_eq!(client.terminal_failure(), None);
            drop(channel);
            assert_eq!(
                client.handle.state.channel_slots.available_permits(),
                MAX_LOGICAL_CHANNELS
            );
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn pending_remote_closed_drains_hold_local_admission_until_local_drop() {
        time::timeout(Duration::from_secs(10), async {
            let (client, mut server) = test_client(64 * 1024).await;
            let state = &client.handle.state;
            let mut channels = Vec::new();
            for _ in 0..MAX_LOGICAL_CHANNELS {
                let channel = test_open(&client, &mut server).await;
                let channel_id = channel.channel_id();
                send_incoming_data(state, channel_id, vec![0xaa; MAX_DATA_BYTES]).unwrap();
                send_incoming_data(state, channel_id, vec![0xbb]).unwrap();
                close_remote_channel(state, channel_id).unwrap();
                channels.push(channel);
            }
            assert!(state.routes.lock().unwrap().is_empty());
            assert_eq!(state.channel_slots.available_permits(), 0);
            assert!(
                client
                    .open_channel()
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("channel limit")
            );
            drop(channels.pop());
            while state.channel_slots.available_permits() == 0 {
                tokio::task::yield_now().await;
            }
            let reopened = test_open(&client, &mut server).await;
            assert_eq!(state.channel_slots.available_permits(), 0);
            drop(reopened);
            drop(channels);
            while state.channel_slots.available_permits() != MAX_LOGICAL_CHANNELS {
                tokio::task::yield_now().await;
            }
            assert_eq!(
                state.incoming_session_bytes.available_permits(),
                MAX_SESSION_QUEUED_BYTES
            );
            assert_eq!(client.terminal_failure(), None);
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn session_cancel_interrupts_physical_writer_and_outgoing_drain() {
        time::timeout(Duration::from_secs(2), async {
            let (client, mut server) = test_client(64).await;
            let mut channel = test_open(&client, &mut server).await;
            channel
                .write_all(&vec![0xaa; MAX_DATA_BYTES])
                .await
                .unwrap();
            channel.shutdown().await.unwrap();
            // Consuming one header byte proves the physical frame write has begun.
            assert_eq!(server.read(&mut [0; 1]).await.unwrap(), 1);
            let state = client.handle.state.clone();
            let weak = Arc::downgrade(&state);
            state.fail("test cancellation");
            drop(channel);
            drop(client);
            drop(state);
            while weak.strong_count() != 0 {
                tokio::task::yield_now().await;
            }
            let mut pending = Vec::new();
            server.read_to_end(&mut pending).await.unwrap();
            assert!(pending.len() < MAX_DATA_BYTES);
        })
        .await
        .unwrap();
    }

    async fn blocked_incoming_teardown(session: bool) {
        time::timeout(Duration::from_secs(2), async {
            let (control_outbound, _control_rx) = mpsc::channel(4);
            let (data_outbound, _data_rx) = fair_data_queue();
            let state = Arc::new(ClientState {
                control_outbound,
                data_outbound,
                incoming_session_bytes: Arc::new(Semaphore::new(MAX_SESSION_QUEUED_BYTES)),
                channel_slots: Arc::new(Semaphore::new(MAX_LOGICAL_CHANNELS)),
                routes: Mutex::new(HashMap::new()),
                allocator: Mutex::new(ChannelIdAllocator::default()),
                failure: Mutex::new(None),
                cancellation: CancellationToken::new(),
            });
            let (mut application, bridge) = tokio::io::duplex(1);
            // Retain the other half: dropping the incoming half alone does not signal EOF.
            let (_bridge_reader, bridge_writer) = tokio::io::split(bridge);
            let (incoming, incoming_rx) = mpsc::channel(CHANNEL_QUEUE_FRAMES);
            let incoming_bytes = Arc::new(Semaphore::new(MAX_INCOMING_CHANNEL_QUEUED_BYTES));
            let cancellation = state.cancellation.child_token();
            state.routes.lock().unwrap().insert(
                1,
                Route {
                    opened: None,
                    incoming: incoming.clone(),
                    incoming_bytes: incoming_bytes.clone(),
                    outgoing_cancellation: state.cancellation.child_token(),
                    incoming_cancellation: cancellation.clone(),
                    data_outbound: state.data_outbound.channel(1),
                },
            );
            let (sibling, _sibling_rx) = mpsc::channel(1);
            state.routes.lock().unwrap().insert(
                2,
                Route {
                    opened: None,
                    incoming: sibling,
                    outgoing_cancellation: state.cancellation.child_token(),
                    incoming_cancellation: state.cancellation.child_token(),
                    data_outbound: state.data_outbound.channel(2),
                    incoming_bytes: Arc::new(Semaphore::new(MAX_INCOMING_CHANNEL_QUEUED_BYTES)),
                },
            );
            send_incoming_data(&state, 1, vec![0xaa; MAX_DATA_BYTES]).unwrap();
            let task = tokio::spawn(run_incoming_channel(
                1,
                bridge_writer,
                incoming_rx,
                state.clone(),
                cancellation.clone(),
                Arc::new(state.channel_slots.clone().try_acquire_owned().unwrap()),
            ));
            while incoming.capacity() != CHANNEL_QUEUE_FRAMES {
                tokio::task::yield_now().await;
            }
            assert_eq!(
                incoming_bytes.available_permits(),
                MAX_INCOMING_CHANNEL_QUEUED_BYTES - MAX_DATA_BYTES
            );
            if session {
                state.fail("test cancellation");
            } else {
                close_remote_channel(&state, 1).unwrap();
                // Ordered remote EOF retains a bounded drain; local close interrupts it.
                assert!(!task.is_finished());
                cancellation.cancel();
            }
            task.await.unwrap();
            assert_eq!(
                incoming_bytes.available_permits(),
                MAX_INCOMING_CHANNEL_QUEUED_BYTES
            );
            let mut received = Vec::new();
            application.read_to_end(&mut received).await.unwrap();
            assert_eq!(received, vec![0xaa]);
            if !session {
                assert!(state.routes.lock().unwrap().contains_key(&2));
                assert!(!state.cancellation.is_cancelled());
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn incoming_channel_local_close_interrupts_remote_closed_blocked_consumer() {
        blocked_incoming_teardown(false).await;
    }

    #[tokio::test]
    async fn incoming_channel_session_cancel_interrupts_blocked_consumer() {
        blocked_incoming_teardown(true).await;
    }

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
    async fn delayed_consumer_accepts_one_fragmented_private_frame_without_killing_siblings() {
        const FRAMES: usize = 16;
        let (client, server) = tokio::io::duplex(MAX_DATA_BYTES * FRAMES * 2);
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
            assert_eq!(
                read_frame(&mut server_reader).await.unwrap(),
                Some(Frame::OpenChannel { channel_id: 1 })
            );
            write_frame(&mut server_writer, &Frame::ChannelOpened { channel_id: 1 })
                .await
                .unwrap();
            for index in 0..FRAMES {
                write_frame(
                    &mut server_writer,
                    &Frame::Data {
                        channel_id: 1,
                        bytes: vec![u8::try_from(index).unwrap(); MAX_DATA_BYTES],
                    },
                )
                .await
                .unwrap();
            }
            assert_eq!(
                read_frame(&mut server_reader).await.unwrap(),
                Some(Frame::OpenChannel { channel_id: 2 })
            );
            write_frame(&mut server_writer, &Frame::ChannelOpened { channel_id: 2 })
                .await
                .unwrap();
            let Some(Frame::Data { channel_id, bytes }) =
                read_frame(&mut server_reader).await.unwrap()
            else {
                panic!("expected sibling channel data");
            };
            assert_eq!(channel_id, 2);
            write_frame(&mut server_writer, &Frame::Data { channel_id, bytes })
                .await
                .unwrap();
            done_rx.await.unwrap();
        });

        let client = ClientMultiplexer::negotiate(client_reader, client_writer)
            .await
            .unwrap();
        let mut delayed = client.open_channel().await.unwrap();
        time::sleep(Duration::from_millis(50)).await;
        let mut received = vec![0_u8; MAX_DATA_BYTES * FRAMES];
        delayed.read_exact(&mut received).await.unwrap();
        for (index, chunk) in received.chunks_exact(MAX_DATA_BYTES).enumerate() {
            assert!(
                chunk
                    .iter()
                    .all(|byte| *byte == u8::try_from(index).unwrap())
            );
        }
        assert_eq!(client.terminal_failure(), None);

        let mut sibling = client.open_channel().await.unwrap();
        sibling.write_all(b"sibling").await.unwrap();
        let mut echoed = [0_u8; 7];
        sibling.read_exact(&mut echoed).await.unwrap();
        assert_eq!(&echoed, b"sibling");
        assert_eq!(client.terminal_failure(), None);
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
