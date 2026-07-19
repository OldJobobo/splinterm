# Spike 0017: Renderer path equivalence and scrollback viewport foundation

- **Status:** Foundation implemented; Wayland navigation and history paging remain
- **Roadmap:** Phase 8.1

## Renderer path equivalence

The production snapshot cache now uses the exact FreeType grayscale bridge for
non-color faces. Tests additionally prove:

- cold and warm cache runs emit identical printable-ASCII metrics and alpha;
- full-frame paint and an all-rows semantic-damage paint are byte-identical for
  equivalent state, including backgrounds, glyphs, and cursor; and
- scale remains part of both glyph and raster-face cache identity.

The pinned oracle separately proves 95/95 exact fcft parity through both the
isolated bridge and the real production cache. Final Foot pre-compositor cell
composition and decoration geometry remain open gates.

## Scrollback authorization and attachment

The matching first-party Splinterm UI may now request the explicit
`Scrollback` scope together with `Observe`; clipboard and termination authority
remain excluded. Graphical attach and resynchronization request the protocol's
bounded history maximum instead of zero rows. The daemon still enforces the
scope and the existing 16-row transfer bound.

Protocol v9 adds bounded scrollback replacement metadata to semantic updates.
When terminal history appends, trims, or clears, the daemon sends the newest
bounded rows plus available/omitted counts; the client validates and applies the
replacement atomically with the terminal revision. A detached viewport can now
observe history changes without waiting for a full resnapshot.

This is intentionally not a claim of complete graphical scrollback. Sixteen
rows are only the current bounded bootstrap/update payload; practical history
still needs stable paging or row identities before the cap can safely grow.

## Client viewport model

`ScrollbackViewport` introduces a renderer-independent client state model with:

- live-bottom versus detached offset;
- bounded up/down clamping;
- history-plus-live visible-row composition;
- unseen-row accounting while detached;
- anchor adjustment as available history grows; and
- return-to-live behavior for alternate screen and cleared history.

Pure tests cover clamping, row composition, new output while detached, explicit
return to live, alternate screen, and history clearing. The Wayland client now
renders the composed viewport, suppresses the live cursor while detached,
routes wheel input locally when application mouse tracking is disabled, retains
xterm mouse reports when tracking is enabled, and supports Shift+PageUp,
Shift+PageDown, and Shift+End. URL hover and selection-copy resolve against the
composed display rather than the hidden live grid.

## Remaining work

1. Add revision-bound history paging or daemon-issued stable row identities.
2. Add a visible unseen-output indication and explicit clickable return-to-live
   affordance.
3. Define resize/reflow anchors and selection persistence across history-page
   boundaries.
4. Add injected Wayland wheel/key end-to-end tests plus detach/reattach and
   history-capacity overflow coverage.
