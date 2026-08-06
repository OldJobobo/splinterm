//! Opt-in, body-free performance trace side channel.
//!
//! Records use Linux `CLOCK_MONOTONIC_RAW`, so independently written daemon and
//! client events share one host clock domain. The trace is disabled unless both
//! `SPLINTERM_PERF_TRACE_DIR` and `SPLINTERM_PERF_RUN_ID` are present. Output is
//! bounded and never changes terminal or protocol behavior.

use std::{
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use serde::Serialize;
use splinterm_core::SplintId;

const DEFAULT_MAX_EVENTS: u64 = 4096;
const MAX_EVENTS: u64 = 65_536;
const MAX_COMPONENT_BYTES: usize = 64;

static TRACE: OnceLock<Option<TraceSink>> = OnceLock::new();

struct TraceSink {
    run_id: String,
    sequence: AtomicU64,
    max_events: u64,
    writer: Mutex<BufWriter<File>>,
}

/// Body-free fields shared by correlated performance stages.
#[derive(Clone, Copy, Debug, Default)]
pub struct PerfTraceEvent {
    pub splint_id: Option<SplintId>,
    pub incarnation: Option<u64>,
    pub base_revision: Option<u64>,
    pub revision: Option<u64>,
    pub subscription_id: Option<u64>,
    pub transaction_sequence: Option<u64>,
    pub commit_sequence: Option<u64>,
    pub duration_ns: Option<u64>,
    pub queue_wait_ns: Option<u64>,
    pub bytes: Option<u64>,
    pub rows: Option<u64>,
    pub cells: Option<u64>,
    pub count: Option<u64>,
    pub queue_depth: Option<u64>,
    pub pane_role: Option<&'static str>,
    pub pane_count: Option<u64>,
    pub active_pane_count: Option<u64>,
    pub columns: Option<u64>,
    pub cached_history_rows: Option<u64>,
    pub cached_history_bytes: Option<u64>,
    pub copied_history_rows: Option<u64>,
    pub copied_history_bytes: Option<u64>,
    pub history_scan_rows: Option<u64>,
    pub history_trim_rows: Option<u64>,
    pub receiver_batch_size: Option<u64>,
    pub contiguous_updates: Option<u64>,
    pub superseded_revisions: Option<u64>,
    pub dirty_rows: Option<u64>,
    pub prepared_rows: Option<u64>,
    pub prepared_cells: Option<u64>,
    pub inactive_panes_dirty: Option<u64>,
    pub inactive_panes_prepared: Option<u64>,
    pub inactive_panes_skipped: Option<u64>,
    pub inactive_panes_superseded: Option<u64>,
    pub configure_count: Option<u64>,
    pub output_enter_events: Option<u64>,
    pub output_leave_events: Option<u64>,
    pub old_width: Option<u64>,
    pub old_height: Option<u64>,
    pub final_width: Option<u64>,
    pub final_height: Option<u64>,
    pub scale_120: Option<u64>,
    pub glyph_cache_hits: Option<u64>,
    pub glyph_cache_misses: Option<u64>,
    pub image_generation: Option<u64>,
    pub backing_clear_bytes: Option<u64>,
    pub backing_copy_bytes: Option<u64>,
    pub damage_regions: Option<u64>,
    pub damage_area: Option<u64>,
    pub shm_acquire_ns: Option<u64>,
    pub buffers_available: Option<u64>,
    pub buffers_total: Option<u64>,
    pub callbacks_coalesced: Option<u64>,
    pub event_loop_active_ns: Option<u64>,
    pub full_reload: Option<bool>,
    pub resync: Option<bool>,
    pub scale_changed: Option<bool>,
}

#[derive(Serialize)]
struct TraceRecord<'a> {
    schema: &'static str,
    run_id: &'a str,
    process: &'a str,
    pid: u32,
    sequence: u64,
    clock: &'static str,
    monotonic_raw_ns: u64,
    stage: &'a str,
    #[serde(flatten)]
    event: TraceRecordEvent,
}

