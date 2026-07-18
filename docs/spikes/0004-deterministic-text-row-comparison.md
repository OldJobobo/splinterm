# Spike 0004: deterministic text-row comparison

- **Status:** Evidence captured; renderer decision remains open
- **Date:** 2026-07-17
- **Plan:** [Omarchy-native terminal MVP](../plans/0002-omarchy-terminal-mvp.md)
- **Artifacts:**
  - `crates/splinterm/src/renderer.rs`
  - `crates/splinterm/src/wayland.rs`
  - `tools/run-wayland-window-demo.py`
  - `tools/foot-oracle/run-text-row-reference.py`
  - `docs/spikes/artifacts/0004/`
    (`renderer-benchmark.json`, `fcft-mask-probe.jsonl`, and
    `raster-evidence-comparison.json`)

## Question

Can Swash place and raster a fixed terminal corpus deterministically in the
native SHM window, and how does that first result differ from pinned Foot/fcft?

Foot 1.27.0 at commit
`3c5b584b0eafa772eb4376fb6eaf6643399e190e` remains authoritative.

## Fixed reference

The compared corpus is:

```text
ASCII ┌─┼─┐  é 界 🙂
```

Both captures use JetBrains Mono Nerd Font at 22 pixels, with explicit Noto Sans
CJK JP and Noto Color Emoji fallbacks. The reference font files on the capture
host were:

```text
0ec29a68b539ece7078fc714cebff0c0accb2f4948f8f7963d9f5e86633b12d9  JetBrainsMonoNerdFont-Regular.ttf
b76b0433203017ca80401b2ee0dd69350349871c4b19d504c34dbdd80541690a  NotoSansCJK-Regular.ttc
72a635cb3d2f3524c51620cdde406b217204e8a6a06c6a096ff8ed4b5fd6e27b  NotoColorEmoji.ttf
```

All graphical launches used the workspace-safe launchers. They refused an
occupied target and routed windows through Hyprland's `hl.exec_cmd` workspace
rule to workspace 8 on DP-2:

```bash
python tools/run-wayland-window-demo.py --workspace 8 \
  --capture /tmp/splinterm-row.ppm
python tools/foot-oracle/run-text-row-reference.py --workspace 8
```

The pinned unmodified Foot build, patched semantic oracle build, Foot tests,
and all five semantic fixtures also passed before capture.

## Splinterm mechanism

The candidate renderer now:

- resolves three explicit fontconfig patterns before connecting to Wayland;
- loads only the selected font files rather than scanning all system fonts;
- derives a 14×30 pixel cell and 23 pixel baseline from the primary face;
- shapes the combining sequence with Swash;
- centers the shaped pen advance in the assigned terminal cell before applying
  glyph bearings and shaping offsets;
- caches each rasterized mask or color image;
- blends masks and color emoji into opaque ARGB8888 SHM memory;
- writes an optional lossless pre-compositor PPM capture; and
- avoids continuous animated repaint.

The Wayland candidate keeps configure, SHM reuse, frame callback, output,
integer scale notification, seat, keyboard, resize, and clean-close ownership
inside the graphical client. It now propagates setup and draw failures through
a `Result` boundary and tracks the keyboard's owning seat.

## Captures

| Renderer | Reviewed crop | SHA-256 |
| --- | --- | --- |
| Splinterm/Swash | `artifacts/0004/splinterm-row.png` | `5363a6d79899557b01055ef4c4aef605a2cab6c4765c4792b00b683a3589d3dd` |
| Foot/fcft | `artifacts/0004/foot-row.png` | `9542de911a9fe0e1cfe4c746c19c9e7e1e8f9f5c316506c53e75d1d35caa8eed` |

The Splinterm artifact is cropped from its raw pre-submit SHM capture. The Foot
artifact is cropped from the compositor-presented window. Their hashes are
provenance markers, not an equality assertion; the capture stages differ.

## Findings

