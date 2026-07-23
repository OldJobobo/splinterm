# Five-terminal graphical memory-retention evidence

This directory preserves the guarded development retention matrix recorded on
2026-07-23. Each sample emits 5,000 mixed plain, ANSI, and Unicode rows, clears
the screen every 500 lines, detects the final visible marker, and records
child-inclusive observed peak and two-second post-settle RSS.

- Warmup blocks: 3
- Measured blocks: 10
- Measured cases: 50
- Randomization seed: `20260726`
- Result: every case valid with cleanup verified on workspace 8 / DP-2

Observed peak is sampled during screenshot polling rather than obtained from a
kernel peak-memory counter. Post-settle growth is therefore the stronger memory
retention signal. The manifest records the dirty worktree and exact binary
hashes. This is single-host development evidence, not a universal ranking.
