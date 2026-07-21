# Plan 0005: line and frame pane divider styles

- **Status:** Complete and guarded-validated (2026-07-20)
- **Roadmap:** Phase 3 — Multiplexing, Slice 5 visual follow-up
- **Foundation:** [Plan 0004](0004-phase3-multiplexing.md), [ADR 0004](../adr/0004-font-and-cpu-renderer.md)
- **Reference source:** Foot 1.27.0, commit
  `3c5b584b0eafa772eb4376fb6eaf6643399e190e`

## Goal

Give multi-Splint windows an unmistakable, terminal-native visual structure with
two selectable styles:

1. **line:** one shared box-drawing divider between adjacent Splints, including
   correct corners, tees, and crossings for nested layouts; and
2. **frame:** every Splint is a separately framed panel with its own complete
   box-drawing border and an optional title in its top edge.

Divider rendering is trusted client chrome. PTY output cannot draw into it,
selection cannot claim it, and divider space is never reported as terminal
rows or columns.

## Implementation evidence

The configuration, style-aware geometry, Foot-derived box masks, trusted title
cache, live theme roles, clipped Wayland painting, and development-only capture
hook are implemented. The complete workspace test/clippy contract passes. The
former aggregate SIGINT shutdown race was independently resolved during Phase 3
closure by owning connection tasks and pinning one prioritized shutdown signal;
all seven serialized daemon lifecycle scenarios now pass together. No divider
code runs in the daemon.

The single guarded graphical case passed on inactive workspace 8 on DP-2 for a
nested three-Splint layout. It captured both styles at the same 888×608 surface,
proved active and inactive chrome colors, showed frame titles including wide
Unicode, retained user workspace/window/pointer state, and left no window or
socket residue. Evidence: [`../spikes/artifacts/phase3-pane-dividers/`](../spikes/artifacts/phase3-pane-dividers/).

## Product decisions

### Configuration

Add a project-owned section to `config.ini`:

```ini
[multiplexer]
divider-style=line
frame-title=splint
```

Accepted divider values are:

- `line` — shared single-cell divider network;
- `frame` — complete one-cell frame around every Splint; and
- `none` — compatibility and troubleshooting mode matching the current
  unpainted layout.

`line` becomes the default for multi-Splint windows so a new split is visibly a
split without additional configuration. A single-Splint window has no divider
in `line` mode. `frame` frames a single Splint because framing is part of that
style's panel identity.

Accepted frame-title values are:

- `splint` — show the daemon-owned Splint title in the top frame; and
- `none` — draw an uninterrupted top frame without text.

`frame-title` defaults to `splint` and has no visual effect unless
`divider-style=frame`. The style and title mode are loaded when the client
starts. Live style switching and a runtime key binding are deferred; live theme
color updates remain supported.

### Theme roles

Extend the project theme with two optional roles:

```json
{
  "pane_border": "#5b6570",
  "pane_border_active": "#78d2ff"
}
```

- `pane_border` paints inactive dividers and frames.
- `pane_border_active` paints the focused Splint boundary.
- Existing theme JSON remains valid: a missing inactive role derives a muted
  color from foreground/background, and a missing active role uses
  `ui_accent`.
- The Omarchy theme generator emits both roles and live theme reload repaints
  all divider chrome.

### Box-drawing authority

Use Splinterm's existing Foot-derived `box_drawing` masks for
`─ │ ┌ ┐ └ ┘ ├ ┤ ┬ ┴ ┼`. Do not depend on font-provided box glyph bearings and
do not draw unrelated freehand pixel lines. This keeps thickness, scale, and
cell-edge continuity aligned with terminal box drawing.

Divider glyphs use the active window cell metrics and scale. They are client
chrome rather than cells in any terminal snapshot.

## Style contracts

### Line style

- Reserve one cell-width lane for a left/right split and one cell-height lane
  for a top/bottom split.
- Convert the binary layout tree into a shared orthogonal segment graph.
- Resolve every divider cell from its north/east/south/west connectivity:
  straight line, corner, tee, or crossing.
- Nested splits must join without gaps, doubled strokes, or ambiguous overlap.
- Internal divider segments bordering the focused Splint use
  `pane_border_active`; remaining segments use `pane_border`.
- At a junction containing active and inactive segments, paint each arm with
  its semantic color before resolving overlap so focus does not erase network
  continuity.
- Do not draw an outer frame around the entire window.

### Frame style

