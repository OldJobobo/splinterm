# Plan 0022: history-heavy graphical catch-up

- **Status:** In progress
- **Date:** 2026-08-05
- **Parent performance plan:** [Plan 0010](0010-full-performance-optimization-pass.md)
- **Benchmark baseline:** [Plan 0016 publication](../benchmarks/artifacts/2026-08-05-plan0016-publication/README.md)
- **Architecture coordination:** [Plan 0020](0020-client-module-decomposition.md)
- **Behavioral authority:** Foot 1.27.0 commit `3c5b584b0eafa772eb4376fb6eaf6643399e190e`

## Decision

Run a profiling-first optimization pass over the Splinterm graphical client's
update-to-commit path, with explicit history-size and pane-count dimensions.
Prioritize removing avoidable work from the Wayland event-loop thread before
considering worker-thread rendering or a renderer replacement.

This is a focused execution plan for the client-side portions of Plan 0010. It
does not reopen Plan 0010's daemon publication, protocol throughput, input, or
retained-memory scope unless a correlated trace proves those stages dominate the
new workload. It must not be combined with Plan 0020's mechanical module moves:
behavioral optimization and module extraction remain separately reviewed
changes.

The Plan 0016 four-pane input result—approximately 15.6 ms to child receipt and
807.1 ms to screenshot marker observation—motivates investigation but does not
prove an approximately 800 ms rendering delay. Marker polling launches `grim`,
writes a PNG, and scans pixels serially. The observation may include one or more
capture cycles. Internal revision-to-commit and frame-callback boundaries must
be measured before attributing the gap.

## Execution progress

- Milestone 0A's body-free stage-trace v2, transaction-to-pane commit
  correlation, surface commit/callback ownership, strict v1/v2 summarizer, and
  deterministic tests are implemented and independently approved.
- Milestone 0B's versioned plan/report schemas and portable finite-manifest
  builder are implemented and independently approved. The default
  3-warmup/10-measured plan contains 130 scheduled cases across the ten fixed
  diagnostic cells and does not launch graphical work.
- The non-graphical terminal-state timing harness now exercises the real update
  and viewport reducers across 0/1,000/4,096 cached rows, live/detached state,
  1/2/4 pane targeting, focused/all/inactive activity, and one-update/2,000-line
  shapes. Its fixed full contract is five warmups and 30 interleaved samples;
  the bounded smoke runs small-update cases only. A separate bounded `PaneView`
  reducer harness now covers 1/16/64-update focused-role and inactive batches
  across 0/1,000/4,096 cached rows and live/detached state. Its report explicitly
  limits focused-role interpretation to the shared semantic reducer rather than
  claiming coverage of the full `App::apply_updates` active path.
- Milestone 1's live-viewport fast path is implemented: previous history rows
  are cloned only while the viewport is detached. Matched release 5/30 reducer
  diagnostics remove ordinary live-update history amplification (98%+ p95
  improvement at 4,096 rows and 99%+ for four-pane all-active small updates,
  including one-sided bootstrap bounds). The zero-history microsecond-scale
  upper bound remains statistically unresolved, and 2,000-append work still
  identifies history bounding/scanning as a separate later candidate.
- The guarded seed-`220022` graphical diagnosis completed against plan digest
  `091cc4d798e0ed460f421902713d1bd17efc2f1e68379f1859da372739d87869`:
  30 warmups and 100 measured cases cover all ten cells, with 130/130 valid
  execution indexes and verified cleanup. The compact publication is
  [2026-08-06 Plan 0022 history catch-up](../benchmarks/artifacts/2026-08-06-plan0022-history-catchup/README.md).
- Focused 4,096-row small updates measure 8.029 ms median / 10.025 ms p95
  receive-to-commit. Four-pane focused-only scaling measures 8.860 / 10.423 ms;
  its four-pane/one-pane p95 ratio is 1.040 with a one-sided 95% upper bound of
  1.165, passing the 1.25 gate. Static movement causes zero semantic applies,
  history clones, pane-frame rebuilds, or configure events.
