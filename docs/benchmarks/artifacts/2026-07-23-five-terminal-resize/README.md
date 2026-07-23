# Five-terminal graphical resize benchmark evidence

This directory preserves the guarded development resize matrix recorded on
2026-07-23. Each case alternates an isolated floating terminal between 800×500
and 1200×700 six times, verifies all twelve settled geometries, samples the
child-inclusive process forest, and verifies cleanup on workspace 8 / DP-2.

- Warmup blocks: 3
- Measured blocks: 10
- Measured cases: 50
- Randomization seed: `20260725`
- Result: every case valid; no focus or cleanup violation

`manifest.json` records the dirty-worktree host and exact terminal binary
hashes. `matrix.json` keeps full statistics and execution order, `summary.md`
contains median results, and `raw/` contains all 65 records. These are
single-host development measurements, not a universal ranking.
