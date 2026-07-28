# Plan 0012: bounded compact publication frames

- **Status:** Blocked — first bounded checkpoint-frame experiment rejected before graphics; sparse frame ownership redesign required
- **Release decision:** Do not tag `beta1` until this plan passes its non-graphical, graphical, and review gates
- **Parent plans:** [Plan 0011](0011-burst-output-memory-retention.md) and [Plan 0010](0010-full-performance-optimization-pass.md)
- **Behavioral authority:** Foot 1.27.0 commit `3c5b584b0eafa772eb4376fb6eaf6643399e190e`
- **Primary evidence:** [Plan 0011 final no-go](../benchmarks/artifacts/2026-07-27-plan0011-scroll-bound-fix-3/summary.md)

## Decision

Continue the accepted Plan 0011 daemon-ownership work with one architectural
change: preserve protocol-sized publication boundaries in a private compact
representation instead of coalescing an arbitrarily large semantic tail against
one latest snapshot and materializing it as one client update.

The implementation must keep the daemon memory gains from compact cells,
one-snapshot delayed-subscriber ownership, and selective history materialization.
It must also restore bounded client allocation, CPU, and marker latency by
sending fast subscribers contiguous, protocol-valid incremental frames.

This is not an allocator, renderer, or public-protocol rewrite. If bounded private
frames cannot improve aggregate retention without restoring delayed-subscriber
snapshot retention, record that boundary and stop.

## Why Plan 0011 cannot close

The final randomized 5,000-line mixed/clear comparison completed with two
warmups and ten measured samples per variant:

| Process or metric | Clean-HEAD control | Plan 0011 candidate | Result |
|---|---:|---:|---:|
| Daemon retained RSS | 34.11 MiB | 21.17 MiB | 37.9% better |
| Client retained RSS | 36.31 MiB | 56.84 MiB | 56.5% worse |
| Aggregate retained RSS | 70.45 MiB | 78.05 MiB | 10.78% worse |
| CPU ticks | 19.0 | 76.5 | worse |
| Marker latency | 396.13 ms | 615.86 ms | worse |

Plan 0011 successfully removed daemon snapshot retention, but its mailbox can
combine many ordered terminal updates while retaining only one latest compact
snapshot. The daemon then reconstructs one large final-state publication. That
publication remains wire-valid after the accepted bounded scroll/history
fallbacks, but it creates a large client allocation and rendering burst.

The bytes were moved rather than removed. Foot/Kitty/Ghostty comparison was
correctly blocked because the Splinterm improvement gate failed.

## Working hypothesis

A normal PTY burst already arrives as multiple read/parse batches. Each batch has
an exact terminal revision and can usually be represented inside existing wire
bounds. The regression appears when subscriber-side coalescing erases those
boundaries.

Preserve each exact batch as a private `CompactPublicationFrame` containing only
what is required to reconstruct that revision interval:

- base and final terminal revisions;
- compact changed rows and row identities;
- ordered scroll operations;
- cursor, title, modes, palette, dimensions, screen, and image metadata flags;
- bounded append/trim or reset history metadata and only the required compact
  history tail; and
- the history-policy and generation identity used to create the frame.

Adjacent frames may merge only when the merged result remains exact and inside
all existing protocol and encoded-frame bounds. Otherwise the earlier frame is
sealed and delivered separately.

## Required subscriber state machine

Each compact first-party subscriber must have explicit states equivalent to:

1. **Streaming** — a bounded queue of ordered compact frames plus at most one
   producer wake token.
2. **Resync pending** — intermediate unsent frames are released and exactly one
   latest compact snapshot is retained.
3. **Exit pending** — the existing reserved exit behavior remains authoritative.
4. **Closed** — all frame, snapshot, permit, and metric ownership is released.

For a fast subscriber:

- every delivered frame is contiguous from the last delivered revision;
- no `SubscriberStalled` resync occurs for the 5,000-line closure workload;
- each frame validates against the client's current revision and dimensions; and
- the final visible state and history identities match an authoritative snapshot.

For a delayed or saturated subscriber:

- queued compact frames remain bounded by the existing semantic queue limit;
- saturation deterministically transitions to resync pending;
- unsent frames are dropped only after resync precedence is established;
- at most one latest compact snapshot is retained per subscriber; and
- a trailing exit cannot be lost behind updates or resync.

