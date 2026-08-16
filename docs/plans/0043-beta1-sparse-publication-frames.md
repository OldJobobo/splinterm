# Plan 0043: Beta1 sparse terminal publication frames

- **Status:** Graphical aggregate retention gate failed; daemon follow-up required
- **Date:** 2026-08-15
- **Release decision:** Do not tag `0.1.0-beta.1` until this plan passes its
  non-graphical, graphical, review, integration, and release gates
- **Release line:** `maint/0.1`
- **Depends on:** accepted and integrated [Plan 0042](0042-beta1-wide-splint-grid.md)
  at maintenance commit `ba8f1cd28f1aca25d83f198c593832b829601a6c`, the retained
  [Plan 0011](0011-burst-output-memory-retention.md)
  no-go evidence, and the
  rejected [Plan 0012](0012-bounded-compact-publication-frames.md) experiment
- **Behavioral authority:** Foot 1.27.0 commit
  `3c5b584b0eafa772eb4376fb6eaf6643399e190e`

## Decision

Replace the current first-party subscriber path that coalesces multiple terminal
producer batches against one latest compact snapshot and then materializes one
large client update. Preserve exact producer boundaries in genuinely sparse,
privately owned publication frames and seal them before semantic, encoded,
queue, or memory limits are crossed.

A sparse frame owns only the exact changed rows and identities, ordered scroll
operations, bounded history delta, and terminal metadata required to reconstruct
its revision interval. It must not retain a full visible-grid or history
checkpoint per queued frame. A delayed subscriber may collapse to exactly one
latest compact snapshot only after deterministic resynchronization precedence is
established.

This plan does not reduce PTY read size, queue capacity, scrollback, renderer
caches, glyph caches, SHM buffers, or the validated Beta1 grid. It does not use
an allocator change, `malloc_trim`, routine resynchronization, or a larger memory
ceiling to manufacture a benchmark win.

## Why this succeeds Plan 0012 instead of editing it in place

Plan 0012 correctly identified the retained-memory and responsiveness defect,
but its implementation contract froze the Alpha3 protocol: `240x80` grids,
8 MiB frames, no public protocol change, and no aggregate terminal transaction.
Plan 0042 changes that authority for Beta1 to an exact-version `480x128` grid,
16 MiB individual wire frames, bounded 32 MiB aggregate terminal transactions,
and explicit queued/in-flight terminal-publication memory admission.

Plan 0042 does not change `crates/splinterd/src/live.rs`'s semantic coalescing
path. Its transport transaction can carry one oversized logical terminal event,
but it does not prevent many producer batches from becoming one expensive
logical client update. The original Plan 0012 diagnosis therefore remains open
while its literal limits and protocol-compatibility assumptions are stale.

Keep all Plan 0011 and Plan 0012 artifacts immutable. This plan is the current
implementation and acceptance authority after Plan 0042 is integrated.

## Preserved baseline defect

The accepted Plan 0011 candidate improved daemon retention but regressed the
complete graphical system under the 5,000-line mixed-output/clear workload:

| Process or metric | Clean control | Plan 0011 candidate | Result |
| --- | ---: | ---: | ---: |
| Daemon retained RSS | 34.11 MiB | 21.17 MiB | 37.9% better |
| Client retained RSS | 36.31 MiB | 56.84 MiB | 56.5% worse |
| Aggregate retained RSS | 70.45 MiB | 78.05 MiB | 10.78% worse |
| CPU ticks | 19.0 | 76.5 | worse |
| Marker latency | 396.13 ms | 615.86 ms | worse |

The current compact mailbox stores ordered `TerminalUpdate` batches plus one
latest compact snapshot. The consumer drains the batches and snapshot together,
then first-party wire publication reduces the complete interval into one event.
This keeps delayed-subscriber snapshot high water at one, but can erase the
bounded production boundaries and produce a large client allocation and render
burst.

The first Plan 0012 experiment proved that preserving boundaries can restore a
contiguous fast stream. It was correctly rejected because each sealed frame
still owned a full compact checkpoint, receiver-local frames escaped semantic
queue accounting, encoded-size and identity proofs were incomplete, resync could
lose a trailing exit, exact reconstruction was not proven, the delayed case was
intermittent, and default-off instrumentation missed its overhead gate. The
runtime experiment was rolled back.

## Authority and integration order

1. Plan 0042 must pass its separately approved packaged graphical acceptance and
   be integrated into `maint/0.1`.
2. Create the implementation branch from the resulting reviewed
   `origin/maint/0.1`; do not transplant runtime work from an Alpha3 or
   pre-transaction base.
