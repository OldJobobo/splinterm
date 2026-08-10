---
title: Why native Wayland?
description: What Splinterm gains from speaking Wayland directly, how persistence changes the model, and where the current limits are.
---

Splinterm is a native Wayland client. It speaks directly to the compositor instead of presenting an X11 window through the XWayland compatibility layer.

That choice improves how the terminal participates in a modern Wayland desktop. It is not, by itself, what makes Splinterm unique. The larger difference is that the native window is only a disposable view into terminal state owned by the headless `splinterd` daemon.

```text
Hyprland / Wayland compositor
             ↓
  disposable splinterm window
             ↓
   persistent splinterd topology
             ↓
     shells · layouts · scrollback
```

## What users gain

### Compositor-aware scaling

Splinterm receives output geometry and scale information directly from Wayland. Its default `output-scale` font policy follows compositor output scaling, including fractional output scale. This avoids routing display behavior through an X11 compatibility window.

See [Configuration](/docs/configure/configuration/#font-sizing-and-wayland-scale) for the available sizing policies.

### Native input and clipboard

The graphical client handles Wayland keyboard and pointer events, clipboard exchange, compose input, and IME preedit. The IME cursor rectangle follows the active terminal cell so candidate and composition interfaces can be positioned by the compositor.

### Damage-driven presentation

Splinterm tracks changed terminal regions and schedules frames with the compositor. It can update damaged areas without treating every terminal change as a reason to blindly redraw the complete window.

Damage-driven does not mean GPU-rendered. The current renderer uses CPU composition with Wayland shared-memory buffers.

### Optional compositor effects

When the active theme requests translucency and blur, and the compositor advertises the compatible protocol, Splinterm can request native background blur for its window region. Missing protocol support remains a normal no-blur case rather than a startup failure.

### Stronger client isolation than traditional X11

Wayland does not give ordinary clients the broad global window inspection and input-injection model traditionally available to X11 clients. This is a useful platform boundary, but it does not make Splinterm absolutely secure. Automation authority, terminal output, controller ownership, and graphical focus remain separately constrained.

## The persistence difference

Many current terminal emulators can use a native Wayland backend. Native Wayland support alone therefore is not the main product distinction.

In Splinterm, `splinterd` owns shell processes, terminal state, scrollback, layouts, and persistent session metadata. A `splinterm` Wayland window displays that state but does not own its lifetime. Closing the window detaches the view; it does not end the work beneath it.

The daemon itself requires neither Wayland nor X11. It can continue serving persistent sessions while no graphical client is connected.

| Model | Presentation | Process owner | Closing the window |
| --- | --- | --- | --- |
| X11 terminal under XWayland | X11 compatibility surface | Usually the terminal process | Usually ends the shell |
| Terminal with a Wayland backend | Native or selectable Wayland surface | Usually the terminal process | Usually ends the shell |
| Terminal with tmux or Zellij | Terminal plus nested multiplexer | The nested multiplexer | Multiplexer session persists |
| Splinterm | Disposable native Wayland surface | Headless `splinterd` | Daemon-owned shells and layouts persist |

These rows describe common ownership models, not universal behavior for every terminal or configuration.

Splinterm derives terminal behavior from [Foot](https://codeberg.org/dnkl/foot), a Wayland-native terminal. The architectural departure is Splinterm's persistent daemon topology, native panes and tabs, and bounded access to that same topology for graphical, CLI, SSH relay, and automation clients.

## What native does not promise

- **Not automatic speed.** Removing XWayland is not a universal performance guarantee.
- **Not GPU rendering.** Splinterm currently uses CPU/Wayland-SHM composition.
- **Not universal compositor support.** The validated target is x86_64 Omarchy/Arch Linux under the documented Hyprland environment.
- **Not an absolute security claim.** Wayland isolation is one boundary inside a larger authority model.
- **Not compositor control.** Creating or mutating a persistent Dojo does not map, focus, move, resize, or assign a native window to a workspace.

See [Current status](/docs/status/) for the supported target and [Core concepts](/docs/concepts/) for the distinction between persistent topology and disposable presentation.
