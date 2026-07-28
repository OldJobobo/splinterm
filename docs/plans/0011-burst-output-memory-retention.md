# Plan 0011: burst-output memory-retention optimization

- **Status:** Final closure no-go — daemon retention improved, but randomized graphical aggregate/client retention and responsiveness regressed
- **Release decision:** Do not tag `beta1` until this pass has recorded validation and review
- **Parent plan:** [Plan 0010](0010-full-performance-optimization-pass.md), especially Slice 5
- **Behavioral authority:** Foot 1.27.0 commit `3c5b584b0eafa772eb4376fb6eaf6643399e190e`
- **Primary evidence:** [five-terminal retention matrix](../benchmarks/artifacts/2026-07-23-five-terminal-retention/summary.md) and [first measured performance pass](../spikes/artifacts/0026-performance-optimization-pass/README.md)

## Decision

Run one focused, measurement-led pass against burst-output memory retention. CPU,
responsiveness, correctness, and idle behavior are release-candidate quality and
must not be traded away. Prefer bounded ownership and representation changes over
parser, renderer, protocol, or allocator rewrites.

The retained July 23 matrix is diagnostic rather than a current baseline. It
reports child-inclusive process-tree RSS and predates later performance changes.
The first slice must therefore refresh exact current-source evidence and split
retained pages by process and memory class before an optimization is accepted.

## Existing evidence and target

The retained mixed-output matrix reports these medians after 5,000 mixed rows and
a two-second settle:

| Terminal | Baseline RSS | Post-settle RSS | Retained growth |
|---|---:|---:|---:|
| Splinterm | 44.7 MiB | 84.0 MiB | 39.3 MiB |
| Foot | 50.6 MiB | 67.7 MiB | 17.1 MiB |
| Kitty | 416.2 MiB | 440.3 MiB | 24.1 MiB |
| Ghostty | 375.0 MiB | 392.3 MiB | 17.3 MiB |

The primary retained-growth target is **at most 17 MiB**, with comparative
confidence evidence showing Splinterm below Kitty and Ghostty. Because Foot and
Ghostty are nearly tied in the retained artifact, this is deliberately a stretch
target.

If that target would require a major rewrite, record the boundary rather than
crossing it silently:

- preferred bounded result: at most 20 MiB and at least 40% below current control;
- minimum useful result: below 24 MiB, clearly beating Kitty's retained growth;
- no release claim based only on lower absolute RSS while retained growth remains
  unexplained.

Current clean-HEAD evidence may tighten these thresholds but may not loosen them
merely to declare success.

## Invariants

Every retained change must preserve:

- parser action order, PTY reply order, and chunk-boundary independence;
- exact terminal revisions, row identities, history generations, paging, and
  snapshot/update reconstruction;
- bounded update history, subscriber queues, scrollback, image stores, caches,
  SHM buffers, and worker threads;
- deterministic overflow, resnapshot, detach/reattach, and slow-consumer behavior;
- input-to-child, input-to-visible, small-write, bulk-output, and resize behavior
  within the accepted control bounds;
- accepted idle CPU and no-image RSS behavior;
- daemon ownership of canonical terminal state and disposable clients;
- public CLI, automation, MCP, and private protocol compatibility;
- body-free diagnostics; and
- the pinned Foot checkout and retained comparison evidence.

Do not lower scrollback, queue, cache, SHM, image, or update-history limits as the
memory optimization.

## Primary ownership hypothesis

The first implementation target is daemon publication:

1. `publish_updates` constructs one complete `LiveSnapshot` per subscriber and
   publication.
2. A graphical attachment requests the protocol maximum and the daemon clamps it
   to 1,000 scrollback rows.
3. The actor subscriber channel is bounded at 64 entries, reserving one internal
   exit slot.
4. Each queued update owns semantic updates plus a boxed full snapshot.
5. Each `LiveCell` currently owns a `String`, including empty and one-scalar
   cells.
6. Downstream draining coalesces intermediate snapshots, but freed allocation
   pages may remain in allocator arenas.