## Merge contract

Two adjacent compact frames may merge only when all of the following are proven:

- `left.revision == right.base_revision`;
- the merged row-patch count is at most `MAX_UPDATE_ROW_PATCHES`;
- the merged scroll count is at most `MAX_UPDATE_SCROLLS`;
- an append transition remains representable with `appended_rows <= MAX_ROWS`;
- history generation, active screen, dimensions, reflow, clear, and image
  transitions remain unambiguous;
- duplicate row patches can be reduced to the newest exact row without changing
  scroll order or row identity;
- the encoded frame is below `MAX_FRAME_BYTES`; and
- the frame's snapshot/history policy identity still matches its final revision.

If any proof fails, do not force a final-state viewport replacement merely to
continue merging. Seal the current frame. If the queue cannot admit the sealed
frame, use the existing deterministic resync path.

## Invariants

Every retained change must preserve:

- parser action order, PTY reply order, and chunk-boundary independence;
- exact revisions, row IDs, history generations, append/trim accounting, paging,
  detach/reattach, and final snapshot reconstruction;
- public `Subscription`, `LiveCell`, `LiveSnapshot`, `LiveEvent`, CLI,
  automation, MCP, protocol DTOs, and serialized wire compatibility;
- `MAX_ROWS`, `MAX_UPDATE_ROW_PATCHES`, `MAX_UPDATE_SCROLLS`,
  `MAX_SNAPSHOT_SCROLLBACK_ROWS`, `MAX_FRAME_BYTES`, and every existing queue,
  scrollback, cache, image, SHM, and update-history limit;
- one retained latest compact snapshot per delayed subscriber;
- the existing semantic queue capacity and reserved exit slot;
- no polling loop, resnapshot storm, new idle wakeup, or allocator-specific
  production behavior;
- daemon ownership of canonical terminal state and disposable clients; and
- Foot 1.27.0 as the behavioral oracle.

Do not obtain a win by reducing PTY read size, queue capacity, scrollback,
renderer caches, glyph caches, backing buffers, or SHM buffers.

## Progress record

- **Slice 0 attribution implementation (2026-07-27):** default-off daemon
  metrics now distinguish compact producer batches created and merged, pending
  batch/update/scroll/append ownership current and high water, and the exact
  batch/update/scroll/append shape materialized for first-party publication.
  Ownership is RAII-bound to the pending mailbox tail, so drain, clear, drop,
  saturation, and resnapshot release current gauges. Focused merge/materialize,
  multi-subscriber, saturation, and complete daemon-library tests pass. Exact
  release baseline evidence and the raw production-socket Slice 1 regression are
  still pending; no compact-frame behavior change is claimed.
- **First bounded checkpoint-frame experiment rejected (2026-07-27):** producer
  batches were merged and sealed at the existing 80-scroll/80-append bounds,
  the raw fast production socket remained contiguous, and five-cycle probes
  returned queued ownership to zero. The experiment is not acceptable:
  per-frame full compact checkpoints violate the intended one-latest-snapshot
  ownership model; receiver-local ready frames escape semantic queue accounting;
  encoded-size and complete identity proofs are absent; resync can discard a
  pending trailing exit; exact client reconstruction is not proven; and the
  opt-in metrics-overhead comparison failed its confidence-bound gate. The
  strict delayed-subscriber regression was restored, graphical work was not
  authorized or run, and `beta1` remains blocked. Evidence:
  [first bounded-frame rejection](../benchmarks/artifacts/2026-07-27-plan0012-bounded-frames/summary.md).
- **Runtime rollback completed (2026-07-27):** the rejected checkpoint-frame
  types, receiver-local ready queue, frame metrics, altered receive behavior,
  and raw experiment regression were removed. The accepted one-latest-snapshot
  mailbox, strict delayed-subscriber test, Plan 0011 correctness fallbacks, and
  Slice 0 batch attribution remain. `cargo test -p splinterd --
  --test-threads=1` and the complete serial workspace suite pass. Plan 0012 is
  now documentation/evidence for a future isolated sparse-frame spike, not an
  active production-code experiment.

