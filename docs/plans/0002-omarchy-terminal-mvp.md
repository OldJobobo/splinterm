# Plan 0002: Omarchy-native Wayland terminal MVP

- **Status:** Phase 0 in progress — native Wayland/SHM spike successful
- **Roadmap:** Phase 2
- **Foundation:** [Plan 0001](0001-terminal-kernel.md), [ADR 0001](../adr/0001-foot-rust-port.md)
- **Reference source:** Foot 1.27.0, commit
  `3c5b584b0eafa772eb4376fb6eaf6643399e190e`

## Execution progress

- [x] Audit the installed Wayland, xkbcommon, fontconfig, and FreeType baseline.
- [x] Add a safe Rust native xdg-shell/SHM mechanism spike.
- [x] Validate the first genuine Splinterm Wayland window under Hyprland on
  workspace 8 with a stable provisional app ID.
- [x] Exercise configure, frame callback, SHM buffer reuse, resize, output,
  scale, seat, keyboard, and clean-close mechanisms in the spike.
- [x] Record initial evidence in
  [Spike 0001](../spikes/0001-native-wayland-window.md).
- [x] Record the initial allocate-vs-reuse CPU canvas baseline at 80×24,
  120×40, and 240×80 equivalents in
  [Spike 0002](../spikes/0002-cpu-shm-paint-baseline.md).
- [ ] Benchmark actual Wayland SHM slot release and resize churn.
- [x] Record initial `fontdb`/Swash discovery, metrics, and fallback coverage
  evidence in [Spike 0003](../spikes/0003-font-stack-inventory.md).
- [x] Render, shape, cache, capture, and compare the first deterministic corpus
  against pinned Foot/fcft in
  [Spike 0004](../spikes/0004-deterministic-text-row-comparison.md).
- [ ] Complete shaping/raster visual comparisons against Foot/fcft, including
  box-drawing geometry and measured fallback ink bounds.
- [x] Add deterministic Foot text-row renderer references.
- [ ] Accept Wayland/event-loop and font/renderer ADRs.
- [ ] Promote the native window shell into the graphical `splinterm` client.

## Goal

Turn the completed headless one-Splint service into the first real Splinterm
terminal application:

1. `splinterd` continues to own the shell, PTY, canonical terminal state, and
   persistence boundary;
2. `splinterm` becomes a disposable native Wayland client under Hyprland;
3. the client renders one attached Splint, accepts keyboard input, owns the
   visible size, and can disappear without ending the shell; and
4. the result behaves like an Omarchy-native terminal rather than a generic
   demo window.

The first usable slice must arrive early. Do not wait for clipboard, IME, theme
migration, or packaging before opening a native window containing a live shell.
Those features complete the MVP after the basic window/render/input loop works.

## User-visible definition of done

Roadmap Phase 2 is complete when a user can:

- launch `splinterm` through `xdg-terminal-exec` on Omarchy;
- see a native Wayland window with a stable Splinterm app ID;
- create or attach to one daemon-owned shell Splint;
- type normal shell commands with correct modifiers and repeat behavior;
- see Unicode, wide characters, combining characters, ANSI colors, cursor
  movement, erase operations, scrollback output, and terminal titles rendered;
- resize, maximize, fullscreen, change output scale, and move between monitors
  without grid/PTY divergence;
- copy and paste through the clipboard and primary selection;
- enter basic composed text through the active Wayland input method;
- open detected URLs through an explicit user gesture;
- grant, observe, and revoke terminal read/control access through trusted UI;
- close and reopen the graphical client without terminating the shell;
- receive the current snapshot after reconnect and continue with ordered
  updates; and
- follow the active Omarchy palette and live theme changes.

The milestone must be demonstrated using the Splinterm window itself. Foot may
remain an oracle or debugging presenter, but it cannot be used as the product
window when claiming this phase complete.

## Non-goals

This milestone deliberately excludes:

- multiple Splints, split-tree editing, and focus navigation;
- multiple graphical windows per Dojo;
- durable scrollback across daemon restart;
- search, selection history, and advanced URL modes;
- sixels or other terminal image protocols;
- GPU rendering as a requirement;
- supported third-party automation or MCP access;
- SSH relay and remote graphical operation;
- full Foot configuration-file compatibility;
- Nix, Flatpak, Snap, or other tertiary packaging; and
- exact process restoration after daemon or host restart.

These remain Roadmap Phases 3–5 or later compatibility work.

## Architectural invariants

### Daemon remains headless

`splinterd` must not gain Wayland, font, renderer, clipboard, or IME
dependencies. It remains usable without a graphical session.

### Client remains disposable

Wayland objects, shared-memory buffers, glyph caches, input-method state, and
clipboard offers belong to `splinterm`. A client crash or compositor restart
must not terminate the shell or corrupt canonical terminal state.

### Terminal semantics remain renderer-independent

`splinterm-terminal` must not gain Wayland, font, shaping, raster, texture,
shared-memory, or protocol dependencies. Renderer caches are derived data.

### Wire DTOs remain separate

The graphical client consumes explicit protocol snapshots and updates. It must
not serialize terminal internals or depend on daemon runtime structs.

### Foot remains authoritative

Foot defines baseline behavior for font selection, cell placement, cursor
rendering, damage, keyboard mapping, mouse behavior, clipboard, scaling, IME,
Wayland lifecycle, and configuration semantics. Rust libraries may provide
mechanisms; they do not silently replace Foot behavior.

### Unsafe remains isolated

First-party crates keep `unsafe_code = "forbid"`. Any unavoidable native bridge
requires a dedicated ADR, a narrowly scoped crate, provenance, tests, and a
removal or maintenance strategy. No renderer convenience is sufficient reason
to spread unsafe code through the client.

## Proposed client decomposition

Keep platform and rendering concerns out of the current CLI entry point:

```text
crates/
├── splinterm/                 # command selection and graphical application
├── splinterm-client/          # attach/session/controller state machine
├── splinterm-render/          # CPU renderer, glyph cache, damage composition
└── splinterm-wayland/         # Wayland objects, seats, outputs, clipboard, IME
```

The exact crate split is a Phase 0 spike. At minimum, preserve these logical
boundaries even if the first implementation uses modules inside `splinterm`.
Do not create public crates merely to make the workspace look architectural.

Expected ownership:

```text
GraphicalApp
├── WaylandConnection
├── WindowSurface
├── SeatState
├── InputMethodState
├── ControllerLease
├── AttachedSplint
│   ├── SplintId
│   ├── ProcessIncarnation
│   ├── TerminalRevision
│   └── client-owned semantic view
├── Renderer
│   ├── FontSet
│   ├── GlyphCache
│   ├── DamageAccumulator
│   └── SHM buffer pool
└── ThemeState
```

## Foot source map

Port observable behavior from the pinned Foot source rather than reconstructing
terminal presentation from general Wayland tutorials.

| Area | Primary Foot source |
| --- | --- |
| Wayland registry, outputs, surfaces | `wayland.c`, `wayland.h` |
| xdg-shell window lifecycle | `wayland.c`, `main.c` |
| shared-memory buffers | `shm.c`, `shm.h` |
| renderer and damage | `render.c`, `render.h` |
| fonts and glyph lookup | `fonts.c`, `fcft` integration in `render.c` |
| cursor rendering | `render.c`, `terminal.c` |
| keyboard and xkb state | `input.c`, `input.h`, `key-binding.c` |
| mouse and pointer state | `input.c`, `selection.c`, `url-mode.c` |
| clipboard and primary selection | `wayland.c`, `input.c`, `selection.c` |
| text input / IME | `ime.c`, `wayland.c` |
| scaling and viewport behavior | `wayland.c`, `render.c` |
| app ID, title, activation | `main.c`, `wayland.c`, `client.c` |
| configuration semantics | `config.c`, `config.h`, `foot.ini` |

Translated modules and behavior fixtures must record the Foot files and pinned
revision in module documentation and `THIRD_PARTY.md`.

## Phase 0: dependency and behavior spikes

Do not begin a broad renderer port before these focused spikes have evidence.

### 0.1 Wayland client stack

