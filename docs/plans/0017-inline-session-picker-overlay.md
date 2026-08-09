# Plan 0017: Native Inline Session Picker Overlay

## Status

Complete — implementation, current non-graphical validation, the
operator-approved graphical matrix, exact cleanup, and independent review pass.
Evidence is retained under
[`artifacts/0017-inline-session-picker/closure-2026-08-09/`](artifacts/0017-inline-session-picker/closure-2026-08-09/).

## Final closure — 2026-08-09

The Pacman-verified client passed the approved isolated workspace-8 / DP-2 smoke
and bounded matrix across dark/light, opaque/translucent, normal/compact/minimal,
scales 120/150/240, empty/single/paged/long-Unicode catalogs, keyboard and
pointer activation, paired pointer cancellation, Escape restoration, ignored-key
PTY isolation, and New/existing same-Window switching. Every run used a private
daemon/socket/state/config, freshly validated exact Window address/PID, and exact
cleanup. DP-2 returned to scale 1.0/transform 0, workspace 8 ended empty, and the
pre-run Foot focus and cursor were restored.

Current validation passes 293 active Splinterm library tests with one ignored
manual benchmark, exact strict workspace Clippy, formatting, and diff hygiene.
Exact captures, summaries, executed harnesses, cleanup records, false-start
diagnoses, acceptance mapping, and SHA-256 manifest are retained in
[`EVIDENCE.md`](artifacts/0017-inline-session-picker/closure-2026-08-09/EVIDENCE.md).
Fresh review `c341bfbb` found no product, source, input, matrix-coverage, or visual blocker. Its sole cleanup-attestation finding was resolved by retaining exact pre/post cursor comparisons, removing every temporary harness root/script after export, and recording `final-cleanup.json`; no unresolved blocker remains.

## Objective

Replace the inline session picker's synthetic terminal presentation with a
highly polished, TUI-inspired simulated floating window rendered as trusted
Splinterm application chrome.

The picker remains:

- inside the existing Splinterm Wayland surface;
- in the current native window;
- application-owned and unavailable to terminal content;
- compatible with existing session-switching behavior; and
- fully operable by keyboard and pointer.

It must not become a shell TUI, terminal subprocess, separate Wayland window,
subsurface, or general-purpose widget framework.

## Design direction: Pane Switchboard

The picker appears as a centered, rectilinear switchboard over dimmed live pane
content:

```text
┌ RECENT SESSIONS                          6 available ┐
│ Switch to a running logical window.                  │
├──────────────────────────────────────────────────────┤
│ › + New terminal                  Start a fresh shell │
│                                                      │
│   work / editor                         2/2 running   │
│     ~/Projects/splinterm                              │
│                                                      │
│   notes                                 1/1 running   │
│     ~/Documents                                      │
├──────────────────────────────────────────────────────┤
│ ↑↓ / J K navigate   Enter open   N new   Esc cancel  │
└──────────────────────────────────────────────────────┘
```

### Visual treatment

- Dim the current panes with a theme-derived full-surface scrim.
- Use an opaque panel with a subtle offset shadow and crisp one-pixel frame.
- Relate the frame directly to Splinterm's existing pane chrome.
- Mark the selected row with three redundant signals:
  - a three-logical-pixel accent rail;
  - a subdued full-row selection fill; and
  - a leading `›` marker.
- Keep primary labels prominent and metadata muted but readable.
- Keep `New terminal` pinned above the running-session list.
- Present navigation help in a compact footer.
- Avoid glass effects, gradients, rounded card stacks, ornamental terminal
  prompts, and excessive box-drawing decoration.

### Initial geometry

All measurements are logical pixels and must be converted deterministically for
`scale_120`:

- outer margin: 16;
- maximum panel width: 680;
- panel width: the smaller of 680 or the surface width minus both margins;
- header height: 64 in normal mode;
- normal row height: 56;
- minimum pointer target height: 44;
- footer height: 40;
- border width: 1; and
- selected rail width: 3.