- Four-pane all-active updates remain above the ordinary focused target at
  49.078 ms median / 65.172 ms p95. The paced 2,000-line ANSI stress case remains
  slow at 293.804 / 379.673 ms receive-to-commit. No measured trace resynced,
  saturated, became ambiguous, or failed cleanup.
- No sampled profile was added: the complete body-free stage traces already name
  the remaining bounded slow paths, and no further optimization candidate is
  being implemented in this slice. The graphical matrix is candidate-only, so
  graphical control-relative history amplification and zero-history regression
  remain explicitly unresolved. The matched non-graphical control/candidate
  evidence and its zero-history confidence uncertainty remain authoritative.

## Current evidence and hypotheses

The following source observations are facts:

- The Milestone 1 candidate avoids cloning previous `scrollback_rows` for live
  viewports in active, inactive, and snapshot replacement paths; detached
  viewports retain the exact previous rows needed for anchor accounting.
- `observe_history_change()` returns immediately for a live viewport after its
  generation/alternate-screen checks, so the omitted live clone cannot affect
  viewport behavior.
- The history cache is bounded to 4,096 rows and 16 MiB per terminal.
- History bounding recomputes total row bytes and may repeatedly remove index
  zero from a `Vec`.
- Inactive updates are drained and reduced, then dirty inactive frames are
  rebuilt during `App::apply_updates()` before the next draw.
- A size-changing Wayland configure clears the SHM buffer list, marks a full
  redraw, emits terminal resize, and schedules drawing.
- Terminal redraw scheduling coalesces while a frame callback is pending;
  `terminal_draw_waits_for_frame()` currently ignores available-buffer capacity.
- Existing body-free performance stages reach from terminal mutation through
  `client_apply`, `frame_prepare`, `pane_commit`, and `draw_commit`, but do not yet fully
  describe history-copy volume, inactive-pane rebuilds, configure bursts,
  buffer acquisition, or compositor callbacks.

The retained evidence resolves or bounds the original hypotheses:

1. live-viewport history cloning was material in matched reducer controls and is
   eliminated in graphical live-update traces; detached viewports intentionally
   retain exact previous-row work;
2. inactive-pane scaling does not materially delay the focused pane in this
   matrix: the four-pane/one-pane p95 ratio is 1.040 (UCB 1.165);
3. pure same-output movement performs no semantic or frame-rebuild work, while
   the paced twelve-step resize lane still performs multiple pane preparations
   and remains a separate future candidate rather than a Milestone 1 blocker;
4. callback p95 exceeds one refresh interval in several cells, but screenshot
   quantization is much larger and cannot be interpreted as presentation;
5. the paced 2,000-line ANSI workload remains expensive after the clone removal,
   consistent with repeated history bounding/scanning and terminal admission
   work being a separate candidate;
6. no ordinary focused small-update client stage exceeds 50 ms; all-active and
   ANSI stress are the retained bounded exceptions; and
7. the Plan 0016 screenshot gap is not an approximately 800 ms ordinary focused
   render path: candidate receive-to-commit is 1.563–10.025 ms p95 across the
   focused live-history curve, while screenshot polling remains coarsely
   quantized.

## Goals

- Correlate one terminal revision from client receipt through semantic apply,
  frame preparation, surface commit, and compositor callback.
- Quantify event-loop work by pane count, cached-history size, viewport state,
  update activity, and geometry change.
- Remove full-history copying from ordinary live-viewport updates when behavior
  does not require it.
- Ensure inactive panes do not perform duplicate preparation for intermediate
  states that cannot be presented.
- Coalesce resize/configure work without stale pixels, incorrect terminal
  geometry, or delayed final settlement.
- Preserve focused-pane responsiveness as inactive pane history grows.
- Leave body-free regression counters, deterministic tests, and a guarded
  graphical workload that can detect recurrence.

## Non-goals

- Replacing the CPU renderer or adopting a GPU backend in the initial pass.
- Treating screenshot polling as compositor presentation or input-to-photon
  latency.
- Optimizing until Splinterm wins a peer table or changing the benchmark after
  seeing candidate results without independent calibration.
- Increasing history, queue, SHM, cache, channel, or thread bounds as a fix.
- Dropping valid revisions, rendering a stale pane after focus, or weakening
  lag/resync behavior.
