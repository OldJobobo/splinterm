# Pre-planning research

> **Historical pre-implementation research.** Current product and architecture
> status are documented in [`README.md`](../README.md), [`status.md`](status.md),
> and [`architecture.md`](architecture.md). “Proposed” language below describes
> its original planning context, not current product maturity.

This document records research directions and provisional decisions for
Splinterm before implementation planning. It covers a Rust evolution of Foot,
persistent multiplexing, platform priorities, and a secure automation surface.

## Executive recommendation

Splinterm's terminal foundation is a Rust port of Foot. Foot is the
authoritative implementation and behavioral baseline, as recorded in
[ADR 0001](adr/0001-foot-rust-port.md).

The port should proceed incrementally rather than as an unverified mechanical,
file-for-file rewrite. Preserve Foot semantics, retain exact provenance, and
use differential tests against the local Foot 1.27.0 source and binary. The
fundamental ownership change is that `splinterd`, not the graphical client,
owns PTYs, child processes, canonical terminal state, scrollback, and layout.

Platform priority is part of the product architecture:

1. **Omarchy** is the reference desktop and acceptance environment.
2. **Arch Linux** is the first general packaging target.
3. **NixOS/Home Manager** is the second packaging model.
4. Other Linux distributions are tertiary until the first three are reliable.

Automation should use a local, versioned Unix-socket API plus stable JSON CLI.
An optional MCP adapter can come later as a separate, least-privileged client.
Headless `splinterd` deployments—such as the homelab host `neuromancer`—are an
explicit target. Remote use should travel through authenticated SSH; the daemon
must not expose a network listener by default.

## 1. What a Foot port actually entails

The local reference is Foot 1.27.0 at `~/Playground/foot`, commit
`3c5b584b0eafa772eb4376fb6eaf6643399e190e`.

### Foot subsystem map

| Foot source | Responsibility | Splinterm direction |
| --- | --- | --- |
| `main.c`, `fdm.c` | composition and single epoll reactor | Rust daemon/client reactors |
| `slave.c`, `spawn.c`, `reaper.c` | PTY setup, child launch and reaping | daemon-owned PTY/process crate |
| `vt.c`, `csi.c`, `osc.c`, `dcs.c` | byte parser and terminal commands | renderer-independent terminal engine |
| `terminal.c`, `terminal.h` | aggregate terminal state and coordination | split into engine, session and view state |
| `grid.c`, `grid.h` | circular grid, scrollback and reflow | owned Rust model with property tests |
| `render.c`, `shm.c` | rasterization, damage and Wayland buffers | disposable client renderer |
| `wayland.c`, `input.c`, `ime.c` | compositor, seats, clipboard and input | Wayland-only graphical client |
| `client.c`, `server.c` | server-mode process launch | replace with persistent mux protocol |

Foot's PTY reads feed its VT parser directly on the main reactor thread.
Rendering uses worker threads for CPU raster work, while Wayland operations
remain on the reactor thread. Its central `struct terminal` joins parser, grid,
PTY, renderer, Wayland and input state. That coupling is the main thing a Rust
architecture should **not** transliterate.

Foot server mode is also not equivalent to persistent multiplexing. It
amortizes initialization and creates graphical terminals for clients; the
canonical terminal lifetime is still coupled to Foot's terminal instances.
Splinterm requires tmux/Zellij-style daemon ownership.

### Recommended port strategy

1. Establish Foot provenance and a behavioral corpus before adapting code.
2. Port Foot's leaf utilities and configuration semantics.
3. Port its cell/grid model and resize/reflow invariants into owned Rust types.
4. Port Foot's streaming VT parser and handlers behind a Splinterm-owned
   terminal-engine interface.
5. Port PTY/process behavior into `splinterd` ownership.
6. Port the Wayland frontend and renderer against terminal snapshots/damage.
7. Port advanced protocols—sixel, synchronized updates, shell integration—only
   after the foundational path is measured and stable.

Potential temporary FFI boundaries are Foot's fcft/fontconfig and pixman path,
generated Wayland protocol coverage, and the post-fork PTY child setup. The
post-fork branch is especially sensitive: adding allocator or async-runtime work
between `fork` and `exec` is unsafe in a multithreaded process. A small audited
Unix implementation—or initially a retained helper—is preferable.

### Supporting crates and external references

Rust crates may support the port where they do not replace Foot behavior. The
specific choices remain subject to parity tests and license review:

- [`portable-pty`](https://github.com/wez/wezterm/tree/main/pty) may support an
  initial PTY spike if it can reproduce required Foot process behavior.
- [`wayland-client`](https://github.com/Smithay/wayland-rs) and
  [Smithay Client Toolkit](https://github.com/Smithay/client-toolkit) may provide
  Rust Wayland plumbing while Foot remains the behavioral authority.
- Font/shaping libraries may supply low-level facilities, but fallback, sizing,
  cell placement and rendering policy must preserve Foot-derived behavior.

`vte`, `alacritty_terminal`, WezTerm, Rio, Zellij and tmux are useful sources of
Rust patterns, compatibility comparisons, test ideas, benchmarks, and
multiplexing research. They are not alternative terminal foundations.
Splinterm's Foot-derived terminal behavior and its Lair/Dojo/Window/Splint
semantics, persistence and protocol remain owned by this project.

### Testing requirement

Foot's visible upstream unit coverage is sparse relative to the port's state
space. The port needs its own compatibility program:

- table-driven parser and terminal semantic tests;
- chunk-boundary invariance tests;
- grid/reflow property tests;
- arbitrary-byte and resize/input interleaving fuzzing;
- differential terminal-state fixtures against Foot;
- headless compositor screenshots for rendering, scaling and fonts;
- PTY-to-present latency, allocation, RSS, scrollback and idle-wakeup benchmarks.

Screenshots alone are insufficient. Golden state must include cells and
attributes, wrap flags, cursor, modes, tabs, hyperlinks, scroll regions, title,
palette and terminal replies.

## 2. Persistent multiplexing architecture

### Ownership model

`splinterd` should own:

- PTY masters and child supervision;
- one canonical parser/grid per live Splint;
- scrollback and terminal modes;
- Lair/Dojo/window/splint topology;
- controller arbitration and client subscriptions;
- durable metadata and recovery policy.

`splinterm` should own:

- Wayland objects and compositor lifetime;
- keyboard, mouse, IME, clipboard and primary selection;
- font discovery/shaping and glyph caches;
- rendering, frame callbacks and client-local UI state.

A client crash must not terminate or stall a Splint. A slow client must never
backpressure PTY consumption.

### Concurrency model

Use one logical actor/task per live Splint to serialize PTY output, terminal
mutation, resize, input, child exit and snapshots. A Lair/topology actor
serializes structural edits. This avoids pervasive locks in terminal state and
makes event ordering testable.

Keep topology revisions separate from high-rate per-Splint terminal revisions.
Stable object IDs must not depend on tree position. A restarted child should
have a new process-incarnation ID even if the logical Splint ID is retained.

### Attach and streaming model

Attach should be **snapshot plus ordered deltas**:

1. negotiate versions, limits and features;
2. return topology, dimensions, modes, cursor, palette and visible grid;
3. optionally return a bounded scrollback window;
4. stream revisioned damage/state events;
5. require resnapshot when a client detects a gap.

Each client gets bounded queues. Replaceable state such as title, cursor and
row damage may be coalesced. If the queue overflows, discard uncertain deltas
and issue `resync_required`; never retain unbounded output or stop reading the
PTY.

Multi-client policy should initially permit multiple observers but only one
active controller/size owner per Dojo or window. Control takeover must be
explicit and visible.

### Persistence claim

Client detach/crash persistence is achievable because `splinterd` owns the PTY.
Daemon or host restart is different: ordinary PTY file descriptors and child
relationships cannot be reconstructed from serialized Rust state. Durable
metadata can support **relaunch/recovery**, not transparent process continuity.
Documentation must preserve this distinction.

## 3. Omarchy-first product integration

The inspected live Omarchy installation is `omarchy-dev 4.0.0.r1069...` on Arch
and currently installs Foot 1.27.0 and `xdg-terminal-exec` 0.14.0.

### Verified local integration points

- `/usr/share/omarchy/default/xdg-terminal-exec/hyprland-xdg-terminals.list`
  places `foot.desktop` first.
- `/usr/share/omarchy/applications/foot.desktop` defines the
  `X-TerminalArgExec`, `X-TerminalArgAppId`, `X-TerminalArgTitle` and
  `X-TerminalArgDir` contract used by Omarchy's `xdg-terminal-exec` flow.
- `/usr/share/omarchy/bin/omarchy-default-terminal` and
  `omarchy-install-terminal` currently whitelist Alacritty, Foot, Ghostty and
  Kitty. First-class Splinterm support requires an upstream Omarchy change, not
  only a Splinterm package.
- `/usr/share/omarchy/config/foot/foot.ini` includes
  `~/.local/state/omarchy/current/theme/foot.ini`.
- `/usr/share/omarchy/default/themed/foot.ini.tpl` renders Omarchy palette roles
  into Foot colors.
- `omarchy-theme-set` regenerates templates and runs both terminal restart logic
  and `omarchy-theme-set-foot`.
- `omarchy-theme-set-foot` sends palette OSC sequences into running Foot PTYs.
- `/usr/share/omarchy/default/hypr/apps/terminals.lua` tags known terminal app
  IDs for uniform opacity. Splinterm's stable app ID must be added.
- Omarchy launches terminals through `xdg-terminal-exec`, including explicit
  `--app-id` uses for floating/system terminal windows.

These paths are evidence from the current local development release, not a
promise that Omarchy treats every file as a stable public API. Integration
should be proposed upstream and verified against a tagged Omarchy release.

### Consequences

Omarchy support belongs in the first usable vertical slice, not a late polish
phase. That slice must include:

- native Wayland operation under Hyprland;
- a stable app ID and desktop entry;
- `xdg-terminal-exec` execution, app-id, title and working-directory arguments;
- config under `$XDG_CONFIG_HOME/splinterm`;
- inclusion of generated Omarchy colors without overwriting user settings;
- immediate live recolor through a supported IPC method or OSC compatibility;
- an Arch package and user daemon lifecycle;
- fractional scaling, clipboard, IME and login/logout acceptance tests.

The best long-term design is a direct `splinterm theme apply`/IPC operation
called by Omarchy. Supporting Foot-compatible OSC palette updates remains
valuable for applications and transitional integration, but theme hooks should
not need to discover child PTYs through `/proc` once a daemon API exists.

Splinterm must still start with built-in defaults when Omarchy state is absent.
Omarchy knowledge belongs in adapters and packaging, not terminal-engine code.

## 4. Arch, NixOS and tertiary distributions

### Arch Linux

Arch is the first general distribution target because Omarchy is Arch-based.
Start with a source AUR package built using `cargo build --release --locked` in
a clean chroot. Install both binaries, desktop metadata, icons, user service
units, completions, manual pages, licenses and eventually terminfo. Packaging
must not enable services or edit home-directory configuration.

Use `namcap`, `readelf`, clean-chroot builds, `desktop-file-validate` and
`systemd-analyze --user verify` to derive and verify dependencies. Do not copy
Foot's dependency list before renderer/font choices are made.

References: [Arch PKGBUILD](https://wiki.archlinux.org/title/PKGBUILD),
[Arch Rust packaging](https://wiki.archlinux.org/title/Rust_package_guidelines),
[AUR submission](https://wiki.archlinux.org/title/AUR_submission_guidelines).

### NixOS and Home Manager

After stable Arch releases, provide a reproducible package and flake with
packages, app, checks and development shell. Commit `flake.lock`, keep a plain
package expression for non-flake/Nixpkgs use, and avoid dirty-tree inputs.

A Home Manager module is the correct first configuration layer because the
config, desktop preference and daemon are per-user. A NixOS module should only
cover system-level defaults/policy and must not force lingering or mutate user
homes.

References: [Nix flakes](https://nixos.org/manual/nix/stable/command-ref/new-cli/nix3-flake.html),
[Nixpkgs Rust packaging](https://nixos.org/manual/nixpkgs/stable/#rust),
[Home Manager](https://nix-community.github.io/home-manager/).

### Tertiary Linux

Publish portable source artifacts before creating many distro-specific
packages. AppImage, Flatpak and Snap should wait: sandbox-to-user-daemon socket
sharing, GPU/font access and consent identity require separate design work.

### Terminfo

Use `xterm-256color` as a temporary compatibility value only. Do not announce
`TERM=splinterm` until an accurate entry exists and is tested with ncurses,
Neovim, less, fzf, tmux, SSH, color, mouse, focus, bracketed paste, hyperlinks,
undercurl and the selected keyboard protocol. Native packages should own the
compiled terminfo entry.

## 5. AI-accessible IPC and API

### Recommended interface stack

1. **Primary:** Unix-domain socket under `$XDG_RUNTIME_DIR/splinterm`.
2. **Public automation baseline:** stable CLI with JSON and NDJSON output.
3. **Later adapter:** separate `splinterm-mcp` stdio process.
4. **Optional later facade:** small D-Bus activation/open-window API.
5. **Deferred:** HTTP/gRPC and all remote transport.

Do not make MCP the daemon's native protocol. MCP is an AI integration surface,
not an authorization boundary, and terminal content can contain prompt
injection. A separate adapter can request narrow capabilities through the same
API as editors and other local clients.

### Protocol requirements before terminal streaming

The current newline JSON scaffold is useful for inspection but must gain:

- bounded framing before allocation;
- request IDs and stable machine error codes;
- version-range and feature negotiation;
- deadlines and cooperative cancellation;
- authenticated peer identity (`SO_PEERCRED` on Linux);
- enforceable per-operation authorization before any terminal snapshot, input,
  process, clipboard, or mutation endpoint;
- typed subscriptions with sequence and resource revisions;
- bounded queues, gap notification and resynchronization;
- explicit frame/read/write/connection limits;
- audit events that exclude terminal bodies and secrets.

Create the runtime directory with mode `0700`, make the socket user-only, verify
peer UID, and fail closed on unsafe ownership/modes. Before removing a stale
endpoint, inspect it without following symlinks and reject anything that is not
an owned Unix socket. Development socket overrides must meet the same checks. A
pathname and same-user peer identity authenticate locality; they do not grant a
process permission to read or control terminals.

References: [`unix(7)`](https://man7.org/linux/man-pages/man7/unix.7.html),
[XDG Base Directory specification](https://specifications.freedesktop.org/basedir-spec/latest/),
[JSON-RPC 2.0](https://www.jsonrpc.org/specification).

### Permission model

Separate scopes at least as finely as:

- Lair/Dojo/Splint metadata;
- visible-screen read;
- scrollback read;
- terminal event subscription;
- terminal input write;
- structured process spawn;
- layout/session mutation;
- session termination;
- clipboard read and write.

Use short-lived, opaque daemon-minted capabilities bound to the peer and exact
resources. Reading terminal content must never imply permission to send input.
A trusted visible Splinterm client—not the model—should show consent with client
identity, resources, scopes and duration. Sensitive access should fail closed
when no trusted UI is available unless the user has installed an explicit,
user-owned policy.

### Agent safety

- Treat all terminal output as untrusted data, including output from commands an
  agent launched.
- Never interpret terminal prose as consent, a protocol frame or a tool call.
- Return provenance and truncation metadata with terminal reads.
- Never construct `sh -c` strings from API parameters. Spawn with program and
  argument arrays. If shell evaluation is ever added, make it a separate,
  conspicuous, high-risk operation.
- Make agent control visible and instantly revocable.
- Do not log terminal bodies, clipboard contents, tokens or complete commands.
- Default MCP tools to metadata and bounded, user-confirmed screen reads; do not
  expose unrestricted command execution or live output by default.

References: [CWE-78](https://cwe.mitre.org/data/definitions/78.html),
[MCP tool security](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#security-considerations),
[MCP security practices](https://modelcontextprotocol.io/specification/2025-06-18/basic/security_best_practices),
[OWASP prompt injection guidance](https://cheatsheetseries.owasp.org/cheatsheets/LLM_Prompt_Injection_Prevention_Cheat_Sheet.html).

### Headless servers and remote access

`splinterd` should support running without Wayland on a headless Linux host such
as `neuromancer`. It can own PTYs, Lairs, Dojos, windows and Splints while no
graphical client is attached. A local `splinterm` client can later attach and
render those sessions.

The preferred remote transport is SSH, not a daemon TCP listener. Two designs
should be spiked:

1. OpenSSH Unix-socket forwarding between the local client and the remote
   `$XDG_RUNTIME_DIR/splinterm/splinterd.sock`.
2. `splinterm relay --stdio`, launched through SSH, which forwards framed
   protocol traffic between standard input/output and the remote Unix socket.

The stdio relay is likely easier to discover, authorize and support across
machines. SSH supplies host authentication, encryption and user login, while
Splinterm still applies protocol capabilities and resource authorization. A
remote login must not automatically receive unrestricted terminal content or
control merely because it has the same username.

Headless operation also needs explicit service-lifetime policy. A systemd user
service normally follows the user's login manager; keeping Dojos alive after
logout may require administrator-approved lingering or a deliberately managed
service account. Splinterm must document this rather than enabling lingering
silently.

Do not bind TCP, expose loopback HTTP, automatically open firewall ports or add
LAN discovery. A future non-SSH gateway would be a separate opt-in component
with authenticated encryption, pairing/host verification, revocable
credentials, narrower scopes and auditable consent. `splinterd` itself should
remain Unix-socket-only.

## 6. Revised pre-plan order

This is a sequencing recommendation, not yet an implementation plan.

### Stage 0 — contracts and evidence

- Record Foot provenance and selected compatibility target.
- Build parser/grid corpus, differential harness and benchmarks.
- Record Foot port boundaries, temporary bridge criteria, renderer strategy and
  persistence semantics in ADRs.
- Define XDG paths, app ID, CLI execution contract and threat model.

### Stage 1 — secure one-Splint daemon vertical slice

- Daemon-owned PTY and child lifecycle.
- Rust ports of Foot's terminal parser/grid behind a project-owned interface.
- Hardened local handshake, framing, IDs, limits and peer checks.
- Per-operation authorization and a capability bootstrap design before exposing
  terminal content or control.
- Attach snapshot and bounded delta stream with forced-resync tests. Before the
  trusted UI exists, this stream is test-only behind an explicit development
  mode; it is not a public automation interface.

### Stage 2 — Omarchy-native terminal MVP

- Native Wayland client with one attached Splint.
- Trusted consent UI, grant/revoke flow, and visible control indication.
- Foot-level baseline input, clipboard, IME, scaling and damage rendering.
- Omarchy theme include/live apply, desktop metadata and `xdg-terminal-exec`.
- Arch package and systemd user lifecycle.
- Test on the current Omarchy reference system before broadening scope.

### Stage 3 — multiplexing

- Stable Lair/Dojo/Window/Splint persistence metadata.
- Splint-tree mutation, focus and controller ownership.
- Multiple windows, detach/reattach, bounded scrollback and search.
- Multiple observers and explicit control takeover.

### Stage 4 — headless access, automation and AI adapters

- Support headless `splinterd` under a systemd user service.
- Add an SSH-mediated stdio relay or Unix-socket forwarding workflow for hosts
  such as `neuromancer`; keep `splinterd` free of network listeners.
- Promote the previously internal authorization/grant path into a supported,
  documented third-party automation contract.
- Stable JSON/NDJSON CLI and published schemas.
- Capability policy management and audit inspection.
- Editor/client library after protocol stabilization.
- Optional read-mostly `splinterm-mcp` adapter.

Security foundations for Stage 4 must exist in Stage 1; Stage 4 is when the
surface becomes supported for third-party automation.

### Stage 5 — Nix and secondary feature depth

- Nix package/flake/checks and Home Manager module.
- Advanced terminal compatibility and renderer optimization.
- Other distributions only from maintainable release artifacts.

## 7. Decisions that still need spikes

The terminal foundation is not an open decision: it is the Rust port of Foot.
The remaining implementation questions are:

1. Start with a temporary Foot C PTY bridge, `portable-pty`, or a Linux-specific
   audited Rust PTY layer?
2. Port Foot's CPU shared-memory renderer first, then evaluate a GPU evolution,
   or maintain both behind one interface?
3. Which Rust font stack can reproduce Foot behavior on Omarchy?
4. Exact initial protocol frame limits and snapshot encoding?
5. Controller ownership granularity: Dojo, window or Splint?
6. Scrollback memory/disk policy and privacy defaults?
7. Stable reverse-DNS app ID and repository identity?
8. Which Foot configuration features are required in each compatibility
   milestone?
9. What does “persistent” promise across client loss, logout, daemon upgrade,
   daemon crash and host reboot?
10. For headless hosts, should the supported SSH path use Unix-socket forwarding,
    a stdio relay, or both?

## 8. Planning gates

Implementation planning should not freeze major dependencies until these gates
have evidence:

- a one-Splint parser/grid comparison against Foot;
- a stalled-client flow-control prototype;
- an Omarchy theme and `xdg-terminal-exec` end-to-end spike;
- a renderer/font bake-off on representative Omarchy hardware;
- a documented local-API threat model;
- clean Arch packaging of the selected native dependency stack;
- explicit recovery semantics reviewed in user-facing language.
