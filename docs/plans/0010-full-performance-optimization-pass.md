# Plan 0010: full terminal performance optimization pass

- **Status:** In progress — Slice 0 recorded; partial Slice 1 instrumentation and measured Slice 3 development candidates retained provisionally
- **Roadmap:** post-Phase 5 performance stabilization
- **Primary evidence:** [five-terminal benchmark suite](../benchmarks/terminal-benchmark-plan.md), [output-throughput closure](../spikes/artifacts/0024-output-throughput-graphical/README.md), [targeted-input matrix](../benchmarks/artifacts/2026-07-24-five-terminal-latency/README.md), and [image closure](../spikes/artifacts/0025-terminal-images/slice8-graphical-final/README.md)
- **Foundation:** [Plan 0008](0008-terminal-image-protocols.md) and [Plan 0009](0009-output-throughput-optimization.md)
- **Behavioral authority:** Foot 1.27.0 commit `3c5b584b0eafa772eb4376fb6eaf6643399e190e`
- **Plan review:** two fresh read-only reviews completed; evidence/methodology and architecture/correctness findings incorporated

## Decision

Run one measurement-led optimization pass across the complete terminal path before expanding terminal features again. Work from PTY readiness through terminal mutation, daemon publication, protocol reception, renderer preparation, CPU composition, Wayland submission, input return, and retained-memory behavior.

Do not optimize against the original July 23 output, retention, or scrollback matrices as though they describe the current implementation. Plan 0009, synchronized frame batching, receiver wakeups, bounded receiver draining, terminal images, and later renderer fixes materially changed the path. The first slice therefore establishes a clean current baseline and calibrates the benchmark itself.

This plan is not permission to replace the renderer, weaken correctness, widen queues, or redesign protocol semantics. Each retained change must remove measured work at a named stage and pass the smallest relevant correctness and latency gates before the next slice begins.

## Evidence assessment

All numbers are retained development evidence from one host, not measurements of current `HEAD`. The output artifact records dirty commit `0dd5701`, the input artifact records an isolated patched build derived from `58b5f50`, and several older matrices predate Plan 0009. Their manifests do not retain every font, normalized configuration, geometry, locale, palette, and exception required by the suite's publishable fairness contract. Peer comparisons below establish diagnostic context only. Screenshot polling detects a marker; it is not a compositor presentation timestamp or input-to-photon measurement.

| Lane | Best retained Splinterm evidence | Peer context | Confidence and implication |
|---|---:|---:|---|
| Child ready | 73.1 ms | Foot 72.2 ms | Old but near parity; Splinterm uses a prestarted daemon, so this is not a cold-process comparison. |
| Window mapped | 119.9 ms | Foot 52.7 ms; Alacritty 110.8 ms | Old and worth remeasuring. It suggests a client/Wayland startup opportunity, not a daemon-start conclusion. |
| Idle resources | 51,036,160-byte median RSS; median/p95/max 1 CPU tick over two seconds | Original five-terminal Splinterm median was 47.0 MiB and 0 ticks | The final image-closure artifact passes its strict no-image budget. Preserve it; do not optimize from superseded failed closure directories. |
| Bulk child write | 40.2/41.4/44.8 ms plain/ANSI/Unicode | 1.5–5.2 ms peers in the same guarded matrix | Plan 0009 improved this by about 31×, but a material CPU/backpressure gap remains. |
| Bulk marker visible | 532.5/511.2/529.1 ms | Foot 153.9–156.7 ms; peers vary to 507.3 ms | There is a roughly 470–490 ms post-write tail, but screenshot polling and later client changes require fresh stage timings before attribution. |
| Output RSS growth | 14.7/18.9/34.3 MiB | Foot 2.6/2.8/16.9 MiB | Material, especially plain and ANSI. Determine live data versus allocator/high-water retention. |
| Input to child | 13.98 ms | 8.52–8.87 ms peers | The median gap is 5.11–5.46 ms, while Splinterm's retained p95 is 20.38 ms. Client dispatch, daemon/control handling, or PTY write may contribute. |
| Input marker visible | 184.88 ms | 179.53–189.21 ms peers | Already peer-equivalent at this coarse boundary. Preserve it while reducing input-to-child latency. |
| Resize | 254.8 ms for twelve settled resizes | 248.4–259.2 ms peers | Timing is already at parity. Only the old 3.4 MiB growth merits diagnosis. |
| Retention | 2.924 s visible; 39.3 MiB retained growth | 0.166–0.520 s; 17.1–24.1 MiB | Predates Plan 0009 and must be refreshed before it can justify implementation work. |
| Scrollback | 0.529 s disabled; 1.072 s large | Foot about 0.155 s in both profiles | Predates Plan 0009. The profile-sensitive delta is a useful diagnostic shape, not a current verdict. |
| Lifecycle | child exit 204.7 ms; window persisted 10/10 | peers unmap near 250 ms | Persistence and two residual Splinterm processes are intended architecture, not a performance failure. |
| Correctness | Foot-derived semantic fixtures and final-buffer suites exact | Foot oracle pinned | Performance closure must retain this evidence. The checked report did not itself run the fuzz target. |
| Images | static Kitty/Sixel composition closure passed; no-image idle gate passed | no comparable streamed matrix | Tiny one-batch fixture timings are boundary probes, not general image-throughput evidence. |

