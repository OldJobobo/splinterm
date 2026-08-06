# Plan 0023: ANSI history throughput

- **Status:** Complete
- **Date:** 2026-08-06
- **Parent:** [Plan 0022 history-heavy graphical catch-up](0022-history-heavy-graphical-catch-up.md)
- **Evidence:** [Plan 0023 ANSI history throughput](../benchmarks/artifacts/2026-08-06-plan0023-ansi-history-throughput/README.md)

## Decision

Optimize only the bounded client-side scrollback reducer identified by Plan
0022's 2,000-line ANSI diagnostics. Preserve protocol, viewport, history,
memory, queue, and rendering behavior. Do not add worker threads, alter pacing,
increase bounds, or run another broad graphical matrix.

## Measured problem

`apply_scrollback_update()` rebuilt retained history for every append by cloning
every surviving row. `bound_history_cache()` then rescanned the full cache for
each loop condition and removed the oldest rows one at a time from index zero.
For a 2,000-update operation, this produced quadratic deep-copy, byte-scan, and
prefix-shift amplification even when the cache began empty.

The retained Plan 0022 candidate is the exact control: release profile, five
warmups, 30 measured samples, deterministic interleaving, 2,000 immutable
updates prepared outside the measured interval, and fixed 0/1,000/4,096-row
cases.

## Implementation

1. Move the snapshot's private cached history vector into the reducer and retain
   matching row identities in place instead of deep-cloning every surviving
   row.
2. Compute cached bytes once per bound operation.
3. Trim row-limit and byte-limit prefixes in batches; retain the oldest-page
   path by truncating or popping from the tail.
4. Use saturating byte accounting without changing the 4,096-row or 16 MiB
   limits.
5. Test the batched algorithm against the previous reference implementation for
   empty, exact-bound, row-overflow, byte-overflow, and both-edge retention
   cases. Preserve detached anchoring through the existing 2,000-append test.

## Results

Matched release p95 results:

| Case | Control | Candidate | Improvement | 95% lower improvement |
| --- | ---: | ---: | ---: | ---: |
| ANSI, 0-row start, one focused pane | 117.028 ms | 5.000 ms | 95.7% | 93.6% |
| ANSI, 1,000-row start, one focused pane | 253.246 ms | 8.626 ms | 96.6% | 96.0% |
| ANSI, 4,096 rows, one focused pane | 430.084 ms | 28.515 ms | 93.4% | 93.2% |
| ANSI, 4,096 rows, detached pane | 1,023.232 ms | 604.791 ms | 40.9% | 36.0% |
| ANSI, 4,096 rows, four active panes | 2,048.908 ms | 118.723 ms | 94.2% | 93.6% |
| ANSI, 4,096 rows, three inactive panes | 1,424.074 ms | 102.086 ms | 92.8% | 92.5% |

All ordinary small-update p95 values remain below 0.344 ms. Their relative
ratios are noise-dominated at microsecond scale; the largest absolute candidate
increase is 0.053 ms in the intentionally detached case and does not execute the
optimized scrollback reducer for a small update.

## Acceptance gates

- The new bounding algorithm is byte-for-byte equivalent to the previous
  algorithm across row and byte limits and both retained edges.
- Live and detached history semantics, row IDs, generation, available/omitted
  counts, and viewport anchoring remain exact.
- One-pane maximum-history ANSI p95 improves by at least 50%, with a one-sided
  95% lower improvement bound above 40%.
- Four-pane all-active ANSI p95 improves by at least 50%, with the same bound.
- No history, memory, queue, cache, SHM, thread, or protocol bound increases.
- No graphical claim is made from the non-graphical reducer harness.

The candidate passes every applicable gate. Graphical Plan 0022 evidence remains
immutable. A future end-to-end ANSI claim may use one newly planned focused cell;
it does not justify repeating the 130-case diagnostic matrix.

## Validation

```bash
cargo fmt --all -- --check
cargo test -p splinterm terminal_state
cargo test -p splinterm --example history-catchup-benchmark
cargo test -p splinterm
python tools/performance/summarize-history-catchup.py REPORT SUMMARY
python tools/performance/compare-history-catchup.py CONTROL CANDIDATE COMPARISON
git diff --check
```

Broad Rust 1.97 Clippy remains blocked by pre-existing repository-wide style
findings outside this slice. The changed terminal-state module introduces no
reported Clippy finding.

## Residuals

- Detached viewport work intentionally retains previous-row identity data and
  therefore improves less than live updates.
- This slice removes client reducer amplification; it does not claim to remove
  daemon/socket admission, frame scheduling, rasterization, or compositor cost.
- The Plan 0022 graphical all-pane and ANSI measurements are not silently
  replaced by this non-graphical evidence.
