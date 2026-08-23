# ADR 0003: Use SCTK with calloop for the graphical Wayland boundary

- **Status:** Accepted
- **Date:** 2026-07-18

## Context

Roadmap Phase 2 needs a disposable native Wayland client while `splinterd`
remains headless and owns the persistent shell. The client must explicitly own
xdg-shell lifecycle, SHM buffers, frame pacing, output scale, seats, keyboard,
clipboard, and later IME state. First-party unsafe code remains forbidden.

The native-window and integer-scale spikes proved registry binding, xdg-shell,
configure handling, SHM slot reuse, frame callbacks, resize, output enter/leave,
scale notifications, keyboard discovery, clean close, and nested 2× operation.
Foot 1.27.0 at commit
`3c5b584b0eafa772eb4376fb6eaf6643399e190e` remains the lifecycle reference.

Evaluated approaches were direct `wayland-client` dispatch, Smithay Client
Toolkit (SCTK), a GTK shell, and using Foot as the product presenter. Direct
protocol code adds substantial lifecycle boilerplate without improving the
first slice. GTK would replace rather than preserve the intended native
terminal boundary. Foot is an oracle, not the product window.

## Decision

Use `smithay-client-toolkit` 0.20 with its calloop integration and
`wayland-client` 0.31 inside the graphical `splinterm` client.

Project-owned modules wrap dependency types and retain control over:

- global binding and connection lifetime;
- xdg-surface/toplevel configure and close ordering;
- logical versus physical dimensions;
- SHM buffer allocation, replacement, attachment, and damage;
- frame-pending state and redraw scheduling;
- output scale and renderer-cache invalidation;
- seat-specific keyboard ownership; and
- conversion of callback failures into the client result boundary.

Integer scaling uses `wl_surface.set_buffer_scale`, physical SHM dimensions,
and buffer-coordinate damage. Renderer state is rebuilt for the physical
scale. The 2× nested-compositor spike demonstrated doubled dimensions before
and after resize.

Fractional scaling will use `wp_fractional_scale_v1` plus `wp_viewporter` behind
a project-owned wrapper. Preferred scale is interpreted in units of 1/120;
physical buffer dimensions are rounded up, surface buffer scale remains 1, and
the viewport destination remains the configured logical size. If either global
is absent, integer output scaling remains the fallback.

## Consequences

- Wayland dependencies remain confined to the disposable graphical client.
- `splinterd`, `splinterm-terminal`, and wire DTOs remain renderer-independent.
- SCTK provides safe protocol infrastructure but does not define Splinterm's
  public architecture or override Foot-compatible behavior.
- The production client may expose the accepted native shell before terminal
  snapshots are attached, but it must describe that mode as renderer evidence,
  not a usable terminal.
- Fractional scaling, clipboard, and IME require follow-up implementation on
  this boundary without changing daemon ownership.
- Compositor disconnect recovery remains future work; current failure is clean
  client termination and never shell termination.

## Validation

Retained validation evidence and pure tests cover checked 1×/2× dimensions,
stride, zero scale, and overflow. Human graphical
launches use workspace-safe scripts targeting workspace 8 on DP-2.