3. Record exact Alpha3.3 control, integrated-Plan-0042 baseline, and Plan 0043
   candidate source and binary identities.
4. Integrate an accepted Plan 0043 into `maint/0.1`, then forward-port it to
   `main` through a separate reviewed branch.
5. Reconcile shared `TODO.md`, status, package, and release files only at the
   serialized Beta1 integration boundary.

Plan 0041's theme-role work remains a separate branch. Do not combine theme,
wide-grid, and sparse-publication implementation commits merely because all are
Beta1 work.

## Fresh baseline gate

Before changing runtime behavior, rerun the unchanged 5,000-line mixed/clear
workload against:

- published `v0.1.0-alpha3.3` as the release control;
- the exact integrated Plan 0042 Beta1 baseline without sparse frames; and
- later, the sparse-frame candidate from the same Plan 0042 base.

Use the existing randomized order, warmups, measured samples, process inclusion,
settle points, procfs attribution, and workload seed unless a documented harness
correctness defect requires a separately reviewed fix. Record per-process RSS,
PSS, private-anonymous bytes, CPU ticks, marker latency, frame count and size,
subscriber overflow/resync, and terminal publication memory high water.

**Gate:** either the current Beta1 baseline reproduces the oversized cross-batch
materialization and aggregate/client regression, or this plan stops for a scope
decision. Historical evidence alone does not authorize a new architecture when
the current release-line behavior no longer reproduces the defect.

No graphical comparison is permitted at this gate. The first reproduction must
be headless or use retained historical graphical evidence only.

### Fresh gate result

The 2026-08-16 headless matrix compared exact Alpha3.3 commit `0c42767` with
integrated Plan 0042 commit `ba8f1cd` using identical harness source, two
warmups, ten randomized measured samples per variant, and the unchanged
5,000-line mixed/clear workload. Every integrated-baseline sample retained one
latest snapshot, emitted zero resyncs, and materialized up to 64 producer batches
and approximately 15,554 terminal updates into one subscriber event. Median
baseline marker latency was 127.23 ms and private-anonymous growth was 8.28 MiB.
The attribution gate therefore reproduces and sparse-frame implementation may
proceed. Evidence: [fresh headless baseline](../benchmarks/artifacts/2026-08-16-plan0043-fresh-baseline/summary.md).

## Sparse frame contract

Each private `SparsePublicationFrame` or equivalent owns a contiguous interval
with:

- process incarnation, base revision, and final revision;
- dimensions, active screen, history generation, and history-policy identity;
- exact changed visible rows with stable row identities and compact cells;
- ordered scroll operations that preserve their relationship to row patches;
- bounded append/trim or reset history data and no unrelated history body;
- cursor, title, modes, palette, viewport, image-metadata, clear, reflow, and
  dimension transitions required by the interval; and
- checked semantic-owned and estimated/materialized encoded-byte accounting.

A streaming sparse frame must not own a complete compact visible-grid or history
snapshot. A producer batch that changes the complete grid may legitimately own
all changed rows, but that ownership is damage-derived and remains subject to
all byte ceilings; it must not silently become the default representation for
small damage.

Rows and compact cell values should transfer into protocol DTOs or client state
where existing ownership permits. Do not clone a full checkpoint merely to
simplify queue or materialization lifetimes.

## Subscriber state machine

Every first-party terminal subscriber must have explicit states equivalent to:

1. **Streaming** — a bounded queue of contiguous sparse frames plus at most one
   producer wake token.
2. **Resync pending** — queued sparse frames have been released under accounting
   and exactly one latest compact snapshot is authoritative.
3. **Exit pending** — the reserved exit state outranks ordinary updates and
   survives streaming-to-resync transitions.
4. **Closed** — all frames, snapshots, permits, reservations, and metrics are
   released.

Receiver-local work is part of the same semantic and byte admission as mailbox
work. Moving a frame across an internal channel must transfer its lease; it must
not release admission while retaining the bytes.

For a fast subscriber, every delivered interval is contiguous, no
`SubscriberStalled` occurs in the closure workload, and incrementally applied
state exactly matches an authoritative final snapshot. For a delayed subscriber,
saturation deterministically establishes resync before discarding intervals,
retains at most one latest compact snapshot, and never loses a trailing exit.

## Sealing and merge contract

Adjacent sparse frames may merge only when all of these are proven:

- left final revision equals right base revision;
- incarnation, history generation/policy, dimensions, and active-screen
  transitions remain exact;
- scroll and row-patch order remains reconstructable;
- duplicate row patches reduce to the newest exact row without crossing an
  intervening operation that observes the older identity;
