# Spike 0011: damage-driven rendering

- **Status:** Implementation validated; full performance baseline matrix pending
- **Date:** 2026-07-18
- **Protocol:** version 5
- **Plan:** [Omarchy-native terminal MVP](../plans/0002-omarchy-terminal-mvp.md)

## Question

Can a slow disposable Wayland viewer consume daemon terminal output without
retransmitting, reshaping, and repainting the complete visible grid for every
revision?

## Mechanism

Protocol v5 replaces subscription snapshots with bounded `TerminalUpdate`
messages after the atomic attach snapshot. Each update names its base and next
revision and may carry changed rows, semantic scroll operations, cursor, title,
modes, screen, colors, or dimensions. DTO validation rejects discontinuities,
duplicate/out-of-range rows, invalid scroll regions, oversized grids, and
non-256-entry palettes. Overflow and revision gaps retain the existing explicit
snapshot resynchronization path. If a queued historical update is paired with a
newer actor snapshot, the daemon sends that newer full snapshot instead of
mislabeling its semantics with the historical revision.

The client owns and patches a stable semantic `TerminalSnapshot`. It coalesces
updates until a Wayland frame is available, tracks separately which prepared
rows and presented rows are dirty, copies safe scroll regions in its persistent
CPU backing store, and submits row-sized `damage_buffer` rectangles. Dimension,
scale, palette, default-color, and active-screen changes force a full rebuild.
Cursor and title-only changes do not reshape unchanged rows.

`SnapshotFrame` incrementally prepares damaged rows. Font faces are resolved
once and scale-keyed glyph images live in a process-local cache capped at 2,048
entries with FIFO eviction and hit/miss/byte counters. Each prepared frame also
holds only glyphs referenced by its current visible rows; stale row-refresh
entries are pruned, and active references are bounded by the negotiated
`MAX_COLUMNS * MAX_ROWS` grid. The Wayland SHM pool and CPU backing store are
reused. A pending frame callback prevents commits from
running ahead of the compositor; newer damage remains coalesced. Cursor blink is
a 500 ms client timer that dirties only the cursor row and never mutates daemon
state.

## Headless release baseline

Command:

```sh
cargo run --release -p splinterm --example phase4-renderer-benchmark -- 10
```

Measured on AMD Ryzen 5 5600G (12 logical CPUs), Linux 7.1.3-arch2-1,
rustc 1.91.0, and Hyprland 0.55.4:

| Grid | Warm full prepare median | One-row prepare median | Full paint median | One-row paint median |
|---|---:|---:|---:|---:|
| 80×24 | 35.37 ms | 1.52 ms | 2.27 ms | 0.084 ms |
| 240×80 | 359.08 ms | 4.48 ms | 23.75 ms | 0.293 ms |

The one-glyph synthetic corpus produced one cache miss and 235,519 hits without
eviction. The persistent scale cache is capped at 2,048 glyphs; the active
frame additionally retains only glyphs referenced by the bounded 240×80 grid
and prunes stale references after row refresh. These figures isolate renderer
work; they are not input-to-photon measurements.

## Validation

Tests cover protocol bounds, exact-revision delta selection, semantic row
application, continuity rejection, row-only paint clipping, framebuffer-clipped
scroll pixel copies, incremental row preservation, stale frame-cache pruning,
and cursor/title updates without row reshaping. Workspace tests and strict Clippy
pass.

On empty workspace 8 / DP-2, the protocol-v5 window ran `btop`, survived rapid
resize churn, detached, and reattached to the same live `btop` process. It then
survived repeated 5,000-line `yes` bursts, including one interleaved with rapid
resize, and a 1,000-line ANSI-color burst. A five-second
`/proc` tick sample while idle with cursor blink enabled measured approximately
1.4% of one CPU on this development host. The test window and daemon were
cleaned up after validation.

## Residual risks

- The CPU backing store is copied into the acquired SHM canvas on each committed
  frame; row painting is partial, but upload is not yet per-row.
- FIFO glyph eviction is bounded and deterministic but not workload-optimal.
- The plan's continuous `yes`, large colored-file `cat`, measured rapid-resize,
  detach/reattach full-snapshot, and total renderer-memory baselines remain to be
  recorded with host/software context.
- Input-to-photon latency still needs a dedicated presentation-timestamp probe;
  the current benchmarks measure CPU preparation and painting only.