The canonical grid, client protocol snapshot, prepared renderer frame, persistent
backing, bounded glyph cache, and Wayland SHM are secondary measured classes.
They must not be optimized speculatively before attribution identifies them.

## Slice 0 — record the current baseline

Build exact release control binaries from clean source. Extend the retention
harness so a Splinterm record contains, for each daemon, client, and child PID:

- RSS and PSS;
- private anonymous, private file-backed, and shared/shmem residency from
  `/proc/<pid>/smaps_rollup` where available;
- baseline, observed peak, marker-visible, and 2/10/30/120-second settle samples;
- process identity and inclusion in the aggregate; and
- repeated-cycle endpoint slope rather than one post-settle value.

Add opt-in, bounded ownership counters for:

- canonical allocated grid rows/cells;
- retained update count and damage/event lengths and capacities;
- subscriber count, queue depth, and queue high water;
- owned snapshots built, queued, rows, cells, scalar/composed content, and owned
  string bytes;
- client snapshot/frame vector lengths and capacities;
- persistent and frame glyph bytes;
- backing length/capacity and SHM buffer count/bytes.

Run non-graphical daemon cases first: no subscriber, fast subscriber, delayed
subscriber, overflow/resnapshot, scrollback disabled, 1,000-row scrollback, and
repeated burst/settle cycles. Use Heaptrack or DHAT only for isolated diagnostic
runs; do not enable expensive profiling by default.

**Gate:** most retained growth is attributed to daemon live heap, client live
heap, SHM/mappings, or allocator high water. Instrumentation remains body-free,
bounded, and within Plan 0010's overhead limits.

## Slice 1 — compact internal snapshot cell ownership

Replace `LiveCell.content: String` with an internal representation that stores
empty, scalar, composed, and spacer content without heap-allocating empty or
single-scalar strings. Materialize protocol `String` values only when converting
a row that will actually be sent.

Keep this representation internal to `splinterd`; do not change protocol DTOs or
canonical terminal cells in this slice.

**Gate:** snapshot semantic equality and wire byte identity pass; allocation and
owned-content counters improve; no output, CPU, or RSS counter regresses beyond
measurement noise.

## Slice 2 — bound queued full snapshots

Prevent a delayed subscriber from retaining dozens of complete 1,000-row
snapshots. Preferred design:

- keep semantic update batches/revisions bounded;
- retain at most one latest owned snapshot per subscriber;
- drain contiguous updates against that exact snapshot; and
- require resnapshot when continuity cannot be proven.

A latest-state mailbox/watch channel may be used if revision identity remains
explicit. Do not merely reduce channel capacity: that risks replacing retained
memory with resnapshot storms and responsiveness regressions.

**Gate:** fast, delayed, saturated, closed, and multiple subscribers preserve
exact revisions and terminal state; queued full-snapshot high water is one per
subscriber; overflow/resnapshot remains deterministic.

## Slice 3 — materialize only required history

For contiguous live updates, avoid owning the entire loaded scrollback window
when only visible damage and newly appended/trimmed history are required. Full
history remains available for attach, explicit snapshot, clear, reflow, paging,
and resynchronization.

**Gate:** exact append/trim transitions, row IDs, history generation, omitted-row
accounting, selection pins, paging, and detached viewport reconstruction pass.

## Slice 4 — measured reclamation only

If attribution still shows avoidable retained capacity:

- release unusually oversized update/snapshot containers at rare reset points;
- recreate client frame or backing storage only after a substantial downsize;
- inspect frame-held glyph `Arc`s and map capacities;
- reserve scrollback temporaries from available rows rather than requested maxima;
- compare allocator live bytes with RSS before adding manual shrinking; and
- do not ship unconditional `malloc_trim` or allocator-specific behavior.

**Gate:** the targeted memory class falls without increased allocation churn,
render CPU, full reloads, redraws, or idle work.

## Validation ladder

Run the smallest relevant commands after each coherent slice:

```bash
cargo test -p splinterm-terminal
cargo test -p splinterd
cargo test -p splinterm-protocol
cargo test -p splinterm-automation-client
cargo test -p splinterm --lib
python -m pytest tools/benchmark/test_benchmark.py -q
```

Before closure:

```bash
cargo test --workspace -- --test-threads=1
```

Also retain focused evidence for fast, slow, and multiple subscribers; saturation
and resnapshot; 1,000-row append/trim; clear/reflow; detach/reattach; repeated
burst/settle plateau; no-subscriber behavior; and exact wire/snapshot identity.

## Graphical evidence boundary

Graphical execution requires separate user approval. After non-graphical slices
and review:

1. verify workspace 8 is inactive on DP-2;
2. install pre-map workspace/monitor placement and permanent no-focus rules;
3. run one exact-candidate Splinterm retention smoke;
4. abort and clean up on any placement, focus, identity, or cleanup violation;
5. run a randomized Splinterm control/candidate batch only after the smoke passes;
6. run the Foot/Kitty/Ghostty comparison only after Splinterm improvement is
   established; and
7. preserve old artifacts and write a new dated evidence directory.

## Progress record

- **Superseded development iterations (2026-07-27):** review rejected the
  receiver-wrapper implementation because it changed the public
  `Subscription.events` type, could lose a retained update at disconnect, and
  left a receiver-drop/permit race in queue accounting. Those designs are not
  the current architecture.

- **Closure evidence attempt (2026-07-26):** [the dated closure artifact](../benchmarks/artifacts/2026-07-26-plan0011-closure/summary.md)
  passed the default-off instrumentation overhead gate and the complete serial
  workspace test, but failed the memory gate before any graphical launch. The
  exact 5,000-line mixed/clear delayed compact-subscriber case retained 43.88
  MiB RSS/PSS, almost entirely daemon private-anonymous memory, with queue high
  water of 64 snapshots and 691,360 cells. A capacity-one overflow control
  retained 6.88 MiB, identifying queued snapshots as the avoidable class. The
  result exceeds the strict `<24 MiB` minimum useful threshold, so the approved
  workspace-8/DP-2 smoke and comparison matrix were not run. Slice 2 remains
  required; no limit was reduced and no closure is claimed. The ordinary
  `splinterd` suite reproduced the known concurrent policy timeout, its exact
  isolated test passed in 14.98 seconds, and the full workspace serial run
  passed.

- **Slice 2 accepted (2026-07-27):** [the dated Slice 2 artifact](../benchmarks/artifacts/2026-07-27-plan0011-slice2/summary.md)
  records the bounded producer-coalesced semantic mailbox and one-entry compact
  snapshot slot. The original queue capacity remains unchanged; one wake token
  represents a tail of at most 63 ordered semantic batches while the exit slot
  remains reserved. Fast and two-subscriber compact cases completed without
  actor resnapshot, delayed and capacity-one cases saturated deterministically,
  and retained full-snapshot high water was exactly one per compact subscriber
  (one for fast/delayed/overflow, two aggregate for two subscribers), with zero
  retained snapshots after teardown. Delayed retained growth fell from 43.88
  MiB to 4.81 MiB. Producer-batch completion is event-driven through Tokio
  `Notify`; the focused regression records one park/wake per synchronous PTY
  read rather than polling. The final full serial workspace passed; the earlier
  ordinary concurrent daemon suite reproduced only the documented policy
  timeout, whose isolated run passed in 14.82 seconds. Successful fast 1,000-row
  materialization still retained 34.33 MiB, naming required-history
  materialization as Slice 3's next measured class. Fresh independent review
  accepted the bounded mailbox, revision/exit/resnapshot semantics, exact
  ownership accounting, and event-driven producer completion with no blockers.