## Slice 0 — freeze the current boundary

Retain exact clean-HEAD and Plan 0011 candidate binaries and source identities.
Extend default-off attribution to record per subscriber:

- compact frames created, merged, sealed, admitted, released, and discarded for
  resync;
- frame row patches, scrolls, history rows, compact content bytes, and estimated
  encoded bytes;
- frame queue current/high-water count and bytes;
- resync reason and revision interval;
- largest materialized wire frame and decode/application allocation proxy; and
- client update/snapshot queue depth and application time.

Run the final Plan 0011 workload unchanged. Do not optimize in this slice.

**Gate:** the candidate reproduces the daemon improvement and client regression,
and the counters identify one or more oversized cross-batch publications rather
than an unrelated renderer/cache class.

## Slice 1 — add the exact headless production regression

Create a no-Wayland production-socket test that:

1. launches the exact daemon and first-party subscriber;
2. subscribes before triggering the 5,000-line mixed/clear workload;
3. validates every raw `TerminalUpdate` with `validate_against`;
4. records frame revision intervals and all protocol-bound counts;
5. requires the fast path to remain contiguous without `SubscriberStalled`;
6. obtains an authoritative final snapshot containing `SPLINTERBENCH_DONE`;
7. compares update reconstruction with that snapshot; and
8. repeats enough times to cover the prior timing-dependent coalescing boundary.

Add deterministic unit tests for merge-at-limit, one-over-limit sealing,
history-generation change, clear, reflow, alternate screen, images, saturation,
receiver drop, and trailing exit.

**Gate:** the regression fails against the Plan 0011 final architecture for the
measured reason and cannot pass by accepting resync or only checking the final
snapshot.

## Slice 2 — private compact publication frames

Build one private compact frame at an exact producer batch boundary. Reuse Plan
0011's compact cell/row ownership and selective history policy. Do not materialize
public `String` cells or protocol DTOs while the frame remains queued.

The initial implementation should use existing PTY read/terminal-update
boundaries. If one producer batch alone exceeds protocol limits, add a bounded
in-memory parse checkpoint within that already-read buffer; do not increase PTY
read syscalls or globally reduce read size.

**Gate:** compact frames reconstruct the exact authoritative state at each
revision; default-off overhead remains within Plan 0010 limits; fast-path frame
allocation is materially below a full visible/history snapshot.

## Slice 3 — protocol-aware sealing and merging

Replace unbounded cross-batch coalescing with the merge contract above. Merge
cheap compatible frames, but seal before any wire, history, image, or encoded-size
limit would be crossed.

Materialize protocol DTOs once, only when a sealed frame is written. Transfer
owned row/cell values into the client state where existing APIs permit; do not
clone merely to satisfy the private representation.

**Gate:** every emitted frame validates against the previous client state; the
5,000-line fast case uses multiple bounded updates, has no overflow/resync, and
reconstructs the exact final snapshot.

## Slice 4 — delayed-subscriber collapse without snapshot accumulation

Integrate compact frames with the accepted Plan 0011 mailbox:

- retain the existing semantic capacity and reserved exit slot;
- retain at most one latest compact snapshot only after saturation/resync;
- release unsent compact frames deterministically on resync precedence;
- keep accounting RAII-safe across receive, drop, close, unwind, and reservation
  failure; and
- use event-driven completion only.

**Gate:** fast, delayed, capacity-one, closed, and multiple-subscriber cases
preserve exact revisions and final state. Delayed snapshot high water remains one
per subscriber, queued frame ownership returns to zero, and no permit or wakeup
race is introduced.

## Slice 5 — measured client allocation follow-up only

After bounded frames are proven, measure client retained memory again. Only if a
specific remaining live-capacity class is identified may this slice:

- transfer rather than clone decoded row/cell ownership;
- reuse a bounded frame-decode buffer;
- release an unusually oversized decode container at a proven rare boundary; or
- tighten private temporary reservations to actual frame counts.

Do not add `malloc_trim`, change allocators, rewrite the renderer, widen protocol
limits, or broadly clear caches.

**Gate:** the named client class falls without higher CPU, marker latency, redraw,
resnapshot, or idle work.

## Acceptance targets