- append/trim/reset and image transitions remain unambiguous;
- row-patch and scroll counts remain within negotiated and absolute Beta1 limits;
- compact semantic bytes remain admitted beneath the per-subscriber and
  per-Splint queue ceilings;
- exact or conservative encoded size remains below the selected logical-event
  seal threshold; and
- materialization cannot create multiple queued aggregate transactions.

The 32 MiB transport transaction is a fail-closed way to carry one eligible
logical terminal event after materialization. It is not permission to merge
semantic frames until they approach 32 MiB. Prefer several contiguous ordinary
frames when that reduces peak client allocation and application latency.

If a single producer batch exceeds the seal threshold, split only at an exact
terminal parser/update checkpoint inside the already-read buffer. Do not add PTY
read syscalls or globally reduce read size.

## Quantitative memory boundaries

Inherit Plan 0042's accepted ceilings without widening them:

- 16 MiB per individual wire frame;
- at most one 32 MiB aggregate terminal transaction in flight per connection;
- no more than 16 MiB queued terminal-publication payload per subscriber outside
  that one admitted aggregate transaction;
- 64 MiB aggregate queued terminal-publication payload per Splint;
- 256 MiB aggregate queued/in-flight terminal-publication payload in the daemon;
- 64 MiB semantic-plus-prepared terminal state per graphical pane; and
- 512 MiB aggregate terminal presentation state per Window.

Sparse compact ownership, materialized DTO ownership, JSON/base64 framing,
outbound queues, receiver-local queues, decode buffers, and client application
queues must be attributed to the applicable boundary. Count and byte limits are
both authoritative. Release accounting through RAII on delivery, resync, close,
write failure, decode failure, cancellation, unwind, and process exit.

## Milestone 1 — current-line attribution and headless regression

Expected areas:

- `crates/splinterd/src/live.rs`
- `crates/splinterd/src/main.rs`
- `crates/splinterd/tests/`
- `tools/performance/`
- `tools/benchmark/test_benchmark.py`

Work:

- retain default-off compact-batch attribution and add logical-event encoded
  size, client application-time, queue-byte, and resync-reason evidence where
  missing;
- add a no-Wayland production-socket test that subscribes before the mixed/clear
  workload, validates every update, applies every interval to a bounded test
  client model, and compares complete visible/history state with an
  authoritative final snapshot;
- prove the test fails or exposes the measured coalescing boundary on the
  integrated Plan 0042 baseline for the expected reason; and
- prove instrumentation satisfies the existing default-off overhead confidence
  gate.

Do not implement sparse frames in this milestone.

## Milestone 2 — sparse producer frames

Expected primary area: `crates/splinterd/src/live.rs`.

Work:

- capture exact damage-derived compact ownership at producer batch boundaries;
- represent rows, ordered scrolls, history deltas, and metadata without a full
  checkpoint per frame;
- add checked semantic-byte accounting and RAII leases; and
- prove each standalone frame reconstructs its exact final revision.

Focused tests cover one-row damage, complete-grid damage, clear, reflow,
dimensions, normal/alternate screen transitions, palette/mode/title/cursor,
images, history append/trim/reset, and a single oversized producer batch.

### Milestone 2 implementation record — 2026-08-16

- Each compact producer boundary now owns a private `SparsePublicationFrame`:
  ordered terminal updates, damage-selected final rows, a bounded append or
  replacement history delta, and row-free final metadata. Producer snapshots are
  ephemeral capture inputs and are not installed in the mailbox. The receiver
  owns only its current visible-grid materialization base; ordinary queued frames
  do not own a visible-grid or history checkpoint, while full damage owns only
  the complete state it actually changed.
- Checked semantic-byte attribution charges owned vector capacities, nested
  update/event bodies, compact rows and composed strings, history deltas, title,
  and image metadata. The existing `PendingFrameLease` now admits, merges,
  materializes, and releases those bytes through the same RAII paths as
  batch/count attribution.
- Standalone reconstruction tests cover one-row damage, ordered multi-scroll
  updates, bounded append/trim, clear/reset, normal-screen reflow, `480×128`
  complete-grid damage, dimensions, normal/alternate screen transitions,
  palette/mode/title/cursor metadata, image metadata, and one 92,544-revision
  oversized producer batch.
- Independent review found the first staged implementation still retained one
  latest compact snapshot and did not enforce cross-frame continuity or charge
  spare vector capacity. The reviewed fixes remove ordinary snapshot-slot
  ownership, materialize and merge sparse tails transactionally at the receive
  boundary, fail closed on incarnation/revision mismatches, and charge owned
  capacities plus nested update event bodies.