- **Slice 3 accepted (2026-07-27):** [the dated Slice 3 artifact](../benchmarks/artifacts/2026-07-27-plan0011-slice3/summary.md)
  records a private fail-safe compact history policy. Exact contiguous updates
  own the complete visible grid plus either no history rows or only a proven
  normal-screen full-height forward-scroll append tail. Full snapshot,
  dimensions/reflow, clear/replacement, alternate-screen, generation change,
  reverse/partial scroll, and unexplained scrollback remain full-history
  fallbacks. Attach, explicit snapshots, paging, search, protocol DTOs, renderer
  state, and all configured limits are unchanged. Append-delta history is wire
  byte-identical to full materialization after the daemon selects the exact
  appended tail, and private policy/revision mismatch requires resnapshot. Fast
  retained growth fell 44.50% from 34.33 MiB to 19.05 MiB, with zero overflow
  and one retained snapshot; delayed growth was 5.46 MiB with deterministic
  saturation, and two fast subscribers retained 20.41 MiB with aggregate
  snapshot high water two. Direct package checks and the bounded serial
  workspace retry passed. The ordinary concurrent daemon run had the known
  policy timeout plus one phase-8 timing failure, both passing exact isolated;
  the first serial workspace attempt had one unrelated MCP controller flake,
  whose exact isolated test and bounded retry passed. Fresh independent review
  accepted the selective history policy, fail-safe fallbacks, exact mailbox
  policy/revision pairing, append/trim metadata, row identities, wire identity,
  and unchanged public/protocol/limit boundaries with no blockers. The 19.05 MiB
  result satisfies the preferred bounded whole-plan target (at most 20 MiB and
  at least 40% below control), but not the 17 MiB stretch target. Graphical and
  comparative closure evidence remains open.

- **Final graphical no-go (2026-07-27):** [the corrected final artifact](../benchmarks/artifacts/2026-07-27-plan0011-scroll-bound-fix-3/summary.md)
  records the exact rebuilt candidate, final serial workspace pass, guarded smoke,
  and ten-sample-per-variant randomized clean-HEAD comparison. The candidate
  daemon improved from 34.11 MiB to 21.17 MiB median retained RSS (37.9%), but
  the client regressed from 36.31 MiB to 56.84 MiB (56.5%); aggregate retained
  growth regressed from 70.45 MiB to 78.05 MiB (10.78%), with worse CPU and
  marker latency. The optimization therefore shifts high-water allocation from
  daemon snapshot ownership into a large coalesced client update. The required
  40% aggregate improvement is not established, so Foot/Kitty/Ghostty were not
  run and `beta1` remains forbidden. Correctness fallbacks keep oversized
  coalesced scroll/update history within unchanged protocol bounds; addressing
  the client regression requires a separately planned bounded-checkpoint/mailbox
  architecture, not another closure tweak. Slice 4 still justifies no allocator-
  specific reclamation behavior.

- **Closure attempt and Slice 4 attribution (2026-07-27):** [the final evidence artifact](../benchmarks/artifacts/2026-07-27-plan0011-final/summary.md)
  retains the exact candidate patch/source bundle, toolchain and binary hashes,
  corrected five-cycle/120-second evidence, allocator diagnostics, guarded
  graphical smoke, and aborted randomized batch. Heaptrack identified the
  original repeated-cycle probe itself as a major temporary allocator: marker
  polling materialized a 1,000-row snapshot every 10 ms. Visible-only marker
  polling corrected the five-cycle final RSS growth from 26.37 MiB to 15.52 MiB
  with 11.32 MiB private-anonymous growth and no overflow, meeting the 17 MiB
  stretch target. A diagnostic `MALLOC_ARENA_MAX=1` run was worse than the
  corrected default, so it does not prove arena causality. Heaptrack's 9.40 MiB
  peak tracked heap / 25.73 KiB leak result and heavy temporary row/snapshot
  construction make allocator high-water plausible while showing no retained
  live snapshot ownership. No allocator-specific product behavior, manual trim,
  or Slice 4 reclamation edit is justified. The exact graphical candidate smoke
  passed workspace 8 / DP-2 placement, no-focus, identity, and cleanup checks.
  The subsequent randomized clean-HEAD control/candidate batch aborted on a
  workspace-8 cardinality violation; cleanup was verified, the batch was not
  retried, and Foot/Kitty/Ghostty comparisons were not run. Closure therefore
  remains incomplete and `beta1` must not be tagged.

