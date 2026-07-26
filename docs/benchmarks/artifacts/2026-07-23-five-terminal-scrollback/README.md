# Five-terminal scrollback policy benchmark evidence

This directory preserves the guarded disabled-versus-large scrollback matrix
recorded on 2026-07-23. Each case emits 5,000 plain rows with either zero
history lines or a 100,000-line history budget.

- Warmup blocks: 3
- Measured blocks: 10
- Measured cases: 100
- Randomization seed: `20260727`
- Result: all 130 raw cases valid with cleanup verified

The comparison isolates configured history policy, not terminal-internal data
structure equivalence. Visible timing remains a screenshot-polling
approximation. The manifest records exact binaries and the dirty worktree.
