# Splinterbench graphical resize matrix

Measured samples per terminal: 10  
Randomization seed: 20260725

Each sample alternates 800×500 and 1200×700 six times and verifies every
settled geometry before continuing.

| Terminal | 12 resizes settled | Dispatch time | RSS growth | CPU ticks |
|---|---:|---:|---:|---:|
| Splinterm | 254.8 ms | 50.4 ms | 3.4 MiB | 2 |
| Foot | 259.2 ms | 51.0 ms | 1.1 MiB | 4 |
| Kitty | 256.9 ms | 52.0 ms | 0.0 MiB | 1 |
| Ghostty | 256.0 ms | 50.9 ms | 1.2 MiB | 2 |
| Alacritty | 248.4 ms | 50.1 ms | 1.5 MiB | 2 |

Raw records and execution order are retained beside this report.