These are implementation starting points rather than theme configuration.
Responsive layout tests may justify small adjustments while preserving the
visual hierarchy.

### Theme roles

Do not expand the strict theme schema for the initial implementation. Derive an
internal picker palette from existing `ResolvedTheme` roles with pure, tested
color helpers. Perform mixing and relative-luminance calculations in linearized
sRGB, then encode the result back to sRGB:

- scrim source: the theme `background` mixed 80% toward black, composited over
  the rendered terminal at 55% source alpha;
- panel surface: `background` written with opaque alpha, independently of the
  terminal's configured background alpha;
- primary text: `foreground`, corrected toward whichever of black or white
  provides the higher contrast when it is below 4.5:1 against its surface;
- secondary text: a 70/30 primary-text/background mix, moved toward the primary
  text until it reaches 4.5:1 against its surface;
- frame: `pane_border`;
- selected rail: `ui_accent`;
- selected fill: a 24/76 `selection`/background mix, with selected-row text
  corrected by the same 4.5:1 rule;
- focused frame treatment: `pane_border_active` when distinguishable from the
  frame, otherwise `ui_accent`; and
- shadow: black at 35% source alpha.

The palette helper must return opaque panel, fill, frame, and text colors plus
explicit-alpha scrim and shadow colors. Tests must cover light and dark themes,
low-contrast custom themes, deterministic conversion, and the required contrast
ratios. Color must never be the only selection indicator. Do not use small
accent-colored text in the initial design; accent is reserved for the rail and
frame treatment.

## Current constraints

The current inline picker in `crates/splinterm/src/wayland.rs`:

- synthesizes terminal rows through `SessionPickerUi::snapshot`;
- constructs a synthetic `TerminalSnapshot` and `SplintId`;
- temporarily replaces the active pane, inactive panes, and layout with a fake
  `PaneView`;
- rebuilds a complete snapshot frame and clears backing storage when selection
  changes;
- maps pointer activation through terminal row numbers; and
- uses a fixed seven-item page and minimum terminal dimensions.

That implementation preserves trusted ownership and same-window switching, but
its terminal-cell presentation cannot provide a responsive application-owned
frame, explicit component hit areas, stable hover/press states, or efficient
selection-only repaints.

## Architecture decision

Keep the real pane frontend installed and introduce one specialized modal
overlay path. Do not build a generic window or widget system.

The inline composition becomes:

1. current terminal panes;
2. terminal-local overlays;
3. pane and history chrome;
4. picker scrim;
5. picker shadow, opaque surface, and frame; and
6. picker text, rows, selection state, and footer.

The picker must remain out of the persistent terminal backing buffer. It is
painted as transient application chrome after the synchronized terminal backing
has reached the selected Wayland SHM canvas.

## Ownership boundaries

### Session and metadata ownership

`crates/splinterm/src/session_picker.rs` continues to own:

- session collection and recent-first ordering;
- reopenability policy;
- human-facing metadata construction;
- UUID omission; and
- untrusted metadata sanitation and bounds.

Picker presentation items should preserve structured fields instead of
flattening them into one secondary string:

- `display_title`: the current human-facing dojo/window title, including the
  existing duplicate-name suppression and `dojo / window` composition;
- `working_directory`;
- `pane_count`; and
- `running_pane_count`.

Keep host completion data separate from those presentation fields. Inline
completion resolves an application-owned action to `(DojoId, WindowId)`;
standalone completion may continue to resolve an action to its entry index.
Avoid deriving a target by parsing rendered text.

