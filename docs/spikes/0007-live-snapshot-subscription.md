# Spike 0007: live ordered snapshot subscription

- **Status:** Implemented and validated headlessly and on workspace 8
- **Date:** 2026-07-18
- **Plan:** [Omarchy-native terminal MVP](../plans/0002-omarchy-terminal-mvp.md)

## Question

Can the graphical client keep the daemon attachment alive, apply ordered
subscription snapshots in its Wayland-owned renderer, recover explicitly from
stream gaps, and close without terminating the daemon-owned shell?

## Mechanism

`splinterm window` now keeps its authenticated protocol connection and
subscription alive while the Wayland client runs in a Tokio blocking task. A
bounded four-entry Tokio MPSC channel is the only protocol-to-Wayland bridge.
The async producer awaits capacity; it cannot grow without bound.

The protocol reader validates the active subscription ID and monotonically
contiguous event sequence. Events for an old subscription are ignored. A
sequence gap or `ResyncRequired` detaches the old subscription, performs a new
atomic attach, sends the fresh owned snapshot to Wayland, and resets sequence
tracking. `Exited` sends an explicit shutdown message.

The Wayland loop drains the bounded queue before dispatch, coalesces pending
snapshots to the newest revision, rejects a different Splint identity or process
incarnation, ignores stale/duplicate revisions, rebuilds the scale-specific
snapshot frame, updates the title, and redraws. A disconnected update producer
closes the disposable graphical client cleanly.

Closing the window completes the blocking task and drops the protocol
connection/subscription. It never issues `Terminate`; the daemon-owned shell
continues running.

## Current repaint model

The current protocol deliberately sends complete owned snapshots for updates.
Each accepted revision therefore rebuilds the immutable snapshot frame and
submits full-buffer damage. This is bounded and correct for the Phase 2 bridge,
but it is **not** damage-driven rendering and no such claim is made.

Phase 4 must introduce semantic row/cell damage DTOs, apply updates to a
client-owned semantic view, coalesce damage, and repaint only affected regions.

## Validation scope

Pure tests cover subscription identity filtering, contiguous ordering, sequence
gap resynchronization decisions, `ResyncRequired`, exit shutdown, snapshot
identity checks, and stale/duplicate revision rejection. Existing renderer
tests continue to cover wide/composed cells, colors, conceal, cursor placement,
and bounded dimensions.

The workspace-safe isolated-daemon demo launched the production window on
workspace 8 / DP-2. Additional PTY input was sent after the window mapped; the
visible frame advanced through multiple revisions without reopening the client.
The reviewed capture is
`artifacts/0007/live-snapshot-updates.png` (SHA-256
`c9664bcba06668eadfe6b648a0ecbe6ca4e3e682881f8e956f3389fb73a1af54`).
Workspace 8 was empty after cleanup.

## Remaining limitations

- Keyboard input and resize ownership are not implemented.
- Indexed/default colors still use the fixed client fallback palette because
  palette/default-color state is absent from the wire snapshot DTO.
- Cursor visibility, style, and color are absent from the wire DTO; rendering
  still uses the fixed outline cursor.
- Updates remain full snapshots/full-frame repaints until Phase 4.
