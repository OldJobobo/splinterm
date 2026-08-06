# Plan 0024 resize preparation evidence

This compact publication resolves Plan 0022's remaining resize-preparation
question without running another graphical workload.

The analysis reads the ten retained measured `outer-resize` cases from
`benchmark-results/20260806T030231Z-plan0022-graphical-catchup-matrix-final/`.
Each interval is bounded by its final twelve configure events and the exact
`pane_commit` whose same-clock correlated `client_receive -> pane_commit`
duration equals the report's final-marker boundary. All compared interval
values use `CLOCK_MONOTONIC_RAW`. A content preparation has `full_reload=true`
or nonzero dirty rows; the remaining preparations are configure-only
cursor/frame refreshes.

## Result

| Per twelve-step sequence | Median | Range |
| --- | ---: | ---: |
| Configure events | 12 | 12 |
| All frame preparations | 25 | 25–26 |
| Content preparations | 13 | 13 |
| Distinct content revisions | 13 | 13 |
| Configure-only refresh preparation | 0.073 ms | 0.064–0.092 ms |
| Content preparation | 35.116 ms | 33.474–38.176 ms |
| Draw commits | 43 | 39–47 |
| Draw-commit work | 18.201 ms | 16.779–20.489 ms |
| Backing copies | 84.792 MB | 76.618–92.966 MB |

There are zero duplicate content preparations across all ten measured cases.
Each case has twelve full-reload terminal resize preparations and one dirty-row
final-marker preparation, all with distinct exact splint/incarnation/revision
keys. The apparent second preparation per configure is a microsecond refresh,
not repeated row preparation.

## Decision

No production optimization is justified. Delaying configure draws for
asynchronous pane responses risks stale content and resize feedback for at most
0.092 ms of removable preparation per complete sequence. Distinct terminal
revisions must not be discarded.

The draw-copy totals are complete-stack diagnostic work over a deliberately
paced multi-second sequence, not a latency claim. They do not justify changing
SHM bounds, callback scheduling, or pane settlement without a new correlated
slow-path observation.

## Contents

- `analyze.py`: bounded deterministic retained-trace analysis.
- `aggregate.json`: all ten case summaries and median decision values.
- `PROVENANCE.json`: exact source identities and commands.
- `SHA256SUMS`: compact publication checksums.

The authoritative raw evidence remains locally retained and checksum-bound by
Plan 0022. No graphical command was run for Plan 0024.
