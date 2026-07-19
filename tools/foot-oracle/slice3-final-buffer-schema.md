# Slice 3 final-buffer v2 contract

Slice 3 does not change `splinterm.final-buffer.v1`. Decoration/cursor evidence uses
`schema: splinterm.final-buffer.slice3.v2` and the structured fixture manifest
`schema: splinterm.final-buffer.slice3-fixtures.v2`.

The v2 capture retains v1 width, height, stride, format, scale, grid, cell,
padding, origin, fixture, frame ID, background, and provenance fields. It replaces
the ambiguous cursor/composition declarations with:

```json
{
  "schema": "splinterm.final-buffer.slice3.v2",
  "cursor": {
    "position": {"column": 1, "row": 0},
    "configured_shape": "beam",
    "effective_shape": "beam",
    "target_focus_semantics": "focused-steady"
  },
  "capture_context": {
    "actual_keyboard_focus": false,
    "unfocused_style": "unchanged"
  },
  "composition": "foot-cell-rtl-v1"
}
```

`configured_shape` is `block`, `beam`, or `underline`. `effective_shape` is one
of those values plus `hollow` or `none`. Configured and effective values must
never be inferred from each other.

Two physically unfocused oracle lanes are valid:

- `focused-steady`: Foot is launched with `cursor.unfocused-style=unchanged` and
  a steady DECSCUSR form; Splinterm uses semantic focus. The effective shape is
  the configured shape.
- `unfocused`: Foot uses its configured unfocused policy (the manifest currently
  attests default `hollow`); Splinterm uses semantic unfocus. A visible cursor's
  effective shape is `hollow` regardless of its configured shape.

The manifest carries both bounded lowercase `vt_hex` bytes for Foot and a
rectangular structured semantic cell grid for Splinterm. A two-column leader is
followed by exactly one `{ "spacer_remaining": 1 }` cell. Unknown keys,
oversized grids/steps, invalid colors/scales, inconsistent lanes, and malformed
wide cells are rejected.

Composition is per row, right-to-left by leader cell. Each cell is composed in
this exact order: background; focused block cursor; glyphs; underline; strike;
then beam, underline, hollow, or other non-block cursor. Captures compare exact
ARGB by default. No image translation or general decoration tolerance is
permitted.
