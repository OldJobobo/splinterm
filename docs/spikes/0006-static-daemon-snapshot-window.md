# Spike 0006: static daemon snapshot in the native window

- **Status:** Successful first terminal frame
- **Date:** 2026-07-18
- **Plan:** [Omarchy-native terminal MVP](../plans/0002-omarchy-terminal-mvp.md)
- **Demo:** `tools/run-static-snapshot-window-demo.py`
- **Capture:** `artifacts/0006/static-daemon-snapshot.png`

## Question

Can `splinterm window` attach once to the daemon-owned live Splint and render the
owned semantic `TerminalSnapshot` in the accepted native Wayland window without
making the graphical client responsible for shell lifetime?

## Result

The production command now performs the protocol handshake, inspects the live
Splint identity, attaches with zero scrollback rows, takes ownership of the
returned snapshot, drops the connection, and opens the Wayland window.

The client renders:

- physical visible rows and columns;
- empty cells and wide-character spacer cells;
- composed strings as shaped clusters;
- one- and two-cell glyph spans;
- Foot-derived supported box drawing;
- RGB and fixed indexed/default colors;
- dim, reverse, and conceal rendition; and
- the snapshot cursor coordinate.

The workspace-safe demo created an isolated daemon and PTY, wrote ASCII, box,
Nerd Font, combining, CJK, emoji, and ANSI color output, waited for a marker in
the semantic snapshot, then routed `splinterm window` to workspace 8 on DP-2.
The reviewed capture has SHA-256
`1281b7aac64463685191d48b8d2f42283abce7abd018423917ec7f6fbe456a0f`.

After closing the graphical window, a new CLI snapshot still returned the same
Splint incarnation and `FRAME_READY_7D31`, proving that closing the static viewer
did not terminate the daemon-owned shell.

## Current limitations

- This is one immutable snapshot; subscription updates are not consumed yet.
- Keyboard input is not sent to the shell.
- The wire DTO does not yet include the daemon's active palette/default colors,
  so indexed/default colors use a documented fixed client palette.
- The wire DTO does not expose cursor visibility, style, or color; the first
  frame uses a fixed outline cursor.
- Resize does not yet send a PTY/grid resize request.

These limitations prevent a terminal-usability claim, but the frame is real
daemon terminal state rather than placeholder graphics.