- Changing daemon ownership, topology, authorization, controller, lifecycle,
  detach/reattach, or protocol compatibility.
- Hiding performance changes inside Plan 0020 module extraction.
- Running graphical tests without the separate guarded approval required by
  `AGENTS.md`.

## Invariants

Every retained slice preserves:

1. exact terminal snapshot/update reconstruction and monotonic revisions;
2. parser, PTY reply, final-update, exit, lag, and resync ordering;
3. bounded history, update queues, receiver drains, prepared frames, glyph
   caches, backing storage, SHM buffers, and worker counts;
4. detached viewport anchoring, unseen-row accounting, history clear/trim,
   alternate-screen, selection, search, image, and focus-switch behavior;
5. current full/partial damage correctness, surface scale, fractional scale,
   resize, and output migration semantics;
6. daemon authority and disposable-client architecture;
7. body-free diagnostics with no terminal, input, clipboard, or image payloads;
8. the pinned Foot oracle, canonical checkout cleanliness, and unchanged
   comparison images and tolerances; and
9. exact graphical placement, ownership, focus, and cleanup guardrails.

## Measurement contract

### Primary boundary

The primary optimization boundary is correlated
`client_receive -> pane_commit`, where `pane_commit` associates one terminal
transaction with the exact timestamp of its containing `wl_surface.commit`.
`draw_commit` owns surface-wide work and bytes without duplicating those totals
per pane. Record the compositor frame callback as a separate scheduling
observation. Do not claim actual presentation unless a
reviewed presentation-timing protocol supplies that timestamp.

Milestone 0 must introduce an explicit `splinterm.performance.stage.v2` rather
than silently extending v1. Update the emitter, summarizer, and stage-trace tests
together. The schema must define every field's type and bound; use subscription
and transaction sequence with pane incarnation and revision for correlation;
represent revisions coalesced, superseded, or uncommitted before commit;
associate callbacks with the surface commit they release; and compute
`client_receive -> pane_commit` directly. Document interval ownership so queue wait, active work,
commit-to-callback wait, and screenshot observation neither overlap nor leave an
unattributed gap.

For each correlated revision, retain only bounded metadata:

- run, process, splint, incarnation, revision, subscription, and transaction
  identities;
- pane role: focused, visible inactive, hidden, or detached viewport;
- pane count, active updating pane count, terminal rows/columns;
- cached history rows and estimated bytes before and after apply;
- copied history rows/bytes and history scan/trim counts;
- receiver batch size, queue depth, contiguous updates reduced, and resync/full
  reload flags;
- semantic rows changed, dirty visible rows, and full-frame reason;
- inactive panes marked dirty, prepared, skipped, or superseded;
- configure count coalesced into the draw, old/final geometry, and scale change;
- frame preparation duration, prepared rows/cells, glyph cache hits/misses, and
  image-source generation changes;
- backing clear/copy bytes, damage region count/area, SHM acquisition duration,
  buffer availability, commit time, callback time, and callbacks coalesced; and
- event-loop iteration active time plus the longest named stage.

Counters and histograms are preferred over unbounded event logs. New tracing
must remain opt-in and pass Plan 0010's instrumentation-overhead gate. The v2
summarizer must reject unknown fields, saturated traces, ambiguous correlation,
and impossible stage order as strictly as v1.

### Screenshot calibration

Record separately:

1. marker trigger and child receipt;
2. the matching Splinterm revision's receive, apply, prepare, and commit;
3. frame callback;
4. each screenshot capture start/end and marker result; and
5. PNG decode/pixel-scan completion.

The peer-capable screenshot lane remains the external comparison. Splinterm
optimization decisions use the internal correlated boundary. If a persistent
screencopy probe is proposed, it requires its own correctness, overhead,
security, and graphical-safety review.

### Focused workload matrix

