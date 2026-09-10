//! One cancellable Tokio owner for native transport. No D-Bus future is polled
//! by the UI; the watch channel retains only the newest bounded publication.

use super::{Connection, NativeConnection, SharedTree, focus_id};
use crate::accessibility::{SemanticActionQueue, SemanticNavigationSnapshot};
use std::{
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::Duration,
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

const OPERATION_TIMEOUT: Duration = Duration::from_millis(500);
const RETRY_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone)]
struct Publication {
    snapshot: SemanticNavigationSnapshot,
    focused: bool,
}

trait Transport {
    async fn publish(&mut self, state: Publication) -> Result<(), &'static str>;
    fn reset(&mut self);
}

/// Drive zbus's existing async-io executor on the worker runtime rather than
/// letting each connection spawn an unowned executor thread. The reactor is
/// shared by async-io; all connection tasks and their driver are cancellable.
pub(super) struct ConnectionDriver(tokio::task::JoinHandle<()>);

impl ConnectionDriver {
    pub(super) fn new(connection: &Connection) -> Self {
        let connection = connection.clone();
        Self(tokio::spawn(async move {
            loop {
                connection.executor().tick().await;
            }
        }))
    }
}

impl Drop for ConnectionDriver {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct NativeTransport {
    tree: SharedTree,
    session: Option<(Connection, ConnectionDriver)>,
    native: Option<NativeConnection>,
}

async fn connect_session(
    builder: zbus::connection::Builder<'_>,
) -> zbus::Result<(Connection, ConnectionDriver)> {
    let connection = builder.internal_executor(false).build().await?;
    let driver = ConnectionDriver::new(&connection);
    Ok((connection, driver))
}

impl Transport for NativeTransport {
    async fn publish(&mut self, state: Publication) -> Result<(), &'static str> {
        let previous = self.tree.snapshot().clone();
        let previous_focus = *self
            .tree
            .window_focused
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.tree.set_snapshot(state.snapshot.clone());
        *self
            .tree
            .window_focused
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = state.focused;
        if self.session.is_none() {
            // Do not allow transport discovery to spawn autolaunch commands or
            // resolve remote hosts. The desktop accessibility transport is local.
            if std::env::var("DBUS_SESSION_BUS_ADDRESS")
                .is_ok_and(|address| !address.starts_with("unix:") || address.contains(';'))
            {
                return Err("non-local session bus rejected");
            }
            let builder =
                zbus::connection::Builder::session().map_err(|_| "session bus unavailable")?;
            self.session = Some(
                connect_session(builder)
                    .await
                    .map_err(|_| "session bus unavailable")?,
            );
        }
        let (session, _) = self.session.as_ref().expect("connected session");
        if !NativeConnection::is_enabled(session)
            .await
            .map_err(|_| "accessibility service unavailable")?
        {
            self.native = None;
            return Ok(());
        }
        if self.native.is_none() {
            self.native = NativeConnection::connect(session, &self.tree)
                .await
                .map_err(|_| "accessibility connection failed")?;
            if let Some(native) = &self.native {
                native
                    .emit_window_focus(state.focused)
                    .await
                    .map_err(|_| "accessibility focus failed")?;
                if state.snapshot.focus != crate::accessibility::SemanticFocus::None {
                    native
                        .emit_state(focus_id(&state.snapshot), "focused", state.focused)
                        .await
                        .map_err(|_| "accessibility focus failed")?;
                }
            }
        } else if let Some(native) = &mut self.native {
            native
                .publish_update(&self.tree, &previous, &state.snapshot, previous_focus)
                .await
                .map_err(|_| "accessibility publication failed")?;
            if previous_focus != state.focused {
                native
                    .emit_window_focus(state.focused)
                    .await
                    .map_err(|_| "accessibility focus failed")?;
            }
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.native = None;
        self.session = None;
        *self
            .tree
            .bus_name
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}

async fn run_transport(
    mut transport: impl Transport,
    mut updates: watch::Receiver<Publication>,
    stop: CancellationToken,
    error: Arc<Mutex<Option<&'static str>>>,
) {
    loop {
        let state = updates.borrow_and_update().clone();
        let result = tokio::select! {
            biased;
            () = stop.cancelled() => break,
            result = tokio::time::timeout(OPERATION_TIMEOUT, transport.publish(state)) => result.unwrap_or(Err("accessibility transport timed out")),
        };
        *error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = result.err();
        if result.is_err() {
            transport.reset();
            // Back off even during publication bursts; never busy-reconnect.
            tokio::select! {
                biased;
                () = stop.cancelled() => break,
                () = tokio::time::sleep(RETRY_INTERVAL) => {},
            }
        } else {
            tokio::select! {
                biased;
                () = stop.cancelled() => break,
                result = updates.changed() => if result.is_err() { break; },
                () = tokio::time::sleep(RETRY_INTERVAL) => {},
            }
        }
    }
    transport.reset();
}

pub(crate) struct NativeAtspiPublisher {
    latest: Publication,
    updates: watch::Sender<Publication>,
    stop: CancellationToken,
    worker: Option<JoinHandle<()>>,
    error: Arc<Mutex<Option<&'static str>>>,
}

impl NativeAtspiPublisher {
    pub(crate) fn new(snapshot: SemanticNavigationSnapshot, actions: SemanticActionQueue) -> Self {
        let transport = NativeTransport {
            tree: SharedTree::new(snapshot.clone(), actions),
            session: None,
            native: None,
        };
        Self::start(snapshot, transport)
    }