Sanitation is field-local and deterministic. Map general control characters to
ASCII space, delete every bidi formatting character in U+061C, U+200E–U+200F,
U+202A–U+202E, and U+2066–U+2069, and collapse consecutive sanitation-produced
spaces. Bound `display_title` to 256 Unicode scalars and 160 display cells and
`working_directory` to 512 Unicode scalars and 240 display cells. Truncate only
at a scalar boundary; whenever truncation occurs, reserve one display cell and
append `…`. The display width calculation must use the same Unicode-width
convention as `ChromeText`.
The painter must still clip every untrusted string to its assigned rectangle as
a final safety boundary.

### Interaction ownership

`SessionPickerUi` should own only presentation interaction state:

- items;
- absolute selected action, including `New terminal`;
- visible range start;
- hovered action; and
- stable source keys for presentation text.

Shaped `ChromeText` objects belong to the application-owned renderer cache, not
the navigation model.

Inline and standalone completion behavior should be explicit, for example with
an application-owned host distinction rather than a discarded inline decision
receiver. Synthetic terminal revision and identity do not belong in the inline
presentation state.

### Layout and rendering ownership

Add one specialized pure layout and painter seam, likely around these concepts:

- `SessionPickerOverlayLayout`;
- `SessionPickerRowLayout`;
- `PickerHitTarget`;
- `session_picker_overlay_layout(...)`; and
- `paint_session_picker_overlay(...)`.

Layout output must include:

- panel, header, action, list, and footer rectangles;
- visible item range;
- explicit logical hit rectangles;
- compact or minimal presentation mode;
- text clipping widths; and
- a viewport/scale cache key.

Use `renderer::ChromeText` as the native text primitive. Extend it only with
measurement accessors and a narrowly scoped style input if required. Cache
shaped text by source, constrained width, style, `scale_120`, and a renderer
font-generation value. Increment that generation whenever font zoom, DPI,
font-sizing policy, or renderer font configuration changes without necessarily
changing `scale_120`. Cache only static chrome and the current, previous, and
next visible ranges; evict text outside that bounded window. Do not reshape
unchanged labels when the selection moves.

### Modal update contract

Keeping the real frontend installed means the modal must define, rather than
implicitly inherit, the treatment of every asynchronous update source:

| Input or update | While the inline picker is open | On dismissal |
| --- | --- | --- |
| Focused and inactive pane snapshots/patches | Drain and apply them to the real `PaneView`s and backing state. They may repaint beneath the scrim and must never mutate picker navigation. | Present the newest valid state; do not roll back to pixels captured at open. |
| Topology `Apply` updates | Drain and retain them in order because they carry incremental pane ownership and receivers. Do not partially install them beneath the modal. | Replay them in order before the final resize reconciliation. If the existing bounded deferred queue would overflow, cancel the picker without issuing a picker action, replay immediately, and report the cancellation rather than blocking the producer or terminating the client. |
| Theme updates from topology, focused-pane, and inactive-pane paths | Coalesce all sources into one newest deferred theme. Keep the opening palette stable while modal. | Apply the newest theme once, after deferred topology establishes the current panes. |
| Surface configure | Update surface dimensions, recompute overlay layout, invalidate committed hit geometry, and repaint. Do not emit terminal resize commands. | Emit at most one resize reconciliation for the final dimensions. |
| Scale or DPI/font-raster change | Rebuild pane rasters needed for correct display, increment the renderer generation, invalidate picker text/layout, and repaint. Do not resize the terminal grid solely because the modal is open. | Include the final scale and DPI in the one resize reconciliation. |
| Pane title, authority, and control metadata | Drain and retain the newest frontend state, but keep the picker window title stable. | Restore the normal title from the newest state. |
| Close, shutdown, or channel disconnect | Handle immediately using the normal lifecycle; the picker must not keep a dead frontend alive. | Not applicable. |

“Exact restoration” therefore means that opening and dismissing the picker does
not replace, resize, clear, or otherwise mutate terminal frontend state for
presentation purposes. Legitimate producer updates may advance that state while
the modal is open and must be visible after dismissal.

## Implementation milestones

### Milestone 1: Structured metadata and stronger sanitation

