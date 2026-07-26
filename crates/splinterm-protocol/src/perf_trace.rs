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
    pub duration_ns: Option<u64>,
    pub queue_wait_ns: Option<u64>,
    pub bytes: Option<u64>,
    pub rows: Option<u64>,
    pub cells: Option<u64>,
    pub count: Option<u64>,
    pub queue_depth: Option<u64>,
    pub full_reload: Option<bool>,
    pub resync: Option<bool>,
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
    full_reload: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resync: Option<bool>,
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
            duration_ns: event.duration_ns,
            queue_wait_ns: event.queue_wait_ns,
            bytes: event.bytes,
            rows: event.rows,
            cells: event.cells,
            count: event.count,
            queue_depth: event.queue_depth,
            full_reload: event.full_reload,
            resync: event.resync,
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
        schema: "splinterm.performance.stage.v1",
        run_id: &trace.run_id,
        process,
        pid: std::process::id(),
        sequence,
        clock: "CLOCK_MONOTONIC_RAW shared host namespace",
        monotonic_raw_ns: monotonic_raw_ns(),
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
}
