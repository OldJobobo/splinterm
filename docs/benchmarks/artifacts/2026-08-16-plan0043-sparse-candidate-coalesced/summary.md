# Plan 0043 sparse candidate attribution — Experiment 2: producer-frame update coalescing

**Rejected memory experiment.** The candidate removes the retained-frame defect but fails the mandatory daemon-retention boundary.

Randomization seed: `43`
Warmups: 2
Measured samples per variant: 10

| Variant | RSS/PSS growth | Private-anon growth | CPU ticks | Marker latency | Events | Batch HWM | Update HWM | Resync |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Integrated Plan 0042 | 8.97 MiB | 8.88 MiB | 14 | 126.59 ms | 69.5 | 64 | 15556 | 0 |
| Sparse candidate | 12.60 MiB | 12.54 MiB | 13 | 124.13 ms | 70.5 | 8 | 8 | 0 |

All 20 measured cases completed. The generic baseline-defect runner records `valid: false` and exits 1 because its old success predicate requires both variants to reproduce the 64-batch defect; the sparse candidate intentionally does not. This Markdown is the candidate interpretation of the unchanged raw records and `summary.json`. No graphical process or user window participated.
