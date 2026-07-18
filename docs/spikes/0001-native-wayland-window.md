# Spike 0001: native Wayland window and SHM lifecycle

- **Status:** Successful initial mechanism spike
- **Date:** 2026-07-17
- **Plan:** [Omarchy-native terminal MVP](../plans/0002-omarchy-terminal-mvp.md)
- **Foot reference:** 1.27.0 commit
  `3c5b584b0eafa772eb4376fb6eaf6643399e190e`

## Question

Can Splinterm open and maintain a first-party native Wayland window under the
installed Hyprland session, using safe Rust project code and CPU shared-memory
buffers, without Foot, GTK, Qt, Electron, or a browser hosting the window?

## Evaluated mechanism

The spike uses:

- `wayland-client` 0.31.14 for the Wayland connection and generated protocol
  objects;
- `smithay-client-toolkit` 0.20.0 for registry dispatch, xdg-shell lifecycle,
  seat/output state, keyboard setup, and SHM slot pooling;
- SCTK's re-exported `calloop` integration for one explicit client event loop;
- `wl_shm` ARGB8888 buffers rendered entirely by project-owned safe Rust code;
  and
- server-side decorations requested through xdg-decoration support when the
  compositor/toolkit path provides them.

No first-party `unsafe` block or post-fork callback was introduced.

## Artifact

`crates/splinterm/examples/wayland-window-spike.rs`

Run directly inside a Wayland session:

```bash
cargo run -p splinterm --example wayland-window-spike
```

Press `Q`, `Shift+Q`, or `Escape`, or use the compositor close action, to exit.

## Observed result on the Omarchy reference system

The spike was launched on empty Hyprland workspace 8 and reported by
`hyprctl clients` as:

```text
class: com.oldjobobo.splinterm.Spike
title: Splinterm - Native Wayland Spike
```

The window is a genuine xdg-shell toplevel produced by the Splinterm process.
It is not displayed inside Foot. It successfully:

- connected through the Wayland registry;
- bound `wl_compositor`, xdg-shell, and `wl_shm`;
- performed the initial no-buffer commit and configure/ack lifecycle through
  SCTK;
- allocated and reused CPU SHM buffers;
- submitted full-buffer damage;
- received frame callbacks and paced an animated pulse through them;
- handled compositor resize configures and recreated size-dependent buffers;
- tracked surface output enter/leave and integer scale-factor notifications;
- discovered seats and created an xkb-backed keyboard;
- exited through keyboard or xdg-toplevel close; and
- preserved workspace `unsafe_code = "forbid"` and strict Clippy.

## Visual content

The spike deliberately renders only project-owned rectangles: a dark terminal
surface, teal status edge, header band, and cell-like grid. It does not yet
render terminal text and does not attach to `splinterd`. Its purpose is to
validate the native window, event loop, SHM, configure, frame, seat, and output
mechanisms before those concerns become production architecture.

## Initial decision

Continue the Phase 0 evidence work with this stack as the leading Wayland
candidate. Do not yet freeze it as the permanent public architecture. The
following still need evidence before the Wayland/event-loop ADR is accepted:

- repeated open/resize/maximize/fullscreen/close stress;
- compositor disconnect behavior;
- fractional-scale and viewport protocols;
- measured SHM allocation/reuse under resize churn;
- headless compositor automation;
- integration of the daemon socket without starving Wayland dispatch; and
- comparison of direct toolkit use against a thinner project-owned wrapper.

## Remaining Phase 0 work

1. Add SHM timing and allocation measurements at 80×24, 120×40, and 240×80
   equivalent pixel surfaces.
2. Perform the font discovery/shaping/raster bake-off against Foot/fcft.
3. Add deterministic text-row reference captures from pinned Foot.
4. Record the selected Wayland/event-loop and font/renderer stacks in ADRs.
5. Promote the window shell from an example into the graphical client only
   after those decisions pass review.
