//! Non-graphical Plan 0011 repeated burst/settle retention probe.

use std::{
    collections::BTreeSet,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use splinterd::{LiveSnapshot, LiveSplintConfig, LiveSplintRuntime, SubscriptionReceive};
use splinterm_core::SplintId;
use splinterm_pty::{LinuxPtyBackend, PtyCommand};
use tokio::time::{sleep, timeout};

#[derive(Clone, Debug, Default, Serialize)]
struct Memory {
    rss_bytes: u64,
    pss_bytes: u64,
    private_anon_bytes: u64,
    private_file_bytes: u64,
    shared_bytes: u64,
    shmem_bytes: u64,
}

#[derive(Debug, Serialize)]
struct ProcessMemory {
    pid: u32,
    name: String,
    memory: Memory,
}

fn field(values: &std::collections::BTreeMap<String, u64>, name: &str) -> u64 {
    values.get(name).copied().unwrap_or(0)
}

fn process_memory(pid: u32) -> Option<ProcessMemory> {
    let root = PathBuf::from(format!("/proc/{pid}"));
    let text = fs::read_to_string(root.join("smaps_rollup")).ok()?;
    let mut values = std::collections::BTreeMap::new();
    for line in text.lines() {
        let Some((key, raw)) = line.split_once(':') else {
            continue;
        };
        let Some(value) = raw
            .split_whitespace()
            .next()
            .and_then(|v| v.parse::<u64>().ok())
        else {
            continue;
        };
        values.insert(key.to_owned(), value.saturating_mul(1024));
    }
    let private = field(&values, "Private_Clean").saturating_add(field(&values, "Private_Dirty"));
    let private_anon = private.min(field(&values, "Anonymous"));
    Some(ProcessMemory {
        pid,
        name: fs::read_to_string(root.join("comm"))
            .unwrap_or_default()
            .trim()
            .to_owned(),
        memory: Memory {
            rss_bytes: field(&values, "Rss"),
            pss_bytes: field(&values, "Pss"),
            private_anon_bytes: private_anon,
            private_file_bytes: private.saturating_sub(private_anon),
            shared_bytes: field(&values, "Shared_Clean")
                .saturating_add(field(&values, "Shared_Dirty")),
            shmem_bytes: field(&values, "ShmemPmdMapped"),
        },
    })
}

fn descendants(pid: u32, found: &mut BTreeSet<u32>) {
    if !found.insert(pid) {
        return;
    }
    let path = format!("/proc/{pid}/task/{pid}/children");
    if let Ok(text) = fs::read_to_string(path) {
        for child in text
            .split_whitespace()
            .filter_map(|value| value.parse().ok())
        {
            descendants(child, found);
        }
    }
}

fn sample() -> serde_json::Value {
    let mut pids = BTreeSet::new();
    descendants(std::process::id(), &mut pids);
    let processes: Vec<_> = pids.into_iter().filter_map(process_memory).collect();
    let total = processes.iter().fold(Memory::default(), |mut sum, item| {
        sum.rss_bytes += item.memory.rss_bytes;
        sum.pss_bytes += item.memory.pss_bytes;
        sum.private_anon_bytes += item.memory.private_anon_bytes;
        sum.private_file_bytes += item.memory.private_file_bytes;
        sum.shared_bytes += item.memory.shared_bytes;
        sum.shmem_bytes += item.memory.shmem_bytes;
        sum
    });
    serde_json::json!({"aggregate": total, "processes": processes})
}

fn contains(snapshot: &LiveSnapshot, marker: &str) -> bool {
    snapshot
        .visible_rows
        .iter()
        .chain(&snapshot.scrollback_rows)
        .any(|row| {
            row.cells
                .iter()
                .filter(|cell| cell.spacer_remaining.is_none())
                .map(|cell| cell.content.as_str())
                .collect::<String>()
                .contains(marker)
        })
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    if cfg!(debug_assertions) {
        bail!("run this probe in release mode");
    }
    let case = std::env::var("PLAN11_CASE").unwrap_or_else(|_| "delayed".into());
    let cycles: usize = std::env::var("PLAN11_CYCLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let final_settle: u64 = std::env::var("PLAN11_FINAL_SETTLE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120);
    let scrollback = std::env::var("PLAN11_SCROLLBACK")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| {
            if case == "scrollback-disabled" {
                0
            } else {
                1_000
            }
        });
    let helper = std::env::var_os("SPLINTERM_PTY_HELPER").map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/release/splinterm-pty-child")
        },
        PathBuf::from,
    );
    let script = format!(
        r#"IFS= read -r _; c=0; while [ $c -lt {cycles} ]; do i=0; while [ $i -lt 5000 ]; do if [ $i -gt 0 ] && [ $((i % 500)) -eq 0 ]; then printf '\033[2J\033[H'; fi; case $((i % 3)) in 0) printf 'retain-%08d plain xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n' $i;; 1) printf '\033[3%dmretain-%08d ansi xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\033[0m\n' $((i % 8)) $i;; 2) printf 'retain-%08d unicode-naive-cafe-lambda-emoji\n' $i;; esac; i=$((i+1)); done; printf 'PLAN11_CYCLE_%02d\n' $c; c=$((c+1)); if [ $c -lt {cycles} ]; then IFS= read -r _; fi; done; sleep 180"#
    );
    let config = LiveSplintConfig {
        columns: 80,
        rows: 24,
        subscriber_capacity: if case == "overflow" { 1 } else { 64 },
        max_scrollback_snapshot_rows: 1_000,
        terminal: splinterm_terminal::TerminalConfig {
            scrollback_lines: scrollback,
            ..Default::default()
        },
        ..Default::default()
    };
    let runtime = LiveSplintRuntime::spawn_with_publication_memory_metrics(
        SplintId::new(),
        LinuxPtyBackend::new(helper),
        PtyCommand::new("/bin/sh", "/tmp").args(["-c", &script]),
        config,
    )
    .await?;
    let handle = runtime.handle();
    let mut drains = Vec::new();
    let mut held = Vec::new();
    let drain_events = Arc::new(AtomicU64::new(0));
    let drain_resnapshots = Arc::new(AtomicU64::new(0));
    match case.as_str() {
        "no-subscriber" | "scrollback-disabled" | "scrollback-1000" => {}
        "fast" => {
            let (_, mut subscription) = handle.attach_compact_with_scrollback(scrollback).await?;
            let event_count = Arc::clone(&drain_events);
            let resnapshot_count = Arc::clone(&drain_resnapshots);
            drains.push(tokio::spawn(async move {
                loop {
                    match subscription.recv_coalesced().await.0 {
                        SubscriptionReceive::Event(_) => {
                            event_count.fetch_add(1, Ordering::Relaxed);
                        }
                        SubscriptionReceive::ResnapshotRequired => {
                            resnapshot_count.fetch_add(1, Ordering::Relaxed);
                            break;
                        }
                        SubscriptionReceive::Closed => break,
                    }
                }
            }));
        }
        "multiple" => {
            for _ in 0..2 {
                let (_, mut subscription) =
                    handle.attach_compact_with_scrollback(scrollback).await?;
                let event_count = Arc::clone(&drain_events);
                let resnapshot_count = Arc::clone(&drain_resnapshots);
                drains.push(tokio::spawn(async move {
                    loop {
                        match subscription.recv_coalesced().await.0 {
                            SubscriptionReceive::Event(_) => {
                                event_count.fetch_add(1, Ordering::Relaxed);
                            }
                            SubscriptionReceive::ResnapshotRequired => {
                                resnapshot_count.fetch_add(1, Ordering::Relaxed);
                                break;
                            }
                            SubscriptionReceive::Closed => break,
                        }
                    }
                }));
            }
        }
        "delayed" | "overflow" => {
            let (_, subscription) = handle.attach_compact_with_scrollback(scrollback).await?;
            held.push(subscription);
        }
        other => bail!("unknown PLAN11_CASE {other}"),
    }
    let baseline = sample();
    let mut endpoints = Vec::new();
    for cycle in 0..cycles {
        handle.input(b"start\n".to_vec()).await?;
        let marker = format!("PLAN11_CYCLE_{cycle:02}");
        timeout(Duration::from_secs(30), async {
            loop {
                let snapshot = handle.snapshot_with_scrollback(0).await?;
                if contains(&snapshot, &marker) {
                    return Ok::<_, anyhow::Error>(());
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .context("cycle output timeout")??;
        let marker_sample = sample();
        sleep(Duration::from_secs(2)).await;
        endpoints.push(
            serde_json::json!({"cycle": cycle, "marker": marker_sample, "settle_2s": sample()}),
        );
    }
    if final_settle > 2 {
        sleep(Duration::from_secs(final_settle - 2)).await;
    }
    let final_sample = sample();
    let metrics = handle.metrics();
    for drain in drains {
        drain.abort();
        let _ = drain.await;
    }
    drop(held);
    runtime.shutdown().await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "schema": "splinterm.plan0011.daemon-retention.v1", "case": case, "cycles": cycles,
            "lines_per_cycle": 5000, "clear_interval_lines": 500, "scrollback_lines": scrollback,
            "baseline": baseline, "endpoints": endpoints, "final_settle_seconds": final_settle,
            "final": final_sample, "runtime_metrics": metrics,
            "drain_events": drain_events.load(Ordering::Relaxed),
            "drain_resnapshots": drain_resnapshots.load(Ordering::Relaxed)
        }))?
    );
    Ok(())
}