Milestone 0 must add versioned plan/report schemas and portable tests before any
graphical execution. Use `tools/benchmark/graphical-catchup-plan-schema.json`,
`tools/benchmark/graphical-catchup-report-schema.json`,
`tools/benchmark/graphical_catchup.py`,
`tools/benchmark/run-graphical-catchup.py`, and
`tools/benchmark/test_graphical_catchup.py` unless implementation review finds a
more cohesive name before code lands. The harness must preload and verify actual
cached row counts, prove live/detached viewport transitions, distinguish
focused-only from all-pane activity, and timestamp screenshot capture start/end
plus decode/scan completion. Preconditioning must fill the 4,096-row cache
before a measured 2,000-line operation rather than assuming that workload
reaches the bound.

Development diagnosis uses three warmups and ten measured samples for this
finite native matrix:

| Lane | Panes | Cached rows per pane | Viewport/activity | Operation |
| --- | ---: | ---: | --- | --- |
| zero-history control | 1 | 0 | live, focused only | small marker/input echo |
| history curve | 1 | 1,000 and 4,096 | live, focused only | small marker/input echo |
| detached history | 1 | 4,096 | detached, focused only | continuing small output |
| inactive scaling | 2 and 4 | 4,096 | live, focused only | small marker/input echo |
| all-pane scaling | 4 | 4,096 | live, all panes | small marker/input echo |
| static movement control | 4 | 4,096 | live, idle | same-output position movement |
| resize | 4 | 4,096 | live, idle then focused output | twelve-step outer resize |
| ANSI stress | 4 | 4,096 | live, all panes | bounded 2,000-line ANSI output |

Movement must distinguish pure position changes from configure and
output-enter/leave events. Output/scale migration is deferred to a separately
approved sequence because the default guard requires the window to remain on
workspace 8 / DP-2. Do not weaken that guard to create a migration sample.

Graphical execution occurs only after explicit approval for the complete bounded
manifest. It remains isolated to workspace 8 / DP-2, starts with one smoke, and
aborts on placement, focus, ownership, targeting, or cleanup failure.

## Dependency-ordered milestones

### Milestone 0 — calibration and attribution

1. Implement the v2 trace schema, emitter, strict summarizer, correlation rules,
   callback stage, and focused `test_benchmark.py` coverage described above.
2. Add the versioned graphical plan/report schemas, finite manifest builder, and
   portable tests for history preload, viewport transitions, activity modes,
   and capture-phase evidence.
3. Add a terminal-state microbenchmark or deterministic timing harness that
   varies history rows, viewport state, pane count, and update shape without
   Wayland or screenshots.
4. Add a bounded client reducer benchmark for active and inactive update batches.
5. Measure trace-disabled overhead with interleaved release runs.
6. After separate graphical approval, run one guarded smoke, then the focused
   matrix against an exact release binary.
7. Produce flamegraphs or equivalent sampled profiles for only the slowest
   history-heavy static, output, and resize cells. Record tool versions and
   binary hashes.

Gate: the v2 schema and harness tests pass; one trace reconciles
receive-to-commit time without saturated tracing, ambiguous coalescing, or
missing stage ownership; screenshot-capture cost is separated; and the dominant
client-side stage or copy count is named. If client-side work is not dominant,
stop and amend the plan rather than implementing the candidates below.

### Milestone 1 — remove unnecessary history copying

Preferred first candidate:

- give `ScrollbackViewport` an explicit live/detached transition contract;
- avoid constructing previous-row history when the viewport is live;
- for detached viewports, replace whole-history cloning and set construction
  with the smallest exact row-ID/anchor transition metadata supported by the
  measured update shapes; and
- preserve generation changes, alternate screen, clear, trim, eviction, unseen
  rows, and anchor loss behavior.

Measure history-byte scans and front trimming separately. Replace repeated full
byte scans or `Vec::remove(0)` only if they remain material after the copy fix;
prefer batch trimming or maintained bounded byte accounting over a container
rewrite without evidence.

Gate: no live update allocates or copies the previous full history; detached
viewport tests remain exact; and client-apply p95 improves by at least 30% when
the removed work accounted for at least 20% or 5 ms of control
receive-to-commit p95. At lower measured opportunity, require a confidence-bound
improvement proportional to the removable stage rather than manufacturing a
30% target. The maximum-history/zero-history amplification ratio at fixed pane
count and activity must fall by at least 50% without increasing retained-memory
bounds.

