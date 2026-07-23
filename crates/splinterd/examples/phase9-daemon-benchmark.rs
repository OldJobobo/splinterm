//! Content-free release benchmark for daemon output, paging, and responsiveness.

use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail};
use splinterd::{
    LiveCell, LiveRow, LiveScrollbackPage, LiveSnapshot, LiveSplintConfig, LiveSplintRuntime,
    SubscriptionReceive,
};
use splinterm_core::SplintId;
use splinterm_pty::{LinuxPtyBackend, PtyCommand, PtySize};
use tokio::time::{Instant, sleep, timeout};

const OUTPUT_DONE: &str = "PHASE9_OUTPUT_DONE";
const INPUT_DONE: &str = "PHASE9_INPUT_DONE";

fn snapshot_contains(snapshot: &LiveSnapshot, marker: &str) -> bool {
    snapshot
        .scrollback_rows
        .iter()
        .chain(&snapshot.visible_rows)
        .any(|row| {
            let text = row
                .cells
                .iter()
                .filter(|cell| cell.spacer_remaining.is_none())
                .map(|cell| cell.content.as_str())
                .collect::<String>();
            text.contains(marker)
        })
}

async fn wait_for_marker(
    handle: &splinterd::LiveSplintHandle,
    marker: &str,
    deadline: Duration,
) -> Result<LiveSnapshot> {
    timeout(deadline, async {
        loop {
            let snapshot = handle.snapshot_with_scrollback(16).await?;
            if snapshot_contains(&snapshot, marker) {
                return Ok(snapshot);
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("timed out waiting for bounded benchmark marker")?
}

fn timing_summary(samples: &mut [u64]) -> serde_json::Value {
    samples.sort_unstable();
    let percentile =
        |numerator: usize| samples[(samples.len() - 1).saturating_mul(numerator) / 100];
    serde_json::json!({
        "min": samples[0],
        "median": percentile(50),
        "p95": percentile(95),
        "max": samples[samples.len() - 1],
    })
}

fn approximate_page_bytes(page: &LiveScrollbackPage) -> usize {
    size_of::<LiveScrollbackPage>()
        + page.title.capacity()
        + page.rows.capacity() * size_of::<LiveRow>()
        + page
            .rows
            .iter()
            .map(|row| {
                row.cells.capacity() * size_of::<LiveCell>()
                    + row
                        .cells
                        .iter()
                        .map(|cell| cell.content.capacity())
                        .sum::<usize>()
            })
            .sum::<usize>()
}

fn elapsed_ns(started: Instant) -> Result<u64> {
    u64::try_from(started.elapsed().as_nanos()).context("duration fits u64")
}

fn rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?
        .checked_mul(1024)
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
#[allow(
    clippy::too_many_lines,
    reason = "one benchmark transaction keeps workload timing and cleanup auditable"
)]
async fn main() -> Result<()> {
    if cfg!(debug_assertions) {
        bail!("phase9-daemon-benchmark must run from a release build");
    }
    let helper = std::env::var_os("SPLINTERM_PTY_HELPER").map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/release/splinterm-pty-child")
        },
        PathBuf::from,
    );
    if !helper.is_file() {
        bail!(
            "build the release splinterm-pty-child helper first: {}",
            helper.display()
        );
    }
    let script = format!(
        "IFS= read -r _; yes phase9 | head -n 10000; \
         i=0; while [ $i -lt 2000 ]; do printf '\\033[3%dmcolor-%06d\\033[0m\\n' $((i % 8)) $i; i=$((i + 1)); done; \
         printf '{OUTPUT_DONE}\\n'; IFS= read -r _; printf '{INPUT_DONE}\\n'; sleep 60"
    );
    let config = LiveSplintConfig {
        columns: 80,
        rows: 24,
        command_capacity: 64,
        subscriber_capacity: 1,
        max_scrollback_snapshot_rows: 16,
        poll_interval: Duration::from_millis(2),
        hangup_grace: Duration::from_millis(100),
        terminate_grace: Duration::from_millis(100),
        terminal: splinterm_terminal::TerminalConfig {
            scrollback_lines: 4096,
            ..splinterm_terminal::TerminalConfig::default()
        },
        ..LiveSplintConfig::default()
    };
    let runtime = LiveSplintRuntime::spawn(
        SplintId::new(),
        LinuxPtyBackend::new(helper),
        PtyCommand::new("/bin/sh", "/tmp").args(["-c", &script]),
        config.clone(),
    )
    .await?;
    let handle = runtime.handle();
    let (_, mut stalled_subscription) = handle.attach_with_scrollback(16).await?;
    let rss_before = rss_bytes();

    let output_started = Instant::now();
    handle.input(b"start\n".to_vec()).await?;
    let completed = wait_for_marker(&handle, OUTPUT_DONE, Duration::from_secs(20)).await?;
    let output_ns = elapsed_ns(output_started)?;
    let subscriber_resnapshot_required = timeout(Duration::from_secs(1), async {
        loop {
            match stalled_subscription.recv().await {
                SubscriptionReceive::ResnapshotRequired => return true,
                SubscriptionReceive::Closed => return false,
                SubscriptionReceive::Event(_) => {}
            }
        }
    })
    .await?;

    let mut snapshot_samples = Vec::with_capacity(20);
    for _ in 0..20 {
        let started = Instant::now();
        std::hint::black_box(handle.snapshot_with_scrollback(16).await?);
        snapshot_samples.push(elapsed_ns(started)?);
    }

    let mut page_samples = Vec::new();
    let mut page_rows = 0_usize;
    let mut page_bytes = 0_usize;
    let mut before = completed
        .scrollback
        .newest_available_row_id
        .and_then(|id| id.checked_add(1));
    for _ in 0..8 {
        let Some(cursor) = before else { break };
        let started = Instant::now();
        let page = handle.scrollback_page(cursor, 16).await?;
        page_samples.push(elapsed_ns(started)?);
        page_rows = page_rows.saturating_add(page.rows.len());
        page_bytes = page_bytes.saturating_add(approximate_page_bytes(&page));
        before = page.rows.first().and_then(|row| row.row_id);
        if !page.has_older {
            break;
        }
    }

    let generation_before = completed.scrollback.history_generation;
    let resize_started = Instant::now();
    handle.resize(PtySize::cells(120, 40)).await?;
    let resized = timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = handle.snapshot_with_scrollback(16).await?;
            if snapshot.dimensions.columns == 120 && snapshot.dimensions.rows == 40 {
                return Ok::<_, anyhow::Error>(snapshot);
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .context("resize benchmark timed out")??;
    let resize_ns = elapsed_ns(resize_started)?;

    let input_started = Instant::now();
    handle.input(b"continue\n".to_vec()).await?;
    wait_for_marker(&handle, INPUT_DONE, Duration::from_secs(5)).await?;
    let input_response_ns = elapsed_ns(input_started)?;
    let metrics = handle.metrics();
    let rss_after = rss_bytes();
    runtime.shutdown().await?;

    let page_timing = if page_samples.is_empty() {
        serde_json::Value::Null
    } else {
        timing_summary(&mut page_samples)
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "schema": "splinterm.performance.daemon.v1",
            "profile": "release",
            "workload": { "plain_lines": 10000, "colored_lines": 2000 },
            "output_ns": output_ns,
            "subscriber_resnapshot_required": subscriber_resnapshot_required,
            "snapshot_ns": timing_summary(&mut snapshot_samples),
            "paging": {
                "pages": page_samples.len(),
                "rows": page_rows,
                "approximate_retained_bytes": page_bytes,
                "fetch_ns": page_timing
            },
            "resize": {
                "ns": resize_ns,
                "columns": resized.dimensions.columns,
                "rows": resized.dimensions.rows,
                "generation_advanced": resized.scrollback.history_generation > generation_before,
            },
            "post_output_input_response_ns": input_response_ns,
            "history": {
                "available_rows": completed.scrollback.available_rows,
                "returned_rows": completed.scrollback.returned_rows,
                "requested_rows": config.terminal.scrollback_lines,
                "effective_row_bound": config
                    .terminal
                    .scrollback_lines
                    .saturating_add(usize::from(config.rows))
                    .next_power_of_two()
                    .saturating_sub(usize::from(config.rows)),
                "snapshot_bound": config.max_scrollback_snapshot_rows,
            },
            "bounds": {
                "command_capacity": config.command_capacity,
                "input_byte_limit": config.input_byte_limit,
                "reply_byte_limit": config.reply_byte_limit,
                "subscriber_capacity": config.subscriber_capacity,
            },
            "runtime_metrics": metrics,
            "rss_bytes": { "before": rss_before, "after": rss_after },
        }))?
    );
    Ok(())
}
