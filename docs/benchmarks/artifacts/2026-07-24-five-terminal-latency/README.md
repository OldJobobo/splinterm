# Five-terminal targeted-input latency evidence

This development artifact records 3 randomized warmups and 10 measured samples
for Splinterm, Foot, Kitty, Ghostty, and Alacritty (15 warmup and 50 measured
cases). Seed: `20260729`.

## Boundaries

- Input delivery: Hyprland 0.56 `hl.dsp.send_shortcut`, targeted by the owned
  benchmark-window address while the window remains unfocused on workspace 8,
  DP-2.
- Input-to-child: controller `CLOCK_MONOTONIC` timestamp immediately before the
  targeted `x` and Return dispatches through the child's atomic receipt record.
- Input-to-visible: the same initial timestamp through detection of the RGB
  marker in repeated tightly cropped `grim` captures.
- Presentation: **not measured**. Screenshot polling is an approximation and is
  not compositor presentation or input-to-photon latency.

Every raw result records continuously checked host focus/workspace and verified
cleanup. `matrix.json` records 50/50 valid measured cases; warmup records are
retained but excluded from aggregation.

## Splinterm latency fixes

Terminal protocol updates may commit through one released or second bounded SHM
buffer while an earlier frame callback is delayed. The client retains at most
two SHM buffers, preserves terminal priority across buffer exhaustion, retries
when release events return control to the loop, and leaves wheel,
cursor-animation, and other redraws frame-paced.

Tokio update receivers are drained through `poll_recv` with a calloop
`PingSource`-backed waker. Once a receiver reaches `Pending`, the next successful
sender enqueue wakes calloop immediately. Topology is applied before the focused
receiver is armed, pings coalesce, channel backpressure remains unchanged, and
the 50 ms dispatch timeout remains only for cursor/signoff/clipboard fallback.

The measured Splinterm input-to-visible median fell from the original
**371.45 ms** to **184.88 ms**. The earlier bounded frame-only matrix measured
**203.22 ms**. Current peer medians are **179.53–189.21 ms**.

The live worktree contained unrelated incomplete iTerm and renderer changes. The
patched Splinterm client, matching daemon, and PTY helper were built from the
recorded base commit in an isolated worktree. Explicit benchmark binary-path
overrides avoided replacing live binaries. `build-provenance.json`,
`implementation.json`, and `implementation/` retain the base commit, patch,
source/config snapshots, and binary hashes used for this matrix.

## Reproduction

```bash
export SPLINTERBENCH_SPLINTERM_CLIENT=/path/to/isolated/target/release/splinterm
export SPLINTERBENCH_SPLINTERM_DAEMON=/path/to/isolated/target/release/splinterd
python tools/benchmark/run.py probe-latency-boundary --json
python tools/benchmark/run-graphical-latency.py /tmp/splinterbench-latency-smoke \
  --terminal splinterm
python tools/benchmark/run-latency-matrix.py \
  docs/benchmarks/artifacts/2026-07-24-five-terminal-latency \
  --warmup-runs 3 --samples 10 --seed 20260729
(cd docs/benchmarks/artifacts/2026-07-24-five-terminal-latency && \
  sha256sum -c SHA256SUMS)
```