### Milestone 2 — coalesce inactive-pane work

- Preserve semantic application of every required contiguous revision while
  preparing only the newest presentable state per inactive pane per compositor
  frame.
- Separate “semantic state dirty” from “prepared frame ready.”
- Prepare the focused pane first.
- Lazily prepare an inactive pane when it becomes visible/focused, keyed by exact
  revision, geometry, scale, theme, palette/default colors, renderer options,
  and image-source generation.
- Discard stale prepared work rather than presenting it.
- Add a per-frame inactive preparation budget only if tracing proves one is
  needed and focus-switch settlement remains bounded.

Gate: the four-pane/one-pane focused-only p95 ratio at fixed maximum history is
no greater than 1.25; each inactive pane is prepared at most once for the newest
eligible state per frame; focus-switch pixels/state remain exact; and queue,
reload, resync, and memory bounds do not increase.

### Milestone 3 — configure, damage, and buffer efficiency

Use traces to retain only relevant candidates:

- coalesce configure bursts to the latest acknowledged geometry that can be
  rendered for a frame;
- avoid buffer clearing/reallocation when dimensions and scale are unchanged;
- reuse backing capacity where the SHM and damage contracts permit it;
- preserve prepared terminal content when only destination position changes;
- rebuild all pane frames only when geometry, scale, font metrics, theme, or
  another exact frame key changes;
- keep terminal resize requests and final surface settlement ordered; and
- revisit frame-callback gating or buffer-capacity use only when measured wait
  exceeds the expected compositor cadence.

Gate: same-output position movement performs no semantic history work or pane
frame rebuild in a static window; resize performs no more than one expensive
preparation per presentable compositor frame; final geometry and pixels are
exact; and resize settlement and RSS improve without callback storms or SHM
budget growth.

### Milestone 4 — renderer isolation only if still justified

If row preparation, rasterization, or backing composition remains the dominant
event-loop stage after Milestones 1–3, propose a separately reviewed bounded
worker design:

- immutable work descriptors keyed by pane identity, revision, frame key, and
  damage;
- bounded queue and worker count;
- stale-result cancellation/drop;
- Wayland proxy, SHM attachment, damage, and commit retained on the event-loop
  thread; and
- deterministic shutdown, resize, focus, theme, image, and lifecycle handling.

This milestone is not pre-approved implementation. Stop for architecture review
before adding worker threads.

### Milestone 5 — closure

1. Run focused deterministic tests, Foot-derived renderer/final-buffer tests,
   terminal/client/daemon suites, Clippy, formatting, and diff checks.
2. Run tracing-overhead and exact-binary release gates.
3. After graphical approval, use 3 warmups/10 measured samples only for
   diagnosis. Retained affected control/candidate cells require interleaved 5
   warmups/20 measured samples. A renewed peer-comparative claim requires 5
   warmups/30 measured randomized blocks.
4. Apply Plan 0010's nearest-rank p95, bootstrap confidence intervals, one-sided
   upper-bound decisions, predeclared invalidation criteria, and complete-matrix
   rerun rule when invalid samples exceed 5%.
5. If the focused gates pass, run the affected Plan 0016 native cells and retain
   peer controls only where the claim requires comparison.
6. Publish a new dated artifact with source/binary identities, raw checksums,
   invalidation reasons, summaries, and independent review.

Plan 0016 evidence remains immutable. Never replace or silently regenerate its
tracked publication artifact.

## Provisional acceptance gates

Milestone 0 may tighten these gates from current exact-binary evidence. It may
not loosen them merely to retain a candidate.

Every numeric gate applies both to its point estimate and the relevant bootstrap
one-sided 95% upper confidence bound. Candidate improvement is
`1 - candidate/control`. History amplification is
`maximum-history p95 / zero-history p95` at fixed pane count, viewport, activity,
and operation. Inactive-pane scaling is the four-pane/one-pane focused-only p95
ratio at fixed maximum history. Use nearest-rank p95.

- Ordinary focused small updates: correlated receive-to-commit median at or
  below one 60 Hz frame period and p95 at or below 50 ms.
