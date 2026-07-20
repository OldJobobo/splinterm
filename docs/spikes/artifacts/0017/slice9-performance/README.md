# Slice 9 performance baseline

Release evidence captured on 2026-07-20 at commit `69f1927` on an AMD Ryzen 5
5600G, Omarchy, Linux 7.1.3, Rust 1.91.0. The preserved unrelated working-tree
items make `host.git_dirty` true; the benchmark code and binaries correspond to
the recorded commit.

## Results

- `headless.json`: all enforced renderer and daemon budgets passed.
  - 80×24 full paint p95: 4.03 ms; one-row paint p95: 0.16 ms.
  - 240×80 full paint p95: 212.16 ms; one-row paint p95: 2.61 ms.
  - 12,000-line mixed daemon output: 4.35 s while PTY reads remained active.
  - Snapshot p95: 0.18 ms; page fetch p95: 0.07 ms.
  - Eight 16-row pages retain approximately 663,424 bytes.
  - Resize/reflow: 21.18 ms; post-output input response: 11.74 ms.
  - Renderer RSS: 56.3/89.5 MB; daemon RSS: 45.9 MB.
  - Subscriber overflow required resnapshot; history, command, write, glyph,
    glyph-byte, raster-face, page-byte, and RSS bounds all held.
- `graphical.json`: all guarded workspace-8/DP-2 budgets passed.
  - Steady idle: 0 CPU ticks and 0 context switches over two seconds after a
    two-second warmup; 60.0 MB RSS and 3.17 MB SHM.
  - 2,500-line mixed output: 206.94 ms; twelve targeted resizes: 206.65 ms.
  - Detach/reattach: 132.33 ms with daemon-owned process continuity.
  - Focus remained untouched, DP-2 stayed at scale 1, and workspace 8 was empty
    after cleanup.

Numeric policy lives in `tools/performance/phase9-thresholds.json`.

## Reproduce

```bash
python tools/performance/run-phase9-baseline.py /tmp/splinterm-phase9-headless --samples 10
python tools/performance/run-phase9-graphical.py /tmp/splinterm-phase9-graphical --case all
```

The graphical command refuses to launch unless workspace 8 is assigned to
DP-2, inactive, and empty; it uses silent pre-map placement and no initial
focus.
