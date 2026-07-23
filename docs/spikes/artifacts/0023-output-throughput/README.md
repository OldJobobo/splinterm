# Plan 0009 non-graphical output-throughput evidence

This directory records the first implementation checkpoint for
[Plan 0009](../../../plans/0009-output-throughput-optimization.md). It is
non-graphical development evidence, not closure evidence and not a replacement
for the guarded five-terminal matrix.

## Root cause

The terminal action baseline called `snapshot_scrollback_rows(usize::MAX)` both
before and after every parsed action. That helper walks the scrollback ring and
allocates a vector of retained row references. Ordinary printable characters do
not need the result; only `CSI 3 J` uses it to decide whether clearing history
caused scrollback damage.

The retained fix makes the before/after row-count work conditional on the exact
`CSI 3 J` action hint. It preserves action-based revisions, update history,
parser effects, PTY replies, and damage behavior.

## Result

The release `phase9-daemon-benchmark` workload consumes 126,056 PTY bytes and
produces 108,056 terminal updates. The original instrumented run spent 4.740 s
end to end and 4.740 s in output processing. Five optimized runs measured:

| Boundary | Minimum | Median | Maximum |
|---|---:|---:|---:|
| Output completion | 125.1 ms | 131.1 ms | 147.1 ms |
| Output processing | 123.3 ms | 129.3 ms | 145.6 ms |

The median development result is a 36.17× end-to-end improvement and a 36.65×
output-processing improvement. Update count remains exactly 108,056, proving
that the retained result does not depend on coarser revision semantics.

## Instrumentation

`LiveRuntimeMetrics` now reports aggregate, body-free counters for PTY reads,
parse batches, terminal updates, live events, subscriber overflow, output
processing time, and snapshot construction. The Phase 9 validator requires the
new counters to be populated.

## Rejected experiments

- Coalescing one terminal revision per 256-byte transaction reduced 108,056
  updates to about 496, but provided only a small improvement before the actual
  root cause was fixed and changed revision granularity. It was removed.
- Increasing the publication transaction from 256 bytes to 1 KiB, 4 KiB, and
  16 KiB reduced event counts but measured slower in the diagnostic workload.
  The 256-byte bound remains.
- Protocol subscription coalescing was reverted before checkpointing after a
  lifecycle integration timeout appeared. Restoring the original subscription
  and revision behavior did not clear that timeout, so it remains a separately
  recorded validation blocker rather than part of this checkpoint.

## Validation status

Passed:

- `cargo test -p splinterm-terminal`
- `cargo test -p splinterd --lib`
- daemon unit tests
- detach/reattach overflow-resync integration test
- all other daemon end-to-end cases in the workspace run
- formatting and whitespace checks

Blocked:

- `headless_policy_reload_fails_closed_and_cleans_up` consistently reaches its
  fixed 20-second outer timeout. The optimization does not touch policy reload,
  and restoring original subscription and action-revision semantics did not
  clear it. No further retries were made.
- Guarded graphical validation has not run. It requires explicit approval and
  must follow the workspace-8/DP-2 isolation rules.

## Reproduction

```bash
cargo build --release -p splinterm-pty --bin splinterm-pty-child
cargo build --release -p splinterd --example phase9-daemon-benchmark
target/release/examples/phase9-daemon-benchmark
```

Host for these development measurements:

- Linux 7.1.4-arch1-1 x86_64
- rustc 1.91.0
- planning baseline commit `aef92502c029de8f3b684e7482bda9f8d7000dd3`
- optimized benchmark binary SHA-256:
  `75900afb3e96b986b92dc968e73f57ef294c62184b89a2bccbe8ad28a079be0b`
- PTY helper SHA-256:
  `351c488ae86526804d202f9f72bb1ebfe21166aae51df95a74cb6a2ffbf653fd`

The raw before/after JSON records are retained under `raw/`. The original
instrumented binary was replaced during iteration, so its binary hash was not
recorded; this limitation is why the artifact is labeled development evidence.
