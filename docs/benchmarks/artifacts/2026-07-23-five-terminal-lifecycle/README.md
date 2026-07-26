# Five-terminal process-exit lifecycle evidence

This directory preserves the guarded no-hold lifecycle matrix recorded on
2026-07-23. The benchmark child exits after 250 ms. Each case records child
exit, window unmap or intentional persistence, immediate residual process-tree
count, focus isolation, and cleanup.

- Warmup blocks: 3
- Measured blocks: 10
- Measured cases: 50
- Randomization seed: `20260728`
- Result: all 65 cases valid with cleanup verified

Splinterm intentionally retains an exited Splint; this is lifecycle semantics,
not a failure. Residual process counts are immediate observations rather than a
long post-exit grace-period measurement. The manifest records exact binaries
and the dirty worktree.