- The production-socket mixed/clear test exposed a test-model error under a newly
  observed append boundary: wire `omitted_oldest_rows` describes the delta rows,
  while the reconstructed snapshot must recompute omission from its retained
  bounded history. The bounded client model now does so; ten consecutive focused
  repetitions passed without resync or invalid state.
- Validation passed: `cargo test --workspace --all-targets -- --test-threads=1`,
  `cargo clippy --workspace --all-targets -- -D warnings`, 63 benchmark harness
  tests, formatting, and `git diff --check`. The fresh read-only review chain
  `1cca02fc` → `dd64a6ef` → `b34efcf6` found the staged snapshot retention,
  continuity/accounting, and append-metadata defects, verified each bounded fix,
  and returned **CLEAN** with no residual risk. No graphical test, installation,
  package replacement, oracle refresh, or release action was performed.

## Milestone 3 — bounded queue, sealing, and resync

Work:

- implement protocol-aware merge and seal decisions;
- keep streaming frames inside one counted-and-byte-bounded mailbox;
- preserve wake-token and reserved-exit semantics;
- transition delayed/saturated subscribers to one latest compact snapshot; and
- materialize each sealed logical frame once at the writer boundary.

Focused tests cover one-under, exact, and one-over every count/byte limit; fast,
delayed, capacity-one, multiple-subscriber, receiver-drop, writer-failure,
cancellation, resync, trailing-exit, and closed-state behavior.

### Milestone 3 implementation record — 2026-08-16

- The compact mailbox is now a counted `VecDeque` of sealed sparse chunks rather
  than one indefinitely growing tail. Adjacent contiguous producer frames merge
  only while the chunk remains below eight frames and 4 MiB of checked semantic
  ownership. The authoritative per-subscriber frame count remains independent
  of chunk count.
- One wake token still represents the complete nonempty mailbox. The receiver
  atomically takes all sealed chunks covered by that token, validates continuity,
  and materializes every sparse frame once in one writer-boundary state pass. A
  producer that races an emptied mailbox emits a fresh token. The extra channel
  slot remains reserved for process exit.
- Sparse semantic ownership is admitted through RAII against fixed 16 MiB
  per-subscriber and 64 MiB per-Splint limits. Sparse queues and materialized
  outbound terminal transactions share one process-wide 256 MiB authority, so
  the two stages cannot each consume an independent daemon ceiling.
- Count, byte, continuity, and channel overflow clear all queued sparse ownership
  before resync becomes authoritative. Receiver drop, cancellation, closed
  channels, materialization failure, and successful delivery release the same
  leases. Optional publication metrics do not control enforcement.
- Focused coverage proves seal boundaries and one-wake behavior, exact local and
  shared byte limits, overflow release and resync precedence, capacity-one exit
  reservation, multiple subscribers, concurrent receiver drop, append-history
  reconstruction, and fast-reader behavior. The production-socket 5,000-line
  mixed/clear reconstruction gate passed ten consecutive runs without resync.
- Validation passed: `cargo test --workspace --all-targets -- --test-threads=1`,
  `cargo clippy --workspace --all-targets -- -D warnings`, 63 benchmark harness
  tests, `cargo fmt --all -- --check`, and `git diff --check`. Fresh read-only
  review `3b3a8228` blocked acceptance pending explicit proof of the cross-drain
  append-delta contract and admission before writer-side materialization and
  encoding. Both findings are now addressed: a preexisting-history regression
  proves separately delivered append tails reconstruct the exact bounded client
  history, and compact receive acquires a conservative 32 MiB process-wide lease
  after its wake but before sparse materialization, retaining it through wire
  encoding and delivery. Saturation fails to resync before materialization and
  releases queued sparse ownership. This test also exposed and fixed eager
  `bool::then_some` construction that released a lease after failed admission;
  lazy construction now preserves exact counter balance. Full serialized tests,
  Clippy, formatting, benchmark tests, and ten production-socket reconstruction
  runs pass after the fixes. Follow-up review and randomized candidate
  memory/latency attribution remain pending. No graphical test, installation,
  package replacement, oracle refresh, push, or release action was performed.

### Candidate attribution stop-loss — 2026-08-16

The same randomized two-warmup, ten-sample, seed-43 headless comparison was run
against integrated Plan 0042 and two sparse candidates. The existing runner is a
defect-reproduction gate, so it exits 1 and writes `valid: false` when the
candidate successfully stops reproducing the old 64-batch predicate; all 20
measured cases completed and the candidate interpretation below uses the exact
recorded metrics rather than treating that expected predicate result as a
runtime failure.