Files:

- `crates/splinterm/src/session_picker.rs`;
- `crates/splinterm/src/main.rs`;
- `crates/splinterm/src/wayland.rs`, where `SessionPickerItem` currently lives;
  and
- focused picker tests.

Work:

1. Replace `SessionPickerItem { primary, secondary }` with structured
   `display_title`, `working_directory`, `pane_count`, and
   `running_pane_count` fields.
2. Preserve the current display-title composition, recent ordering,
   reopenability, and UUID omission.
3. Implement the exact control, bidi, scalar-bound, and display-width sanitation
   contract above.
4. Keep protocol and daemon session policy unchanged.

Validation:

```bash
cargo test -p splinterm session_picker --lib
```

### Milestone 2: Presentation state independent of terminal snapshots

File:

- `crates/splinterm/src/wayland.rs`.

Work:

1. Separate navigation state from synthetic rendering and host completion.
2. Introduce `ensure_selected_visible(visible_count)` or an equivalent adaptive
   range operation.
3. Preserve arrow, J/K, Home/End, Enter, N, Escape, and selection wrapping.
4. Keep `snapshot()` temporarily only for the standalone `splinterm sessions`
   host if necessary to bound the inline milestone.

Validation:

- selection wrapping for 0, 1, 7, 8, and 64 sessions;
- Home/End behavior; and
- adaptive range tests.

### Milestone 3: Pure responsive layout and cached text

Files:

- `crates/splinterm/src/renderer.rs`; and
- `crates/splinterm/src/wayland.rs`.

Work:

1. Implement deterministic logical-pixel layout.
2. Return bounded, non-overlapping hit rectangles.
3. Add `ChromeText` measurement accessors and bounded text truncation.
4. Cache static and visible-range session text by source, width, style, scale,
   and renderer font generation, with deterministic bounded eviction.
5. Recalculate visible capacity from the available height instead of retaining
   a fixed seven-item page.
6. Do not assume the catalog contains at most 64 sessions; layout and cache work
   must remain bounded for larger daemon catalogs.

Validation:

- deterministic layouts at scales 120, 150, and 240;
- normal, compact, and minimal viewport cases;
- selected action always visible; and
- every hit target contained by the panel.

### Milestone 4: Native inline overlay lifecycle and composition

Files:

- `crates/splinterm/src/renderer.rs`; and
- `crates/splinterm/src/wayland.rs`.

This is one coherent acceptance boundary: do not remove the synthetic inline
frontend in a commit or milestone that cannot yet paint the native overlay.

Work:

1. Implement the modal update contract above, including ordered topology replay,
   cross-source theme coalescing, stable modal title behavior, and one final
   resize reconciliation.
2. Paint the scrim and picker after pane and history chrome.
3. Change `show_embedded_session_picker` to install modal state only, then stop
   constructing a fake `WindowPaneOptions` and `PaneView` for the inline picker.
4. Keep the real active pane, inactive panes, and layout installed and drain
   their update streams while modal.
5. Remove `SavedFrontend` and `session_picker_restore` once inline and standalone
   ownership is explicit; replace their predicates with one explicit
   `inline_picker_open()` state query.
6. Keep modal pixels out of `self.backing` and treat an open picker as transient
   canvas content so reused SHM buffers cannot retain stale chrome.
7. Render the background terminal cursor as unfocused and non-blinking while
   modal without mutating pane state or scheduling cursor-blink ticks.
8. Begin with full-surface damage for correctness. Optimize to picker-region
   damage only after deterministic validation.
9. Ensure selection movement does not rebuild `SnapshotFrame`, clear the backing
   allocation, reshape unchanged text, or repaint terminal backing unnecessarily.
10. Keep the native window identity and current same-window switch flow.

Do not add opening or selection animations in the first implementation. A
static design naturally honors reduced-motion behavior and keeps the initial
rendering contract bounded.