All targets use the same harness, source identity, process inclusion, and settle
points as the final Plan 0011 artifact.

Mandatory correctness and responsiveness:

- zero invalid protocol frames and client exits;
- zero fast-subscriber overflow or `SubscriberStalled` in the closure workload;
- exact final snapshot/wire identity;
- no input, resize, idle, CPU, or marker-latency regression beyond the accepted
  control allowance; and
- no graphical isolation or cleanup violation.

Memory gates:

- delayed compact snapshot high water remains one per subscriber;
- daemon retained growth does not materially regress from Plan 0011's accepted
  improvement;
- client retained growth is no worse than clean-HEAD control;
- aggregate retained growth is below clean-HEAD control; and
- release preference remains at least 40% below the current aggregate control.

Do not loosen the 40% preference merely to declare success. If the bounded-frame
architecture produces only a smaller regression or a marginal aggregate win,
record it as another no-go.

## Validation ladder

After each coherent slice, run the smallest relevant commands:

```bash
cargo test -p splinterm-terminal
cargo test -p splinterd
cargo test -p splinterm-protocol
cargo test -p splinterm-automation-client
cargo test -p splinterm --lib
python -m pytest tools/benchmark/test_benchmark.py -q
```

Before graphical work:

```bash
cargo test --workspace -- --test-threads=1
```

Retain exact evidence for:

- one-under, exact, and one-over every frame limit;
- fast, delayed, saturated, closed, and multiple subscribers;
- clear/reflow, alternate screen, dimensions, images, and history generation;
- 1,000-row append/trim and detached viewport reconstruction;
- repeated burst/settle plateau and no-subscriber behavior;
- frame count/size and per-process RSS/PSS/private-anonymous attribution; and
- default-off instrumentation overhead.

## Graphical evidence boundary

Graphical execution requires user approval for the complete bounded sequence.
After non-graphical implementation, serial validation, and fresh review:

1. verify workspace 8 is inactive on DP-2 and empty;
2. verify no crashed session-lock fallback obscures DP-2;
3. use pre-map silent placement and permanent no-focus rules;
4. run one exact-candidate smoke;
5. abort and clean up on any placement, focus, identity, cardinality, lock-overlay,
   or cleanup violation;
6. after a valid smoke, run a randomized clean-HEAD/control candidate batch with
   retained per-process attribution; and
7. run Foot/Kitty/Ghostty only after Splinterm aggregate improvement and
   responsiveness are established.

One authorization covers the declared smoke and its conditional matrix. Do not
insert another confirmation gate between successful stages.

## Review and evidence

Keep the active worktree single-writer. Require parent self-validation before
launching a reviewer. Use:

- one fresh read-only review after the compact-frame/mailbox architecture is
  coherent and focused tests pass; and
- one final fresh read-only release review after exact provenance, serial tests,
  and graphical evidence are assembled.

Record rejected experiments and preserve the Plan 0011 no-go artifacts. Do not
silently regenerate or replace them.

## Stop-loss

Stop and reassess when:

- counters do not attribute the client regression to oversized cross-batch
  publication;
- two bounded-frame experiments fail to improve aggregate retention;
- the design requires more than one retained latest snapshot per delayed
  subscriber;
- fast-path correctness requires routine resync;
- memory falls only by moving bytes back into daemon queues or unmeasured maps;
- CPU, marker latency, input, resize, idle, or redraw behavior regresses;
- protocol/queue/cache/scrollback limits must be widened or reduced;
- public API or serialized compatibility would change; or
- success requires a renderer, allocator, or protocol rewrite.

## Completion record

At closure append links to:

- clean control and exact candidate source/binary identities;
- Plan 0011 comparison and the new per-frame attribution;
- headless production-socket reconstruction evidence;
- fast/delayed/saturated/multiple-subscriber evidence;
- repeated non-graphical RSS/PSS/private-anonymous evidence;
- approved graphical smoke, randomized control/candidate, and conditional
  terminal comparison;
- full serial validation and fresh reviews; and
- any remaining client, daemon, or Foot gap deliberately deferred.

A successful implementation may unblock `beta1` only after all mandatory gates
and final review pass. A daemon-only win, a client regression, or a failed
comparative gate remains a no-go.