    fn start(
        snapshot: SemanticNavigationSnapshot,
        transport: impl Transport + Send + 'static,
    ) -> Self {
        let latest = Publication {
            snapshot,
            focused: false,
        };
        let (updates, receiver) = watch::channel(latest.clone());
        let stop = CancellationToken::new();
        let worker_stop = stop.clone();
        let error = Arc::new(Mutex::new(None));
        let worker_error = error.clone();
        let worker = std::thread::Builder::new()
            .name("splinterm-atspi".into())
            .spawn(move || {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime.block_on(run_transport(
                        transport,
                        receiver,
                        worker_stop,
                        worker_error,
                    )),
                    Err(_) => {
                        *worker_error
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) =
                            Some("accessibility runtime unavailable");
                    }
                }
            })
            .ok();
        if worker.is_none() {
            *error
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some("accessibility worker unavailable");
        }
        Self {
            latest,
            updates,
            stop,
            worker,
            error,
        }
    }

    pub(crate) fn publish(&mut self, snapshot: &SemanticNavigationSnapshot) {
        self.latest.snapshot = snapshot.clone();
        self.updates.send_replace(self.latest.clone());
    }

    pub(crate) fn update_window_focus_state(&mut self, focused: bool) {
        if self.latest.focused != focused {
            self.latest.focused = focused;
            self.updates.send_replace(self.latest.clone());
        }
    }

    pub(crate) fn transport_error(&self) -> Option<String> {
        self.error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .map(str::to_owned)
    }
}

impl Drop for NativeAtspiPublisher {
    fn drop(&mut self) {
        self.stop.cancel();
        // All I/O is cancellable async I/O on this one current-thread runtime;
        // there are no blocking tasks, detached workers, or bus-dependent joins.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };
    use std::time::Instant;

    struct MockTransport {
        started: mpsc::Sender<u64>,
        resets: Arc<AtomicUsize>,
        unavailable: bool,
    }

    impl Transport for MockTransport {
        async fn publish(&mut self, state: Publication) -> Result<(), &'static str> {
            self.started.send(state.snapshot.generation).unwrap();
            if self.unavailable {
                Err("mock bus unavailable")
            } else {
                std::future::pending().await
            }
        }
        fn reset(&mut self) {
            self.resets.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn mock(unavailable: bool) -> (NativeAtspiPublisher, mpsc::Receiver<u64>, Arc<AtomicUsize>) {
        let (started, receiver) = mpsc::channel();
        let resets = Arc::new(AtomicUsize::new(0));
        let publisher = NativeAtspiPublisher::start(
            super::super::tests::snapshot(),
            MockTransport {
                started,
                resets: resets.clone(),
                unavailable,
            },
        );
        (publisher, receiver, resets)
    }

    #[test]
    fn native_session_handshake_is_cancellable_without_a_live_bus() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (client, mut silent_peer) = std::os::unix::net::UnixStream::pair().unwrap();
        let started = Instant::now();
        runtime.block_on(async {
            let builder = zbus::connection::Builder::async_io_unix_stream(client);
            assert!(
                tokio::time::timeout(Duration::from_millis(50), connect_session(builder))
                    .await
                    .is_err()
            );
        });
        assert!(started.elapsed() < OPERATION_TIMEOUT);
        // The cancelled handshake drops its transport, rather than leaving a
        // hidden blocking connect thread holding the socket open.
        silent_peer
            .set_read_timeout(Some(OPERATION_TIMEOUT))
            .unwrap();
        let mut sent = Vec::new();
        std::io::Read::read_to_end(&mut silent_peer, &mut sent).unwrap();
        assert!(!sent.is_empty());
    }

    #[test]
    fn native_session_unavailable_peer_fails_without_a_live_bus() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (client, peer) = std::os::unix::net::UnixStream::pair().unwrap();
        drop(peer);
        runtime.block_on(async {
            let builder = zbus::connection::Builder::async_io_unix_stream(client);
            assert!(
                tokio::time::timeout(OPERATION_TIMEOUT, connect_session(builder))
                    .await
                    .unwrap()
                    .is_err()
            );
        });
    }

    #[test]
    fn slow_transport_keeps_only_latest_state_and_teardown_cancels_io() {
        let (mut publisher, started, resets) = mock(false);
        assert_eq!(started.recv_timeout(Duration::from_secs(2)).unwrap(), 0);
        let start = Instant::now();
        for generation in 1..=1000 {
            let mut snapshot = super::super::tests::snapshot();
            snapshot.generation = generation;
            publisher.publish(&snapshot);
        }
        assert_eq!(publisher.updates.borrow().snapshot.generation, 1000);
        assert!(start.elapsed() < OPERATION_TIMEOUT);
        let start = Instant::now();
        drop(publisher);
        assert!(start.elapsed() < OPERATION_TIMEOUT);
        assert!(resets.load(Ordering::SeqCst) > 0);
    }

    #[test]
    fn unavailable_transport_backs_off_and_teardown_does_not_wait_for_retry() {
        let (publisher, started, resets) = mock(true);
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(started.recv_timeout(Duration::from_millis(50)).is_err());
        assert_eq!(
            publisher.transport_error().as_deref(),
            Some("mock bus unavailable")
        );
        let start = Instant::now();
        drop(publisher);
        assert!(start.elapsed() < OPERATION_TIMEOUT);
        assert!(resets.load(Ordering::SeqCst) >= 2);
    }

    #[test]
    fn transport_deadline_cancels_a_never_completing_operation() {
        let (publisher, started, resets) = mock(false);
        started.recv_timeout(Duration::from_secs(2)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while publisher.transport_error().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            publisher.transport_error().as_deref(),
            Some("accessibility transport timed out")
        );
        assert!(resets.load(Ordering::SeqCst) > 0);
        drop(publisher);
    }
}
