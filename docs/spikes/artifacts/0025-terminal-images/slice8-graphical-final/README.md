# Phase 5 Slice 8 final guarded graphical matrix

All four final-source cases pass:

- `kitty-single-scaled`: direct RGBA scaled to 16×8 cells;
- `sixel-single-scaled`: bounded 10×12 Sixel raster;
- `kitty-horizontal-panes`: red and green images clipped to separate panes;
- `kitty-vertical-panes`: red and green images clipped to separate panes.

Every report records workspace 8 on DP-2, pre-map no-focus placement, the
workspace/window never becoming active, preserved placement, and verified
cleanup. Every case uses client SHA-256
`e96280902c8b36ffce63acf7582ad68d3ac9e8b2c5f3055947983ea7a25feea2`
and daemon SHA-256
`4abbf0075aefb04359140f9ebbab8ae600340c22323182b10cadb8d15f9da262`.

Reports include decoder and compositor boundary samples, daemon/client
RSS/PSS/SHM mappings, canonical and resident content bytes, static frame-pacing
applicability, and two-second post-capture idle counters. These timings are
bounded probe samples for the tiny one-batch fixtures, not general streamed
end-to-end protocol latency claims. Animation remains deferred with Slice 7.
