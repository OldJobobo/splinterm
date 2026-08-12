# Theme-parity review record

A fresh read-only reviewer inspected the renderer milestone after the initial
non-graphical validation. The review confirmed:

- raster fills now receive RGBA and retain the renderer's BGRA storage contract;
- selection bounds are normalized and clipped across rows and columns;
- wide glyphs and box drawing are clipped to selected cells; and
- pixel tests establish the exact Sakura Mochi selection role, selection
  foreground, and history background/accent roles.

The review initially blocked acceptance on two defects:

1. the opaque selection fill erased underlines and strikethroughs; and
2. selected glyph repaint used forward order instead of the renderer's
   intentional Foot-derived right-to-left overlap order.

Both findings were fixed before graphical acceptance. Selected decoration spans
are now sliced to the selected cells and repainted with
`selection_foreground`; selected glyph repaint iterates right-to-left. Regression
tests cover the decoration foreground and observable overlap order.

Final parent validation after those fixes:

- `cargo test -p splinterm --lib`: 351 passed, 1 ignored manual benchmark;
- `cargo clippy -p splinterm --lib -- -D warnings`: passed;
- `cargo fmt --all --check`: passed;
- `cargo test --workspace -- --test-threads=1`: passed; and
- `git diff --check`: passed.

The guarded Sakura Mochi graphical acceptance recorded in [`README.md`](README.md)
subsequently passed. The review round and its in-scope fixes are therefore
closed.
