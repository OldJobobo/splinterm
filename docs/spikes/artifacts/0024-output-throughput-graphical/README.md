# Plan 0009 guarded graphical output evidence

This directory records the guarded five-terminal output matrix after the
terminal damage-accounting and bounded subscription-draining fixes.

## Run shape

- Terminals: Splinterm, Foot, Kitty, Ghostty, and Alacritty
- Workloads: 2,000 lines each of plain, ANSI/SGR, and Unicode text
- Guarded location: workspace 8 on DP-2
- Warmup blocks: 3
- Measured blocks: 10
- Measured cases: 150
- Randomization seed: `20260724`
- Result: all measured records valid and cleanup verified

The user was free to work on DP-1 and DP-3 during the matrix. Isolation required
only that benchmark windows remain unfocused on workspace 8/DP-2. The matrix
completed without a placement, focus, or cleanup violation.

## Splinterm result

| Workload | Child write | Maximum child write | Trigger→visible | Previous child write | Previous visible |
|---|---:|---:|---:|---:|---:|
| Plain | 40.2 ms | 58.1 ms | 532.5 ms | 1276.6 ms | 1890.2 ms |
| ANSI | 41.4 ms | 59.8 ms | 511.2 ms | 1319.5 ms | 1865.8 ms |
| Unicode | 44.8 ms | 46.4 ms | 529.1 ms | 1379.5 ms | 1880.1 ms |

Median child-write blocking improved by approximately 30.8–31.9×. The
screenshot-polling visible approximation improved by approximately 3.55–3.65×.
All child-write samples are below Plan 0009's 250 ms maximum gate, all medians
are below its 125 ms gate and 50 ms stretch target, and all visible medians are
below its 750 ms closure gate.

The visible marker is detected by polling guarded screenshots. It is not a
Wayland presentation timestamp and must not be reported as input-to-photon
latency.

## Interpretation

The retained terminal-kernel fix avoids enumerating the complete scrollback ring
before and after ordinary parser actions. The protocol follow-up drains only
already-queued bounded live updates before requesting one snapshot, reducing
snapshot amplification for an attached graphical client. It preserves
per-action terminal revisions and falls back to explicit resynchronization when
the bounded live subscription reports overflow.

Splinterm still has higher child-write and visible latency than Foot, and its
plain/ANSI RSS growth remains higher than the other terminals. Those are future
optimization opportunities, not blockers for this plan's provisional gates.

## Contents

- `manifest.json`: host, repository state, versions, and binary hashes.
- `matrix.json`: execution order and complete statistics.
- `summary.md`: median comparison tables.
- `comparison.json`: before/after Splinterm values and ratios.
- `raw/`: all warmup and measured records.
- `SHA256SUMS`: integrity hashes for every artifact file except itself.

The manifest records a dirty worktree because unrelated local benchmark and
documentation files remained intentionally uncommitted. Binary hashes identify
the exact executables used.
