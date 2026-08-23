# Plan 0010 performance optimization — measured Slice 0, partial Slice 1, and Slice 3 development pass

This directory records development evidence from the first full-performance
optimization implementation pass. It is not integrated closure or release-grade
comparative evidence. The provisional Foot
parity gates are not yet met.

## Scope

- Baseline commit: `077e060205191d607d1c03be9bd133982df60ab2`
- Behavioral authority: Foot 1.27.0 commit
  `3c5b584b0eafa772eb4376fb6eaf6643399e190e`
- Host: AMD Ryzen 5 5600G, kernel 7.1.4-arch1-1, Omarchy
- Geometry: 80 columns, 2,000 lines, guarded workspace 8 on DP-2
- Output cases: plain, ANSI, Unicode
- Development statistics: 3 warmups and 10 measured samples
- Visible boundary: screenshot marker polling approximation, not compositor
  presentation

The five-terminal baseline completed all 195 randomized cases. The final
candidate matrix reran Splinterm only; peer values below come from the unchanged
baseline binaries. The baseline matrix manifest reported a dirty harness
worktree, so the comparison is diagnostic rather than publishable. Runtime
binary hashes were recorded explicitly.

## Accepted changes

1. Replace a full scrollback-ring allocation and scan on every forward scroll
   with direct circular indices for only the rows entering history.
2. Preserve chronological row IDs for multi-row scrolls and skip unallocated
   lazy history slots without consuming IDs.
3. Count retained terminal updates without cloning them and consume an already
   owned `UpdateBatch` without cloning it a second time.
4. Require permanent no-focus graphical placement and calibrate marker detection
   for bounded inactive-window alpha composition.
5. Require a clean source tree and record exact binary identities in the Phase 9
   non-graphical baseline runner.
6. Add partial bounded, opt-in, body-free stage tracing under
   `SPLINTERM_PERF_TRACE_DIR` plus `SPLINTERM_PERF_RUN_ID`, using one documented
   `CLOCK_MONOTONIC_RAW` domain and revision/Splint correlation metadata.
7. Encode protocol frames directly after one reserved four-byte length prefix and
   backfill that prefix, removing the second encoded-body allocation and copy.
8. After wire materialization borrows a live snapshot, move its already-owned
   visible rows into the next semantic-diff baseline instead of cloning every row.

## Results

### Non-graphical daemon

| Metric | Baseline median | Final median | Delta |
|---|---:|---:|---:|
| Output completion | 110.41 ms | 41.83 ms | -62.1% |
| Output processing | 108.72 ms | 31.68 ms | -70.9% |
| RSS after workload/resize | 43.93 MiB | 43.65 MiB | -0.6% |
| Post-output input response | 11.76 ms | 11.73 ms | -0.2% |

The daemon benchmark retained exactly 108,056 semantic terminal updates. The
improvement therefore does not change revision meaning. A record-only
post-change Heaptrack capture fell from 344,203 to 208,866 allocations and from
201,539 to 65,954 temporary allocations; no Heaptrack GUI was invoked.

### Body-free stage attribution and Slice 3 candidates

Disabled instrumentation passed the retained 5-warmup/20-sample interleaved
release gate. The one-sided 95% upper regressions were +1.27% for output
completion, -1.00% for process CPU time, and +1.12% for the small-write lane,
all within the Plan 0010 2%/2%/5% limits. The complete report, including
interleaving order, identities, measured records, bootstrap seed, and 10,000
resamples, is retained at `slice1/instrumentation-overhead.json`.

A traced release overflow/resync workload attributed 17.2 ms to terminal
mutation, 11.5 ms to owned snapshots, 7.6 ms to wire materialization, and
24.4 ms to frame encoding before the Slice 3 changes. This justified optimizing
protocol construction rather than returning to the rejected client snapshot
fast path.

The deterministic byte-identity frame benchmark used a representative 1.28 MiB
frame and 40 iterations. Direct prefix-backfill encoding improved its median
from 70.15 to 65.65 ms (-6.4%). The visible-row ownership change improved traced
wire-materialization median from 48.5 to 44.1 microseconds (-9.0%) and
nearest-rank p95 from 191.2 to 146.8 microseconds (-23.2%). The normal release scrollback/search case
and the bounded overflow/resync case both passed.

### Guarded graphical output

Exact pre-review candidate binaries:

- `splinterm` SHA-256:
  `a2ae81d0f15deedeb4466982fd83fd6b1dbc6c8d63340376a67d67fff3ecfd8d`
- `splinterd` SHA-256:
  `823b24e6a2703f35f613764c5a7053464c1003aebb01c8d16b70e85aa84ee3a8`

The replacement Slice 3 development matrix ran three warmups and ten measured
samples per case. All 39 launches were valid and preserved workspace 8
on unfocused DP-2 with verified cleanup.

