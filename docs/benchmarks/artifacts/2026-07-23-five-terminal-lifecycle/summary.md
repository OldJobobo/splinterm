# Splinterbench process-exit lifecycle matrix

Measured samples per terminal: 10  
Randomization seed: 20260728

No terminal hold option is enabled. Persisted windows are recorded as lifecycle semantics, not failures.

| Terminal | Child exit | Window unmap | Persisted | Residual processes |
|---|---:|---:|---:|---:|
| Splinterm | 204.7 ms | n/a | 10/10 | 2 |
| Foot | 252.9 ms | 251.8 ms | 0/10 | 0 |
| Kitty | 252.4 ms | 260.1 ms | 0/10 | 3 |
| Ghostty | 252.3 ms | 263.7 ms | 0/10 | 1 |
| Alacritty | 250.7 ms | 249.9 ms | 0/10 | 0 |
