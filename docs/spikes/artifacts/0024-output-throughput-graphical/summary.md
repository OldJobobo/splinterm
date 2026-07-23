# Splinterbench graphical output matrix

Measured samples per terminal/workload: 10
Lines per workload: 2000
Randomization seed: 20260724

Child-write timing and screenshot polling are distinct boundaries. The visible
marker is an approximation based on detecting a final uncommon truecolor row in guarded
window screenshots; it is not a compositor presentation timestamp.

## Plain

| Terminal | Child write | Trigger→write | Trigger→visible | RSS growth | CPU ticks |
|---|---:|---:|---:|---:|---:|
| Splinterm | 40.2 ms | 41.0 ms | 532.5 ms | 14.7 MiB | 10 |
| Foot | 1.9 ms | 3.1 ms | 156.7 ms | 2.6 MiB | 1 |
| Kitty | 1.5 ms | 3.1 ms | 255.0 ms | 7.2 MiB | 0 |
| Ghostty | 3.3 ms | 4.3 ms | 261.5 ms | 3.8 MiB | 2.5 |
| Alacritty | 3.6 ms | 4.6 ms | 250.9 ms | 2.8 MiB | 1 |

## Ansi

| Terminal | Child write | Trigger→write | Trigger→visible | RSS growth | CPU ticks |
|---|---:|---:|---:|---:|---:|
| Splinterm | 41.4 ms | 42.2 ms | 511.2 ms | 18.9 MiB | 10 |
| Foot | 2.4 ms | 3.9 ms | 155.6 ms | 2.8 MiB | 0 |
| Kitty | 1.6 ms | 2.9 ms | 251.5 ms | 7.0 MiB | 0.5 |
| Ghostty | 5.2 ms | 6.4 ms | 474.3 ms | 6.1 MiB | 2 |
| Alacritty | 3.9 ms | 5.1 ms | 252.0 ms | 2.8 MiB | 0.5 |

## Unicode

| Terminal | Child write | Trigger→write | Trigger→visible | RSS growth | CPU ticks |
|---|---:|---:|---:|---:|---:|
| Splinterm | 44.8 ms | 45.9 ms | 529.1 ms | 34.3 MiB | 11 |
| Foot | 4.3 ms | 5.4 ms | 153.9 ms | 16.9 MiB | 1 |
| Kitty | 2.0 ms | 2.5 ms | 484.2 ms | 24.1 MiB | 2 |
| Ghostty | 4.1 ms | 5.5 ms | 255.6 ms | 15.6 MiB | 2 |
| Alacritty | 4.5 ms | 5.8 ms | 507.3 ms | 19.9 MiB | 3 |

Raw records and randomized execution order are retained beside this report.
This is development evidence from one dirty-worktree host, not a universal ranking.
