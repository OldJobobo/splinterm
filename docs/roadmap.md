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

## Phase 2 — Omarchy-native terminal MVP (complete)

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
complete with durable evidence. Phase 9 produced and isolated-validated a
private Arch prerelease package with coherent binaries, service, desktop/theme
integration, upgrade handling, documentation, and licenses. Installation and
public/AUR publication remain optional post-milestone decisions.

## Phase 3 — multiplexing (complete)

Detailed plans: [`plans/0004-phase3-multiplexing.md`](plans/0004-phase3-multiplexing.md) and the [`line/frame pane divider follow-up`](plans/0005-pane-divider-styles.md)

- Splint-tree editing and focus navigation
- Multiple windows per dojo
- Detach, reattach, rename, kill, and restore workflows
- Scrollback ownership and search
- Multiple simultaneous clients with explicit control-transfer semantics
- Bounded daemon-owned literal scrollback search with client-local navigation

Protocol v17 completes explicit multi-client control status, deny/accept/confirmed
forced transfer, disconnect and timeout handling, and revision-bound search. The
serialized daemon lifecycle suite and guarded line/frame graphical smoke pass;
the former aggregate SIGINT shutdown race is closed by owned connection tasks
and one pinned shutdown signal.

## Phase 4 — headless access and supported automation

- Headless `splinterd` service for homelab/server deployments
- Dedicated policy-scoped SSH stdio relay (complete)
- Stable JSON/NDJSON CLI and published schemas
- Supported third-party capability policy and audit inspection
- Publication-snapshotted Dojo/window descendant policy semantics (complete)
- Inject non-authoritative Dojo/window/Splint/incarnation context into PTY children (complete)
- Public-CLI reference session picker and client-author examples (complete)
- Required full-capability `splinterm-mcp` adapter with supported-automation parity (complete)
- Reference in-Splint flow: discover, split, launch, observe, denied control, reconcile (complete)
- Logical topology automation remains separate from compositor-native window control
- No network listener in `splinterd` by default

## Phase 4.1 — output throughput stabilization (complete)

Detailed plan: [`plans/0009-output-throughput-optimization.md`](plans/0009-output-throughput-optimization.md)

- Instrumented PTY drain, terminal parsing, update publication, subscriptions, and snapshots
- Removed unconditional full-scrollback enumeration from ordinary parser actions
- Coalesced already-queued bounded subscription updates before protocol snapshots
- Preserved action-based revisions, exact resync behavior, bounded memory, and small-write latency
- Revalidated the randomized five-terminal output matrix before image-protocol implementation

The guarded matrix reduced Splinterm's 2,000-line child-write medians from
1.28–1.38 seconds to 40.2–44.8 milliseconds and visible-marker medians from
1.87–1.89 seconds to 511–533 milliseconds. All 150 measured cases were valid,
cleanup was verified, and the complete workspace validation suite passes.

## Phase 5 — bounded terminal image protocols

Detailed plan: [`plans/0008-terminal-image-protocols.md`](plans/0008-terminal-image-protocols.md)

Slices 0–1 are accepted: contracts, budgets, pinned-Foot fixtures, and the
bounded generic image lifecycle are implemented. Slice 2 is in progress with a
streaming DCS Sixel path and a bounded decoder matching the five pinned semantic
fixtures; Foot cursor/scroller, query, reflow, fuzz, and full differential work
remain before Slice 2 closes.

- Generic sparse image-content and placement plane without enlarging every cell
- Streaming, bounded graphics parsing rather than whole-image OSC/DCS/APC buffers
- Foot-compatible Sixel with pinned differential fixtures
- Practical Kitty static-image support: direct chunked PNG/RGB/RGBA,
  transmit/display/query/delete, IDs, crop, scale, offsets, and z-order
- Bounded daemon-owned semantics across scrollback, resize, panes,
  detach/reattach, updates, and resynchronization
- Separate on-demand pixel transport with a low-copy local path and bounded
  transport-independent fallback
- CPU/Wayland-SHM composition before any separately justified GPU evolution
- Optional iTerm2 compatibility, external Kitty transports, Unicode
  placeholders, and animation only through later security/performance gates

## Phase 6 — Nix and tertiary distribution

- Nix package, flake checks, and Home Manager module
- Reproducible release artifacts for other distributions
- Sandboxed formats only after daemon/socket integration is designed

## Deferred decisions

Foot is the authoritative terminal implementation and behavioral baseline; see
[ADR 0001](adr/0001-foot-rust-port.md). Benchmark before selecting the final
Rust renderer/font infrastructure, wire encoding, persistence format, and any
temporary migration bridges. The scaffold keeps those choices behind crate and
protocol boundaries.
