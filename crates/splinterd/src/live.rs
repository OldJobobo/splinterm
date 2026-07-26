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
const SYNCHRONIZED_UPDATE_TIMEOUT: Duration = Duration::from_secs(1);
const SYNCHRONIZED_FRAME_INTERVAL: Duration = Duration::from_millis(33);
const MAX_SUBSCRIBER_QUEUE_CAPACITY: usize = 1_048_576;

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
        snapshot: Box<LiveSnapshot>,
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
                } else if changed.is_err() {
                    self.events.try_recv().map_or(
                        SubscriptionReceive::Closed,
                        SubscriptionReceive::Event,
                    )
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
    Subscribe(usize, usize, Reply<Subscription>),
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
        self.request(|reply| Command::Subscribe(capacity, self.default_snapshot_rows, reply))
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
    published_revision: TerminalRevision,
    snapshot_rows: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SynchronizedPublication {
    published_revision: TerminalRevision,
    active: bool,
    timed_out: bool,
    deadline: Option<Instant>,
    next_frame_at: Option<Instant>,
}

impl SynchronizedPublication {
    fn new(revision: TerminalRevision) -> Self {
        Self {
            published_revision: revision,
            active: false,
            timed_out: false,
            deadline: None,
            next_frame_at: None,
        }
    }

    fn observe(&mut self, active: bool, now: Instant) {
        if active && !self.active {
            self.timed_out = false;
            self.deadline = Some(now + SYNCHRONIZED_UPDATE_TIMEOUT);
        } else if !active {
            self.timed_out = false;
            self.deadline = None;
        }
        self.active = active;
    }

    fn expire(&mut self) {
        self.timed_out = true;
        self.deadline = None;
    }

    fn should_publish_frame(&mut self, now: Instant) -> bool {
        if self.next_frame_at.is_some_and(|deadline| now < deadline) {
            return false;
        }
        self.next_frame_at = Some(now + SYNCHRONIZED_FRAME_INTERVAL);
        true
    }
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
    let mut publication = SynchronizedPublication::new(terminal.revision());
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
                        &mut publication,
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
                                splint_id,
                                incarnation,
                                child_exit,
                                &mut terminal,
                                &mut reply_writes,
                                &mut subscribers,
                                &mut publication,
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
            () = time::sleep_until(publication.deadline.unwrap_or_else(Instant::now)), if publication.deadline.is_some() => {
                terminal.expire_synchronized_updates();
                publication.observe(false, Instant::now());
                publication.expire();
                publish_updates(
                    splint_id,
                    &terminal,
                    &mut publication,
                    incarnation,
                    child_exit,
                    &mut subscribers,
                );
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
    terminal.expire_synchronized_updates();
    publication.observe(false, Instant::now());
    publication.expire();
    publish_updates(
        splint_id,
        &terminal,
        &mut publication,
        incarnation,
        Some(status),
        &mut subscribers,
    );
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

fn subscriber_channel_capacity(requested: usize, configured: usize) -> Result<usize, LiveError> {
    let effective = requested.min(configured);
    if effective == 0 || effective > MAX_SUBSCRIBER_QUEUE_CAPACITY {
        return Err(LiveError::InvalidSubscriberCapacity);
    }
    effective
        .checked_add(1)
        .ok_or(LiveError::InvalidSubscriberCapacity)
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
    publication: &mut SynchronizedPublication,
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
                set_terminal_pixel_geometry(
                    terminal,
                    size.columns,
                    size.rows,
                    size.pixel_width,
                    size.pixel_height,
                );
                terminal.resize(usize::from(size.columns), usize::from(size.rows));
                publication.observe(terminal.synchronized_updates(), Instant::now());
                publish_updates(
                    splint_id,
                    terminal,
                    publication,
                    incarnation,
                    child_exit,
                    subscribers,
                );
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
        Command::Subscribe(capacity, max_rows, reply) => {
            subscribers.retain(|subscriber| !subscriber.events.is_closed());
            let Ok(event_capacity) =
                subscriber_channel_capacity(capacity, config.subscriber_capacity)
            else {
                let _ = reply.send(Err(LiveError::InvalidSubscriberCapacity));
                return;
            };
            if subscribers.len() >= config.max_subscribers {
                let _ = reply.send(Err(LiveError::InvalidSubscriberCapacity));
                return;
            }
            let (event_sender, events) = mpsc::channel(event_capacity);
            let (resnapshot, resnapshot_receiver) = watch::channel(false);
            subscribers.push(Subscriber {
                events: event_sender,
                resnapshot,
                published_revision: terminal.revision(),
                snapshot_rows: max_rows.min(config.max_scrollback_snapshot_rows),
            });
            let _ = reply.send(Ok(Subscription {
                events,
                resnapshot: resnapshot_receiver,
            }));
        }
        Command::Attach(max_rows, capacity, reply) => {
            subscribers.retain(|subscriber| !subscriber.events.is_closed());
            let Ok(event_capacity) =
                subscriber_channel_capacity(capacity, config.subscriber_capacity)
            else {
                let _ = reply.send(Err(LiveError::InvalidSubscriberCapacity));
                return;
            };
            if subscribers.len() >= config.max_subscribers {
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
            let (event_sender, events) = mpsc::channel(event_capacity);
            let (resnapshot, resnapshot_receiver) = watch::channel(false);
            subscribers.push(Subscriber {
                events: event_sender,
                resnapshot,
                published_revision: terminal.revision(),
                snapshot_rows: max_rows.min(config.max_scrollback_snapshot_rows),
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
    splint_id: SplintId,
    terminal: &Terminal,
    publication: &mut SynchronizedPublication,
    incarnation: ProcessIncarnation,
    child_exit: Option<ProcessExit>,
    subscribers: &mut Vec<Subscriber>,
) -> (usize, usize) {
    let update_count = terminal
        .updates_since(publication.published_revision)
        .map_or(0, |batch| batch.updates().len());
    publication.published_revision = terminal.revision();

    let mut overflows = 0_usize;
    subscribers.retain_mut(|subscriber| {
        if subscriber.events.is_closed() {
            return false;
        }
        let Ok(batch) = terminal.updates_since(subscriber.published_revision) else {
            subscriber.resnapshot.send_replace(true);
            overflows = overflows.saturating_add(1);
            return false;
        };
        let updates = batch.updates().cloned().collect::<Vec<_>>();
        if updates.is_empty() {
            return true;
        }
        // One internal slot is reserved for the terminal Exited event.
        if subscriber.events.capacity() <= 1 {
            subscriber.resnapshot.send_replace(true);
            overflows = overflows.saturating_add(1);
            return false;
        }
        let snapshot = owned_snapshot(
            splint_id,
            incarnation,
            terminal,
            subscriber.snapshot_rows,
            child_exit,
        );
        match subscriber.events.try_send(LiveEvent::Update {
            incarnation,
            updates,
            snapshot: Box::new(snapshot),
        }) {
            Ok(()) => {
                subscriber.published_revision = terminal.revision();
                true
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                subscriber.resnapshot.send_replace(true);
                overflows = overflows.saturating_add(1);
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    });
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

#[allow(
    clippy::too_many_arguments,
    reason = "PTY parsing, immutable frame capture, replies, and subscriber publication form one actor transaction"
)]
fn process_output(
    bytes: &[u8],
    splint_id: SplintId,
    incarnation: ProcessIncarnation,
    child_exit: Option<ProcessExit>,
    terminal: &mut Terminal,
    reply_writes: &mut WriteQueue,
    subscribers: &mut Vec<Subscriber>,
    publication: &mut SynchronizedPublication,
    reply_limit: usize,
) -> Result<ProcessOutputMetrics, LiveError> {
    let mut metrics = ProcessOutputMetrics::default();
    for batch in bytes.chunks(PARSE_BATCH) {
        metrics.parse_batches = metrics.parse_batches.saturating_add(1);
        let image_metrics_before = terminal.image_metrics();
        let parse_started = Instant::now();
        let mut remaining = batch;
        while !remaining.is_empty() {
            let revision_before = terminal.revision();
            let (consumed, completed_frame) = terminal.advance_to_synchronized_boundary(remaining);
            debug_assert!(consumed > 0 && consumed <= remaining.len());
            remaining = &remaining[consumed..];
            let now = Instant::now();
            publication.observe(terminal.synchronized_updates(), now);
            let publish_now = if completed_frame {
                publication.should_publish_frame(now)
            } else {
                !terminal.synchronized_updates() && terminal.revision() != revision_before
            };
            let (updates, overflows) = if publish_now {
                publish_updates(
                    splint_id,
                    terminal,
                    publication,
                    incarnation,
                    child_exit,
                    subscribers,
                )
            } else {
                (0, 0)
            };
            metrics.terminal_updates = metrics
                .terminal_updates
                .saturating_add(u64::try_from(updates).unwrap_or(u64::MAX));
            metrics.live_events = metrics.live_events.saturating_add(u64::from(updates > 0));
            metrics.subscriber_overflows = metrics
                .subscriber_overflows
                .saturating_add(u64::try_from(overflows).unwrap_or(u64::MAX));
        }
        if std::env::var_os("SPLINTERM_IMAGE_TRACE").is_some()
            && terminal.image_metrics() != image_metrics_before
        {
            let image_metrics = terminal.image_metrics();
            eprintln!(
                "phase5-image-trace decode_ns={} content_bytes={} content_count={} placement_count={}",
                parse_started.elapsed().as_nanos(),
                image_metrics.content_bytes,
                image_metrics.content_count,
                image_metrics.placement_count,
            );
        }
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

    fn synchronized_test_subscriber(
        terminal: &Terminal,
    ) -> (Subscriber, mpsc::Receiver<LiveEvent>, watch::Receiver<bool>) {
        let (events, receiver) = mpsc::channel(4);
        let (resnapshot, resnapshot_receiver) = watch::channel(false);
        (
            Subscriber {
                events,
                resnapshot,
                published_revision: terminal.revision(),
                snapshot_rows: 0,
            },
            receiver,
            resnapshot_receiver,
        )
    }

    #[test]
    fn synchronized_publication_defers_updates_but_not_pty_replies() {
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        let (subscriber, mut receiver, _) = synchronized_test_subscriber(&terminal);
        let mut subscribers = vec![subscriber];
        let mut publication = SynchronizedPublication::new(terminal.revision());
        let mut replies = WriteQueue::default();

        let metrics = process_output(
            b"\x1b[2026lA\x1b[5n",
            SplintId::new(),
            incarnation,
            None,
            &mut terminal,
            &mut replies,
            &mut subscribers,
            &mut publication,
            1024,
        )
        .unwrap();
        assert!(publication.active && !publication.timed_out);
        assert_eq!(metrics.terminal_updates, 0);
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        assert!(
            !replies.is_empty(),
            "DSR reply must not wait for synchronized rendering"
        );

        process_output(
            b"B\x1b[2026h\x1b\\C",
            SplintId::new(),
            incarnation,
            None,
            &mut terminal,
            &mut replies,
            &mut subscribers,
            &mut publication,
            1024,
        )
        .unwrap();
        assert!(!publication.active);
        let LiveEvent::Update {
            updates, snapshot, ..
        } = receiver.try_recv().unwrap()
        else {
            panic!("expected the completed synchronized frame");
        };
        assert_eq!(updates.len(), 1);
        assert_eq!(snapshot.visible_rows[0].cells[0].content, "A");
        assert_eq!(snapshot.visible_rows[0].cells[1].content, "B");
        assert_eq!(snapshot.visible_rows[0].cells[2].content, "");

        let LiveEvent::Update {
            updates, snapshot, ..
        } = receiver.try_recv().unwrap()
        else {
            panic!("expected trailing normal output");
        };
        assert_eq!(updates.len(), 1);
        assert_eq!(snapshot.visible_rows[0].cells[2].content, "C");
    }

    #[test]
    fn completed_cava_frame_publishes_when_batch_begins_next_frame() {
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        let (subscriber, mut receiver, _) = synchronized_test_subscriber(&terminal);
        let mut subscribers = vec![subscriber];
        let mut publication = SynchronizedPublication::new(terminal.revision());
        let mut replies = WriteQueue::default();

        let metrics = process_output(
            b"\x1b[2026lA\x1b[2026h\x1b\\\x1b[2026lB",
            SplintId::new(),
            incarnation,
            None,
            &mut terminal,
            &mut replies,
            &mut subscribers,
            &mut publication,
            1024,
        )
        .unwrap();

        assert!(terminal.synchronized_updates());
        assert!(publication.active && !publication.timed_out);
        assert_eq!(metrics.terminal_updates, 1);
        let LiveEvent::Update {
            updates, snapshot, ..
        } = receiver.try_recv().unwrap()
        else {
            panic!("completed Cava frame was not published");
        };
        assert_eq!(updates.len(), 1);
        assert_eq!(snapshot.visible_rows[0].cells[0].content, "A");
        assert_eq!(
            snapshot.visible_rows[0].cells[1].content, "",
            "the immutable completed frame must exclude partial next-frame state"
        );
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn ordinary_output_bypasses_synchronized_frame_throttle() {
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        let (subscriber, mut receiver, _) = synchronized_test_subscriber(&terminal);
        let mut subscribers = vec![subscriber];
        let mut publication = SynchronizedPublication::new(terminal.revision());
        publication.next_frame_at = Some(Instant::now() + Duration::from_secs(1));
        let mut replies = WriteQueue::default();

        process_output(
            b"\x1b[2026lA\x1b[2026h",
            SplintId::new(),
            incarnation,
            None,
            &mut terminal,
            &mut replies,
            &mut subscribers,
            &mut publication,
            1024,
        )
        .unwrap();
        process_output(
            b"\x1b\\",
            SplintId::new(),
            incarnation,
            None,
            &mut terminal,
            &mut replies,
            &mut subscribers,
            &mut publication,
            1024,
        )
        .unwrap();
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        let metrics = process_output(
            b"Z",
            SplintId::new(),
            incarnation,
            None,
            &mut terminal,
            &mut replies,
            &mut subscribers,
            &mut publication,
            1024,
        )
        .unwrap();

        assert_eq!(metrics.terminal_updates, 2);
        let LiveEvent::Update {
            updates, snapshot, ..
        } = receiver.try_recv().unwrap()
        else {
            panic!("ordinary output must publish immediately");
        };
        assert_eq!(updates.len(), 2);
        assert_eq!(snapshot.visible_rows[0].cells[0].content, "A");
        assert_eq!(snapshot.visible_rows[0].cells[1].content, "Z");
    }

    #[tokio::test]
    async fn capacity_one_preserves_final_synchronized_update_before_exit() {
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        let (events, receiver) = mpsc::channel(2);
        let (resnapshot, resnapshot_receiver) = watch::channel(false);
        let mut subscribers = vec![Subscriber {
            events,
            resnapshot,
            published_revision: terminal.revision(),
            snapshot_rows: 0,
        }];
        let mut subscription = Subscription {
            events: receiver,
            resnapshot: resnapshot_receiver,
        };
        let mut publication = SynchronizedPublication::new(terminal.revision());
        terminal.advance(b"\x1b[?2026hfinal\x1b[?2026l");
        publish_updates(
            SplintId::new(),
            &terminal,
            &mut publication,
            incarnation,
            None,
            &mut subscribers,
        );
        let status = ProcessExit {
            code: Some(0),
            signal: None,
        };
        assert_eq!(
            publish(
                &mut subscribers,
                LiveEvent::Exited {
                    incarnation,
                    status,
                },
            ),
            0
        );
        drop(subscribers);

        assert!(matches!(
            subscription.recv().await,
            SubscriptionReceive::Event(LiveEvent::Update { .. })
        ));
        assert!(matches!(
            subscription.recv().await,
            SubscriptionReceive::Event(LiveEvent::Exited {
                incarnation: event_incarnation,
                status: event_status,
            }) if event_incarnation == incarnation && event_status == status
        ));
    }

    #[test]
    fn subscriber_capacity_reserves_exit_slot_and_rejects_extremes() {
        assert_eq!(subscriber_channel_capacity(1, 64).unwrap(), 2);
        assert_eq!(
            subscriber_channel_capacity(MAX_SUBSCRIBER_QUEUE_CAPACITY, usize::MAX).unwrap(),
            MAX_SUBSCRIBER_QUEUE_CAPACITY + 1
        );
        assert!(subscriber_channel_capacity(0, 64).is_err());
        assert!(subscriber_channel_capacity(usize::MAX, usize::MAX).is_err());
    }

    #[test]
    fn synchronized_publication_timeout_is_fixed_and_commits_one_frame() {
        let incarnation = ProcessIncarnation::allocate();
        let mut config = TerminalConfig::default();
        config.update_history_limit = 4;
        let mut terminal = Terminal::new(8, 2, config);
        let initial_revision = terminal.revision();
        let (subscriber, mut receiver, resnapshot) = synchronized_test_subscriber(&terminal);
        let mut subscribers = vec![subscriber];
        let mut publication = SynchronizedPublication::new(terminal.revision());
        let mut replies = WriteQueue::default();
        let started = Instant::now();

        process_output(
            b"\x1b[?2026hABCDEFGHIJK",
            SplintId::new(),
            incarnation,
            None,
            &mut terminal,
            &mut replies,
            &mut subscribers,
            &mut publication,
            1024,
        )
        .unwrap();
        assert_eq!(terminal.revision(), initial_revision);
        let deadline = publication.deadline.unwrap();
        publication.observe(true, started + Duration::from_millis(900));
        assert_eq!(publication.deadline, Some(deadline));
        terminal.expire_synchronized_updates();
        publication.observe(false, Instant::now());
        publication.expire();
        let (updates, overflows) = publish_updates(
            SplintId::new(),
            &terminal,
            &mut publication,
            incarnation,
            None,
            &mut subscribers,
        );
        assert_eq!((updates, overflows), (1, 0));
        assert_eq!(subscribers.len(), 1);
        assert!(!*resnapshot.borrow());
        let LiveEvent::Update { updates, .. } = receiver.try_recv().unwrap() else {
            panic!("expected timeout frame");
        };
        assert_eq!(updates.len(), 1);
    }

    #[test]
    fn publication_history_gap_requests_resnapshot_without_panicking() {
        let incarnation = ProcessIncarnation::allocate();
        let mut config = TerminalConfig::default();
        config.update_history_limit = 4;
        let mut terminal = Terminal::new(8, 2, config);
        let (subscriber, _receiver, resnapshot) = synchronized_test_subscriber(&terminal);
        let mut subscribers = vec![subscriber];
        let mut publication = SynchronizedPublication::new(terminal.revision());
        terminal.advance(b"ABCDEFGHIJK");

        let (_, overflows) = publish_updates(
            SplintId::new(),
            &terminal,
            &mut publication,
            incarnation,
            None,
            &mut subscribers,
        );
        assert_eq!(overflows, 1);
        assert!(subscribers.is_empty());
        assert!(*resnapshot.borrow());
    }

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
    async fn shared_image_budget_rejects_across_actors_and_releases_on_exit() {
        let budget = splinterm_terminal::SharedImageBudget::new(96);
        let image_script = "printf '\\033Pq#1;2;100;0;0#1~\\033\\\\'; sleep 5";
        let config = || {
            let mut config = fast_config();
            config.pixel_width = 320;
            config.pixel_height = 96;
            config.terminal.shared_image_budget = Some(budget.clone());
            config
        };
        let first =
            LiveSplintRuntime::spawn(SplintId::new(), backend(), shell(image_script), config())
                .await
                .unwrap();
        let first_handle = first.handle();
        let second =
            LiveSplintRuntime::spawn(SplintId::new(), backend(), shell(image_script), config())
                .await
                .unwrap();
        let second_handle = second.handle();
        time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            first_handle.snapshot().await.unwrap().image_contents.len(),
            1
        );
        assert_eq!(
            second_handle.snapshot().await.unwrap().image_contents.len(),
            1
        );
        assert_eq!(budget.metrics().content_bytes, 96);

        let rejected =
            LiveSplintRuntime::spawn(SplintId::new(), backend(), shell(image_script), config())
                .await
                .unwrap();
        let rejected_handle = rejected.handle();
        time::sleep(Duration::from_millis(50)).await;
        assert!(
            rejected_handle
                .snapshot()
                .await
                .unwrap()
                .image_contents
                .is_empty()
        );
        assert_eq!(budget.metrics().content_bytes, 96);

        first.shutdown().await.unwrap();
        assert_eq!(budget.metrics().content_bytes, 48);
        let replacement =
            LiveSplintRuntime::spawn(SplintId::new(), backend(), shell(image_script), config())
                .await
                .unwrap();
        let replacement_handle = replacement.handle();
        time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            replacement_handle
                .snapshot()
                .await
                .unwrap()
                .image_contents
                .len(),
            1
        );
        assert_eq!(budget.metrics().content_bytes, 96);
        assert_eq!(budget.metrics().high_water_content_bytes, 96);

        second.shutdown().await.unwrap();
        rejected.shutdown().await.unwrap();
        replacement.shutdown().await.unwrap();
        assert_eq!(budget.metrics().content_bytes, 0);
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
