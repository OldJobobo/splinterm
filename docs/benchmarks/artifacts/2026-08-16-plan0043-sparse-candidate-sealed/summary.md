# Plan 0043 sparse candidate attribution — Accepted experiment: true sealed sparse ownership

**Accepted headless candidate.** The candidate is about 40.6% below the paired integrated baseline in retained RSS/PSS growth and passes the daemon-retention preference.

Randomization seed: `43`
Warmups: 2
Measured samples per variant: 10

| Variant | RSS/PSS growth | Private-anon growth | CPU ticks | Marker latency | Events | Batch HWM | Update HWM | Resync |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Integrated Plan 0042 | 9.73 MiB | 9.67 MiB | 14 | 132.48 ms | 68.5 | 64 | 15553 | 0 |
| Sparse candidate | 5.78 MiB | 5.72 MiB | 13 | 121.19 ms | 69 | 8 | 1 | 0 |

All 20 measured cases completed. The generic baseline-defect runner records `valid: false` and exits 1 because its old success predicate requires both variants to reproduce the 64-batch defect; the sparse candidate intentionally does not. This Markdown is the candidate interpretation of the unchanged raw records and `summary.json`. No graphical process or user window participated.