#[derive(Serialize)]
struct TraceRecordEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    splint_id: Option<SplintId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    incarnation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subscription_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transaction_sequence: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit_sequence: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    queue_wait_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rows: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cells: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    queue_depth: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pane_role: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pane_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_pane_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    columns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cached_history_rows: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cached_history_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    copied_history_rows: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    copied_history_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    history_scan_rows: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    history_trim_rows: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    receiver_batch_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    contiguous_updates: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    superseded_revisions: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dirty_rows: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prepared_rows: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prepared_cells: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inactive_panes_dirty: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inactive_panes_prepared: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inactive_panes_skipped: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inactive_panes_superseded: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    configure_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_enter_events: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_leave_events: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    old_width: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    old_height: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    final_width: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    final_height: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scale_120: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    glyph_cache_hits: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    glyph_cache_misses: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    backing_clear_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    backing_copy_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    damage_regions: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    damage_area: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shm_acquire_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    buffers_available: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    buffers_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    callbacks_coalesced: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_loop_active_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    full_reload: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resync: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scale_changed: Option<bool>,
}

impl From<PerfTraceEvent> for TraceRecordEvent {
    fn from(event: PerfTraceEvent) -> Self {
        Self {
            splint_id: event.splint_id,
            incarnation: event.incarnation,
            base_revision: event.base_revision,
            revision: event.revision,
            subscription_id: event.subscription_id,
            transaction_sequence: event.transaction_sequence,
            commit_sequence: event.commit_sequence,
            duration_ns: event.duration_ns,
            queue_wait_ns: event.queue_wait_ns,
            bytes: event.bytes,
            rows: event.rows,
            cells: event.cells,
            count: event.count,
            queue_depth: event.queue_depth,
            pane_role: event.pane_role,
            pane_count: event.pane_count,
            active_pane_count: event.active_pane_count,
            columns: event.columns,
            cached_history_rows: event.cached_history_rows,
            cached_history_bytes: event.cached_history_bytes,
            copied_history_rows: event.copied_history_rows,
            copied_history_bytes: event.copied_history_bytes,
            history_scan_rows: event.history_scan_rows,
            history_trim_rows: event.history_trim_rows,
            receiver_batch_size: event.receiver_batch_size,
            contiguous_updates: event.contiguous_updates,
            superseded_revisions: event.superseded_revisions,
            dirty_rows: event.dirty_rows,
            prepared_rows: event.prepared_rows,
            prepared_cells: event.prepared_cells,
            inactive_panes_dirty: event.inactive_panes_dirty,
            inactive_panes_prepared: event.inactive_panes_prepared,
            inactive_panes_skipped: event.inactive_panes_skipped,
            inactive_panes_superseded: event.inactive_panes_superseded,
            configure_count: event.configure_count,
            output_enter_events: event.output_enter_events,
            output_leave_events: event.output_leave_events,
            old_width: event.old_width,
            old_height: event.old_height,
            final_width: event.final_width,
            final_height: event.final_height,
            scale_120: event.scale_120,
            glyph_cache_hits: event.glyph_cache_hits,
            glyph_cache_misses: event.glyph_cache_misses,
            image_generation: event.image_generation,
            backing_clear_bytes: event.backing_clear_bytes,
            backing_copy_bytes: event.backing_copy_bytes,
            damage_regions: event.damage_regions,
            damage_area: event.damage_area,
            shm_acquire_ns: event.shm_acquire_ns,
            buffers_available: event.buffers_available,
            buffers_total: event.buffers_total,
            callbacks_coalesced: event.callbacks_coalesced,
            event_loop_active_ns: event.event_loop_active_ns,
            full_reload: event.full_reload,
            resync: event.resync,
            scale_changed: event.scale_changed,
        }
    }
}

/// Returns whether the bounded diagnostic side channel is active.
#[must_use]
pub fn perf_trace_enabled() -> bool {
    trace().is_some()
}

/// Writes one bounded, body-free stage record when tracing is active.
pub fn emit_perf_trace(process: &str, stage: &str, event: PerfTraceEvent) {
    emit_perf_trace_at(process, stage, event, monotonic_raw_ns());
}

