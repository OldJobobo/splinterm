# Splinterbench guarded graphical multiplexer smoke

This is topology, isolation, and cleanup evidence—not a performance ranking.
Every window was silently mapped to inactive workspace 8 on DP-2.

| Stack | Topology | Pane geometry | Host state | Cleanup | Result |
|---|---|---|---|---|---|
| splinterm-native | two columns | pane-0=57×28, pane-1=57×28 | preserved | verified | PASS |
| foot-tmux | two columns | pane-0=58×28, pane-1=58×28 | preserved | verified | PASS |
| foot-zellij | two columns | pane-0=59×28, pane-1=58×28 | preserved | verified | PASS |

The Splinterm-native case gated both Foot peer cases. A placement, focus,
pointer, topology, process-incarnation, or cleanup violation stops the sequence.
