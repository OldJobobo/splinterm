# ADR 0004: Use narrow fontconfig discovery with Swash and CPU SHM rendering

- **Status:** Accepted — Phase 8.1 renderer and oracle policy closed
- **Date:** 2026-07-18

## Context

The graphical client needs deterministic terminal cell metrics, shaping,
fallback, color emoji, custom box drawing, glyph caching, damage composition,
and scale-specific rasterization without adding unsafe first-party code.

A full `fontdb` system scan selected a different generic monospace face than the
active fontconfig configuration and took roughly three seconds in a debug
build. Narrow fontconfig resolution selected the configured JetBrains Mono Nerd
Font and explicit Noto CJK/emoji fallbacks in about 50–90 ms including file
reads, shaping, and cold raster setup. Swash parsed, shaped, and rasterized the
required corpus safely.

The initial pinned Foot/fcft comparisons covered representative ASCII, box
drawing, a Nerd Font glyph, a combining sequence, CJK, and color emoji. They
established placement, image dimensions, and ink bounds, but did not compare
all grayscale alpha bytes. Phase 8.1 corrected that evidence gap: the complete
printable-ASCII comparison found only 8 of 95 Swash masks byte-identical after
cell-height correction. The earlier representative evidence therefore cannot
support an exact-raster-parity claim. Foot's custom box geometry remains a safe
translation for the supported initial subset.

## Decision

Use a client-owned CPU renderer backed by:

- narrow fontconfig-compatible face resolution before Wayland dispatch;
- owned selected font bytes and face indices;
- Swash 0.2 for shaping and the current provisional outline/bitmap raster path;
  production grayscale rasterization remains subject to the Phase 8.1 decision
  below;
- explicit primary, CJK, and emoji fallback ordering;
- fixed terminal cell spans independent of fractional glyph advances;
- scale-specific glyph-image caches;
- Foot-derived custom geometry for supported box-drawing characters; and
- direct blending into premultiplied ARGB8888 SHM buffers, opaque by default
  with Foot-compatible default-background alpha when configured.

The current discovery adapter invokes `fc-match` for explicit patterns and
validates the resolved family/style before reading only the selected files. It
must not run repeatedly on the Wayland dispatch path. Production configuration
will supply the requested primary pattern; generic `fontdb` monospace selection
is not authoritative.

Regular, Bold, Italic, and Bold Italic faces must resolve to distinct identities
and preserve the terminal advance contract. Combining sequences are shaped as
clusters. CJK and emoji receive explicit two-cell spans. Color-emoji raster ink
may differ from fcft by one transparent-edge pixel; grayscale evidence requires
exact bounds for the pinned reference fonts.

Custom box drawing is a narrow safe-Rust translation of Foot 1.27.0
`box-drawing.c` at commit
`3c5b584b0eafa772eb4376fb6eaf6643399e190e`. Provenance and supported
codepoints remain documented in the module and `THIRD_PARTY.md`.

## Phase 8.1 amendment

The pinned reference host resolves 22 px JetBrains Mono through fontconfig with
antialiasing, slight hinting, no forced autohint, and embedded bitmaps enabled.
fcft consequently loads with FreeType's light hinting target and renders through
`FT_RENDER_MODE_NORMAL`. Swash 0.2.10 instead uses Skrifa's fixed smooth
horizontal-LCD hinting configuration and Zeno's alpha scan converter. Its public
API does not expose a FreeType-light-equivalent hinting target. Equal font,
size, advance, bounds, and output format therefore do not establish equal
pixels across these engines.

The default cell geometry now rounds the complete scaled line extent once,
matching Foot's 13×29 cell and 23 px baseline at the pinned 22 px fixture. This
improved the strict result from 0/95 to 8/95 passing characters. The remaining
87 alpha mismatches must not be hidden with glyph-specific offsets, alpha
patches, best-fit translation, or a broad image-similarity tolerance.

Regular, bold, italic, and bold-italic primary faces are now selected from cell
attributes and receive distinct cache identities. Synthetic style parity and
fallback style policy remain separate gates.

The selected production candidate is the dedicated `splinterm-freetype` crate
using system FreeType through the safe `freetype-rs` wrapper. It loads the
selected file/index at 72 DPI with FreeType's light hinting target, renders
normal grayscale, normalizes pitch into bounded owned alpha data, and exposes
no native pointers. No first-party unsafe code is required. Its initial pinned
U+0020–U+007E differential passes 95/95 with exact geometry and alpha bytes.

The bridge now backs the production scale/face-keyed snapshot cache for
non-color faces; the real production-cache exporter passes the pinned 95/95
ASCII gate. Swash remains responsible for shaping. Color emoji uses the
FreeType fixed strike selected by fcft and Foot's pixman Lanczos3 scaling path.

## Phase 8.1 closure decision

