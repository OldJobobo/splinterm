# Foot semantic oracle

This directory defines how Splinterm will compare its Rust terminal port against
the pinned Foot reference implementation.

## Reference

- Project: Foot
- Version: 1.27.0
- Commit: `3c5b584b0eafa772eb4376fb6eaf6643399e190e`
- Local source: `${FOOT_SOURCE:-$HOME/Playground/foot}`
- License: MIT

See `provenance.json` and the repository's `THIRD_PARTY.md`.

## Build

Build the unmodified reference outside the source checkout:

```bash
tools/foot-oracle/build-reference.sh
```

Build the patched semantic oracle in a disposable worktree:

```bash
tools/foot-oracle/build-oracle.sh
```

Then compare every fixture with Foot. Either use an isolated compositor:

```bash
FOOT_ORACLE_WAYLAND_DISPLAY=oracle-wayland-1 \
  tools/foot-oracle/run-fixtures.py
```

Or route every test window silently to an unused Hyprland workspace. On the
current development machine, workspace 8 is reserved on DP-2:

```bash
FOOT_ORACLE_HYPRLAND_WORKSPACE=8 \
  tools/foot-oracle/run-fixtures.py
```

The workspace route preserves the focused workspace and waits for each fixture
window to close before starting the next one. The runner refuses to open test
windows on the active workspace by default. For deliberate manual debugging
only, `SPLINTERM_FOOT_ORACLE_ALLOW_LIVE_WAYLAND=1` bypasses that protection.

## Human-viewable Phase 2 demo

The Phase 2 grid and Phase 3 VT implementations have scripted visual
walkthroughs rendered as text inside the pinned Foot presentation window. They
exercise the Rust kernel directly rather than replaying fixture expectations:

```bash
tools/foot-oracle/run-phase2-demo.py
tools/foot-oracle/run-phase3-demo.py
```

On the current development machine both default to workspace 8. Each launcher
builds its Rust example, refuses an occupied target workspace, and preserves
the currently focused workspace. The final frame offers `R` + Enter to replay
or `Q` + Enter to close. Each demo uses a large 22-point font only for its own
Foot window; it does not change the user's Foot configuration. Frames remain
visible for six seconds by default. Use `--workspace N` to select another empty
workspace, `--font-size N` to adjust the demo-local font, or
`--delay-seconds N` to change the reading time per frame.

The default build directories are `/tmp/splinterm-foot-build` and
`/tmp/splinterm-foot-oracle-build`. The scripts verify the exact reference
commit. The minimal builds disable documentation, themes, terminfo, utmp, and
grapheme clustering while retaining Foot's normal tests and terminal code.

## Why an oracle adapter is needed

Text output is not enough to establish terminal compatibility. Two terminal
engines can display the same text while disagreeing about:

- cell attributes and color source;
- cursor position and last-column flag;
- soft wraps versus hard line breaks;
- terminal modes and scroll regions;
- alternate-screen state;
- replies written back to the PTY;
- row metadata used during resize and reflow.

Foot does not currently ship a machine-readable semantic state dumper. The
maintained test-only patch in `patches/0001-semantic-state-dump.patch` adds one
to the pinned reference build without changing the canonical Foot checkout.

## Adapter design

The oracle uses a maintained patch applied only to a disposable Foot worktree:

1. create a detached worktree at the pinned commit;
2. apply `patches/0001-semantic-state-dump.patch`;
3. build the patched Foot executable;
4. launch it with an exact fixture payload and oracle-only logical grid size;
5. dump normalized JSON matching `fixtures/terminal/v1/schema.md` after parser
   input is consumed;
6. compare that JSON with the fixture and, later, the Rust terminal snapshot.

The adapter may add test-only constructors or accessors, but it must not change
terminal behavior. Its patch must never be applied to the canonical
`~/Playground/foot` checkout.

If a full terminal constructor remains too coupled to Wayland, implement the
adapter incrementally:

1. direct C harnesses for grid algorithms;
2. a parser/handler harness with a minimal test terminal;
3. a headless compositor-backed Foot process for remaining integration state;
4. screenshot comparison only for behavior that is inherently graphical.

## Canonical output

The oracle output will use the fixture schema and normalize implementation-only
values. It must include at least:

- dimensions;
- cursor and last-column flag;
- complete visible rows;
- hard/soft linebreak metadata;
- non-default cell attributes;
- relevant modes;
- title changes and terminal replies.

It must not include pointers, allocation sizes, renderer buffers, timestamps,
or other nondeterministic data.

## Initial fixture status

