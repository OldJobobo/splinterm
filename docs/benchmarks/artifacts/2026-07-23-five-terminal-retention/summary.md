# Splinterbench graphical memory-retention matrix

Measured samples per terminal: 10  
Mixed lines per sample: 5000  
Randomization seed: 20260726

Each workload mixes plain, ANSI, and Unicode rows, clears every 500 lines,
then records observed peak and post-settle child-inclusive RSS.

| Terminal | Trigger→visible | Peak RSS | Post-settle RSS | Retained growth | CPU ticks |
|---|---:|---:|---:|---:|---:|
| Splinterm | 2923.9 ms | 84.0 MiB | 84.0 MiB | 39.3 MiB | 251.5 |
| Foot | 165.7 ms | 67.7 MiB | 67.7 MiB | 17.1 MiB | 1.5 |
| Kitty | 504.2 ms | 440.3 MiB | 440.3 MiB | 24.1 MiB | 3 |
| Ghostty | 271.4 ms | 392.3 MiB | 392.3 MiB | 17.3 MiB | 4 |
| Alacritty | 520.4 ms | 314.0 MiB | 314.0 MiB | 20.0 MiB | 3.5 |

Raw records and execution order are retained beside this report.