- Inset every leaf by one cell on all four sides and reserve the surrounding
  cells for its frame.
- Paint a complete `┌─┐ / │ │ / └─┘` frame for each Splint.
- The focused Splint's complete frame uses `pane_border_active`; every other
  frame uses `pane_border`.
- Adjacent Splints retain distinct frames. Shared boundaries therefore show the
  two panel edges rather than collapsing into the line network.
- When `frame-title=splint`, interrupt the top edge with the daemon-owned Splint
  title, visually equivalent to `┌─ title ─────┐`. The title and its padding use
  the same active/inactive color as that frame.
- The title source is `Splint.title` from the authoritative topology snapshot.
  It is not the Dojo window title and is never terminal-controlled OSC title
  text.
- Normalize control characters and line breaks to spaces before display. Shape
  through the existing renderer and truncate at a complete shaped cluster to
  fit the available top edge; never split a combining sequence or wide glyph.
- Preserve both corners and at least one horizontal edge cell. If the frame is
  too narrow for safely padded text, omit the title for that pane rather than
  clipping a corner or shrinking the terminal further.
- A rename invalidates only that pane's top frame. Title visibility does not
  change the pane content rectangle or PTY dimensions.

## Geometry and behavior invariants

- The daemon's binary tree, ratios, IDs, and topology revision are unchanged.
  Divider style is client-local presentation and is never persisted in daemon
  metadata.
- Pane rectangles are divided into a **chrome rectangle** and a **terminal
  content rectangle** before `WindowGeometry` is fitted.
- PTY resize uses only the content rectangle. Divider or frame cells never
  inflate reported terminal pixel or cell dimensions.
- Every content rectangle must still fit the protocol minimum of 2×2 cells.
  If the surface cannot fit the requested topology plus chrome, reject the
  layout with a targeted `SurfaceTooSmall` result; never overlap or silently
  drop a frame.
- Ratio calculations remain deterministic. First-child rounding and
  second-child residual ownership stay unchanged after subtracting the exact
  style-specific chrome budget.
- Fractional scaling converts cell-sized chrome through the existing buffer
  geometry once. Neighboring box masks must meet at every supported scale.
- Pointer hit testing, selection, mouse reporting, URL hover, and IME placement
  use content rectangles only.
- Clicking a frame or divider focuses neither pane in the first version.
  Divider dragging and ratio adjustment by pointer are non-goals.
- Keyboard focus traversal and existing split/close/ratio bindings are
  unchanged.
- Terminal output, OSC titles, clipboard content, and terminal escape sequences
  cannot control frame-title text, divider color, or focus indication.
- Background alpha applies behind chrome; border colors remain composited by
  the same SHM path as other trusted overlays.

## Implementation slices

### Slice 0 — configuration and pure style contracts

**Work**

- Add `PaneDividerStyle::{None, Line, Frame}` and
  `FrameTitleMode::{None, Splint}` to client configuration.
- Accept `[multiplexer] divider-style` and `frame-title`; reject unknown values
  with line-aware diagnostics.
- Add optional `pane_border` and `pane_border_active` theme roles with
  backward-compatible defaults.
- Update the sample config, theme JSON, Omarchy generator, and configuration
  guide.

**Tests**

- divider default is `line` and frame-title default is `splint`;
- every divider and frame-title value parses exactly;
- malformed values fail without fallback;
- frame-title is inert outside frame style;
- old theme JSON resolves successfully;
- explicit border roles survive theme resolution and reload.

### Slice 1 — style-neutral chrome geometry

**Work**

- Replace the current fixed one-logical-pixel separator input with a typed
  chrome specification derived from cell metrics and style.
- Return pane content rectangles plus explicit chrome cells/segments.
- Keep focus navigation based on pane extents while routing terminal interaction
  through content rectangles.
- Add helpers that prove pane content and chrome are disjoint and fully bounded.

**Tests**

- horizontal, vertical, and nested asymmetric trees;
- odd dimensions and ratio residuals;
- minimum viable and one-pixel-too-small surfaces;
- scales 1×, 1.25×, 1.5×, and 2×;
- no content overlap and no unowned interior pixels.

### Slice 2 — line divider network

**Work**

- Build a cell-addressed orthogonal connectivity map from branch separators.
- Resolve N/E/S/W masks to the supported box-drawing character set.
- Rasterize each character through the existing Foot-derived box masks.
- Track active-boundary arms separately from inactive arms for focus color.
- Produce bounded damage rectangles for topology, ratio, scale, theme, and focus
  changes.

