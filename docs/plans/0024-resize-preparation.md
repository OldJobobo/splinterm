# Plan 0024: resize preparation attribution

- **Status:** Complete
- **Date:** 2026-08-06
- **Parent plan:** [Plan 0022](0022-history-heavy-graphical-catch-up.md)
- **Evidence:** [Plan 0024 resize preparation](../benchmarks/artifacts/2026-08-06-plan0024-resize-preparation/README.md)

## Decision

Resolve Plan 0022's remaining resize-preparation question from the retained ten
measured outer-resize traces before changing configure, damage, SHM, or terminal
resize behavior. Implement a change only if repeated expensive preparation is
proven. Do not delay visible resize settlement or add workers, queues, timers,
protocol state, memory bounds, or another graphical matrix to manufacture an
optimization target.

## Scope and invariants

The bounded workload is Plan 0022's four-pane, 4,096-row, twelve-step outer
resize cell. The retained trace interval begins at the first of the final twelve
configure events and ends at the exact `pane_commit` whose same-clock correlated
`client_receive -> pane_commit` duration equals the report's final-marker
boundary. All interval timestamps therefore use `CLOCK_MONOTONIC_RAW`.

A content preparation is a `frame_prepare` with either `full_reload=true` or
nonzero dirty rows. A configure-only refresh has neither. Content preparations
are keyed by exact splint, incarnation, and revision. The decision must preserve:

- every required terminal-grid revision and final marker revision;
- immediate configure handling and final geometry settlement;
- full-damage behavior when dimensions change;
- terminal resize ordering and active/inactive pane correctness;
- SHM and backing-store bounds; and
- Plan 0022's graphical placement, ownership, and cleanup guarantees.

## Result

All ten measured cases contain twelve configure events. Median per sequence:

- 25 frame preparations;
- 13 content preparations for 13 distinct terminal revisions;
- 12 configure-only refresh preparations totaling 0.073 ms;
- 35.116 ms total content preparation;
- 43 draw commits totaling 18.201 ms and 84.792 MB of backing copies across the
  deliberately paced multi-second sequence.

Across all ten cases there are **zero duplicate content preparations**. Each
sequence contains twelve required resize-response revisions plus the final
marker revision. The twelve apparent duplicates are configure/cursor refreshes;
they cost only 0.064–0.092 ms for the complete sequence.

## Outcome

No production change is justified. Deferring configure draws until asynchronous
pane responses arrive would risk stale panes and delayed resize feedback to
remove work below the measurable opportunity threshold. Coalescing distinct
terminal revisions would violate the workload's exact settlement contract. The
existing implementation already meets the substantive Plan 0022 gate of no more
than one expensive preparation per required presentable revision.

## Validation

- Reproduce `aggregate.json` byte-for-byte with the retained analysis script.
- Verify all Plan 0022 retained-raw checksums.
- Verify ten measured cases, twelve configures each, twelve full-reload resize
  preparations, one dirty final-marker preparation, and zero duplicate content
  preparations.
- Run Python formatting/lint/byte-compilation and JSON/diff checks.
- Obtain independent read-only review before marking this plan complete.

No graphical work is required or authorized by this plan.
