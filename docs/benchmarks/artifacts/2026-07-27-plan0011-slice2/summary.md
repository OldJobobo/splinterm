# Plan 0011 Slice 2 evidence

Source HEAD: `0ce4fc62ed7ade2138fd35f492075942b415a571` (uncommitted candidate)

| Case | RSS growth | PSS growth | Overflows | Queue HWM | Snapshot HWM |
|---|---:|---:|---:|---:|---:|
| fast | 34.33 MiB | 34.33 MiB | 0 | 27 | 1 |
| delayed | 4.81 MiB | 4.81 MiB | 1 | 64 | 1 |
| overflow | 2.34 MiB | 2.34 MiB | 1 | 1 | 1 |
| multiple | 23.04 MiB | 23.04 MiB | 0 | 32 | 2 |

Slice 2 gate: fast/multiple preserve delivery without resnapshot; delayed saturation remains deterministic; retained full-snapshot high water is one per compact subscriber.

Producer-batch completion is event-driven through Tokio Notify; the focused regression records one wait/wake per synchronous PTY read rather than cooperative polling.

The whole-plan `<24 MiB` gate is not claimed: successful 1,000-row materialization remains the measured Slice 3 target.

Validation: final splinterd library and full serial workspace passed. The earlier ordinary concurrent `splinterd` run reproduced the known policy timeout; its isolated run passed in 14.82 seconds.