## Current execution note (2026-07-26)

A partial Slice 1 implementation now has opt-in, body-free
`CLOCK_MONOTONIC_RAW` stage tracing from terminal mutation through Wayland
commit. PTY readiness/drain, input, callback, retained-memory attribution, and a
complete queue-wait reconciliation remain open, so the Slice 1 gate is not
claimed. Disabled tracing passed the interleaved
5-warmup/20-sample overhead gate: one-sided 95% upper regressions were +1.27%
output completion, -1.00% process CPU, and +1.12% small writes.

The trace justified two bounded Slice 3 copy reductions: direct frame-prefix
backfill encoding and moving the owned visible-row snapshot into the next daemon
semantic-diff baseline after borrowed materialization. Deterministic encoding
median improved 6.4%; traced wire-materialization median improved 9.0% and
nearest-rank p95 23.2%. Full workspace tests, release scrollback/search, and overflow/resync pass.
A guarded exact pre-review Slice 3 3-warmup/10-sample matrix is valid, and a
deterministic real-Cava graphical observation advanced ten distinct applied and
committed revisions. Post-review trace-integrity hardening changed binary
identity, so fresh exact-binary graphical evidence remains required.
The absolute bulk/visible/CPU/RSS gates and retained 5-warmup/20-sample Slice 3
comparison remain open, so Slice 3 closure is not claimed.

## Primary questions

1. Where do the remaining 40–45 ms of child-write backpressure and 10–11 CPU ticks accrue after Plan 0009?
2. How much of the write-to-visible tail is daemon backlog, protocol transfer, client update application, row shaping, pixel composition, SHM copying, Wayland scheduling, screenshot polling, or compositor delay?
3. Why do plain and ANSI bursts retain 15–19 MiB, and how much is canonical terminal state, update history, protocol materialization, renderer state, font residency, SHM/backing storage, or allocator high water?
4. Does large scrollback still amplify current output and snapshot work, and if so at which layer?
5. Which client-to-daemon or daemon-to-PTY stage accounts for the roughly 5.1–5.5 ms median input-to-child gap and wider Splinterm tail?
6. Can startup mapping and idle/resource behavior improve without trading away the already competitive interactive visible latency?
7. What is the bounded throughput and CPU cost of representative static images and multi-pane composition after ordinary text paths are stable?

## Goals

- Produce a clean, statistically useful current baseline for all supported performance lanes.
- Attribute latency, CPU, allocation, copy, queue, and wakeup costs to explicit stage boundaries without logging terminal bodies.
- Reduce ordinary text child-write backpressure and output CPU materially beyond Plan 0009.
- Bring bulk text marker visibility closer to the fastest peer without regressing small interactive writes.
- Reduce avoidable retained memory and scrollback-sensitive amplification.
- Close the input-to-child gap while preserving authorization and controller semantics.
- Preserve or improve startup, idle, resize, detach/reattach, multi-pane, and image behavior.
- Leave reusable performance thresholds and evidence that can catch regressions in later feature work.

## Non-goals

