# PR #7 active-divider review

- **Commit:** `7813220e757535d8a3bad18717d8122a4c1fd5e6`
- **Base:** `23e398733bdda68aa4dd348b2b90b7a16cc13b24`
- **Scope:** `crates/splinterm/src/wayland/chrome.rs`
- **Decision:** Approve

## Parent inspection

The change paints the complete inactive divider network first, then overlays
only active segments and junction arms. `divider_active_segment` derives exact
leaf overlap from the separator rectangles produced by `PaneLayout`; complete
two-pane borders are partitioned into directional halves with the leading side
owning an odd remainder. Checked logical bounds and endpoint-scaled buffer
conversion preserve clipping and adjacency.

The four new regressions directly cover both two-pane orientations, odd shared
extents, nested active spans, junction-arm selection, and a 150/120 raster case.
No correctness or scope blocker was found in the actual diff.

## Independent read-only review

A fresh reviewer independently inspected `origin/main...HEAD`, including the
separator invariants in `crates/splinterm/src/pane.rs`. It approved without a
blocker. The reviewer confirmed:

- exact two-pane directional ownership;
- active spans limited to geometrically adjacent nested leaves;
- inactive-first painting preserving junction connectivity;
- independent arm overlays for both axes;
- endpoint-based fractional scaling without clip gaps; and
- checked logical bounds plus final canvas clipping.

Its residual risk was visual symmetry across the other tee orientations and
fractional-scale appearance. The guarded graphical matrix covered all four tee
orientations and found no junction gap or whole-tee promotion.

## Validation

The rebased commit passed:

```text
cargo test -p splinterm --lib
  386 passed; 0 failed; 1 ignored
cargo clippy -p splinterm --all-targets -- -D warnings
cargo fmt --all --check
git diff --check origin/main...HEAD
```

Each of the four added tests was also invoked by name and passed. Guarded visual
acceptance produced sixteen deterministic captures at `888×642` on isolated
workspace 8 / DP-2. Evidence is retained under
`docs/benchmarks/artifacts/2026-08-17-pr7-active-divider-graphical/`.

## Residual risk

The graphical matrix ran at compositor scale 1.0. The raster regression covers
150/120 scaling, but additional compositor-scale screenshots were not required.
The mixed legacy line/frame smoke retains an unrelated stale frame-edge
assertion and is explicitly excluded from acceptance authority.
