# Plan 0043 sparse candidate attribution — Experiment 1: sealed chunks retaining individual sparse frames

**Rejected memory experiment.** The candidate removes the retained-frame defect but fails the mandatory daemon-retention boundary.

Randomization seed: `43`
Warmups: 2
Measured samples per variant: 10

| Variant | RSS/PSS growth | Private-anon growth | CPU ticks | Marker latency | Events | Batch HWM | Update HWM | Resync |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Integrated Plan 0042 | 7.94 MiB | 7.87 MiB | 14 | 130.43 ms | 69.5 | 64 | 15556 | 0 |
| Sparse candidate | 23.33 MiB | 23.26 MiB | 14 | 119.88 ms | 64 | 8 | 1949 | 0 |

All 20 measured cases completed. The generic baseline-defect runner records `valid: false` and exits 1 because its old success predicate requires both variants to reproduce the 64-batch defect; the sparse candidate intentionally does not. This Markdown is the candidate interpretation of the unchanged raw records and `summary.json`. No graphical process or user window participated.