- Replacing the CPU renderer or adopting a GPU backend in this pass.
- Changing terminal revision meaning merely to lower a counter.
- Increasing queue, cache, scrollback, SHM, or thread limits as the sole fix.
- Unbounded buffering, deferred correctness, dropped PTY replies, or lossy resynchronization.
- Weakening authorization, controller ownership, daemon canonical-state ownership, or process-incarnation checks.
- Optimizing unsupported protocols or implementing new terminal features.
- Treating screenshot detection as compositor presentation or input-to-photon timing.
- Rewriting the benchmark until Splinterm wins; benchmark changes require independent calibration evidence.

## Invariants

Every slice preserves:

- the pinned Foot oracle and canonical checkout cleanliness;
- parser action ordering, PTY reply ordering, and chunk-boundary independence;
- monotonic revisions and exact snapshot/update reconstruction;
- bounded update history, scrollback, queues, snapshots, image stores, caches, SHM buffers, and worker threads;
- deterministic resnapshot after lag, overflow, or revision gaps;
- daemon ownership of canonical terminal state and disposable graphical clients;
- detach/reattach, multi-pane, process continuity, and lifecycle semantics;
- public CLI, automation, MCP, and private protocol compatibility unless separately reviewed;
- no terminal body, input body, or image payload in performance logs; and
- immediate behavior for prompts, echo, cursor motion, PTY replies, and small writes.

## Measurement contract

### Evidence classes

Keep four evidence classes separate:

1. **Internal deterministic:** terminal/daemon/renderer microbenchmarks and counters.
2. **Splinterm end-to-end:** isolated current release binaries with exact hashes.
3. **Five-terminal comparative:** identical host, configuration, randomized blocks, and workload bytes.
4. **Graphical approximation:** guarded screenshot-marker results with their claim limits stated.

A change may use internal evidence for iteration, but closure requires end-to-end evidence. Comparative claims require the five-terminal lane.

### Statistics

- Development diagnosis: at least 3 warmups and 10 measured samples.
- Retained slice evidence: at least 5 warmups and 20 measured samples for affected Splinterm cases, interleaving control and candidate builds where possible.
- Final comparative closure: 5 warmups and 30 measured samples per case in randomized blocks.
- Report median, nearest-rank p95, min, max, median absolute deviation, and bootstrap 95% confidence intervals for the median, p95, and candidate/control deltas. Record the bootstrap method, seed, and resample count.
- Absolute gates require both the point estimate and the relevant one-sided 95% upper confidence bound to pass. Non-regression gates require the upper confidence bound for candidate/control regression to remain within the stated allowance.
- Predeclare invalidation criteria in the manifest: graphical placement/focus or cleanup violation, schema failure, missing dependency, workload timeout, binary/config identity mismatch, or a numeric thermal/background preflight failure. Preserve every invalid run and reason. Any safety violation aborts immediately; if other invalid runs exceed 5%, diagnose and rerun the complete randomized matrix rather than selectively replacing samples.
- Record numeric thermal/background thresholds plus CPU governor, kernel, compositor, monitor, scale, font hashes, normalized configurations, palette, locale, binary hashes, and every fairness exception.

### Instrumentation overhead

Every new diagnostic counter or timestamp must be body-free, bounded, and cheap. Compare instrumented and uninstrumented release builds in interleaved non-graphical runs. Use high-resolution process/thread CPU time or hardware counters for overhead decisions; the external suite's 100 Hz CPU ticks cannot resolve a 2% change around a 10-tick workload.

**Gate:** the point estimate and one-sided 95% upper confidence bound show no more than 2% regression in output completion or high-resolution CPU time, and no more than 5% small-write regression. Expensive profiling belongs behind an explicit diagnostic flag and must not ship enabled by default.

## Provisional closure gates

Slice 0 may tighten these gates from clean current evidence, but it may not loosen them merely to declare closure.

### Correctness and bounds

- All Foot-derived semantic fixtures, final-buffer comparisons, terminal tests, daemon protocol tests, renderer tests, automation/MCP fixtures, detach/reattach, lag/resync, scrollback, lifecycle, and image tests pass.
- A recorded bounded parser fuzz run completes without crash, timeout, or sanitizer finding.
- No queue, history, cache, thread, SHM, or image budget increases without separate measured justification.
- No terminal-body content appears in diagnostics or artifacts.