Evaluate a small project-owned interface over the current Rust Wayland
libraries. The spike must prove:

- registry binding without generated first-party unsafe code;
- `xdg_wm_base` ping/pong;
- one `xdg_surface`/`xdg_toplevel`;
- configure/ack ordering;
- frame callbacks;
- `wl_shm` buffer creation and release;
- seat and keyboard discovery;
- output enter/leave and scale events;
- clean compositor disconnect; and
- operation under the installed Hyprland version.

Likely infrastructure includes `wayland-client`, `wayland-protocols`, and either
`calloop` or an equally explicit event loop. Evaluate Smithay Client Toolkit as
infrastructure, not as permission to lose Foot lifecycle behavior. Record the
selected stack and event-loop ownership in an ADR before its types spread
through public APIs.

### 0.2 CPU renderer strategy

Start with Foot's CPU/shared-memory model. The spike must draw deterministic
rectangles and text into a reusable SHM buffer pool and measure:

- full-window fill throughput;
- 80×24, 120×40, and 240×80 redraw time;
- buffer allocation and reuse;
- frame callback pacing;
- damage submission;
- fractional-scale buffer dimensions; and
- resize churn under Hyprland.

GPU rendering may be evaluated after parity and profiling. It is not on the
critical path to the first real window.

### 0.3 Font stack bake-off

Compare candidate Rust mechanisms against Foot/fcft on the current Omarchy
system. The corpus must include:

- ASCII and box drawing;
- Nerd Font/private-use glyphs;
- emoji with fallback;
- CJK wide characters;
- combining marks and composed sequences;
- bold, italic, bold-italic, dim, underline, strike, and undercurl;
- missing-glyph fallback;
- synthetic style policy;
- font size and DPI changes; and
- mixed-script baseline/cell placement.

Evaluate font discovery, fallback, shaping, rasterization, and cache ownership
as separate concerns. Candidate mechanisms may include fontconfig-backed
discovery plus Rust shaping/raster libraries. Do not select a stack from API
convenience alone. Record visual diffs, licenses, MSRV, binary size, and frame
cost in an ADR.

### 0.4 Renderer oracle

Add deterministic reference captures from pinned Foot for representative cell
runs. Prefer semantic placement and pixel-region comparisons over whole-window
screenshots where compositor decorations would add noise. Store:

- input transcript;
- font/config identity;
- logical grid and scale;
- expected cell positions and attributes;
- cropped reference image or hash;
- tolerance and known platform variance; and
- Foot revision.

**Phase 0 exit gate:** selected Wayland/event-loop and font/renderer decisions
are recorded, dependencies are license-audited, and one native spike window can
present a deterministic text row on workspace 8.

## Phase 1: native window shell

Create the first actual Splinterm window before terminal rendering is complete.

Deliver:

- Wayland connection and registry lifecycle;
- xdg-shell toplevel with stable provisional app ID;
- title updates;
- close, maximize, fullscreen, and compositor configure handling;
- SHM buffer pool with release tracking;
- solid-color frame rendering;
- frame callback pacing;
- output/scale tracking;
- clean reconnect failure behavior; and
- a demo-local launch path targeting an empty workspace 8.

The window must identify itself as Splinterm and must not be hosted inside Foot,
another terminal, GTK, Qt, Electron, or a browser.

**Exit gate:** `hyprctl clients` reports the Splinterm app ID, the native window
survives resize/maximize/fullscreen, and repeated open/close cycles leave no
buffers or client tasks behind.

## Phase 2: attached semantic snapshot renderer

Attach the native client to the Phase 1 daemon through the secure protocol.

Deliver:

- graphical hello/version negotiation;
- trusted development or user-consent authorization path;
- current Splint/process-incarnation selection;
- atomic attach returning snapshot revision `R`;
- client-owned semantic screen representation;
- full visible-grid rendering from the wire snapshot;
- background, foreground, palette, reverse, conceal, bold, dim, italic,
  underline, strike, and spacer/wide-cell handling;
- cursor shape/visibility/blink baseline;
- terminal title propagation;
- explicit resnapshot after gaps or incarnation changes; and
- detached/error/exit visual states.

