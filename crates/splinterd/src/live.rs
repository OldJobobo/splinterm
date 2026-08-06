use std::{
    collections::VecDeque,
    ffi::OsString,
    io::{self, Read, Write},
    os::unix::process::ExitStatusExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use splinterm_core::SplintId;
use splinterm_protocol::perf_trace::{PerfTraceEvent, emit_perf_trace, perf_trace_enabled};
use splinterm_pty::{
    LinuxPtyBackend, LinuxPtySession, PtyCommand, PtyError, PtySession, PtySignal, PtySize,
};
use splinterm_terminal::{
    ActiveScreen, CellAttributesSnapshot, CellSnapshotContent, CursorSnapshot, Dimensions,
    ImageContent, ImageContentId, ImageContentMetadata, ImagePlacement, ScrollDirection,
    ScrollRegion, ScrollbackSnapshot, SearchPage, SnapshotRequest, Terminal, TerminalConfig,
    TerminalDamage, TerminalEvent, TerminalModes, TerminalRevision, TerminalUpdate,
};
use thiserror::Error;
use tokio::{
    io::unix::AsyncFd,
    sync::{Notify, mpsc, oneshot, watch},
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

#[derive(Clone, Debug, Eq, PartialEq)]
enum CompactCellContent {
    Empty,
    Scalar(char),
    Composed(String),
    Spacer { remaining: u32 },
}

impl CompactCellContent {
    fn into_live(self, attributes: CellAttributesSnapshot) -> LiveCell {
        match self {
            Self::Empty => LiveCell {
                content: String::new(),
                spacer_remaining: None,
                attributes,
            },
            Self::Scalar(character) => LiveCell {
                content: character.to_string(),
                spacer_remaining: None,
                attributes,
            },
            Self::Composed(characters) => LiveCell {
                content: characters,
                spacer_remaining: None,
                attributes,
            },
            Self::Spacer { remaining } => LiveCell {
                content: String::new(),
                spacer_remaining: Some(remaining),
                attributes,
            },
        }
    }

    fn owned_string_bytes(&self) -> usize {
        match self {
            Self::Composed(characters) => characters.capacity(),
            Self::Empty | Self::Scalar(_) | Self::Spacer { .. } => 0,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CompactLiveCell {
    content: CompactCellContent,
    attributes: CellAttributesSnapshot,
}

impl CompactLiveCell {
    fn into_live(self) -> LiveCell {
        self.content.into_live(self.attributes)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CompactLiveRow {
    row_id: Option<u64>,
    linebreak: bool,
    cells: Vec<CompactLiveCell>,
}

impl CompactLiveRow {
    fn into_live(self) -> LiveRow {
        LiveRow {
            row_id: self.row_id,
            linebreak: self.linebreak,
            cells: self
                .cells
                .into_iter()
                .map(CompactLiveCell::into_live)
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CompactLiveSnapshot {
    metadata: LiveSnapshot,
    visible_rows: Vec<CompactLiveRow>,
    scrollback_rows: Vec<CompactLiveRow>,
    history_policy: CompactHistoryPolicy,
}

impl CompactLiveSnapshot {
    fn into_live(mut self) -> LiveSnapshot {
        self.metadata.visible_rows = self
            .visible_rows
            .into_iter()
            .map(CompactLiveRow::into_live)
            .collect();
        self.metadata.scrollback_rows = self
            .scrollback_rows
            .into_iter()
            .map(CompactLiveRow::into_live)
            .collect();
        self.metadata
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct SnapshotAttribution {
    rows: u64,
    cells: u64,
    empty_cells: u64,
    scalar_cells: u64,
    composed_cells: u64,
    spacer_cells: u64,
    owned_string_bytes: u64,
}

#[derive(Debug)]
enum CompactQueuedEvent {
    /// Notification only. Ordered semantic batches and their exact snapshot are
    /// owned together by the synchronized per-subscriber mailbox.
    UpdateReady,
    Exited {
        incarnation: ProcessIncarnation,
        status: ProcessExit,
        admitted: Option<QueueLease>,
    },
}

impl CompactQueuedEvent {
    fn release_admitted_ownership(&mut self) {
        if let Self::Exited { admitted, .. } = self {
            drop(admitted.take());
        }
    }
}

#[derive(Debug)]
struct SnapshotEnvelope {
    revision: TerminalRevision,
    snapshot: Box<CompactLiveSnapshot>,
    _admitted: Option<SnapshotLease>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CompactHistoryPolicy {
    FullHistory,
    NoHistory,
    AppendTail(usize),
}

impl CompactHistoryPolicy {
    fn merge(self, next: Self) -> Self {
        match (self, next) {
            (Self::FullHistory, _) | (_, Self::FullHistory) => Self::FullHistory,
            (Self::NoHistory, policy) | (policy, Self::NoHistory) => policy,
            (Self::AppendTail(left), Self::AppendTail(right)) => {
                Self::AppendTail(left.saturating_add(right))
            }
        }
    }
}

fn compact_history_policy(
    updates: &[TerminalUpdate],
    dimensions: Dimensions,
    active_screen: ActiveScreen,
) -> CompactHistoryPolicy {
    if active_screen != ActiveScreen::Normal {
        return CompactHistoryPolicy::FullHistory;
    }
    let mut appended_rows = 0_usize;
    let mut saw_scrollback = false;
    for update in updates {
        let mut update_appended = 0_usize;
        let mut update_scrollback = false;
        for damage in update.damage() {
            match damage {
                TerminalDamage::FullSnapshot | TerminalDamage::Dimensions => {
                    return CompactHistoryPolicy::FullHistory;
                }
                TerminalDamage::Scroll {
                    direction: ScrollDirection::Forward,
                    region,
                    rows,
                } if region.start() == 0
                    && usize::try_from(region.end()).ok() == Some(dimensions.rows) =>
                {
                    update_appended = update_appended.saturating_add(*rows);
                }
                TerminalDamage::Scroll { .. } => return CompactHistoryPolicy::FullHistory,
                TerminalDamage::Scrollback => update_scrollback = true,
                TerminalDamage::Rows { .. }
                | TerminalDamage::Cursor { .. }
                | TerminalDamage::Modes
                | TerminalDamage::Viewport
                | TerminalDamage::Title
                | TerminalDamage::Palette { .. }
                | TerminalDamage::Images { .. } => {}
            }
        }
        if update_scrollback {
            if update_appended == 0 {
                return CompactHistoryPolicy::FullHistory;
            }
            saw_scrollback = true;
            appended_rows = appended_rows.saturating_add(update_appended);
        } else if update_appended > 0 {
            return CompactHistoryPolicy::FullHistory;
        }
    }
    if saw_scrollback {
        CompactHistoryPolicy::AppendTail(appended_rows)
    } else {
        CompactHistoryPolicy::NoHistory
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PendingFrameAttribution {
    batches: u64,
    terminal_updates: u64,
    scrolls: u64,
    appended_rows: u64,
}

impl PendingFrameAttribution {
    fn one_batch(updates: &[TerminalUpdate], history_policy: CompactHistoryPolicy) -> Self {
        Self {
            batches: 1,
            terminal_updates: u64::try_from(updates.len()).unwrap_or(u64::MAX),
            scrolls: u64::try_from(
                updates
                    .iter()
                    .flat_map(TerminalUpdate::damage)
                    .filter(|damage| matches!(damage, TerminalDamage::Scroll { .. }))
                    .count(),
            )
            .unwrap_or(u64::MAX),
            appended_rows: match history_policy {
                CompactHistoryPolicy::AppendTail(rows) => u64::try_from(rows).unwrap_or(u64::MAX),
                CompactHistoryPolicy::FullHistory | CompactHistoryPolicy::NoHistory => 0,
            },
        }
    }

    fn merge(&mut self, next: Self) {
        self.batches = self.batches.saturating_add(next.batches);
        self.terminal_updates = self.terminal_updates.saturating_add(next.terminal_updates);
        self.scrolls = self.scrolls.saturating_add(next.scrolls);
        self.appended_rows = self.appended_rows.saturating_add(next.appended_rows);
    }
}

#[derive(Debug)]
struct PendingCompactUpdates {
    incarnation: ProcessIncarnation,
    batches: Vec<Vec<TerminalUpdate>>,
    end_revision: TerminalRevision,
    history_policy: CompactHistoryPolicy,
    admitted: Vec<Option<QueueLease>>,
    pending_attribution: Option<PendingFrameLease>,
}

impl PendingCompactUpdates {
    fn into_updates(self) -> Vec<TerminalUpdate> {
        if let Some(attribution) = &self.pending_attribution {
            attribution.record_materialization();
        }
        let total = self.batches.iter().map(Vec::len).sum();
        let mut updates = Vec::with_capacity(total);
        for batch in self.batches {
            updates.extend(batch);
        }
        updates
    }
}

#[derive(Debug, Default)]
struct CompactMailboxState {
    generation: u64,
    pending: Option<PendingCompactUpdates>,
    snapshot: Option<SnapshotEnvelope>,
}

#[derive(Debug, Default)]
struct CompactSnapshotSlot {
    current: Mutex<CompactMailboxState>,
    producer_batch_active: AtomicBool,
    producer_batch_done: Notify,
    #[cfg(test)]
    producer_batch_waits: AtomicUsize,
    #[cfg(test)]
    producer_batch_wakes: AtomicUsize,
}

#[derive(Debug)]
enum MailboxTake {
    Exact {
        incarnation: ProcessIncarnation,
        updates: Vec<TerminalUpdate>,
        end_revision: TerminalRevision,
        snapshot: Box<CompactLiveSnapshot>,
    },
    MissingOrMismatched,
}

impl CompactSnapshotSlot {
    fn lock(&self) -> std::sync::MutexGuard<'_, CompactMailboxState> {
        self.current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn begin_producer_batch(&self) {
        self.producer_batch_active.store(true, Ordering::Release);
    }

    fn end_producer_batch(&self) {
        self.producer_batch_active.store(false, Ordering::Release);
        self.producer_batch_done.notify_one();
    }

    async fn wait_for_producer_batch(&self, resnapshot: &mut watch::Receiver<bool>) -> bool {
        loop {
            if *resnapshot.borrow() {
                return true;
            }
            let completed = self.producer_batch_done.notified();
            tokio::pin!(completed);
            if !self.producer_batch_active.load(Ordering::Acquire) {
                return false;
            }
            #[cfg(test)]
            self.producer_batch_waits.fetch_add(1, Ordering::Relaxed);
            tokio::select! {
                biased;
                changed = resnapshot.changed() => {
                    if changed.is_ok() && *resnapshot.borrow() {
                        return true;
                    }
                    if changed.is_err() {
                        completed.as_mut().await;
                    }
                }
                () = completed.as_mut() => {}
            }
            #[cfg(test)]
            self.producer_batch_wakes.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn clear(&self) {
        let mut current = self.lock();
        current.generation = current.generation.wrapping_add(1);
        drop(current.pending.take());
        drop(current.snapshot.take());
    }

    fn take_pending(&self) -> MailboxTake {
        let mut current = self.lock();
        let pending = current.pending.take();
        let snapshot = current.snapshot.take();
        current.generation = current.generation.wrapping_add(1);
        let Some(pending) = pending else {
            return MailboxTake::MissingOrMismatched;
        };
        let Some(snapshot) = snapshot else {
            return MailboxTake::MissingOrMismatched;
        };
        if pending.end_revision != snapshot.revision
            || pending.history_policy != snapshot.snapshot.history_policy
        {
            return MailboxTake::MissingOrMismatched;
        }
        let incarnation = pending.incarnation;
        let end_revision = pending.end_revision;
        let updates = pending.into_updates();
        MailboxTake::Exact {
            incarnation,
            updates,
            end_revision,
            snapshot: snapshot.snapshot,
        }
    }
}

#[derive(Debug)]
struct QueueAccounting {
    enabled: bool,
    local_events: AtomicUsize,
    metrics: Arc<RuntimeMetrics>,
    #[cfg(test)]
    materializations: AtomicUsize,
}

impl QueueAccounting {
    fn new(enabled: bool, metrics: Arc<RuntimeMetrics>) -> Self {
        Self {
            enabled,
            local_events: AtomicUsize::new(0),
            metrics,
            #[cfg(test)]
            materializations: AtomicUsize::new(0),
        }
    }

    fn admit_event(&self) {
        debug_assert!(self.enabled);
        let local = RuntimeMetrics::add_usize_saturating(&self.local_events, 1);
        RuntimeMetrics::observe_max(
            &self.metrics.subscriber_queue_per_subscriber_high_water,
            local,
        );
        let aggregate =
            RuntimeMetrics::add_usize_saturating(&self.metrics.subscriber_queue_events_current, 1);
        RuntimeMetrics::observe_max(&self.metrics.subscriber_queue_events_high_water, aggregate);
    }

    fn release_event(&self) {
        debug_assert!(self.enabled);
        RuntimeMetrics::sub_usize_saturating(&self.local_events, 1);
        RuntimeMetrics::sub_usize_saturating(&self.metrics.subscriber_queue_events_current, 1);
    }

    fn admit_snapshot(&self, attribution: SnapshotAttribution) {
        debug_assert!(self.enabled);
        let snapshots =
            RuntimeMetrics::add_usize_saturating(&self.metrics.queued_snapshot_events_current, 1);
        RuntimeMetrics::observe_max(&self.metrics.queued_snapshot_events_high_water, snapshots);
        self.metrics.add_queued_snapshot(attribution);
        RuntimeMetrics::add_saturating(&self.metrics.publication_snapshot_enqueues, 1);
        RuntimeMetrics::add_saturating(
            &self.metrics.publication_snapshot_enqueued_rows,
            attribution.rows,
        );
        RuntimeMetrics::add_saturating(
            &self.metrics.publication_snapshot_enqueued_cells,
            attribution.cells,
        );
        RuntimeMetrics::add_saturating(
            &self
                .metrics
                .publication_snapshot_enqueued_owned_string_bytes,
            attribution.owned_string_bytes,
        );
    }

    fn release_snapshot(&self, attribution: SnapshotAttribution) {
        debug_assert!(self.enabled);
        RuntimeMetrics::sub_usize_saturating(&self.metrics.queued_snapshot_events_current, 1);
        self.metrics.remove_queued_snapshot(attribution);
    }
}

/// Ownership guard for one permit-admitted compact semantic event.
#[derive(Debug)]
struct QueueLease {
    state: Arc<QueueLeaseState>,
}

#[derive(Debug)]
struct QueueAdmissionWitness {
    state: Arc<QueueLeaseState>,
    armed: bool,
}

#[derive(Debug)]
struct QueueLeaseState {
    active: std::sync::atomic::AtomicBool,
    accounting: Arc<QueueAccounting>,
}

impl QueueLeaseState {
    fn release(&self) {
        if self.active.swap(false, Ordering::AcqRel) {
            self.accounting.release_event();
        }
    }
}

impl QueueLease {
    fn new(accounting: &Arc<QueueAccounting>) -> Option<Self> {
        if !accounting.enabled {
            return None;
        }
        accounting.admit_event();
        Some(Self {
            state: Arc::new(QueueLeaseState {
                active: std::sync::atomic::AtomicBool::new(true),
                accounting: Arc::clone(accounting),
            }),
        })
    }

    fn witness(&self) -> QueueAdmissionWitness {
        QueueAdmissionWitness {
            state: Arc::clone(&self.state),
            armed: true,
        }
    }
}

impl QueueAdmissionWitness {
    fn release(mut self) {
        self.state.release();
        self.armed = false;
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for QueueAdmissionWitness {
    fn drop(&mut self) {
        if self.armed {
            self.state.release();
        }
    }
}

impl Drop for QueueLease {
    fn drop(&mut self) {
        self.state.release();
    }
}

#[derive(Debug)]
struct SnapshotLease {
    accounting: Arc<QueueAccounting>,
    attribution: SnapshotAttribution,
}

impl SnapshotLease {
    fn new(
        accounting: &Arc<QueueAccounting>,
        attribution: Option<SnapshotAttribution>,
    ) -> Option<Self> {
        let attribution = attribution?;
        accounting.admit_snapshot(attribution);
        Some(Self {
            accounting: Arc::clone(accounting),
            attribution,
        })
    }
}

impl Drop for SnapshotLease {
    fn drop(&mut self) {
        self.accounting.release_snapshot(self.attribution);
    }
}

#[derive(Debug)]
struct PendingFrameLease {
    accounting: Arc<QueueAccounting>,
    attribution: PendingFrameAttribution,
}

impl PendingFrameLease {
    fn new(
        accounting: &Arc<QueueAccounting>,
        attribution: PendingFrameAttribution,
    ) -> Option<Self> {
        if !accounting.enabled {
            return None;
        }
        accounting.metrics.add_queued_compact(attribution);
        Some(Self {
            accounting: Arc::clone(accounting),
            attribution,
        })
    }

    fn merge(&mut self, attribution: PendingFrameAttribution) {
        self.accounting.metrics.add_queued_compact(attribution);
        RuntimeMetrics::add_saturating(
            &self.accounting.metrics.publication_compact_batch_merges,
            attribution.batches,
        );
        self.attribution.merge(attribution);
    }

    fn record_materialization(&self) {
        self.accounting
            .metrics
            .record_compact_materialization(self.attribution);
    }
}

impl Drop for PendingFrameLease {
    fn drop(&mut self) {
        self.accounting
            .metrics
            .remove_queued_compact(self.attribution);
    }
}

fn send_permit_admitted_compact(
    sender: &mpsc::Sender<CompactQueuedEvent>,
    permit: mpsc::Permit<'_, CompactQueuedEvent>,
    accounting: &Arc<QueueAccounting>,
    build: impl FnOnce(Option<QueueLease>) -> CompactQueuedEvent,
    before_send: impl FnOnce(),
) {
    let admitted = QueueLease::new(accounting);
    let admission_witness = admitted.as_ref().map(QueueLease::witness);
    let event = build(admitted);
    before_send();
    permit.send(event);
    // A receiver can close after reserving the permit but before send. Tokio
    // may then retain the value until the sender is dropped, so release the
    // idempotent ownership state eagerly while leaving event Drop as fallback.
    if let Some(witness) = admission_witness {
        if sender.is_closed() {
            witness.release();
        } else {
            witness.disarm();
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CompactPublishOutcome {
    Published,
    Full,
    Closed,
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "compact publication validates and commits one mailbox transaction"
)]
fn publish_compact_update(
    sender: &mpsc::Sender<CompactQueuedEvent>,
    accounting: &Arc<QueueAccounting>,
    snapshot_slot: &Arc<CompactSnapshotSlot>,
    semantic_capacity: usize,
    incarnation: ProcessIncarnation,
    updates: Vec<TerminalUpdate>,
    end_revision: TerminalRevision,
    history_policy: CompactHistoryPolicy,
    metrics: &Arc<RuntimeMetrics>,
    mut build_snapshot: impl FnMut(CompactHistoryPolicy) -> CompactLiveSnapshot,
) -> CompactPublishOutcome {
    // Inspect the bounded semantic tail without holding the mailbox across the
    // expensive full-snapshot build. The actor is the only producer, while the
    // receiver may consume and advance `generation` during construction.
    let (observed_generation, had_pending, combined_policy) = {
        let mut current = snapshot_slot.lock();
        if sender.is_closed() {
            return CompactPublishOutcome::Closed;
        }
        match current.pending.as_ref() {
            Some(pending) => {
                if pending.batches.len() >= semantic_capacity {
                    current.generation = current.generation.wrapping_add(1);
                    drop(current.pending.take());
                    drop(current.snapshot.take());
                    return CompactPublishOutcome::Full;
                }
                (
                    current.generation,
                    true,
                    pending.history_policy.merge(history_policy),
                )
            }
            None => (current.generation, false, history_policy),
        }
    };

    // A previously empty mailbox needs a wake token. Reserve it before doing
    // allocation work, but do not send it until the matching state is committed.
    let mut reserved = if had_pending {
        None
    } else {
        match sender.try_reserve() {
            Ok(permit) => Some(permit),
            Err(mpsc::error::TrySendError::Full(())) => return CompactPublishOutcome::Full,
            Err(mpsc::error::TrySendError::Closed(())) => {
                return CompactPublishOutcome::Closed;
            }
        }
    };

    let mut snapshot = build_snapshot(combined_policy);
    let mut attribution = accounting
        .enabled
        .then(|| compact_snapshot_attribution(&snapshot));
    if let Some(attribution) = attribution {
        record_publication_snapshot(metrics, attribution);
    }

    let mut current = snapshot_slot.lock();
    if sender.is_closed() {
        return CompactPublishOutcome::Closed;
    }

    if current.generation == observed_generation {
        if let Some(pending) = current.pending.as_mut() {
            if !had_pending || pending.batches.len() >= semantic_capacity {
                current.generation = current.generation.wrapping_add(1);
                drop(current.pending.take());
                drop(current.snapshot.take());
                return CompactPublishOutcome::Full;
            }
            let batch_attribution = PendingFrameAttribution::one_batch(&updates, history_policy);
            let snapshot = Box::new(snapshot);
            pending.batches.push(updates);
            pending.admitted.push(QueueLease::new(accounting));
            if let Some(lease) = pending.pending_attribution.as_mut() {
                lease.merge(batch_attribution);
            }
            pending.end_revision = end_revision;
            pending.history_policy = snapshot.history_policy;
            drop(current.snapshot.take());
            current.snapshot = Some(SnapshotEnvelope {
                revision: end_revision,
                snapshot,
                _admitted: SnapshotLease::new(accounting, attribution),
            });
            return CompactPublishOutcome::Published;
        }
        if had_pending {
            // The generation is unchanged, so a tail observed before the build
            // cannot disappear without violating mailbox ownership.
            return CompactPublishOutcome::Full;
        }
    } else if current.pending.is_some() {
        // Only the receiver may advance the generation. It removes the complete
        // tail, so finding a replacement tail here would make ordering unclear.
        current.generation = current.generation.wrapping_add(1);
        drop(current.pending.take());
        drop(current.snapshot.take());
        return CompactPublishOutcome::Full;
    }

    // If the receiver drained an observed tail during construction, rebuild
    // against only this publication. Reusing a combined append-tail snapshot
    // would attach extra history rows to a shorter semantic tail.
    if had_pending && current.generation != observed_generation && combined_policy != history_policy
    {
        let fresh_generation = current.generation;
        drop(current);
        snapshot = build_snapshot(history_policy);
        attribution = accounting
            .enabled
            .then(|| compact_snapshot_attribution(&snapshot));
        if let Some(attribution) = attribution {
            record_publication_snapshot(metrics, attribution);
        }
        current = snapshot_slot.lock();
        if sender.is_closed() {
            return CompactPublishOutcome::Closed;
        }
        if current.generation != fresh_generation || current.pending.is_some() {
            current.generation = current.generation.wrapping_add(1);
            drop(current.pending.take());
            drop(current.snapshot.take());
            return CompactPublishOutcome::Full;
        }
    }

    // The receiver drained the observed tail while this snapshot was built (or
    // the mailbox was initially empty). Establish a fresh tail and notification.
    let permit = match reserved.take() {
        Some(permit) if current.generation == observed_generation => permit,
        Some(_) | None => match sender.try_reserve() {
            Ok(permit) => permit,
            Err(mpsc::error::TrySendError::Full(())) => return CompactPublishOutcome::Full,
            Err(mpsc::error::TrySendError::Closed(())) => {
                return CompactPublishOutcome::Closed;
            }
        },
    };
    debug_assert!(current.pending.is_none());
    drop(current.snapshot.take());
    let batch_attribution = PendingFrameAttribution::one_batch(&updates, history_policy);
    let snapshot = Box::new(snapshot);
    current.pending = Some(PendingCompactUpdates {
        incarnation,
        batches: vec![updates],
        end_revision,
        history_policy: snapshot.history_policy,
        admitted: vec![QueueLease::new(accounting)],
        pending_attribution: PendingFrameLease::new(accounting, batch_attribution),
    });
    current.snapshot = Some(SnapshotEnvelope {
        revision: end_revision,
        snapshot,
        _admitted: SnapshotLease::new(accounting, attribution),
    });
    permit.send(CompactQueuedEvent::UpdateReady);
    CompactPublishOutcome::Published
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

/// Additive first-party subscription that retains compact snapshots until a
/// pending update tail has been coalesced.
///
/// The original [`Subscription`] API and behavior remain unchanged. This type
/// intentionally exposes no raw receiver because its queued event types are
/// private implementation details.
#[derive(Debug)]
pub struct CompactSubscription {
    events: mpsc::Receiver<CompactQueuedEvent>,
    resnapshot: watch::Receiver<bool>,
    #[cfg_attr(not(test), allow(dead_code))]
    accounting: Arc<QueueAccounting>,
    snapshot_slot: Arc<CompactSnapshotSlot>,
}

impl CompactSubscription {
    #[must_use]
    pub fn resnapshot_required(&self) -> bool {
        *self.resnapshot.borrow()
    }

    pub async fn changed(&mut self) -> bool {
        self.resnapshot.changed().await.is_ok() && *self.resnapshot.borrow()
    }

    #[cfg(test)]
    async fn recv_queued(&mut self) -> Option<CompactQueuedEvent> {
        let mut event = self.events.recv().await?;
        event.release_admitted_ownership();
        Some(event)
    }

    fn try_recv_queued(&mut self) -> Result<CompactQueuedEvent, mpsc::error::TryRecvError> {
        let mut event = self.events.try_recv()?;
        event.release_admitted_ownership();
        Ok(event)
    }

    #[cfg(test)]
    fn try_recv(&mut self) -> Result<LiveEvent, mpsc::error::TryRecvError> {
        let event = self.try_recv_queued()?;
        match event {
            CompactQueuedEvent::UpdateReady => match self.snapshot_slot.take_pending() {
                MailboxTake::Exact {
                    incarnation,
                    updates,
                    snapshot,
                    ..
                } => {
                    self.accounting
                        .materializations
                        .fetch_add(1, Ordering::Relaxed);
                    Ok(LiveEvent::Update {
                        incarnation,
                        updates,
                        snapshot: Box::new(snapshot.into_live()),
                    })
                }
                MailboxTake::MissingOrMismatched => Err(mpsc::error::TryRecvError::Empty),
            },
            CompactQueuedEvent::Exited {
                incarnation,
                status,
                ..
            } => Ok(LiveEvent::Exited {
                incarnation,
                status,
            }),
        }
    }

    /// Receives one event after coalescing an immediately pending contiguous
    /// semantic-update tail and pairing it with the exact revision-tagged
    /// snapshot retained in the subscriber's one-entry slot.
    ///
    /// A trailing process exit is returned beside the retained update so a
    /// caller can publish final state before exit. If exact revision continuity
    /// cannot be proven, resnapshot is required instead of pairing mismatched
    /// state.
    pub async fn recv_coalesced(&mut self) -> (SubscriptionReceive, Option<ProcessExit>) {
        if *self.resnapshot.borrow() {
            self.snapshot_slot.clear();
            return (SubscriptionReceive::ResnapshotRequired, None);
        }
        let events = &mut self.events;
        let resnapshot = &mut self.resnapshot;
        let mut first = tokio::select! {
            biased;
            changed = resnapshot.changed() => {
                if changed.is_ok() && *resnapshot.borrow() {
                    self.snapshot_slot.clear();
                    return (SubscriptionReceive::ResnapshotRequired, None);
                }
                match events.try_recv() {
                    Ok(event) => event,
                    Err(_) => return (SubscriptionReceive::Closed, None),
                }
            }
            event = events.recv() => match event {
                Some(event) => event,
                None => return (SubscriptionReceive::Closed, None),
            },
        };
        first.release_admitted_ownership();
        if self
            .snapshot_slot
            .wait_for_producer_batch(&mut self.resnapshot)
            .await
        {
            self.snapshot_slot.clear();
            return (SubscriptionReceive::ResnapshotRequired, None);
        }
        self.coalesce_queued(&first)
    }

    fn coalesce_queued(
        &mut self,
        retained: &CompactQueuedEvent,
    ) -> (SubscriptionReceive, Option<ProcessExit>) {
        if let CompactQueuedEvent::Exited {
            incarnation,
            status,
            ..
        } = retained
        {
            return (
                SubscriptionReceive::Event(LiveEvent::Exited {
                    incarnation: *incarnation,
                    status: *status,
                }),
                None,
            );
        }

        let MailboxTake::Exact {
            mut incarnation,
            mut updates,
            mut end_revision,
            mut snapshot,
        } = self.snapshot_slot.take_pending()
        else {
            self.snapshot_slot.clear();
            return (SubscriptionReceive::ResnapshotRequired, None);
        };
        let mut trailing_exit = None;

        loop {
            if *self.resnapshot.borrow() {
                self.snapshot_slot.clear();
                return (SubscriptionReceive::ResnapshotRequired, None);
            }
            match self.try_recv_queued() {
                Ok(CompactQueuedEvent::UpdateReady) => {
                    let MailboxTake::Exact {
                        incarnation: pending_incarnation,
                        updates: pending_updates,
                        end_revision: pending_revision,
                        snapshot: pending_snapshot,
                    } = self.snapshot_slot.take_pending()
                    else {
                        self.snapshot_slot.clear();
                        return (SubscriptionReceive::ResnapshotRequired, None);
                    };
                    debug_assert_eq!(incarnation, pending_incarnation);
                    updates.extend(pending_updates);
                    incarnation = pending_incarnation;
                    end_revision = pending_revision;
                    snapshot = pending_snapshot;
                }
                Ok(CompactQueuedEvent::Exited { status, .. }) => {
                    trailing_exit = Some(status);
                    break;
                }
                Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected) => {
                    break;
                }
            }
        }

        if *self.resnapshot.borrow() || snapshot.metadata.revision != end_revision {
            self.snapshot_slot.clear();
            return (SubscriptionReceive::ResnapshotRequired, None);
        }
        #[cfg(test)]
        self.accounting
            .materializations
            .fetch_add(1, Ordering::Relaxed);
        (
            SubscriptionReceive::Event(LiveEvent::Update {
                incarnation,
                updates,
                snapshot: Box::new(snapshot.into_live()),
            }),
            trailing_exit,
        )
    }
}

impl Drop for CompactSubscription {
    fn drop(&mut self) {
        self.events.close();
        while let Ok(mut event) = self.events.try_recv() {
            event.release_admitted_ownership();
        }
        self.snapshot_slot.clear();
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
    SubscribeCompact(usize, usize, Reply<CompactSubscription>),
    AttachCompact(usize, usize, Reply<(LiveSnapshot, CompactSubscription)>),
    Shutdown(oneshot::Sender<()>),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct LiveRuntimeMetrics {
    pub command_queue_high_water: usize,
    pub user_write_queue_high_water_bytes: usize,
    pub reply_write_queue_high_water_bytes: usize,
    /// Maximum active subscribers observed while publication attribution was enabled.
    pub subscriber_count_high_water: usize,
    /// Compact events whose ownership was admitted by a reserved queue permit
    /// and has not yet been received, dropped, or otherwise released.
    pub subscriber_queue_events_current: usize,
    /// Maximum aggregate admitted compact-event ownership.
    pub subscriber_queue_events_high_water: usize,
    /// Maximum admitted compact-event ownership for any one subscriber.
    pub subscriber_queue_per_subscriber_high_water: usize,
    pub pty_read_calls: u64,
    pub pty_read_bytes: u64,
    pub output_parse_batches: u64,
    pub output_terminal_updates: u64,
    pub output_live_events: u64,
    pub output_subscriber_overflows: u64,
    pub output_processing_ns: u64,
    pub snapshot_builds: u64,
    pub snapshot_build_ns: u64,
    /// Compact publication snapshots built for permit-admitted subscribers.
    pub publication_snapshot_builds: u64,
    pub publication_snapshot_rows: u64,
    pub publication_snapshot_cells: u64,
    pub publication_snapshot_empty_cells: u64,
    pub publication_snapshot_scalar_cells: u64,
    pub publication_snapshot_composed_cells: u64,
    pub publication_snapshot_spacer_cells: u64,
    pub publication_snapshot_owned_string_bytes: u64,
    /// Compact snapshots installed into a subscriber's one-entry latest-state slot.
    pub publication_snapshot_enqueues: u64,
    pub publication_snapshot_enqueued_rows: u64,
    pub publication_snapshot_enqueued_cells: u64,
    pub publication_snapshot_enqueued_owned_string_bytes: u64,
    /// Producer publication batches admitted to compact subscriber mailboxes.
    pub publication_compact_batches: u64,
    /// Additional batches merged into an existing pending compact tail.
    pub publication_compact_batch_merges: u64,
    /// Pending compact tails materialized for first-party wire publication.
    pub publication_compact_materializations: u64,
    pub publication_compact_materialized_batches: u64,
    pub publication_compact_materialized_terminal_updates: u64,
    pub publication_compact_materialized_scrolls: u64,
    pub publication_compact_materialized_appended_rows: u64,
    pub publication_compact_materialized_batches_high_water: u64,
    pub publication_compact_materialized_terminal_updates_high_water: u64,
    pub publication_compact_materialized_scrolls_high_water: u64,
    pub publication_compact_materialized_appended_rows_high_water: u64,
    /// Current and high-water semantic ownership before wire materialization.
    pub queued_compact_batches_current: u64,
    pub queued_compact_batches_high_water: u64,
    pub queued_compact_terminal_updates_current: u64,
    pub queued_compact_terminal_updates_high_water: u64,
    pub queued_compact_scrolls_current: u64,
    pub queued_compact_scrolls_high_water: u64,
    pub queued_compact_appended_rows_current: u64,
    pub queued_compact_appended_rows_high_water: u64,
    /// Compact snapshots currently installed in subscriber latest-state slots.
    pub queued_snapshot_events_current: usize,
    /// Maximum aggregate installed snapshot ownership. This is at most one per
    /// live compact subscriber, independent of semantic-event queue depth.
    pub queued_snapshot_events_high_water: usize,
    /// Rows owned by currently installed compact snapshots.
    pub queued_snapshot_rows_current: u64,
    pub queued_snapshot_rows_high_water: u64,
    /// Cells owned by currently installed compact snapshots.
    pub queued_snapshot_cells_current: u64,
    pub queued_snapshot_cells_high_water: u64,
    /// Heap bytes owned by composed cell strings in installed compact snapshots.
    pub queued_snapshot_owned_string_bytes_current: u64,
    pub queued_snapshot_owned_string_bytes_high_water: u64,
}

#[derive(Debug, Default)]
struct RuntimeMetrics {
    command_queue_high_water: AtomicUsize,
    user_write_queue_high_water_bytes: AtomicUsize,
    reply_write_queue_high_water_bytes: AtomicUsize,
    subscriber_count_high_water: AtomicUsize,
    subscriber_queue_events_current: AtomicUsize,
    subscriber_queue_events_high_water: AtomicUsize,
    subscriber_queue_per_subscriber_high_water: AtomicUsize,
    pty_read_calls: AtomicU64,
    pty_read_bytes: AtomicU64,
    output_parse_batches: AtomicU64,
    output_terminal_updates: AtomicU64,
    output_live_events: AtomicU64,
    output_subscriber_overflows: AtomicU64,
    output_processing_ns: AtomicU64,
    snapshot_builds: AtomicU64,
    snapshot_build_ns: AtomicU64,
    publication_snapshot_builds: AtomicU64,
    publication_snapshot_rows: AtomicU64,
    publication_snapshot_cells: AtomicU64,
    publication_snapshot_empty_cells: AtomicU64,
    publication_snapshot_scalar_cells: AtomicU64,
    publication_snapshot_composed_cells: AtomicU64,
    publication_snapshot_spacer_cells: AtomicU64,
    publication_snapshot_owned_string_bytes: AtomicU64,
    publication_snapshot_enqueues: AtomicU64,
    publication_snapshot_enqueued_rows: AtomicU64,
    publication_snapshot_enqueued_cells: AtomicU64,
    publication_snapshot_enqueued_owned_string_bytes: AtomicU64,
    publication_compact_batches: AtomicU64,
    publication_compact_batch_merges: AtomicU64,
    publication_compact_materializations: AtomicU64,
    publication_compact_materialized_batches: AtomicU64,
    publication_compact_materialized_terminal_updates: AtomicU64,
    publication_compact_materialized_scrolls: AtomicU64,
    publication_compact_materialized_appended_rows: AtomicU64,
    publication_compact_materialized_batches_high_water: AtomicU64,
    publication_compact_materialized_terminal_updates_high_water: AtomicU64,
    publication_compact_materialized_scrolls_high_water: AtomicU64,
    publication_compact_materialized_appended_rows_high_water: AtomicU64,
    queued_compact_batches_current: AtomicU64,
    queued_compact_batches_high_water: AtomicU64,
    queued_compact_terminal_updates_current: AtomicU64,
    queued_compact_terminal_updates_high_water: AtomicU64,
    queued_compact_scrolls_current: AtomicU64,
    queued_compact_scrolls_high_water: AtomicU64,
    queued_compact_appended_rows_current: AtomicU64,
    queued_compact_appended_rows_high_water: AtomicU64,
    queued_snapshot_events_current: AtomicUsize,
    queued_snapshot_events_high_water: AtomicUsize,
    queued_snapshot_rows_current: AtomicU64,
    queued_snapshot_rows_high_water: AtomicU64,
    queued_snapshot_cells_current: AtomicU64,
    queued_snapshot_cells_high_water: AtomicU64,
    queued_snapshot_owned_string_bytes_current: AtomicU64,
    queued_snapshot_owned_string_bytes_high_water: AtomicU64,
}

impl RuntimeMetrics {
    fn observe_max(value: &AtomicUsize, candidate: usize) {
        value.fetch_max(candidate, Ordering::Relaxed);
    }

    fn observe_max_u64(value: &AtomicU64, candidate: u64) {
        value.fetch_max(candidate, Ordering::Relaxed);
    }

    fn add_usize_saturating(value: &AtomicUsize, amount: usize) -> usize {
        let previous = value
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.saturating_add(amount))
            })
            .unwrap_or_else(|current| current);
        previous.saturating_add(amount)
    }

    fn sub_usize_saturating(value: &AtomicUsize, amount: usize) {
        let _ = value.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            Some(current.saturating_sub(amount))
        });
    }

    fn add_u64_saturating(value: &AtomicU64, amount: u64) -> u64 {
        let previous = value
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.saturating_add(amount))
            })
            .unwrap_or_else(|current| current);
        previous.saturating_add(amount)
    }

    fn sub_u64_saturating(value: &AtomicU64, amount: u64) {
        let _ = value.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            Some(current.saturating_sub(amount))
        });
    }

    fn add_saturating(value: &AtomicU64, amount: u64) {
        let _ = Self::add_u64_saturating(value, amount);
    }

    fn add_queued_snapshot(&self, attribution: SnapshotAttribution) {
        let rows = Self::add_u64_saturating(&self.queued_snapshot_rows_current, attribution.rows);
        Self::observe_max_u64(&self.queued_snapshot_rows_high_water, rows);
        let cells =
            Self::add_u64_saturating(&self.queued_snapshot_cells_current, attribution.cells);
        Self::observe_max_u64(&self.queued_snapshot_cells_high_water, cells);
        let strings = Self::add_u64_saturating(
            &self.queued_snapshot_owned_string_bytes_current,
            attribution.owned_string_bytes,
        );
        Self::observe_max_u64(&self.queued_snapshot_owned_string_bytes_high_water, strings);
    }

    fn remove_queued_snapshot(&self, attribution: SnapshotAttribution) {
        Self::sub_u64_saturating(&self.queued_snapshot_rows_current, attribution.rows);
        Self::sub_u64_saturating(&self.queued_snapshot_cells_current, attribution.cells);
        Self::sub_u64_saturating(
            &self.queued_snapshot_owned_string_bytes_current,
            attribution.owned_string_bytes,
        );
    }

    fn add_queued_compact(&self, attribution: PendingFrameAttribution) {
        RuntimeMetrics::add_saturating(&self.publication_compact_batches, attribution.batches);
        let batches =
            Self::add_u64_saturating(&self.queued_compact_batches_current, attribution.batches);
        Self::observe_max_u64(&self.queued_compact_batches_high_water, batches);
        let updates = Self::add_u64_saturating(
            &self.queued_compact_terminal_updates_current,
            attribution.terminal_updates,
        );
        Self::observe_max_u64(&self.queued_compact_terminal_updates_high_water, updates);
        let scrolls =
            Self::add_u64_saturating(&self.queued_compact_scrolls_current, attribution.scrolls);
        Self::observe_max_u64(&self.queued_compact_scrolls_high_water, scrolls);
        let appended = Self::add_u64_saturating(
            &self.queued_compact_appended_rows_current,
            attribution.appended_rows,
        );
        Self::observe_max_u64(&self.queued_compact_appended_rows_high_water, appended);
    }

    fn remove_queued_compact(&self, attribution: PendingFrameAttribution) {
        Self::sub_u64_saturating(&self.queued_compact_batches_current, attribution.batches);
        Self::sub_u64_saturating(
            &self.queued_compact_terminal_updates_current,
            attribution.terminal_updates,
        );
        Self::sub_u64_saturating(&self.queued_compact_scrolls_current, attribution.scrolls);
        Self::sub_u64_saturating(
            &self.queued_compact_appended_rows_current,
            attribution.appended_rows,
        );
    }

    fn record_compact_materialization(&self, attribution: PendingFrameAttribution) {
        Self::add_saturating(&self.publication_compact_materializations, 1);
        Self::add_saturating(
            &self.publication_compact_materialized_batches,
            attribution.batches,
        );
        Self::add_saturating(
            &self.publication_compact_materialized_terminal_updates,
            attribution.terminal_updates,
        );
        Self::add_saturating(
            &self.publication_compact_materialized_scrolls,
            attribution.scrolls,
        );
        Self::add_saturating(
            &self.publication_compact_materialized_appended_rows,
            attribution.appended_rows,
        );
        Self::observe_max_u64(
            &self.publication_compact_materialized_batches_high_water,
            attribution.batches,
        );
        Self::observe_max_u64(
            &self.publication_compact_materialized_terminal_updates_high_water,
            attribution.terminal_updates,
        );
        Self::observe_max_u64(
            &self.publication_compact_materialized_scrolls_high_water,
            attribution.scrolls,
        );
        Self::observe_max_u64(
            &self.publication_compact_materialized_appended_rows_high_water,
            attribution.appended_rows,
        );
    }

    #[allow(
        clippy::too_many_lines,
        reason = "runtime metrics snapshot explicitly loads every independent counter"
    )]
    fn snapshot(&self) -> LiveRuntimeMetrics {
        LiveRuntimeMetrics {
            command_queue_high_water: self.command_queue_high_water.load(Ordering::Relaxed),
            user_write_queue_high_water_bytes: self
                .user_write_queue_high_water_bytes
                .load(Ordering::Relaxed),
            reply_write_queue_high_water_bytes: self
                .reply_write_queue_high_water_bytes
                .load(Ordering::Relaxed),
            subscriber_count_high_water: self.subscriber_count_high_water.load(Ordering::Relaxed),
            subscriber_queue_events_current: self
                .subscriber_queue_events_current
                .load(Ordering::Relaxed),
            subscriber_queue_events_high_water: self
                .subscriber_queue_events_high_water
                .load(Ordering::Relaxed),
            subscriber_queue_per_subscriber_high_water: self
                .subscriber_queue_per_subscriber_high_water
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
            publication_snapshot_builds: self.publication_snapshot_builds.load(Ordering::Relaxed),
            publication_snapshot_rows: self.publication_snapshot_rows.load(Ordering::Relaxed),
            publication_snapshot_cells: self.publication_snapshot_cells.load(Ordering::Relaxed),
            publication_snapshot_empty_cells: self
                .publication_snapshot_empty_cells
                .load(Ordering::Relaxed),
            publication_snapshot_scalar_cells: self
                .publication_snapshot_scalar_cells
                .load(Ordering::Relaxed),
            publication_snapshot_composed_cells: self
                .publication_snapshot_composed_cells
                .load(Ordering::Relaxed),
            publication_snapshot_spacer_cells: self
                .publication_snapshot_spacer_cells
                .load(Ordering::Relaxed),
            publication_snapshot_owned_string_bytes: self
                .publication_snapshot_owned_string_bytes
                .load(Ordering::Relaxed),
            publication_snapshot_enqueues: self
                .publication_snapshot_enqueues
                .load(Ordering::Relaxed),
            publication_snapshot_enqueued_rows: self
                .publication_snapshot_enqueued_rows
                .load(Ordering::Relaxed),
            publication_snapshot_enqueued_cells: self
                .publication_snapshot_enqueued_cells
                .load(Ordering::Relaxed),
            publication_snapshot_enqueued_owned_string_bytes: self
                .publication_snapshot_enqueued_owned_string_bytes
                .load(Ordering::Relaxed),
            publication_compact_batches: self.publication_compact_batches.load(Ordering::Relaxed),
            publication_compact_batch_merges: self
                .publication_compact_batch_merges
                .load(Ordering::Relaxed),
            publication_compact_materializations: self
                .publication_compact_materializations
                .load(Ordering::Relaxed),
            publication_compact_materialized_batches: self
                .publication_compact_materialized_batches
                .load(Ordering::Relaxed),
            publication_compact_materialized_terminal_updates: self
                .publication_compact_materialized_terminal_updates
                .load(Ordering::Relaxed),
            publication_compact_materialized_scrolls: self
                .publication_compact_materialized_scrolls
                .load(Ordering::Relaxed),
            publication_compact_materialized_appended_rows: self
                .publication_compact_materialized_appended_rows
                .load(Ordering::Relaxed),
            publication_compact_materialized_batches_high_water: self
                .publication_compact_materialized_batches_high_water
                .load(Ordering::Relaxed),
            publication_compact_materialized_terminal_updates_high_water: self
                .publication_compact_materialized_terminal_updates_high_water
                .load(Ordering::Relaxed),
            publication_compact_materialized_scrolls_high_water: self
                .publication_compact_materialized_scrolls_high_water
                .load(Ordering::Relaxed),
            publication_compact_materialized_appended_rows_high_water: self
                .publication_compact_materialized_appended_rows_high_water
                .load(Ordering::Relaxed),
            queued_compact_batches_current: self
                .queued_compact_batches_current
                .load(Ordering::Relaxed),
            queued_compact_batches_high_water: self
                .queued_compact_batches_high_water
                .load(Ordering::Relaxed),
            queued_compact_terminal_updates_current: self
                .queued_compact_terminal_updates_current
                .load(Ordering::Relaxed),
            queued_compact_terminal_updates_high_water: self
                .queued_compact_terminal_updates_high_water
                .load(Ordering::Relaxed),
            queued_compact_scrolls_current: self
                .queued_compact_scrolls_current
                .load(Ordering::Relaxed),
            queued_compact_scrolls_high_water: self
                .queued_compact_scrolls_high_water
                .load(Ordering::Relaxed),
            queued_compact_appended_rows_current: self
                .queued_compact_appended_rows_current
                .load(Ordering::Relaxed),
            queued_compact_appended_rows_high_water: self
                .queued_compact_appended_rows_high_water
                .load(Ordering::Relaxed),
            queued_snapshot_events_current: self
                .queued_snapshot_events_current
                .load(Ordering::Relaxed),
            queued_snapshot_events_high_water: self
                .queued_snapshot_events_high_water
                .load(Ordering::Relaxed),
            queued_snapshot_rows_current: self.queued_snapshot_rows_current.load(Ordering::Relaxed),
            queued_snapshot_rows_high_water: self
                .queued_snapshot_rows_high_water
                .load(Ordering::Relaxed),
            queued_snapshot_cells_current: self
                .queued_snapshot_cells_current
                .load(Ordering::Relaxed),
            queued_snapshot_cells_high_water: self
                .queued_snapshot_cells_high_water
                .load(Ordering::Relaxed),
            queued_snapshot_owned_string_bytes_current: self
                .queued_snapshot_owned_string_bytes_current
                .load(Ordering::Relaxed),
            queued_snapshot_owned_string_bytes_high_water: self
                .queued_snapshot_owned_string_bytes_high_water
                .load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LiveSplintHandle {
    pub splint_id: SplintId,
    pub incarnation: ProcessIncarnation,
    child_pid: u32,
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
    #[must_use]
    pub const fn child_pid(&self) -> u32 {
        self.child_pid
    }

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

    /// Attaches through the additive compact publication path used by the
    /// first-party daemon.
    pub async fn attach_compact_with_scrollback(
        &self,
        max_scrollback_rows: usize,
    ) -> Result<(LiveSnapshot, CompactSubscription), LiveError> {
        self.request(|reply| {
            Command::AttachCompact(
                max_scrollback_rows.min(self.default_snapshot_rows),
                self.default_subscriber_capacity,
                reply,
            )
        })
        .await
    }

    /// Subscribes through the additive compact publication path.
    pub async fn subscribe_compact_with_capacity(
        &self,
        capacity: usize,
    ) -> Result<CompactSubscription, LiveError> {
        self.request(|reply| Command::SubscribeCompact(capacity, self.default_snapshot_rows, reply))
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

    pub async fn shutdown(&self) -> Result<(), LiveError> {
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
        Self::spawn_inner(splint_id, backend, command, config, false).await
    }

    /// Spawns a runtime with default-off compact-publication ownership metrics
    /// enabled for an explicit benchmark or diagnostic run.
    pub async fn spawn_with_publication_memory_metrics(
        splint_id: SplintId,
        backend: LinuxPtyBackend,
        command: PtyCommand,
        config: LiveSplintConfig,
    ) -> Result<Self, LiveError> {
        Self::spawn_inner(splint_id, backend, command, config, true).await
    }

    async fn spawn_inner(
        splint_id: SplintId,
        backend: LinuxPtyBackend,
        command: PtyCommand,
        config: LiveSplintConfig,
        publication_memory_metrics: bool,
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
            publication_memory_metrics,
        ))
    }

    fn from_session(
        splint_id: SplintId,
        incarnation: ProcessIncarnation,
        session: LinuxPtySession,
        io: AsyncFd<std::fs::File>,
        config: LiveSplintConfig,
        publication_memory_metrics: bool,
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
        let child_pid = session.child_id();
        let handle = LiveSplintHandle {
            splint_id,
            incarnation,
            child_pid,
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
            publication_memory_metrics,
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

enum SubscriberEvents {
    Legacy(mpsc::Sender<LiveEvent>),
    Compact {
        sender: mpsc::Sender<CompactQueuedEvent>,
        accounting: Arc<QueueAccounting>,
        snapshot_slot: Arc<CompactSnapshotSlot>,
        semantic_capacity: usize,
    },
}

impl SubscriberEvents {
    fn is_closed(&self) -> bool {
        match self {
            Self::Legacy(sender) => sender.is_closed(),
            Self::Compact { sender, .. } => sender.is_closed(),
        }
    }

    fn capacity(&self) -> usize {
        match self {
            Self::Legacy(sender) => sender.capacity(),
            Self::Compact { sender, .. } => sender.capacity(),
        }
    }

    #[cfg(test)]
    fn snapshot_slot(&self) -> &CompactSnapshotSlot {
        match self {
            Self::Compact { snapshot_slot, .. } => snapshot_slot,
            Self::Legacy(_) => panic!("legacy subscriber has no compact snapshot slot"),
        }
    }
}

struct Subscriber {
    events: SubscriberEvents,
    resnapshot: watch::Sender<bool>,
    published_revision: TerminalRevision,
    published_history_generation: u64,
    snapshot_rows: usize,
}

impl Subscriber {
    fn require_resnapshot(&self) {
        if let SubscriberEvents::Compact { snapshot_slot, .. } = &self.events {
            snapshot_slot.clear();
        }
        self.resnapshot.send_replace(true);
    }
}

struct CompactProducerBatch {
    slots: Vec<Arc<CompactSnapshotSlot>>,
}

impl CompactProducerBatch {
    fn begin(subscribers: &[Subscriber]) -> Self {
        let slots: Vec<_> = subscribers
            .iter()
            .filter_map(|subscriber| match &subscriber.events {
                SubscriberEvents::Compact { snapshot_slot, .. } => Some(Arc::clone(snapshot_slot)),
                SubscriberEvents::Legacy(_) => None,
            })
            .collect();
        for slot in &slots {
            slot.begin_producer_batch();
        }
        Self { slots }
    }
}

impl Drop for CompactProducerBatch {
    fn drop(&mut self) {
        for slot in &self.slots {
            slot.end_producer_batch();
        }
    }
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
    publication_memory_metrics: bool,
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
        publication_memory_metrics,
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
    publication_memory_metrics: bool,
    metrics: &Arc<RuntimeMetrics>,
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
                        publication_memory_metrics,
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
                                metrics,
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
                            if output.live_events > 0
                                && subscribers.iter().any(|subscriber| {
                                    matches!(subscriber.events, SubscriberEvents::Compact { .. })
                                })
                            {
                                // One PTY read can synchronously publish exactly the
                                // complete bounded semantic tail. Yield once before
                                // another readable turn so an already-woken compact
                                // consumer can take that tail without a false overflow.
                                tokio::task::yield_now().await;
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
                    metrics,
                );
            }
            _ = interval.tick() => {
                if child_exit.is_none() && let Some(status) = session.try_wait()? {
                    child_exit = Some(status.into());
                    drain_deadline = Some(Instant::now() + config.exit_drain_timeout);
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
        metrics,
    );
    publish_exit(&mut subscribers, incarnation, status);
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
    publication_memory_metrics: bool,
    metrics: &Arc<RuntimeMetrics>,
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
                    metrics,
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
                events: SubscriberEvents::Legacy(event_sender),
                resnapshot,
                published_revision: terminal.revision(),
                published_history_generation: terminal_history_generation(terminal),
                snapshot_rows: max_rows.min(config.max_scrollback_snapshot_rows),
            });
            if publication_memory_metrics {
                RuntimeMetrics::observe_max(
                    &metrics.subscriber_count_high_water,
                    subscribers.len(),
                );
            }
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
                events: SubscriberEvents::Legacy(event_sender),
                resnapshot,
                published_revision: terminal.revision(),
                published_history_generation: terminal_history_generation(terminal),
                snapshot_rows: max_rows.min(config.max_scrollback_snapshot_rows),
            });
            if publication_memory_metrics {
                RuntimeMetrics::observe_max(
                    &metrics.subscriber_count_high_water,
                    subscribers.len(),
                );
            }
            let subscription = Subscription {
                events,
                resnapshot: resnapshot_receiver,
            };
            let _ = reply.send(Ok((snapshot, subscription)));
        }
        Command::SubscribeCompact(capacity, max_rows, reply) => {
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
            let accounting = Arc::new(QueueAccounting::new(
                publication_memory_metrics,
                Arc::clone(metrics),
            ));
            let snapshot_slot = Arc::new(CompactSnapshotSlot::default());
            let (resnapshot, resnapshot_receiver) = watch::channel(false);
            subscribers.push(Subscriber {
                events: SubscriberEvents::Compact {
                    sender: event_sender,
                    accounting: Arc::clone(&accounting),
                    snapshot_slot: Arc::clone(&snapshot_slot),
                    semantic_capacity: event_capacity - 1,
                },
                resnapshot,
                published_revision: terminal.revision(),
                published_history_generation: terminal_history_generation(terminal),
                snapshot_rows: max_rows.min(config.max_scrollback_snapshot_rows),
            });
            if publication_memory_metrics {
                RuntimeMetrics::observe_max(
                    &metrics.subscriber_count_high_water,
                    subscribers.len(),
                );
            }
            let _ = reply.send(Ok(CompactSubscription {
                events,
                resnapshot: resnapshot_receiver,
                accounting,
                snapshot_slot,
            }));
        }
        Command::AttachCompact(max_rows, capacity, reply) => {
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
            let accounting = Arc::new(QueueAccounting::new(
                publication_memory_metrics,
                Arc::clone(metrics),
            ));
            let snapshot_slot = Arc::new(CompactSnapshotSlot::default());
            let (resnapshot, resnapshot_receiver) = watch::channel(false);
            subscribers.push(Subscriber {
                events: SubscriberEvents::Compact {
                    sender: event_sender,
                    accounting: Arc::clone(&accounting),
                    snapshot_slot: Arc::clone(&snapshot_slot),
                    semantic_capacity: event_capacity - 1,
                },
                resnapshot,
                published_revision: terminal.revision(),
                published_history_generation: terminal_history_generation(terminal),
                snapshot_rows: max_rows.min(config.max_scrollback_snapshot_rows),
            });
            if publication_memory_metrics {
                RuntimeMetrics::observe_max(
                    &metrics.subscriber_count_high_water,
                    subscribers.len(),
                );
            }
            let subscription = CompactSubscription {
                events,
                resnapshot: resnapshot_receiver,
                accounting,
                snapshot_slot,
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

#[allow(
    clippy::too_many_lines,
    reason = "subscriber publication keeps ordering, admission, and resync decisions in one transaction"
)]
fn publish_updates(
    splint_id: SplintId,
    terminal: &Terminal,
    publication: &mut SynchronizedPublication,
    incarnation: ProcessIncarnation,
    child_exit: Option<ProcessExit>,
    subscribers: &mut Vec<Subscriber>,
    metrics: &Arc<RuntimeMetrics>,
) -> (usize, usize) {
    let update_count = terminal
        .update_count_since(publication.published_revision)
        .unwrap_or(0);
    publication.published_revision = terminal.revision();
    let terminal_metadata = terminal.snapshot(SnapshotRequest {
        max_scrollback_rows: 0,
    });
    let terminal_dimensions = terminal_metadata.dimensions();
    let terminal_active_screen = terminal_metadata.active_screen();
    let terminal_history_generation = terminal_metadata.scrollback().history_generation;
    drop(terminal_metadata);

    let mut overflows = 0_usize;
    subscribers.retain_mut(|subscriber| {
        if subscriber.events.is_closed() {
            return false;
        }
        let Ok(batch) = terminal.updates_since(subscriber.published_revision) else {
            subscriber.require_resnapshot();
            overflows = overflows.saturating_add(1);
            return false;
        };
        let updates = batch.into_updates();
        if updates.is_empty() {
            return true;
        }
        // One internal slot is reserved for the terminal Exited event.
        if subscriber.events.capacity() <= 1 {
            subscriber.require_resnapshot();
            overflows = overflows.saturating_add(1);
            return false;
        }

        let snapshot_rows = subscriber.snapshot_rows;
        let history_policy =
            compact_history_policy(&updates, terminal_dimensions, terminal_active_screen);
        let previous_history_generation = subscriber.published_history_generation;
        let admitted = match &subscriber.events {
            SubscriberEvents::Legacy(sender) => {
                let permit = match sender.try_reserve() {
                    Ok(permit) => permit,
                    Err(mpsc::error::TrySendError::Full(())) => {
                        subscriber.resnapshot.send_replace(true);
                        overflows = overflows.saturating_add(1);
                        return false;
                    }
                    Err(mpsc::error::TrySendError::Closed(())) => return false,
                };
                let snapshot =
                    owned_snapshot(splint_id, incarnation, terminal, snapshot_rows, child_exit);
                permit.send(LiveEvent::Update {
                    incarnation,
                    updates,
                    snapshot: Box::new(snapshot),
                });
                true
            }
            SubscriberEvents::Compact {
                sender,
                accounting,
                snapshot_slot,
                semantic_capacity,
            } => match publish_compact_update(
                sender,
                accounting,
                snapshot_slot,
                *semantic_capacity,
                incarnation,
                updates,
                terminal.revision(),
                history_policy,
                metrics,
                |policy| {
                    let snapshot = compact_snapshot_with_history(
                        splint_id,
                        incarnation,
                        terminal,
                        snapshot_rows,
                        child_exit,
                        policy,
                    );
                    if policy != CompactHistoryPolicy::FullHistory
                        && snapshot.metadata.scrollback.history_generation
                            != previous_history_generation
                    {
                        compact_snapshot_with_history(
                            splint_id,
                            incarnation,
                            terminal,
                            snapshot_rows,
                            child_exit,
                            CompactHistoryPolicy::FullHistory,
                        )
                    } else {
                        snapshot
                    }
                },
            ) {
                CompactPublishOutcome::Published => true,
                CompactPublishOutcome::Full => {
                    snapshot_slot.clear();
                    subscriber.resnapshot.send_replace(true);
                    overflows = overflows.saturating_add(1);
                    false
                }
                CompactPublishOutcome::Closed => {
                    snapshot_slot.clear();
                    false
                }
            },
        };
        if admitted {
            subscriber.published_revision = terminal.revision();
            subscriber.published_history_generation = terminal_history_generation;
        }
        admitted
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
    clippy::too_many_lines,
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
    runtime_metrics: &Arc<RuntimeMetrics>,
    reply_limit: usize,
) -> Result<ProcessOutputMetrics, LiveError> {
    let _compact_batch = CompactProducerBatch::begin(subscribers);
    let mut metrics = ProcessOutputMetrics::default();
    let trace_enabled = perf_trace_enabled();
    let trace_base_revision = terminal.revision();
    let mut mutation_ns = 0_u64;
    let mut publication_ns = 0_u64;
    for batch in bytes.chunks(PARSE_BATCH) {
        metrics.parse_batches = metrics.parse_batches.saturating_add(1);
        let image_metrics_before = terminal.image_metrics();
        let parse_started = Instant::now();
        let mut remaining = batch;
        while !remaining.is_empty() {
            let revision_before = terminal.revision();
            let mutation_started = trace_enabled.then(Instant::now);
            let (consumed, completed_frame) = terminal.advance_to_synchronized_boundary(remaining);
            if let Some(started) = mutation_started {
                mutation_ns = mutation_ns.saturating_add(
                    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
                );
            }
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
                let publication_started = trace_enabled.then(Instant::now);
                let result = publish_updates(
                    splint_id,
                    terminal,
                    publication,
                    incarnation,
                    child_exit,
                    subscribers,
                    runtime_metrics,
                );
                if let Some(started) = publication_started {
                    publication_ns = publication_ns.saturating_add(
                        u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
                    );
                }
                result
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
    if trace_enabled {
        let common = PerfTraceEvent {
            splint_id: Some(splint_id),
            incarnation: Some(incarnation.value()),
            base_revision: Some(trace_base_revision.value()),
            revision: Some(terminal.revision().value()),
            bytes: Some(u64::try_from(bytes.len()).unwrap_or(u64::MAX)),
            count: Some(metrics.parse_batches),
            ..PerfTraceEvent::default()
        };
        emit_perf_trace(
            "splinterd",
            "terminal_mutation",
            PerfTraceEvent {
                duration_ns: Some(mutation_ns),
                ..common
            },
        );
        emit_perf_trace(
            "splinterd",
            "daemon_publication",
            PerfTraceEvent {
                duration_ns: Some(publication_ns),
                count: Some(metrics.live_events),
                resync: Some(metrics.subscriber_overflows > 0),
                ..common
            },
        );
    }
    Ok(metrics)
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "fanout takes ownership of the event and clones only for retained subscribers"
)]
fn publish_exit(
    subscribers: &mut Vec<Subscriber>,
    incarnation: ProcessIncarnation,
    status: ProcessExit,
) -> usize {
    let mut overflows = 0_usize;
    subscribers.retain(|subscriber| match &subscriber.events {
        SubscriberEvents::Legacy(sender) => {
            let permit = match sender.try_reserve() {
                Ok(permit) => permit,
                Err(mpsc::error::TrySendError::Full(())) => {
                    overflows = overflows.saturating_add(1);
                    subscriber.resnapshot.send_replace(true);
                    return false;
                }
                Err(mpsc::error::TrySendError::Closed(())) => return false,
            };
            permit.send(LiveEvent::Exited {
                incarnation,
                status,
            });
            true
        }
        SubscriberEvents::Compact {
            sender,
            accounting,
            snapshot_slot,
            ..
        } => {
            let permit = match sender.try_reserve() {
                Ok(permit) => permit,
                Err(mpsc::error::TrySendError::Full(())) => {
                    overflows = overflows.saturating_add(1);
                    snapshot_slot.clear();
                    subscriber.resnapshot.send_replace(true);
                    return false;
                }
                Err(mpsc::error::TrySendError::Closed(())) => {
                    snapshot_slot.clear();
                    return false;
                }
            };
            send_permit_admitted_compact(
                sender,
                permit,
                accounting,
                |admitted| CompactQueuedEvent::Exited {
                    incarnation,
                    status,
                    admitted,
                },
                || {},
            );
            true
        }
    });
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

fn terminal_history_generation(terminal: &Terminal) -> u64 {
    terminal
        .snapshot(SnapshotRequest {
            max_scrollback_rows: 0,
        })
        .scrollback()
        .history_generation
}

fn owned_snapshot(
    splint_id: SplintId,
    incarnation: ProcessIncarnation,
    terminal: &Terminal,
    max_rows: usize,
    exited: Option<ProcessExit>,
) -> LiveSnapshot {
    let trace_started = perf_trace_enabled().then(Instant::now);
    let snapshot = terminal.snapshot(SnapshotRequest {
        max_scrollback_rows: max_rows,
    });
    let owned = LiveSnapshot {
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
    };
    emit_owned_snapshot_trace(
        trace_started,
        splint_id,
        incarnation,
        owned.revision,
        owned.visible_rows.len() + owned.scrollback_rows.len(),
        owned
            .visible_rows
            .iter()
            .chain(&owned.scrollback_rows)
            .map(|row| row.cells.len())
            .sum(),
    );
    owned
}

#[cfg(test)]
fn compact_snapshot(
    splint_id: SplintId,
    incarnation: ProcessIncarnation,
    terminal: &Terminal,
    max_rows: usize,
    exited: Option<ProcessExit>,
) -> CompactLiveSnapshot {
    compact_snapshot_with_history(
        splint_id,
        incarnation,
        terminal,
        max_rows,
        exited,
        CompactHistoryPolicy::FullHistory,
    )
}

fn compact_snapshot_with_history(
    splint_id: SplintId,
    incarnation: ProcessIncarnation,
    terminal: &Terminal,
    max_rows: usize,
    exited: Option<ProcessExit>,
    history_policy: CompactHistoryPolicy,
) -> CompactLiveSnapshot {
    let trace_started = perf_trace_enabled().then(Instant::now);
    let requested_rows = match history_policy {
        CompactHistoryPolicy::FullHistory => max_rows,
        CompactHistoryPolicy::NoHistory => usize::from(max_rows > 0),
        CompactHistoryPolicy::AppendTail(rows) => rows.min(max_rows),
    };
    let snapshot = terminal.snapshot(SnapshotRequest {
        max_scrollback_rows: requested_rows,
    });
    let visible_rows = snapshot.visible_rows().map(compact_row).collect::<Vec<_>>();
    let scrollback_rows = if history_policy == CompactHistoryPolicy::NoHistory {
        Vec::new()
    } else {
        snapshot
            .scrollback_rows()
            .map(compact_row)
            .collect::<Vec<_>>()
    };
    let rows = visible_rows.len() + scrollback_rows.len();
    let cells = visible_rows
        .iter()
        .chain(&scrollback_rows)
        .map(|row| row.cells.len())
        .sum();
    let mut scrollback = snapshot.scrollback();
    if history_policy == CompactHistoryPolicy::NoHistory {
        scrollback.returned_rows = 0;
        scrollback.omitted_oldest_rows = scrollback.available_rows;
    }
    let metadata = LiveSnapshot {
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
        visible_rows: Vec::new(),
        scrollback_rows: Vec::new(),
        scrollback,
        exited,
    };
    emit_owned_snapshot_trace(
        trace_started,
        splint_id,
        incarnation,
        metadata.revision,
        rows,
        cells,
    );
    CompactLiveSnapshot {
        metadata,
        visible_rows,
        scrollback_rows,
        history_policy,
    }
}

fn emit_owned_snapshot_trace(
    started: Option<Instant>,
    splint_id: SplintId,
    incarnation: ProcessIncarnation,
    revision: TerminalRevision,
    rows: usize,
    cells: usize,
) {
    if let Some(started) = started {
        emit_perf_trace(
            "splinterd",
            "owned_snapshot",
            PerfTraceEvent {
                splint_id: Some(splint_id),
                incarnation: Some(incarnation.value()),
                revision: Some(revision.value()),
                duration_ns: Some(u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)),
                rows: Some(u64::try_from(rows).unwrap_or(u64::MAX)),
                cells: Some(u64::try_from(cells).unwrap_or(u64::MAX)),
                ..PerfTraceEvent::default()
            },
        );
    }
}

fn compact_snapshot_attribution(snapshot: &CompactLiveSnapshot) -> SnapshotAttribution {
    let mut attribution = SnapshotAttribution {
        rows: u64::try_from(snapshot.visible_rows.len() + snapshot.scrollback_rows.len())
            .unwrap_or(u64::MAX),
        ..SnapshotAttribution::default()
    };
    for cell in snapshot
        .visible_rows
        .iter()
        .chain(&snapshot.scrollback_rows)
        .flat_map(|row| &row.cells)
    {
        attribution.cells = attribution.cells.saturating_add(1);
        match &cell.content {
            CompactCellContent::Empty => {
                attribution.empty_cells = attribution.empty_cells.saturating_add(1);
            }
            CompactCellContent::Scalar(_) => {
                attribution.scalar_cells = attribution.scalar_cells.saturating_add(1);
            }
            CompactCellContent::Composed(_) => {
                attribution.composed_cells = attribution.composed_cells.saturating_add(1);
                attribution.owned_string_bytes = attribution.owned_string_bytes.saturating_add(
                    u64::try_from(cell.content.owned_string_bytes()).unwrap_or(u64::MAX),
                );
            }
            CompactCellContent::Spacer { .. } => {
                attribution.spacer_cells = attribution.spacer_cells.saturating_add(1);
            }
        }
    }
    attribution
}

fn record_publication_snapshot(metrics: &RuntimeMetrics, attribution: SnapshotAttribution) {
    RuntimeMetrics::add_saturating(&metrics.publication_snapshot_builds, 1);
    RuntimeMetrics::add_saturating(&metrics.publication_snapshot_rows, attribution.rows);
    RuntimeMetrics::add_saturating(&metrics.publication_snapshot_cells, attribution.cells);
    RuntimeMetrics::add_saturating(
        &metrics.publication_snapshot_empty_cells,
        attribution.empty_cells,
    );
    RuntimeMetrics::add_saturating(
        &metrics.publication_snapshot_scalar_cells,
        attribution.scalar_cells,
    );
    RuntimeMetrics::add_saturating(
        &metrics.publication_snapshot_composed_cells,
        attribution.composed_cells,
    );
    RuntimeMetrics::add_saturating(
        &metrics.publication_snapshot_spacer_cells,
        attribution.spacer_cells,
    );
    RuntimeMetrics::add_saturating(
        &metrics.publication_snapshot_owned_string_bytes,
        attribution.owned_string_bytes,
    );
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

fn compact_row(row: splinterm_terminal::RowSnapshot<'_>) -> CompactLiveRow {
    CompactLiveRow {
        row_id: row.id(),
        linebreak: row.linebreak(),
        cells: row
            .cells()
            .map(|cell| CompactLiveCell {
                content: match cell.content() {
                    CellSnapshotContent::Empty => CompactCellContent::Empty,
                    CellSnapshotContent::Scalar(character) => CompactCellContent::Scalar(character),
                    CellSnapshotContent::Composed(characters) => {
                        CompactCellContent::Composed(characters.iter().collect())
                    }
                    CellSnapshotContent::Spacer { remaining } => {
                        CompactCellContent::Spacer { remaining }
                    }
                },
                attributes: cell.attributes(),
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

    fn test_subscriber(
        terminal: &Terminal,
        capacity: usize,
        enabled: bool,
        metrics: Arc<RuntimeMetrics>,
    ) -> (Subscriber, CompactSubscription, watch::Receiver<bool>) {
        test_subscriber_with_rows(terminal, capacity, enabled, metrics, 0)
    }

    fn test_subscriber_with_rows(
        terminal: &Terminal,
        capacity: usize,
        enabled: bool,
        metrics: Arc<RuntimeMetrics>,
        snapshot_rows: usize,
    ) -> (Subscriber, CompactSubscription, watch::Receiver<bool>) {
        let (events, receiver) = mpsc::channel(capacity);
        let accounting = Arc::new(QueueAccounting::new(enabled, metrics));
        let snapshot_slot = Arc::new(CompactSnapshotSlot::default());
        let (resnapshot, resnapshot_receiver) = watch::channel(false);
        (
            Subscriber {
                events: SubscriberEvents::Compact {
                    sender: events,
                    accounting: Arc::clone(&accounting),
                    snapshot_slot: Arc::clone(&snapshot_slot),
                    semantic_capacity: capacity - 1,
                },
                resnapshot,
                published_revision: terminal.revision(),
                published_history_generation: terminal_history_generation(terminal),
                snapshot_rows,
            },
            CompactSubscription {
                events: receiver,
                resnapshot: resnapshot_receiver.clone(),
                accounting,
                snapshot_slot,
            },
            resnapshot_receiver,
        )
    }

    fn synchronized_test_subscriber(
        terminal: &Terminal,
    ) -> (Subscriber, mpsc::Receiver<LiveEvent>, watch::Receiver<bool>) {
        let (events, receiver) = mpsc::channel(4);
        let (resnapshot, resnapshot_receiver) = watch::channel(false);
        (
            Subscriber {
                events: SubscriberEvents::Legacy(events),
                resnapshot,
                published_revision: terminal.revision(),
                published_history_generation: terminal_history_generation(terminal),
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
        let attribution = Arc::new(RuntimeMetrics::default());

        let metrics = process_output(
            b"\x1b[2026lA\x1b[5n",
            SplintId::new(),
            incarnation,
            None,
            &mut terminal,
            &mut replies,
            &mut subscribers,
            &mut publication,
            &attribution,
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
            &attribution,
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
        let attribution = Arc::new(RuntimeMetrics::default());

        let metrics = process_output(
            b"\x1b[2026lA\x1b[2026h\x1b\\\x1b[2026lB",
            SplintId::new(),
            incarnation,
            None,
            &mut terminal,
            &mut replies,
            &mut subscribers,
            &mut publication,
            &attribution,
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
        let attribution = Arc::new(RuntimeMetrics::default());

        process_output(
            b"\x1b[2026lA\x1b[2026h",
            SplintId::new(),
            incarnation,
            None,
            &mut terminal,
            &mut replies,
            &mut subscribers,
            &mut publication,
            &attribution,
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
            &attribution,
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
            &attribution,
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
        let metrics = Arc::new(RuntimeMetrics::default());
        let (events, receiver) = mpsc::channel(2);
        let (resnapshot, resnapshot_receiver) = watch::channel(false);
        let mut subscribers = vec![Subscriber {
            events: SubscriberEvents::Legacy(events),
            resnapshot,
            published_revision: terminal.revision(),
            published_history_generation: terminal_history_generation(&terminal),
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
            &metrics,
        );
        let status = ProcessExit {
            code: Some(0),
            signal: None,
        };
        assert_eq!(publish_exit(&mut subscribers, incarnation, status), 0);
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

    #[allow(
        clippy::similar_names,
        reason = "receiver names distinguish the channel from its delivered event"
    )]
    #[tokio::test]
    async fn coalesced_receive_materializes_only_the_latest_snapshot_and_preserves_order() {
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        let metrics = Arc::new(RuntimeMetrics::default());
        let (subscriber, receiver, _resnapshot) =
            test_subscriber(&terminal, 8, true, Arc::clone(&metrics));
        let materializations = Arc::clone(&receiver.accounting);
        let mut subscription = receiver;
        let mut subscribers = vec![subscriber];
        let mut publication = SynchronizedPublication::new(terminal.revision());

        for byte in *b"ABC" {
            terminal.advance(&[byte]);
            assert_eq!(
                publish_updates(
                    SplintId::new(),
                    &terminal,
                    &mut publication,
                    incarnation,
                    None,
                    &mut subscribers,
                    &metrics,
                ),
                (1, 0)
            );
        }
        assert_eq!(
            materializations.materializations.load(Ordering::Relaxed),
            0,
            "queued compact snapshots must not allocate public cell strings"
        );

        let (received, trailing_exit) = subscription.recv_coalesced().await;
        assert_eq!(trailing_exit, None);
        let SubscriptionReceive::Event(LiveEvent::Update {
            updates, snapshot, ..
        }) = received
        else {
            panic!("coalesced receive did not return the retained update");
        };
        assert!(
            updates
                .windows(2)
                .all(|pair| pair[0].revision() < pair[1].revision())
        );
        assert_eq!(updates.last().unwrap().revision(), snapshot.revision);
        assert_eq!(snapshot.revision, terminal.revision());
        assert!(snapshot_text(&snapshot).contains("ABC"));
        assert_eq!(
            materializations.materializations.load(Ordering::Relaxed),
            1,
            "only the retained latest compact snapshot may materialize"
        );
        assert_eq!(metrics.snapshot().subscriber_queue_events_current, 0);
        assert_eq!(metrics.snapshot().queued_snapshot_events_current, 0);
        assert_eq!(metrics.snapshot().queued_snapshot_events_high_water, 1);
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one concurrency regression covers paused construction, receiver progress, and resumed delivery"
    )]
    #[tokio::test]
    async fn compact_snapshot_build_does_not_block_receiver_or_overflow_fast_tail() {
        let incarnation = ProcessIncarnation::allocate();
        let splint_id = SplintId::new();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        let metrics = Arc::new(RuntimeMetrics::default());
        let (subscriber, mut subscription, _resnapshot) =
            test_subscriber(&terminal, 8, true, Arc::clone(&metrics));
        let (sender, accounting, snapshot_slot, semantic_capacity) = match &subscriber.events {
            SubscriberEvents::Compact {
                sender,
                accounting,
                snapshot_slot,
                semantic_capacity,
            } => (
                sender.clone(),
                Arc::clone(accounting),
                Arc::clone(snapshot_slot),
                *semantic_capacity,
            ),
            SubscriberEvents::Legacy(_) => unreachable!(),
        };

        let base_revision = terminal.revision();
        terminal.advance(b"A");
        let first_revision = terminal.revision();
        let first_updates = terminal
            .updates_since(base_revision)
            .unwrap()
            .into_updates();
        assert_eq!(
            publish_compact_update(
                &sender,
                &accounting,
                &snapshot_slot,
                semantic_capacity,
                incarnation,
                first_updates,
                first_revision,
                CompactHistoryPolicy::FullHistory,
                &metrics,
                |_| compact_snapshot(splint_id, incarnation, &terminal, 0, None),
            ),
            CompactPublishOutcome::Published
        );

        terminal.advance(b"B");
        let second_revision = terminal.revision();
        let second_updates = terminal
            .updates_since(first_revision)
            .unwrap()
            .into_updates();
        let (build_started_tx, build_started_rx) = std::sync::mpsc::sync_channel(0);
        let (resume_build_tx, resume_build_rx) = std::sync::mpsc::sync_channel(0);
        let writer_sender = sender.clone();
        let writer_accounting = Arc::clone(&accounting);
        let writer_slot = Arc::clone(&snapshot_slot);
        let writer_metrics = Arc::clone(&metrics);
        let writer = std::thread::spawn(move || {
            publish_compact_update(
                &writer_sender,
                &writer_accounting,
                &writer_slot,
                semantic_capacity,
                incarnation,
                second_updates,
                second_revision,
                CompactHistoryPolicy::FullHistory,
                &writer_metrics,
                |_| {
                    build_started_tx.send(()).unwrap();
                    resume_build_rx.recv().unwrap();
                    compact_snapshot(splint_id, incarnation, &terminal, 0, None)
                },
            )
        });

        build_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("second snapshot build did not start");
        assert!(matches!(
            subscription.recv_queued().await,
            Some(CompactQueuedEvent::UpdateReady)
        ));
        let MailboxTake::Exact {
            end_revision,
            snapshot,
            ..
        } = snapshot_slot.take_pending()
        else {
            panic!("receiver could not drain while snapshot construction was paused");
        };
        assert_eq!(end_revision, first_revision);
        assert_eq!(snapshot.metadata.revision, first_revision);
        drop(snapshot);

        resume_build_tx.send(()).unwrap();
        assert_eq!(
            writer.join().expect("snapshot writer panicked"),
            CompactPublishOutcome::Published
        );
        let (received, trailing_exit) =
            time::timeout(Duration::from_secs(1), subscription.recv_coalesced())
                .await
                .expect("fresh wake token was not delivered");
        assert_eq!(trailing_exit, None);
        let SubscriptionReceive::Event(LiveEvent::Update {
            updates, snapshot, ..
        }) = received
        else {
            panic!("fast receiver did not receive the post-build publication");
        };
        assert_eq!(updates.last().unwrap().revision(), second_revision);
        assert_eq!(snapshot.revision, second_revision);
        assert_eq!(metrics.snapshot().queued_snapshot_events_high_water, 1);
        assert_eq!(metrics.snapshot().subscriber_queue_events_current, 0);
        drop(subscriber);
    }

    #[tokio::test]
    async fn cooperative_read_boundary_prevents_false_fast_subscriber_overflow() {
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(80, 24, TerminalConfig::default());
        let metrics = Arc::new(RuntimeMetrics::default());
        let (subscriber, mut subscription, resnapshot) =
            test_subscriber(&terminal, 65, true, Arc::clone(&metrics));
        let mut subscribers = vec![subscriber];
        let mut publication = SynchronizedPublication::new(terminal.revision());
        let consumer = tokio::spawn(async move {
            let mut revisions = Vec::new();
            while revisions.len() < 2 {
                match subscription.recv_coalesced().await.0 {
                    SubscriptionReceive::Event(LiveEvent::Update { snapshot, .. }) => {
                        revisions.push(snapshot.revision);
                    }
                    SubscriptionReceive::Event(LiveEvent::Exited { .. })
                    | SubscriptionReceive::ResnapshotRequired
                    | SubscriptionReceive::Closed => break,
                }
            }
            revisions
        });

        for read_index in 0..2 {
            let producer_batch = CompactProducerBatch::begin(&subscribers);
            for batch_index in 0..(READ_BUFFER / PARSE_BATCH) {
                let index = read_index * (READ_BUFFER / PARSE_BATCH) + batch_index;
                terminal.advance(format!("fast-{index:03}\r\n").as_bytes());
                let (updates, overflows) = publish_updates(
                    SplintId::new(),
                    &terminal,
                    &mut publication,
                    incarnation,
                    None,
                    &mut subscribers,
                    &metrics,
                );
                assert!(updates > 0);
                assert_eq!(overflows, 0);
            }
            tokio::task::yield_now().await;
            assert_eq!(
                metrics.snapshot().subscriber_queue_events_current,
                READ_BUFFER / PARSE_BATCH,
                "consumer must not materialize an incomplete synchronous PTY read"
            );
            drop(producer_batch);
            tokio::task::yield_now().await;
            assert_eq!(metrics.snapshot().subscriber_queue_events_current, 0);
        }

        let revisions = time::timeout(Duration::from_secs(1), consumer)
            .await
            .expect("fast compact consumer was not scheduled at read boundaries")
            .expect("fast compact consumer panicked");
        assert_eq!(revisions.len(), 2);
        assert_eq!(revisions.last().copied(), Some(terminal.revision()));
        assert!(!*resnapshot.borrow());
        assert_eq!(metrics.snapshot().output_subscriber_overflows, 0);
        assert_eq!(metrics.snapshot().queued_snapshot_events_high_water, 1);
        assert_eq!(
            subscribers[0]
                .events
                .snapshot_slot()
                .producer_batch_waits
                .load(Ordering::Relaxed),
            2,
            "each synchronous PTY read must park the receiver exactly once",
        );
        assert_eq!(
            subscribers[0]
                .events
                .snapshot_slot()
                .producer_batch_wakes
                .load(Ordering::Relaxed),
            2,
            "each parked receiver must wake exactly once",
        );
    }

    #[tokio::test]
    async fn delayed_compact_subscriber_retains_one_latest_snapshot() {
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(80, 24, TerminalConfig::default());
        let metrics = Arc::new(RuntimeMetrics::default());
        // Raw capacity 65 represents the configured 64 update entries plus the
        // reserved process-exit slot.
        let (subscriber, mut subscription, resnapshot) =
            test_subscriber(&terminal, 65, true, Arc::clone(&metrics));
        let mut subscribers = vec![subscriber];
        let mut publication = SynchronizedPublication::new(terminal.revision());

        for index in 0..64 {
            terminal.advance(format!("slice2-{index:02}\r\n").as_bytes());
            let (updates, overflows) = publish_updates(
                SplintId::new(),
                &terminal,
                &mut publication,
                incarnation,
                None,
                &mut subscribers,
                &metrics,
            );
            assert!(updates > 0);
            assert_eq!(overflows, 0);
        }

        let retained = metrics.snapshot();
        assert_eq!(retained.subscriber_queue_events_current, 64);
        assert_eq!(retained.queued_snapshot_events_current, 1);
        assert_eq!(retained.queued_snapshot_events_high_water, 1);
        assert!(!*resnapshot.borrow());

        let (received, trailing_exit) = subscription.recv_coalesced().await;
        assert_eq!(trailing_exit, None);
        let SubscriptionReceive::Event(LiveEvent::Update {
            updates, snapshot, ..
        }) = received
        else {
            panic!("delayed compact subscriber did not receive exact latest state");
        };
        assert_eq!(updates.last().unwrap().revision(), snapshot.revision);
        assert_eq!(snapshot.revision, terminal.revision());
        assert!(snapshot_text(&snapshot).contains("slice2-63"));
        assert_eq!(metrics.snapshot().subscriber_queue_events_current, 0);
        assert_eq!(metrics.snapshot().queued_snapshot_events_current, 0);
    }

    #[tokio::test]
    async fn compact_slot_pairs_thousand_row_history_clear_and_reflow_revision() {
        let incarnation = ProcessIncarnation::allocate();
        let config = TerminalConfig {
            scrollback_lines: 1_000,
            ..TerminalConfig::default()
        };
        let mut terminal = Terminal::new(8, 2, config);
        let metrics = Arc::new(RuntimeMetrics::default());
        let (subscriber, mut subscription, resnapshot) =
            test_subscriber_with_rows(&terminal, 1_100, true, Arc::clone(&metrics), 1_000);
        let base_revision = terminal.revision();
        let mut subscribers = vec![subscriber];
        let mut publication = SynchronizedPublication::new(base_revision);

        for index in 0..1_050 {
            terminal.advance(format!("row-{index:04}\r\n").as_bytes());
            assert_eq!(
                publish_updates(
                    SplintId::new(),
                    &terminal,
                    &mut publication,
                    incarnation,
                    None,
                    &mut subscribers,
                    &metrics,
                )
                .1,
                0
            );
        }
        terminal.advance(b"\x1b[2J\x1b[Hafter-clear");
        assert_eq!(
            publish_updates(
                SplintId::new(),
                &terminal,
                &mut publication,
                incarnation,
                None,
                &mut subscribers,
                &metrics,
            )
            .1,
            0
        );
        terminal.resize(10, 3);
        assert_eq!(
            publish_updates(
                SplintId::new(),
                &terminal,
                &mut publication,
                incarnation,
                None,
                &mut subscribers,
                &metrics,
            )
            .1,
            0
        );

        let (received, trailing_exit) = subscription.recv_coalesced().await;
        assert_eq!(trailing_exit, None);
        let SubscriptionReceive::Event(LiveEvent::Update {
            updates, snapshot, ..
        }) = received
        else {
            panic!("history/reflow compact subscription did not produce state");
        };
        assert!(!*resnapshot.borrow());
        assert!(updates.first().unwrap().revision() > base_revision);
        assert_eq!(updates.last().unwrap().revision(), snapshot.revision);
        assert_eq!(snapshot.revision, terminal.revision());
        assert_eq!(snapshot.dimensions.columns, 10);
        assert_eq!(snapshot.dimensions.rows, 3);
        assert!(snapshot.scrollback_rows.len() <= 1_000);
        assert!(snapshot_text(&snapshot).contains("after-clear"));
        assert_eq!(metrics.snapshot().queued_snapshot_events_high_water, 1);
    }

    #[test]
    fn snapshot_slot_replacement_and_receiver_drop_release_exact_ownership() {
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        let metrics = Arc::new(RuntimeMetrics::default());
        let (subscriber, receiver, _) = test_subscriber(&terminal, 8, true, Arc::clone(&metrics));
        let mut subscribers = vec![subscriber];
        let mut publication = SynchronizedPublication::new(terminal.revision());

        for byte in *b"ABC" {
            terminal.advance(&[byte]);
            assert_eq!(
                publish_updates(
                    SplintId::new(),
                    &terminal,
                    &mut publication,
                    incarnation,
                    None,
                    &mut subscribers,
                    &metrics,
                ),
                (1, 0)
            );
            let retained = metrics.snapshot();
            assert_eq!(retained.queued_snapshot_events_current, 1);
            assert_eq!(retained.queued_snapshot_events_high_water, 1);
            assert_eq!(retained.queued_snapshot_cells_current, 16);
        }

        drop(receiver);
        let released = metrics.snapshot();
        assert_eq!(released.queued_snapshot_events_current, 0);
        assert_eq!(released.queued_snapshot_rows_current, 0);
        assert_eq!(released.queued_snapshot_cells_current, 0);
        assert_eq!(released.subscriber_queue_events_current, 0);
    }

    #[tokio::test]
    async fn coalesced_receive_returns_retained_final_update_before_disconnect() {
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        let metrics = Arc::new(RuntimeMetrics::default());
        let (subscriber, mut subscription, _) =
            test_subscriber(&terminal, 4, true, Arc::clone(&metrics));
        let mut subscribers = vec![subscriber];
        let mut publication = SynchronizedPublication::new(terminal.revision());
        terminal.advance(b"final");
        publish_updates(
            SplintId::new(),
            &terminal,
            &mut publication,
            incarnation,
            None,
            &mut subscribers,
            &metrics,
        );
        drop(subscribers);

        assert!(matches!(
            subscription.recv_coalesced().await,
            (SubscriptionReceive::Event(LiveEvent::Update { .. }), None)
        ));
        assert!(matches!(
            subscription.recv_coalesced().await,
            (SubscriptionReceive::Closed, None)
        ));
        assert_eq!(metrics.snapshot().subscriber_queue_events_current, 0);
    }

    #[allow(
        clippy::similar_names,
        reason = "receiver names distinguish the channel from its delivered event"
    )]
    #[tokio::test]
    async fn coalesced_receive_preserves_trailing_exit_and_resnapshot_precedence() {
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        let metrics = Arc::new(RuntimeMetrics::default());
        let (subscriber, receiver, _resnapshot) =
            test_subscriber(&terminal, 4, true, Arc::clone(&metrics));
        let materializations = Arc::clone(&receiver.accounting);
        let mut subscription = receiver;
        let mut subscribers = vec![subscriber];
        let mut publication = SynchronizedPublication::new(terminal.revision());
        terminal.advance(b"final");
        publish_updates(
            SplintId::new(),
            &terminal,
            &mut publication,
            incarnation,
            None,
            &mut subscribers,
            &metrics,
        );
        let status = ProcessExit {
            code: Some(7),
            signal: None,
        };
        assert_eq!(publish_exit(&mut subscribers, incarnation, status), 0);
        let (received, trailing_exit) = subscription.recv_coalesced().await;
        assert!(matches!(
            received,
            SubscriptionReceive::Event(LiveEvent::Update { .. })
        ));
        assert_eq!(trailing_exit, Some(status));
        assert_eq!(materializations.materializations.load(Ordering::Relaxed), 1);

        let metrics = Arc::new(RuntimeMetrics::default());
        let (subscriber, receiver, mut resnapshot) =
            test_subscriber(&terminal, 4, true, Arc::clone(&metrics));
        let materializations = Arc::clone(&receiver.accounting);
        let mut subscription = receiver;
        let mut subscribers = vec![subscriber];
        let mut publication = SynchronizedPublication::new(terminal.revision());
        terminal.advance(b"discarded");
        publish_updates(
            SplintId::new(),
            &terminal,
            &mut publication,
            incarnation,
            None,
            &mut subscribers,
            &metrics,
        );
        subscribers[0].resnapshot.send_replace(true);
        resnapshot.changed().await.unwrap();
        let (received, trailing_exit) = subscription.recv_coalesced().await;
        assert!(matches!(received, SubscriptionReceive::ResnapshotRequired));
        assert_eq!(trailing_exit, None);
        assert_eq!(materializations.materializations.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn failed_admission_does_not_raise_successful_queue_high_water() {
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        let metrics = Arc::new(RuntimeMetrics::default());
        let (subscriber, receiver, _) = test_subscriber(&terminal, 2, true, Arc::clone(&metrics));
        drop(receiver);
        let mut subscribers = vec![subscriber];
        let mut publication = SynchronizedPublication::new(terminal.revision());
        terminal.advance(b"closed");
        let (_, overflows) = publish_updates(
            SplintId::new(),
            &terminal,
            &mut publication,
            incarnation,
            None,
            &mut subscribers,
            &metrics,
        );
        assert_eq!(overflows, 0);
        let observed = metrics.snapshot();
        assert_eq!(observed.publication_snapshot_builds, 0);
        assert_eq!(observed.publication_snapshot_enqueues, 0);
        assert_eq!(observed.subscriber_queue_events_high_water, 0);
        assert_eq!(observed.queued_snapshot_events_high_water, 0);

        let metrics = Arc::new(RuntimeMetrics::default());
        let (subscriber, _receiver, resnapshot) =
            test_subscriber(&terminal, 2, true, Arc::clone(&metrics));
        let mut subscribers = vec![subscriber];
        let mut publication = SynchronizedPublication::new(terminal.revision());
        terminal.advance(b"A");
        assert_eq!(
            publish_updates(
                SplintId::new(),
                &terminal,
                &mut publication,
                incarnation,
                None,
                &mut subscribers,
                &metrics,
            ),
            (1, 0)
        );
        terminal.advance(b"B");
        assert_eq!(
            publish_updates(
                SplintId::new(),
                &terminal,
                &mut publication,
                incarnation,
                None,
                &mut subscribers,
                &metrics,
            ),
            (1, 1)
        );
        let observed = metrics.snapshot();
        assert_eq!(observed.publication_snapshot_builds, 1);
        assert_eq!(observed.publication_snapshot_enqueues, 1);
        assert_eq!(observed.subscriber_queue_events_high_water, 1);
        assert_eq!(observed.queued_snapshot_events_high_water, 1);
        assert!(*resnapshot.borrow());
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
        let config = TerminalConfig {
            update_history_limit: 4,
            ..TerminalConfig::default()
        };
        let mut terminal = Terminal::new(8, 2, config);
        let initial_revision = terminal.revision();
        let (subscriber, mut receiver, resnapshot) = synchronized_test_subscriber(&terminal);
        let mut subscribers = vec![subscriber];
        let mut publication = SynchronizedPublication::new(terminal.revision());
        let mut replies = WriteQueue::default();
        let attribution = Arc::new(RuntimeMetrics::default());
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
            &attribution,
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
            &attribution,
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
        let config = TerminalConfig {
            update_history_limit: 4,
            ..TerminalConfig::default()
        };
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
            &Arc::new(RuntimeMetrics::default()),
        );
        assert_eq!(overflows, 1);
        assert!(subscribers.is_empty());
        assert!(*resnapshot.borrow());
    }

    #[test]
    fn public_subscription_events_remains_the_full_tokio_receiver() {
        fn accepts_original_receiver_type(_: &mut mpsc::Receiver<LiveEvent>) {}

        let (_sender, events) = mpsc::channel(2);
        let (_resnapshot_sender, resnapshot) = watch::channel(false);
        let mut subscription = Subscription { events, resnapshot };
        accepts_original_receiver_type(&mut subscription.events);
        assert_eq!(subscription.events.max_capacity(), 2);
        subscription.events.close();
        assert!(subscription.events.is_closed());
    }

    #[test]
    fn public_live_cell_api_remains_source_compatible() {
        let cell = LiveCell {
            content: "A".to_owned(),
            spacer_remaining: None,
            attributes: splinterm_terminal::Attributes::default().into(),
        };
        assert_eq!(cell.content, "A");
        assert_eq!(cell.spacer_remaining, None);
    }

    #[test]
    fn compact_history_policy_materializes_only_proven_required_rows() {
        let splint_id = SplintId::new();
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        let base = terminal.revision();
        terminal.advance(b"one\r\ntwo\r\nthree");
        let updates = terminal.updates_since(base).unwrap().into_updates();
        let metadata = terminal.snapshot(SnapshotRequest {
            max_scrollback_rows: 0,
        });
        let policy =
            compact_history_policy(&updates, metadata.dimensions(), metadata.active_screen());
        let CompactHistoryPolicy::AppendTail(appended) = policy else {
            panic!("full-height normal scroll must be a proven append");
        };
        assert!(appended > 0);
        let partial =
            compact_snapshot_with_history(splint_id, incarnation, &terminal, 1_000, None, policy);
        let full = compact_snapshot_with_history(
            splint_id,
            incarnation,
            &terminal,
            1_000,
            None,
            CompactHistoryPolicy::FullHistory,
        );
        assert_eq!(partial.scrollback_rows.len(), appended.min(1_000));
        assert_eq!(
            partial
                .scrollback_rows
                .iter()
                .map(|row| row.row_id)
                .collect::<Vec<_>>(),
            full.scrollback_rows[full.scrollback_rows.len() - partial.scrollback_rows.len()..]
                .iter()
                .map(|row| row.row_id)
                .collect::<Vec<_>>()
        );
        assert_eq!(partial.metadata.scrollback, full.metadata.scrollback);

        let base = terminal.revision();
        terminal.advance(b"X");
        let updates = terminal.updates_since(base).unwrap().into_updates();
        let metadata = terminal.snapshot(SnapshotRequest {
            max_scrollback_rows: 0,
        });
        assert_eq!(
            compact_history_policy(&updates, metadata.dimensions(), metadata.active_screen(),),
            CompactHistoryPolicy::NoHistory
        );
        let metadata_only = compact_snapshot_with_history(
            splint_id,
            incarnation,
            &terminal,
            1_000,
            None,
            CompactHistoryPolicy::NoHistory,
        );
        assert!(metadata_only.scrollback_rows.is_empty());
        assert_eq!(
            metadata_only.metadata.scrollback.available_rows,
            full.metadata.scrollback.available_rows
        );
        assert_eq!(metadata_only.metadata.scrollback.returned_rows, 0);
        assert_eq!(
            metadata_only.metadata.scrollback.omitted_oldest_rows,
            metadata_only.metadata.scrollback.available_rows
        );
        assert_eq!(
            metadata_only.metadata.scrollback.oldest_available_row_id,
            full.metadata.scrollback.oldest_available_row_id
        );
        assert_eq!(
            metadata_only.metadata.scrollback.newest_available_row_id,
            full.metadata.scrollback.newest_available_row_id
        );
    }

    #[test]
    fn compact_history_policy_falls_back_for_ambiguous_history_changes() {
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        terminal.advance(b"one\r\ntwo\r\nthree");

        let base = terminal.revision();
        terminal.advance(b"\x1b[3J");
        let updates = terminal.updates_since(base).unwrap().into_updates();
        let metadata = terminal.snapshot(SnapshotRequest {
            max_scrollback_rows: 0,
        });
        assert_eq!(
            compact_history_policy(&updates, metadata.dimensions(), metadata.active_screen(),),
            CompactHistoryPolicy::FullHistory
        );

        let base = terminal.revision();
        terminal.resize(10, 3);
        let updates = terminal.updates_since(base).unwrap().into_updates();
        let metadata = terminal.snapshot(SnapshotRequest {
            max_scrollback_rows: 0,
        });
        assert_eq!(
            compact_history_policy(&updates, metadata.dimensions(), metadata.active_screen(),),
            CompactHistoryPolicy::FullHistory
        );

        let base = terminal.revision();
        terminal.advance(b"\x1b[?1049h");
        let updates = terminal.updates_since(base).unwrap().into_updates();
        let metadata = terminal.snapshot(SnapshotRequest {
            max_scrollback_rows: 0,
        });
        assert_eq!(
            compact_history_policy(&updates, metadata.dimensions(), metadata.active_screen(),),
            CompactHistoryPolicy::FullHistory
        );
        assert_eq!(
            CompactHistoryPolicy::AppendTail(2).merge(CompactHistoryPolicy::AppendTail(3)),
            CompactHistoryPolicy::AppendTail(5)
        );
        assert_eq!(
            CompactHistoryPolicy::NoHistory.merge(CompactHistoryPolicy::FullHistory),
            CompactHistoryPolicy::FullHistory
        );
    }

    #[test]
    fn partial_compact_snapshot_policy_mismatch_requires_resnapshot() {
        let terminal = Terminal::new(8, 2, TerminalConfig::default());
        let incarnation = ProcessIncarnation::allocate();
        let snapshot = compact_snapshot_with_history(
            SplintId::new(),
            incarnation,
            &terminal,
            0,
            None,
            CompactHistoryPolicy::NoHistory,
        );
        let slot = CompactSnapshotSlot::default();
        {
            let mut current = slot.lock();
            current.pending = Some(PendingCompactUpdates {
                incarnation,
                batches: vec![Vec::new()],
                end_revision: terminal.revision(),
                history_policy: CompactHistoryPolicy::AppendTail(1),
                admitted: Vec::new(),
                pending_attribution: None,
            });
            current.snapshot = Some(SnapshotEnvelope {
                revision: terminal.revision(),
                snapshot: Box::new(snapshot),
                _admitted: None,
            });
        }
        assert!(matches!(
            slot.take_pending(),
            MailboxTake::MissingOrMismatched
        ));
    }

    #[test]
    fn compact_live_cells_allocate_owned_strings_only_for_composed_content() {
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        terminal.advance("Ae\u{301}界".as_bytes());

        let snapshot = compact_snapshot(
            SplintId::new(),
            ProcessIncarnation::allocate(),
            &terminal,
            0,
            None,
        );
        let cells = &snapshot.visible_rows[0].cells;
        assert_eq!(cells[0].content, CompactCellContent::Scalar('A'));
        assert!(matches!(
            &cells[1].content,
            CompactCellContent::Composed(characters) if characters == "e\u{301}"
        ));
        assert_eq!(cells[2].content, CompactCellContent::Scalar('界'));
        assert_eq!(
            cells[3].content,
            CompactCellContent::Spacer { remaining: 1 }
        );
        assert_eq!(cells[4].content, CompactCellContent::Empty);
        assert_eq!(cells[0].content.owned_string_bytes(), 0);
        assert_eq!(cells[2].content.owned_string_bytes(), 0);
        assert_eq!(cells[3].content.owned_string_bytes(), 0);
        assert_eq!(cells[4].content.owned_string_bytes(), 0);
        assert!(cells[1].content.owned_string_bytes() >= "e\u{301}".len());

        let attribution = compact_snapshot_attribution(&snapshot);
        assert_eq!(attribution.rows, 2);
        assert_eq!(attribution.cells, 16);
        assert_eq!(attribution.scalar_cells, 2);
        assert_eq!(attribution.composed_cells, 1);
        assert_eq!(attribution.spacer_cells, 1);
        assert_eq!(attribution.empty_cells, 12);

        let live = snapshot.into_live();
        assert_eq!(live.visible_rows[0].cells[0].content, "A");
        assert_eq!(live.visible_rows[0].cells[1].content, "e\u{301}");
        assert_eq!(live.visible_rows[0].cells[2].content, "界");
        assert_eq!(live.visible_rows[0].cells[3].spacer_remaining, Some(1));
        assert_eq!(live.visible_rows[0].cells[4].content, "");
    }

    #[test]
    fn disabled_publication_attribution_stays_zero() {
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        let metrics = Arc::new(RuntimeMetrics::default());
        let (subscriber, mut receiver, _) =
            test_subscriber(&terminal, 4, false, Arc::clone(&metrics));
        let mut subscribers = vec![subscriber];
        let mut publication = SynchronizedPublication::new(terminal.revision());
        terminal.advance(b"private-body");

        publish_updates(
            SplintId::new(),
            &terminal,
            &mut publication,
            incarnation,
            None,
            &mut subscribers,
            &metrics,
        );
        assert!(matches!(receiver.try_recv(), Ok(LiveEvent::Update { .. })));
        let observed = metrics.snapshot();
        assert_eq!(observed.publication_snapshot_builds, 0);
        assert_eq!(observed.publication_snapshot_enqueues, 0);
        assert_eq!(observed.subscriber_queue_events_high_water, 0);
        assert_eq!(observed.queued_snapshot_cells_high_water, 0);
    }

    #[test]
    fn publication_attribution_distinguishes_build_enqueue_and_dequeue() {
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        let metrics = Arc::new(RuntimeMetrics::default());
        let (subscriber, mut receiver, _) =
            test_subscriber(&terminal, 4, true, Arc::clone(&metrics));
        let mut subscribers = vec![subscriber];
        let mut publication = SynchronizedPublication::new(terminal.revision());
        terminal.advance(b"private-body");

        let (updates, overflows) = publish_updates(
            SplintId::new(),
            &terminal,
            &mut publication,
            incarnation,
            None,
            &mut subscribers,
            &metrics,
        );
        assert!(updates > 0);
        assert_eq!(overflows, 0);
        let queued = metrics.snapshot();
        assert_eq!(queued.publication_snapshot_builds, 1);
        assert_eq!(queued.publication_snapshot_enqueues, 1);
        assert_eq!(queued.publication_snapshot_rows, 2);
        assert_eq!(queued.publication_snapshot_cells, 16);
        assert_eq!(queued.publication_snapshot_enqueued_rows, 2);
        assert_eq!(queued.publication_snapshot_enqueued_cells, 16);
        assert_eq!(queued.subscriber_queue_events_current, 1);
        assert_eq!(queued.subscriber_queue_events_high_water, 1);
        assert_eq!(queued.subscriber_queue_per_subscriber_high_water, 1);
        assert_eq!(queued.queued_snapshot_events_current, 1);
        assert_eq!(queued.queued_snapshot_cells_current, 16);
        assert_eq!(queued.queued_snapshot_cells_high_water, 16);
        assert_eq!(queued.publication_compact_batches, 1);
        assert_eq!(queued.publication_compact_batch_merges, 0);
        assert_eq!(queued.queued_compact_batches_current, 1);
        assert_eq!(queued.queued_compact_batches_high_water, 1);
        assert!(queued.queued_compact_terminal_updates_current > 0);
        assert_eq!(
            queued.queued_compact_terminal_updates_current,
            queued.queued_compact_terminal_updates_high_water
        );
        let serialized = serde_json::to_string(&queued).unwrap();
        assert!(!serialized.contains("private-body"));

        assert!(matches!(receiver.try_recv(), Ok(LiveEvent::Update { .. })));
        let drained = metrics.snapshot();
        assert_eq!(drained.subscriber_queue_events_current, 0);
        assert_eq!(drained.queued_snapshot_events_current, 0);
        assert_eq!(drained.queued_snapshot_rows_current, 0);
        assert_eq!(drained.queued_snapshot_cells_current, 0);
        assert_eq!(drained.publication_snapshot_enqueues, 1);
        assert_eq!(drained.queued_compact_batches_current, 0);
        assert_eq!(drained.queued_compact_terminal_updates_current, 0);
        assert_eq!(drained.queued_compact_scrolls_current, 0);
        assert_eq!(drained.queued_compact_appended_rows_current, 0);
        assert_eq!(drained.publication_compact_materializations, 1);
        assert_eq!(drained.publication_compact_materialized_batches, 1);
        assert_eq!(
            drained.publication_compact_materialized_terminal_updates,
            queued.queued_compact_terminal_updates_high_water
        );

        metrics
            .publication_snapshot_cells
            .store(u64::MAX - 1, Ordering::Relaxed);
        RuntimeMetrics::add_saturating(&metrics.publication_snapshot_cells, 8);
        assert_eq!(
            metrics.publication_snapshot_cells.load(Ordering::Relaxed),
            u64::MAX
        );
    }

    #[test]
    fn compact_batch_attribution_tracks_merge_materialize_and_release() {
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        let metrics = Arc::new(RuntimeMetrics::default());
        let (subscriber, mut receiver, _) =
            test_subscriber(&terminal, 4, true, Arc::clone(&metrics));
        let mut subscribers = vec![subscriber];
        let mut publication = SynchronizedPublication::new(terminal.revision());

        terminal.advance(b"A");
        assert_eq!(
            publish_updates(
                SplintId::new(),
                &terminal,
                &mut publication,
                incarnation,
                None,
                &mut subscribers,
                &metrics,
            ),
            (1, 0)
        );
        terminal.advance(b"B");
        assert_eq!(
            publish_updates(
                SplintId::new(),
                &terminal,
                &mut publication,
                incarnation,
                None,
                &mut subscribers,
                &metrics,
            ),
            (1, 0)
        );

        let queued = metrics.snapshot();
        assert_eq!(queued.publication_compact_batches, 2);
        assert_eq!(queued.publication_compact_batch_merges, 1);
        assert_eq!(queued.queued_compact_batches_current, 2);
        assert_eq!(queued.queued_compact_batches_high_water, 2);
        assert!(queued.queued_compact_terminal_updates_current >= 2);

        assert!(matches!(receiver.try_recv(), Ok(LiveEvent::Update { .. })));
        let drained = metrics.snapshot();
        assert_eq!(drained.queued_compact_batches_current, 0);
        assert_eq!(drained.queued_compact_terminal_updates_current, 0);
        assert_eq!(drained.publication_compact_materializations, 1);
        assert_eq!(drained.publication_compact_materialized_batches, 2);
        assert_eq!(
            drained.publication_compact_materialized_batches_high_water,
            2
        );
        assert_eq!(
            drained.publication_compact_materialized_terminal_updates,
            queued.queued_compact_terminal_updates_high_water
        );
    }

    #[test]
    fn saturated_and_multiple_subscriber_metrics_are_exact() {
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        let metrics = Arc::new(RuntimeMetrics::default());
        let (first, mut first_receiver, _) =
            test_subscriber(&terminal, 2, true, Arc::clone(&metrics));
        let (second, second_receiver, _) =
            test_subscriber(&terminal, 2, true, Arc::clone(&metrics));
        let mut subscribers = vec![first, second];
        let mut publication = SynchronizedPublication::new(terminal.revision());
        terminal.advance(b"A");

        assert_eq!(
            publish_updates(
                SplintId::new(),
                &terminal,
                &mut publication,
                incarnation,
                None,
                &mut subscribers,
                &metrics,
            ),
            (1, 0)
        );
        let queued = metrics.snapshot();
        assert_eq!(queued.publication_snapshot_builds, 2);
        assert_eq!(queued.publication_snapshot_enqueues, 2);
        assert_eq!(queued.subscriber_queue_events_current, 2);
        assert_eq!(queued.subscriber_queue_events_high_water, 2);
        assert_eq!(queued.subscriber_queue_per_subscriber_high_water, 1);
        assert_eq!(queued.queued_snapshot_events_current, 2);
        assert_eq!(queued.publication_compact_batches, 2);
        assert_eq!(queued.queued_compact_batches_current, 2);
        assert_eq!(queued.queued_compact_batches_high_water, 2);

        assert!(first_receiver.try_recv().is_ok());
        assert_eq!(metrics.snapshot().subscriber_queue_events_current, 1);
        assert_eq!(metrics.snapshot().queued_compact_batches_current, 1);
        drop(second_receiver);
        assert_eq!(metrics.snapshot().subscriber_queue_events_current, 0);
        assert_eq!(metrics.snapshot().queued_snapshot_events_current, 0);
        assert_eq!(metrics.snapshot().queued_compact_batches_current, 0);

        let metrics = Arc::new(RuntimeMetrics::default());
        let (subscriber, receiver, resnapshot) =
            test_subscriber(&terminal, 2, true, Arc::clone(&metrics));
        let mut subscribers = vec![subscriber];
        let mut publication = SynchronizedPublication::new(terminal.revision());
        terminal.advance(b"B");
        assert_eq!(
            publish_updates(
                SplintId::new(),
                &terminal,
                &mut publication,
                incarnation,
                None,
                &mut subscribers,
                &metrics,
            ),
            (1, 0)
        );
        terminal.advance(b"C");
        assert_eq!(
            publish_updates(
                SplintId::new(),
                &terminal,
                &mut publication,
                incarnation,
                None,
                &mut subscribers,
                &metrics,
            ),
            (1, 1)
        );
        assert!(subscribers.is_empty());
        assert!(*resnapshot.borrow());
        assert_eq!(metrics.snapshot().publication_snapshot_builds, 1);
        assert_eq!(metrics.snapshot().publication_snapshot_enqueues, 1);
        assert_eq!(metrics.snapshot().queued_snapshot_events_current, 0);
        assert_eq!(metrics.snapshot().queued_compact_batches_current, 0);
        drop(receiver);
        assert_eq!(metrics.snapshot().subscriber_queue_events_current, 0);
    }

    #[tokio::test]
    async fn concurrent_drain_and_receiver_drop_restore_queue_current_metrics() {
        let metrics = Arc::new(RuntimeMetrics::default());
        let accounting = Arc::new(QueueAccounting::new(true, Arc::clone(&metrics)));
        let (sender, events) = mpsc::channel(64);
        let (_, resnapshot) = watch::channel(false);
        let mut receiver = CompactSubscription {
            events,
            resnapshot,
            accounting: Arc::clone(&accounting),
            snapshot_slot: Arc::new(CompactSnapshotSlot::default()),
        };
        let consumer = tokio::spawn(async move {
            let mut count = 0;
            while receiver.recv_queued().await.is_some() {
                count += 1;
                tokio::task::yield_now().await;
            }
            count
        });
        for _ in 0..100 {
            let permit = sender.reserve().await.unwrap();
            let admitted = QueueLease::new(&accounting);
            permit.send(CompactQueuedEvent::Exited {
                incarnation: ProcessIncarnation::allocate(),
                status: ProcessExit {
                    code: Some(0),
                    signal: None,
                },
                admitted,
            });
            tokio::task::yield_now().await;
        }
        drop(sender);
        assert_eq!(consumer.await.unwrap(), 100);
        let observed = metrics.snapshot();
        assert_eq!(observed.subscriber_queue_events_current, 0);
        assert!(observed.subscriber_queue_events_high_water >= 1);
        assert!(observed.subscriber_queue_per_subscriber_high_water >= 1);
    }

    #[test]
    fn receiver_drop_between_admission_and_send_releases_ownership() {
        let metrics = Arc::new(RuntimeMetrics::default());
        let accounting = Arc::new(QueueAccounting::new(true, Arc::clone(&metrics)));
        let (sender, receiver) = mpsc::channel(1);
        let permit = sender.try_reserve().unwrap();
        send_permit_admitted_compact(
            &sender,
            permit,
            &accounting,
            |admitted| CompactQueuedEvent::Exited {
                incarnation: ProcessIncarnation::allocate(),
                status: ProcessExit {
                    code: Some(0),
                    signal: None,
                },
                admitted,
            },
            || {
                assert_eq!(metrics.snapshot().subscriber_queue_events_current, 1);
                drop(receiver);
            },
        );
        assert_eq!(metrics.snapshot().subscriber_queue_events_current, 0);
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
        let metrics = Arc::new(RuntimeMetrics::default());
        let accounting = Arc::new(QueueAccounting::new(false, metrics));
        let (event_tx, events) = mpsc::channel(1);
        event_tx
            .send(CompactQueuedEvent::Exited {
                incarnation: ProcessIncarnation::allocate(),
                status: ProcessExit {
                    code: Some(0),
                    signal: None,
                },
                admitted: None,
            })
            .await
            .unwrap();
        let (resnapshot_tx, resnapshot) = watch::channel(false);
        resnapshot_tx.send(true).unwrap();
        let mut subscription = CompactSubscription {
            events,
            resnapshot,
            accounting,
            snapshot_slot: Arc::new(CompactSnapshotSlot::default()),
        };

        assert!(matches!(
            subscription.recv_coalesced().await.0,
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
            .filter(|cell| cell.spacer_remaining.is_none())
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