### Interactive latency

- Input-to-child median at or below 10 ms and p95 at or below 15 ms.
- Input-to-visible median no worse than 190 ms and p95 no worse than the clean current control by more than 5%.
- Small echo and prompt workloads show no candidate/control regression above 5%.

### Bulk text

For plain, ANSI, and Unicode 2,000-line workloads at 80x24:

- child-write median at or below 25 ms and p95 at or below 50 ms;
- marker-visible median at or below 300 ms and p95 at or below 400 ms;
- child-inclusive CPU ticks at or below 6/6/8 respectively; and
- no subscriber overflow or resnapshot in the normal attached-client case.

At 240x80, require at least a 25% candidate/control improvement in the stage proven dominant while preserving the same correctness and bounds. Do not force the 80x24 absolute thresholds onto a different geometry without baseline evidence.

### Memory and idle

- Preserve the accepted no-image idle ceiling: median RSS no more than 5% or 4 MiB above its clean baseline, whichever is smaller; median/p95 idle CPU at or below the final accepted control.
- Plain and ANSI output RSS growth at or below 8 MiB; Unicode at or below 24 MiB, unless Slice 0 proves a lower stable current baseline and tightens these.
- Mixed-output retained growth at or below 25 MiB and at least 30% below the clean current control.
- Disabled-scrollback growth at or below 2 MiB; large-scrollback growth at or below 12 MiB.
- At least ten workload/settle cycles reach a bounded plateau: after the declared warm-up cycles, the final five cycle endpoints stay within the fixed byte ceiling and the confidence interval for retained-growth slope includes zero. Repeat the test in a second independent batch.

### Startup, resize, and images

- Window-map median at or below 100 ms without regressing child-ready median beyond 5%.
- Resize settled time remains within 5% of the clean control and resize growth is at or below 2 MiB.
- Image-free text gates remain identical whether image support is compiled/configured but unused.
- Representative static-image throughput and two-pane composition remain bounded, exact, and free of idle work after completion; absolute image targets are set only after Slice 0 records a representative baseline.

## Dependency-ordered execution

### Slice 0 — clean baseline and benchmark calibration

Build release binaries from a clean commit and record exact source and binary identities. Run the non-graphical Phase 9 diagnostic first. Then, with explicit graphical-test approval, run one guarded Splinterm smoke before any matrix.

Refresh these current-source lanes:

- startup/idle at 80x24 and 240x80;
- plain, ANSI, Unicode, scrolling, and synchronized-output bursts;
- disabled and large scrollback;
- mixed retention with repeated output/clear/settle cycles;
- targeted input;
- resize and fractional scale;
- one-pane and two-pane text output;
- no-image idle; and
- representative bounded Sixel and Kitty static-image cases.

Calibrate screenshot polling by recording poll interval, marker-detection quantization, Wayland commit time, frame callback time, and screenshot detection separately. Add a direct client-state marker timestamp so renderer backlog can be distinguished from screenshot delay without claiming presentation.

Every graphical run in this and later slices requires fresh explicit approval for that run, an inactive workspace 8 on DP-2, pre-map placement and no-focus rules, one guarded smoke of the exact candidate binary, and verified cleanup before its matrix. A smoke from an earlier slice or build does not authorize or validate a later candidate.

**Gate:** all schemas, hashes, normalized configurations, fairness exceptions, sample counts, cleanup checks, and claim boundaries pass; the run identifies which provisional gates are already met and ranks the remaining gaps by absolute user impact and measured CPU/memory cost.

### Slice 1 — body-free stage instrumentation

Extend aggregate diagnostics across the complete path:

- PTY readiness, read calls/bytes, bytes left queued, and drain time;
- parser bytes/actions, synchronized boundaries, revisions, damage rows, history insertions, and PTY replies;
- publication transactions, retained update rows/cells, event clones, subscriber queue high water, overflow, and resnapshot;
- snapshot/update materialization counts, rows/cells/bytes, allocation counts where available, encode time, frame bytes, and socket backpressure;
- client wakeups, receiver drain batches, updates received/coalesced/applied, full reloads, dirty rows, scroll copies, and resyncs;
- row preparation, shaping, glyph-cache hits/misses, image preparation, full/partial paint, backing clear/copy bytes, SHM acquisition wait, damage regions, commit, callback, and draw coalescing;
- input receipt, client dispatch, daemon decode/authorization/control resolution, PTY queue admission, and PTY write completion; and
- live/retained bytes by canonical grid, scrollback, update history, protocol snapshot, client snapshot, prepared rows, glyph/font state, image state, backing canvas, and SHM.

