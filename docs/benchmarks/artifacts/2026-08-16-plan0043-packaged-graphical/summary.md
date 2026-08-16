# Plan 0043 guarded graphical acceptance

**Execution valid; acceptance failed.** The exact Plan 0043 candidate completed the smoke and all randomized Splinterm cases with correct isolation and cleanup, but materially regressed daemon and aggregate retained memory.

Seed: `4304`
Warmups: 2 per variant
Measured samples: 10 per variant
Workload: 5,000 mixed plain/ANSI/Unicode lines, clear every 500 lines, 2-second settle

| Variant | Application RSS growth | PSS growth | Private-anon growth | Daemon RSS growth | Client RSS growth | Marker latency | CPU ticks |
|---|---:|---:|---:|---:|---:|---:|---:|
| Packaged Alpha3.3 | 28.66 MiB | 12.11 MiB | 10.87 MiB | 6.70 MiB | 21.95 MiB | 373.66 ms | 22.5 |
| Integrated Plan 0042 | 28.76 MiB | 12.36 MiB | 11.14 MiB | 6.75 MiB | 21.99 MiB | 369.12 ms | 21.5 |
| Plan 0043 candidate | 36.27 MiB | 19.69 MiB | 18.43 MiB | 14.57 MiB | 21.68 MiB | 379.63 ms | 23 |

Candidate application RSS growth was **26.55% worse** than Alpha3.3 and **26.09% worse** than Plan 0042. Client retention slightly improved, while daemon RSS/PSS/private-anonymous growth increased to 14.57 MiB from about 6.7 MiB.

Foot, Kitty, and Ghostty comparators were not run because the approved sequence gated them on Splinterm passing aggregate retention.

Every test window was confined to workspace 8 / DP-2 without initial focus. Workspace 8 is empty after cleanup, the original Foot window remains focused on workspace 1 / DP-1, unrelated client addresses are unchanged, isolated daemons and children exited, and Pacman reports 56 files with 0 alterations.
