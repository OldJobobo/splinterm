use std::{
    collections::VecDeque,
    ffi::OsString,
    io::{self, Read, Write},
    os::unix::process::ExitStatusExt,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use splinterm_core::SplintId;
use splinterm_pty::{
    LinuxPtyBackend, LinuxPtySession, PtyCommand, PtyError, PtySession, PtySignal, PtySize,
};
use splinterm_terminal::{
    ActiveScreen, CellAttributesSnapshot, CellSnapshotContent, CursorSnapshot, Dimensions,
    ImageContent, ImageContentId, ImageContentMetadata, ImagePlacement, ScrollRegion,
    ScrollbackSnapshot, SearchPage, SnapshotRequest, Terminal, TerminalConfig, TerminalEvent,
    TerminalModes, TerminalRevision, TerminalUpdate,
};
use thiserror::Error;
use tokio::{
    io::unix::AsyncFd,
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
    time::{self, Instant, MissedTickBehavior},
};

static NEXT_INCARNATION: AtomicU64 = AtomicU64::new(1);
const PARSE_BATCH: usize = 256;
const READ_BUFFER: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ProcessIncarnation(u64);

impl ProcessIncarnation {
    /// Advances process-wide allocation beyond a persisted incarnation.
    ///
    /// # Panics
    ///
    /// Panics if the persisted value has exhausted the `u64` incarnation space.
    pub fn reserve_after(incarnation: u64) {
        let next = incarnation
            .checked_add(1)
            .expect("process incarnation space exhausted");
        NEXT_INCARNATION.fetch_max(next, Ordering::Relaxed);
    }

    fn allocate() -> Self {
        let value = NEXT_INCARNATION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .expect("process incarnation space exhausted");
        Self(value)
    }

    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessExit {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

impl From<std::process::ExitStatus> for ProcessExit {
    fn from(status: std::process::ExitStatus) -> Self {
        Self {
            code: status.code(),
            signal: status.signal(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveCell {
    pub content: String,
    pub spacer_remaining: Option<u32>,
    pub attributes: CellAttributesSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveRow {
    pub row_id: Option<u64>,
    pub linebreak: bool,
    pub cells: Vec<LiveCell>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveSnapshot {
    pub splint_id: SplintId,
    pub incarnation: ProcessIncarnation,
    pub revision: TerminalRevision,
    pub dimensions: Dimensions,
    pub active_screen: ActiveScreen,
    pub cursor: CursorSnapshot,
    pub modes: TerminalModes,
    pub scroll_region: ScrollRegion,
    pub view_follows_live: bool,
    pub title: String,
    pub palette: [u32; 256],
    pub default_colors: [u32; 3],
    pub image_contents: Vec<ImageContentMetadata>,
    pub image_placements: Vec<ImagePlacement>,
    pub visible_rows: Vec<LiveRow>,
    pub scrollback_rows: Vec<LiveRow>,
    pub scrollback: ScrollbackSnapshot,
    pub exited: Option<ProcessExit>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveScrollbackPage {
    pub terminal_revision: TerminalRevision,
    pub history_generation: u64,
    pub title: String,
    pub oldest_available_row_id: Option<u64>,
    pub newest_available_row_id: Option<u64>,
    pub rows: Vec<LiveRow>,
    pub has_older: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveSearchPage {
    pub terminal_revision: TerminalRevision,
    pub history_generation: u64,
    pub title: String,
    pub page: SearchPage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LiveEvent {
    Update {
        incarnation: ProcessIncarnation,
        updates: Vec<TerminalUpdate>,
    },
    Exited {
        incarnation: ProcessIncarnation,
        status: ProcessExit,
    },
}

#[derive(Debug)]
pub struct Subscription {
    pub events: mpsc::Receiver<LiveEvent>,
    resnapshot: watch::Receiver<bool>,
}

#[derive(Debug)]
pub enum SubscriptionReceive {
    Event(LiveEvent),
    ResnapshotRequired,
    Closed,
}

impl Subscription {
    #[must_use]
    pub fn resnapshot_required(&self) -> bool {
        *self.resnapshot.borrow()
    }

    pub async fn changed(&mut self) -> bool {
        self.resnapshot.changed().await.is_ok() && *self.resnapshot.borrow()
    }

    pub async fn recv(&mut self) -> SubscriptionReceive {
        if *self.resnapshot.borrow() {
            return SubscriptionReceive::ResnapshotRequired;
        }
        tokio::select! {
            biased;
            changed = self.resnapshot.changed() => {
                if changed.is_ok() && *self.resnapshot.borrow() {
                    SubscriptionReceive::ResnapshotRequired
                } else {
                    SubscriptionReceive::Closed
                }
            }
            event = self.events.recv() => event.map_or(
                SubscriptionReceive::Closed,
                SubscriptionReceive::Event,
            ),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LiveSplintConfig {
    pub columns: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
    pub command_capacity: usize,
    pub input_byte_limit: usize,
    pub reply_byte_limit: usize,
    pub subscriber_capacity: usize,
    pub max_subscribers: usize,
    pub max_scrollback_snapshot_rows: usize,
    pub exit_drain_timeout: Duration,
    pub hangup_grace: Duration,
    pub terminate_grace: Duration,
    pub poll_interval: Duration,
    pub terminal: TerminalConfig,
    pub incarnation_environment: Option<OsString>,
}

impl Default for LiveSplintConfig {
    fn default() -> Self {
        Self {
            columns: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
            command_capacity: 64,
            input_byte_limit: 1024 * 1024,
            reply_byte_limit: 64 * 1024,
            subscriber_capacity: 64,
            max_subscribers: 8,
            max_scrollback_snapshot_rows: 1_000,
            exit_drain_timeout: Duration::from_millis(250),
            hangup_grace: Duration::from_secs(30),
            terminate_grace: Duration::from_secs(30),
            poll_interval: Duration::from_millis(10),
            terminal: TerminalConfig::default(),
            incarnation_environment: None,
        }
    }
}

#[derive(Debug, Error)]
pub enum LiveError {
    #[error(transparent)]
    Pty(#[from] PtyError),
    #[error("live Splint command channel is closed")]
    Closed,
    #[error("input queue limit exceeded")]
    InputQueueFull,
    #[error("terminal dimensions must be non-zero")]
    InvalidDimensions,
    #[error("terminal row identity is exhausted")]
    RowIdentityExhausted,
    #[error("subscriber capacity must be non-zero")]
    InvalidSubscriberCapacity,
    #[error("PTY reply queue limit exceeded")]
    ReplyQueueFull,
    #[error("child process has already exited")]
    ProcessExited,
    #[error("image content does not exist on the active screen")]
    ImageContentNotFound,
    #[error("image content generation or digest is stale")]
    StaleImageContent,
    #[error("poll interval must be non-zero")]
    InvalidPollInterval,
    #[error("live Splint task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("live Splint I/O failed: {0}")]
    Io(#[from] io::Error),
}

type Reply<T> = oneshot::Sender<Result<T, LiveError>>;

enum Command {
    Input(Vec<u8>, Reply<()>),
    Resize(PtySize, Reply<()>),
    Snapshot(usize, Reply<LiveSnapshot>),
    ImageContent(ImageContentId, u64, [u8; 32], Reply<ImageContent>),
    ScrollbackPage(Option<u64>, usize, Reply<LiveScrollbackPage>),
    Search(String, bool, usize, usize, Duration, Reply<LiveSearchPage>),
    Subscribe(usize, Reply<Subscription>),
    Attach(usize, usize, Reply<(LiveSnapshot, Subscription)>),
    Shutdown(oneshot::Sender<()>),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct LiveRuntimeMetrics {
    pub command_queue_high_water: usize,
    pub user_write_queue_high_water_bytes: usize,
    pub reply_write_queue_high_water_bytes: usize,
    pub pty_read_calls: u64,
    pub pty_read_bytes: u64,
    pub output_parse_batches: u64,
    pub output_terminal_updates: u64,
    pub output_live_events: u64,
    pub output_subscriber_overflows: u64,
    pub output_processing_ns: u64,
    pub snapshot_builds: u64,
    pub snapshot_build_ns: u64,
}

#[derive(Debug, Default)]
struct RuntimeMetrics {
    command_queue_high_water: AtomicUsize,
    user_write_queue_high_water_bytes: AtomicUsize,
    reply_write_queue_high_water_bytes: AtomicUsize,
    pty_read_calls: AtomicU64,
    pty_read_bytes: AtomicU64,
    output_parse_batches: AtomicU64,
    output_terminal_updates: AtomicU64,
    output_live_events: AtomicU64,
    output_subscriber_overflows: AtomicU64,
    output_processing_ns: AtomicU64,
    snapshot_builds: AtomicU64,
    snapshot_build_ns: AtomicU64,
}

impl RuntimeMetrics {
    fn observe_max(value: &AtomicUsize, candidate: usize) {
        value.fetch_max(candidate, Ordering::Relaxed);
    }

    fn snapshot(&self) -> LiveRuntimeMetrics {
        LiveRuntimeMetrics {
            command_queue_high_water: self.command_queue_high_water.load(Ordering::Relaxed),
            user_write_queue_high_water_bytes: self
                .user_write_queue_high_water_bytes
                .load(Ordering::Relaxed),
            reply_write_queue_high_water_bytes: self
                .reply_write_queue_high_water_bytes
                .load(Ordering::Relaxed),
            pty_read_calls: self.pty_read_calls.load(Ordering::Relaxed),
            pty_read_bytes: self.pty_read_bytes.load(Ordering::Relaxed),
            output_parse_batches: self.output_parse_batches.load(Ordering::Relaxed),
            output_terminal_updates: self.output_terminal_updates.load(Ordering::Relaxed),
            output_live_events: self.output_live_events.load(Ordering::Relaxed),
            output_subscriber_overflows: self.output_subscriber_overflows.load(Ordering::Relaxed),
            output_processing_ns: self.output_processing_ns.load(Ordering::Relaxed),
            snapshot_builds: self.snapshot_builds.load(Ordering::Relaxed),
            snapshot_build_ns: self.snapshot_build_ns.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LiveSplintHandle {
    pub splint_id: SplintId,
    pub incarnation: ProcessIncarnation,
    commands: mpsc::Sender<Command>,
    default_snapshot_rows: usize,
    default_subscriber_capacity: usize,
    max_input_message_bytes: usize,
    metrics: Arc<RuntimeMetrics>,
    exit: watch::Receiver<Option<ProcessExit>>,
}

#[allow(
    clippy::missing_errors_doc,
    reason = "all handle operations share the closed actor and operation-specific LiveError cases"
)]
impl LiveSplintHandle {
    pub async fn input(&self, bytes: Vec<u8>) -> Result<(), LiveError> {
        if bytes.len() > self.max_input_message_bytes {
            return Err(LiveError::InputQueueFull);
        }
        self.request(|reply| Command::Input(bytes, reply)).await
    }

    pub async fn resize(&self, size: PtySize) -> Result<(), LiveError> {
        self.request(|reply| Command::Resize(size, reply)).await
    }

    pub async fn snapshot(&self) -> Result<LiveSnapshot, LiveError> {
        self.snapshot_with_scrollback(self.default_snapshot_rows)
            .await
    }

    pub async fn snapshot_with_scrollback(
        &self,
        max_scrollback_rows: usize,
    ) -> Result<LiveSnapshot, LiveError> {
        self.request(|reply| Command::Snapshot(max_scrollback_rows, reply))
            .await
    }

    pub async fn image_content(
        &self,
        content_id: ImageContentId,
        generation: u64,
        digest: [u8; 32],
    ) -> Result<ImageContent, LiveError> {
        self.request(|reply| Command::ImageContent(content_id, generation, digest, reply))
            .await
    }

    pub async fn scrollback_page(
        &self,
        before_row_id: u64,
        max_rows: usize,
    ) -> Result<LiveScrollbackPage, LiveError> {
        self.request(|reply| Command::ScrollbackPage(Some(before_row_id), max_rows, reply))
            .await
    }

    pub async fn start_scrollback_page(
        &self,
        max_rows: usize,
    ) -> Result<LiveScrollbackPage, LiveError> {
        self.request(|reply| Command::ScrollbackPage(None, max_rows, reply))
            .await
    }

    pub async fn search(
        &self,
        query: String,
        case_sensitive: bool,
        skip_rows: usize,
        max_results: usize,
        deadline: Duration,
    ) -> Result<LiveSearchPage, LiveError> {
        self.request(|reply| {
            Command::Search(
                query,
                case_sensitive,
                skip_rows,
                max_results,
                deadline,
                reply,
            )
        })
        .await
    }

    pub async fn attach(&self) -> Result<(LiveSnapshot, Subscription), LiveError> {
        self.request(|reply| {
            Command::Attach(
                self.default_snapshot_rows,
                self.default_subscriber_capacity,
                reply,
            )
        })
        .await
    }

    pub async fn attach_with_scrollback(
        &self,
        max_scrollback_rows: usize,
    ) -> Result<(LiveSnapshot, Subscription), LiveError> {
        self.request(|reply| {
            Command::Attach(
                max_scrollback_rows.min(self.default_snapshot_rows),
                self.default_subscriber_capacity,
                reply,
            )
        })
        .await
    }

    pub async fn subscribe(&self) -> Result<Subscription, LiveError> {
        self.subscribe_with_capacity(self.default_subscriber_capacity)
            .await
    }

    pub async fn subscribe_with_capacity(
        &self,
        capacity: usize,
    ) -> Result<Subscription, LiveError> {
        self.request(|reply| Command::Subscribe(capacity, reply))
            .await
    }

    #[must_use]
    pub fn exit_status(&self) -> Option<ProcessExit> {
        *self.exit.borrow()
    }

    #[must_use]
    pub fn metrics(&self) -> LiveRuntimeMetrics {
        self.metrics.snapshot()
    }

    pub async fn wait_for_exit(&self) -> Option<ProcessExit> {
        let mut exit = self.exit.clone();
        loop {
            if let Some(status) = *exit.borrow() {
                return Some(status);
            }
            if exit.changed().await.is_err() {
                return *exit.borrow();
            }
        }
    }

    async fn shutdown(&self) -> Result<(), LiveError> {
        let (sender, receiver) = oneshot::channel();
        self.commands
            .send(Command::Shutdown(sender))
            .await
            .map_err(|_| LiveError::Closed)?;
        receiver.await.map_err(|_| LiveError::Closed)
    }

    async fn request<T>(&self, build: impl FnOnce(Reply<T>) -> Command) -> Result<T, LiveError> {
        let queued = self
            .commands
            .max_capacity()
            .saturating_sub(self.commands.capacity())
            .saturating_add(1);
        RuntimeMetrics::observe_max(&self.metrics.command_queue_high_water, queued);
        let (sender, receiver) = oneshot::channel();
        self.commands
            .send(build(sender))
            .await
            .map_err(|_| LiveError::Closed)?;
        receiver.await.map_err(|_| LiveError::Closed)?
    }
}

#[derive(Debug)]
pub struct LiveSplintRuntime {
    handle: LiveSplintHandle,
    task: JoinHandle<Result<ProcessExit, LiveError>>,
}

#[allow(
    clippy::missing_errors_doc,
    reason = "runtime lifecycle operations return the documented LiveError variants"
)]
impl LiveSplintRuntime {
    pub async fn spawn(
        splint_id: SplintId,
        backend: LinuxPtyBackend,
        command: PtyCommand,
        config: LiveSplintConfig,
    ) -> Result<Self, LiveError> {
        let incarnation = ProcessIncarnation::allocate();
        let command = if let Some(name) = &config.incarnation_environment {
            command.env(name, incarnation.value().to_string())
        } else {
            command
        };
        validate_dimensions(config.columns, config.rows)?;
        if config.poll_interval.is_zero() {
            return Err(LiveError::InvalidPollInterval);
        }
        let size = PtySize {
            columns: config.columns,
            rows: config.rows,
            pixel_width: config.pixel_width,
            pixel_height: config.pixel_height,
        };
        let session = tokio::task::spawn_blocking(move || backend.spawn(&command, size)).await??;
        let reader = match session.try_clone_reader() {
            Ok(reader) => reader,
            Err(error) => {
                cleanup_failed_spawn(session).await;
                return Err(error.into());
            }
        };
        let io = match AsyncFd::new(reader) {
            Ok(io) => io,
            Err(error) => {
                cleanup_failed_spawn(session).await;
                return Err(error.into());
            }
        };
        Ok(Self::from_session(
            splint_id,
            incarnation,
            session,
            io,
            config,
        ))
    }

    fn from_session(
        splint_id: SplintId,
        incarnation: ProcessIncarnation,
        session: LinuxPtySession,
        io: AsyncFd<std::fs::File>,
        config: LiveSplintConfig,
    ) -> Self {
        let mut terminal = Terminal::new(
            usize::from(config.columns),
            usize::from(config.rows),
            config.terminal.clone(),
        );
        set_terminal_pixel_geometry(
            &mut terminal,
            config.columns,
            config.rows,
            config.pixel_width,
            config.pixel_height,
        );
        let (sender, receiver) = mpsc::channel(config.command_capacity.max(1));
        let (exit_sender, exit) = watch::channel(None);
        let metrics = Arc::new(RuntimeMetrics::default());
        let handle = LiveSplintHandle {
            splint_id,
            incarnation,
            commands: sender,
            default_snapshot_rows: config.max_scrollback_snapshot_rows,
            default_subscriber_capacity: config.subscriber_capacity,
            max_input_message_bytes: config.input_byte_limit / config.command_capacity.max(1),
            metrics: Arc::clone(&metrics),
            exit,
        };
        let task = tokio::spawn(run_actor(
            splint_id,
            incarnation,
            session,
            io,
            terminal,
            receiver,
            config,
            metrics,
            exit_sender,
        ));
        Self { handle, task }
    }

    #[must_use]
    pub fn handle(&self) -> LiveSplintHandle {
        self.handle.clone()
    }

    pub async fn shutdown(self) -> Result<ProcessExit, LiveError> {
        let request = self.handle.shutdown().await;
        let task = self.task.await?;
        if task.is_ok() {
            return task;
        }
        request?;
        task
    }

    pub async fn wait(self) -> Result<ProcessExit, LiveError> {
        self.task.await?
    }
}

struct Subscriber {
    events: mpsc::Sender<LiveEvent>,
    resnapshot: watch::Sender<bool>,
}

#[derive(Default)]
struct WriteQueue {
    chunks: VecDeque<Vec<u8>>,
    offset: usize,
    bytes: usize,
}

impl WriteQueue {
    fn push(&mut self, bytes: Vec<u8>, limit: usize) -> Result<(), LiveError> {
        if bytes.len() > limit.saturating_sub(self.bytes) {
            return Err(LiveError::InputQueueFull);
        }
        self.bytes += bytes.len();
        if !bytes.is_empty() {
            self.chunks.push_back(bytes);
        }
        Ok(())
    }

    fn front(&self) -> Option<&[u8]> {
        self.chunks.front().map(|chunk| &chunk[self.offset..])
    }

    fn consume(&mut self, count: usize) {
        self.bytes -= count;
        self.offset += count;
        if self
            .chunks
            .front()
            .is_some_and(|chunk| self.offset == chunk.len())
        {
            self.chunks.pop_front();
            self.offset = 0;
        }
    }

    fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }
}

#[derive(Clone, Copy)]
enum ShutdownStage {
    Hangup(Instant),
    Terminate(Instant),
    Kill,
}

#[allow(
    clippy::too_many_arguments,
    reason = "the actor exclusively owns its runtime state"
)]
async fn run_actor(
    splint_id: SplintId,
    incarnation: ProcessIncarnation,
    mut session: LinuxPtySession,
    io: AsyncFd<std::fs::File>,
    terminal: Terminal,
    commands: mpsc::Receiver<Command>,
    config: LiveSplintConfig,
    metrics: Arc<RuntimeMetrics>,
    exit_sender: watch::Sender<Option<ProcessExit>>,
) -> Result<ProcessExit, LiveError> {
    let result = run_actor_body(
        splint_id,
        incarnation,
        &mut session,
        io,
        terminal,
        commands,
        config,
        &metrics,
    )
    .await;
    let forced_status = if result.is_err() {
        force_reap(&mut session).await
    } else {
        None
    };
    if let Some(status) = result.as_ref().ok().copied().or(forced_status) {
        exit_sender.send_replace(Some(status));
    }
    result
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the actor loop keeps ownership and serialized readiness transitions together"
)]
async fn run_actor_body(
    splint_id: SplintId,
    incarnation: ProcessIncarnation,
    session: &mut LinuxPtySession,
    io: AsyncFd<std::fs::File>,
    mut terminal: Terminal,
    mut commands: mpsc::Receiver<Command>,
    config: LiveSplintConfig,
    metrics: &RuntimeMetrics,
) -> Result<ProcessExit, LiveError> {
    let mut subscribers = Vec::<Subscriber>::new();
    let mut user_writes = WriteQueue::default();
    let mut reply_writes = WriteQueue::default();
    let mut interval = time::interval(config.poll_interval);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut child_exit = None;
    let mut eof = false;
    let mut drain_deadline = None;
    let mut shutdown = None;
    let mut shutdown_replies = Vec::new();
    let mut read_buffer = vec![0_u8; READ_BUFFER];
    let mut commands_open = true;

    loop {
        let shutdown_settled = shutdown.is_none() || matches!(shutdown, Some(ShutdownStage::Kill));
        if child_exit.is_some()
            && (eof
                || (shutdown_settled
                    && drain_deadline.is_some_and(|deadline| Instant::now() >= deadline)))
        {
            break;
        }

        tokio::select! {
            command = commands.recv(), if commands_open => {
                if let Some(command) = command {
                    handle_command(
                        command,
                        splint_id,
                        incarnation,
                        session,
                        &mut terminal,
                        &mut subscribers,
                        &mut user_writes,
                        &mut shutdown,
                        &mut shutdown_replies,
                        &config,
                        metrics,
                        child_exit,
                    );
                    RuntimeMetrics::observe_max(
                        &metrics.user_write_queue_high_water_bytes,
                        user_writes.bytes,
                    );
                } else {
                    commands_open = false;
                    if shutdown.is_none() {
                        let _ = session.signal_process_group(PtySignal::Hangup);
                        shutdown = Some(ShutdownStage::Hangup(Instant::now() + config.hangup_grace));
                    }
                }
            }
            ready = io.readable(), if !eof => {
                let mut ready = ready?;
                let result = ready.try_io(|inner| inner.get_ref().read(&mut read_buffer));
                if let Ok(result) = result {
                    match result {
                        Ok(0) => eof = true,
                        Ok(count) => {
                            metrics.pty_read_calls.fetch_add(1, Ordering::Relaxed);
                            metrics.pty_read_bytes.fetch_add(
                                u64::try_from(count).unwrap_or(u64::MAX),
                                Ordering::Relaxed,
                            );
                            let started = Instant::now();
                            let output = process_output(
                                &read_buffer[..count],
                                incarnation,
                                &mut terminal,
                                &mut reply_writes,
                                &mut subscribers,
                                config.reply_byte_limit,
                            )?;
                            metrics.output_parse_batches.fetch_add(
                                output.parse_batches,
                                Ordering::Relaxed,
                            );
                            metrics.output_terminal_updates.fetch_add(
                                output.terminal_updates,
                                Ordering::Relaxed,
                            );
                            metrics.output_live_events.fetch_add(
                                output.live_events,
                                Ordering::Relaxed,
                            );
                            metrics.output_subscriber_overflows.fetch_add(
                                output.subscriber_overflows,
                                Ordering::Relaxed,
                            );
                            metrics.output_processing_ns.fetch_add(
                                u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
                                Ordering::Relaxed,
                            );
                            RuntimeMetrics::observe_max(
                                &metrics.reply_write_queue_high_water_bytes,
                                reply_writes.bytes,
                            );
                            if child_exit.is_some() {
                                drain_deadline = Some(Instant::now() + config.exit_drain_timeout);
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        Err(error) if error.raw_os_error() == Some(5) => eof = true,
                        Err(error) => return Err(error.into()),
                    }
                }
            }
            ready = io.writable(), if !reply_writes.is_empty() || !user_writes.is_empty() => {
                let mut ready = ready?;
                let queue = if reply_writes.is_empty() {
                    &mut user_writes
                } else {
                    &mut reply_writes
                };
                if let Some(bytes) = queue.front() {
                    let result = ready.try_io(|inner| inner.get_ref().write(bytes));
                    if let Ok(result) = result {
                        match result {
                            Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "PTY write returned zero").into()),
                            Ok(count) => queue.consume(count),
                            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                            Err(error) => return Err(error.into()),
                        }
                    }
                }
            }
            _ = interval.tick() => {
                if child_exit.is_none() {
                    if let Some(status) = session.try_wait()? {
                        child_exit = Some(status.into());
                        drain_deadline = Some(Instant::now() + config.exit_drain_timeout);
                    }
                }
                advance_shutdown(session, &mut shutdown, &config);
            }
        }
    }

    let status = child_exit.expect("actor only completes after observing child exit");
    publish(
        &mut subscribers,
        LiveEvent::Exited {
            incarnation,
            status,
        },
    );
    for reply in shutdown_replies {
        let _ = reply.send(());
    }
    Ok(status)
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "command application is the actor's serialization point"
)]
fn handle_command(
    command: Command,
    splint_id: SplintId,
    incarnation: ProcessIncarnation,
    session: &mut LinuxPtySession,
    terminal: &mut Terminal,
    subscribers: &mut Vec<Subscriber>,
    writes: &mut WriteQueue,
    shutdown: &mut Option<ShutdownStage>,
    shutdown_replies: &mut Vec<oneshot::Sender<()>>,
    config: &LiveSplintConfig,
    metrics: &RuntimeMetrics,
    child_exit: Option<ProcessExit>,
) {
    match command {
        Command::Input(bytes, reply) => {
            let result = if child_exit.is_some() {
                Err(LiveError::ProcessExited)
            } else {
                writes.push(bytes, config.input_byte_limit)
            };
            let _ = reply.send(result);
        }
        Command::Resize(size, reply) => {
            let result = if child_exit.is_some() {
                Err(LiveError::ProcessExited)
            } else {
                validate_dimensions(size.columns, size.rows).and_then(|()| {
                    session.resize(size)?;
                    Ok(())
                })
            };
            if result.is_ok() {
                let base = terminal.revision();
                set_terminal_pixel_geometry(
                    terminal,
                    size.columns,
                    size.rows,
                    size.pixel_width,
                    size.pixel_height,
                );
                terminal.resize(usize::from(size.columns), usize::from(size.rows));
                publish_updates(terminal, base, incarnation, subscribers);
            }
            let _ = reply.send(result);
        }
        Command::Snapshot(max_rows, reply) => {
            let started = Instant::now();
            let snapshot = owned_snapshot(
                splint_id,
                incarnation,
                terminal,
                max_rows.min(config.max_scrollback_snapshot_rows),
                child_exit,
            );
            metrics.snapshot_builds.fetch_add(1, Ordering::Relaxed);
            metrics.snapshot_build_ns.fetch_add(
                u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
                Ordering::Relaxed,
            );
            let _ = reply.send(Ok(snapshot));
        }
        Command::ImageContent(content_id, generation, digest, reply) => {
            let _ = reply.send(resolve_image_content(
                terminal, content_id, generation, digest,
            ));
        }
        Command::ScrollbackPage(before_row_id, max_rows, reply) => {
            let snapshot = terminal.snapshot(SnapshotRequest {
                max_scrollback_rows: 1,
            });
            let scrollback = snapshot.scrollback();
            let before_row_id = match (before_row_id, scrollback.newest_available_row_id) {
                (Some(before_row_id), _) => before_row_id,
                (None, Some(newest_row_id)) => {
                    let Some(before_row_id) = newest_row_id.checked_add(1) else {
                        let _ = reply.send(Err(LiveError::RowIdentityExhausted));
                        return;
                    };
                    before_row_id
                }
                (None, None) => 1,
            };
            let page = terminal.scrollback_page(
                before_row_id,
                max_rows.min(config.max_scrollback_snapshot_rows),
            );
            let _ = reply.send(Ok(LiveScrollbackPage {
                terminal_revision: page.terminal_revision,
                history_generation: page.history_generation,
                title: snapshot.title().to_owned(),
                oldest_available_row_id: scrollback.oldest_available_row_id,
                newest_available_row_id: scrollback.newest_available_row_id,
                rows: page.rows.into_iter().map(owned_row).collect(),
                has_older: page.has_older,
            }));
        }
        Command::Search(query, case_sensitive, skip_rows, maximum_results, deadline, reply) => {
            let snapshot = terminal.snapshot(SnapshotRequest {
                max_scrollback_rows: 0,
            });
            let title = snapshot.title().to_owned();
            let page = terminal.search_normal(
                &query,
                case_sensitive,
                skip_rows,
                maximum_results,
                deadline,
            );
            let _ = reply.send(Ok(LiveSearchPage {
                terminal_revision: page.terminal_revision,
                history_generation: page.history_generation,
                title,
                page,
            }));
        }
        Command::Subscribe(capacity, reply) => {
            subscribers.retain(|subscriber| !subscriber.events.is_closed());
            if capacity == 0 {
                let _ = reply.send(Err(LiveError::InvalidSubscriberCapacity));
                return;
            }
            if subscribers.len() >= config.max_subscribers {
                let _ = reply.send(Err(LiveError::InvalidSubscriberCapacity));
                return;
            }
            let (event_sender, events) = mpsc::channel(capacity.min(config.subscriber_capacity));
            let (resnapshot, resnapshot_receiver) = watch::channel(false);
            subscribers.push(Subscriber {
                events: event_sender,
                resnapshot,
            });
            let _ = reply.send(Ok(Subscription {
                events,
                resnapshot: resnapshot_receiver,
            }));
        }
        Command::Attach(max_rows, capacity, reply) => {
            subscribers.retain(|subscriber| !subscriber.events.is_closed());
            if capacity == 0 || subscribers.len() >= config.max_subscribers {
                let _ = reply.send(Err(LiveError::InvalidSubscriberCapacity));
                return;
            }
            let started = Instant::now();
            let snapshot = owned_snapshot(
                splint_id,
                incarnation,
                terminal,
                max_rows.min(config.max_scrollback_snapshot_rows),
                child_exit,
            );
            metrics.snapshot_builds.fetch_add(1, Ordering::Relaxed);
            metrics.snapshot_build_ns.fetch_add(
                u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
                Ordering::Relaxed,
            );
            let (event_sender, events) = mpsc::channel(capacity.min(config.subscriber_capacity));
            let (resnapshot, resnapshot_receiver) = watch::channel(false);
            subscribers.push(Subscriber {
                events: event_sender,
                resnapshot,
            });
            let subscription = Subscription {
                events,
                resnapshot: resnapshot_receiver,
            };
            let _ = reply.send(Ok((snapshot, subscription)));
        }
        Command::Shutdown(reply) => {
            shutdown_replies.push(reply);
            if shutdown.is_none() {
                let _ = session.signal_process_group(PtySignal::Hangup);
                *shutdown = Some(ShutdownStage::Hangup(Instant::now() + config.hangup_grace));
            }
        }
    }
}

fn publish_updates(
    terminal: &Terminal,
    base: TerminalRevision,
    incarnation: ProcessIncarnation,
    subscribers: &mut Vec<Subscriber>,
) -> (usize, usize) {
    let updates = terminal
        .updates_since(base)
        .expect("immediate update history cannot have a revision gap")
        .updates()
        .cloned()
        .collect::<Vec<_>>();
    let update_count = updates.len();
    let overflows = if updates.is_empty() {
        0
    } else {
        publish(
            subscribers,
            LiveEvent::Update {
                incarnation,
                updates,
            },
        )
    };
    (update_count, overflows)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ProcessOutputMetrics {
    parse_batches: u64,
    terminal_updates: u64,
    live_events: u64,
    subscriber_overflows: u64,
}

fn set_terminal_pixel_geometry(
    terminal: &mut Terminal,
    columns: u16,
    rows: u16,
    pixel_width: u16,
    pixel_height: u16,
) {
    let cell_width = if pixel_width == 0 {
        0
    } else {
        u32::from(pixel_width) / u32::from(columns)
    };
    let cell_height = if pixel_height == 0 {
        0
    } else {
        u32::from(pixel_height) / u32::from(rows)
    };
    terminal.set_cell_pixel_size(cell_width, cell_height);
}

fn process_output(
    bytes: &[u8],
    incarnation: ProcessIncarnation,
    terminal: &mut Terminal,
    reply_writes: &mut WriteQueue,
    subscribers: &mut Vec<Subscriber>,
    reply_limit: usize,
) -> Result<ProcessOutputMetrics, LiveError> {
    let mut metrics = ProcessOutputMetrics::default();
    for batch in bytes.chunks(PARSE_BATCH) {
        metrics.parse_batches = metrics.parse_batches.saturating_add(1);
        let base = terminal.revision();
        terminal.advance(batch);
        let (updates, overflows) = publish_updates(terminal, base, incarnation, subscribers);
        metrics.terminal_updates = metrics
            .terminal_updates
            .saturating_add(u64::try_from(updates).unwrap_or(u64::MAX));
        metrics.live_events = metrics.live_events.saturating_add(u64::from(updates > 0));
        metrics.subscriber_overflows = metrics
            .subscriber_overflows
            .saturating_add(u64::try_from(overflows).unwrap_or(u64::MAX));
        for event in terminal.drain_events() {
            if let TerminalEvent::PtyWrite(bytes) = event {
                reply_writes
                    .push(bytes, reply_limit)
                    .map_err(|_| LiveError::ReplyQueueFull)?;
            }
        }
    }
    Ok(metrics)
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "fanout takes ownership of the event and clones only for retained subscribers"
)]
fn publish(subscribers: &mut Vec<Subscriber>, event: LiveEvent) -> usize {
    let mut overflows = 0_usize;
    subscribers.retain(
        |subscriber| match subscriber.events.try_send(event.clone()) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                overflows = overflows.saturating_add(1);
                subscriber.resnapshot.send_replace(true);
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        },
    );
    overflows
}

fn advance_shutdown(
    session: &LinuxPtySession,
    shutdown: &mut Option<ShutdownStage>,
    config: &LiveSplintConfig,
) {
    let now = Instant::now();
    match *shutdown {
        Some(ShutdownStage::Hangup(deadline)) if now >= deadline => {
            let _ = session.signal_process_group(PtySignal::Terminate);
            *shutdown = Some(ShutdownStage::Terminate(now + config.terminate_grace));
        }
        Some(ShutdownStage::Terminate(deadline)) if now >= deadline => {
            let _ = session.signal_process_group(PtySignal::Kill);
            *shutdown = Some(ShutdownStage::Kill);
        }
        _ => {}
    }
}

fn resolve_image_content(
    terminal: &Terminal,
    content_id: ImageContentId,
    generation: u64,
    digest: [u8; 32],
) -> Result<ImageContent, LiveError> {
    let content = terminal
        .image_content(content_id)
        .ok_or(LiveError::ImageContentNotFound)?;
    let metadata = content.metadata();
    if metadata.generation != generation || metadata.digest != digest {
        return Err(LiveError::StaleImageContent);
    }
    Ok(content.clone())
}

fn owned_snapshot(
    splint_id: SplintId,
    incarnation: ProcessIncarnation,
    terminal: &Terminal,
    max_rows: usize,
    exited: Option<ProcessExit>,
) -> LiveSnapshot {
    let snapshot = terminal.snapshot(SnapshotRequest {
        max_scrollback_rows: max_rows,
    });
    LiveSnapshot {
        splint_id,
        incarnation,
        revision: snapshot.revision(),
        dimensions: snapshot.dimensions(),
        active_screen: snapshot.active_screen(),
        cursor: snapshot.cursor(),
        modes: snapshot.modes(),
        scroll_region: snapshot.scroll_region(),
        view_follows_live: snapshot.view_follows_live(),
        title: snapshot.title().to_owned(),
        palette: *snapshot.palette(),
        default_colors: *snapshot.default_colors(),
        image_contents: snapshot.image_contents().collect(),
        image_placements: snapshot.image_placements().collect(),
        visible_rows: snapshot.visible_rows().map(owned_row).collect(),
        scrollback_rows: snapshot.scrollback_rows().map(owned_row).collect(),
        scrollback: snapshot.scrollback(),
        exited,
    }
}

fn owned_row(row: splinterm_terminal::RowSnapshot<'_>) -> LiveRow {
    LiveRow {
        row_id: row.id(),
        linebreak: row.linebreak(),
        cells: row
            .cells()
            .map(|cell| {
                let (content, spacer_remaining) = match cell.content() {
                    CellSnapshotContent::Empty => (String::new(), None),
                    CellSnapshotContent::Scalar(character) => (character.to_string(), None),
                    CellSnapshotContent::Composed(characters) => {
                        (characters.iter().collect(), None)
                    }
                    CellSnapshotContent::Spacer { remaining } => (String::new(), Some(remaining)),
                };
                LiveCell {
                    content,
                    spacer_remaining,
                    attributes: cell.attributes(),
                }
            })
            .collect(),
    }
}

async fn cleanup_failed_spawn(mut session: LinuxPtySession) {
    let _ = session.signal_process_group(PtySignal::Kill);
    let _ = tokio::task::spawn_blocking(move || session.wait()).await;
}

async fn force_reap(session: &mut LinuxPtySession) -> Option<ProcessExit> {
    let _ = session.signal_process_group(PtySignal::Kill);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match session.try_wait() {
            Ok(Some(status)) => return Some(status.into()),
            Err(_) => return None,
            Ok(None) => time::sleep(Duration::from_millis(5)).await,
        }
    }
    None
}

fn validate_dimensions(columns: u16, rows: u16) -> Result<(), LiveError> {
    if columns == 0 || rows == 0 {
        Err(LiveError::InvalidDimensions)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn owned_snapshot_retains_image_metadata_without_pixel_bodies() {
        let mut terminal = Terminal::new(4, 2, TerminalConfig::default());
        terminal.set_cell_pixel_size(8, 16);
        terminal.advance(b"\x1bPq#1;2;100;0;0#1~\x1b\\");

        let snapshot = owned_snapshot(
            SplintId::new(),
            ProcessIncarnation::allocate(),
            &terminal,
            0,
            None,
        );
        assert_eq!(snapshot.image_contents.len(), 1);
        assert_eq!(snapshot.image_placements.len(), 1);
        let metadata = snapshot.image_contents[0];
        assert_eq!(
            metadata.byte_charge,
            usize::try_from(metadata.width).unwrap()
                * usize::try_from(metadata.height).unwrap()
                * 4
        );
        assert_eq!(
            snapshot.image_placements[0].content_id,
            snapshot.image_contents[0].id
        );

        let exact =
            resolve_image_content(&terminal, metadata.id, metadata.generation, metadata.digest)
                .expect("exact image identity");
        let repeated =
            resolve_image_content(&terminal, metadata.id, metadata.generation, metadata.digest)
                .expect("repeated immutable image identity");
        assert!(std::ptr::eq(
            exact.pixels().as_ptr(),
            repeated.pixels().as_ptr()
        ));
        assert!(matches!(
            resolve_image_content(
                &terminal,
                metadata.id,
                metadata.generation + 1,
                metadata.digest,
            ),
            Err(LiveError::StaleImageContent)
        ));
        assert!(matches!(
            resolve_image_content(
                &terminal,
                ImageContentId::new(u64::MAX).unwrap(),
                metadata.generation,
                metadata.digest,
            ),
            Err(LiveError::ImageContentNotFound)
        ));
    }

    #[tokio::test]
    async fn resnapshot_state_wins_over_an_already_queued_event() {
        let (event_tx, events) = mpsc::channel(1);
        event_tx
            .send(LiveEvent::Exited {
                incarnation: ProcessIncarnation::allocate(),
                status: ProcessExit {
                    code: Some(0),
                    signal: None,
                },
            })
            .await
            .unwrap();
        let (resnapshot_tx, resnapshot) = watch::channel(false);
        resnapshot_tx.send(true).unwrap();
        let mut subscription = Subscription { events, resnapshot };

        assert!(matches!(
            subscription.recv().await,
            SubscriptionReceive::ResnapshotRequired
        ));
    }

    fn backend() -> LinuxPtyBackend {
        let test_binary = std::env::current_exe().unwrap();
        let debug_directory = test_binary.parent().unwrap().parent().unwrap();
        let helper = debug_directory.join("splinterm-pty-child");
        assert!(
            helper.is_file(),
            "build the workspace helper before running splinterd tests: {}",
            helper.display()
        );
        LinuxPtyBackend::new(helper)
    }

    fn shell(script: &str) -> PtyCommand {
        PtyCommand::new("/bin/sh", PathBuf::from("/tmp")).args(["-c", script])
    }

    fn fast_config() -> LiveSplintConfig {
        LiveSplintConfig {
            columns: 40,
            rows: 6,
            hangup_grace: Duration::from_millis(30),
            terminate_grace: Duration::from_millis(30),
            poll_interval: Duration::from_millis(5),
            exit_drain_timeout: Duration::from_millis(50),
            ..LiveSplintConfig::default()
        }
    }

    fn snapshot_text(snapshot: &LiveSnapshot) -> String {
        snapshot
            .visible_rows
            .iter()
            .flat_map(|row| row.cells.iter())
            .map(|cell| cell.content.as_str())
            .collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn actor_resolves_only_exact_immutable_image_content() {
        let mut config = fast_config();
        config.pixel_width = 320;
        config.pixel_height = 96;
        let runtime = LiveSplintRuntime::spawn(
            SplintId::new(),
            backend(),
            shell("printf '\\033Pq#1;2;100;0;0#1~\\033\\\\'; sleep 0.2"),
            config,
        )
        .await
        .unwrap();
        let handle = runtime.handle();
        time::sleep(Duration::from_millis(50)).await;
        let snapshot = handle.snapshot().await.unwrap();
        let metadata = snapshot.image_contents[0];
        let content = handle
            .image_content(metadata.id, metadata.generation, metadata.digest)
            .await
            .unwrap();
        assert_eq!(content.metadata(), metadata);
        assert!(matches!(
            handle
                .image_content(metadata.id, metadata.generation + 1, metadata.digest)
                .await,
            Err(LiveError::StaleImageContent)
        ));
        assert!(matches!(
            handle
                .image_content(
                    ImageContentId::new(u64::MAX).unwrap(),
                    metadata.generation,
                    metadata.digest,
                )
                .await,
            Err(LiveError::ImageContentNotFound)
        ));
        runtime.wait().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn detached_actor_keeps_consuming_and_snapshots_current_state() {
        let runtime = LiveSplintRuntime::spawn(
            SplintId::new(),
            backend(),
            shell("printf detached-marker; sleep 0.2"),
            fast_config(),
        )
        .await
        .unwrap();
        let handle = runtime.handle();
        time::sleep(Duration::from_millis(50)).await;
        let snapshot = handle.snapshot().await.unwrap();
        assert!(snapshot_text(&snapshot).contains("detached-marker"));
        assert!(snapshot.revision.value() > 0);
        assert_eq!(runtime.wait().await.unwrap().code, Some(0));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn input_resize_and_subscriber_overflow_do_not_block_the_actor() {
        let mut config = fast_config();
        config.subscriber_capacity = 1;
        let runtime = LiveSplintRuntime::spawn(
            SplintId::new(),
            backend(),
            shell("read value; printf '%s' \"$value\"; sleep 0.2"),
            config,
        )
        .await
        .unwrap();
        let handle = runtime.handle();
        let mut slow = handle.subscribe().await.unwrap();
        handle.input(b"ordered-input\n".to_vec()).await.unwrap();
        handle.resize(PtySize::cells(50, 8)).await.unwrap();
        time::sleep(Duration::from_millis(50)).await;
        let snapshot = handle.snapshot().await.unwrap();
        assert_eq!(
            snapshot.dimensions,
            Dimensions {
                columns: 50,
                rows: 8
            }
        );
        assert!(snapshot_text(&snapshot).contains("ordered-input"));
        assert!(slow.changed().await);
        assert_eq!(runtime.wait().await.unwrap().code, Some(0));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn atomic_attach_starts_updates_after_snapshot_revision() {
        let runtime = LiveSplintRuntime::spawn(
            SplintId::new(),
            backend(),
            shell("read value; printf '%s' \"$value\"; sleep 0.2"),
            fast_config(),
        )
        .await
        .unwrap();
        let handle = runtime.handle();
        let (snapshot, mut subscription) = handle.attach().await.unwrap();
        handle.input(b"after-attach\n".to_vec()).await.unwrap();
        let event = time::timeout(Duration::from_secs(1), subscription.recv())
            .await
            .unwrap();
        let SubscriptionReceive::Event(LiveEvent::Update { updates, .. }) = event else {
            panic!("expected an ordered terminal update batch")
        };
        assert!(!updates.is_empty());
        assert!(updates.last().unwrap().revision() > snapshot.revision);
        handle.snapshot().await.unwrap();
        let metrics = handle.metrics();
        assert!(metrics.command_queue_high_water >= 1);
        assert!(metrics.user_write_queue_high_water_bytes >= b"after-attach\n".len());
        assert!(metrics.pty_read_calls > 0);
        assert!(metrics.pty_read_bytes > 0);
        assert!(metrics.output_parse_batches > 0);
        assert!(metrics.output_terminal_updates > 0);
        assert!(metrics.output_live_events > 0);
        assert!(metrics.output_processing_ns > 0);
        assert!(metrics.snapshot_builds > 0);
        assert!(metrics.snapshot_build_ns > 0);
        runtime.wait().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resize_publishes_the_committed_terminal_update() {
        let runtime =
            LiveSplintRuntime::spawn(SplintId::new(), backend(), shell("sleep 1"), fast_config())
                .await
                .unwrap();
        let handle = runtime.handle();
        let mut subscription = handle.subscribe_with_capacity(8).await.unwrap();
        handle.resize(PtySize::cells(60, 10)).await.unwrap();
        let event = time::timeout(Duration::from_secs(1), subscription.events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(event, LiveEvent::Update { .. }));
        runtime.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_runtime_still_closes_channel_and_reaps_child() {
        let runtime = LiveSplintRuntime::spawn(
            SplintId::new(),
            backend(),
            shell("printf '%s' $$; sleep 30"),
            fast_config(),
        )
        .await
        .unwrap();
        let handle = runtime.handle();
        time::sleep(Duration::from_millis(50)).await;
        let text = snapshot_text(&handle.snapshot().await.unwrap());
        let pid = text
            .split_whitespace()
            .next()
            .unwrap()
            .parse::<u32>()
            .unwrap();
        drop(handle);
        drop(runtime);
        let process = PathBuf::from(format!("/proc/{pid}"));
        time::timeout(Duration::from_secs(2), async {
            while process.exists() {
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_escalates_after_group_leader_exits() {
        let runtime = LiveSplintRuntime::spawn(
            SplintId::new(),
            backend(),
            shell("(trap '' HUP TERM; while :; do printf x; sleep 0.01; done) & exit 0"),
            fast_config(),
        )
        .await
        .unwrap();
        time::sleep(Duration::from_millis(80)).await;
        let status = time::timeout(Duration::from_secs(2), runtime.shutdown())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status.code, Some(0));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_escalates_and_reaps_the_process() {
        let runtime = LiveSplintRuntime::spawn(
            SplintId::new(),
            backend(),
            shell("trap '' HUP TERM; printf ready; while :; do sleep 1; done"),
            fast_config(),
        )
        .await
        .unwrap();
        time::sleep(Duration::from_millis(50)).await;
        let status = time::timeout(Duration::from_secs(2), runtime.shutdown())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status.signal, Some(9));
    }

    #[test]
    fn write_queue_is_byte_bounded_and_preserves_partial_chunks() {
        let mut queue = WriteQueue::default();
        queue.push(vec![1, 2, 3], 5).unwrap();
        queue.push(vec![4, 5], 5).unwrap();
        assert_eq!(queue.front(), Some([1, 2, 3].as_slice()));
        queue.consume(2);
        assert_eq!(queue.front(), Some([3].as_slice()));
        queue.consume(1);
        assert_eq!(queue.front(), Some([4, 5].as_slice()));
        assert!(matches!(
            queue.push(vec![6, 7, 8, 9], 5),
            Err(LiveError::InputQueueFull)
        ));
    }
}