The accepted observable tolerance is zero for every closure lane: final ARGB
bytes, grayscale masks, cell metrics, advances, placement, four-sided ink and
padding geometry, decoration vectors, cursor composition, renderer-path output,
and accepted source-first scale cases. Comparators may not translate images,
widen tolerances, or regenerate references during CI. The pinned authority is
Foot 1.27.0 commit `3c5b584b0eafa772eb4376fb6eaf6643399e190e`.

Strict reference generation is supported only when provenance matches the
recorded Linux x86_64 font files/indices and hashes, fontconfig/FreeType/fcft/
pixman versions and policy, Foot patch/build options, palette, geometry,
scales, and Rust lockfile recorded by schema-v3
`tools/foot-oracle/provenance.json`. Portable CI validates fixtures and tools;
a supported reference-host drift fails rather than rewriting evidence, and an
unsupported host is reported explicitly.

The production cache policy is output-independent and bounded: 2,048 persistent
glyph entries, 64 MiB of glyph data, 24 raster faces, and a 4,096-entry active
frame warm-cache target that may retain only currently referenced overflow.
The client history window is separately bounded to 4,096 rows and 16 MiB.
Scale, face identity, raster size, theme, and output-DPI changes use explicit
cache keys or invalidation; cold/warm and full/damage/scroll-copy paths must
produce identical pixels.

Release budgets are enforced by `tools/performance/phase9-thresholds.json`.
Notable limits are 10/300 ms full-paint p95 at 80×24/240×80, 1/10 ms one-row
paint p95, 128/256 MiB renderer RSS, 256 MiB graphical RSS, 128 MiB SHM, five
idle CPU ticks over two seconds, 100 ms post-output input response, two seconds
for twelve resizes, and three seconds for reattach. The accepted reference-host
measurements and host manifest are archived under
`docs/spikes/artifacts/0017/slice9-performance/`.

Intentional or deferred Foot divergences are:

- Swash shapes text while safe FreeType/fcft-compatible paths rasterize
  grayscale and color glyphs; engine identity may differ but accepted output
  lanes do not.
- Splinterm translates the supported Foot box-drawing geometry into safe Rust.
- Configuration is a documented subset. Background `alpha` supports Foot's
  default mode, and opt-in native blur uses compositor-owned
  `ext-background-effect-v1` presentation state when translucent and supported.
  This remains a staging protocol, initially validated against Hyprland 0.56.1
  or newer. Missing protocol capability falls back to ordinary transparency;
  `alpha-mode=matching/all` remains unsupported.
- `TERM=xterm-256color` is advertised until a tested project terminfo and full
  Foot keyboard contract exist; claiming `TERM=foot` broke Neovim input.
- The extra exploratory `underline-double-indexed` 1.25× case is recorded as
  non-exact outside the reviewed closure set; no tolerance/reference changed.
- Sixels, synthetic styled fallback, advanced URL/configuration behavior, and
  other declared MVP non-goals remain deferred.

The accepted evidence is indexed in
`docs/spikes/artifacts/0017/README.md`; Slice 10 additionally proved real
Hyprland/Omarchy lifecycle behavior, Fcitx5/Mozc preedit/commit, clipboard
paths, live theme integration, focus-safe history, and application-keypad text
input.

## Consequences

- The renderer remains disposable derived state in the graphical client.
- Native blur is also disposable graphical-client presentation state. It does
  not change CPU-rendered SHM pixels, terminal snapshots, or renderer oracle
  semantics; blur algorithm and cost remain compositor policy.
- The daemon, protocol, and terminal-semantic crates gain no font or graphics
  dependencies.
- CPU SHM rendering remains the baseline; a future GPU renderer must preserve
  the same cell, fallback, shaping, and damage semantics.
- Glyph caches are invalidated and rerasterized when physical scale or font
  identity changes.
- Full-window blend is fast for the current evidence row, but terminal rendering
  must consume semantic damage rather than repaint large windows continuously.
- Fontconfig process startup is acceptable for the first client slice but may be
  replaced by a cached or dedicated safe adapter without changing renderer
  contracts.
- The accepted mechanism does not imply terminal usability; snapshot, cursor,
  selection, and input rendering remain separate work.

## Validation

[Spike 0003](../spikes/0003-font-stack-inventory.md) records discovery evidence.
[Spike 0004](../spikes/0004-deterministic-text-row-comparison.md) records the
initial visual, direct fcft-mask, style, ink-bound, and release timing evidence.
[Spike 0016](../spikes/0016-phase8.1-ascii-raster-baseline.md) records the strict
95-character correction and supersedes Spike 0004 for grayscale-mask parity.
Unit tests cover
deterministic blending, clipping, shaped placement, mask/color ink bounds,
Foot-derived box geometry, cache reuse, and capture encoding.
