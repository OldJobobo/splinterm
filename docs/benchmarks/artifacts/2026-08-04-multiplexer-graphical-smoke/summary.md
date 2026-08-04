# Splinterbench guarded graphical multiplexer smoke

This is topology, isolation, and cleanup evidence—not a performance ranking.
Every window was silently mapped to inactive workspace 8 on DP-2.

| Stack | Topology | Pane geometry | Host state | Cleanup | Result |
|---|---|---|---|---|---|
| splinterm-native | two columns | unavailable | preserved | verified | FAIL |
| foot-tmux | not run | — | — | — | SKIPPED |
| foot-zellij | not run | — | — | — | SKIPPED |

Sequence stopped: `splinterm-native guarded smoke failed: RuntimeError: Splinterm snapshot geometry is malformed`

The Splinterm-native case gated both Foot peer cases. A placement, focus,
pointer, topology, process-incarnation, or cleanup violation stops the sequence.