- **Architectural-pivot development milestone (2026-07-27):** the original
  public `Subscription` API is restored exactly, including
  `pub events: tokio::sync::mpsc::Receiver<LiveEvent>`, and the existing
  `attach`, `subscribe`, DTO, and wire paths remain unchanged. The first-party
  daemon and Phase 9 probe use a separate additive `CompactSubscription` path;
  its cell, row, snapshot, and queued-event representations remain private.
  Pending compact updates coalesce before one materialization of the latest
  retained public snapshot, preserving update order and revision, resnapshot
  precedence, trailing exit delivery, and a final retained update before
  disconnect. Default-off attribution uses an idempotent RAII lease owned by
  each permit-admitted compact event; receive, coalescing, receiver drop,
  unwind, and the receiver-close-after-reservation path all release current
  ownership, while closed/full reservation failures never affect current or
  high-water metrics. Focused library tests passed 49/49, daemon binary tests
  39/39, protocol tests 15/15, automation-client tests 31/31, client/renderer
  library tests 172/172, and benchmark harness tests 26/26. The release Phase 9
  compact probe completed output in 39.98 ms and recorded one admitted 24-row,
  1,920-cell snapshot, zero compact-content owned string bytes, and exact
  one-event admitted-ownership high water. The single permitted full daemon run
  passed all unit and binary tests and 15 of 16 integrations; the documented
  `parent_policy_snapshot_excludes_new_splint_until_reload` suite-wide timeout
  reproduced, while its exact isolated run passed in 14.69 seconds. Fresh
  independent review, instrumentation-overhead comparison, current-source
  repeated-cycle RSS/PSS evidence, and all graphical gates remain open; no
  slice closure is claimed.

## Review and evidence

Keep one active-worktree writer. Use one measured hypothesis per implementation
commit or clearly separated diff slice. Record rejected experiments. Require a
fresh read-only review after daemon publication ownership changes and before
closure. Do not claim a slice complete without both recorded validation evidence
and recorded review.

## Stop-loss

Stop and reassess when:

- a candidate cannot name the measured retained class it reduces;
- two controlled experiments fail to improve that class;
- memory falls only by moving bytes between daemon, client, SHM, or an unmeasured
  mapping;
- resnapshot, full-reload, redraw, or wakeup frequency rises;
- output CPU, child-write responsiveness, input, resize, or idle regresses beyond
  the accepted control allowance;
- correctness requires weakening revision, history, or reconstruction semantics;
- limits must be reduced to show improvement; or
- beating Ghostty requires a major renderer/protocol rewrite.

## Completion record

The failed 2026-07-26 closure attempt, exact binary identities, process-class
attribution, overhead results, repeated-cycle evidence, validation logs, and
read-only graphical preflight are retained in the
[Plan 0011 closure artifact](../benchmarks/artifacts/2026-07-26-plan0011-closure/summary.md).
This link is evidence of a failed gate, not a completion record. The later
[final closure attempt](../benchmarks/artifacts/2026-07-27-plan0011-final/summary.md)
records corrected 15.52 MiB repeated-cycle evidence and exact provenance, but is
also not a completion record because the randomized graphical batch violated its
workspace-cardinality guard and comparative evidence was not run. The later
[corrected final artifact](../benchmarks/artifacts/2026-07-27-plan0011-scroll-bound-fix-3/summary.md)
completed a valid randomized control/candidate comparison and records the final
no-go: aggregate and client retention regressed, so the comparative-terminal
stage remained correctly blocked.

At closure append links to:

- clean control and candidate source/binary identities;
- per-process and per-memory-class attribution;
- accepted and rejected hypotheses;
- non-graphical repeated-cycle evidence;
- approved graphical control/candidate and comparative evidence;
- correctness, package, fuzz, and review evidence; and
- the remaining Foot gap and any deliberately deferred architectural work.
