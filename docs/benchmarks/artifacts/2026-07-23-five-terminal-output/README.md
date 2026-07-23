# Five-terminal graphical output benchmark evidence

This directory preserves the corrected development output matrix recorded on
2026-07-23. It is durable evidence for the benchmark harness and a concrete
performance lead for Splinterm development, not a universal terminal ranking.

## Run shape

- Terminals: Splinterm, Foot, Kitty, Ghostty, and Alacritty
- Workloads: 2,000 lines each of plain, ANSI/SGR, and Unicode text
- Guarded location: inactive workspace 8 on DP-2
- Warmup blocks: 3
- Measured blocks: 10
- Measured cases: 150
- Randomization seed: `20260724`
- Result: all corrected raw records valid with cleanup verified

The child waits for an explicit trigger after its terminal window maps. Each
record keeps the child PTY-write duration, trigger-to-write completion,
child-inclusive process-tree resource changes, and a screenshot-polling visible
marker approximation. The visible boundary detects a final uncommon truecolor
row; it is not a Wayland presentation timestamp or input-to-photon measurement.

## Important interpretation

Splinterm's child writes block against the daemon-owned PTY/output path for much
longer than writes into the other terminals. That makes the write timing a
useful backpressure observation, but it does not by itself identify which
internal stage is responsible. The visible-marker results are the more relevant
end-to-end approximation in this lane. Profiling and queue instrumentation
should precede optimization claims.

## Audit events

Two invalid attempts were deliberately excluded from summary statistics but
preserved under `diagnostics/`:

1. The first ANSI marker used magenta, which also appeared in the ANSI workload.
   A calibration run then showed the replacement truecolor marker captured as
   `(18, 231, 111)`; the detector was bounded around that observed color.
2. One late Splinterm/ANSI attempt activated reserved workspace 8. The guard
   aborted immediately, killed the window, and stopped the matrix. Resume mode
   reused 147 valid measured records and reran only that failed case plus the two
   unexecuted tail cases after focus was safely back on workspace 1/DP-1.

## Contents

- `manifest.json`: host, repository state, terminal versions, and binary hashes.
- `matrix.json`: execution order and complete statistics.
- `summary.md`: median result tables and boundary warning.
- `raw/`: all warmup and corrected measured records.
- `diagnostics/`: discarded failure records and marker-calibration screenshot.
- `SHA256SUMS`: integrity hashes for all evidence files except itself.

The manifest records a dirty worktree, so the exact Splinterm executable hash is
the binary identity for this run. See
[`../../terminal-benchmark-plan.md`](../../terminal-benchmark-plan.md) for the
methodology and fairness contract.
