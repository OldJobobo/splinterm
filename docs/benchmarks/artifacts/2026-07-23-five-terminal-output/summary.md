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
| Splinterm | 1276.6 ms | 1277.3 ms | 1890.2 ms | 17.1 MiB | 156.5 |
| Foot | 2.0 ms | 2.5 ms | 154.8 ms | 2.6 MiB | 0 |
| Kitty | 1.4 ms | 2.8 ms | 242.4 ms | 7.2 MiB | 1 |
| Ghostty | 4.0 ms | 4.5 ms | 383.4 ms | 3.8 MiB | 2 |
| Alacritty | 3.7 ms | 5.0 ms | 258.7 ms | 2.8 MiB | 1 |

## Ansi

| Terminal | Child write | Trigger→write | Trigger→visible | RSS growth | CPU ticks |
|---|---:|---:|---:|---:|---:|
| Splinterm | 1319.5 ms | 1321.5 ms | 1865.8 ms | 21.7 MiB | 158.5 |
| Foot | 2.1 ms | 3.2 ms | 157.1 ms | 2.8 MiB | 1 |
| Kitty | 1.5 ms | 2.6 ms | 246.8 ms | 7.0 MiB | 0.5 |
| Ghostty | 5.8 ms | 7.1 ms | 488.1 ms | 6.1 MiB | 3 |
| Alacritty | 4.1 ms | 5.4 ms | 251.6 ms | 2.8 MiB | 1 |

## Unicode

| Terminal | Child write | Trigger→write | Trigger→visible | RSS growth | CPU ticks |
|---|---:|---:|---:|---:|---:|
| Splinterm | 1379.5 ms | 1380.5 ms | 1880.1 ms | 41.1 MiB | 159.5 |
| Foot | 5.2 ms | 6.3 ms | 162.6 ms | 20.8 MiB | 1 |
| Kitty | 2.1 ms | 2.9 ms | 497.5 ms | 28.1 MiB | 2.5 |
| Ghostty | 4.0 ms | 4.9 ms | 265.5 ms | 19.5 MiB | 2 |
| Alacritty | 4.5 ms | 5.4 ms | 512.7 ms | 24.7 MiB | 3 |

Raw records and randomized execution order are retained beside this report.
This is development evidence from one dirty-worktree host, not a universal ranking.