Do not request a snapshot for every rendered frame. The client maintains an
owned semantic view and renders from that stable local state.

The current Phase 1 wire stream sends owned snapshots for updates. During this
phase, define damage/update DTOs capable of carrying changed semantic rows and
metadata without full-screen retransmission. Keep protocol conversion in the
daemon/protocol boundary.

**Exit gate:** a real shell transcript, ANSI color fixture, Unicode fixture, and
cursor-motion fixture render in the native window and match semantic state from
the daemon.

## Phase 3: keyboard input and resize ownership

Port Foot's keyboard behavior and establish one explicit graphical controller.

Deliver:

- Wayland seat capability tracking;
- xkb keymap, state, modifiers, compose, and repeat;
- Foot-derived key-to-terminal sequence mapping;
- normal text input without shell-string construction;
- control, alt/meta, shift, function, navigation, keypad, and application modes;
- bracketed paste distinction;
- focus enter/leave behavior;
- one controller/size-owner lease per live Splint;
- explicit controller indication in the window;
- configure-size → cell-grid calculation;
- ordered daemon PTY/grid resize;
- minimum grid size and resize debounce policy; and
- stale-incarnation/controller rejection.

Authorization is checked when the controller lease is granted and immediately
before input/resize operations. Disconnect releases the lease but does not end
the shell.

**Exit gate:** the user can work interactively in a shell, run a full-screen
TUI, resize it, close the window, reopen it, and continue the same process.

## Phase 4: damage-driven rendering and performance baseline

Replace full redraw/update behavior with bounded damage flow.

Deliver:

- wire updates containing changed rows/cells and semantic metadata;
- client revision continuity checks;
- damage coalescing;
- cursor-only and title-only updates;
- scroll-copy optimization where correct;
- frame callback throttling;
- reusable SHM buffers;
- glyph cache budgets and eviction;
- output-scale-specific caches;
- blink timers that do not mutate daemon terminal state;
- stalled compositor/frame behavior; and
- renderer memory/latency metrics.

Record baselines for:

- idle CPU;
- continuous `yes` output;
- `cat` of a large colored file;
- 80×24 and 240×80 redraw;
- rapid resize;
- glyph-cache cold/warm frames; and
- detach/reattach full snapshot.

A slow renderer must not backpressure daemon PTY consumption. Overflow forces a
fresh snapshot through the established resync path.

## Phase 5: pointer, selection, clipboard, and URLs

Add the minimum daily-use pointer workflows.

Deliver:

- pointer enter/leave/motion/button/axis handling;
- Foot-compatible terminal mouse reporting for implemented modes;
- local text selection independent of daemon renderer state;
- regular clipboard copy/paste;
- primary selection on supported compositors;
- bounded offer reads and MIME filtering;
- bracketed paste handling;
- explicit confirmation/policy for unsafe control characters where required;
- URL metadata or detection sufficient for visible hover/open behavior; and
- user-gesture-only URL launching.

Clipboard contents and selected terminal text must never be logged. Clipboard
read/write remain separate authorization scopes for automation; normal local UI
interaction does not silently grant those scopes to other clients.

## Phase 6: scaling, IME, and accessibility baseline

Deliver:

- integer and fractional output scaling;
- correct logical/buffer coordinate conversion;
- monitor movement and scale-change rerasterization;
- viewport/fractional-scale protocol use where supported;
- basic `text-input-v3` activation and surrounding/cursor rectangle updates;
- preedit and commit display;
- compose fallback when no IME is active;
- focus-safe IME enable/disable;
- high-contrast cursor/focus indication;
- reduced-motion handling for blink where configured; and
- semantic labels for trusted consent/control UI where the chosen toolkit makes
  them available.

Test at 1×, 1.25×, 1.5×, and 2× where the compositor/output setup permits.
Pixel comparisons must account for the selected rasterizer's documented scale
variance.

## Phase 7: trusted consent and control UI

Replace the environment-variable development grant as the normal graphical
workflow.

Deliver:

