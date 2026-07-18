# Spike 0008: essential keyboard input and resize ownership

- **Status:** First interactive Phase 3 slice implemented
- **Date:** 2026-07-18
- **Plan:** [Omarchy-native terminal MVP](../plans/0002-omarchy-terminal-mvp.md)

## Question

Can the native client send bounded essential keyboard input and own the live
terminal grid size without allowing control requests to consume or reorder the
subscription stream?

## Mechanism

The Wayland thread now owns a bounded 64-entry command sender alongside its
bounded snapshot receiver. `WindowCommand` carries either literal input bytes
or a checked resize containing columns, rows, and pixel dimensions. `try_send`
overflow or receiver disconnection fails the disposable graphical client
explicitly; input is never silently dropped.

`run_live_window` opens a second authenticated protocol connection for control.
It verifies that this connection resolves the same live Splint identity and
incarnation, then owns it in a dedicated async task. Subscription reads remain
on the original connection and therefore cannot be cancellation-corrupted or
have events consumed by an input/resize request. Every command retains the
existing daemon authorization and stale-incarnation checks.

The first key map supports:

- SCTK-provided UTF-8, including control bytes when xkb supplies them;
- Enter, Backspace, Tab, and Escape;
- arrows, Home, End, Insert, Delete, Page Up, and Page Down;
- Alt as an ESC prefix; and
- repeat through the same mapping path as initial key press.

Production no longer treats Q or Escape as window-close shortcuts. The
renderer evidence example opts into those shortcuts explicitly.

On configure and integer-scale changes, the client derives columns and rows
from physical drawable pixels and scale-specific cell metrics. It subtracts
renderer padding, clamps to the protocol minimum and maximum, includes pixel
size, and suppresses duplicate resize commands.

## Validation

Pure tests cover:

- UTF-8, essential keys, terminal sequences, Alt prefix, and SCTK control UTF-8;
- identical press/repeat mapping;
- bounded queue overflow and disconnected receivers;
- normal, minimum, and protocol-maximum grid calculations;
- duplicate resize suppression; and
- all previously established live snapshot ordering and resynchronization.

The workspace-safe isolated-daemon demo launched on workspace 8 / DP-2. After
mapping, `wtype` entered `printf "KEYBOARD_INPUT_OK\\n"` through the Wayland
keyboard path; a fresh daemon snapshot contained both the command and
`KEYBOARD_INPUT_OK`. The initial 1820×977 window produced a 128×31 terminal
grid. Floating resize to 1000×701 produced an ordered 69×22 PTY/grid resize.
Focus was restored to workspace 1 and workspace 8 was empty after cleanup.

## Deliberate remaining Phase 3 work

This slice still uses the development control authorization already present in
the protocol. It does **not** yet claim the complete Phase 3 exit gate. Pending:

- one explicit controller/size-owner lease per live Splint;
- controller indication and release/revocation behavior;
- complete Foot-derived function, keypad, modifier, and mode-dependent mapping;
- application cursor/keypad modes;
- compose and Wayland IME input;
- bracketed paste and clipboard distinction;
- focus reporting; and
- an interactive TUI close/reopen validation.

Semantic damage DTOs and damage-driven repaint remain Phase 4 and are not part
of this change.