### Milestone 5: Explicit modal input and hit testing

File:

- `crates/splinterm/src/wayland.rs`.

Work:

1. Replace terminal-row pointer mapping with layout-provided logical hit
   rectangles.
2. Track a picker-specific press owner and target. Activate only when a left
   press and release resolve to the same committed target.
3. Keep pointer hover separate from keyboard selection. Vertical wheel or
   touchpad-axis steps move the keyboard selection and adaptive page while
   modal; horizontal axes are ignored. No axis event reaches terminal history or
   mouse reporting.
4. In minimal mode, make the selected-action row a hit target and use vertical
   axis navigation to reach every action before click activation. This preserves
   complete pointer operation even when only one action fits.
5. Consume every key press/repeat/release and every pointer enter, leave, motion,
   button, and axis event at the modal boundary. Modifier state may still be
   tracked, but unrecognized picker keys do nothing.
6. On open, settle and clear pre-existing terminal press ownership: send a
   matching release for an application-owned mouse press when a valid last cell
   exists, cancel an unfinished local selection without publishing it, and
   clear paste and URL press owners.
7. Give asynchronous clipboard reads an input-generation token. Advance it on
   modal open and drop completions from older generations so a pre-modal paste
   cannot arrive during or after the picker.
8. Disable text-input-v3 and clear pending preedit/commit state on open. Ignore
   preedit, commit, and done events while modal, then start a fresh text-input
   transaction on dismissal when keyboard focus permits.
9. Suppress terminal focus-in/out reports while modal, track the newest keyboard
   focus, and emit at most one reconciled report on dismissal when the effective
   terminal focus changed.
10. Suppress selection, paste, URL activation, terminal mouse reporting, history
    scrolling, and IME input while modal.
11. Invalidate committed hit geometry on configure, scale, renderer-generation,
    or visible-range changes and ignore activation until fresh layout has been
    painted and committed.

Validation:

- press-inside/release-outside does not activate;
- hover does not change keyboard selection;
- hit rectangles map to one stable action; and
- no terminal input path receives modal events.

### Milestone 6: Responsive completion and documentation

Files:

- `crates/splinterm/src/wayland.rs`;
- `crates/splinterm/src/renderer.rs`;
- `README.md`; and
- `docs/configuration.md`.

Responsive modes:

- **Normal, at least 480×320:** full header, subtitle, two-line session rows, and
  complete footer.
- **Compact:** reduced margins, one-line rows, abbreviated pane status, and no
  subtitle before removing essential controls.
- **Minimal, below approximately 280×180:** edge-to-edge application panel with
  the selected action and essential Enter/Escape help. The selected-action row
  remains clickable, and vertical pointer-axis input navigates hidden actions.

Every mode must keep `New terminal` and every session keyboard-reachable through
adaptive paging. Every mode must also remain pointer-operable: visible rows are
direct hit targets, while vertical axis navigation exposes hidden rows before
activation. The layout must never disappear solely because the preferred panel
size does not fit.

Documentation must describe the inline picker as appearing over the current
terminal rather than replacing its view. The standalone `splinterm sessions`
presentation remains outside the first inline milestone unless a later approved
slice deliberately unifies it.

## Accessibility and interaction requirements

- Maintain complete keyboard operation.
- Use at least 44 logical pixels for normal pointer targets.
- Use half-open, non-overlapping hit rectangles.
- Keep every action reachable by pointer through direct row hits or vertical
  axis navigation in compact and minimal modes.
- Communicate selection through rail, marker, fill, and text hierarchy rather
  than color alone.
- Retain selection on keyboard-focus loss while muting its focused treatment.
- Ensure primary and secondary text retain readable contrast.
- Clip every untrusted string to its assigned rectangle.
- Do not claim screen-reader support: the current CPU/Wayland renderer does not
  expose an accessibility tree. AT-SPI or another accessibility architecture is
  a separate scope.

## Validation contract

