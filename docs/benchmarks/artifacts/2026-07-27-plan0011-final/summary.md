# Plan 0011 final closure attempt

**Decision: closure incomplete; do not tag `beta1`.**

Corrected five-cycle/120-second non-graphical retained RSS growth: **15.52 MiB**.
Private-anonymous growth: **11.32 MiB**.

The prior 26.37 MiB repeated result was contaminated by the probe polling full 1,000-row snapshots. Visible-only marker polling reduced the exact result to 15.52 MiB with no overflow, meeting the 17 MiB stretch target.

## Slice 4 diagnostic

`MALLOC_ARENA_MAX=1` measured 17.14 MiB RSS and 12.88 MiB private-anonymous growth, worse than the corrected default result, so it does not prove arena causality. Heaptrack reported about 9.40 MiB peak tracked heap, only about 25.73 KiB leaked, and heavy temporary allocation in snapshot/row construction. Allocator high-water remains plausible, but no retained live snapshot ownership or justification for allocator-specific product behavior/manual trim was found.

## Graphical evidence

The exact candidate smoke passed workspace 8 / DP-2 placement, no-focus, identity, and cleanup checks. The randomized clean-HEAD control/candidate batch later aborted on a workspace-8 cardinality violation. Cleanup was verified and workspace 8 is empty. The batch is invalid and was not retried; Foot/Kitty/Ghostty comparisons were not run.

Valid partial records are retained for diagnosis only:
- control: no valid records
- candidate: no valid records

Exact source patch, untracked source bundle, toolchain, binary hashes, raw records, allocator diagnostics, and cleanup evidence are retained beside this report.