The first five fixtures are `oracle_verified` against the pinned Foot build:
printable text, soft wrapping, cursor positioning, erase-line, and basic SGR.
The current adapter requires a Wayland compositor because it exercises the
patched Foot executable. Use either an isolated compositor or an explicitly
selected empty Hyprland workspace. Opening fixture windows on an occupied
workspace can trigger compositor tiling and PTY resize events in existing
terminals. A future fully headless C test harness should remove this
requirement.

Validate fixture structure with:

```bash
python tools/foot-oracle/validate-fixtures.py
```

## fcft raster evidence probe

The test-only probe reports raw fcft glyph placement, image dimensions,
advances, half-open nonzero-alpha ink bounds, and tightly packed row-major alpha
masks without opening a window. It emits every printable ASCII character
(U+0020 through U+007E) plus the existing CJK, emoji, and combining evidence:

```bash
tools/foot-oracle/run-fcft-mask-probe.sh \
  > /tmp/splinterm-fcft-glyphs.jsonl
```

It compiles `fcft-mask-probe.c` against the fcft 3.3.3 static library produced
by the pinned Foot reference build. It validates the Foot revision first and
builds the reference when necessary. This compares the raster stage directly;
it does not substitute compositor screenshots for glyph-mask evidence.

Compare two probe-compatible JSONL streams by explicit glyph identity and
geometry. The command exits nonzero for missing glyphs, geometry differences,
or any alpha mismatch and writes `comparison.json` plus PGM heatmaps:

```bash
python tools/foot-oracle/compare-glyph-masks.py \
  --reference /tmp/splinterm-fcft-glyphs.jsonl \
  --actual /tmp/splinterm-glyphs.jsonl \
  --output-dir /tmp/splinterm-glyph-diff
```

The comparator does not translate or best-fit masks. Splinterm provides the
provisional Swash `ascii-glyph-evidence` exporter and the production-candidate
FreeType `ascii-freetype-evidence` exporter. Run both against pinned fcft with:

```bash
tools/foot-oracle/run-ascii-comparison.sh /tmp/splinterm-ascii-comparison
```

The command reports provisional Swash, isolated FreeType, and the real
production snapshot cache separately. Review `swash-diff/`, `freetype-diff/`,
and `production-diff/`; never replace the fcft reference with any generated
output. Both pinned FreeType gates must remain 95/95 exact.

## Final-buffer oracle

The versioned raw-buffer contract is documented in
`final-buffer-schema.md`. Splinterm can already export the exact production
`TerminalSnapshot` → `SnapshotFrame` → `paint_snapshot` buffer without opening
a Wayland window:

```bash
cargo run -q -p splinterm --bin final-buffer-capture -- \
  --output-prefix /tmp/splinterm-final/ascii \
  --fixture ascii --font-size 12 --scale-120 120 \
  --columns 95 --rows 1 --hide-cursor
```

The exporter writes atomic `.json` metadata and `.argb` raw little-endian BGRA
bytes. The bounded fixture manifest covers ASCII, spacing/punctuation,
narrow/wide runs, 80/240-column drift, edge cells, reverse, dim, conceal, and
hidden/block/beam/underline cursor states. Four-sided padding is reported
explicitly but remains symmetric until Slice 2 owns that geometry.

Compare two captures without translation or best-fit alignment:

```bash
python tools/foot-oracle/compare-final-buffers.py \
  --reference-metadata /tmp/foot/ascii.json \
  --actual-metadata /tmp/splinterm-final/ascii.json \
  --output-dir /tmp/final-buffer-diff
```

The comparator rejects incompatible declared origins/geometry and unsafe input,
then writes exact mismatch count, maximum channel delta, bounds, per-cell
counts, edge-clearance deltas, first divergent cell, a PPM heatmap, and a
paired reference/actual/difference crops. Its parser, comparator, and manifest
tests run with:

```bash
python -m pytest -q \
  tools/foot-oracle/test_compare_final_buffers.py \
  tools/foot-oracle/test_run_final_buffer_comparison.py
```

The matching disposable Foot pre-submit capture is implemented by
`patches/0002-final-buffer-dump.patch`. A runner-controlled marker prevents
startup frames from becoming evidence, and the patch retains the most complete
marked frame so shutdown repainting cannot replace it.
`build-oracle.sh` applies all numbered patches in lexical order and never
changes the canonical Foot checkout.

Run the default end-to-end capture only on the reserved workspace 8 / DP-2:

```bash
tools/foot-oracle/run-final-buffer-comparison.py \
  /tmp/splinterm-final-buffer --workspace 8
```

The runner refuses every other workspace/monitor mapping, refuses to run while
DP-2/workspace 8 is active or occupied, uses an `exec` launcher so Hyprland's
silent no-focus rule follows the Foot PID, checks placement before and throughout
capture, and aborts and cleans up if the user's active workspace moves to the
test display. Window operations use Hyprland 0.55's table-form Lua dispatchers.
If the compositor contributes trailing residual pixels, the runner recaptures
Splinterm at Foot's declared logical surface size instead of translating images
or widening comparison tolerances.