- visible local prompt identifying the requesting client and requested scopes;
- grant-once, deny, revoke, and controller-release actions;
- persistent policy only after an explicit product/security decision;
- always-visible indication while another client can read or control a Splint;
- distinction between observe, scrollback, input, resize, clipboard, and
  terminate scopes;
- capability binding to peer identity, Splint ID, and process incarnation;
- revocation propagation to active subscriptions/controllers;
- bounded audit metadata without terminal bodies or input bytes; and
- development-mode labeling that cannot be mistaken for supported automation.

The consent surface must be rendered by trusted Splinterm UI, not by terminal
content that an application can spoof.

## Phase 8: Omarchy integration and configuration

### Application identity

Select and document the stable reverse-DNS app ID before packaging. Apply it
consistently to:

- Wayland app ID;
- desktop file name;
- icon name;
- systemd unit names;
- notification identity; and
- packaging metadata.

### `xdg-terminal-exec`

Implement and test:

- normal terminal launch;
- execute-command arguments without shell interpolation;
- working-directory argument contract;
- desktop entry `X-TerminalArgExec` and `X-TerminalArgDir` behavior;
- Omarchy terminal priority/list integration; and
- fallback/error behavior when the daemon is unavailable.

### Theme integration

Start with project-owned theme/config inputs rather than editing live Omarchy
files during development. Deliver:

- Omarchy palette role mapping;
- background/foreground, ANSI 16, cursor, selection, URL, and UI colors;
- live theme re-read/apply without restarting the shell;
- theme change damage invalidation;
- safe fallback palette; and
- a documented generated-config/include path.

### Configuration migration

Define an explicit compatibility subset for this MVP:

- font family/size;
- initial window size;
- shell command and login-shell behavior;
- title/app ID controls where appropriate;
- colors/palette;
- cursor style/blink;
- scrollback size;
- key bindings implemented by this phase; and
- resize/dpi behavior.

Do not claim arbitrary `foot.ini` compatibility. Provide diagnostics for
unsupported keys and a migration document referencing the pinned Foot baseline.

## Phase 9: packaging and release validation

Deliver:

- Arch `PKGBUILD` or repository packaging path;
- desktop entry and icon;
- `xdg-terminal-exec` registration;
- optional systemd user service/socket lifecycle consistent with headless use;
- dependency/license manifest updates;
- release build with reproducible feature selection;
- crash-safe cleanup and daemon/client upgrade mismatch handling;
- Omarchy install/uninstall/upgrade notes; and
- no modifications under `/usr/share/omarchy/` outside package-managed install
  artifacts.

## Test strategy

### Unit tests

- damage coalescing and rectangle clipping;
- grid-to-pixel coordinate conversion;
- scale and cell-size rounding;
- glyph-cache keys and eviction;
- palette/style resolution;
- key/modifier/mode sequence mapping;
- controller lease transitions;
- clipboard MIME and size policy;
- configuration parsing/migration; and
- protocol snapshot/update application.

### Foot differential tests

Compare:

- glyph/cell placement;
- cursor geometry;
- underline/undercurl/strike placement;
- fallback font choice for the accepted corpus;
- key sequences across terminal modes;
- mouse reports;
- clipboard and primary-selection behavior;
- scale and resize grid calculations; and
- title/app ID/config semantics.

Every fixture records the Foot commit and any intentional divergence.

### Headless compositor tests

Use a nested/headless Wayland compositor where practical to automate:

- window creation/configure/close;
- SHM buffer lifecycle;
- frame callbacks and damage;
- keyboard/pointer injection;
- output scale changes;
- clipboard offers;
- IME protocol state; and
- screenshot crops.

Hyprland workspace 8 remains the human visual review target. Automated tests
must not depend exclusively on the user's live compositor.

### End-to-end scenarios