Prefer counters and histograms over per-event logs. When exact allocator attribution needs profiling, use isolated Heaptrack, DHAT, `perf`, or equivalent runs outside the default path and record tool/version/overhead.

For cross-process traces, define an opt-in diagnostic run ID plus bounded transaction sequence/correlation IDs, use one documented Linux monotonic clock domain such as `CLOCK_MONOTONIC_RAW`, and assign interval ownership explicitly so queue wait and active work are neither omitted nor double-counted. Carry correlation metadata only through a private diagnostic side channel or separately reviewed diagnostic protocol extension; never place terminal bodies in it or silently change public/private production schemas.

**Gate:** instrumentation overhead passes the 2% rule and one correlated trace reconciles end-to-end time into named queue-wait and active-work stages with documented clock semantics and interval ownership.

### Slice 2 — PTY drain, terminal mutation, and daemon publication

Use Slice 1 evidence to isolate remaining child backpressure. Test one variable at a time:

1. avoid repeated allocation in damage/update construction while preserving per-action revisions;
2. reuse bounded row/cell/update storage where ownership permits;
3. avoid cloning unchanged metadata or history tails;
4. separate semantic revision accounting from expensive owned publication materialization without changing observable revision meaning;
5. publish bounded immutable transaction frames only where continuity and PTY reply ordering are proven; and
6. revisit parse/publication budgets only if stage data shows the retained 256-byte bound is now dominant.

Do not repeat rejected Plan 0009 experiments without new evidence. Larger parse batches previously slowed the diagnostic; transaction-level revision coalescing changed semantics for little gain.

Test no subscriber, publication without wire materialization, synchronized output, ordinary output, PTY replies, input during output, resize during output, overflow, exit, and detach/reattach. Fast/slow/multiple attached consumers remain required cases, but their absolute end-to-end gate follows Slice 3 because current publication constructs an owned snapshot consumed by protocol materialization.

**Gate:** the no-subscriber and publication-core stages meet the Slice 0/1 stage budget and materially improve the stage proven dominant; action/reply ordering, exact revisions, queue bounds, and resync behavior remain intact. Do not force the absolute attached-client gate if protocol materialization remains the measured bottleneck.

### Slice 3 — protocol materialization and client update application

Measure and remove work between publication and a prepared client frame, including the daemon's per-subscriber owned snapshot and wire-update materialization:

- avoid rebuilding complete owned snapshots for continuous compatible updates;
- retain exact immutable frame ownership without cloning unchanged visible rows, scrollback rows, palette, title, image metadata, or source leases;
- coalesce only immediately pending contiguous updates and preserve explicit resync on uncertainty;
- replace repeated `Vec`, `String`, row-index, and scrollback-history copies with bounded reuse or structural sharing where lifetimes remain clear;
- defer inactive-pane preparation only if focus activation rebuilds from the latest accepted revision with current scale, theme, palette/default colors, image sources/leases, and bounded focus-switch latency; and
- ensure title, cursor, authority, and control-only updates do not reshape rows.

The current client clones previous scrollback rows while observing history changes and can reload a full `SnapshotFrame` for full updates or non-live viewports. Treat those as hypotheses to measure, not pre-approved rewrites.

**Gate:** when Slice 0/1 proves this stage both dominant and large enough, client receive-to-prepared-frame p95 improves by at least 30%; otherwise require a statistically supported improvement proportional to the measured opportunity. The absolute attached-client bulk gates now pass; full reloads and resyncs remain bounded; exact incremental-versus-full frame tests pass for text, scrolls, images, scale, and panes; and focus-switch latency/state tests pass for deferred inactive panes.

### Slice 4 — row preparation, CPU composition, SHM, and Wayland scheduling

Optimize the measured render tail without changing terminal semantics:

- cache prepared row runs only with an exact identity or invalidation contract covering content, attributes, resolved font/fallback, scale, palette, default colors, theme/render options, and relevant image-source generations;
- shape only dirty clusters and reuse stable fallback/font decisions;
- preserve scroll-copy while reducing exposed-row and overlay repaint work;
- avoid full backing clears or full-surface copies only when each reusable SHM buffer tracks the frame/version it contains and reconciles every byte outside current damage from the persistent backing;
- account separately for the persistent backing copy and SHM canvas copy;
- keep the bounded two-buffer path and never allocate around compositor backpressure;
- merge compatible damage regions without promoting small updates to full-surface damage;
- preserve terminal-priority draw scheduling while preventing wake or draw storms; and
- keep compositor callbacks as flow-control evidence, not presentation timestamps.

Profile 80x24, 240x80, fractional scale, Unicode fallback, selection/URL overlays, pane chrome, scrolling, and image-free versus image-bearing frames.

**Gate:** receive-to-commit and bulk marker-visible gates pass; one-row echo remains within 5% of control; full and incremental captures remain byte-exact; alternating buffer release/reuse order tests prove submitted-buffer coherence; and SHM count and byte bounds do not grow.

### Slice 5 — scrollback, retention, and allocator behavior

First classify retained memory as live canonical data, duplicated derived data, mapped font/image pages, SHM/backing, or allocator high water. Then optimize only the measured classes:

- compact or reuse row/cell content without changing Unicode, style, image-anchor, or reflow semantics;
- avoid copying the bounded scrollback window during live updates when history did not change;
- measure snapshot/page byte distributions and enforce existing frame/admission failures without implicit truncation; adding byte-based truncation, continuation, or new limits requires a separately reviewed protocol contract defining direction, oversized single rows, omitted-row accounting, selection pins, and resync;
- release obsolete update history, renderer frames, image leases, and temporary decode/encode buffers promptly;
- preserve selected history endpoints and deterministic paging/resync;
- test allocator behavior before adding manual `shrink_to_fit` calls; and
- keep font mappings reclaimable and image content charged to existing budgets.

Run disabled, 1,000-line, large, wrap-heavy Unicode, clear-cycle, resize/reflow, selection-pinned paging, detach/reattach, and multi-pane cases.

**Gate:** memory and scrollback gates pass across two independent batches; exact reflow, row IDs, image anchors, selection pins, paging, and snapshot reconstruction pass; no memory fix merely shifts bytes to an uncounted process or mapping.

### Slice 6 — input, startup, idle, and lifecycle polish

Use stage timestamps before changing the interactive path.

For input, investigate redundant task hops, socket flush timing, protocol decoding/copies, authorization/control lookup, command-queue wakeups, and PTY write admission. Preserve fail-closed authorization and controller ownership.

For startup, separate client process creation, configuration/theme fingerprinting, font discovery/mapping, daemon handshake, initial snapshot, Wayland registry/configure, first preparation, first commit, child-ready, and mapped boundaries. Keep the prestarted-daemon claim explicit.

For idle, audit timers, cursor blink, theme watching, image-token expiry, receiver arming, calloop wake sources, frame callbacks, and inactive panes. Event-driven work must sleep when no deadline or state requires it.

Lifecycle persistence is intentional. Optimize shutdown/reap only if measurements show leaked work or missed bounds; do not force Splinterm to mimic peers that close the window on child exit.

**Gate:** input, startup, and idle gates pass in isolated current release builds; authorization, control transfer, shutdown, persistence, and process cleanup tests pass.

### Slice 7 — image, scale, resize, and multi-pane cost

Only after the ordinary text path is stable, measure representative image and complex-surface cases:

- bounded Sixel decode and static Kitty/PNG transfer;
- cache hit, cache miss, detach/reattach, and resync;
- crop, bilinear scale, alpha, z-order, and fractional scale;
- one active pane plus inactive panes;
- image-bearing scroll and resize; and
- completion followed by idle.

Investigate decoder allocation, source transfer, lease-set churn, prepared image rebuilding, full-pane reconstruction, bilinear composition, and unnecessary inactive-pane painting. Do not add scaled-surface caches unless a measured design fits existing byte budgets and has deterministic invalidation.