The command preflights Foot, patch, font, native-library, and Cargo.lock
provenance; builds and tests patched Foot; runs all 16 manifest cases; and writes
an aggregate `summary.json`. The clean 2026-07-19 closure run at
`/tmp/splinterm-final-buffer-clean-retest/` passed 16/16 with zero ARGB
mismatches, exact geometry, four-edge padding, origins, ink bounds, and no
80/240-column drift. The Slice 2 closure run at
`/tmp/splinterm-slice2-foot-edge-final/` retained exact bytes and zero edge delta
for all 13 non-cursor fixtures with one compositor-added bottom residual pixel.
The three cursor-cell-only differences remain assigned to Slice 3 because Foot
is intentionally unfocused on the inactive test workspace.

The final two mismatches were not FreeType differences: 12 px fcft, isolated
FreeType, and the production cache had identical masks. Foot paints each row
right-to-left, so the one-alpha overhang pixel from `%` is composed after the
left edge of `&`; Splinterm had painted glyphs left-to-right. The compositor now
uses Foot's observable order for full and dirty-row paths, with a focused
overhang regression test. The matrix additionally resolved Foot's two-thirds
dim intensity and opaque block/one-pixel underline cursor composition.

### Slice 3 decoration/cursor v2

Slice 3 keeps v1 closed and adds separate, truthful artifacts:

- `slice3-final-buffer-fixtures.json` — structured semantic cells plus exact Foot
  VT bytes for decoration, wide-cell, italic-overhang, and dual cursor lanes;
- `validate-slice3-fixtures.py` — rejects unknown modes, malformed wide cells,
  oversized steps, and incompatible scales;
- `slice3-final-buffer-schema.md` — configured/effective cursor and physical
  capture-focus provenance;
- `compare-slice3-final-buffers.py` — exact ARGB comparison with no translation
  or pre-authorized decoration tolerance;
- `run-slice3-final-buffer-comparison.py` — bounded source-first integration
  cases at 1×, fractional, and high integer scale.

Exact source-derived vectors cover every decoration/cursor formula and rounding
boundary. The graphical subset tests integration rather than replaying that
Cartesian product. Focused-steady beam/underline evidence leaves Foot physically
unfocused and configures `cursor.unfocused-style=unchanged`; default-unfocused
evidence retains Foot's hollow cursor. Focused block ordering remains a portable
pinned-source vector because Foot deliberately paints an actually unfocused block
after its glyph even when the configured unfocused shape is unchanged. Cursor
colors use Foot's unset fallback policy rather than an explicit override.

```bash
tools/foot-oracle/run-slice3-final-buffer-comparison.py \
  /tmp/splinterm-slice3-final --workspace 8
```

The runner requires inactive, empty workspace 8 on DP-2. It snapshots DP-2's
mode, position, transform, and scale; changes only that inactive output's scale;
rechecks placement/focus isolation; and restores the original monitor state in
`finally` on success or failure. The 2026-07-20 closure run at
`/tmp/splinterm-slice3-source-first-final-3/` passed all six bounded cases with
exact bytes at 1×, 1.25×, 1.5×, and 2×.

### Slice 4 primary face/size/scale matrix

`run-font-matrix.py` compares pinned fcft against the isolated FreeType bridge
and production glyph cache for regular, bold, italic, and bold italic faces;
logical sizes 6, 12, 22, 32, 48, and 96 px; and scales 1×, 1.25×, 1.5×, and 2×.
Every cell records the resolved face file, index, SHA-256, logical size, scale,
and effective fractional 26.6 size. Metrics, decorations, advances, placement,
ink bounds, dimensions, and grayscale masks are exact with zero tolerance.

```bash
# One portable smoke cell
tools/foot-oracle/run-font-matrix.py \
  /tmp/splinterm-font-smoke --case regular-6px-120

# Complete 96-cell primary matrix
tools/foot-oracle/run-font-matrix.py /tmp/splinterm-font-matrix
```

The 2026-07-20 primary closure run at `/tmp/splinterm-font-matrix-final/`
passed 96/96 cells, 95 printable-ASCII glyphs per cell, through both Rust paths.
Fallback, combining, box-drawing, and color-glyph lanes remain separate so their
identity and color policies cannot weaken the strict grayscale gate.

## Rules

- Never silently update expected output to match Rust.
- Record intentional Splinterm divergences explicitly in the fixture.
- Keep raw input byte-exact using lowercase hexadecimal.
- Compare arbitrary input chunkings against the same final state.
- Treat Foot as the authority until an accepted Splinterm ADR documents a
  divergence.
