use std::{
    collections::VecDeque,
    ffi::OsString,
    io::{self, Read, Write},
    mem::size_of,
    os::unix::process::ExitStatusExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

#[cfg(test)]
use std::os::fd::AsRawFd;

use splinterm_core::SplintId;
use splinterm_protocol::perf_trace::{PerfTraceEvent, emit_perf_trace, perf_trace_enabled};
use splinterm_pty::{
    AdoptableLinuxPtySession, LinuxPtyBackend, LinuxPtyIdentity, LinuxPtySession, PtyCommand,
    PtyError, PtySession, PtySignal, PtySize,
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
const SUBSCRIBER_SPARSE_SEMANTIC_BYTES: u64 = 16 * 1024 * 1024;
const SPLINT_SPARSE_SEMANTIC_BYTES: u64 = 64 * 1024 * 1024;
const DAEMON_TERMINAL_PUBLICATION_BYTES: u64 = 256 * 1024 * 1024;
static DAEMON_TERMINAL_PUBLICATION_BYTES_CURRENT: AtomicU64 = AtomicU64::new(0);

/// Process-wide terminal-publication admission shared by sparse runtime queues
/// and materialized outbound transactions.
#[derive(Debug)]
pub struct TerminalPublicationMemoryLease {
    bytes: u64,
}

impl TerminalPublicationMemoryLease {
    /// Attempts to reserve bytes beneath the daemon's fixed Beta1 publication
    /// ceiling. Dropping the returned lease releases the reservation.
    #[must_use]
    pub fn try_new(bytes: usize) -> Option<Self> {
        let bytes = u64::try_from(bytes).ok()?;
        QueueAccounting::try_reserve_counter(
            &DAEMON_TERMINAL_PUBLICATION_BYTES_CURRENT,
            bytes,
            DAEMON_TERMINAL_PUBLICATION_BYTES,
        )
        .then(|| Self { bytes })
    }
}

impl Drop for TerminalPublicationMemoryLease {
    fn drop(&mut self) {
        DAEMON_TERMINAL_PUBLICATION_BYTES_CURRENT.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

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

    /// Allocates the next process-wide incarnation.
    ///
    /// # Panics
    ///
    /// Panics if the process has exhausted the `u64` incarnation space.
    #[must_use]
    pub fn allocate() -> Self {
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

#[derive(Debug, Eq, PartialEq)]
enum CompactCellContent {
    Empty,
    Scalar(char),
    Composed(String),
    Spacer { remaining: u32 },
}

impl Clone for CompactCellContent {
    fn clone(&self) -> Self {
        match self {
            Self::Empty => Self::Empty,
            Self::Scalar(character) => Self::Scalar(*character),
            Self::Composed(characters) => Self::Composed(characters.clone()),
            Self::Spacer { remaining } => Self::Spacer {
                remaining: *remaining,
            },
        }
    }

    fn clone_from(&mut self, source: &Self) {
        if let (Self::Composed(current), Self::Composed(next)) = (&mut *self, source) {
            if current.capacity() >= next.len() {
                current.clone_from(next);
            } else {
                let mut replacement = String::with_capacity(next.len());
                replacement.push_str(next);
                *current = replacement;
            }
        } else {
            *self = source.clone();
        }
    }
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

#[derive(Debug, Eq, PartialEq)]
struct CompactLiveCell {
    content: CompactCellContent,
    attributes: CellAttributesSnapshot,
}

impl Clone for CompactLiveCell {
    fn clone(&self) -> Self {
        Self {
            content: self.content.clone(),
            attributes: self.attributes,
        }
    }

    fn clone_from(&mut self, source: &Self) {
        self.content.clone_from(&source.content);
        self.attributes = source.attributes;
    }
}

impl CompactLiveCell {
    fn into_live(self) -> LiveCell {
        self.content.into_live(self.attributes)
    }
}

#[derive(Debug, Eq, PartialEq)]
struct CompactLiveRow {
    row_id: Option<u64>,
    linebreak: bool,
    cells: Vec<CompactLiveCell>,
}

impl Clone for CompactLiveRow {
    fn clone(&self) -> Self {
        Self {
            row_id: self.row_id,
            linebreak: self.linebreak,
            cells: self.cells.clone(),
        }
    }

    fn clone_from(&mut self, source: &Self) {
        self.row_id = source.row_id;
        self.linebreak = source.linebreak;
        if self.cells.capacity() >= source.cells.len() {
            self.cells.clone_from(&source.cells);
        } else {
            let mut cells = Vec::with_capacity(source.cells.len());
            cells.extend(source.cells.iter().cloned());
            self.cells = cells;
        }
    }
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct SparseRowPatch {
    index: usize,
    row: CompactLiveRow,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum SparseHistoryDelta {
    None,
    Append {
        rows: Vec<CompactLiveRow>,
        final_rows: usize,
    },
    Replace(Vec<CompactLiveRow>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SparseHistoryCapture {
    None,
    Append { first: usize, final_rows: usize },
    Replace,
}

/// Validated successor metadata and damage without duplicated row/history
/// bodies. A queued tail may compose this capture directly from the ephemeral
/// compact snapshot, while a new tail materializes it into an owned frame.
#[derive(Debug)]
struct SparsePublicationCapture {
    incarnation: ProcessIncarnation,
    base_revision: TerminalRevision,
    final_revision: TerminalRevision,
    update: TerminalUpdate,
    metadata: Box<LiveSnapshot>,
    damaged_rows: Vec<usize>,
    history: SparseHistoryCapture,
    history_policy: CompactHistoryPolicy,
    semantic_bytes: u64,
}

impl SparsePublicationCapture {
    fn prepare(
        incarnation: ProcessIncarnation,
        updates: Vec<TerminalUpdate>,
        final_revision: TerminalRevision,
        history_policy: CompactHistoryPolicy,
        history_limit: usize,
        snapshot: &CompactLiveSnapshot,
    ) -> Option<Self> {
        let first_revision = updates.first()?.revision().value();
        let base_revision = TerminalRevision::new(first_revision.checked_sub(1)?);
        if updates.last()?.revision() != final_revision
            || snapshot.metadata.revision != final_revision
            || snapshot.metadata.incarnation != incarnation
            || !updates.windows(2).all(|pair| {
                pair[0].revision().value().checked_add(1) == Some(pair[1].revision().value())
            })
        {
            return None;
        }
        let update = TerminalUpdate::coalesce_contiguous(updates)?;
        let damaged_rows =
            sparse_damaged_rows(std::slice::from_ref(&update), snapshot.visible_rows.len())?;
        let history = match history_policy {
            CompactHistoryPolicy::NoHistory => SparseHistoryCapture::None,
            CompactHistoryPolicy::AppendTail(rows) => SparseHistoryCapture::Append {
                first: snapshot.scrollback_rows.len().saturating_sub(rows),
                final_rows: snapshot
                    .metadata
                    .scrollback
                    .available_rows
                    .min(history_limit),
            },
            CompactHistoryPolicy::FullHistory => SparseHistoryCapture::Replace,
        };
        let metadata = Box::new(snapshot.metadata.clone());
        if !metadata.visible_rows.is_empty() || !metadata.scrollback_rows.is_empty() {
            return None;
        }
        let semantic_bytes = sparse_capture_semantic_bytes(
            &update,
            &damaged_rows,
            damaged_rows.capacity(),
            history,
            snapshot,
            &metadata,
        )?;
        Some(Self {
            incarnation,
            base_revision,
            final_revision,
            update,
            metadata,
            damaged_rows,
            history,
            history_policy,
            semantic_bytes,
        })
    }

    fn attribution(&self) -> PendingFrameAttribution {
        PendingFrameAttribution::one_frame(
            std::slice::from_ref(&self.update),
            self.history_policy,
            self.semantic_bytes,
        )
    }

    fn into_frame(self, snapshot: &CompactLiveSnapshot) -> Option<SparsePublicationFrame> {
        let mut visible_rows = Vec::with_capacity(self.damaged_rows.capacity());
        for index in self.damaged_rows {
            visible_rows.push(SparseRowPatch {
                index,
                row: snapshot.visible_rows.get(index)?.clone(),
            });
        }
        let history = match self.history {
            SparseHistoryCapture::None => SparseHistoryDelta::None,
            SparseHistoryCapture::Append { first, final_rows } => {
                let source = snapshot.scrollback_rows.get(first..)?;
                let mut rows = Vec::with_capacity(source.len());
                rows.extend(source.iter().cloned());
                SparseHistoryDelta::Append { rows, final_rows }
            }
            SparseHistoryCapture::Replace => {
                let mut rows = Vec::with_capacity(snapshot.scrollback_rows.len());
                rows.extend(snapshot.scrollback_rows.iter().cloned());
                SparseHistoryDelta::Replace(rows)
            }
        };
        let updates = vec![self.update];
        let semantic_bytes = sparse_frame_semantic_bytes(
            &updates,
            updates.capacity(),
            &visible_rows,
            visible_rows.capacity(),
            &history,
            &self.metadata,
        )?;
        Some(SparsePublicationFrame {
            incarnation: self.incarnation,
            base_revision: self.base_revision,
            final_revision: self.final_revision,
            updates,
            metadata: self.metadata,
            visible_rows,
            history,
            history_policy: self.history_policy,
            semantic_bytes,
        })
    }
}

/// One producer-boundary publication frame. Ordinary frames own only final rows
/// selected by semantic damage and the exact bounded history delta. The
/// metadata snapshot deliberately has no visible or history row bodies.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug)]
struct SparsePublicationFrame {
    incarnation: ProcessIncarnation,
    base_revision: TerminalRevision,
    final_revision: TerminalRevision,
    updates: Vec<TerminalUpdate>,
    metadata: Box<LiveSnapshot>,
    visible_rows: Vec<SparseRowPatch>,
    history: SparseHistoryDelta,
    history_policy: CompactHistoryPolicy,
    semantic_bytes: u64,
}

impl SparsePublicationFrame {
    #[cfg(test)]
    fn capture(
        incarnation: ProcessIncarnation,
        updates: Vec<TerminalUpdate>,
        final_revision: TerminalRevision,
        history_policy: CompactHistoryPolicy,
        history_limit: usize,
        snapshot: &CompactLiveSnapshot,
    ) -> Option<Self> {
        SparsePublicationCapture::prepare(
            incarnation,
            updates,
            final_revision,
            history_policy,
            history_limit,
            snapshot,
        )?
        .into_frame(snapshot)
    }

    fn attribution(&self) -> PendingFrameAttribution {
        PendingFrameAttribution::one_frame(&self.updates, self.history_policy, self.semantic_bytes)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "direct sparse composition keeps prevalidation and exact-capacity mutation together"
    )]
    fn merge_capture(
        &mut self,
        capture: SparsePublicationCapture,
        snapshot: &CompactLiveSnapshot,
    ) -> Option<()> {
        if self.incarnation != capture.incarnation
            || self.final_revision != capture.base_revision
            || capture.metadata.incarnation != capture.incarnation
            || capture.metadata.revision != capture.final_revision
            || snapshot.metadata.incarnation != capture.incarnation
            || snapshot.metadata.revision != capture.final_revision
            || self.updates.is_empty()
        {
            return None;
        }
        let final_row_count = capture.metadata.dimensions.rows;
        if snapshot.visible_rows.len() != final_row_count
            || capture
                .damaged_rows
                .iter()
                .any(|index| *index >= final_row_count)
            || !capture
                .damaged_rows
                .windows(2)
                .all(|pair| pair[0] < pair[1])
            || (self.metadata.dimensions.rows != final_row_count
                && capture.damaged_rows.len() != final_row_count)
        {
            return None;
        }
        let history_source = match capture.history {
            SparseHistoryCapture::None => None,
            SparseHistoryCapture::Append { first, .. } => {
                Some(snapshot.scrollback_rows.get(first..)?)
            }
            SparseHistoryCapture::Replace => Some(snapshot.scrollback_rows.as_slice()),
        };
        let required_visible_rows = if self.metadata.dimensions.rows == final_row_count {
            self.visible_rows.len().checked_add(
                capture
                    .damaged_rows
                    .iter()
                    .filter(|&&index| {
                        self.visible_rows
                            .binary_search_by_key(&index, |patch| patch.index)
                            .is_err()
                    })
                    .count(),
            )?
        } else {
            final_row_count
        };
        ensure_exact_vec_capacity(&mut self.visible_rows, required_visible_rows);
        let required_updates = self.updates.len().checked_add(1)?;
        ensure_exact_vec_capacity(&mut self.updates, required_updates);
        if self.metadata.dimensions.rows == final_row_count {
            for index in capture.damaged_rows {
                match self
                    .visible_rows
                    .binary_search_by_key(&index, |patch| patch.index)
                {
                    Ok(position) => self.visible_rows[position]
                        .row
                        .clone_from(snapshot.visible_rows.get(index)?),
                    Err(position) => self.visible_rows.insert(
                        position,
                        SparseRowPatch {
                            index,
                            row: snapshot.visible_rows.get(index)?.clone(),
                        },
                    ),
                }
            }
        } else {
            clone_sparse_rows_from_snapshot(&mut self.visible_rows, &snapshot.visible_rows);
        }

        match capture.history {
            SparseHistoryCapture::None => {}
            SparseHistoryCapture::Append { final_rows, .. } => {
                let replace = matches!(self.history, SparseHistoryDelta::Replace(_));
                let mut rows = take_sparse_history_rows(&mut self.history);
                append_bounded_sparse_rows(&mut rows, history_source?, final_rows);
                self.history = if replace {
                    SparseHistoryDelta::Replace(rows)
                } else {
                    SparseHistoryDelta::Append { rows, final_rows }
                };
            }
            SparseHistoryCapture::Replace => {
                let mut rows = take_sparse_history_rows(&mut self.history);
                clone_compact_rows_from_slice(&mut rows, history_source?);
                self.history = SparseHistoryDelta::Replace(rows);
            }
        }
        self.updates.push(capture.update);
        self.final_revision = capture.final_revision;
        self.metadata = capture.metadata;
        self.history_policy = self.history_policy.merge(capture.history_policy);
        self.semantic_bytes = sparse_frame_semantic_bytes(
            &self.updates,
            self.updates.capacity(),
            &self.visible_rows,
            self.visible_rows.capacity(),
            &self.history,
            &self.metadata,
        )?;
        Some(())
    }

    #[cfg(test)]
    fn apply_to(&self, base: &CompactLiveSnapshot) -> Option<CompactLiveSnapshot> {
        if base.metadata.incarnation != self.incarnation
            || base.metadata.revision != self.base_revision
            || self.metadata.revision != self.final_revision
        {
            return None;
        }
        let final_rows = self.metadata.dimensions.rows;
        let mut rows: Vec<Option<CompactLiveRow>> = if base.visible_rows.len() == final_rows {
            base.visible_rows.iter().cloned().map(Some).collect()
        } else {
            (0..final_rows).map(|_| None).collect()
        };
        for patch in &self.visible_rows {
            *rows.get_mut(patch.index)? = Some(patch.row.clone());
        }
        let visible_rows = rows.into_iter().collect::<Option<Vec<_>>>()?;
        let scrollback_rows = match &self.history {
            SparseHistoryDelta::None => base.scrollback_rows.clone(),
            SparseHistoryDelta::Append { rows, final_rows } => {
                let mut result = base.scrollback_rows.clone();
                result.extend(rows.iter().cloned());
                if result.len() > *final_rows {
                    result.drain(..result.len() - *final_rows);
                }
                result
            }
            SparseHistoryDelta::Replace(rows) => rows.clone(),
        };
        let mut metadata = (*self.metadata).clone();
        metadata.visible_rows.clear();
        metadata.scrollback_rows.clear();
        Some(CompactLiveSnapshot {
            metadata,
            visible_rows,
            scrollback_rows,
            history_policy: self.history_policy,
        })
    }
}

fn sparse_damaged_rows(updates: &[TerminalUpdate], row_count: usize) -> Option<Vec<usize>> {
    let mut damaged = vec![false; row_count];
    for damage in updates.iter().flat_map(TerminalUpdate::damage) {
        match damage {
            TerminalDamage::FullSnapshot
            | TerminalDamage::Viewport
            | TerminalDamage::Dimensions
            | TerminalDamage::Images { .. } => damaged.fill(true),
            TerminalDamage::Rows { start, end } => {
                if start > end || *end > damaged.len() {
                    return None;
                }
                damaged[*start..*end].fill(true);
            }
            TerminalDamage::Scroll { region, .. } => {
                let start = usize::try_from(region.start()).ok()?;
                let end = usize::try_from(region.end()).ok()?;
                if start >= end || end > damaged.len() {
                    return None;
                }
                damaged[start..end].fill(true);
            }
            TerminalDamage::Cursor { .. }
            | TerminalDamage::Modes
            | TerminalDamage::Scrollback
            | TerminalDamage::Title
            | TerminalDamage::Palette { .. } => {}
        }
    }
    let damaged_count = damaged.iter().filter(|changed| **changed).count();
    let mut selected = Vec::with_capacity(damaged_count);
    selected.extend(
        damaged
            .into_iter()
            .enumerate()
            .filter_map(|(index, changed)| changed.then_some(index)),
    );
    Some(selected)
}

fn ensure_exact_vec_capacity<T>(values: &mut Vec<T>, required: usize) {
    if values.capacity() >= required {
        return;
    }
    let mut replacement = Vec::with_capacity(required);
    replacement.append(values);
    *values = replacement;
}

fn clone_sparse_rows_from_snapshot(rows: &mut Vec<SparseRowPatch>, source: &[CompactLiveRow]) {
    ensure_exact_vec_capacity(rows, source.len());
    for (index, source_row) in source.iter().enumerate() {
        if let Some(patch) = rows.get_mut(index) {
            patch.index = index;
            patch.row.clone_from(source_row);
        } else {
            rows.push(SparseRowPatch {
                index,
                row: source_row.clone(),
            });
        }
    }
    rows.truncate(source.len());
}

fn clone_compact_rows_from_slice(rows: &mut Vec<CompactLiveRow>, source: &[CompactLiveRow]) {
    ensure_exact_vec_capacity(rows, source.len());
    for (index, source_row) in source.iter().enumerate() {
        if let Some(row) = rows.get_mut(index) {
            row.clone_from(source_row);
        } else {
            rows.push(source_row.clone());
        }
    }
    rows.truncate(source.len());
}

fn append_bounded_sparse_rows(
    rows: &mut Vec<CompactLiveRow>,
    source: &[CompactLiveRow],
    final_rows: usize,
) {
    if final_rows == 0 {
        rows.clear();
        return;
    }
    if source.len() >= final_rows {
        clone_compact_rows_from_slice(rows, &source[source.len() - final_rows..]);
        return;
    }
    ensure_exact_vec_capacity(
        rows,
        rows.len().saturating_add(source.len()).min(final_rows),
    );
    while rows.len() > final_rows {
        rows.remove(0);
    }
    for source_row in source {
        if rows.len() == final_rows {
            let mut reused = rows.remove(0);
            reused.clone_from(source_row);
            rows.push(reused);
        } else {
            rows.push(source_row.clone());
        }
    }
}

fn take_sparse_history_rows(history: &mut SparseHistoryDelta) -> Vec<CompactLiveRow> {
    match std::mem::replace(history, SparseHistoryDelta::None) {
        SparseHistoryDelta::None => Vec::new(),
        SparseHistoryDelta::Append { rows, .. } | SparseHistoryDelta::Replace(rows) => rows,
    }
}

fn sparse_capture_semantic_bytes(
    update: &TerminalUpdate,
    damaged_rows: &[usize],
    damaged_rows_capacity: usize,
    history: SparseHistoryCapture,
    snapshot: &CompactLiveSnapshot,
    metadata: &LiveSnapshot,
) -> Option<u64> {
    let mut total = size_of::<SparsePublicationFrame>().checked_add(size_of::<LiveSnapshot>())?;
    checked_owned_bytes(&mut total, 1, size_of::<TerminalUpdate>())?;
    total = total.checked_add(update.owned_allocation_bytes()?)?;
    checked_owned_bytes(
        &mut total,
        damaged_rows_capacity,
        size_of::<SparseRowPatch>(),
    )?;
    for index in damaged_rows {
        total = total.checked_add(compact_row_nested_bytes(
            snapshot.visible_rows.get(*index)?,
        )?)?;
    }
    let history_rows = match history {
        SparseHistoryCapture::None => None,
        SparseHistoryCapture::Append { first, .. } => Some(snapshot.scrollback_rows.get(first..)?),
        SparseHistoryCapture::Replace => Some(snapshot.scrollback_rows.as_slice()),
    };
    if let Some(rows) = history_rows {
        checked_owned_bytes(&mut total, rows.len(), size_of::<CompactLiveRow>())?;
        for row in rows {
            total = total.checked_add(compact_row_nested_bytes(row)?)?;
        }
    }
    total = total.checked_add(metadata.title.capacity())?;
    checked_owned_bytes(
        &mut total,
        metadata.image_contents.capacity(),
        size_of::<ImageContentMetadata>(),
    )?;
    checked_owned_bytes(
        &mut total,
        metadata.image_placements.capacity(),
        size_of::<ImagePlacement>(),
    )?;
    u64::try_from(total).ok()
}

fn checked_owned_bytes(total: &mut usize, count: usize, item_size: usize) -> Option<()> {
    *total = total.checked_add(count.checked_mul(item_size)?)?;
    Some(())
}

fn compact_row_nested_bytes(row: &CompactLiveRow) -> Option<usize> {
    let mut total = 0;
    checked_owned_bytes(
        &mut total,
        row.cells.capacity(),
        size_of::<CompactLiveCell>(),
    )?;
    for cell in &row.cells {
        total = total.checked_add(cell.content.owned_string_bytes())?;
    }
    Some(total)
}

fn compact_snapshot_semantic_bytes(snapshot: &CompactLiveSnapshot) -> Option<u64> {
    let mut total = size_of::<CompactLiveSnapshot>().checked_add(size_of::<LiveSnapshot>())?;
    checked_owned_bytes(
        &mut total,
        snapshot.visible_rows.capacity(),
        size_of::<CompactLiveRow>(),
    )?;
    checked_owned_bytes(
        &mut total,
        snapshot.scrollback_rows.capacity(),
        size_of::<CompactLiveRow>(),
    )?;
    for row in snapshot
        .visible_rows
        .iter()
        .chain(&snapshot.scrollback_rows)
    {
        total = total.checked_add(compact_row_nested_bytes(row)?)?;
    }
    total = total.checked_add(snapshot.metadata.title.capacity())?;
    checked_owned_bytes(
        &mut total,
        snapshot.metadata.image_contents.capacity(),
        size_of::<ImageContentMetadata>(),
    )?;
    checked_owned_bytes(
        &mut total,
        snapshot.metadata.image_placements.capacity(),
        size_of::<ImagePlacement>(),
    )?;
    u64::try_from(total).ok()
}

fn compact_materialization_semantic_bytes(
    rows: &[CompactLiveRow],
    rows_capacity: usize,
) -> Option<u64> {
    let mut total = size_of::<CompactMaterializationState>();
    checked_owned_bytes(&mut total, rows_capacity, size_of::<CompactLiveRow>())?;
    for row in rows {
        total = total.checked_add(compact_row_nested_bytes(row)?)?;
    }
    u64::try_from(total).ok()
}

fn compact_materialization_clone_bound(
    current: &[CompactLiveRow],
    current_capacity: usize,
    source: &[CompactLiveRow],
) -> Option<u64> {
    let mut total = size_of::<CompactMaterializationState>();
    checked_owned_bytes(
        &mut total,
        current_capacity.max(source.len()),
        size_of::<CompactLiveRow>(),
    )?;
    for (index, source_row) in source.iter().enumerate() {
        let current_row = current.get(index);
        let cells_capacity = current_row.map_or(source_row.cells.len(), |row| {
            row.cells.capacity().max(source_row.cells.len())
        });
        checked_owned_bytes(&mut total, cells_capacity, size_of::<CompactLiveCell>())?;
        for (cell_index, source_cell) in source_row.cells.iter().enumerate() {
            let source_bytes = source_cell.content.owned_string_bytes();
            let retained_bytes = current_row
                .and_then(|row| row.cells.get(cell_index))
                .map_or(0, |cell| match (&cell.content, &source_cell.content) {
                    (CompactCellContent::Composed(current), CompactCellContent::Composed(next))
                        if current.capacity() >= next.len() =>
                    {
                        current.capacity()
                    }
                    _ => 0,
                });
            total = total.checked_add(retained_bytes.max(source_bytes))?;
        }
    }
    u64::try_from(total).ok()
}

fn sparse_frame_semantic_bytes(
    updates: &[TerminalUpdate],
    updates_capacity: usize,
    visible_rows: &[SparseRowPatch],
    visible_rows_capacity: usize,
    history: &SparseHistoryDelta,
    metadata: &LiveSnapshot,
) -> Option<u64> {
    let mut total = size_of::<SparsePublicationFrame>().checked_add(size_of::<LiveSnapshot>())?;
    checked_owned_bytes(&mut total, updates_capacity, size_of::<TerminalUpdate>())?;
    for update in updates {
        total = total.checked_add(update.owned_allocation_bytes()?)?;
    }
    checked_owned_bytes(
        &mut total,
        visible_rows_capacity,
        size_of::<SparseRowPatch>(),
    )?;
    for patch in visible_rows {
        total = total.checked_add(compact_row_nested_bytes(&patch.row)?)?;
    }
    let history_rows = match history {
        SparseHistoryDelta::None => None,
        SparseHistoryDelta::Append { rows, .. } | SparseHistoryDelta::Replace(rows) => Some(rows),
    };
    if let Some(history_rows) = history_rows {
        checked_owned_bytes(
            &mut total,
            history_rows.capacity(),
            size_of::<CompactLiveRow>(),
        )?;
        for row in history_rows {
            total = total.checked_add(compact_row_nested_bytes(row)?)?;
        }
    }
    total = total.checked_add(metadata.title.capacity())?;
    checked_owned_bytes(
        &mut total,
        metadata.image_contents.capacity(),
        size_of::<ImageContentMetadata>(),
    )?;
    checked_owned_bytes(
        &mut total,
        metadata.image_placements.capacity(),
        size_of::<ImagePlacement>(),
    )?;
    u64::try_from(total).ok()
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
struct CompactMaterializationState {
    incarnation: Option<ProcessIncarnation>,
    revision: TerminalRevision,
    visible_rows: Vec<CompactLiveRow>,
    semantic_admission: SemanticByteLease,
    // History is intentionally event-local: AppendTail snapshots carry only the
    // rows a client must append to its already retained history. FullHistory
    // replaces that client state. Retaining old rows here would duplicate them
    // across separately delivered mailbox drains.
    history_limit: usize,
}

impl CompactMaterializationState {
    fn from_snapshot(
        snapshot: CompactLiveSnapshot,
        history_limit: usize,
        accounting: &Arc<QueueAccounting>,
    ) -> Option<Self> {
        let semantic_bytes = compact_materialization_semantic_bytes(
            &snapshot.visible_rows,
            snapshot.visible_rows.capacity(),
        )?;
        Some(Self {
            incarnation: Some(snapshot.metadata.incarnation),
            revision: snapshot.metadata.revision,
            visible_rows: snapshot.visible_rows,
            semantic_admission: SemanticByteLease::try_new(accounting, semantic_bytes)?,
            history_limit,
        })
    }

    fn replace_visible_rows(&mut self, source: &[CompactLiveRow]) -> Option<()> {
        let bound = compact_materialization_clone_bound(
            &self.visible_rows,
            self.visible_rows.capacity(),
            source,
        )?;
        self.semantic_admission.resize(bound)?;
        clone_compact_rows_from_slice(&mut self.visible_rows, source);
        let exact = compact_materialization_semantic_bytes(
            &self.visible_rows,
            self.visible_rows.capacity(),
        )?;
        debug_assert!(exact <= bound);
        self.semantic_admission.resize(exact)
    }
}

fn merge_materialized_snapshots(
    current: &mut Box<CompactLiveSnapshot>,
    mut next: Box<CompactLiveSnapshot>,
    history_limit: usize,
) {
    let history_policy = current.history_policy.merge(next.history_policy);
    let mut history_rows = std::mem::take(&mut current.scrollback_rows);
    match next.history_policy {
        CompactHistoryPolicy::FullHistory => {
            history_rows = std::mem::take(&mut next.scrollback_rows);
        }
        CompactHistoryPolicy::AppendTail(_) => {
            history_rows.append(&mut next.scrollback_rows);
            let final_rows = next.metadata.scrollback.available_rows.min(history_limit);
            if history_rows.len() > final_rows {
                history_rows.drain(..history_rows.len() - final_rows);
            }
        }
        CompactHistoryPolicy::NoHistory => {}
    }
    next.metadata.scrollback.returned_rows = history_rows.len();
    next.metadata.scrollback.omitted_oldest_rows = next
        .metadata
        .scrollback
        .available_rows
        .saturating_sub(history_rows.len());
    next.scrollback_rows = history_rows;
    next.history_policy = history_policy;
    *current = next;
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
    semantic_bytes: u64,
}

impl PendingFrameAttribution {
    fn one_frame(
        updates: &[TerminalUpdate],
        history_policy: CompactHistoryPolicy,
        semantic_bytes: u64,
    ) -> Self {
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
            semantic_bytes,
        }
    }

    fn merge(&mut self, next: Self) {
        self.batches = self.batches.saturating_add(next.batches);
        self.terminal_updates = self.terminal_updates.saturating_add(next.terminal_updates);
        self.scrolls = self.scrolls.saturating_add(next.scrolls);
        self.appended_rows = self.appended_rows.saturating_add(next.appended_rows);
        self.semantic_bytes = self.semantic_bytes.saturating_add(next.semantic_bytes);
    }
}

#[derive(Debug)]
struct PendingCompactUpdates {
    incarnation: ProcessIncarnation,
    frames: Vec<SparsePublicationFrame>,
    end_revision: TerminalRevision,
    history_policy: CompactHistoryPolicy,
    admissions: Vec<Option<QueueLease>>,
    semantic_admissions: Vec<SemanticByteLease>,
    pending_attributions: Vec<PendingFrameLease>,
}

impl PendingCompactUpdates {
    fn materialize(
        self,
        state: &mut CompactMaterializationState,
    ) -> Option<(
        ProcessIncarnation,
        Vec<TerminalUpdate>,
        TerminalRevision,
        CompactLiveSnapshot,
    )> {
        let total_updates = self.frames.iter().try_fold(0_usize, |total, frame| {
            total.checked_add(frame.updates.len())
        })?;
        let mut updates = Vec::with_capacity(total_updates);
        let mut history_rows = Vec::new();
        let mut final_metadata = None;
        let mut materialized_incarnation = state.incarnation;
        let mut materialized_revision = state.revision;
        let mut materialized_rows = state.visible_rows.clone();
        for frame in self.frames {
            if frame.incarnation != self.incarnation
                || materialized_incarnation.is_some_and(|current| current != frame.incarnation)
                || frame.base_revision != materialized_revision
                || frame.metadata.incarnation != frame.incarnation
                || frame.metadata.revision != frame.final_revision
            {
                return None;
            }
            materialized_incarnation = Some(frame.incarnation);
            let final_row_count = frame.metadata.dimensions.rows;
            let mut visible_rows: Vec<Option<CompactLiveRow>> =
                if materialized_rows.len() == final_row_count {
                    materialized_rows.drain(..).map(Some).collect()
                } else {
                    (0..final_row_count).map(|_| None).collect()
                };
            for patch in frame.visible_rows {
                *visible_rows.get_mut(patch.index)? = Some(patch.row);
            }
            materialized_rows = visible_rows.into_iter().collect::<Option<Vec<_>>>()?;
            materialized_revision = frame.final_revision;
            match frame.history {
                SparseHistoryDelta::None => {}
                SparseHistoryDelta::Append { rows, final_rows } => {
                    history_rows.extend(rows);
                    if history_rows.len() > final_rows {
                        history_rows.drain(..history_rows.len() - final_rows);
                    }
                }
                SparseHistoryDelta::Replace(rows) => history_rows = rows,
            }
            updates.extend(frame.updates);
            final_metadata = Some(frame.metadata);
        }
        if materialized_revision != self.end_revision {
            return None;
        }
        let updates = vec![TerminalUpdate::coalesce_publication_summaries(updates)?];
        let mut metadata = *final_metadata?;
        metadata.visible_rows.clear();
        metadata.scrollback_rows.clear();
        metadata.scrollback.returned_rows = history_rows.len();
        metadata.scrollback.omitted_oldest_rows = metadata
            .scrollback
            .available_rows
            .saturating_sub(history_rows.len());
        for attribution in &self.pending_attributions {
            attribution.record_materialization();
        }
        state.replace_visible_rows(&materialized_rows)?;
        state.incarnation = materialized_incarnation;
        state.revision = materialized_revision;
        Some((
            self.incarnation,
            updates,
            self.end_revision,
            CompactLiveSnapshot {
                metadata,
                visible_rows: materialized_rows,
                scrollback_rows: history_rows,
                history_policy: self.history_policy,
            },
        ))
    }
}

#[derive(Debug, Default)]
struct CompactMailboxState {
    pending: VecDeque<PendingCompactUpdates>,
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
        self.lock().pending.clear();
    }

    fn take_pending(&self, state: &mut CompactMaterializationState) -> MailboxTake {
        let mut pending = std::mem::take(&mut self.lock().pending);
        let Some(mut combined) = pending.pop_front() else {
            return MailboxTake::MissingOrMismatched;
        };
        for mut sealed in pending {
            if combined.incarnation != sealed.incarnation
                || combined.end_revision
                    != sealed
                        .frames
                        .first()
                        .map(|frame| frame.base_revision)
                        .unwrap_or_default()
            {
                return MailboxTake::MissingOrMismatched;
            }
            combined.frames.append(&mut sealed.frames);
            combined.admissions.append(&mut sealed.admissions);
            combined
                .semantic_admissions
                .append(&mut sealed.semantic_admissions);
            combined
                .pending_attributions
                .append(&mut sealed.pending_attributions);
            combined.end_revision = sealed.end_revision;
            combined.history_policy = combined.history_policy.merge(sealed.history_policy);
        }
        let Some((incarnation, updates, end_revision, snapshot)) = combined.materialize(state)
        else {
            return MailboxTake::MissingOrMismatched;
        };
        MailboxTake::Exact {
            incarnation,
            updates,
            end_revision,
            snapshot: Box::new(snapshot),
        }
    }
}

#[derive(Debug)]
struct QueueAccounting {
    enabled: bool,
    local_events: AtomicUsize,
    local_semantic_bytes: AtomicU64,
    local_semantic_byte_limit: u64,
    metrics: Arc<RuntimeMetrics>,
    #[cfg(test)]
    materializations: AtomicUsize,
}

impl QueueAccounting {
    fn new(enabled: bool, metrics: Arc<RuntimeMetrics>) -> Self {
        Self::with_semantic_byte_limit(enabled, metrics, SUBSCRIBER_SPARSE_SEMANTIC_BYTES)
    }

    fn with_semantic_byte_limit(
        enabled: bool,
        metrics: Arc<RuntimeMetrics>,
        local_semantic_byte_limit: u64,
    ) -> Self {
        Self {
            enabled,
            local_events: AtomicUsize::new(0),
            local_semantic_bytes: AtomicU64::new(0),
            local_semantic_byte_limit,
            metrics,
            #[cfg(test)]
            materializations: AtomicUsize::new(0),
        }
    }

    fn try_reserve_counter(counter: &AtomicU64, amount: u64, limit: u64) -> bool {
        counter
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(amount).filter(|next| *next <= limit)
            })
            .is_ok()
    }

    fn try_admit_producer_bytes(&self, amount: u64) -> Option<TerminalPublicationMemoryLease> {
        if !Self::try_reserve_counter(
            &self.metrics.sparse_semantic_bytes_current,
            amount,
            SPLINT_SPARSE_SEMANTIC_BYTES,
        ) {
            return None;
        }
        let Some(daemon) = usize::try_from(amount)
            .ok()
            .and_then(TerminalPublicationMemoryLease::try_new)
        else {
            self.metrics
                .sparse_semantic_bytes_current
                .fetch_sub(amount, Ordering::AcqRel);
            return None;
        };
        Some(daemon)
    }

    fn try_admit_semantic_bytes(&self, amount: u64) -> Option<TerminalPublicationMemoryLease> {
        if !Self::try_reserve_counter(
            &self.local_semantic_bytes,
            amount,
            self.local_semantic_byte_limit,
        ) {
            return None;
        }
        if !Self::try_reserve_counter(
            &self.metrics.sparse_semantic_bytes_current,
            amount,
            SPLINT_SPARSE_SEMANTIC_BYTES,
        ) {
            self.local_semantic_bytes
                .fetch_sub(amount, Ordering::AcqRel);
            return None;
        }
        let Some(daemon) = usize::try_from(amount)
            .ok()
            .and_then(TerminalPublicationMemoryLease::try_new)
        else {
            self.metrics
                .sparse_semantic_bytes_current
                .fetch_sub(amount, Ordering::AcqRel);
            self.local_semantic_bytes
                .fetch_sub(amount, Ordering::AcqRel);
            return None;
        };
        Some(daemon)
    }

    fn release_semantic_bytes(&self, amount: u64) {
        self.metrics
            .sparse_semantic_bytes_current
            .fetch_sub(amount, Ordering::AcqRel);
        self.local_semantic_bytes
            .fetch_sub(amount, Ordering::AcqRel);
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

/// Conservative transient producer ownership under Splint and daemon ceilings.
/// It transfers into exact retained subscriber ownership without releasing the
/// shared authorities in between.
#[derive(Debug)]
struct ProducerBuildLease {
    accounting: Arc<QueueAccounting>,
    daemon: TerminalPublicationMemoryLease,
    bytes: u64,
}

impl ProducerBuildLease {
    fn try_new(accounting: &Arc<QueueAccounting>, bytes: u64) -> Option<Self> {
        let daemon = accounting.try_admit_producer_bytes(bytes)?;
        Some(Self {
            accounting: Arc::clone(accounting),
            daemon,
            bytes,
        })
    }

    fn into_semantic(mut self, bytes: u64) -> Option<SemanticByteLease> {
        if bytes > self.bytes
            || !QueueAccounting::try_reserve_counter(
                &self.accounting.local_semantic_bytes,
                bytes,
                self.accounting.local_semantic_byte_limit,
            )
        {
            return None;
        }
        let released = self.bytes - bytes;
        if released > 0 {
            self.accounting
                .metrics
                .sparse_semantic_bytes_current
                .fetch_sub(released, Ordering::AcqRel);
            DAEMON_TERMINAL_PUBLICATION_BYTES_CURRENT.fetch_sub(released, Ordering::AcqRel);
        }
        self.bytes = 0;
        self.daemon.bytes = bytes;
        let daemon = std::mem::replace(
            &mut self.daemon,
            TerminalPublicationMemoryLease { bytes: 0 },
        );
        Some(SemanticByteLease {
            accounting: Arc::clone(&self.accounting),
            daemon,
            bytes,
        })
    }
}

impl Drop for ProducerBuildLease {
    fn drop(&mut self) {
        self.accounting
            .metrics
            .sparse_semantic_bytes_current
            .fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// Checked ownership of sparse semantic bytes across subscriber, Splint, and
/// daemon publication ceilings.
#[derive(Debug)]
struct SemanticByteLease {
    accounting: Arc<QueueAccounting>,
    daemon: TerminalPublicationMemoryLease,
    bytes: u64,
}

impl SemanticByteLease {
    fn try_new(accounting: &Arc<QueueAccounting>, bytes: u64) -> Option<Self> {
        let daemon = accounting.try_admit_semantic_bytes(bytes)?;
        Some(Self {
            accounting: Arc::clone(accounting),
            daemon,
            bytes,
        })
    }

    fn resize(&mut self, bytes: u64) -> Option<()> {
        match bytes.cmp(&self.bytes) {
            std::cmp::Ordering::Greater => {
                let mut extra = Self::try_new(&self.accounting, bytes - self.bytes)?;
                extra.bytes = 0;
                extra.daemon.bytes = 0;
            }
            std::cmp::Ordering::Less => {
                let released = self.bytes - bytes;
                self.accounting.release_semantic_bytes(released);
                DAEMON_TERMINAL_PUBLICATION_BYTES_CURRENT.fetch_sub(released, Ordering::AcqRel);
            }
            std::cmp::Ordering::Equal => {}
        }
        self.bytes = bytes;
        self.daemon.bytes = bytes;
        Some(())
    }

    fn consolidate(leases: &mut Vec<Self>, bytes: u64) -> Option<()> {
        let accounting = Arc::clone(&leases.first()?.accounting);
        if !leases
            .iter()
            .all(|lease| Arc::ptr_eq(&lease.accounting, &accounting))
        {
            return None;
        }
        let admitted = leases
            .iter()
            .try_fold(0_u64, |total, lease| total.checked_add(lease.bytes))?;
        let mut extra = if bytes > admitted {
            Some(Self::try_new(&accounting, bytes - admitted)?)
        } else {
            None
        };
        if admitted > bytes {
            accounting.release_semantic_bytes(admitted - bytes);
            DAEMON_TERMINAL_PUBLICATION_BYTES_CURRENT.fetch_sub(admitted - bytes, Ordering::AcqRel);
        }
        for lease in leases.iter_mut() {
            lease.bytes = 0;
            lease.daemon.bytes = 0;
        }
        if let Some(lease) = extra.as_mut() {
            lease.bytes = 0;
            lease.daemon.bytes = 0;
        }
        leases.clear();
        leases.push(Self {
            accounting,
            daemon: TerminalPublicationMemoryLease { bytes },
            bytes,
        });
        Some(())
    }
}

impl Drop for SemanticByteLease {
    fn drop(&mut self) {
        self.accounting.release_semantic_bytes(self.bytes);
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

    fn replace_after_seal(&mut self, attribution: PendingFrameAttribution) {
        self.accounting
            .metrics
            .remove_queued_compact(self.attribution);
        self.accounting
            .metrics
            .add_queued_compact_current(attribution);
        self.attribution = attribution;
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
    reason = "compact publication validates, merges, seals, and admits one producer frame"
)]
fn publish_compact_update(
    sender: &mpsc::Sender<CompactQueuedEvent>,
    accounting: &Arc<QueueAccounting>,
    snapshot_slot: &Arc<CompactSnapshotSlot>,
    semantic_capacity: usize,
    history_limit: usize,
    incarnation: ProcessIncarnation,
    updates: Vec<TerminalUpdate>,
    end_revision: TerminalRevision,
    history_policy: CompactHistoryPolicy,
    build_semantic_bound: u64,
    metrics: &Arc<RuntimeMetrics>,
    mut build_snapshot: impl FnMut(CompactHistoryPolicy) -> CompactLiveSnapshot,
) -> CompactPublishOutcome {
    let queued_frames = {
        let mailbox = snapshot_slot.lock();
        if sender.is_closed() {
            return CompactPublishOutcome::Closed;
        }
        mailbox.pending.iter().try_fold(0_usize, |total, pending| {
            total.checked_add(pending.admissions.len())
        })
    };
    if queued_frames.is_none_or(|frames| frames >= semantic_capacity) {
        snapshot_slot.clear();
        return CompactPublishOutcome::Full;
    }

    // Cover the complete ephemeral snapshot and worst-case sparse capture under
    // Splint and daemon authority before either allocates. After construction,
    // transfer this lease into exact retained subscriber ownership.
    let Some(build_admission) = ProducerBuildLease::try_new(accounting, build_semantic_bound)
    else {
        snapshot_slot.clear();
        return CompactPublishOutcome::Full;
    };
    let snapshot = build_snapshot(history_policy);
    if accounting.enabled {
        record_publication_snapshot(metrics, compact_snapshot_attribution(&snapshot));
    }
    let Some(snapshot_semantic_bytes) = compact_snapshot_semantic_bytes(&snapshot) else {
        snapshot_slot.clear();
        return CompactPublishOutcome::Full;
    };
    let Some(capture) = SparsePublicationCapture::prepare(
        incarnation,
        updates,
        end_revision,
        snapshot.history_policy,
        history_limit,
        &snapshot,
    ) else {
        snapshot_slot.clear();
        return CompactPublishOutcome::Full;
    };
    let capture_attribution = capture.attribution();
    let actual_build_bytes =
        snapshot_semantic_bytes.checked_add(capture_attribution.semantic_bytes);
    if actual_build_bytes.is_none_or(|bytes| bytes > build_semantic_bound) {
        snapshot_slot.clear();
        return CompactPublishOutcome::Full;
    }

    let mut mailbox = snapshot_slot.lock();
    if sender.is_closed() {
        return CompactPublishOutcome::Closed;
    }
    let queued_frames = mailbox.pending.iter().try_fold(0_usize, |total, pending| {
        total.checked_add(pending.admissions.len())
    });
    if queued_frames.is_none_or(|frames| frames >= semantic_capacity) {
        mailbox.pending.clear();
        return CompactPublishOutcome::Full;
    }
    if mailbox.pending.back().is_some_and(|previous| {
        previous.incarnation != incarnation || previous.end_revision != capture.base_revision
    }) {
        mailbox.pending.clear();
        return CompactPublishOutcome::Full;
    }

    // One mailbox-local tail owns the entire admitted sparse sequence. Count
    // and semantic-byte leases remain per producer frame and enforce the
    // existing 64-event and 16 MiB subscriber ceilings.
    let merge_into_tail = mailbox.pending.back().is_some();
    if merge_into_tail {
        let count_admission = QueueLease::new(accounting);
        let tail = mailbox.pending.back_mut().expect("checked sparse tail");
        let Some(aggregate) = tail.frames.first_mut() else {
            mailbox.pending.clear();
            return CompactPublishOutcome::Full;
        };
        // The existing aggregate and complete successor capture are both
        // admitted before mutation. Exact-capacity growth helpers guarantee the
        // merged tail cannot retain more than their combined semantic ownership.
        let Some(admitted_merge_bound) = aggregate
            .semantic_bytes
            .checked_add(capture_attribution.semantic_bytes)
        else {
            mailbox.pending.clear();
            return CompactPublishOutcome::Full;
        };
        if aggregate.merge_capture(capture, &snapshot).is_none()
            || aggregate.semantic_bytes > admitted_merge_bound
        {
            mailbox.pending.clear();
            return CompactPublishOutcome::Full;
        }
        drop(snapshot);
        let Some(semantic_admission) =
            build_admission.into_semantic(capture_attribution.semantic_bytes)
        else {
            mailbox.pending.clear();
            return CompactPublishOutcome::Full;
        };
        tail.admissions.push(count_admission);
        tail.semantic_admissions.push(semantic_admission);
        if SemanticByteLease::consolidate(&mut tail.semantic_admissions, aggregate.semantic_bytes)
            .is_none()
        {
            mailbox.pending.clear();
            return CompactPublishOutcome::Full;
        }
        if let Some(lease) = tail.pending_attributions.first_mut() {
            lease.merge(capture_attribution);
            let mut sealed_attribution = aggregate.attribution();
            sealed_attribution.batches = u64::try_from(tail.admissions.len()).unwrap_or(u64::MAX);
            lease.replace_after_seal(sealed_attribution);
        }
        tail.end_revision = end_revision;
        tail.history_policy = aggregate.history_policy;
        return CompactPublishOutcome::Published;
    }

    let snapshot_history_policy = snapshot.history_policy;
    let Some(frame) = capture.into_frame(&snapshot) else {
        mailbox.pending.clear();
        return CompactPublishOutcome::Full;
    };
    let frame_attribution = frame.attribution();
    drop(snapshot);
    let Some(semantic_admission) = build_admission.into_semantic(frame_attribution.semantic_bytes)
    else {
        mailbox.pending.clear();
        return CompactPublishOutcome::Full;
    };

    // Preserve one wake token per nonempty mailbox. Sealed chunks remain
    // distinct ownership units, but a receiver drains and materializes all
    // chunks covered by the token before the next writer transaction.
    let permit = if mailbox.pending.is_empty() {
        match sender.try_reserve() {
            Ok(permit) => Some(permit),
            Err(mpsc::error::TrySendError::Full(())) => {
                mailbox.pending.clear();
                return CompactPublishOutcome::Full;
            }
            Err(mpsc::error::TrySendError::Closed(())) => {
                return CompactPublishOutcome::Closed;
            }
        }
    } else {
        None
    };
    mailbox.pending.push_back(PendingCompactUpdates {
        incarnation,
        frames: vec![frame],
        end_revision,
        history_policy: snapshot_history_policy,
        admissions: vec![QueueLease::new(accounting)],
        semantic_admissions: vec![semantic_admission],
        pending_attributions: PendingFrameLease::new(accounting, frame_attribution)
            .into_iter()
            .collect(),
    });
    drop(mailbox);
    if let Some(permit) = permit {
        permit.send(CompactQueuedEvent::UpdateReady);
    }
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
    materialization: Box<CompactMaterializationState>,
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
            CompactQueuedEvent::UpdateReady => {
                match self.snapshot_slot.take_pending(&mut self.materialization) {
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
                }
            }
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
        let (received, trailing_exit, _admission) =
            self.recv_coalesced_with_publication_admission(0).await;
        (received, trailing_exit)
    }

    /// Receives and materializes one coalesced event while holding a conservative
    /// process-wide publication reservation acquired only after an update wake.
    /// The returned lease must remain alive through wire encoding and delivery.
    pub async fn recv_coalesced_with_publication_admission(
        &mut self,
        publication_bytes: usize,
    ) -> (
        SubscriptionReceive,
        Option<ProcessExit>,
        Option<TerminalPublicationMemoryLease>,
    ) {
        if *self.resnapshot.borrow() {
            self.snapshot_slot.clear();
            return (SubscriptionReceive::ResnapshotRequired, None, None);
        }
        let events = &mut self.events;
        let resnapshot = &mut self.resnapshot;
        let mut first = tokio::select! {
            biased;
            changed = resnapshot.changed() => {
                if changed.is_ok() && *resnapshot.borrow() {
                    self.snapshot_slot.clear();
                    return (SubscriptionReceive::ResnapshotRequired, None, None);
                }
                match events.try_recv() {
                    Ok(event) => event,
                    Err(_) => return (SubscriptionReceive::Closed, None, None),
                }
            }
            event = events.recv() => match event {
                Some(event) => event,
                None => return (SubscriptionReceive::Closed, None, None),
            },
        };
        first.release_admitted_ownership();
        let publication_admission = if matches!(first, CompactQueuedEvent::UpdateReady)
            && publication_bytes > 0
        {
            let Some(admission) = TerminalPublicationMemoryLease::try_new(publication_bytes) else {
                self.snapshot_slot.clear();
                return (SubscriptionReceive::ResnapshotRequired, None, None);
            };
            Some(admission)
        } else {
            None
        };
        if self
            .snapshot_slot
            .wait_for_producer_batch(&mut self.resnapshot)
            .await
        {
            self.snapshot_slot.clear();
            return (SubscriptionReceive::ResnapshotRequired, None, None);
        }
        let (received, trailing_exit) = self.coalesce_queued(&first);
        (received, trailing_exit, publication_admission)
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
        } = self.snapshot_slot.take_pending(&mut self.materialization)
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
                    } = self.snapshot_slot.take_pending(&mut self.materialization)
                    else {
                        self.snapshot_slot.clear();
                        return (SubscriptionReceive::ResnapshotRequired, None);
                    };
                    debug_assert_eq!(incarnation, pending_incarnation);
                    if self.accounting.enabled {
                        RuntimeMetrics::add_saturating(
                            &self.accounting.metrics.publication_compact_batch_merges,
                            1,
                        );
                    }
                    updates.extend(pending_updates);
                    incarnation = pending_incarnation;
                    end_revision = pending_revision;
                    merge_materialized_snapshots(
                        &mut snapshot,
                        pending_snapshot,
                        self.materialization.history_limit,
                    );
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
    #[error("terminal publication memory limit exceeded")]
    PublicationMemoryFull,
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

#[derive(Debug)]
struct PtyResume {
    session: AdoptableLinuxPtySession,
    acknowledged: Option<Reply<()>>,
}

/// Exclusive ownership of one quiesced actor's canonical PTY master.
///
/// The actor stops reading commands and PTY I/O until this lease is resumed or
/// dropped. Dropping it returns the unchanged descriptor automatically, keeping
/// cancellation before daemon exec recoverable.
#[derive(Debug)]
pub struct PreparedPtyHandoff {
    identity: LinuxPtyIdentity,
    master_raw_fd: i32,
    #[cfg(test)]
    retired_reader_raw_fd: i32,
    session: Option<AdoptableLinuxPtySession>,
    resume: Option<oneshot::Sender<PtyResume>>,
}

impl PreparedPtyHandoff {
    #[must_use]
    pub const fn identity(&self) -> LinuxPtyIdentity {
        self.identity
    }

    #[must_use]
    pub const fn master_raw_fd(&self) -> i32 {
        self.master_raw_fd
    }

    #[cfg(test)]
    #[must_use]
    const fn retired_reader_raw_fd(&self) -> i32 {
        self.retired_reader_raw_fd
    }

    /// Returns the unchanged canonical PTY master to its actor and waits until
    /// one replacement async reader is active.
    ///
    /// # Errors
    /// Returns [`LiveError::Closed`] when the actor ends or cannot restore its
    /// validated PTY session and replacement reader.
    pub async fn resume(mut self) -> Result<(), LiveError> {
        let session = self.session.take().ok_or(LiveError::Closed)?;
        let resume = self.resume.take().ok_or(LiveError::Closed)?;
        let (acknowledged, receiver) = oneshot::channel();
        resume
            .send(PtyResume {
                session,
                acknowledged: Some(acknowledged),
            })
            .map_err(|_| LiveError::Closed)?;
        receiver.await.map_err(|_| LiveError::Closed)?
    }
}

impl Drop for PreparedPtyHandoff {
    fn drop(&mut self) {
        let (Some(session), Some(resume)) = (self.session.take(), self.resume.take()) else {
            return;
        };
        let _ = resume.send(PtyResume {
            session,
            acknowledged: None,
        });
    }
}

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
    PreparePtyHandoff(Reply<PreparedPtyHandoff>),
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
    /// Checked semantic bytes released by compact-frame materialization.
    pub publication_compact_materialized_semantic_bytes: u64,
    pub publication_compact_materialized_batches_high_water: u64,
    pub publication_compact_materialized_terminal_updates_high_water: u64,
    pub publication_compact_materialized_scrolls_high_water: u64,
    pub publication_compact_materialized_appended_rows_high_water: u64,
    pub publication_compact_materialized_semantic_bytes_high_water: u64,
    /// Current and high-water semantic ownership before wire materialization.
    pub queued_compact_batches_current: u64,
    pub queued_compact_batches_high_water: u64,
    pub queued_compact_terminal_updates_current: u64,
    pub queued_compact_terminal_updates_high_water: u64,
    pub queued_compact_scrolls_current: u64,
    pub queued_compact_scrolls_high_water: u64,
    pub queued_compact_appended_rows_current: u64,
    pub queued_compact_appended_rows_high_water: u64,
    pub queued_compact_semantic_bytes_current: u64,
    pub queued_compact_semantic_bytes_high_water: u64,
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
    publication_compact_materialized_semantic_bytes: AtomicU64,
    publication_compact_materialized_batches_high_water: AtomicU64,
    publication_compact_materialized_terminal_updates_high_water: AtomicU64,
    publication_compact_materialized_scrolls_high_water: AtomicU64,
    publication_compact_materialized_appended_rows_high_water: AtomicU64,
    publication_compact_materialized_semantic_bytes_high_water: AtomicU64,
    queued_compact_batches_current: AtomicU64,
    queued_compact_batches_high_water: AtomicU64,
    queued_compact_terminal_updates_current: AtomicU64,
    queued_compact_terminal_updates_high_water: AtomicU64,
    queued_compact_scrolls_current: AtomicU64,
    queued_compact_scrolls_high_water: AtomicU64,
    queued_compact_appended_rows_current: AtomicU64,
    queued_compact_appended_rows_high_water: AtomicU64,
    queued_compact_semantic_bytes_current: AtomicU64,
    queued_compact_semantic_bytes_high_water: AtomicU64,
    /// Authoritative per-Splint admission independent of optional metrics.
    sparse_semantic_bytes_current: AtomicU64,
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

    fn add_queued_compact(&self, attribution: PendingFrameAttribution) {
        RuntimeMetrics::add_saturating(&self.publication_compact_batches, attribution.batches);
        self.add_queued_compact_current(attribution);
    }

    fn add_queued_compact_current(&self, attribution: PendingFrameAttribution) {
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
        let semantic_bytes = Self::add_u64_saturating(
            &self.queued_compact_semantic_bytes_current,
            attribution.semantic_bytes,
        );
        Self::observe_max_u64(
            &self.queued_compact_semantic_bytes_high_water,
            semantic_bytes,
        );
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
        Self::sub_u64_saturating(
            &self.queued_compact_semantic_bytes_current,
            attribution.semantic_bytes,
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
        Self::add_saturating(
            &self.publication_compact_materialized_semantic_bytes,
            attribution.semantic_bytes,
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
        Self::observe_max_u64(
            &self.publication_compact_materialized_semantic_bytes_high_water,
            attribution.semantic_bytes,
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
            publication_compact_materialized_semantic_bytes: self
                .publication_compact_materialized_semantic_bytes
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
            publication_compact_materialized_semantic_bytes_high_water: self
                .publication_compact_materialized_semantic_bytes_high_water
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
            queued_compact_semantic_bytes_current: self
                .queued_compact_semantic_bytes_current
                .load(Ordering::Relaxed),
            queued_compact_semantic_bytes_high_water: self
                .queued_compact_semantic_bytes_high_water
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

    /// Fences later commands, drains accepted PTY writes, closes the cloned
    /// async reader, and returns exclusive ownership of the canonical master.
    ///
    /// Dropping the returned lease resumes the actor automatically. Call
    /// [`PreparedPtyHandoff::resume`] when recovery must be acknowledged.
    ///
    /// # Errors
    /// Returns a PTY identity/exit error when the live session cannot be safely
    /// converted, or [`LiveError::Closed`] when the actor has ended.
    pub async fn prepare_pty_handoff(&self) -> Result<PreparedPtyHandoff, LiveError> {
        self.request(Command::PreparePtyHandoff).await
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

pub trait ProcessPlacement: Send + 'static {
    fn release(self);
}

impl ProcessPlacement for () {
    fn release(self) {}
}

struct SpawnOutcome<P: ProcessPlacement>(Option<Result<(LinuxPtySession, P), PtyError>>);

impl<P: ProcessPlacement> SpawnOutcome<P> {
    fn new(result: Result<(LinuxPtySession, P), PtyError>) -> Self {
        Self(Some(result))
    }

    fn take(&mut self) -> Result<(LinuxPtySession, P), PtyError> {
        self.0.take().expect("spawn outcome consumed exactly once")
    }
}

impl<P: ProcessPlacement> Drop for SpawnOutcome<P> {
    fn drop(&mut self) {
        if let Some(Ok((mut session, placement))) = self.0.take() {
            let _ = session.signal_process_group(PtySignal::Kill);
            let _ = session.wait();
            placement.release();
        }
    }
}

async fn release_placement<P: ProcessPlacement>(placement: P) -> Result<(), LiveError> {
    tokio::task::spawn_blocking(move || placement.release()).await?;
    Ok(())
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
        Self::spawn_inner(
            splint_id,
            ProcessIncarnation::allocate(),
            backend,
            command,
            config,
            false,
            |_| Ok(()),
        )
        .await
    }

    /// Spawns a runtime with default-off compact-publication ownership metrics
    /// enabled for an explicit benchmark or diagnostic run.
    pub async fn spawn_with_publication_memory_metrics(
        splint_id: SplintId,
        backend: LinuxPtyBackend,
        command: PtyCommand,
        config: LiveSplintConfig,
    ) -> Result<Self, LiveError> {
        Self::spawn_inner(
            splint_id,
            ProcessIncarnation::allocate(),
            backend,
            command,
            config,
            true,
            |_| Ok(()),
        )
        .await
    }

    pub async fn spawn_with_placement<P>(
        splint_id: SplintId,
        incarnation: ProcessIncarnation,
        backend: LinuxPtyBackend,
        command: PtyCommand,
        config: LiveSplintConfig,
        place: impl FnOnce(LinuxPtyIdentity) -> io::Result<P> + Send + 'static,
    ) -> Result<Self, LiveError>
    where
        P: ProcessPlacement,
    {
        Self::spawn_inner(
            splint_id,
            incarnation,
            backend,
            command,
            config,
            false,
            place,
        )
        .await
    }

    async fn spawn_inner<P>(
        splint_id: SplintId,
        incarnation: ProcessIncarnation,
        backend: LinuxPtyBackend,
        command: PtyCommand,
        config: LiveSplintConfig,
        publication_memory_metrics: bool,
        place: impl FnOnce(LinuxPtyIdentity) -> io::Result<P> + Send + 'static,
    ) -> Result<Self, LiveError>
    where
        P: ProcessPlacement,
    {
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
        let mut spawned = tokio::task::spawn_blocking(move || {
            SpawnOutcome::new(backend.spawn_with_placement(&command, size, place))
        })
        .await?;
        let (session, placement) = spawned.take()?;
        let reader = match session.try_clone_reader() {
            Ok(reader) => reader,
            Err(error) => {
                cleanup_failed_spawn(session).await;
                release_placement(placement).await?;
                return Err(error.into());
            }
        };
        let io = match AsyncFd::new(reader) {
            Ok(io) => io,
            Err(error) => {
                cleanup_failed_spawn(session).await;
                release_placement(placement).await?;
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
            placement,
        ))
    }

    fn from_session<P: ProcessPlacement>(
        splint_id: SplintId,
        incarnation: ProcessIncarnation,
        session: LinuxPtySession,
        io: AsyncFd<std::fs::File>,
        config: LiveSplintConfig,
        publication_memory_metrics: bool,
        placement: P,
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
        let task = tokio::spawn(async move {
            let result = Box::pin(run_actor(
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
            ))
            .await;
            release_placement(placement).await?;
            result
        });
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

fn restore_actor_io(session: &LinuxPtySession) -> Result<AsyncFd<std::fs::File>, LiveError> {
    Ok(AsyncFd::new(session.try_clone_reader()?)?)
}

#[allow(
    clippy::too_many_arguments,
    reason = "the actor exclusively owns its runtime state"
)]
async fn run_actor(
    splint_id: SplintId,
    incarnation: ProcessIncarnation,
    session: LinuxPtySession,
    io: AsyncFd<std::fs::File>,
    terminal: Terminal,
    commands: mpsc::Receiver<Command>,
    config: LiveSplintConfig,
    publication_memory_metrics: bool,
    metrics: Arc<RuntimeMetrics>,
    exit_sender: watch::Sender<Option<ProcessExit>>,
) -> Result<ProcessExit, LiveError> {
    let mut session = Some(session);
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
        if let Some(session) = session.as_mut() {
            force_reap(session).await
        } else {
            None
        }
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
    session: &mut Option<LinuxPtySession>,
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
    let mut io = Some(io);
    let mut pending_handoff = None::<Reply<PreparedPtyHandoff>>;

    loop {
        if pending_handoff
            .as_ref()
            .is_some_and(oneshot::Sender::is_closed)
        {
            pending_handoff = None;
        }
        if pending_handoff.is_some() && user_writes.is_empty() && reply_writes.is_empty() {
            let reply = pending_handoff
                .take()
                .expect("pending PTY handoff checked as present");
            #[cfg(test)]
            let retired_reader_raw_fd = io
                .as_ref()
                .expect("active actor owns one PTY reader")
                .get_ref()
                .as_raw_fd();
            drop(io.take().expect("active actor owns one PTY reader"));
            let active_session = session
                .take()
                .expect("active actor owns one canonical PTY session");
            let adoptable = match active_session.try_into_adoptable() {
                Ok(adoptable) => adoptable,
                Err((error, active_session)) => {
                    let restored_io = match restore_actor_io(&active_session) {
                        Ok(restored_io) => restored_io,
                        Err(error) => {
                            *session = Some(active_session);
                            return Err(error);
                        }
                    };
                    *session = Some(active_session);
                    io = Some(restored_io);
                    let _ = reply.send(Err(error.into()));
                    continue;
                }
            };
            let identity = adoptable.identity();
            let master_raw_fd = adoptable.master_raw_fd();
            let (resume, resumed) = oneshot::channel();
            let prepared = PreparedPtyHandoff {
                identity,
                master_raw_fd,
                #[cfg(test)]
                retired_reader_raw_fd,
                session: Some(adoptable),
                resume: Some(resume),
            };
            if let Err(returned) = reply.send(Ok(prepared))
                && let Ok(prepared) = returned
            {
                drop(prepared);
            }
            let PtyResume {
                session: adoptable,
                acknowledged,
            } = resumed.await.map_err(|_| LiveError::Closed)?;
            let active_session = match adoptable.try_adopt() {
                Ok(active_session) => active_session,
                Err((_error, retained)) => {
                    let status =
                        tokio::task::spawn_blocking(move || retained.kill_child_and_wait())
                            .await??;
                    child_exit = Some(status.into());
                    if let Some(acknowledged) = acknowledged {
                        let _ = acknowledged.send(Err(LiveError::Closed));
                    }
                    break;
                }
            };
            let restored_io = match restore_actor_io(&active_session) {
                Ok(restored_io) => restored_io,
                Err(error) => {
                    *session = Some(active_session);
                    if let Some(acknowledged) = acknowledged {
                        let _ = acknowledged.send(Err(LiveError::Closed));
                    }
                    return Err(error);
                }
            };
            *session = Some(active_session);
            io = Some(restored_io);
            if let Some(acknowledged) = acknowledged {
                let _ = acknowledged.send(Ok(()));
            }
            continue;
        }

        let shutdown_settled = shutdown.is_none() || matches!(shutdown, Some(ShutdownStage::Kill));
        if child_exit.is_some()
            && (eof
                || (shutdown_settled
                    && drain_deadline.is_some_and(|deadline| Instant::now() >= deadline)))
        {
            break;
        }

        tokio::select! {
            () = async {
                pending_handoff
                    .as_mut()
                    .expect("pending handoff branch requires a sender")
                    .closed()
                    .await;
            }, if pending_handoff.is_some() => {
                pending_handoff = None;
            }
            command = commands.recv(), if commands_open && pending_handoff.is_none() => {
                if let Some(Command::PreparePtyHandoff(reply)) = command {
                    pending_handoff = Some(reply);
                } else if let Some(command) = command {
                    handle_command(
                        command,
                        splint_id,
                        incarnation,
                        session.as_mut().expect("active actor owns its PTY session"),
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
                        let _ = session
                            .as_ref()
                            .expect("active actor owns its PTY session")
                            .signal_process_group(PtySignal::Hangup);
                        shutdown = Some(ShutdownStage::Hangup(Instant::now() + config.hangup_grace));
                    }
                }
            }
            ready = io.as_ref().expect("active actor owns one PTY reader").readable(), if !eof => {
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
            ready = io.as_ref().expect("active actor owns one PTY reader").writable(), if !reply_writes.is_empty() || !user_writes.is_empty() => {
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
                let session = session
                    .as_mut()
                    .expect("active actor owns its PTY session");
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
            let snapshot_rows = max_rows.min(config.max_scrollback_snapshot_rows);
            let accounting = Arc::new(QueueAccounting::new(
                publication_memory_metrics,
                Arc::clone(metrics),
            ));
            let Some(materialization) = CompactMaterializationState::from_snapshot(
                compact_snapshot_with_history(
                    splint_id,
                    incarnation,
                    terminal,
                    snapshot_rows,
                    child_exit,
                    CompactHistoryPolicy::FullHistory,
                ),
                snapshot_rows,
                &accounting,
            ) else {
                let _ = reply.send(Err(LiveError::PublicationMemoryFull));
                return;
            };
            let (event_sender, events) = mpsc::channel(event_capacity);
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
                snapshot_rows,
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
                materialization: Box::new(materialization),
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
            let snapshot_rows = max_rows.min(config.max_scrollback_snapshot_rows);
            let started = Instant::now();
            let snapshot =
                owned_snapshot(splint_id, incarnation, terminal, snapshot_rows, child_exit);
            let accounting = Arc::new(QueueAccounting::new(
                publication_memory_metrics,
                Arc::clone(metrics),
            ));
            let Some(materialization) = CompactMaterializationState::from_snapshot(
                compact_snapshot_with_history(
                    splint_id,
                    incarnation,
                    terminal,
                    snapshot_rows,
                    child_exit,
                    CompactHistoryPolicy::FullHistory,
                ),
                snapshot_rows,
                &accounting,
            ) else {
                let _ = reply.send(Err(LiveError::PublicationMemoryFull));
                return;
            };
            metrics.snapshot_builds.fetch_add(1, Ordering::Relaxed);
            metrics.snapshot_build_ns.fetch_add(
                u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX),
                Ordering::Relaxed,
            );
            let (event_sender, events) = mpsc::channel(event_capacity);
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
                snapshot_rows,
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
                materialization: Box::new(materialization),
            };
            let _ = reply.send(Ok((snapshot, subscription)));
        }
        Command::PreparePtyHandoff(_) => {
            unreachable!("PTY handoff is intercepted by the actor loop")
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
        let snapshot_rows = subscriber.snapshot_rows;
        let history_policy =
            compact_history_policy(&updates, terminal_dimensions, terminal_active_screen);
        let previous_history_generation = subscriber.published_history_generation;
        let snapshot_history_policy = if history_policy != CompactHistoryPolicy::FullHistory
            && terminal_history_generation != previous_history_generation
        {
            CompactHistoryPolicy::FullHistory
        } else {
            history_policy
        };
        let compact_build_bound = compact_snapshot_capture_build_bound(
            terminal,
            snapshot_rows,
            snapshot_history_policy,
            &updates,
        )
        .unwrap_or(u64::MAX);
        let admitted = match &subscriber.events {
            SubscriberEvents::Legacy(sender) => {
                // One internal slot is reserved for the terminal Exited event.
                if sender.capacity() <= 1 {
                    subscriber.resnapshot.send_replace(true);
                    overflows = overflows.saturating_add(1);
                    return false;
                }
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
                snapshot_rows,
                incarnation,
                updates,
                terminal.revision(),
                snapshot_history_policy,
                compact_build_bound,
                metrics,
                |policy| {
                    compact_snapshot_with_history(
                        splint_id,
                        incarnation,
                        terminal,
                        snapshot_rows,
                        child_exit,
                        policy,
                    )
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

fn compact_snapshot_requested_rows(max_rows: usize, history_policy: CompactHistoryPolicy) -> usize {
    match history_policy {
        CompactHistoryPolicy::FullHistory => max_rows,
        CompactHistoryPolicy::NoHistory => usize::from(max_rows > 0),
        CompactHistoryPolicy::AppendTail(rows) => rows.min(max_rows),
    }
}

fn compact_snapshot_capture_build_bound(
    terminal: &Terminal,
    max_rows: usize,
    history_policy: CompactHistoryPolicy,
    updates: &[TerminalUpdate],
) -> Option<u64> {
    let snapshot = terminal.snapshot(SnapshotRequest {
        max_scrollback_rows: compact_snapshot_requested_rows(max_rows, history_policy),
    });
    let visible_rows = snapshot.dimensions().rows;
    let history_rows = if history_policy == CompactHistoryPolicy::NoHistory {
        0
    } else {
        snapshot.scrollback().returned_rows
    };
    let mut nested_rows = 0_usize;
    for row in snapshot.visible_rows().chain(snapshot.scrollback_rows()) {
        let cells = row.cells();
        checked_owned_bytes(&mut nested_rows, cells.len(), size_of::<CompactLiveCell>())?;
        for cell in cells {
            if let CellSnapshotContent::Composed(characters) = cell.content() {
                nested_rows = characters
                    .iter()
                    .try_fold(nested_rows, |total, character| {
                        total.checked_add(character.len_utf8())
                    })?;
            }
        }
    }
    let title_bytes = snapshot.title().len();
    let image_contents = snapshot.image_contents().len();
    let image_placements = snapshot.image_placements().len();

    let mut snapshot_bytes =
        size_of::<CompactLiveSnapshot>().checked_add(size_of::<LiveSnapshot>())?;
    checked_owned_bytes(
        &mut snapshot_bytes,
        visible_rows.checked_add(history_rows)?,
        size_of::<CompactLiveRow>(),
    )?;
    snapshot_bytes = snapshot_bytes
        .checked_add(nested_rows)?
        .checked_add(title_bytes)?;
    checked_owned_bytes(
        &mut snapshot_bytes,
        image_contents,
        size_of::<ImageContentMetadata>(),
    )?;
    checked_owned_bytes(
        &mut snapshot_bytes,
        image_placements,
        size_of::<ImagePlacement>(),
    )?;

    let mut capture_bytes =
        size_of::<SparsePublicationFrame>().checked_add(size_of::<LiveSnapshot>())?;
    checked_owned_bytes(
        &mut capture_bytes,
        updates.len().max(1),
        size_of::<TerminalUpdate>(),
    )?;
    for update in updates {
        capture_bytes = capture_bytes.checked_add(update.owned_allocation_bytes()?)?;
    }
    checked_owned_bytes(
        &mut capture_bytes,
        visible_rows,
        size_of::<usize>().checked_add(size_of::<SparseRowPatch>())?,
    )?;
    checked_owned_bytes(
        &mut capture_bytes,
        history_rows,
        size_of::<CompactLiveRow>(),
    )?;
    capture_bytes = capture_bytes
        .checked_add(nested_rows)?
        .checked_add(title_bytes)?;
    checked_owned_bytes(
        &mut capture_bytes,
        image_contents,
        size_of::<ImageContentMetadata>(),
    )?;
    checked_owned_bytes(
        &mut capture_bytes,
        image_placements,
        size_of::<ImagePlacement>(),
    )?;
    u64::try_from(snapshot_bytes.checked_add(capture_bytes)?).ok()
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
    let requested_rows = compact_snapshot_requested_rows(max_rows, history_policy);
    let snapshot = terminal.snapshot(SnapshotRequest {
        max_scrollback_rows: requested_rows,
    });
    let mut visible_rows = Vec::with_capacity(snapshot.dimensions().rows);
    visible_rows.extend(snapshot.visible_rows().map(compact_row));
    let mut scrollback_rows = if history_policy == CompactHistoryPolicy::NoHistory {
        Vec::new()
    } else {
        Vec::with_capacity(snapshot.scrollback().returned_rows)
    };
    if history_policy != CompactHistoryPolicy::NoHistory {
        scrollback_rows.extend(snapshot.scrollback_rows().map(compact_row));
    }
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
    let mut title = String::with_capacity(snapshot.title().len());
    title.push_str(snapshot.title());
    let mut image_contents_source = snapshot.image_contents();
    let mut image_contents = Vec::with_capacity(image_contents_source.len());
    image_contents.extend(image_contents_source.by_ref());
    let mut image_placements_source = snapshot.image_placements();
    let mut image_placements = Vec::with_capacity(image_placements_source.len());
    image_placements.extend(image_placements_source.by_ref());
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
        title,
        palette: *snapshot.palette(),
        default_colors: *snapshot.default_colors(),
        image_contents,
        image_placements,
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
    let mut source_cells = row.cells();
    let mut cells = Vec::with_capacity(source_cells.len());
    cells.extend(source_cells.by_ref().map(|cell| CompactLiveCell {
        content: match cell.content() {
            CellSnapshotContent::Empty => CompactCellContent::Empty,
            CellSnapshotContent::Scalar(character) => CompactCellContent::Scalar(character),
            CellSnapshotContent::Composed(characters) => {
                let required = characters
                    .iter()
                    .map(|character| character.len_utf8())
                    .sum();
                let mut content = String::with_capacity(required);
                content.extend(characters.iter());
                CompactCellContent::Composed(content)
            }
            CellSnapshotContent::Spacer { remaining } => CompactCellContent::Spacer { remaining },
        },
        attributes: cell.attributes(),
    }));
    CompactLiveRow {
        row_id: row.id(),
        linebreak: row.linebreak(),
        cells,
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
        test_subscriber_with_limits(
            terminal,
            capacity,
            enabled,
            metrics,
            snapshot_rows,
            SUBSCRIBER_SPARSE_SEMANTIC_BYTES,
        )
    }

    fn test_subscriber_with_limits(
        terminal: &Terminal,
        capacity: usize,
        enabled: bool,
        metrics: Arc<RuntimeMetrics>,
        snapshot_rows: usize,
        semantic_byte_limit: u64,
    ) -> (Subscriber, CompactSubscription, watch::Receiver<bool>) {
        let (events, receiver) = mpsc::channel(capacity);
        let materialization_snapshot = compact_snapshot(
            SplintId::new(),
            ProcessIncarnation::allocate(),
            terminal,
            snapshot_rows,
            None,
        );
        let base_bytes = compact_materialization_semantic_bytes(
            &materialization_snapshot.visible_rows,
            materialization_snapshot.visible_rows.capacity(),
        )
        .unwrap();
        let accounting = Arc::new(QueueAccounting::with_semantic_byte_limit(
            enabled,
            metrics,
            base_bytes.checked_add(semantic_byte_limit).unwrap(),
        ));
        let snapshot_slot = Arc::new(CompactSnapshotSlot::default());
        let mut materialization = CompactMaterializationState::from_snapshot(
            materialization_snapshot,
            snapshot_rows,
            &accounting,
        )
        .unwrap();
        materialization.incarnation = None;
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
                materialization: Box::new(materialization),
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
            "the sparse tail must materialize exactly once"
        );
        assert_eq!(metrics.snapshot().subscriber_queue_events_current, 0);
        assert_eq!(metrics.snapshot().queued_snapshot_events_current, 0);
        assert_eq!(metrics.snapshot().queued_snapshot_events_high_water, 0);
    }

    #[test]
    fn compact_snapshot_build_is_admitted_before_capture_allocation() {
        let incarnation = ProcessIncarnation::allocate();
        let splint_id = SplintId::new();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        let base_revision = terminal.revision();
        terminal.advance(b"A");
        let updates = terminal
            .updates_since(base_revision)
            .unwrap()
            .into_updates();
        let bound = compact_snapshot_capture_build_bound(
            &terminal,
            0,
            CompactHistoryPolicy::FullHistory,
            &updates,
        )
        .unwrap();
        let metrics = Arc::new(RuntimeMetrics::default());
        let denied = Arc::new(QueueAccounting::with_semantic_byte_limit(
            true,
            Arc::clone(&metrics),
            bound,
        ));
        let saturation =
            ProducerBuildLease::try_new(&denied, SPLINT_SPARSE_SEMANTIC_BYTES - (bound - 1))
                .unwrap();
        let (sender, _receiver) = mpsc::channel(4);
        let slot = Arc::new(CompactSnapshotSlot::default());
        assert_eq!(
            publish_compact_update(
                &sender,
                &denied,
                &slot,
                3,
                0,
                incarnation,
                updates.clone(),
                terminal.revision(),
                CompactHistoryPolicy::FullHistory,
                bound,
                &metrics,
                |_| panic!("snapshot construction ran before admission"),
            ),
            CompactPublishOutcome::Full
        );
        assert_eq!(denied.local_semantic_bytes.load(Ordering::Acquire), 0);
        drop(saturation);
        assert_eq!(
            metrics
                .sparse_semantic_bytes_current
                .load(Ordering::Acquire),
            0
        );

        let admitted = Arc::new(QueueAccounting::with_semantic_byte_limit(
            true, metrics, bound,
        ));
        assert_eq!(
            publish_compact_update(
                &sender,
                &admitted,
                &slot,
                3,
                0,
                incarnation,
                updates,
                terminal.revision(),
                CompactHistoryPolicy::FullHistory,
                bound,
                &Arc::clone(&admitted.metrics),
                |_| compact_snapshot(splint_id, incarnation, &terminal, 0, None),
            ),
            CompactPublishOutcome::Published
        );
        let retained = admitted.local_semantic_bytes.load(Ordering::Acquire);
        assert!(retained > 0);
        assert!(retained < bound);
        slot.clear();
        assert_eq!(admitted.local_semantic_bytes.load(Ordering::Acquire), 0);
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
        let first_build_bound = compact_snapshot_capture_build_bound(
            &terminal,
            0,
            CompactHistoryPolicy::FullHistory,
            &first_updates,
        )
        .unwrap();
        assert_eq!(
            publish_compact_update(
                &sender,
                &accounting,
                &snapshot_slot,
                semantic_capacity,
                0,
                incarnation,
                first_updates,
                first_revision,
                CompactHistoryPolicy::FullHistory,
                first_build_bound,
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
        let second_build_bound = compact_snapshot_capture_build_bound(
            &terminal,
            0,
            CompactHistoryPolicy::FullHistory,
            &second_updates,
        )
        .unwrap();
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
                0,
                incarnation,
                second_updates,
                second_revision,
                CompactHistoryPolicy::FullHistory,
                second_build_bound,
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
        } = snapshot_slot.take_pending(&mut subscription.materialization)
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
        assert_eq!(metrics.snapshot().queued_snapshot_events_high_water, 0);
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
        assert_eq!(metrics.snapshot().queued_snapshot_events_high_water, 0);
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
    async fn delayed_compact_subscriber_retains_sparse_frames_without_snapshot_slot() {
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
        assert_eq!(retained.queued_snapshot_events_current, 0);
        assert_eq!(retained.queued_snapshot_events_high_water, 0);
        assert!(retained.queued_compact_semantic_bytes_current > 0);
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
        assert_eq!(metrics.snapshot().queued_snapshot_events_high_water, 0);
    }

    #[test]
    fn sparse_tail_merge_and_receiver_drop_release_exact_ownership() {
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
            assert_eq!(retained.queued_snapshot_events_current, 0);
            assert_eq!(retained.queued_snapshot_events_high_water, 0);
            assert_eq!(retained.queued_snapshot_cells_current, 0);
            assert!(retained.queued_compact_semantic_bytes_current > 0);
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
        assert_eq!(observed.publication_snapshot_enqueues, 0);
        assert_eq!(observed.subscriber_queue_events_high_water, 1);
        assert_eq!(observed.queued_snapshot_events_high_water, 0);
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

    fn sparse_test_history_policy(
        terminal: &Terminal,
        updates: &[TerminalUpdate],
    ) -> CompactHistoryPolicy {
        let snapshot = terminal.snapshot(SnapshotRequest {
            max_scrollback_rows: 0,
        });
        compact_history_policy(updates, snapshot.dimensions(), snapshot.active_screen())
    }

    fn expected_materialized_sparse_state(
        mut delta: CompactLiveSnapshot,
        full: &CompactLiveSnapshot,
    ) -> CompactLiveSnapshot {
        delta.visible_rows.clone_from(&full.visible_rows);
        delta.scrollback_rows.clone_from(&full.scrollback_rows);
        delta
    }

    fn prepare_sparse_test_transition(
        splint_id: SplintId,
        incarnation: ProcessIncarnation,
        terminal: &Terminal,
        base_revision: TerminalRevision,
        history_limit: usize,
    ) -> (
        SparsePublicationCapture,
        CompactLiveSnapshot,
        CompactLiveSnapshot,
    ) {
        let updates = terminal
            .updates_since(base_revision)
            .unwrap()
            .into_updates();
        let policy = sparse_test_history_policy(terminal, &updates);
        let delta = compact_snapshot_with_history(
            splint_id,
            incarnation,
            terminal,
            history_limit,
            None,
            policy,
        );
        let full = compact_snapshot_with_history(
            splint_id,
            incarnation,
            terminal,
            history_limit,
            None,
            CompactHistoryPolicy::FullHistory,
        );
        let capture = SparsePublicationCapture::prepare(
            incarnation,
            updates,
            terminal.revision(),
            policy,
            history_limit,
            &delta,
        )
        .expect("valid sparse test capture");
        let expected = expected_materialized_sparse_state(delta.clone(), &full);
        (capture, delta, expected)
    }

    fn capture_sparse_test_transition(
        splint_id: SplintId,
        incarnation: ProcessIncarnation,
        terminal: &Terminal,
        base: &CompactLiveSnapshot,
        base_revision: TerminalRevision,
        history_limit: usize,
    ) -> (SparsePublicationFrame, CompactLiveSnapshot) {
        let updates = terminal
            .updates_since(base_revision)
            .unwrap()
            .into_updates();
        let policy = sparse_test_history_policy(terminal, &updates);
        let delta = compact_snapshot_with_history(
            splint_id,
            incarnation,
            terminal,
            history_limit,
            None,
            policy,
        );
        let full = compact_snapshot_with_history(
            splint_id,
            incarnation,
            terminal,
            history_limit,
            None,
            CompactHistoryPolicy::FullHistory,
        );
        let frame = SparsePublicationFrame::capture(
            incarnation,
            updates,
            terminal.revision(),
            policy,
            history_limit,
            &delta,
        )
        .expect("valid sparse test transition");
        let reconstructed = frame.apply_to(base).expect("contiguous sparse transition");
        assert_eq!(
            reconstructed,
            expected_materialized_sparse_state(delta, &full)
        );
        (frame, reconstructed)
    }

    #[test]
    fn sparse_frame_reconstructs_damage_selected_rows_without_a_grid_checkpoint() {
        let splint_id = SplintId::new();
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 3, TerminalConfig::default());
        let base = compact_snapshot_with_history(
            splint_id,
            incarnation,
            &terminal,
            16,
            None,
            CompactHistoryPolicy::FullHistory,
        );
        let base_revision = terminal.revision();

        terminal.advance(b"ABC");
        let updates = terminal
            .updates_since(base_revision)
            .unwrap()
            .into_updates();
        let policy = sparse_test_history_policy(&terminal, &updates);
        let delta =
            compact_snapshot_with_history(splint_id, incarnation, &terminal, 16, None, policy);
        let full = compact_snapshot_with_history(
            splint_id,
            incarnation,
            &terminal,
            16,
            None,
            CompactHistoryPolicy::FullHistory,
        );
        let frame = SparsePublicationFrame::capture(
            incarnation,
            updates,
            terminal.revision(),
            policy,
            16,
            &delta,
        )
        .expect("valid sparse frame");

        assert_eq!(frame.visible_rows.len(), 1);
        assert!(frame.visible_rows.len() < full.metadata.dimensions.rows);
        assert!(frame.metadata.visible_rows.is_empty());
        assert!(frame.metadata.scrollback_rows.is_empty());
        assert!(frame.semantic_bytes > 0);
        assert_eq!(
            frame.apply_to(&base).expect("contiguous reconstruction"),
            expected_materialized_sparse_state(delta, &full)
        );
    }

    #[test]
    fn sealed_sparse_frame_composes_history_resize_and_final_metadata() {
        let splint_id = SplintId::new();
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 3, TerminalConfig::default());
        let base = compact_snapshot_with_history(
            splint_id,
            incarnation,
            &terminal,
            16,
            None,
            CompactHistoryPolicy::FullHistory,
        );

        let base_revision = terminal.revision();
        terminal.advance(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
        let (mut sealed, _first_state) = capture_sparse_test_transition(
            splint_id,
            incarnation,
            &terminal,
            &base,
            base_revision,
            16,
        );
        let first_revision = terminal.revision();
        terminal.resize(10, 3);
        terminal.advance(b"\x1b]2;sealed-title\x07");
        let (second, second_snapshot, _second_state) =
            prepare_sparse_test_transition(splint_id, incarnation, &terminal, first_revision, 16);
        assert!(sealed.merge_capture(second, &second_snapshot).is_some());
        let second_revision = terminal.revision();
        terminal.advance(b"\r\nsix");
        let (third, third_snapshot, final_state) =
            prepare_sparse_test_transition(splint_id, incarnation, &terminal, second_revision, 16);
        let row_buffers: Vec<_> = sealed
            .visible_rows
            .iter()
            .map(|patch| (patch.index, patch.row.cells.as_ptr()))
            .collect();

        assert!(sealed.merge_capture(third, &third_snapshot).is_some());
        for (index, pointer) in row_buffers {
            let patch = sealed
                .visible_rows
                .iter()
                .find(|patch| patch.index == index)
                .unwrap();
            assert_eq!(patch.row.cells.as_ptr(), pointer);
        }
        assert_eq!(sealed.base_revision, base_revision);
        assert_eq!(sealed.final_revision, terminal.revision());
        assert_eq!(sealed.updates.len(), 3);
        let mut expected = final_state;
        expected.history_policy = CompactHistoryPolicy::FullHistory;
        assert_eq!(sealed.apply_to(&base), Some(expected));
    }

    #[test]
    fn direct_sparse_tail_reuses_bounded_history_and_prevalidation_is_transactional() {
        let splint_id = SplintId::new();
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        let base = compact_snapshot_with_history(
            splint_id,
            incarnation,
            &terminal,
            1,
            None,
            CompactHistoryPolicy::FullHistory,
        );

        let base_revision = terminal.revision();
        terminal.advance(b"one\r\ntwo\r\nthree");
        let (mut sealed, first_state) = capture_sparse_test_transition(
            splint_id,
            incarnation,
            &terminal,
            &base,
            base_revision,
            1,
        );
        let history_pointer = match &sealed.history {
            SparseHistoryDelta::Append { rows, .. } => {
                assert_eq!(rows.len(), 1);
                rows[0].cells.as_ptr()
            }
            history => panic!("expected append history, got {history:?}"),
        };

        let first_revision = terminal.revision();
        terminal.advance(b"\r\nfour");
        let (mut invalid, snapshot, _) =
            prepare_sparse_test_transition(splint_id, incarnation, &terminal, first_revision, 1);
        invalid.base_revision = base_revision;
        let unchanged_revision = sealed.final_revision;
        let unchanged_rows = sealed.visible_rows.clone();
        let unchanged_history = sealed.history.clone();
        let unchanged_metadata = sealed.metadata.clone();
        let unchanged_semantic_bytes = sealed.semantic_bytes;
        assert!(sealed.merge_capture(invalid, &snapshot).is_none());
        assert_eq!(sealed.final_revision, unchanged_revision);
        assert_eq!(sealed.visible_rows, unchanged_rows);
        assert_eq!(sealed.history, unchanged_history);
        assert_eq!(sealed.metadata, unchanged_metadata);
        assert_eq!(sealed.semantic_bytes, unchanged_semantic_bytes);

        let (capture, snapshot, mut expected) =
            prepare_sparse_test_transition(splint_id, incarnation, &terminal, first_revision, 1);
        sealed.updates.shrink_to_fit();
        sealed.visible_rows.shrink_to_fit();
        let admitted_merge_bound = sealed
            .semantic_bytes
            .checked_add(capture.semantic_bytes)
            .unwrap();
        assert!(sealed.merge_capture(capture, &snapshot).is_some());
        assert!(sealed.semantic_bytes <= admitted_merge_bound);
        let rows = match &sealed.history {
            SparseHistoryDelta::Append { rows, .. } => rows,
            history => panic!("expected append history, got {history:?}"),
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].cells.as_ptr(), history_pointer);
        expected.history_policy = first_state.history_policy.merge(expected.history_policy);
        assert_eq!(sealed.apply_to(&base), Some(expected));
    }

    #[test]
    fn sparse_tail_rejects_cross_frame_revision_without_advancing_materialized_state() {
        let splint_id = SplintId::new();
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        let base = compact_snapshot_with_history(
            splint_id,
            incarnation,
            &terminal,
            0,
            None,
            CompactHistoryPolicy::FullHistory,
        );

        let base_revision = terminal.revision();
        terminal.advance(b"A");
        let (first, first_state) = capture_sparse_test_transition(
            splint_id,
            incarnation,
            &terminal,
            &base,
            base_revision,
            0,
        );
        let first_revision = terminal.revision();
        terminal.advance(b"B");
        let (mut second, _) = capture_sparse_test_transition(
            splint_id,
            incarnation,
            &terminal,
            &first_state,
            first_revision,
            0,
        );
        second.base_revision = base_revision;
        let accounting = Arc::new(QueueAccounting::new(
            false,
            Arc::new(RuntimeMetrics::default()),
        ));
        let pending = PendingCompactUpdates {
            incarnation,
            frames: vec![first, second],
            end_revision: terminal.revision(),
            history_policy: CompactHistoryPolicy::NoHistory,
            admissions: Vec::new(),
            semantic_admissions: vec![SemanticByteLease::try_new(&accounting, 0).unwrap()],
            pending_attributions: Vec::new(),
        };
        let mut materialization =
            CompactMaterializationState::from_snapshot(base, 0, &accounting).unwrap();
        let original_revision = materialization.revision;
        let original_rows = materialization.visible_rows.clone();

        assert!(pending.materialize(&mut materialization).is_none());
        assert_eq!(materialization.revision, original_revision);
        assert_eq!(materialization.visible_rows, original_rows);
    }

    #[test]
    fn sparse_frame_semantic_bytes_charge_reserved_update_capacity() {
        let splint_id = SplintId::new();
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        let base_revision = terminal.revision();
        terminal.advance(b"A");
        let update = terminal
            .updates_since(base_revision)
            .unwrap()
            .into_updates()
            .pop()
            .unwrap();
        let mut updates = Vec::with_capacity(32);
        updates.push(update);
        let snapshot = compact_snapshot_with_history(
            splint_id,
            incarnation,
            &terminal,
            0,
            None,
            CompactHistoryPolicy::NoHistory,
        );
        let frame = SparsePublicationFrame::capture(
            incarnation,
            updates,
            terminal.revision(),
            CompactHistoryPolicy::NoHistory,
            0,
            &snapshot,
        )
        .expect("reserved-capacity frame");
        let reserved_update_bytes = 32_usize
            .checked_mul(size_of::<TerminalUpdate>())
            .and_then(|bytes| u64::try_from(bytes).ok())
            .unwrap();

        assert!(frame.semantic_bytes >= reserved_update_bytes);
    }

    #[test]
    fn sparse_frame_reconstructs_ordered_scrolls_and_bounded_history_tail() {
        let splint_id = SplintId::new();
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 3, TerminalConfig::default());
        terminal.advance(b"one\r\ntwo\r\nthree");
        let base = compact_snapshot_with_history(
            splint_id,
            incarnation,
            &terminal,
            4,
            None,
            CompactHistoryPolicy::FullHistory,
        );
        let base_revision = terminal.revision();

        terminal.advance(b"\r\nfour\r\nfive\r\nsix");
        let updates = terminal
            .updates_since(base_revision)
            .unwrap()
            .into_updates();
        let policy = sparse_test_history_policy(&terminal, &updates);
        assert!(matches!(policy, CompactHistoryPolicy::AppendTail(rows) if rows > 0));
        let expected_scrolls: Vec<_> = updates
            .iter()
            .flat_map(TerminalUpdate::damage)
            .filter(|damage| matches!(damage, TerminalDamage::Scroll { .. }))
            .cloned()
            .collect();
        assert!(expected_scrolls.len() >= 2);
        let delta =
            compact_snapshot_with_history(splint_id, incarnation, &terminal, 4, None, policy);
        let full = compact_snapshot_with_history(
            splint_id,
            incarnation,
            &terminal,
            4,
            None,
            CompactHistoryPolicy::FullHistory,
        );
        let frame = SparsePublicationFrame::capture(
            incarnation,
            updates,
            terminal.revision(),
            policy,
            4,
            &delta,
        )
        .expect("valid sparse history frame");

        let retained_scrolls: Vec<_> = frame
            .updates
            .iter()
            .flat_map(TerminalUpdate::damage)
            .filter(|damage| matches!(damage, TerminalDamage::Scroll { .. }))
            .cloned()
            .collect();
        assert_eq!(retained_scrolls, expected_scrolls);
        assert_eq!(
            frame.apply_to(&base).expect("history reconstruction"),
            expected_materialized_sparse_state(delta, &full)
        );
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one reconstruction test covers independent metadata and screen transitions"
    )]
    #[test]
    fn sparse_frames_reconstruct_dimensions_screen_palette_and_images() {
        let splint_id = SplintId::new();
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        terminal.set_cell_pixel_size(8, 16);
        let mut reconstructed = compact_snapshot_with_history(
            splint_id,
            incarnation,
            &terminal,
            8,
            None,
            CompactHistoryPolicy::FullHistory,
        );
        let default_palette_entry = reconstructed.metadata.palette[1];
        let default_modes = reconstructed.metadata.modes;
        let default_cursor = reconstructed.metadata.cursor;

        for (transition_index, transition) in [
            b"\x1bPq#1;2;100;0;0#1~\x1b\\".as_slice(),
            b"\x1b]2;sparse\x07\x1b]4;1;#abc\x07\x1b[?25l\x1b[2;3H".as_slice(),
            b"\x1b[?1049h".as_slice(),
        ]
        .into_iter()
        .enumerate()
        {
            let base_revision = terminal.revision();
            terminal.advance(transition);
            let updates = terminal
                .updates_since(base_revision)
                .unwrap()
                .into_updates();
            let policy = sparse_test_history_policy(&terminal, &updates);
            let delta =
                compact_snapshot_with_history(splint_id, incarnation, &terminal, 8, None, policy);
            let full = compact_snapshot_with_history(
                splint_id,
                incarnation,
                &terminal,
                8,
                None,
                CompactHistoryPolicy::FullHistory,
            );
            let frame = SparsePublicationFrame::capture(
                incarnation,
                updates.clone(),
                terminal.revision(),
                policy,
                8,
                &delta,
            )
            .unwrap_or_else(|| panic!("invalid metadata frame: {updates:?}, policy={policy:?}"));
            reconstructed = frame
                .apply_to(&reconstructed)
                .expect("metadata reconstruction");
            assert_eq!(
                reconstructed,
                expected_materialized_sparse_state(delta, &full)
            );
            if transition_index == 0 {
                assert_eq!(reconstructed.metadata.image_contents.len(), 1);
                assert_eq!(reconstructed.metadata.image_placements.len(), 1);
            } else if transition_index == 1 {
                assert_eq!(reconstructed.metadata.title, "sparse");
                assert_ne!(reconstructed.metadata.palette[1], default_palette_entry);
                assert_ne!(reconstructed.metadata.modes, default_modes);
                assert_ne!(reconstructed.metadata.cursor, default_cursor);
            }
        }
        assert_eq!(
            reconstructed.metadata.active_screen,
            ActiveScreen::Alternate
        );

        let base_revision = terminal.revision();
        terminal.resize(10, 3);
        let updates = terminal
            .updates_since(base_revision)
            .unwrap()
            .into_updates();
        let policy = sparse_test_history_policy(&terminal, &updates);
        let delta =
            compact_snapshot_with_history(splint_id, incarnation, &terminal, 8, None, policy);
        let full = compact_snapshot_with_history(
            splint_id,
            incarnation,
            &terminal,
            8,
            None,
            CompactHistoryPolicy::FullHistory,
        );
        let frame = SparsePublicationFrame::capture(
            incarnation,
            updates,
            terminal.revision(),
            policy,
            8,
            &delta,
        )
        .expect("valid dimensions frame");
        reconstructed = frame
            .apply_to(&reconstructed)
            .expect("dimension reconstruction");
        assert_eq!(
            reconstructed,
            expected_materialized_sparse_state(delta, &full)
        );
        assert_eq!(reconstructed.visible_rows.len(), 3);
        assert_eq!(frame.visible_rows.len(), 3);
    }

    #[tokio::test]
    async fn mailbox_local_append_frames_materialize_exact_history_metadata() {
        let incarnation = ProcessIncarnation::allocate();
        let splint_id = SplintId::new();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        terminal.advance(b"one\r\ntwo");
        let metrics = Arc::new(RuntimeMetrics::default());
        let (subscriber, mut subscription, _) =
            test_subscriber_with_rows(&terminal, 8, true, Arc::clone(&metrics), 4);
        let mut subscribers = vec![subscriber];
        let mut publication = SynchronizedPublication::new(terminal.revision());

        for line in [b"\r\nthree".as_slice(), b"\r\nfour".as_slice()] {
            terminal.advance(line);
            let (updates, overflows) = publish_updates(
                splint_id,
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
        let (received, trailing_exit) = subscription.recv_coalesced().await;
        assert_eq!(trailing_exit, None);
        let SubscriptionReceive::Event(LiveEvent::Update { snapshot, .. }) = received else {
            panic!("mailbox-local append tail did not materialize");
        };
        let authoritative = owned_snapshot(splint_id, incarnation, &terminal, 4, None);

        assert_eq!(snapshot.scrollback_rows, authoritative.scrollback_rows);
        assert_eq!(
            snapshot.scrollback.returned_rows,
            snapshot.scrollback_rows.len()
        );
        assert_eq!(
            snapshot.scrollback.omitted_oldest_rows,
            snapshot
                .scrollback
                .available_rows
                .saturating_sub(snapshot.scrollback_rows.len())
        );
    }

    #[test]
    fn separately_materialized_append_tails_merge_into_one_exact_wire_history() {
        let splint_id = SplintId::new();
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        terminal.advance(b"one\r\ntwo");

        terminal.advance(b"\r\nthree");
        let first = compact_snapshot_with_history(
            splint_id,
            incarnation,
            &terminal,
            4,
            None,
            CompactHistoryPolicy::AppendTail(1),
        );
        terminal.advance(b"\r\nfour");
        let second = compact_snapshot_with_history(
            splint_id,
            incarnation,
            &terminal,
            4,
            None,
            CompactHistoryPolicy::AppendTail(1),
        );
        let full = compact_snapshot_with_history(
            splint_id,
            incarnation,
            &terminal,
            4,
            None,
            CompactHistoryPolicy::FullHistory,
        );
        let mut merged = Box::new(first);
        merge_materialized_snapshots(&mut merged, Box::new(second), 4);

        assert_eq!(merged.history_policy, CompactHistoryPolicy::AppendTail(2));
        assert_eq!(merged.scrollback_rows, full.scrollback_rows);
        assert_eq!(
            merged.metadata.scrollback.returned_rows,
            merged.scrollback_rows.len()
        );
        assert_eq!(
            merged.metadata.scrollback.omitted_oldest_rows,
            merged
                .metadata
                .scrollback
                .available_rows
                .saturating_sub(merged.scrollback_rows.len())
        );
    }

    #[test]
    fn append_deltas_preserve_preexisting_client_history_across_drains() {
        let splint_id = SplintId::new();
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        terminal.advance(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
        let initial = compact_snapshot_with_history(
            splint_id,
            incarnation,
            &terminal,
            4,
            None,
            CompactHistoryPolicy::FullHistory,
        );

        terminal.advance(b"\r\nsix");
        let first_drain = compact_snapshot_with_history(
            splint_id,
            incarnation,
            &terminal,
            4,
            None,
            CompactHistoryPolicy::AppendTail(1),
        );
        terminal.advance(b"\r\nseven");
        let second_drain = compact_snapshot_with_history(
            splint_id,
            incarnation,
            &terminal,
            4,
            None,
            CompactHistoryPolicy::AppendTail(1),
        );
        let authoritative = compact_snapshot_with_history(
            splint_id,
            incarnation,
            &terminal,
            4,
            None,
            CompactHistoryPolicy::FullHistory,
        );

        let mut retained = initial.scrollback_rows;
        retained.extend(first_drain.scrollback_rows);
        retained.extend(second_drain.scrollback_rows);
        let excess = retained.len().saturating_sub(4);
        retained.drain(..excess);
        assert_eq!(retained, authoritative.scrollback_rows);
        assert_eq!(
            authoritative.metadata.scrollback.omitted_oldest_rows,
            authoritative
                .metadata
                .scrollback
                .available_rows
                .saturating_sub(retained.len())
        );
    }

    #[test]
    fn sparse_frames_reconstruct_history_trim_reflow_clear_and_reset() {
        let splint_id = SplintId::new();
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 3, TerminalConfig::default());
        terminal.advance(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix");
        let mut reconstructed = compact_snapshot_with_history(
            splint_id,
            incarnation,
            &terminal,
            4,
            None,
            CompactHistoryPolicy::FullHistory,
        );

        let base_revision = terminal.revision();
        terminal.advance(b"\r\nseven\r\neight\r\nnine");
        let (append, next) = capture_sparse_test_transition(
            splint_id,
            incarnation,
            &terminal,
            &reconstructed,
            base_revision,
            4,
        );
        assert!(matches!(
            append.history,
            SparseHistoryDelta::Append { final_rows: 4, .. }
        ));
        assert_eq!(next.scrollback_rows.len(), 4);
        reconstructed = next;

        let base_revision = terminal.revision();
        terminal.resize(10, 4);
        let (reflow, next) = capture_sparse_test_transition(
            splint_id,
            incarnation,
            &terminal,
            &reconstructed,
            base_revision,
            4,
        );
        assert!(matches!(reflow.history, SparseHistoryDelta::Replace(_)));
        assert_eq!(reflow.visible_rows.len(), 4);
        reconstructed = next;

        let base_revision = terminal.revision();
        terminal.advance(b"\x1b[2J\x1b[H\x1b[3J");
        let (clear, next) = capture_sparse_test_transition(
            splint_id,
            incarnation,
            &terminal,
            &reconstructed,
            base_revision,
            4,
        );
        assert!(matches!(clear.history, SparseHistoryDelta::Replace(_)));
        assert!(next.scrollback_rows.is_empty());
        assert_eq!(clear.visible_rows.len(), next.visible_rows.len());
    }

    #[test]
    fn oversized_sparse_producer_batch_is_checked_and_reconstructable() {
        let splint_id = SplintId::new();
        let incarnation = ProcessIncarnation::allocate();
        let config = TerminalConfig {
            update_history_limit: 100_000,
            ..TerminalConfig::default()
        };
        let mut terminal = Terminal::new(480, 128, config);
        let base = compact_snapshot_with_history(
            splint_id,
            incarnation,
            &terminal,
            256,
            None,
            CompactHistoryPolicy::FullHistory,
        );
        let base_revision = terminal.revision();
        let mut output = Vec::with_capacity(192 * 482);
        for _ in 0..192 {
            output.extend(std::iter::repeat_n(b'X', 480));
            output.extend_from_slice(b"\r\n");
        }
        terminal.advance(&output);
        let (frame, reconstructed) = capture_sparse_test_transition(
            splint_id,
            incarnation,
            &terminal,
            &base,
            base_revision,
            256,
        );

        assert!(frame.semantic_bytes > 256 * 1024);
        assert_eq!(frame.visible_rows.len(), 128);
        assert!(!reconstructed.scrollback_rows.is_empty());
        assert_eq!(
            reconstructed.scrollback_rows.len(),
            reconstructed.metadata.scrollback.available_rows.min(256)
        );
        assert!(frame.metadata.visible_rows.is_empty());
        assert!(frame.metadata.scrollback_rows.is_empty());
    }

    #[test]
    fn empty_sparse_tail_requires_resnapshot() {
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
        let accounting = Arc::new(QueueAccounting::new(
            false,
            Arc::new(RuntimeMetrics::default()),
        ));
        {
            let mut current = slot.lock();
            current.pending.push_back(PendingCompactUpdates {
                incarnation,
                frames: Vec::new(),
                end_revision: terminal.revision(),
                history_policy: CompactHistoryPolicy::AppendTail(1),
                admissions: Vec::new(),
                semantic_admissions: vec![SemanticByteLease::try_new(&accounting, 0).unwrap()],
                pending_attributions: Vec::new(),
            });
        }
        let mut materialization =
            CompactMaterializationState::from_snapshot(snapshot, 0, &accounting).unwrap();
        assert!(matches!(
            slot.take_pending(&mut materialization),
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
    fn publication_attribution_distinguishes_ephemeral_build_sparse_queue_and_materialization() {
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
        assert_eq!(queued.publication_snapshot_enqueues, 0);
        assert_eq!(queued.publication_snapshot_rows, 2);
        assert_eq!(queued.publication_snapshot_cells, 16);
        assert_eq!(queued.publication_snapshot_enqueued_rows, 0);
        assert_eq!(queued.publication_snapshot_enqueued_cells, 0);
        assert_eq!(queued.subscriber_queue_events_current, 1);
        assert_eq!(queued.subscriber_queue_events_high_water, 1);
        assert_eq!(queued.subscriber_queue_per_subscriber_high_water, 1);
        assert_eq!(queued.queued_snapshot_events_current, 0);
        assert_eq!(queued.queued_snapshot_cells_current, 0);
        assert_eq!(queued.queued_snapshot_cells_high_water, 0);
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
        assert_eq!(drained.publication_snapshot_enqueues, 0);
        assert_eq!(drained.queued_compact_batches_current, 0);
        assert_eq!(drained.queued_compact_terminal_updates_current, 0);
        assert_eq!(drained.queued_compact_scrolls_current, 0);
        assert_eq!(drained.queued_compact_appended_rows_current, 0);
        assert_eq!(drained.queued_compact_semantic_bytes_current, 0);
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
    fn semantic_byte_admission_enforces_exact_local_and_splint_boundaries() {
        let metrics = Arc::new(RuntimeMetrics::default());
        let local = Arc::new(QueueAccounting::with_semantic_byte_limit(
            false,
            Arc::clone(&metrics),
            10,
        ));
        let exact = SemanticByteLease::try_new(&local, 10).unwrap();
        assert!(SemanticByteLease::try_new(&local, 1).is_none());
        assert_eq!(local.local_semantic_bytes.load(Ordering::Acquire), 10);
        drop(exact);
        assert_eq!(local.local_semantic_bytes.load(Ordering::Acquire), 0);

        let first = Arc::new(QueueAccounting::new(false, Arc::clone(&metrics)));
        let second = Arc::new(QueueAccounting::with_semantic_byte_limit(
            false,
            Arc::clone(&metrics),
            48 * 1024 * 1024,
        ));
        let first_lease = SemanticByteLease::try_new(&first, 16 * 1024 * 1024).unwrap();
        let second_lease = SemanticByteLease::try_new(&second, 48 * 1024 * 1024).unwrap();
        assert!(SemanticByteLease::try_new(&second, 1).is_none());
        assert_eq!(
            metrics
                .sparse_semantic_bytes_current
                .load(Ordering::Acquire),
            SPLINT_SPARSE_SEMANTIC_BYTES
        );
        drop((first_lease, second_lease));
        assert_eq!(
            metrics
                .sparse_semantic_bytes_current
                .load(Ordering::Acquire),
            0
        );
    }

    #[test]
    fn materialization_base_is_admitted_resized_and_released_exactly() {
        let metrics = Arc::new(RuntimeMetrics::default());
        let terminal = Terminal::new(8, 2, TerminalConfig::default());
        let (_subscriber, mut receiver, _) =
            test_subscriber(&terminal, 4, true, Arc::clone(&metrics));
        let accounting = Arc::clone(&receiver.accounting);
        let initial = accounting.local_semantic_bytes.load(Ordering::Acquire);
        assert!(initial > 0);
        assert_eq!(
            metrics
                .sparse_semantic_bytes_current
                .load(Ordering::Acquire),
            initial
        );

        let mut wider = Terminal::new(64, 4, TerminalConfig::default());
        wider.advance("wide e\u{301} state".as_bytes());
        let snapshot = compact_snapshot(
            SplintId::new(),
            ProcessIncarnation::allocate(),
            &wider,
            0,
            None,
        );
        receiver
            .materialization
            .replace_visible_rows(&snapshot.visible_rows)
            .unwrap();
        let exact = compact_materialization_semantic_bytes(
            &receiver.materialization.visible_rows,
            receiver.materialization.visible_rows.capacity(),
        )
        .unwrap();
        assert_eq!(
            accounting.local_semantic_bytes.load(Ordering::Acquire),
            exact
        );
        assert_eq!(
            metrics
                .sparse_semantic_bytes_current
                .load(Ordering::Acquire),
            exact
        );

        drop(receiver);
        assert_eq!(accounting.local_semantic_bytes.load(Ordering::Acquire), 0);
        assert_eq!(
            metrics
                .sparse_semantic_bytes_current
                .load(Ordering::Acquire),
            0
        );

        let denied_snapshot = compact_snapshot(
            SplintId::new(),
            ProcessIncarnation::allocate(),
            &wider,
            0,
            None,
        );
        let required = compact_materialization_semantic_bytes(
            &denied_snapshot.visible_rows,
            denied_snapshot.visible_rows.capacity(),
        )
        .unwrap();
        let denied = Arc::new(QueueAccounting::with_semantic_byte_limit(
            false,
            Arc::new(RuntimeMetrics::default()),
            required - 1,
        ));
        assert!(CompactMaterializationState::from_snapshot(denied_snapshot, 0, &denied).is_none());
        assert_eq!(denied.local_semantic_bytes.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn pre_materialization_admission_failure_clears_sparse_ownership() {
        const ISOLATED_ENV: &str = "SPLINTERM_TEST_ISOLATED_PUBLICATION_ADMISSION";
        if std::env::var_os(ISOLATED_ENV).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "live::tests::pre_materialization_admission_failure_clears_sparse_ownership",
                    "--test-threads=1",
                ])
                .env(ISOLATED_ENV, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "isolated daemon-admission test failed:\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            return;
        }

        // This child process runs only the exact test above, so its process-wide
        // daemon publication counter cannot race leases owned by parallel tests.
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        let metrics = Arc::new(RuntimeMetrics::default());
        let (subscriber, mut receiver, resnapshot) =
            test_subscriber(&terminal, 4, true, Arc::clone(&metrics));
        let accounting = Arc::clone(&receiver.accounting);
        let materialization_bytes = accounting.local_semantic_bytes.load(Ordering::Acquire);
        assert!(materialization_bytes > 0);
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
        let already_admitted = DAEMON_TERMINAL_PUBLICATION_BYTES_CURRENT.load(Ordering::Acquire);
        let saturation = TerminalPublicationMemoryLease::try_new(
            usize::try_from(DAEMON_TERMINAL_PUBLICATION_BYTES - already_admitted).unwrap(),
        )
        .unwrap();

        let (delivery, trailing_exit, admission) = receiver
            .recv_coalesced_with_publication_admission(32 * 1024 * 1024)
            .await;
        assert!(matches!(delivery, SubscriptionReceive::ResnapshotRequired));
        assert_eq!(trailing_exit, None);
        assert!(admission.is_none());
        assert_eq!(
            accounting.local_semantic_bytes.load(Ordering::Acquire),
            materialization_bytes
        );
        assert_eq!(metrics.snapshot().queued_compact_semantic_bytes_current, 0);
        assert!(!*resnapshot.borrow());
        drop(saturation);
        assert_eq!(
            DAEMON_TERMINAL_PUBLICATION_BYTES_CURRENT.load(Ordering::Acquire),
            materialization_bytes
        );
        drop(receiver);
        assert_eq!(
            DAEMON_TERMINAL_PUBLICATION_BYTES_CURRENT.load(Ordering::Acquire),
            0
        );
    }

    #[tokio::test]
    async fn sparse_semantic_overflow_clears_sealed_tail_before_resync() {
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(8, 2, TerminalConfig::default());
        let metrics = Arc::new(RuntimeMetrics::default());
        let (subscriber, mut receiver, resnapshot) =
            test_subscriber_with_limits(&terminal, 4, true, Arc::clone(&metrics), 0, 1);
        let accounting = Arc::clone(&receiver.accounting);
        let materialization_bytes = accounting.local_semantic_bytes.load(Ordering::Acquire);
        assert!(materialization_bytes > 0);
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
            (1, 1)
        );
        assert!(subscribers.is_empty());
        assert!(*resnapshot.borrow());
        assert_eq!(
            accounting.local_semantic_bytes.load(Ordering::Acquire),
            materialization_bytes
        );
        assert_eq!(
            metrics
                .sparse_semantic_bytes_current
                .load(Ordering::Acquire),
            materialization_bytes
        );
        assert_eq!(metrics.snapshot().queued_compact_semantic_bytes_current, 0);
        assert!(matches!(
            receiver.recv_coalesced().await.0,
            SubscriptionReceive::ResnapshotRequired
        ));
        drop(receiver);
        assert_eq!(
            metrics
                .sparse_semantic_bytes_current
                .load(Ordering::Acquire),
            0
        );
    }

    #[tokio::test]
    async fn sparse_reusable_tail_keeps_one_wake_and_one_materialization() {
        let incarnation = ProcessIncarnation::allocate();
        let mut terminal = Terminal::new(16, 2, TerminalConfig::default());
        let metrics = Arc::new(RuntimeMetrics::default());
        let (subscriber, mut receiver, resnapshot) =
            test_subscriber(&terminal, 17, true, Arc::clone(&metrics));
        let accounting = Arc::clone(&receiver.accounting);
        let materialization_bytes = accounting.local_semantic_bytes.load(Ordering::Acquire);
        let slot = match &subscriber.events {
            SubscriberEvents::Compact { snapshot_slot, .. } => Arc::clone(snapshot_slot),
            SubscriberEvents::Legacy(_) => unreachable!(),
        };
        let mut subscribers = vec![subscriber];
        let mut publication = SynchronizedPublication::new(terminal.revision());

        for byte in *b"ABCDEFGHI" {
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

        {
            let mailbox = slot.lock();
            assert_eq!(mailbox.pending.len(), 1);
            assert_eq!(mailbox.pending[0].frames.len(), 1);
            assert_eq!(mailbox.pending[0].admissions.len(), 9);
            let aggregate = &mailbox.pending[0].frames[0];
            assert_eq!(
                accounting.local_semantic_bytes.load(Ordering::Acquire),
                materialization_bytes + aggregate.semantic_bytes
            );
        }
        assert_eq!(receiver.events.len(), 1);
        assert!(!*resnapshot.borrow());

        let (delivery, trailing_exit) = receiver.recv_coalesced().await;
        assert_eq!(trailing_exit, None);
        let SubscriptionReceive::Event(LiveEvent::Update { snapshot, .. }) = delivery else {
            panic!("sealed sparse mailbox did not materialize");
        };
        assert_eq!(snapshot.revision, terminal.revision());
        assert!(snapshot_text(&snapshot).contains("ABCDEFGHI"));
        let retained_base = compact_materialization_semantic_bytes(
            &receiver.materialization.visible_rows,
            receiver.materialization.visible_rows.capacity(),
        )
        .unwrap();
        assert_eq!(
            accounting.local_semantic_bytes.load(Ordering::Acquire),
            retained_base
        );
        assert_eq!(metrics.snapshot().publication_compact_materializations, 1);
        assert_eq!(metrics.snapshot().queued_compact_batches_current, 0);
    }

    #[tokio::test]
    async fn compact_batch_attribution_tracks_merge_materialize_and_release() {
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
        assert_eq!(queued.queued_compact_terminal_updates_current, 2);
        assert!(queued.queued_compact_semantic_bytes_current > 0);
        assert!(
            queued.queued_compact_semantic_bytes_high_water
                >= queued.queued_compact_semantic_bytes_current
        );

        let SubscriptionReceive::Event(LiveEvent::Update { updates, .. }) =
            receiver.recv_coalesced().await.0
        else {
            panic!("compact summaries did not materialize as one update event");
        };
        assert_eq!(updates.len(), 1);
        let drained = metrics.snapshot();
        assert_eq!(drained.publication_compact_batch_merges, 1);
        assert_eq!(drained.queued_compact_batches_current, 0);
        assert_eq!(drained.queued_compact_terminal_updates_current, 0);
        assert_eq!(drained.queued_compact_semantic_bytes_current, 0);
        assert_eq!(drained.publication_compact_materializations, 1);
        assert_eq!(drained.publication_compact_materialized_batches, 2);
        assert_eq!(
            drained.publication_compact_materialized_batches_high_water,
            2
        );
        assert_eq!(drained.publication_compact_materialized_terminal_updates, 2);
        assert_eq!(
            drained.publication_compact_materialized_semantic_bytes,
            queued.queued_compact_semantic_bytes_current
        );
        assert_eq!(
            drained.publication_compact_materialized_semantic_bytes_high_water,
            queued.queued_compact_semantic_bytes_current
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
        assert_eq!(queued.publication_snapshot_enqueues, 0);
        assert_eq!(queued.subscriber_queue_events_current, 2);
        assert_eq!(queued.subscriber_queue_events_high_water, 2);
        assert_eq!(queued.subscriber_queue_per_subscriber_high_water, 1);
        assert_eq!(queued.queued_snapshot_events_current, 0);
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
        assert_eq!(metrics.snapshot().publication_snapshot_enqueues, 0);
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
            materialization: Box::new(CompactMaterializationState {
                incarnation: None,
                revision: TerminalRevision::default(),
                visible_rows: Vec::new(),
                semantic_admission: SemanticByteLease::try_new(
                    &accounting,
                    compact_materialization_semantic_bytes(&[], 0).unwrap(),
                )
                .unwrap(),
                history_limit: 0,
            }),
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
            accounting: Arc::clone(&accounting),
            snapshot_slot: Arc::new(CompactSnapshotSlot::default()),
            materialization: Box::new(CompactMaterializationState {
                incarnation: None,
                revision: TerminalRevision::default(),
                visible_rows: Vec::new(),
                semantic_admission: SemanticByteLease::try_new(
                    &accounting,
                    compact_materialization_semantic_bytes(&[], 0).unwrap(),
                )
                .unwrap(),
                history_limit: 0,
            }),
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

    struct TestPlacement(Arc<AtomicBool>);

    impl ProcessPlacement for TestPlacement {
        fn release(self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn dropped_spawn_outcome_kills_reaps_and_releases_placement() {
        let session = backend()
            .spawn(
                &shell("trap '' HUP TERM; sleep 30"),
                PtySize {
                    columns: 40,
                    rows: 6,
                    pixel_width: 0,
                    pixel_height: 0,
                },
            )
            .unwrap();
        let child_pid = session.child_id();
        let released = Arc::new(AtomicBool::new(false));
        drop(SpawnOutcome::new(Ok((
            session,
            TestPlacement(Arc::clone(&released)),
        ))));
        assert!(!std::path::Path::new(&format!("/proc/{child_pid}")).exists());
        assert!(released.load(Ordering::SeqCst));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn actor_exit_releases_process_placement() {
        let released = Arc::new(AtomicBool::new(false));
        let placement_released = Arc::clone(&released);
        let runtime = LiveSplintRuntime::spawn_with_placement(
            SplintId::new(),
            ProcessIncarnation::allocate(),
            backend(),
            shell("exit 0"),
            fast_config(),
            move |_| Ok(TestPlacement(placement_released)),
        )
        .await
        .unwrap();

        assert_eq!(runtime.wait().await.unwrap().code, Some(0));
        assert!(released.load(Ordering::SeqCst));
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
    async fn prepared_pty_handoff_fences_commands_and_resumes_one_reader() {
        let runtime = LiveSplintRuntime::spawn(
            SplintId::new(),
            backend(),
            shell(
                "printf ready; read first; printf '<%s>' \"$first\"; read second; printf '<%s>' \"$second\"; sleep 0.2",
            ),
            fast_config(),
        )
        .await
        .unwrap();
        let handle = runtime.handle();
        time::sleep(Duration::from_millis(50)).await;
        assert!(snapshot_text(&handle.snapshot().await.unwrap()).contains("ready"));

        handle.input(b"before-handoff\n".to_vec()).await.unwrap();
        let prepared = handle.prepare_pty_handoff().await.unwrap();
        assert_eq!(prepared.identity().child_pid(), handle.child_pid());
        assert!(PathBuf::from(format!("/proc/self/fd/{}", prepared.master_raw_fd())).exists());
        assert!(
            !PathBuf::from(format!(
                "/proc/self/fd/{}",
                prepared.retired_reader_raw_fd()
            ))
            .exists(),
            "the actor reader must be closed before the PTY master is exposed"
        );

        let resize_handle = handle.clone();
        let mut queued_resize =
            tokio::spawn(async move { resize_handle.resize(PtySize::cells(55, 9)).await });
        assert!(
            time::timeout(Duration::from_millis(40), &mut queued_resize)
                .await
                .is_err(),
            "commands after handoff preparation must remain fenced"
        );

        prepared.resume().await.unwrap();
        queued_resize.await.unwrap().unwrap();
        handle.input(b"after-handoff\n".to_vec()).await.unwrap();
        time::sleep(Duration::from_millis(50)).await;
        let snapshot = handle.snapshot().await.unwrap();
        assert_eq!(
            snapshot.dimensions,
            Dimensions {
                columns: 55,
                rows: 9,
            }
        );
        let text = snapshot_text(&snapshot);
        assert!(text.contains("<before-handoff>"));
        assert!(text.contains("<after-handoff>"));
        assert_eq!(runtime.wait().await.unwrap().code, Some(0));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn child_exit_while_handoff_is_held_is_published_and_reaped() {
        let runtime = LiveSplintRuntime::spawn(
            SplintId::new(),
            backend(),
            shell("sleep 0.5; exit 7"),
            fast_config(),
        )
        .await
        .unwrap();
        let handle = runtime.handle();
        let child_pid = handle.child_pid();
        let prepared = handle.prepare_pty_handoff().await.unwrap();
        time::sleep(Duration::from_millis(650)).await;

        prepared.resume().await.unwrap();
        assert_eq!(runtime.wait().await.unwrap().code, Some(7));
        assert!(!PathBuf::from(format!("/proc/{child_pid}")).exists());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn invalid_handoff_recovery_kills_and_reaps_only_the_exact_child() {
        let runtime = LiveSplintRuntime::spawn(
            SplintId::new(),
            backend(),
            shell("trap '' HUP TERM; sleep 30"),
            fast_config(),
        )
        .await
        .unwrap();
        let handle = runtime.handle();
        let child_pid = handle.child_pid();
        let mut prepared = handle.prepare_pty_handoff().await.unwrap();
        let adoptable = prepared
            .session
            .take()
            .expect("prepared test lease owns the canonical master");
        let (_, master) = adoptable.into_parts();
        let mismatched =
            LinuxPtyIdentity::from_raw(child_pid, child_pid.checked_add(1).unwrap(), child_pid)
                .unwrap();
        prepared.identity = mismatched;
        prepared.session = Some(AdoptableLinuxPtySession::from_parts(mismatched, master));

        assert!(matches!(prepared.resume().await, Err(LiveError::Closed)));
        assert_eq!(runtime.wait().await.unwrap().signal, Some(9));
        assert!(!PathBuf::from(format!("/proc/{child_pid}")).exists());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_preparation_unfences_a_saturated_writer() {
        let mut config = fast_config();
        config.command_capacity = 4;
        config.input_byte_limit = 1024 * 1024;
        config.poll_interval = Duration::from_millis(500);
        let runtime = LiveSplintRuntime::spawn(
            SplintId::new(),
            backend(),
            shell("trap '' HUP TERM; sleep 30"),
            config,
        )
        .await
        .unwrap();
        let handle = runtime.handle();
        let child_pid = handle.child_pid();
        handle.input(vec![b'x'; 256 * 1024]).await.unwrap();

        let prepare_handle = handle.clone();
        let prepare = tokio::spawn(async move { prepare_handle.prepare_pty_handoff().await });
        time::sleep(Duration::from_millis(50)).await;
        let resize_handle = handle.clone();
        let mut resize =
            tokio::spawn(async move { resize_handle.resize(PtySize::cells(57, 11)).await });
        assert!(
            time::timeout(Duration::from_millis(40), &mut resize)
                .await
                .is_err(),
            "the handoff request must fence commands while accepted writes are blocked"
        );

        prepare.abort();
        assert!(prepare.await.unwrap_err().is_cancelled());
        time::timeout(Duration::from_millis(300), &mut resize)
            .await
            .expect("cancelling preparation must release the actor fence directly")
            .unwrap()
            .unwrap();
        let snapshot = handle.snapshot().await.unwrap();
        assert_eq!(
            snapshot.dimensions,
            Dimensions {
                columns: 57,
                rows: 11,
            }
        );
        drop(handle);
        drop(runtime);
        let process = PathBuf::from(format!("/proc/{child_pid}"));
        time::timeout(Duration::from_secs(3), async {
            while process.exists() {
                time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("cancelled preparation cleanup must reap the blocked child");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_prepared_pty_handoff_recovers_the_actor() {
        let runtime = LiveSplintRuntime::spawn(
            SplintId::new(),
            backend(),
            shell("printf ready; read value; printf '<%s>' \"$value\"; sleep 0.2"),
            fast_config(),
        )
        .await
        .unwrap();
        let handle = runtime.handle();
        time::sleep(Duration::from_millis(50)).await;
        let prepared = handle.prepare_pty_handoff().await.unwrap();
        assert!(PathBuf::from(format!("/proc/self/fd/{}", prepared.master_raw_fd())).exists());
        assert!(
            !PathBuf::from(format!(
                "/proc/self/fd/{}",
                prepared.retired_reader_raw_fd()
            ))
            .exists()
        );
        drop(prepared);

        handle.input(b"drop-recovers\n".to_vec()).await.unwrap();
        time::sleep(Duration::from_millis(50)).await;
        assert!(snapshot_text(&handle.snapshot().await.unwrap()).contains("<drop-recovers>"));
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
