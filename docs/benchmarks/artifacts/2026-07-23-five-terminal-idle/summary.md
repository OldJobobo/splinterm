# Splinterbench graphical idle matrix

Measured samples per terminal: 10  
Randomization seed: 20260723

Startup boundaries are observed independently. Splinterm uses a prestarted daemon;
the other terminals use standalone process launches. Values are medians of
child-inclusive process-forest measurements and are not input-to-photon latency.

| Terminal | Child ready | Window mapped | Idle RSS | CPU ticks | Context switches |
|---|---:|---:|---:|---:|---:|
| Splinterm | 73.1 ms | 119.9 ms | 47.0 MiB | 0 | 0 |
| Foot | 72.2 ms | 52.7 ms | 49.1 MiB | 0 | 1.5 |
| Kitty | 239.4 ms | 130.0 ms | 414.2 MiB | 0 | 2 |
| Ghostty | 310.1 ms | 225.8 ms | 371.5 MiB | 0 | 40.5 |
| Alacritty | 145.6 ms | 110.8 ms | 292.5 MiB | 0 | 2 |

Raw samples and execution order are retained beside this report. Do not treat
this development matrix as a publishable cross-host conclusion.
