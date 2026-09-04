# ADR 0001: Foot is Splinterm's terminal foundation

- **Status:** Accepted as source foundation; external release authority superseded by [ADR 0013](0013-splinterm-owned-renderer-acceptance.md)
- **Date:** 2026-07-17
- **Amended:** 2026-09-04

## Context

Splinterm is intended to be a Rust-based evolution of Foot with persistent
multiplexing and first-class Omarchy integration. Early research described
existing Rust terminal engines such as `vte` and `alacritty_terminal` as
possible implementation candidates. That framing did not accurately represent
the project's intent.

The terminal emulator is not a replaceable implementation detail: Splinterm is
a Rust port of Foot as its base. Foot's behavior, data structures, performance
characteristics, supported control sequences, rendering behavior, and Wayland
integration form the starting point.

Persistent multiplexing changes process ownership and lifecycle, so the Rust
architecture will not necessarily reproduce Foot's current module coupling.
That architectural change does not change the source foundation.

## Decision

Splinterm's terminal engine and graphical terminal behavior will be developed
as a Rust port of Foot.

Foot is the historical implementation and differential baseline for:

- VT parsing and command handling;
- cells, grids, scrollback, resize, and reflow;
- terminal modes, selection, search, URLs, and shell integration;
- keyboard, mouse, clipboard, primary selection, and IME behavior;
- font handling, damage tracking, rendering, and Wayland behavior;
- configuration semantics and supported terminal capabilities.

Other terminal emulators and Rust terminal crates may be used as:

- implementation references for idiomatic Rust patterns;
- independent compatibility comparisons;
- sources of test ideas and benchmarks;
- supporting infrastructure where they do not replace Foot behavior.

They are not alternative foundations for Splinterm's terminal engine.

## Port strategy

The port will proceed incrementally rather than as an unverified big-bang
translation:

1. Pin and record the exact Foot source revision.
2. Build behavioral fixtures and differential tests against Foot.
3. Port leaf utilities and configuration semantics.
4. Port the cell, grid, scrollback, resize, and reflow model.
5. Port the VT state machine and CSI/OSC/DCS handlers.
6. Port PTY, process, and reaper behavior into daemon-owned Rust components.
7. Port input, selection, search, and terminal coordination.
8. Port rendering, fonts, shared-memory buffers, and Wayland behavior.
9. Port advanced features such as sixel after the foundational path is stable.

Temporary C/FFI bridges are permitted only as explicit, documented migration
steps. The target implementation is Rust, and each bridge must have removal
criteria.

## Evolution beyond Foot

The Foot port is Splinterm's foundation, not its final feature boundary.
Splinterm will deliberately alter and expand that foundation in the following
areas.

### Persistent multiplexing

Foot's existing server mode is a reference, not the final lifetime model.
`splinterd` will own PTYs, child processes, canonical terminal state,
scrollback, and Splint topology. The graphical `splinterm` client will own
Wayland objects and rendering resources. This enables detach, reattach,
multiple clients, persistent Dojos, windows, and Splint trees while preserving
Foot-derived terminal behavior.

### Headless operation

`splinterd` will run without Wayland on Linux servers such as `neuromancer`.
Graphical clients can attach through an authenticated SSH relay without adding
a network listener to the daemon. Foot's graphical process lifetime therefore
becomes a separable terminal-service lifetime.

### Session restoration

Splinterm will persist Lair, Dojo, window and Splint metadata, layouts, working
directories, launch commands, and optional bounded scrollback. Live processes
survive client disconnection, but a reboot or daemon loss can only relaunch and
restore metadata; it cannot resurrect kernel PTYs or exact process memory.

### Omarchy-native integration

The port will preserve Foot-derived terminal behavior while adding first-class
Omarchy support: stable application identity, `xdg-terminal-exec`, generated
palette inclusion, live theme application, Hyprland behavior, Arch packaging,
and a managed `splinterd` user service.

### Automation and AI integration

Splinterm will expose a versioned, local Unix-socket API and stable JSON/NDJSON
CLI for layout, session, terminal and event operations. Sensitive operations
will use scoped capabilities, consent, auditing, bounded reads, backpressure,
and revocation. An optional MCP adapter will remain a separate, least-privileged
client rather than becoming the terminal engine or daemon protocol.

### Architectural decomposition

The Rust port may reorganize Foot's coupled terminal structure into explicit
engine, PTY/process, protocol, daemon, renderer and Wayland boundaries. This is
an ownership and maintainability change, not permission to substitute another
terminal implementation silently.

## Compatibility policy

Foot compatibility is the baseline, not an absolute prohibition on evolution.
A divergence from Foot is acceptable when it is required by multiplexing,
security, Omarchy integration, accessibility, or an intentional product
improvement. Every material divergence must be:

- deliberate rather than accidental;
- documented with its rationale;
- covered by tests distinguishing inherited and changed behavior;
- evaluated for configuration, protocol and user migration impact;
- recorded in an ADR when it changes a foundational contract.

Features inherited from Foot should remain compatible unless a documented
Splinterm decision supersedes them. New behavior should extend the Foot base
without making parity impossible to measure.

## Provenance

The initial reference is Foot 1.27.0 at commit
`3c5b584b0eafa772eb4376fb6eaf6643399e190e`.

Foot is MIT-licensed. Adapted or translated code must retain applicable
copyright and license notices, and affected files must be recorded in
`THIRD_PARTY.md`. Splinterm must not describe derived code as clean-room work.

## Consequences

### Positive

- The project has product-owned behavioral contracts with a specific historical
  implementation available for differential measurement.
- Existing Foot users and Omarchy integration have a defined migration target.
- Foundational effort is not split across competing terminal engines.

### Costs

- Porting Foot's tightly coupled C state into safe Rust boundaries is substantial.
- Sparse upstream semantic tests require Splinterm to build a larger test corpus.
- Rust ecosystem components cannot be adopted when they conflict with required
  Foot behavior merely because they are convenient.
- Performance parity must be measured continuously rather than assumed.

## Rejected alternatives

- Building Splinterm on `alacritty_terminal`.
- Using `vte` as the authoritative parser instead of porting Foot's parser.
- Treating WezTerm, Rio, Zellij, or another emulator/multiplexer as the terminal
  implementation foundation.
- Reimplementing terminal behavior from specifications without Foot parity as
  the primary target.
