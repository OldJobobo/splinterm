# Plan 0043 reusable direct-tail candidate

**Accepted headless correction; graphical rerun remains required.**

The mailbox now owns one reusable sparse row/history tail for its entire admitted
sequence. Successor captures own metadata, damage indices, history ranges, and
bounded ordered update summaries, but no duplicate row/history bodies. Row, cell,
composed-string, and bounded-history capacities are reused explicitly. Producer
count leases and semantic-byte leases remain exact; ordered summaries coalesce once
at receiver materialization into one wire update.

| Variant | RSS/PSS growth | Private-anon | CPU ticks | Marker latency | Events | Batch HWM | Ordered-summary HWM | Resync |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Integrated Plan 0042 | 8.40 MiB | 8.27 MiB | 14.0 | 125.29 ms | 69.0 | 64.0 | 15553.5 | 0 |
| Reusable direct tail | 6.05 MiB | 5.95 MiB | 13.0 | 119.79 ms | 67.5 | 64.0 | 64.0 | 0 |

Candidate aggregate retained growth is 28.0% below integrated Plan 0042.
CPU and marker latency also improve. All 20 measured cases completed with zero
resync. The generic defect runner's `valid: false` and exit 1 are expected because
its historical predicate requires both variants to retain the old defect.

Rejected intermediate shapes:

- Direct merge with the old 8-frame/4 MiB chunk boundary retained a 7.86 MiB heap
peak; fresh chunk construction still owned 5.87 MiB at peak.
- One exact accumulated update reduced memory further but copied prior ordered work
on every successor, regressing median marker latency to 152.38 ms versus 135.31 ms.

Final validation passed ten consecutive production-socket exact reconstruction
runs, the complete serialized workspace/all-target suite, warnings-denied workspace
Clippy, 63 benchmark tests, formatting, and `git diff --check`. The source identity
is base `d825965` plus working diff `065f1d5c4b7c2d2da7a7bb5cd0e2e8cdb754c8e53742e56e7c58eee66f1de20f`. Raw randomized records and exact
binary hashes are retained here. No graphical process participated; the separately
approved graphical acceptance must be rerun only after fresh review and new approval.

Fresh read-only review `42a73b1e` returned **CLEAN** with the graphical rerun as the only residual acceptance boundary.
