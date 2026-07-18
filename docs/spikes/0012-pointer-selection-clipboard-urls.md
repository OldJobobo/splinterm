# Spike 0012: pointer, selection, clipboard, and URLs

- **Status:** Implemented; core workspace-8 paths validated
- **Date:** 2026-07-18
- **Plan:** [Omarchy-native terminal MVP](../plans/0002-omarchy-terminal-mvp.md)
- **Protocol:** version 6

## Question

Can the disposable graphical client own daily pointer and data-transfer
workflows while the daemon remains headless and terminal content remains out of
logs?

## Mechanism

Protocol version 6 adds the daemon's mouse-tracking and SGR-mouse modes to the
existing input-mode snapshot. The client binds the Wayland pointer, core data
device, and optional `zwp_primary_selection_v1` objects through SCTK 0.20.
Pointer coordinates are converted through the scale-specific terminal frame.
Normal, button-motion, and any-motion modes emit Foot/xterm-compatible button,
motion, wheel, modifier, legacy, and SGR reports. Press-time ownership keeps
local and application press/release pairs consistent. High-resolution vertical
axis input preserves fractional remainder, emits at most eight steps per frame,
and batches reports into one bounded command. Horizontal terminal-wheel
reporting is intentionally omitted because the Foot/xterm baseline does not
assign compatible button codes. Holding Shift bypasses application mouse
reporting and restores local selection.

Left-button dragging maintains a client-local cell selection over the owned
semantic snapshot. Wide-cell spacers are omitted, trailing row spaces are
trimmed, and rows are joined with newlines. Selection highlighting is a client
overlay and does not mutate daemon terminal state. Releasing a selection
publishes it to primary selection when that protocol is available;
Ctrl-Shift-C publishes it to the regular clipboard. Middle click pastes primary
selection and Ctrl-Shift-V pastes the regular clipboard.

Offers are accepted only for `text/plain;charset=utf-8`, `text/plain`, or
`UTF8_STRING`. Reads are capped at 1 MiB and must be UTF-8. The current explicit
safety policy rejects C0 controls other than tab/newline/carriage return, plus
DEL, rather than silently injecting them. Accepted bytes pass through the
active bracketed-paste encoder. Selection source callbacks write an immutable
per-source payload to the compositor-provided file descriptor. Reads and writes
share a four-worker limit and two-second poll deadline, so stalled peers cannot
accumulate threads. Clipboard contents and selected text are never logged.

Visible same-row `http://` and `https://` tokens are detected from the owned
snapshot. Hover adds an underline overlay. Ctrl-left-click is the only launch
gesture and invokes `xdg-open` with the URL as one direct argument; terminal
content cannot launch a URL without that local gesture.

## Validation

Pure tests cover MIME preference, byte limits, UTF-8/control policy,
forward/reverse multi-row selection with wide-cell spacers, URL detection and
punctuation trimming, bracketed paste, and legacy/SGR mouse reports including
modifiers, motion, wheel, and legacy coordinate bounds. Strict workspace Clippy
and workspace tests validate all SCTK dispatch integrations.

On empty workspace 8 / DP-2, live validation exercised pointer enter/motion,
local drag selection with primary publication, regular clipboard paste, primary
middle-click paste, unsafe-control rejection, exact bracketed-paste framing,
application SGR mouse-report delivery, and URL hover motion. The URL launcher is
also protected by pure gesture-routing tests and direct-argument process
construction; an automated Ctrl-click launch was not used to open an external
application on the isolated test workspace.

## Residual constraints

- URL detection is deliberately limited to visible single-row HTTP(S) tokens.
- Unsafe-control paste has a deny policy; a trusted confirmation surface may be
  added with the Phase 7 UI rather than creating a spoofable terminal prompt.
- Primary selection is optional and degrades cleanly when the compositor does
  not advertise the protocol.