1. ASCII, the Nerd Font private-use glyph, the shaped combining sequence, CJK,
   and color emoji all render in their assigned cells.
2. Shaping fixes the visibly detached acute accent from the scalar-by-scalar
   prototype.
3. The primary advance is 13.2 pixels and is placed in a 14 pixel cell. CJK and
   emoji occupy two cells.
4. The four box characters now use a narrow safe-Rust translation of Foot's
   centered line geometry instead of font glyphs. Odd/even cell tests assert
   continuous joins, and the refreshed crop visually matches Foot's one-pixel
   geometry at 1×.
5. The direct fcft mask probe and Swash evidence agree exactly for ASCII,
   combining, and CJK placement/image/ink bounds. Emoji placement and image
   dimensions agree; Swash includes a one-pixel transparent-edge fringe at the
   left and top compared with fcft. The evidence tolerance is one pixel for
   color emoji and zero pixels for the grayscale cases.
6. Explicit Regular, Bold, Italic, and Bold Italic JetBrains faces resolve to
   distinct files and preserve the 13.2-pixel `M` advance within 0.01 pixels.
7. Integer scaling now uses `wl_surface.set_buffer_scale`, checked physical SHM
   dimensions/stride, scale-specific font metrics and glyph caches, and buffer-
   coordinate damage. Pure tests cover 1×, 2×, zero scale, and overflow.
   Fractional scaling remains unimplemented.
8. A production CLI window was deliberately not exposed. The mechanism remains
   reachable through the example and workspace-safe demo script until the ADR
   gates pass.

## Release timing evidence

A 100-sample optimized run:

```bash
cargo run --release -p splinterm \
  --example renderer-evidence-benchmark -- 100
```

recorded:

| Phase | Median | p95 |
| --- | ---: | ---: |
| complete setup: discovery, reads, shaping, cold raster | 90.935 ms single observation | — |
| warm glyph-cache lookup | 0.220 µs | 0.340 µs |
| full 960×600 canvas blend | 65.725 µs | 74.301 µs |
| four custom box masks | 0.311 µs | 0.461 µs |

The setup figure includes six `fc-match` calls and font file reads. It is one
cold-path aggregate observation, not a median or per-glyph latency. The three
warm phases contain 100 samples with min/median/p95/max values. The committed
`artifacts/0004/renderer-benchmark.json` preserves the emitted report.

## Same-pipeline raster comparison

`tools/foot-oracle/run-fcft-mask-probe.sh` builds a test-only C probe against
the fcft 3.3.3 static library from the pinned Foot build. It emits placement,
image dimensions, advances, and half-open nonzero-alpha bounds without a
compositor. `raster-evidence-comparison.json` records the corresponding Swash
values.

| Case | Placement/image | Ink bounds | Advance |
| --- | --- | --- | --- |
| ASCII `A` | exact | exact | fcft 13, Swash 13.2 |
| composed `é` | exact | exact | fcft 13, Swash 13.2 |
| CJK `界` | exact | exact | 22 |
| emoji `🙂` | exact | 1 px left/top fringe | fcft 27, Swash 27.393 |

The fractional Swash advances are centered inside fixed integer terminal spans;
they do not change logical cell placement.

## Decision

Do not accept the permanent font/renderer ADR yet and do not attach terminal
snapshots. The comparison proves the mechanism and identifies concrete parity
work, but it does not yet justify claiming Foot-compatible placement.

The extracted modules remain candidate client-owned implementation boundaries.
They do not affect `splinterd`, `splinterm-terminal`, protocol DTOs, or daemon
shell lifetime.

## Next work

1. Exercise the integer-scale path on a real 2× output or nested compositor;
   current DP-2 evidence is 1×.
2. Evaluate fractional scale and viewport support.
3. Accept the Wayland/event-loop ADR and font/renderer ADR only after those
   results are reviewed.
4. Expose the graphical production command only after ADR acceptance; terminal
   snapshot attachment remains later work.
