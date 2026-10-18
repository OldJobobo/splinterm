//! One owned, joinable thread for the daemon's D-Bus session connection.
//!
//! zbus offers two ready-made ways to call D-Bus from synchronous code, and
//! neither can give a thread back:
//!
//! * The `async-io` backend starts a reactor thread named `async-io` on first
//!   use. That thread loops for the life of the process and is never joined.
//! * With the `tokio` backend, `zbus::blocking` runs every call on a
//!   multi-thread runtime kept in a process-wide static, which is never
//!   dropped.
//!
//! Upgrade handoff needs the old daemon back at exactly one thread, so this
//! worker owns the connection instead. It runs a current-thread Tokio runtime
//! on its own thread. Because the connection is built inside that runtime,
//! zbus picks its Tokio backend even when `async-io` is compiled in, and every
//! connection task runs on the worker. [`DbusWorker::shutdown`] closes the
//! connection, drops the runtime, which joins its blocking pool, and then
//! joins the worker thread.

use std::{
    fmt,
    future::Future,
    io,
    pin::Pin,
    sync::{Mutex, PoisonError, mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};

use thiserror::Error;
use tokio::{sync::mpsc as job_channel, task::JoinSet};

/// Name of the worker thread that owns the connection. Linux keeps at most
/// 15 bytes of a thread name, so both names fit in that.
pub const WORKER_THREAD_NAME: &str = "splinterd-dbus";
/// Name of the blocking-pool thread zbus uses on the worker's runtime. Its
/// Unix handshake sends the credentials byte and reads peer credentials
/// through `spawn_blocking`; dropping the runtime joins that thread.
pub const BLOCKING_THREAD_NAME: &str = "splinterd-dbusb";

type Job = Box<dyn FnOnce(zbus::Connection) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

#[derive(Debug, Error)]
pub enum DbusWorkerError {
    #[error("cannot start the D-Bus worker: {0}")]
    Start(#[source] io::Error),
    #[error("D-Bus session connection failed: {0}")]
    Connect(#[source] zbus::Error),
    #[error("the D-Bus worker is shut down")]
    Closed,
    #[error("the D-Bus worker thread panicked")]
    Panicked,
}

/// Session-bus connection served by one thread that [`Self::shutdown`] joins.
pub struct DbusWorker {
    jobs: Mutex<Option<job_channel::UnboundedSender<Job>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl fmt::Debug for DbusWorker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("DbusWorker").finish_non_exhaustive()
    }
}

impl DbusWorker {
    /// Starts the worker thread and connects it to the session bus.
    ///
    /// Blocks until the connection is ready or has failed. It may be called
    /// from any thread, including a Tokio worker.
    ///
    /// # Errors
    /// Fails if the thread or its runtime cannot start or the session bus
    /// cannot be reached. A failed start leaves no thread behind.
    pub fn connect_session(method_timeout: Duration) -> Result<Self, DbusWorkerError> {
        let (ready, readiness) = mpsc::sync_channel(1);
        let (jobs, queue) = job_channel::unbounded_channel();
        let thread = thread::Builder::new()
            .name(WORKER_THREAD_NAME.into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .thread_name(BLOCKING_THREAD_NAME)
                    .enable_io()
                    .enable_time()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready.send(Err(DbusWorkerError::Start(error)));
                        return;
                    }
                };
                runtime.block_on(serve(method_timeout, ready, queue));
                // Dropping the runtime here, before the thread ends, joins
                // any blocking-pool thread zbus may have used.
                drop(runtime);
            })
            .map_err(DbusWorkerError::Start)?;
        let started = readiness.recv();
        match started {
            Ok(Ok(())) => Ok(Self {
                jobs: Mutex::new(Some(jobs)),
                thread: Mutex::new(Some(thread)),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => Err(thread
                .join()
                .map_or(DbusWorkerError::Panicked, |()| DbusWorkerError::Closed)),
        }
    }

    /// Runs one request on the worker's connection and waits for its result.
    ///
    /// Requests run concurrently on the worker. The calling thread blocks;
    /// never call this from inside a request.
    ///
    /// # Errors
    /// The outer error reports a worker that is shut down or dropped the
    /// request; the inner result is the request's own D-Bus result.
    pub fn call<T, F, Fut>(&self, request: F) -> Result<zbus::Result<T>, DbusWorkerError>
    where
        T: Send + 'static,
        F: FnOnce(zbus::Connection) -> Fut + Send + 'static,
        Fut: Future<Output = zbus::Result<T>> + Send + 'static,
    {
        let (reply, response) = mpsc::sync_channel(1);
        let job: Job = Box::new(move |connection| {
            Box::pin(async move {
                let _ = reply.send(request(connection).await);
            })
        });
        self.jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .ok_or(DbusWorkerError::Closed)?
            .send(job)
            .map_err(|_| DbusWorkerError::Closed)?;
        response.recv().map_err(|_| DbusWorkerError::Closed)
    }

    /// Closes the connection and joins the worker thread.
    ///
    /// Requests already running finish first; each is bounded by the method
    /// timeout. Every later [`Self::call`] fails with
    /// [`DbusWorkerError::Closed`]. Calling this again is a no-op.
    ///
    /// # Errors
    /// Reports a worker thread that panicked.
    pub fn shutdown(&self) -> Result<(), DbusWorkerError> {
        drop(
            self.jobs
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take(),
        );
        let thread = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        thread.map_or(Ok(()), |thread| {
            thread.join().map_err(|_| DbusWorkerError::Panicked)
        })
    }
}

impl Drop for DbusWorker {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

async fn serve(
    method_timeout: Duration,
    ready: mpsc::SyncSender<Result<(), DbusWorkerError>>,
    mut queue: job_channel::UnboundedReceiver<Job>,
) {
    let connection = match connect(method_timeout).await {
        Ok(connection) => connection,
        Err(error) => {
            let _ = ready.send(Err(DbusWorkerError::Connect(error)));
            return;
        }
    };
    if ready.send(Ok(())).is_err() {
        let _ = connection.close().await;
        return;
    }
    let mut requests = JoinSet::new();
    loop {
        tokio::select! {
            job = queue.recv() => match job {
                Some(job) => {
                    requests.spawn(job(connection.clone()));
                }
                None => break,
            },
            Some(_) = requests.join_next(), if !requests.is_empty() => {}
        }
    }
    while requests.join_next().await.is_some() {}
    // Closing ends the socket reader task before the runtime is dropped.
    let _ = connection.close().await;
}

async fn connect(method_timeout: Duration) -> zbus::Result<zbus::Connection> {
    zbus::connection::Builder::session()?
        .method_timeout(method_timeout)
        .build()
        .await
}
