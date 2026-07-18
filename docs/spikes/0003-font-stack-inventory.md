# Spike 0003: font discovery and Swash inventory

- **Status:** Initial candidate evidence; discovery decision remains open
- **Date:** 2026-07-17
- **Plan:** [Omarchy-native terminal MVP](../plans/0002-omarchy-terminal-mvp.md)
- **Artifact:** `crates/splinterm/examples/font-stack-spike.rs`

## Question

Can `fontdb` provide acceptable system discovery and can Swash parse the
selected faces, expose terminal-relevant metrics, and identify coverage for the
initial fallback corpus?

## Candidates

- `fontdb` 0.23.0: system font inventory and CSS-like queries
- `swash` 0.2.10: OpenType metrics, shaping, scaling, and raster mechanisms

This is not yet the font-stack selection ADR.

## Reference system observations

The installed fontconfig command resolves:

```text
monospace regular → JetBrains Mono Nerd Font Regular
monospace bold    → JetBrains Mono Nerd Font Bold
emoji             → Noto Color Emoji
CJK               → Noto Sans CJK JP
```

The initial `fontdb` generic `monospace` query instead selected Nimbus Mono PS.
That difference is material: Nimbus covers ASCII and box drawing but misses the
configured Nerd Font private-use glyph, combining acute accent, CJK, and emoji.

Explicit fallback-family queries successfully selected:

- Noto Color Emoji for `🙂`; and
- Noto Sans CJK JP for `界` and the combining acute accent.

Swash parsed all selected faces and reported metrics and character-map coverage
without first-party unsafe code.

## Timing

A debug-build full `fontdb::Database::load_system_fonts()` scan loaded 3,174
faces in approximately 3.07 seconds on the reference system. This is not an
acceptable synchronous graphical startup path. Release timing and warm-cache
behavior still need measurement, but the mechanism already indicates that the
production client should not blindly scan every system face on its Wayland
thread.

## Coverage summary

| Selected face | ASCII | Box drawing | Nerd PUA | Combining | CJK | Emoji |
| --- | --- | --- | --- | --- | --- | --- |
| Nimbus Mono PS | yes | yes | no | no | no | no |
| Noto Color Emoji | no | no | no | no | no | yes |
| Noto Sans CJK JP | yes | yes | no | yes | yes | no |

## Interpretation

1. Swash remains a viable shaping/raster candidate for deeper visual testing.
2. `fontdb` generic-family resolution does not currently reproduce this
   Omarchy/fontconfig configuration and cannot be accepted as-is.
3. The primary terminal face must honor the user's actual fontconfig/Omarchy
   choice, including JetBrains Mono Nerd Font here.
4. Fallback is necessarily per-character/run and cannot be represented by one
   generic monospace face.
5. Color emoji requires a raster path and cell-placement policy beyond charmap
   coverage.
6. Discovery and font-data loading should be moved off the Wayland dispatch
   path or use a narrower fontconfig-backed lookup/cache.

## Next work

- Query the exact Omarchy-configured family instead of generic `monospace`.
- Compare fontconfig-backed narrow discovery against a full fontdb scan.
- Shape and raster the complete Plan 0002 corpus with Swash.
- Measure regular/bold/italic cell advances and reject faces that violate the
  terminal cell contract without an explicit policy.
- Render deterministic rows into the native SHM window and compare against
  pinned Foot/fcft captures.
- Evaluate color emoji and fallback ordering.
- Record release-mode cold/warm discovery and raster timings.