| Sparse experiment | RSS/PSS growth | Private-anon growth | CPU ticks | Marker latency | Batch HWM | Update HWM | Resync |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Sealed sparse chunks | 23.33 MiB | 23.26 MiB | 14 | 119.88 ms | 8 | 1,949 | 0 |
| Producer-frame update coalescing | 12.60 MiB | 12.54 MiB | 13 | 124.13 ms | 8 | 8 | 0 |

The second experiment preserves all ordered damage and terminal events while
owning one contiguous publication update per producer frame. It substantially
improved the first sparse result, but its retained daemon growth remained about
40% above the paired integrated baseline's 8.97 MiB. That triggered the plan's
stop-loss. The approved bounded follow-up implemented true sealing: each
up-to-eight-frame chunk now owns one composed final sparse representation while
retaining every producer-frame count lease and exact consolidated byte
admission.

The true-sealing candidate passed the same randomized gate with 5.78 MiB
RSS/PSS and 5.72 MiB private-anonymous growth versus the paired integrated
baseline's 9.73 MiB and 9.67 MiB: about 40.6% lower retained growth. Sparse queue
semantic high water fell from 9.22 MiB in the update-only experiment to 3.16 MiB.
Batch high water was 8, materialized update high water was 1, CPU was 13 ticks
versus 14, marker latency was 121.19 ms versus 132.48 ms, and all samples had
zero resync. Full serialized workspace tests, warnings-denied workspace Clippy,
63 benchmark tests, formatting, `git diff --check`, and ten consecutive
production-socket reconstruction runs pass. The measurements and exact raw
records are retained under
`docs/benchmarks/artifacts/2026-08-16-plan0043-sparse-candidate*/`. Fresh
true-sealing review `3e52e7c7` returned **CLEAN** with no blocker or fix worth
doing now; it accepted sparse composition, per-frame count ownership,
consolidated local/Splint/daemon byte accounting, exit/resync precedence, and
the candidate evidence interpretation.

## Milestone 4 — client ownership follow-up, only if attributed

After sparse frames pass correctness and daemon ownership gates, measure client
retention and application latency. Change client ownership only for a named,
measured remaining class. Permitted changes are bounded ownership transfer,
decode-buffer reuse, or release of a proven rare oversized temporary.

Do not rewrite the renderer, change allocator, clear caches broadly, or weaken
Plan 0042 presentation-state bounds.

## Non-graphical acceptance

Mandatory correctness:

- every emitted update validates against the preceding reconstructed state;
- complete final visible rows, row identities, history generation/content,
  dimensions, cursor, modes, palette, images, and active screen match the
  authoritative snapshot;
- zero fast-subscriber overflow, routine resync, invalid frame, lost exit, or
  client termination;
- delayed snapshot high water remains one per subscriber; and
- every sparse/snapshot/DTO/transaction ownership gauge returns to zero after
  drain and close.

Mandatory performance and memory:

- compare candidate medians against both packaged Alpha3.3 and integrated Plan
  0042 controls;
- for daemon, client, and aggregate RSS/PSS/private-anonymous retained growth,
  “no material regression” means the candidate may not exceed either control by
  more than the greater of 3% of that control median or 1 MiB, evaluated per
  metric;
- randomized median CPU ticks and marker latency may not exceed either control
  by more than 3%;
- input, resize, redraw, and idle behavior retain their accepted Plan 0042
  correctness and bounded-work contracts, proven by an unchanged graphical
  client/renderer production diff, the complete serialized workspace, focused
  daemon input/resize/publication-wake tests, and Plan 0042's accepted packaged
  graphical evidence; and
- retain Plan 0012's release preference of at least 40% lower aggregate retained
  growth than control without converting that preference into a mandatory
  completion threshold.

The memory tolerance is an acceptance boundary for repeated randomized medians,
not permission to ignore correctness failures, resync, leaks, unbounded growth,
or a clearly attributed regression. It was explicitly approved after exact
commit `2c5ac9f` eliminated the prior 26% daemon regression and the remaining
0.80% aggregate difference was attributed to overlapping client measurements,
with only a 2 KiB candidate daemon median difference from Alpha3.3.

Run focused suites after each milestone, then the complete serialized workspace,
Clippy with warnings denied, formatting, benchmark harness tests, package and
release-tooling checks, and `git diff --check`. Record exact commands, counts,
source identities, and any isolated rerun without weakening a gate.

## Graphical evidence boundary

