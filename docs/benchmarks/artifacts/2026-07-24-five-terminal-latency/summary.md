# Splinterbench targeted-input latency matrix

Measured samples per terminal: 10
Randomization seed: 20260729

Input is delivered without focus through Hyprland's targeted shortcut dispatcher.
Input-to-child ends at the child's monotonic receipt record. Input-to-visible ends
at screenshot polling detection and is not compositor presentation or input-to-photon.

| Terminal | Input → child median | Input → visible median |
|---|---:|---:|
| Splinterm | 13.98 ms | 184.88 ms |
| Foot | 8.77 ms | 179.53 ms |
| Kitty | 8.87 ms | 187.55 ms |
| Ghostty | 8.86 ms | 180.04 ms |
| Alacritty | 8.52 ms | 189.21 ms |

Raw randomized samples are retained beside this report.