1. Start isolated `splinterd` and graphical client.
2. Create one shell and render prompt.
3. Type `printf`, `pwd`, ANSI, Unicode, wide, combining, and cursor fixtures.
4. Resize repeatedly and verify PTY/grid/window agreement.
5. Run a TUI and verify input modes.
6. Copy, paste, primary-select, and open a URL through user gestures.
7. Exercise IME preedit/commit and output scaling.
8. Close the graphical client while output continues.
9. Reopen and verify current snapshot plus ordered updates.
10. Stall rendering, force resync, and verify the shell remains current.
11. Grant and revoke an observer/controller through trusted UI.
12. Change the Omarchy theme and verify live palette/render refresh.
13. Launch through `xdg-terminal-exec` with command and cwd arguments.
14. Terminate explicitly and verify client, daemon, buffer, and process cleanup.

## Performance and memory gates

Before calling the MVP usable:

- idle client does not continuously redraw;
- cursor/text blink uses bounded timers and damage;
- renderer memory is bounded by configured buffers and glyph-cache budgets;
- resize does not allocate an unbounded buffer backlog;
- slow rendering never blocks daemon PTY reads;
- full snapshot application is bounded and measured;
- 80×24 interactive latency feels immediate on the reference Omarchy machine;
- 240×80 continuous output remains responsive; and
- client close releases Wayland buffers, font caches, subscriptions, and
  controller leases without ending the shell.

Record numbers and hardware/software context rather than using “fast” as an
acceptance criterion.

## Security and privacy gates

- terminal access remains denied by default without trusted grant;
- peer UID remains necessary but insufficient for sensitive scopes;
- the graphical client cannot bypass daemon authorization;
- input and resize require the current controller lease;
- clipboard contents, terminal bodies, and input bytes are never logged;
- URL launching requires an explicit local gesture;
- wire frames, offers, snapshots, updates, image dimensions, and caches remain
  bounded;
- untrusted terminal content cannot draw or imitate trusted consent chrome;
- stale process incarnations cannot receive input or controller authority; and
- disconnect/revoke releases client authority without terminating the shell.

## Documentation deliverables

- ADR: Wayland/event-loop and renderer architecture;
- ADR: font discovery/shaping/raster stack;
- ADR: controller lease and trusted consent model;
- app ID and `xdg-terminal-exec` contract;
- supported Foot configuration subset and migration guide;
- renderer/font benchmark report;
- visual parity fixture documentation;
- Omarchy theme integration documentation;
- packaging/install/uninstall guide; and
- updated architecture and third-party provenance.

## Suggested implementation commits

Keep review units narrow and leave each commit buildable:

1. `Record Wayland and font renderer spike results`
2. `Add native Splinterm Wayland window shell`
3. `Add SHM buffer pool and frame pacing`
4. `Render attached semantic terminal snapshots`
5. `Add graphical controller lease protocol`
6. `Port Foot keyboard input and resize behavior`
7. `Add row damage updates and glyph cache budgets`
8. `Add pointer selection and clipboard behavior`
9. `Add scaling and basic text input v3`
10. `Add trusted terminal access consent UI`
11. `Add Omarchy theme and xdg-terminal-exec integration`
12. `Package and validate the Omarchy terminal MVP`

## Review gates

Do not call Roadmap Phase 2 complete until:

- a native Splinterm Wayland window—not Foot—renders a live shell;
- dependency and font/renderer decisions have ADRs and license audits;
- keyboard and resize ownership pass interactive and automated tests;
- client close/reopen preserves the daemon-owned shell;
- snapshot/update gaps resynchronize without blocking PTY consumption;
- clipboard, primary selection, scaling, and basic IME work under Hyprland;
- trusted grant/revoke/control indication replaces normal reliance on the
  development environment variable;
- Omarchy theme changes apply live;
- `xdg-terminal-exec` command/cwd launch contracts pass;
- Arch packaging installs coherent desktop metadata and helper binaries;
- unsupported Foot/config behavior is listed explicitly; and
- workspace formatting, strict Clippy, tests, visual fixtures, and package
  checks pass.

## Immediate next task

Begin Phase 0 with three parallel evidence spikes:

1. open a native xdg-shell window and cycle SHM buffers under Hyprland;
2. render a deterministic monospace text row through candidate font stacks and
   compare it with pinned Foot; and
3. draft the Wayland/event-loop and font/renderer ADRs from measured results.

Do not begin broad keyboard, clipboard, or configuration work until the first
native window and font/renderer choices have passed review.