Graphical work requires one separate approval for the complete bounded matrix.
Only after coherent implementation, serial validation, parent diff inspection,
and fresh read-only review:

1. verify workspace 8 / DP-2 isolation and record the original focus;
2. install or launch only the exact approved candidate without replacing the
   packaged client unless separately authorized;
3. run one guarded Splinterm smoke;
4. abort and clean up on any placement, focus, cardinality, identity, lock
   overlay, input, or cleanup violation;
5. after a valid smoke, run randomized Alpha3.3 control, integrated-Plan-0042
   baseline, and sparse-frame candidate blocks with retained per-process
   attribution; and
6. run Foot/Kitty/Ghostty comparisons only after Splinterm passes aggregate,
   responsiveness, and correctness gates.

Restore focus, workspace, monitor, package state, runtime processes, and test
artifacts as declared by the approved sequence.

### Graphical gate result — 2026-08-16

The separately approved guarded sequence used workspace 8 / DP-2, preserved the
original Foot focus on workspace 1 / DP-1, and launched only isolated exact
binary sets. The commit-`0a56dbe` 500-line smoke passed. The subsequent seed-4304
matrix completed two warmups and ten measured 5,000-line mixed/ANSI/Unicode
samples per variant with zero isolation or cleanup failure.

| Variant | Application RSS growth | PSS growth | Private-anon growth | Daemon RSS growth | Client RSS growth | Marker latency | CPU ticks |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Packaged Alpha3.3 | 28.66 MiB | 12.11 MiB | 10.87 MiB | 6.70 MiB | 21.95 MiB | 373.66 ms | 22.5 |
| Integrated Plan 0042 | 28.76 MiB | 12.36 MiB | 11.14 MiB | 6.75 MiB | 21.99 MiB | 369.12 ms | 21.5 |
| Plan 0043 candidate | 36.27 MiB | 19.69 MiB | 18.43 MiB | 14.57 MiB | 21.68 MiB | 379.63 ms | 23.0 |

Execution was valid, but acceptance failed. Candidate application RSS growth was
26.55% worse than Alpha3.3 and 26.09% worse than integrated Plan 0042. Client
retention slightly improved; the entire material regression is daemon-side.
Foot, Kitty, and Ghostty comparators were therefore skipped exactly as approved,
because their gate required Splinterm to pass aggregate retention first.

All test windows and isolated processes were removed. Workspace 8 is empty,
unrelated client addresses are unchanged, original focus is restored, and
`pacman -Qkk splinterm` reports 56 files with 0 alterations. Exact binaries,
raw records, randomized order, smoke, cleanup state, summaries, and checksums are
retained under
`docs/benchmarks/artifacts/2026-08-16-plan0043-packaged-graphical/`.
Plan 0043 cannot proceed to final release review or integration until this
measured daemon retention class is corrected and the graphical gate is rerun.

### Paced daemon diagnosis — 2026-08-16

A matched renderer-free production-socket profile exercised the identical test
source, workload, real 33 ms daemon cadence, final-marker condition, and resync
rejection on integrated Plan 0042 and Plan 0043. The existing production-socket
reconstruction regression separately proves exact final state and zero resync.
Heaptrack measured a Plan 0042 heap peak of 3.64 MiB and a Plan 0043 peak of
7.86 MiB. Both leaked only 62.22 KiB. Plan 0043 attributed 6.70 MiB, or 85.2%
of its peak, to allocations rooted at `SparsePublicationFrame::capture`;
materialization and compact-to-live conversion contributed only about 0.10 MiB
and 0.12 MiB at peak.

The supported conclusion is deliberately bounded: sparse capture creates the
additional live heap peak and is the leading source to remove before the
next graphical run. Equal leak totals exclude a differential retained-allocation
leak. Because the profile does not sample live heap at the exact post-drain RSS
timestamp, allocator page retention is a plausible correlation rather than a
proven cause of every byte in the graphical plateau. Moving capture ownership
and widening only the frame-count or per-chunk byte sealing spans each improved
the diagnostic median by at most 1.3 MiB and were rejected.

The next bounded implementation must avoid allocating a complete successor
sparse frame before merge. After admission and complete prevalidation, it should
compose successor damage directly into one reusable mailbox-tail representation,
reuse visible/history row buffers, keep ordered updates separately, and preserve
one count lease per producer frame, exact semantic-byte consolidation,
continuity, exit precedence, and fail-closed resync. This is an ownership-shape
change, not authorization for an allocator, renderer, protocol, or admission
ceiling change. Diagnostic harness patches, plain peak attribution, compressed
heaptrack traces and reports, logs, summaries, and checksums are retained under
`docs/benchmarks/artifacts/2026-08-16-plan0043-paced-daemon-diagnosis/`.
Fresh read-only review `dbd49ebd` initially blocked on harness parity and causal
overstatement; corrected matched evidence and bounded wording received a CLEAN
follow-up from `2c5f9be7`.

