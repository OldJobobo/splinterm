# Roadmap

> This is an early roadmap, not a frozen implementation plan. The research in
> [`pre-planning-research.md`](pre-planning-research.md) makes Omarchy part of
> the first usable vertical slice and moves protocol security foundations ahead
> of terminal streaming.

## Phase 0 — skeleton and research (complete)

- Rust workspace and CI
- Lair/Dojo/window/splint domain model
- Versioned Unix-socket protocol
- Runnable daemon and control client

## Phase 1 — secure persistent splint (complete)

Detailed plan: [`plans/0001-terminal-kernel.md`](plans/0001-terminal-kernel.md)

- PTY spawn, resize, read, write, and child reaping in `splinterd`
- Minimal VT parser and screen grid
- Bounded, authenticated local protocol with request IDs and negotiation
- Per-operation authorization before sensitive terminal access
- Test-only attach snapshot, ordered deltas, backpressure, and resynchronization
- Durable metadata with honest relaunch/recovery semantics

## Phase 2 — Omarchy-native terminal MVP (in progress: packaging/release validation)

Detailed plans: [`plans/0002-omarchy-terminal-mvp.md`](plans/0002-omarchy-terminal-mvp.md) and the completed [`Phase 8.1 closure execution plan`](plans/0003-phase8.1-closure-execution.md)

- Native Wayland window and keyboard input under Hyprland
- Trusted consent UI plus grant, revoke, and control indication
- Font shaping/fallback, glyph cache, and damage-tracked rendering
- Clipboard, primary selection, URLs, scaling, and basic IME
- `xdg-terminal-exec`, stable app ID, desktop entry, and Arch package
- Omarchy palette inclusion and live theme application
- Foot-compatible configuration migration strategy
- Release-blocking Foot differential gate for all printable ASCII: final pixels,
  cell placement, four-sided ink clearance, padding, styles, and scales
- Bounded graphical scrollback viewport with Foot-compatible navigation,
  anchoring, follow-live, selection, resize, resync, and reattach behavior

Part 8.1 parity, scrollback, performance, and Hyprland/Omarchy sign-off are
complete with durable evidence. Phase 2 remains open only for Plan 0002 Phase 9
Arch packaging, install/uninstall/upgrade documentation, and package-level
release validation.

## Phase 3 — multiplexing

- Splint-tree editing and focus navigation
- Multiple windows per dojo
- Detach, reattach, rename, kill, and restore workflows
- Scrollback ownership and search
- Multiple simultaneous clients with explicit control semantics

## Phase 4 — headless access and supported automation

- Headless `splinterd` service for homelab/server deployments
- SSH-mediated stdio relay or Unix-socket forwarding
- Stable JSON/NDJSON CLI and published schemas
- Supported third-party capability policy and audit inspection
- Editor/client integrations
- Optional read-mostly `splinterm-mcp` adapter
- No network listener in `splinterd` by default

## Phase 5 — Nix and tertiary distribution

- Nix package, flake checks, and Home Manager module
- Reproducible release artifacts for other distributions
- Sandboxed formats only after daemon/socket integration is designed

## Deferred decisions

Foot is the authoritative terminal implementation and behavioral baseline; see
[ADR 0001](adr/0001-foot-rust-port.md). Benchmark before selecting the final
Rust renderer/font infrastructure, wire encoding, persistence format, and any
temporary migration bridges. The scaffold keeps those choices behind crate and
protocol boundaries.
