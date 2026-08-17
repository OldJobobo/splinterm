# Plan 0043 producer pre-admission headless validation

PR #22 follow-up review identified that producer snapshot/capture construction could transiently allocate before semantic admission. The candidate now reserves a conservative clone-free upper bound under the existing 64 MiB per-Splint and 256 MiB daemon authorities before constructing either object, verifies actual ownership against that bound, drops the ephemeral snapshot, and transfers only exact retained frame ownership into the existing subscriber lease.

A reconstruction regression initially exposed a 192-byte underestimate caused by geometric damage/event-vector growth in `TerminalUpdate::coalesce_contiguous`. Contiguous coalescing now validates continuity first and allocates exact final capacities. After rebuilding the actual daemon and PTY helper, ten consecutive production-socket mixed-clear reconstruction runs passed with zero resync.

The final seed-4304 randomized headless comparison used two warmups and twenty measured samples per variant. All 40 measured cases completed with `error: null` and zero resnapshots.

| Median | Integrated Plan 0042 | Candidate |
|---|---:|---:|
| RSS growth | 8,665,088 B | 6,154,240 B |
| PSS growth | 8,664,576 B | 6,152,192 B |
| Private-anonymous growth | 8,624,128 B | 6,100,992 B |
| CPU ticks | 14.0 | 14.0 |
| Marker latency | 125.90 ms | 128.97 ms |
| Materialized terminal-update HWM | 15,553 | 64 |
| Resnapshots | 0 | 0 |

Candidate marker latency is 2.44% above Plan 0042 and remains inside the declared 3% allowance. The runner exits 1 and records `valid: false` because its historical success predicate expects the old retained-frame defect to reproduce; `attribution_gate_reproduced: false` is the expected corrected outcome.

`summary.json` retains randomized ordering, all raw measured records, binary identities and SHA-256 hashes, workload parameters, and aggregate summaries.
