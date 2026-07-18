# Spike 0002: CPU/SHM paint baseline

- **Status:** Initial baseline recorded
- **Date:** 2026-07-17
- **Plan:** [Omarchy-native terminal MVP](../plans/0002-omarchy-terminal-mvp.md)
- **Artifact:** `crates/splinterm/examples/cpu-shm-benchmark.rs`

## Purpose

Measure a deterministic full-canvas CPU paint loop at representative terminal
grid sizes before text rasterization and damage tracking are added. The native
Wayland spike uses SCTK's SHM slot pool; this benchmark isolates canvas
allocation/fill cost so later glyph and compositor costs can be measured
separately.

## Reference system

- CPU: AMD Ryzen 5 5600G, 6 cores / 12 threads
- Hyprland: 0.55.4
- Kernel: Linux 7.1.3-arch2-1
- Rust: 1.91.0
- Build: Cargo `--release`
- Assumed baseline cell: 10×20 pixels
- Pixel format model: four-byte ARGB8888 canvas

## Command

```bash
cargo run --release -p splinterm --example cpu-shm-benchmark
```

## Result

| Grid | Pixel surface | Canvas bytes | Iterations | Allocate + paint | Reuse + paint |
| --- | ---: | ---: | ---: | ---: | ---: |
| 80×24 | 800×480 | 1,536,000 | 100 | 1.561 ms | 1.519 ms |
| 120×40 | 1200×800 | 3,840,000 | 50 | 3.878 ms | 3.818 ms |
| 240×80 | 2400×1600 | 15,360,000 | 20 | 16.628 ms | 15.044 ms |

These are single-run mechanism numbers, not release performance claims.
Frequency scaling, system load, compiler changes, and the intentionally naive
per-pixel pattern affect them.

## Interpretation

- Buffer allocation adds little relative to touching every pixel at these
  sizes, but production still needs bounded SHM buffer reuse because compositor
  release is asynchronous.
- A naive full repaint of the large 240×80 surface is already near a 60 Hz
  frame budget before glyph rasterization, blending, protocol dispatch, or
  compositor work.
- Damage-driven row/cursor updates are therefore an architectural requirement,
  not a late optimization.
- The 80×24 and 120×40 full repaint costs leave useful room for the first text
  renderer, but must be remeasured after glyph blending.

## Next measurements

1. Instrument actual SHM slot allocation, reuse, and release in the Wayland
   spike.
2. Record resize-churn allocation high-water marks.
3. Add solid-row, changed-row, and cursor-only damage benchmarks.
4. Add cold/warm glyph raster and blend costs after the font bake-off.
5. Repeat at 1×, 1.25×, 1.5×, and 2× effective scales.
6. Run multiple samples and report median/p95 with the CPU governor recorded.