### Non-graphical validation

Add focused tests for:

- 0, 1, 7, 8, 64, and 256 sessions, including bounded text-cache behavior;
- output scales 120, 150, and 240;
- normal, compact, and minimal layouts;
- long ASCII, Unicode, combining, wide-character, control, and bidi metadata;
- bounded, non-overlapping hit rectangles;
- press-inside/release-outside cancellation;
- modal suppression of pointer, wheel, paste, selection, URL, IME, and terminal
  mouse input;
- exact Escape restoration to the newest valid frontend state;
- stale clipboard/IME completion rejection and focus-report reconciliation;
- same-window selection completion;
- focused, inactive, and topology update behavior while modal;
- cross-source theme coalescing and ordered topology replay, including safe
  queue-overflow dismissal;
- deferred configure, scale, DPI, and final resize behavior;
- font-generation cache invalidation and bounded eviction;
- no `SnapshotFrame` or cached-text rebuild on selection movement; and
- deterministic painter sentinel pixels for scrim, frame, selected rail,
  clipping, and compact fallback.

Run at each coherent milestone as applicable:

```bash
cargo fmt --check
cargo test -p splinterm --lib
cargo clippy -p splinterm --all-targets
git diff --check
```

### Graphical validation

Graphical validation is not authorized by this plan and requires explicit user
approval. When approved, it must follow the repository's guarded graphical test
sequence:

1. confirm workspace 8 is inactive on monitor `DP-2`;
2. install pre-map placement and no-focus rules;
3. run one guarded 960×600 scale-120 smoke case;
4. verify placement, focus preservation, cleanup, keyboard open/cancel, exact
   restoration, and same-window identity; and
5. only after the smoke succeeds, run the approved bounded matrix.

The proposed matrix should cover:

- dark and light themes;
- opaque and translucent terminal backgrounds;
- normal, compact, and minimal sizes;
- scales 120, 150, and 240;
- empty, single, paged, and long-Unicode catalogs;
- keyboard navigation;
- hover and press/release cancellation; and
- Escape restoration and successful same-window switching.

Any workspace, monitor, focus, or cleanup violation aborts the sequence.

## Acceptance criteria

The implementation is accepted when:

1. Ctrl+Shift+S reveals a centered, polished application panel over the current
   panes.
2. The inline picker no longer represents its presentation as terminal snapshot
   content.
3. Escape preserves picker-independent frontend state, presents the newest valid
   pane updates, and applies deferred topology and theme state correctly.
4. Choosing `New terminal` or a running session preserves the existing
   same-window switching contract.
5. No keyboard, pointer, paste, IME, URL, selection, or terminal mouse input
   leaks through the modal.
6. Keyboard and pointer activation agree on explicit picker actions.
7. Normal, compact, and minimal windows remain usable.
8. Selection remains identifiable without color.
9. Theme roles determine the complete picker appearance without new public theme
   keys.
10. Selection-only updates do not rebuild terminal frames or backing storage.
11. Focused tests, formatting, linting, and whitespace validation pass.
12. Required graphical evidence is recorded after separately approved testing.

## Risks and non-goals

### Risks

- Opaque picker surfaces over globally translucent terminal windows require
  compositor validation.
- Exact restoration must mean preservation of client frontend state; subprocess
  output can continue while the modal is open and must be safely reconciled.
- Asynchronous session-switch rejection does not yet have a polished in-window
  recovery state.
- The renderer currently provides no screen-reader accessibility tree.

### Non-goals

- separate Wayland windows or subsurfaces;
- shell or terminal TUI processes;
- a general widget toolkit;
- GPU effects, blur, or glass styling;
- picker search or filtering;
- changes to session collection or daemon lifecycle policy;
- UUID display;
- new public theme configuration keys;
- broad tolerance changes or reference regeneration; and
- redesigning the standalone `splinterm sessions` window in the first inline
  milestone.
