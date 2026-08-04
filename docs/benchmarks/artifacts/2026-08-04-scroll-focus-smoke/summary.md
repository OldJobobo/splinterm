# Splinterm scroll/focus smoke

Guarded candidate validation on inactive workspace 8 / DP-2.

- The inactive left-pane live baseline and detached history capture differed.
- Returning to live and immediately moving focus right reproduced the pending-frame focus-swap boundary.
- The inactive left pane then matched its live baseline byte-for-byte.
- Active workspace, focused window, and pointer were preserved.
- The exact window, daemon, workloads, socket namespace, and process forest were removed.

This is bounded regression evidence for inactive-pane viewport reconciliation, not a performance result.