### Reusable direct-tail correction — 2026-08-16

The corrected mailbox owns one reusable sparse row/history tail for its complete
admitted sequence, bounded by the unchanged 64 producer-event and 16 MiB
subscriber ceilings. A validated successor capture owns metadata, ordered update
summaries, damage indices, and history ranges, but no duplicate visible or
history row bodies. Explicit `clone_from` implementations reuse row, cell, and
composed-string capacities; bounded history replacement and append paths reuse
retained row buffers. Producer count and semantic-byte leases remain per frame
and consolidate to exact final ownership. Ordered summaries remain separate in
the queue and coalesce once, with exact vector capacities, during receiver
materialization into one wire update.

Prevalidation completes before mutation. Failed validation leaves the tail
unchanged; failed admission owns nothing; any later checked accounting failure
clears the mailbox before fail-closed resync. Existing continuity, cancellation,
receiver-drop, writer-failure, and process-exit precedence remain unchanged.
Focused tests prove visible/history buffer reuse, transactional prevalidation,
truthful internal-summary metrics, and one delivered update.

A seed-4304 headless matrix ran two warmups and ten measured 5,000-line cases per
variant against integrated Plan 0042. All 20 measured cases completed with zero
resync.

| Variant | RSS/PSS growth | Private-anon | CPU ticks | Marker latency | Events | Batch HWM | Ordered-summary HWM |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Integrated Plan 0042 | 8.40 MiB | 8.27 MiB | 14.0 | 125.29 ms | 69.0 | 64.0 | 15,553.5 |
| Reusable direct tail | 6.05 MiB | 5.95 MiB | 13.0 | 119.79 ms | 67.5 | 64.0 | 64.0 |

The candidate is 28.0% below integrated Plan 0042 aggregate retention while also
improving CPU and marker latency. This passes the mandatory below-control gate;
the retained 40% reduction remains the explicitly non-mandatory release
preference. The generic attribution runner records `valid: false` and exits 1
because its old defect predicate requires both variants to reproduce the
superseded 64-batch checkpoint defect; every measured case itself completed with
`error: null`.

Ten consecutive production-socket reconstruction runs, the complete serialized
workspace/all-target suite, warnings-denied workspace Clippy, 63 benchmark tests,
formatting, and `git diff --check` passed. Raw records, exact binary identities,
rejected intermediate shapes, summaries, and checksums are retained under
`docs/benchmarks/artifacts/2026-08-16-plan0043-direct-tail-candidate/`.
This is headless acceptance only. Fresh read-only review `42a73b1e` returned
**CLEAN**.

### Reusable direct-tail graphical rerun — 2026-08-16

The separately approved guarded rerun tested exact commit `2c5ac9f` on isolated
workspace 8 / DP-2. The candidate smoke passed, followed by the seed-4304 matrix
with two warmups and ten measured 5,000-line cases per Alpha3.3, integrated Plan
0042, and candidate variant. All 36 records were valid and cleanup-verified.

| Variant | Application RSS | PSS | Private-anon | Daemon RSS | Client RSS | Marker latency | CPU ticks |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Alpha3.3 | 28.78 MiB | 12.30 MiB | 11.11 MiB | 6.78 MiB | 22.07 MiB | 369.85 ms | 22.0 |
| Integrated Plan 0042 | 29.03 MiB | 12.46 MiB | 11.20 MiB | 6.76 MiB | 22.22 MiB | 367.30 ms | 21.0 |
| Reusable direct tail | 29.01 MiB | 12.50 MiB | 11.31 MiB | 6.79 MiB | 22.28 MiB | 370.91 ms | 21.5 |

The prior daemon regression is corrected: candidate application RSS is 0.05%
below integrated Plan 0042, and daemon RSS is only 0.32% above it. Candidate
latency is 0.98% slower and CPU differs by half a tick. The approved
Foot/Kitty/Ghostty comparator block therefore ran; all 36 comparator records were
also valid and cleanup-verified.

