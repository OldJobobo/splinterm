# Spike 0010: terminal input modes and extended key encoding

- **Status:** Implemented; Phase 3 exit gate validated
- **Date:** 2026-07-18
- **Plan:** [Omarchy-native terminal MVP](../plans/0002-omarchy-terminal-mvp.md)
- **Protocol:** version 4

## Question

Can the graphical client encode terminal keys from daemon-owned mode state
rather than treating every key as mode-independent text?

## Mechanism

Protocol version 4 carries the active screen, palette, default colors, and
input-relevant terminal modes with every snapshot:

- application cursor;
- application keypad;
- focus reporting;
- bracketed paste;
- cursor visibility and blink.

The renderer now resolves indexed/default colors from the exact daemon snapshot
and hides the cursor when requested.

The Wayland key encoder supports UTF-8/xkb compose output, Ctrl ASCII control
bytes, Alt ESC prefix, Shift-Tab, F1–F12, arrows, Home/End, Insert/Delete,
Page Up/Down, keypad digits/operators/Enter, xterm modifier parameters, and
application cursor/keypad sequences. Repeat uses the current mode snapshot.
Focus enter/leave emits CSI `I`/`O` only while focus reporting is enabled.

A bracketed-paste helper wraps bytes in `CSI 200~` / `CSI 201~` when the daemon
mode is enabled. Clipboard acquisition remains Phase 5; this phase establishes
the mode-correct encoder only.

SCTK's keyboard path uses xkbcommon compose state and supplies composed UTF-8 to
the same bounded input path. Direct Wayland `text-input-v3` IME preedit/commit
support remains pending and is not faked by this work.

## Validation

Pure tests cover normal and application cursor/keypad sequences, xterm modifier
parameters, F1–F12, Ctrl/Alt/Shift behavior, composed UTF-8, repeat, focus
reporting decisions, bracketed-paste wrapping, cursor visibility, and exact
snapshot palette/default-color conversion. Existing controller lease,
subscription ordering, resize, renderer, and daemon lifecycle tests remain
green.

The workspace-safe demo launched `btop` through the Wayland keyboard on
workspace 8 / DP-2. Supporting Foot-derived Braille masks were added for
U+2800–U+28FF; unknown font glyphs now render the replacement character rather
than terminating the viewer. The graphical client was closed while `btop`
remained alive in the daemon-owned PTY, then reopened against the same socket
and controller lease. The same `btop` process was visible and accepted `q` after
reattach. Workspace 8 was empty after cleanup.
