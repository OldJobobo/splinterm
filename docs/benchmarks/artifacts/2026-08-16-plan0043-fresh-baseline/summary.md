# Plan 0043 fresh headless baseline

**Gate reproduced.** The integrated Plan 0042 baseline still materializes many
producer batches and terminal updates into each first-party subscriber event.
Sparse-frame implementation is authorized to proceed.

Randomization seed: `43`
Warmups: 2
Measured samples per variant: 10

| Variant | RSS growth | PSS growth | Private-anon growth | CPU ticks | Marker latency | Events | Batch HWM | Update HWM | Resync |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Control | 9.22 MiB | 9.22 MiB | 9.10 MiB | 14 | 131.42 ms | 68 | 64 | 15552 | 0 |
| Baseline | 8.34 MiB | 8.34 MiB | 8.28 MiB | 14 | 127.23 ms | 70 | 64 | 15554.5 | 0 |

The workload is one 5,000-line plain/ANSI/Unicode cycle with a clear every
500 lines and a two-second settle. Both variants use the identical recorded
harness source. Raw randomized records and exact binary identities are retained
beside this summary. No graphical process or user Window participates.