**Tests**

- straight horizontal and vertical splits;
- nested T junctions in all orientations;
- four-way crossings where geometry permits them;
- active color on every side of each possible focused leaf;
- exact adjacency with no gap or double thickness;
- deterministic output independent of tree traversal allocation order.

### Slice 3 — framed panels and titles

**Work**

- Derive one complete frame and one inset content rectangle per leaf.
- Paint corners and edges with Foot-derived masks.
- Preserve separate adjacent frames and clip each frame to its leaf allocation.
- Render the sanitized daemon-owned Splint title into the top edge when enabled.
- Repaint the old and new frame when local focus changes and repaint one top edge
  when its Splint is renamed.

**Tests**

- one pane receives a complete frame and its configured title;
- horizontal, vertical, and nested panels retain independent frames;
- active frame changes without repainting terminal cells;
- title disabled, empty available width, truncation, wide glyphs, combining
  sequences, bidi text, and control-character sanitization;
- rename damage is confined to the affected top frame;
- tiny panes fail before PTY resize;
- frame pixels and title ink cannot leak into neighboring content.

### Slice 4 — Wayland integration and incremental damage

**Work**

- Paint divider chrome after terminal backing pixels and before transient trusted
  overlays whose precedence requires visibility.
- Keep terminal snapshots in the backing buffer independent from chrome so a
  terminal-row update cannot erase borders.
- Include chrome changes in frame-callback coalescing and SHM damage submission.
- Ensure theme reload, scale change, topology reconciliation, split, close,
  ratio adjustment, and focus traversal invalidate exactly the necessary
  regions.

**Tests**

- terminal updates adjacent to every border orientation;
- scroll-copy and full redraw retain chrome;
- selection, URL, history, cursor, and IME overlays remain clipped to content;
- stale topology and detached panes cannot leave orphan divider pixels;
- repeated focus movement remains bounded and does not resize PTYs.

### Slice 5 — closure and guarded evidence

Run the standard non-graphical contract first:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p splinterd --test end_to_end -- --test-threads=1
```

Then run one guarded graphical smoke case on inactive workspace 8 on DP-2:

1. launch one nested three-Splint layout in `line` style;
2. verify lines, tees, focus coloring, input routing, ratio change, and
   close/collapse;
3. relaunch the client in `frame` style against the same daemon window;
4. verify independent complete frames, Splint titles, title truncation, rename,
   focus coloring, PTY dimensions, detach, and reattach;
5. confirm active workspace/window/pointer never changed; and
6. remove every test window, process, socket, and workspace rule.

Only after that smoke passes may a broader scale/theme matrix run. Record image,
geometry, PTY-size, focus, cleanup, and host/software evidence under a new
`docs/spikes/artifacts/phase3-pane-dividers/` directory.

## Likely files

- `crates/splinterm/src/config.rs`
- `crates/splinterm/src/pane.rs`
- `crates/splinterm/src/box_drawing.rs` only if a required light-line mask is
  missing; do not change Foot-derived behavior broadly
- `crates/splinterm/src/renderer.rs`
- `crates/splinterm/src/wayland.rs`
- `config/splinterm/config.ini`
- `config/splinterm/theme.json`
- `tools/generate-omarchy-theme.py`
- `docs/configuration.md`
- new focused graphical smoke tool and evidence directory

## Non-goals

- changing daemon topology or protocol;
- persisting style per Dojo, window, or Splint;
- labels beyond the configured Splint frame title, tabs, status bars, or badges;
- terminal-controlled OSC titles or Dojo window titles as frame-title sources;
- new pane-title editing UI beyond the existing Splint rename workflow;
- mouse resize handles or clickable divider actions;
- double/heavy/dashed border families;
- arbitrary user-supplied box characters;
- changing terminal box-drawing output or the pinned Foot oracle;
- broad renderer tolerance changes.

## Definition of done

This feature is complete when both styles visibly and deterministically separate
nested Splints; line mode forms one correct shared box-drawing network; frame
mode gives every Splint its own complete frame and optionally displays its
sanitized, width-bounded daemon-owned title; local focus is immediately visible;
PTY sizes and all input/selection/IME coordinates exclude chrome; old themes
remain valid; live theme changes repaint borders; non-graphical tests and the
single guarded graphical case pass; and no Foot oracle, workspace, process, or
socket residue is altered or left behind.
