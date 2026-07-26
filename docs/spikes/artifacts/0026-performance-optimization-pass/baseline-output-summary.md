# Splinterbench graphical output matrix

Measured samples per terminal/workload: 10  
Lines per workload: 2000  
Randomization seed: 10010

Child-write timing and screenshot polling are distinct boundaries. The visible
marker is an approximation based on detecting a final uncommon truecolor row in guarded
window screenshots; it is not a compositor presentation timestamp.

## Plain

| Terminal | Child write | Trigger→write | Trigger→visible | RSS growth | CPU ticks |
|---|---:|---:|---:|---:|---:|
| Splinterm | 60.1 ms | 61.1 ms | 367.8 ms | 43.0 MiB | 13.5 |
| Foot | 2.0 ms | 2.9 ms | 181.8 ms | 2.6 MiB | 1 |
| Kitty | 1.4 ms | 2.6 ms | 179.1 ms | 7.2 MiB | 1 |
| Ghostty | 3.3 ms | 4.1 ms | 359.9 ms | 3.7 MiB | 2 |
| Alacritty | 3.7 ms | 4.9 ms | 203.9 ms | 2.8 MiB | 0.5 |

## Ansi

| Terminal | Child write | Trigger→write | Trigger→visible | RSS growth | CPU ticks |
|---|---:|---:|---:|---:|---:|
| Splinterm | 62.0 ms | 63.3 ms | 362.7 ms | 47.3 MiB | 14 |
| Foot | 2.2 ms | 3.2 ms | 174.7 ms | 2.8 MiB | 1 |
| Kitty | 1.7 ms | 2.7 ms | 178.8 ms | 7.0 MiB | 1 |
| Ghostty | 5.5 ms | 6.5 ms | 193.4 ms | 5.0 MiB | 2 |
| Alacritty | 4.1 ms | 5.4 ms | 345.7 ms | 2.8 MiB | 0 |

## Unicode

| Terminal | Child write | Trigger→write | Trigger→visible | RSS growth | CPU ticks |
|---|---:|---:|---:|---:|---:|
| Splinterm | 62.1 ms | 63.5 ms | 360.4 ms | 64.1 MiB | 16 |
| Foot | 4.5 ms | 6.1 ms | 184.3 ms | 18.8 MiB | 1.5 |
| Kitty | 2.0 ms | 3.3 ms | 282.8 ms | 26.2 MiB | 2 |
| Ghostty | 3.6 ms | 5.1 ms | 181.9 ms | 17.4 MiB | 2.5 |
| Alacritty | 4.5 ms | 5.9 ms | 360.2 ms | 22.8 MiB | 2 |

Raw records and randomized execution order are retained beside this report.
This is development evidence from one dirty-worktree host, not a universal ranking.