| Case | Child-write median | Marker-visible median | CPU ticks median | RSS-after median |
|---|---:|---:|---:|---:|
| Plain | 49.88 ms | 360.73 ms | 13 | 92.75 MiB |
| ANSI | 51.38 ms | 352.26 ms | 13 | 97.32 MiB |
| Unicode | 52.25 ms | 357.30 ms | 15 | 114.66 MiB |

This replaces the superseded pre-Cava-revert client matrix as exact Slice 3
development evidence, but it is not the required 5-warmup/20-sample retained
slice comparison. Post-review trace-integrity hardening changed binary identity
without changing the tracing-disabled Slice 3 paths; therefore a fresh
exact-binary graphical run is still required before closure. The absolute bulk
text, visible latency, CPU, and RSS gates remain unmet.

The corrected traced graphical smoke recorded both daemon and client processes
and every currently instrumented terminal-mutation-to-commit stage. After
review hardening, its multiple same-revision draw commits are correctly rejected
as ambiguous rather than reported as one wire-to-commit interval. Slice 1's
cross-process transaction identity, earlier PTY, input, callback, memory, and
complete queue-wait boundaries remain open. A
permanent guarded real-Cava gate now uses Cava 0.10.7's actual `noncurses`
`synchronized_sync` output with a deterministic bounded PCM FIFO. The final run
advanced through ten distinct client-applied and ten distinct committed
revisions before its timeout. The checked Cava report is explicitly invalid
overall because an attempted second-connection `q` injection was correctly
rejected: the graphical client already owned terminal control. The frame-
advancement observation is valid, while the external-input method is not. Input responsiveness
continues to rely on the existing controller/PTY tests until a graphical-client
input injection mechanism can exercise it without focusing workspace 8.

## Rejected experiments

- **512-byte parse batches:** large apparent graphical gains, but a batch can
  exceed the fixed 256-revision update-history window and force resnapshot.
  Rejected despite favorable timing. A coordinated 512-byte/1,024-revision
  variant preserved all 108,056 updates but regressed daemon output processing
  16.9% (31.68 to 37.03 ms), increased RSS, and did not improve input; it was
  also reverted without a graphical run.
- **Append-only wire history rows:** regressed child-write, visible latency, CPU,
  and RSS. Reverted.
- **SmallVec damage storage:** about 1.3% median daemon improvement with worse p95
  and slightly higher RSS. Reverted.
- **Zero-copy retained visible rows:** inconsistent timing and higher RSS.
  Reverted.
- **Prepared-row scroll copy:** strict correctness predicate rarely matched
  coalesced output and increased variance. Reverted.
- **Print/execute metadata-baseline gating:** preserved semantic tests but changed
  the large baseline layout and regressed daemon output processing 8.6% (31.68
  to 34.39 ms), increased RSS, and worsened completion variance. Reverted.
- **Live-viewport scrollback-clone and incremental display-snapshot removal:**
  passed synthetic tests but was followed by a live Cava lockup in the installed
  `pre-2` package. Reverted conservatively to the previously working client path;
  the daemon/grid optimization remains retained.

## Validation and review

- `python -m pytest tools/benchmark/test_benchmark.py -q`: 23 passed.
- `cargo test --workspace -- --test-threads=1`: passed, including all 16 daemon
  end-to-end tests, pinned Foot fixtures, renderer equivalence, images,
  automation, MCP, protocol, PTY, detach/reattach, overflow, and resync.
- Strict Clippy is not a green repository gate under Rust 1.97: it stops on
  multiple pre-existing warnings in unchanged core, terminal test, daemon policy,
  and automation-client code. No Clippy-only source edits were made.
- Two traced guarded smokes and the 39-run replacement matrix passed with
  cleanup verified and no focus/placement violation. The deterministic Cava run
  also preserved isolation and cleanup while proving live frame advancement.
- Earlier terminal/grid reviews found and closed the multi-row row-ID ordering
  blocker and added exact regression assertions. The fresh Slice 1/3 reviews
  found no encoder or visible-row ownership defect. Their evidence-integrity
  findings were fixed by rejecting ambiguous revision correlations, using true
  nearest-rank p95, detecting saturated/reused traces, baselining Cava progress
  after readiness, retaining the exact raw artifacts, and narrowing Slice 1
  claims to the implemented partial path.

## Remaining work

Partial Slice 1 instrumentation and two measured Slice 3 copy reductions are
now implemented, but neither Slice 1 nor Slice 3 closure is claimed: the required
retained candidate/control statistics, receive-to-prepared-frame proportional
gate, and absolute attached-client gates remain open. Exact body-free traces and
regenerated nearest-rank summaries are retained under `slice1/` and `slice3/`;
all 39 raw matrix records are under `slice3/pre-review-candidate-matrix/raw/`, and
`SHA256SUMS` covers every artifact. Current medians still miss Plan 0010
gates for child write, marker visibility, CPU, and RSS. Startup, input,
retention, scrollback, resize, scale, pane, image, fuzz, package, and final
30-sample comparative closure remain outstanding.
