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
//! connection task runs on the worker. [`DbusWorker::shutdown`] gives running
//! requests a bounded grace period, aborts any that remain, closes the
//! connection within the same bound, drops the runtime (which joins its
//! blocking pool), and then joins the worker thread.

use std::{
    fmt,
    future::Future,
    io, mem,
    pin::Pin,
    sync::{Mutex, PoisonError, mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};

use thiserror::Error;
use tokio::{
    sync::mpsc as job_channel,
    task::{JoinError, JoinSet},
    time::timeout,
};
use tracing::warn;

/// Name of the worker thread that owns the connection. Linux keeps at most
/// 15 bytes of a thread name, so both names fit in that.
pub const WORKER_THREAD_NAME: &str = "splinterd-dbus";
/// Name of the blocking-pool thread zbus uses on the worker's runtime. On
/// Linux only its peer-credentials read uses `spawn_blocking`; dropping the
/// runtime joins that thread.
pub const BLOCKING_THREAD_NAME: &str = "splinterd-dbusb";
/// Default bound for each shutdown phase. zbus's method timeout covers only
/// the reply wait, not sending a request or closing the socket, so a wedged
/// bus could otherwise hold shutdown forever.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);
/// Longest panic message logged for a failed request.
const PANIC_MESSAGE_LIMIT: usize = 256;

type Job = Box<dyn FnOnce(zbus::Connection) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

enum Command {
    Run(Job),
    Shutdown(Duration),
}

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
    jobs: Mutex<Option<job_channel::UnboundedSender<Command>>>,
    thread: Mutex<WorkerThread>,
}

/// The worker thread, or how its join ended.
enum WorkerThread {
    Running(JoinHandle<()>),
    Joined { panicked: bool },
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
                thread: Mutex::new(WorkerThread::Running(thread)),
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
            .send(Command::Run(job))
            .map_err(|_| DbusWorkerError::Closed)?;
        response.recv().map_err(|_| DbusWorkerError::Closed)
    }

    /// [`Self::shutdown_within`] with the default [`SHUTDOWN_GRACE`].
    ///
    /// # Errors
    /// Reports a worker thread that panicked.
    pub fn shutdown(&self) -> Result<(), DbusWorkerError> {
        self.shutdown_within(SHUTDOWN_GRACE)
    }

    /// Closes the connection and joins the worker thread.
    ///
    /// Running requests get up to `grace` to finish. Any still running are
    /// then aborted, and their callers get [`DbusWorkerError::Closed`]. Closing
    /// the connection is bounded by `grace` too, so the worker ends within
    /// about three `grace` periods (drain, abort, close) plus dropping its
    /// runtime. Every later [`Self::call`] fails with
    /// [`DbusWorkerError::Closed`]. A concurrent or later call waits for the
    /// same join and reports the same result.
    ///
    /// # Errors
    /// Reports a worker thread that panicked.
    pub fn shutdown_within(&self, grace: Duration) -> Result<(), DbusWorkerError> {
        if let Some(jobs) = self
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            // A worker that already stopped has dropped its receiver; the
            // join below still reaps it.
            let _ = jobs.send(Command::Shutdown(grace));
        }
        // Holding the lock across the join makes a concurrent caller wait for
        // it instead of returning while the worker is still running.
        let mut thread = self.thread.lock().unwrap_or_else(PoisonError::into_inner);
        let panicked = match mem::replace(&mut *thread, WorkerThread::Joined { panicked: false }) {
            WorkerThread::Running(handle) => handle.join().is_err(),
            WorkerThread::Joined { panicked } => panicked,
        };
        *thread = WorkerThread::Joined { panicked };
        if panicked {
            Err(DbusWorkerError::Panicked)
        } else {
            Ok(())
        }
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
    mut queue: job_channel::UnboundedReceiver<Command>,
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
    let grace = loop {
        tokio::select! {
            command = queue.recv() => match command {
                Some(Command::Run(job)) => {
                    requests.spawn(job(connection.clone()));
                }
                Some(Command::Shutdown(grace)) => break grace,
                None => break SHUTDOWN_GRACE,
            },
            Some(outcome) = requests.join_next(), if !requests.is_empty() => {
                log_request_outcome(outcome);
            }
        }
    };
    if !drain(&mut requests, grace).await {
        // Aborting drops each stuck request, which releases any socket
        // write lock it holds, so the close below can proceed.
        warn!(
            requests = requests.len(),
            "aborting D-Bus requests still running at shutdown"
        );
        requests.abort_all();
        if !drain(&mut requests, grace).await {
            warn!("aborted D-Bus requests did not finish; dropping them with the runtime");
        }
    }
    // Closing ends the socket reader task before the runtime is dropped.
    if timeout(grace, connection.close()).await.is_err() {
        warn!("D-Bus connection did not close within the shutdown bound");
    }
}

/// Waits up to `grace` for every request; returns whether all finished.
async fn drain(requests: &mut JoinSet<()>, grace: Duration) -> bool {
    timeout(grace, async {
        while let Some(outcome) = requests.join_next().await {
            log_request_outcome(outcome);
        }
    })
    .await
    .is_ok()
}

fn log_request_outcome(outcome: Result<(), JoinError>) {
    let Err(error) = outcome else {
        return;
    };
    if !error.is_panic() {
        return;
    }
    let payload = error.into_panic();
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("non-string panic payload");
    let message: String = message.chars().take(PANIC_MESSAGE_LIMIT).collect();
    warn!(panic = %message, "D-Bus worker request panicked");
}

async fn connect(method_timeout: Duration) -> zbus::Result<zbus::Connection> {
    zbus::connection::Builder::session()?
        .method_timeout(method_timeout)
        .build()
        .await
}
