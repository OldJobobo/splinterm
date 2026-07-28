# Plan 0011 closure evidence — 2026-07-26

## Decision

**Closure gate failed; no graphical window was launched.** The exact non-graphical delayed compact-subscriber case retained **43.88 MiB RSS** and **43.88 MiB PSS** after the 5,000-line mixed workload and two-second settle. This exceeds Plan 0011's strict minimum useful threshold of `<24 MiB`.

Attribution names the retained class: **43.82 MiB private anonymous**, with compact queue high water **64 snapshots / 691,360 cells**. The capacity-one overflow control retained **6.88 MiB**, confirming queued snapshots are the dominant avoidable class. No limit was changed to obtain either result.

## Instrumentation overhead

The preserved 15-sample attempt missed confidence bounds despite neutral point estimates. The pre-declared bounded 40-sample interleaved run passed unchanged limits:

| Metric | Default-off median | Enabled median | Point regression | One-sided 95% upper | Limit |
|---|---:|---:|---:|---:|---:|
| Output | 43.130 ms | 43.110 ms | -0.047% | 1.801% | 2.0% |
| Process CPU | 94.862 ms | 95.019 ms | +0.165% | 1.297% | 2.0% |
| Small write | 11.688 ms | 11.625 ms | -0.541% | 0.576% | 5.0% |


## Non-graphical cases

| Case | RSS growth | PSS growth | Private-anon growth | Notes |
|---|---:|---:|---:|---|
| no-subscriber | 2.40 MiB | 2.40 MiB | 2.21 MiB | no subscriber |
| fast | 60.64 MiB | 60.64 MiB | 60.57 MiB | legacy public fast subscriber diagnostic |
| delayed | 43.88 MiB | 43.88 MiB | 43.82 MiB | optimized compact delayed subscriber |
| overflow | 6.88 MiB | 6.88 MiB | 6.75 MiB | compact subscriber, capacity one |
| scrollback-disabled | 0.80 MiB | 0.80 MiB | 0.61 MiB | no subscriber, scrollback zero |
| scrollback-1000 | 2.41 MiB | 2.41 MiB | 2.29 MiB | no subscriber, 1,000 rows |
| multiple | 850.79 MiB | 846.81 MiB | 846.40 MiB | two legacy public fast subscribers diagnostic |


The five-cycle overflow probe's daemon-only endpoint slope was **0.070 MiB/cycle**. The child-inclusive 120-second final growth was **6.99 MiB**; the final sample includes the held workload `sleep` child.

## Validation

- `cargo test -p splinterm-terminal`: passed.
- `cargo test -p splinterd`: 15/16 integrations; known concurrent policy test timed out.
- Exact isolated policy test: passed in 14.98 s.
- Protocol, automation client, Splinterm library, and benchmark pytest: passed.
- `cargo test --workspace -- --test-threads=1`: passed.
- Harness pytest: 29 passed.

## Graphical boundary

Read-only preflight proved workspace 8 empty/inactive on DP-2 while the user's active workspace remained 1 on DP-1. Because the non-graphical threshold failed, the guarded candidate smoke and all comparison cases were **not run**, as required by the stop-loss.

## Identities and raw evidence

- [Binary/source identities](identities.json)
- [Analysis](analysis.json)
- [Instrumentation overhead](non-graphical/overhead-40/summary.json)
- [Daemon cases](non-graphical/daemon/)
- [Validation statuses](validation/status.tsv)
- [Graphical preflight](graphical-preflight.json)

Plan 0011 is not closed. The remaining implementation target is Slice 2: retain at most one latest full snapshot per subscriber while preserving revision and resnapshot semantics.