- History-heavy four-pane updates: at least 50% lower p95 receive-to-commit than
  the exact control and no more than 10% regression in the zero-history cell.
- Focused-pane p95 with three inactive history-heavy panes: no more than 25%
  above the equivalent one-pane history-heavy case after Milestone 2.
- No event-loop active interval above 50 ms without a retained named-stage
  attribution; closure requires eliminating or explicitly bounding each one.
- Same-output movement of a static terminal: zero terminal semantic applies,
  history clones, or pane-frame rebuilds caused solely by position changes.
- No normal attached-client resync, subscriber overflow, stale prepared-frame
  presentation, callback storm, or cleanup failure.
- No increase to history, queue, cache, SHM, backing, thread, or retained-memory
  limits.
- Screenshot results are reported with capture quantization and may not replace
  the correlated internal gate.

## Non-graphical validation baseline

Exact commands may be narrowed per slice. Retained slice closure must include:

```bash
cargo fmt --all -- --check
cargo test -p splinterm viewport
cargo test -p splinterm terminal_state
cargo test -p splinterm inactive
cargo test -p splinterm
cargo test -p splinterd --lib
cargo clippy -p splinterm -p splinterd --all-targets -- -D warnings
python tools/performance/summarize-stage-trace.py TRACE_DIR SUMMARY.json --run-id RUN_ID
python -m pytest -q tools/benchmark/test_benchmark.py -k stage_trace
python -m pytest -q tools/benchmark/test_multiplexer_matrix.py \
  tools/benchmark/test_graphical_catchup.py
ruff format --check tools/performance/summarize-stage-trace.py \
  tools/benchmark/graphical_catchup.py \
  tools/benchmark/run-graphical-catchup.py \
  tools/benchmark/test_graphical_catchup.py
ruff check tools/performance/summarize-stage-trace.py \
  tools/benchmark/graphical_catchup.py \
  tools/benchmark/run-graphical-catchup.py \
  tools/benchmark/test_graphical_catchup.py
git diff --check
```

The trace command's `TRACE_DIR`, `SUMMARY.json`, and `RUN_ID` are runtime values,
not literal placeholders. A command whose test filter matches no tests is not
evidence, so record the actual test count. Broad pre-existing Ruff debt is
outside this slice unless a changed file cannot pass independently.

Milestone 5 is non-release slice closure. Clean-commit package construction,
extracted-runtime validation, local installation, and release-grade integrated
closure remain Plan 0010 responsibilities. No package installation or packaged
binary replacement is authorized by this plan. Expensive full-suite or graphical
reruns occur only at coherent milestone boundaries.

## Current closure decision

This slice stops after Milestone 1 and the diagnostic matrix. It does not claim
Milestones 2–4 were implemented. The focused live-history fast path is supported
by matched non-graphical control/candidate evidence and candidate graphical
attribution. Ordinary focused graphical updates, inactive scaling, movement,
trace integrity, and cleanup pass their applicable gates.

The following are recorded residuals rather than hidden passes:

- the non-graphical zero-history candidate/control ratio upper bound remains
  above the 1.10 regression limit at a microsecond-scale boundary;
- no graphical control binary was run, so graphical control-relative history
  amplification and zero-history regression are not claimed;
- all-pane and ANSI-stress receive-to-commit remain above 50 ms p95;
- resize still performs multiple pane preparations across its twelve paced
  configure steps; and
- sampled profiling and further Milestone 2–4 implementation are deferred
  because the retained stage traces already bound the current slow paths and no
  additional candidate is part of this slice.

No additional broad graphical matrix is required to publish these bounded
results. Any future optimization must start from a new focused plan and reuse
this artifact as immutable evidence.

## Stop gates

Stop and request a decision before:

- adding renderer worker threads or changing architecture ownership;
- changing private or public protocol schemas for diagnostic correlation;
- changing benchmark workload bytes, screenshot method, or comparison semantics
  after a candidate result is known;
- increasing any bound or tolerance;
- combining optimization with Plan 0020 module movement;
- replacing or deleting retained Plan 0016 evidence;
- installing a development client or replacing a packaged binary; or
