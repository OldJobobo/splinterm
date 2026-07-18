# Spike 0013: fractional scaling, text input, and accessibility

- **Status:** Implemented; fractional-output and active-IME live matrix pending
- **Date:** 2026-07-18
- **Plan:** [Omarchy-native terminal MVP](../plans/0002-omarchy-terminal-mvp.md)

## Mechanism

The client binds `wp_fractional_scale_v1` only as a pair with `wp_viewporter`.
When either global is absent it retains integer `wl_surface` buffer-scale
handling. Preferred scale is represented in protocol-native 120ths. Buffer
sizes use checked `ceil(logical × scale / 120)` arithmetic, viewport destination
stays in logical coordinates, and pointer hit testing, terminal grid sizing,
damage, and IME cursor rectangles convert through the same scale. Scale changes
drop SHM backing and rebuild scale-keyed font metrics and glyph images rather
than bitmap-upscaling an old frame.

One `zwp_text_input_v3` object is created for the keyboard seat when available.
It enables only after both keyboard focus and the protocol `enter` event are
present, disables on focus loss, and is destroyed with its seat. Client commit
requests are counted for `done` serial accounting. Preedit and commit events are
double-buffered until `done`; commit replacement and aggregate text are bounded
to 64 KiB. Committed UTF-8 uses the existing bounded terminal input path.
Preedit text is rendered at the daemon cursor with combining marks attached to
the preceding cell and wide characters represented with spacer cells. The
base daemon snapshot is restored when composition ends, focus leaves, the seat
disappears, or scale changes.

Terminal body text is never exposed as IME surrounding text. Splinterm omits
`set_surrounding_text` and declares terminal content purpose with no hints. The
cursor rectangle is supplied in logical surface coordinates. xkb/compose UTF-8
remains enabled whenever no preedit is active, preserving operation when the
compositor advertises text-input-v3 but no input method consumes keys.

A high-contrast border distinguishes keyboard focus. Cursor blinking requires
focus and is disabled when `SPLINTERM_REDUCED_MOTION` is one of `1`, `true`,
`yes`, or `on`; disabling blink immediately damages the cursor row so it cannot
remain hidden. The xdg-shell protocol has no semantic accessibility-label API,
so the existing title suffix (`— controller`) remains the truthful control
label until the trusted toolkit surface planned for Phase 7.

## Validation

Pure tests cover exact 1×, 1.25×, 1.5×, and 2× buffer dimensions; fractional
cell/cursor coordinate mapping; scale-specific cache identity; focused and
unfocused overlays; reduced-motion blink policy; bounded IME preedit/commit
replacement; stale `done` serial handling; and bounded composed preedit state.
Workspace tests and strict Clippy pass.

On empty workspace 8 / DP-2 at its configured 1× scale, the production window
rendered and accepted normal UTF-8 input while text-input-v3 was bound, proving
the inactive-IME compose fallback does not suppress typing. The installed
Fcitx5 instance remained inactive for the synthetic keyboard gesture, and all
physical outputs are currently configured at 1×, so active preedit/commit and
live 1.25×/1.5×/2× monitor transitions cannot honestly be claimed from this
run. The deterministic protocol/state tests cover those paths pending suitable
output and input-method fixtures.

## Residual risks

- Preedit cursor range styling is not yet rendered; the preedit text itself is
  visible and commits correctly.
- Pixel comparisons across live fractional scales remain hardware/compositor
  validation work when such outputs are available.
- Trusted semantic accessibility labels require the Phase 7 consent/UI surface;
  terminal content cannot safely provide them.
