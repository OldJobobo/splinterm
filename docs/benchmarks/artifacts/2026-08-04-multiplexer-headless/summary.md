# Splinterbench headless multiplexer orchestration matrix

Randomization seed: 20260804

This is non-graphical orchestration and cleanup evidence, not a performance ranking.
No terminal window was launched. Ambient sessions are represented only by counts.

| Implementation | Topology | Panes | All children ready | Cleanup | Result |
|---|---|---:|---:|---|---|
| splinterm | single | 1 | 72.9 ms | verified | PASS |
| splinterm | two-columns | 2 | 87.7 ms | verified | PASS |
| splinterm | four-grid | 4 | 124.5 ms | verified | PASS |
| tmux | single | 1 | 49.9 ms | verified | PASS |
| tmux | two-columns | 2 | 72.3 ms | verified | PASS |
| tmux | four-grid | 4 | 59.4 ms | verified | PASS |
| zellij | single | 1 | 131.1 ms | verified | PASS |
| zellij | two-columns | 2 | 140.6 ms | verified | PASS |
| zellij | four-grid | 4 | 170.8 ms | verified | PASS |

Each case used a unique socket/session namespace, exact process-incarnation
checks, explicit server/workload roles, and namespace-scoped cleanup.
