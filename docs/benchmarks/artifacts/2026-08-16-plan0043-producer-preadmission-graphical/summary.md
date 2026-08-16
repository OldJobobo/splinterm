# Plan 0043 producer pre-admission graphical rerun

Exact candidate commit `9a8d35557208dc13ae8ed0ff60f3dbac24db4f2a` completed one guarded smoke and the seed-4304 randomized three-variant matrix with two warmups and ten measured cases per variant. Every smoke and matrix record is valid and cleanup-verified.

| Variant | Application RSS | PSS | Private-anon | Daemon RSS | Client RSS | Marker latency | CPU ticks |
|---|---:|---:|---:|---:|---:|---:|---:|
| Alpha3.3 | 32.14 MiB | 15.57 MiB | 10.66 MiB | 6.78 MiB | 25.35 MiB | 368.43 ms | 22.0 |
| Plan 0042 | 32.39 MiB | 15.85 MiB | 10.94 MiB | 6.73 MiB | 25.65 MiB | 367.33 ms | 22.0 |
| Plan 0043 | 31.98 MiB | 15.40 MiB | 10.48 MiB | 6.54 MiB | 25.40 MiB | 366.26 ms | 22.0 |

Candidate application RSS is 0.52% below packaged Alpha3.3 and 1.27% below integrated Plan 0042. Candidate marker latency and CPU are also no worse than either control. All 18 daemon/client/aggregate RSS, PSS, and private-anonymous comparisons pass the approved `max(3%, 1 MiB)` tolerance against both controls; all four CPU/marker comparisons pass the 3% responsiveness limit. `acceptance-evaluation.json` records the exact arithmetic.

All windows were isolated to workspace 8 / DP-2 without initial focus. Each case removed its owned processes and window. The active Splinterm window recorded immediately before smoke/matrix remained focused on workspace 1 / DP-1, workspace 8 was empty after both phases, and Pacman reported 56 package files with 0 alterations. No packaged binary was replaced.

The raw records retain process timelines, exact executable identities, marker and CPU results, isolation state, and cleanup verification. `binary-identities.sha256` separately records packaged, Plan 0042, and candidate binary hashes.
