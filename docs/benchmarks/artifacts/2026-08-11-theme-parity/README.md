# Theme-palette parity acceptance

This artifact records the guarded graphical acceptance for exact selection and
scrollback-overlay theme roles after the renderer channel-order correction.

## Scope

- Candidate: dirty development build from the theme-parity slice based on
  `03f45517c87b5ee4a009ed5ec283c91c57404ce6`.
- Theme: the active Sakura Mochi `colors.toml` and `foot.ini`, copied into the
  isolated state directory because the native theme reader correctly rejects a
  cross-directory symlink.
- Isolation: one development client and matching development daemon on workspace
  8 / DP-2 with a private socket, state directory, config directory, and fixture.
- Input: exact-window `Shift+PageUp` / `Shift+End` for history and one bounded
  same-row pointer drag for selection.
- Cleanup: the exact candidate window, client, daemon, fixture, and temporary
  state were removed; workspace 8 was empty; original focus and pointer position
  were restored.

## Result

PASS.

- `selection-sakura.png` shows one opaque Sakura pink selection band with the
  resolved dark selection foreground. It has no translucent tint and no
  red/blue-swapped purple.
- `history-sakura.png` shows the detached-history graphic using the themed dark
  panel and Sakura pink accent. It has no red/blue-swapped purple.
- The screenshots contain no exact `#8838f2` pixels, the swapped form of Sakura
  Mochi `#f23888` that exposed the original defect.
- Wayland screenshot pixels are display-color-managed, so literal source-role
  byte equality is established by the renderer pixel tests rather than by
  sampling these screenshots.

## Guard notes

The first PID-based mapping poll expired while font initialization was still in
progress. Before any input, the same candidate was then identified unambiguously
as exactly one mapped `com.oldjobobo.splinterm` window with the launched PID on
workspace 8 / DP-2. Keyboard copy-mode attempts were intercepted by the active
IME, so the approved pointer-drag path was used instead. No input was sent to an
unidentified or unrelated window.

## Evidence

- `selection-sakura.png`
- `history-sakura.png`
- `SHA256SUMS`
- [`review.md`](review.md)