/// Writes one bounded stage record using a timestamp captured at the named boundary.
pub fn emit_perf_trace_at(
    process: &str,
    stage: &str,
    event: PerfTraceEvent,
    monotonic_raw_ns: u64,
) {
    let Some(trace) = trace() else { return };
    let sequence = trace.sequence.fetch_add(1, Ordering::Relaxed);
    if sequence > trace.max_events {
        return;
    }
    let (stage, event) = if sequence == trace.max_events {
        ("trace_saturated", PerfTraceEvent::default())
    } else {
        (stage, event)
    };
    let record = TraceRecord {
        schema: "splinterm.performance.stage.v2",
        run_id: &trace.run_id,
        process,
        pid: std::process::id(),
        sequence,
        clock: "CLOCK_MONOTONIC_RAW shared host namespace",
        monotonic_raw_ns,
        stage,
        event: event.into(),
    };
    let Ok(mut writer) = trace.writer.lock() else {
        return;
    };
    if serde_json::to_writer(&mut *writer, &record).is_ok() {
        let _ = writer.write_all(b"\n");
        let _ = writer.flush();
    }
}

/// Returns the shared Linux monotonic-raw clock in nanoseconds.
#[must_use]
pub fn monotonic_raw_ns() -> u64 {
    let value = rustix::time::clock_gettime(rustix::time::ClockId::MonotonicRaw);
    u64::try_from(value.tv_sec)
        .unwrap_or(0)
        .saturating_mul(1_000_000_000)
        .saturating_add(u64::try_from(value.tv_nsec).unwrap_or(0))
}

fn trace() -> Option<&'static TraceSink> {
    TRACE.get_or_init(TraceSink::from_environment).as_ref()
}

impl TraceSink {
    fn from_environment() -> Option<Self> {
        let directory = PathBuf::from(std::env::var_os("SPLINTERM_PERF_TRACE_DIR")?);
        let run_id = std::env::var("SPLINTERM_PERF_RUN_ID").ok()?;
        if !valid_component(&run_id) || !directory.is_dir() {
            return None;
        }
        let max_events = std::env::var("SPLINTERM_PERF_TRACE_MAX_EVENTS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_MAX_EVENTS)
            .clamp(1, MAX_EVENTS);
        let path = directory.join(format!("{run_id}-{}.jsonl", std::process::id()));
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .ok()?;
        Some(Self {
            run_id,
            sequence: AtomicU64::new(0),
            max_events,
            writer: Mutex::new(BufWriter::new(file)),
        })
    }
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_COMPONENT_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_components_are_bounded_and_path_safe() {
        assert!(valid_component("slice1-run_001.test"));
        assert!(!valid_component(""));
        assert!(!valid_component("../escape"));
        assert!(!valid_component("contains space"));
        assert!(!valid_component(&"x".repeat(MAX_COMPONENT_BYTES + 1)));
    }

    #[test]
    fn monotonic_raw_clock_advances_in_one_domain() {
        let first = monotonic_raw_ns();
        let second = monotonic_raw_ns();
        assert!(first > 0);
        assert!(second >= first);
    }

    #[test]
    fn v2_record_keeps_body_free_graphical_correlation() {
        let record = TraceRecord {
            schema: "splinterm.performance.stage.v2",
            run_id: "test",
            process: "splinterm",
            pid: 7,
            sequence: 3,
            clock: "CLOCK_MONOTONIC_RAW shared host namespace",
            monotonic_raw_ns: 11,
            stage: "pane_commit",
            event: PerfTraceEvent {
                splint_id: Some(SplintId::new()),
                incarnation: Some(2),
                revision: Some(4),
                subscription_id: Some(5),
                transaction_sequence: Some(6),
                commit_sequence: Some(8),
                pane_role: Some("focused"),
                copied_history_rows: Some(4_096),
                ..PerfTraceEvent::default()
            }
            .into(),
        };
        let value = serde_json::to_value(record).unwrap();
        assert_eq!(value["schema"], "splinterm.performance.stage.v2");
        assert_eq!(value["commit_sequence"], 8);
        assert_eq!(value["pane_role"], "focused");
        assert_eq!(value["copied_history_rows"], 4_096);
        assert!(value.get("terminal_body").is_none());

        let window_event = TraceRecord {
            schema: "splinterm.performance.stage.v2",
            run_id: "test",
            process: "splinterm",
            pid: 7,
            sequence: 4,
            clock: "CLOCK_MONOTONIC_RAW shared host namespace",
            monotonic_raw_ns: 12,
            stage: "window_event",
            event: PerfTraceEvent {
                output_enter_events: Some(1),
                ..PerfTraceEvent::default()
            }
            .into(),
        };
        let value = serde_json::to_value(window_event).unwrap();
        assert_eq!(value["output_enter_events"], 1);
        assert!(value.get("splint_id").is_none());
    }
}
