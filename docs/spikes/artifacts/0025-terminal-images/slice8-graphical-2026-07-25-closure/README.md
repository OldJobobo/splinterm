# Phase 5 Slice 8 guarded graphical matrix

Final-source guarded evidence for the practical static-image milestone.

All four cases pass:

- `kitty-single-scaled`: direct RGBA scaled to 16×8 cells;
- `sixel-single-scaled`: bounded 10×12 Sixel raster;
- `kitty-horizontal-panes`: red and green images clipped to separate panes;
- `kitty-vertical-panes`: red and green images clipped to separate panes.

Every report records workspace 8 on DP-2, pre-map no-focus placement, the
workspace/window never becoming active, preserved placement, and verified
cleanup. Every case uses client SHA-256
`81cebf0fe50a55b5b3542c9d8d66a069208f43cac96d67ce19a9ee1041374f00`
and daemon SHA-256
`4abbf0075aefb04359140f9ebbab8ae600340c22323182b10cadb8d15f9da262`.

Reports include decoder and compositor boundary timings, daemon/client
RSS/PSS/SHM mappings, canonical and resident content bytes, static frame-pacing
applicability, and two-second post-capture idle counters. Animation remains
explicitly deferred with optional Slice 7.