Fresh final release review `2c5dc855` returned **BLOCKED**. Packaged
Alpha3.3, not integrated Plan 0042, is the exact release control. Candidate
application RSS is 0.80% above Alpha3.3, so it does not satisfy the unqualified
“below” gate. Candidate client RSS/PSS/private-anonymous are also above
Alpha3.3, violating the unqualified “no worse” gate. Overlapping ranges cannot
supply a tolerance the plan does not define. The comparator block was
operationally valid and cleanup-verified, but it ran before this baseline
interpretation was corrected and cannot count as release acceptance evidence.
The minimum next action identified by review was one newly authorized, bounded,
attributed retention correction followed by a fresh three-variant graphical
matrix. That attribution was authorized, but read-only scout `7b2b615f` returned
**NO ATTRIBUTED CORRECTION**: candidate daemon median exceeds Alpha3.3 by only
2,048 bytes while the client accounts for 223,232 bytes of the aggregate gap,
and individual samples do not show a stable candidate daemon excess. The prior
sparse-capture peak source has already been removed. Changing materialization
state, sparse capacities, or renderer/client ownership would therefore be
speculative or outside the bounded authorization. No production correction was
made.

The user then explicitly approved an acceptance-policy amendment: repeated
randomized daemon, client, and aggregate RSS/PSS/private-anonymous medians are
accepted when they remain within the greater of 3% or 1 MiB of both packaged
Alpha3.3 and integrated Plan 0042. Under that declared tolerance, every candidate
memory median passes both controls. The original 26% regression still fails by a
wide margin, while the corrected 0.80% aggregate difference and client deltas
are accepted as measurement-scale variance.

The applicable responsiveness allowance is also explicit: randomized median CPU
and marker latency may not exceed either control by more than 3%. Candidate CPU
is below Alpha3.3 and 2.38% above Plan 0042; marker latency is 0.29% above
Alpha3.3 and 0.98% above Plan 0042. Input, resize, redraw, and idle are inherited
qualitative contracts rather than unmeasured quantitative claims: Plan 0043
changes no `splinterm` client/renderer production file relative to integrated
Plan 0042, whose packaged graphical input/resize/redraw evidence is accepted.
The complete serialized candidate workspace additionally passes focused daemon
input/resize/nonblocking and publication-wake tests. No periodic publisher,
client redraw loop, renderer path, or idle timer was added.

The comparator prerequisite is therefore satisfied and its valid,
cleanup-verified records count as acceptance evidence. Review `2c5dc855` remains
the correct verdict under the superseded unqualified wording. Initial amended
review `03b1a7c3` requested this explicit allowance and evidence link. Follow-up
final review `04deabf3` returned **CLEAN**, confirming the amended memory and
responsiveness arithmetic, inherited qualitative evidence, and comparator
eligibility. Plan 0043's implementation and evidence acceptance is complete;
push and protected-branch integration remain separate approval boundaries.

All test windows and isolated processes were removed, the original Vesktop focus
on workspace 6 / DP-3 was preserved, workspace 8 is empty, temporary builds were
removed, and Pacman reports 56 package files with 0 alterations. Raw records,
randomized order, exact binary hashes, summaries, comparator data, cleanup state,
and checksums are retained under
`docs/benchmarks/artifacts/2026-08-16-plan0043-direct-tail-graphical/`.

## Review and stop-loss

Use one writer for the implementation worktree. Require one fresh read-only
architecture/security review after Milestone 3 and one fresh final release review
after exact provenance, serial validation, and graphical evidence are assembled.

Stop and request a scope decision when:

- the fresh baseline does not reproduce the attributed defect;
- two genuinely sparse-frame experiments fail the aggregate gate;
- exact fast-path behavior requires routine resync;
- more than one delayed-subscriber snapshot is required;
- bytes move into an unaccounted daemon, receiver, client, or allocator class;
- correctness requires lowering existing limits or widening Plan 0042 ceilings;
- the proposed fix becomes a renderer or allocator rewrite; or
- the 40% release preference would need to be weakened to claim success.

## Beta1 completion record

Plan 0043 is complete only when the repository records:

- exact control, integrated Plan 0042 baseline, and candidate identities;
- fresh attribution and headless complete-state reconstruction evidence;
- fast, delayed, saturated, closed, exit-pending, and multi-subscriber evidence;
- repeated non-graphical RSS/PSS/private-anonymous and overhead evidence;
- complete serial validation and fresh architecture/security review;
- separately approved graphical smoke and randomized comparative evidence;
- final release review with no unresolved blocker; and
- reviewed integration into `maint/0.1` and forward-port to `main`.

Candidate construction, promotion, package replacement, pushing, AUR
publication, and release publication remain separate approval boundaries. A
daemon-only memory win, a client regression, an unaccounted queue, or a failed
comparative gate remains a Beta1 no-go.
