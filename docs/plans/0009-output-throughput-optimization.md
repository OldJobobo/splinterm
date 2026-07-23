# Plan 0009: PTY output throughput and update-pipeline optimization

- **Status:** Planned — recommended before Phase 5 implementation
- **Roadmap:** Phase 4.1 — output throughput stabilization
- **Evidence:** [five-terminal graphical output benchmark](../benchmarks/artifacts/2026-07-23-five-terminal-output/README.md)
- **Benchmark method:** [terminal benchmark suite plan](../benchmarks/terminal-benchmark-plan.md)
- **Foundation:** [Plan 0001](0001-terminal-kernel.md),
  [Plan 0002](0002-omarchy-terminal-mvp.md), and
  [Plan 0007](0007-phase4-mcp-adapter.md)

## Decision

Pause feature expansion after safely checkpointing the completed Phase 4 work,
then execute this plan before beginning Phase 5 terminal image protocols.

Do not optimize inside an unreviewed mixed Phase 4 worktree. First review and
split the completed protocol, daemon, MCP, packaging, and documentation changes
into coherent commits. The performance work should then start from that known
baseline as a separate, measurable change series.

## Problem statement

In the corrected 80x24 graphical output matrix, a benchmark child writing about
162–283 KiB blocks for a median 1.28–1.38 seconds under Splinterm. The same
boundary completes in roughly 1–6 milliseconds under Foot, Kitty, Ghostty, and
Alacritty. Splinterm reaches the screenshot-detected visible marker in roughly
1.87–1.89 seconds.

The screenshot boundary is an approximation, but the child-write boundary is a
direct backpressure observation. Approximately two thirds of Splinterm's
observed end-to-end delay has already accumulated before the child finishes its
write. This is a foundational output-throughput problem rather than solely a
Wayland presentation problem.

Current evidence suggests CPU-bound amplification across the daemon-owned PTY,
terminal kernel, update history, subscription, and client paths:

1. `splinterd` reads into a 16 KiB buffer but divides output into 256-byte parse
   and publication batches.
2. `Terminal::advance` currently commits a terminal revision for each parsed
   action, commonly each printable character.
3. Each 256-byte batch clones retained updates and publishes a live event.
4. The protocol subscription path obtains a terminal snapshot for each live
   update event before building a wire update.
5. Bounded daemon and client queues can turn an output burst into resnapshot and
   full-redraw work.
6. The graphical client drains pending messages together but still applies each
   update sequentially.

These are hypotheses to instrument and test, not conclusions to encode directly
as a redesign.

## Goal

Make sustained ordinary text output drain from the PTY promptly while preserving
terminal correctness, bounded memory, authorization, detach/reattach semantics,
and exact resynchronization behavior.

The primary outcome is at least a tenfold reduction in the benchmark child's
write-blocking duration, followed by removal of avoidable post-write update and
render latency.

## Non-goals

- Replacing the CPU Wayland renderer or selecting a GPU backend.
- Weakening terminal correctness, Foot-derived behavior, or parser limits.
- Increasing queues without a measured bound and overload policy.
- Hiding backpressure with an unbounded userspace output buffer.
- Changing frozen public automation or MCP schemas.
- Changing authorization, controller, or terminal-provenance scope.
- Broadly reducing scrollback retention or benchmark workload size.
- Claiming compositor presentation latency from screenshot polling.

## Invariants

Every slice must preserve:

- pinned Foot 1.27.0 commit
  `3c5b584b0eafa772eb4376fb6eaf6643399e190e` as behavioral authority;
- parser action and PTY-reply ordering;
- terminal revision monotonicity and exact snapshot/update reconstruction;
- bounded update history, queues, snapshots, and scrollback;
- deterministic resnapshot behavior after lag or a revision gap;
- daemon ownership of canonical terminal state;
- disposable graphical clients and detach/reattach continuity;
- private protocol and public JSON/NDJSON compatibility; and
- no terminal-body content in performance logs or diagnostics.

## Baseline and acceptance gates

The existing five-terminal matrix remains the external baseline. Preserve its
raw records and binary identity; do not regenerate or replace that evidence.
New results belong in a new dated artifact directory.

Provisional closure gates for all three 2,000-line 80-column workloads are:

- median child-write duration at or below 125 ms, a minimum tenfold improvement
  over the current rounded baseline;
- no sample above 250 ms without a documented host-level cause;
- median trigger-to-visible approximation at or below 750 ms;
- no correctness, revision, resync, scrollback, detach/reattach, or lifecycle
  regression;
- bounded memory with no queue-size-only workaround; and
- lower or equal child-inclusive CPU ticks and RSS growth unless a reviewed,
  measured tradeoff is explicitly accepted.

The stretch target is a median child-write duration below 50 ms and a median
visible approximation below 500 ms. Targets may be tightened after Slice 1
produces reliable internal stage timings, but they must not be loosened merely
to declare closure.

## Dependency-ordered execution

### Slice 1 — instrument and reproduce without graphical changes

Add bounded aggregate metrics for:

- PTY read calls, bytes, and time between readiness and drain;
- terminal parser bytes, actions, committed revisions, and elapsed time;
- update-history collection and clone counts;
- live events published, subscriber overflow, and resnapshot requests;
- snapshot requests and time spent constructing wire updates;
- encoded frame bytes and outbound queue saturation; and
- client updates received, coalesced, resynchronized, applied, and rendered.

Do not log PTY content. Counters must be cheap enough to disable or retain in
normal builds without creating the measured regression.

Run non-graphical comparisons with no subscriber, one protocol subscriber, and
one attached client-equivalent consumer. Record a stage-time and amplification
budget before changing batching semantics.

**Gate:** identify which stages dominate CPU time and child backpressure, with a
reproducible non-graphical command and saved baseline.

### Slice 2 — coalesce terminal-kernel transactions

Prototype a bounded `advance` transaction that accumulates semantic damage and
commits fewer terminal updates than the current per-action path. Preserve
ordered semantic events and PTY replies. Bound transactions by bytes, elapsed
work, or both so interactive single-line output is not delayed behind a large
batch.

Test parser chunk-boundary independence: feeding the same bytes as one block,
one byte at a time, and several deterministic chunkings must produce equivalent
terminal state and ordered effects.

**Gate:** materially reduce revisions and allocations per output byte while all
terminal fixtures, differential tests, and update reconstruction tests pass.

### Slice 3 — improve PTY draining and daemon publication

Revisit the 256-byte `PARSE_BATCH` only after Slice 2 establishes transaction
semantics. Compare bounded 1 KiB, 4 KiB, and 16 KiB processing budgets.

If parser throughput still prevents prompt PTY draining, separate readiness
handling from terminal parsing with a bounded userspace byte queue and explicit
overload policy. Do not use queue capacity as the sole performance fix.

Publish at most one coalesced semantic event per bounded processing transaction
rather than repeatedly cloning a near-identical update-history tail.

**Gate:** the non-graphical child-write target passes both without a subscriber
and with an attached subscriber, without starvation of input, resize, shutdown,
or controller commands.

### Slice 4 — remove subscription snapshot amplification

Coalesce immediately pending live updates before obtaining a snapshot and
constructing a wire update. Consider a small bounded latency budget rather than
one snapshot per event. Preserve exact revision ranges and force resync whenever
coalescing cannot prove continuity.

Instrument and test slow consumers, full outbound queues, subscriber eviction,
reattach, revision gaps, daemon shutdown, and broken client connections.

**Gate:** an output burst produces bounded snapshots and frames rather than work
proportional to parser actions or 256-byte chunks.

### Slice 5 — coalesce client application and rendering

Measure the graphical client's update queue, patch application, shaping,
rasterization, SHM copying, frame request, and compositor callback boundaries.
Coalesce compatible pending updates before mutating the client snapshot. When a
backlog cannot be merged safely, prefer one explicit resynchronization over
replaying stale intermediate visual states.

Preserve immediate interactive output behavior: batching throughput must not add
noticeable latency to small writes, prompts, cursor motion, or echo.

**Gate:** the final marker reaches client state promptly after child write
completion, and full redraw/resnapshot frequency remains bounded during bursts.

### Slice 6 — guarded graphical validation and closure

Follow the repository graphical-test guardrails:

1. run one guarded case on inactive workspace 8 on DP-2;
2. abort and clean up on any placement or focus violation;
3. run the randomized matrix only after the smoke case passes; and
4. save new evidence without modifying the existing baseline.

Report child-write and screenshot-visible boundaries separately. Include raw
records, host and binary identity, configuration, randomized order, invalid-run
diagnostics, and integrity hashes.

**Gate:** all provisional closure gates pass across plain, ANSI, and Unicode
workloads, with correctness and package validation recorded alongside the new
performance evidence.

## Validation ladder

Run the smallest relevant checks after each slice, then the full ladder at
closure:

1. terminal-kernel unit, property, fixture, and differential tests;
2. daemon live-actor and protocol end-to-end tests;
3. attach, lag, resync, scrollback, controller, lifecycle, and shutdown tests;
4. automation and MCP schema/fixture compatibility tests;
5. non-graphical output and queue benchmarks;
6. extracted-package runtime validation; and
7. one-case graphical smoke followed by the guarded matrix.

Use release binaries for comparative measurements. A debug-build improvement is
not closure evidence.

## Review and stop-loss

Use one active worktree writer. Prefer one implementation slice per coherent
commit, with fresh read-only review after terminal transaction changes and again
before graphical closure.

Stop and reassess rather than broadening the redesign when:

- transaction coalescing changes parser-visible behavior or PTY-reply ordering;
- an optimization requires unbounded buffering;
- resync frequency rises under a slow-consumer test;
- small interactive writes regress while bulk throughput improves;
- a failed guarded graphical smoke violates placement or focus isolation; or
- the measured bottleneck moves outside the authorized slice.

## Completion record

When complete, update this section with:

- the measured root causes and rejected hypotheses;
- before/after stage timings and amplification counts;
- exact implementation slices and commits;
- test commands and results;
- the new benchmark artifact path; and
- any remaining performance debt or deferred architecture work.
