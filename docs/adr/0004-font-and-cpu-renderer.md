# ADR 0004: Use narrow fontconfig discovery with Swash and CPU SHM rendering

- **Status:** Accepted
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

Pinned Foot/fcft comparisons cover ASCII, box drawing, a Nerd Font glyph, a
combining sequence, CJK, and color emoji. A direct fcft mask probe found exact
placement, image, and ink bounds for grayscale cases. Color emoji differs by at
most one transparent-edge pixel. Foot's custom box geometry was translated for
the supported initial subset.

## Decision

Use a client-owned CPU renderer backed by:

- narrow fontconfig-compatible face resolution before Wayland dispatch;
- owned selected font bytes and face indices;
- Swash 0.2 for metrics, shaping, outline/bitmap rasterization, and color emoji;
- explicit primary, CJK, and emoji fallback ordering;
- fixed terminal cell spans independent of fractional glyph advances;
- scale-specific glyph-image caches;
- Foot-derived custom geometry for supported box-drawing characters; and
- direct blending into opaque ARGB8888 SHM buffers.

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

## Consequences

- The renderer remains disposable derived state in the graphical client.
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
[Spike 0004](../spikes/0004-deterministic-text-row-comparison.md) records visual,
direct fcft-mask, style, ink-bound, and release timing evidence. Unit tests cover
deterministic blending, clipping, shaped placement, mask/color ink bounds,
Foot-derived box geometry, cache reuse, and capture encoding.