**Gate:** exact image captures and protocol fixtures pass; when Slice 0/1 proves an image stage dominant and large enough, representative cases improve it by at least 25%, otherwise require a statistically supported improvement proportional to the measured opportunity; image-free paths do not regress; cache, canonical-content, transfer, SHM, and idle bounds remain unchanged.

### Slice 8 — integrated closure

Run the complete non-graphical validation ladder. Then, only with explicit approval, execute the guarded graphical sequence:

1. verify workspace 8 on DP-2 is empty and inactive;
2. run one Splinterm smoke with pre-map placement and no-focus rules;
3. abort and clean up on any placement or focus violation;
4. run the 30-sample randomized matrix only after the smoke is valid and cleanup is verified; and
5. save new artifacts without altering any prior evidence.

Publish separate internal, Splinterm end-to-end, comparative, and graphical-approximation reports. Include candidate/control deltas, confidence intervals, quantile/bootstrap definitions, raw and invalid runs, predeclared invalidation/preflight rules, source/binary/config/font hashes, normalized peer configurations and fairness exceptions, host state, exact commands, and integrity hashes.

**Gate:** all provisional closure gates pass, recorded review finds no correctness or evidence blocker, package-source builds and extracted runtime validation pass, and the worktree is clean before final evidence is described as release-grade.

## Validation ladder

Run the smallest relevant checks after each coherent slice, then the full ladder at closure:

1. formatting, whitespace, schema, checksum, and fixture-vector checks;
2. terminal unit, property, chunking, fixture, final-buffer, reflow, image, and differential tests;
3. daemon actor, publication, PTY reply, queue, overflow, snapshot, paging, detach/reattach, lifecycle, and shutdown tests;
4. renderer incremental/full equivalence, glyph/font, scale, image, pane, selection, and capture tests;
5. protocol, CLI, automation, MCP, policy, authorization, and controller compatibility tests;
6. non-graphical microbenchmarks and current/control stage comparison;
7. bounded parser fuzzing and relevant sanitizer runs;
8. workspace tests and warning policy recorded honestly;
9. package build and extracted runtime validation; and
10. approved one-case graphical smoke followed by the guarded matrix.

Use release binaries for retained performance evidence. Debug improvements are diagnostic only.

## Commit and review strategy

- Keep one active-worktree writer.
- Commit Slice 0 instrumentation/calibration separately from optimization changes.
- Prefer one measured hypothesis per implementation commit.
- Record rejected experiments with their data; remove diagnostic-only code unless it is cheap, bounded, and intentionally retained.
- Use fresh read-only review after daemon publication changes, renderer/SHM changes, memory-layout changes, and before closure.
- Do not combine optimization work with unrelated feature or protocol expansion.

## Stop-loss

Stop and reassess when:

- a candidate cannot name the measured stage it improves;
- instrumentation changes the measured result beyond its 2% budget;
- an optimization changes parser-visible behavior, PTY replies, revision meaning, or exact reconstruction;
- queue, cache, SHM, thread, scrollback, or image limits must increase to show improvement;
- bulk throughput improves while small writes, input, resize, or idle regress;
- resnapshot, full reload, full redraw, or wakeup frequency rises under slow-consumer or multi-pane load;
- memory appears lower only because bytes moved outside the measured process tree or mapping class;
- a guarded smoke violates workspace, monitor, focus, or cleanup isolation;
- two controlled experiments fail to improve the hypothesized stage; or
- the dominant bottleneck moves outside the authorized slice.

## Progress record

- [First measured optimization pass](../spikes/artifacts/0026-performance-optimization-pass/README.md):
  accepted terminal scroll/update ownership improvements, rejected unsafe or
  regressing experiments, complete sequential workspace validation, two review
  rounds, and remaining Foot gaps. This is development evidence, not closure.

## Expected completion record

At closure, append:

- clean baseline commit and binary identities;
- final commit and binary identities;
- per-stage before/after budgets;
- accepted and rejected hypotheses;
- comparative and Splinterm-only result tables;
- correctness, fuzz, package, and review evidence;
- remaining gaps and deliberately deferred architectural work; and
- links to immutable raw artifacts and checksums.
