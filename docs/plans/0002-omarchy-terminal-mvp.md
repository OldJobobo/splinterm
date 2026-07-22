# Plan 0002: Omarchy-native Wayland terminal MVP

- **Status:** Complete (2026-07-20)
- **Roadmap:** Phase 2 Omarchy-native terminal MVP
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
- [x] Benchmark actual Wayland SHM slot release and resize churn in
  [Plan 0003 Slice 9](0003-phase8.1-closure-execution.md#slice-9--e2-performance-and-memory-baseline).
- [x] Record initial `fontdb`/Swash discovery, metrics, and fallback coverage
  evidence in [Spike 0003](../spikes/0003-font-stack-inventory.md).
- [x] Render, shape, cache, capture, and compare the first deterministic corpus
  against pinned Foot/fcft in
  [Spike 0004](../spikes/0004-deterministic-text-row-comparison.md).
- [x] Complete the initial shaping/raster comparison against Foot/fcft,
  including box-drawing geometry and measured same-pipeline fallback ink bounds.
- [x] Add deterministic Foot text-row renderer references.
- [x] Accept the Wayland/event-loop and font/renderer decisions in
  [ADR 0003](../adr/0003-wayland-client-and-event-loop.md) and
  [ADR 0004](../adr/0004-font-and-cpu-renderer.md).
- [x] Expose the accepted native renderer shell through `splinterm window`.
- [x] Render one owned daemon `TerminalSnapshot` in the native window and prove
  that closing it leaves the daemon-owned shell alive in
  [Spike 0006](../spikes/0006-static-daemon-snapshot-window.md).
- [x] Consume ordered full-snapshot subscription updates and resynchronize after
  sequence gaps or `ResyncRequired` in
  [Spike 0007](../spikes/0007-live-snapshot-subscription.md).
- [x] Add the first interactive keyboard/resize slice with a bounded command
  bridge, essential keys, repeat, and duplicate-suppressed grid resizing in
  [Spike 0008](../spikes/0008-essential-input-and-resize.md).
- [x] Add one exclusive connection-owned controller/size lease per live Splint
  with stale-incarnation checks and automatic release in
  [Spike 0009](../spikes/0009-controller-lease.md).
- [x] Add mode-aware xterm/Foot key encoding, xkb compose UTF-8, focus
  reporting, exact snapshot palette/default colors, and bracketed-paste
  framing in [Spike 0010](../spikes/0010-terminal-input-modes.md).
- [x] Validate the Phase 3 exit gate by launching `btop`, closing the graphical
  client, reopening the same daemon-owned process, and continuing input.
- [x] Add pointer selection, mouse reporting, regular/primary clipboard,
  bounded safe paste, URL hover, and gesture-only URL opening in
  [Spike 0012](../spikes/0012-pointer-selection-clipboard-urls.md).
- [x] Validate pointer lifecycle, local selection/primary publication, regular
  and primary paste, unsafe-control rejection, bracketed paste, mouse reports,
  and URL hover on workspace 8.
- [x] Add paired fractional-scale/viewport rendering, direct `text-input-v3`
  preedit/commit, inactive-IME compose fallback, focus indication, and reduced
  motion behavior in
  [Spike 0013](../spikes/0013-fractional-scale-ime-accessibility.md).
- [x] Validate live 1.25×/1.5×/2× output transitions in
  [Plan 0003 Slice 10](0003-phase8.1-closure-execution.md#slice-10--f1-hyprlandomarchy-sign-off)
  and active Fcitx5/Mozc `text-input-v3` preedit, candidate selection, PTY
  commit, and return to US input during Slice 11 closure.
- [x] Replace full-snapshot/full-frame updates with protocol v5 semantic damage,
  incremental row preparation, scroll-copy, row damage submission, frame
  callback coalescing, local cursor blink, and bounded scale-specific glyph
  caching in [Spike 0011](../spikes/0011-damage-driven-rendering.md).
- [x] Validate Phase 4 mechanisms on workspace 8 with idle CPU sampling,
  `btop`, finite plain/ANSI output bursts, rapid resize, and detach/reattach in
  [Spike 0011](../spikes/0011-damage-driven-rendering.md).
- [x] Replace normal graphical development grants with the daemon-launched,
  private-FD trusted consent broker, scoped grant-once authority, revocation,
  controller release, active authority indication, and bounded metadata audit
  described in [ADR 0005](../adr/0005-trusted-consent-broker.md) and
  [Spike 0014](../spikes/0014-trusted-consent-and-control.md).
- [x] Add stable application identity, safe `xdg-terminal-exec` command/cwd
  launch contracts, project-owned Omarchy theme generation and live reload,
  and the documented Foot configuration subset in
  [Spike 0015](../spikes/0015-omarchy-integration-and-configuration.md).
- [x] Record the full release renderer/daemon/graphical baseline with
  host/software context, output, resize, detach/reattach, idle, SHM, cache, and
  RSS metrics in [Plan 0003 Slice 9](0003-phase8.1-closure-execution.md#slice-9--e2-performance-and-memory-baseline).
- [x] Build a reproducible pre-compositor Foot/fcft oracle for all 95 printable
  ASCII characters, with exact face, metrics, bitmap, placement, and environment
  provenance, plus the 16-case final-buffer matrix closed by Plan 0003 Slice 1.
- [x] Replace approximate font/cell placement with an explicit Foot-derived cell
  geometry contract and verify ink clearance and terminal padding on all sides
  (closed by [Plan 0003 Slice 2](0003-phase8.1-closure-execution.md#slice-2--a4-four-sided-geometry)).
- [x] Make full-frame, row-damage, scroll-copy, cold-cache, and warm-cache paths
  produce equivalent final pixels for equivalent terminal state.
- [x] Implement a bounded graphical scrollback viewport, navigation, follow-live,
  unseen-output, selection, resize, clear-history, resync, and reattach behavior.
- [x] Amend ADR 0004 with exact tolerances, renderer-stack consequences,
  environment/cache/performance policy, and every intentional Foot divergence.

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

## Phase 8.1: Foot visual parity and graphical scrollback completion

Active closure runbook: [Plan 0003](0003-phase8.1-closure-execution.md).

Phase 8 integration exposed a release-blocking quality gap: the current renderer
is not yet close enough to Foot in final glyph pixels, cell placement, or
spacing, and the graphical client does not provide usable scrollback. Reopen
Phase 2 here. Do not hide the gap behind whole-row screenshots, permissive image
tolerances, or subjective review. Packaging may be prepared in parallel, but
Phase 2 cannot be declared complete until this subphase passes.

### 8.1.1 Establish a reproducible Foot render oracle

Extend `tools/foot-oracle/` so one command renders equivalent Foot and
Splinterm fixtures and emits machine-readable evidence. Pin and record:

- Foot commit, fcft version, fontconfig version/configuration, selected font
  file/index/hash, fallback chain, and relevant environment variables;
- font family/style/size, DPI, logical scale, buffer scale, fractional-scale
  rounding, hinting, antialiasing, subpixel mode, palette, grid, and padding;
- capture stage and pixel format, preferring pre-compositor ARGB buffers on both
  sides so compositor color management and decorations cannot affect results;
- glyph ID, selected face, advance, horizontal/vertical bearings, bitmap size,
  ascent, descent, line height, baseline, pen origin, cell origin, and logical
  and physical cell rectangles; and
- the pinned transcript, expected result, tolerance policy, and tool versions.

The oracle must fail loudly on unrecorded environment drift. It must generate a
summary, structured metrics, actual/reference crops, and mismatch heatmaps. Raw
hashes remain provenance only; they are not a parity assertion.

### 8.1.2 Cover every printable ASCII character

The required baseline is U+0020 through U+007E—all 95 printable ASCII
characters—not just `A` or the word `ASCII`. Exercise each character alone and
in rows that reveal placement and cumulative drift:

- the complete printable set in code-point order;
- repeated narrow and wide-looking forms such as `iiii`, `WWWW`, `....`,
  `____`, `||||`, and spaces;
- adjacency-sensitive punctuation, quotes, brackets, braces, slashes,
  backslashes, pipes, accents, and underscore;
- alternating pairs and every adjacent pair in the canonical ASCII row;
- 80- and 240-column repeated rows to detect cumulative pen drift; and
- leading/trailing spaces and glyphs touching each expected ink extreme.

Run the matrix for regular, bold, italic, and bold-italic at the supported
production font sizes, including the current default, and at 1×, 1.25×, 1.5×,
and 2×. Add dim, underline, double underline where supported, strike, undercurl,
reverse, conceal, cursor overlap, and neighboring wide/fallback glyph cases as
separate decoration/interaction fixtures. ASCII parity is the first hard gate;
Unicode, box drawing, Nerd Font glyphs, emoji, CJK, and combining sequences
retain their existing differential coverage and must not regress.

For every rendered cell record:

- final absolute ink bounds and bitmap dimensions;
- left, right, top, and bottom cell-to-ink clearance;
- baseline and decoration coordinates;
- alpha/RGB mismatch count, maximum channel error, mismatch bounding box, and
  structural/edge error; and
- selected face, glyph ID, cache key, and fallback decision.

### 8.1.3 Make cell and window geometry explicit

Introduce one reviewed `CellGeometry`/`CellMetrics` contract used by sizing,
hit-testing, cursor/IME rectangles, glyph placement, damage, and rendering. It
must define cell width/height, ascent/descent, baseline, letter spacing, line
height adjustment, pen rounding, and logical-to-buffer conversion. Replace
implicit centering or rounded-`M` assumptions unless the Foot oracle proves that
they are equivalent.

Model left, right, top, and bottom terminal padding independently. Specify how
odd residual pixels are distributed when a configured window is not an exact
multiple of the cell dimensions. Differentially test `cell_metrics`,
`SnapshotFrame::load_scaled`, initial sizing, `terminal_size`, `cell_at`, cursor
and IME rectangles, selection bounds, and damage rectangles. Matching raw glyph
masks is insufficient: the glyph's final absolute location inside the cell and
the cell's location inside the window must match.

Implementation must follow evidence:

1. determine whether mismatch originates in font selection, shaping, raster
   mask, metrics, pen placement, scaling, composition, or color conversion;
2. correct fontconfig/fallback and metric derivation before adding offsets;
3. remove unconditional cluster centering if it differs from Foot's pen model;
4. centralize integer/fractional rounding and prohibit independent call-site
   rounding; and
5. if Swash cannot meet the accepted mask gate, re-open ADR 0004 and evaluate a
   maintained fcft bridge or another rasterizer rather than accumulating
   per-glyph hacks.

No character-specific placement adjustment is acceptable. A character-specific
pixel tolerance is allowed only when documented with a reproducible cause and
approved in ADR 0004.

### 8.1.4 Add automated pixel and spacing gates

The comparator must align by explicit cell origins, never by best-fit image
translation. For each fixture it produces exact-match count, mismatch count,
maximum channel delta, mismatch bounds, edge-clearance deltas, and a heatmap.
Required gates on the pinned reference host are:

- exact cell, baseline, pen, padding, cursor, and decoration geometry;
- zero cumulative horizontal or vertical drift across long rows;
- no ink crossing an unintended cell edge and no clipping at any window edge;
- zero mismatched grayscale mask pixels for printable ASCII where Foot and
  Splinterm use the same face and raster contract;
- any nonzero color tolerance is explicit, minimal, fixture-specific, and
  justified by a documented color/pixel-format conversion;
- byte-identical output for equivalent full-frame and incremental-row paints;
- equivalent results with cold/warm glyph caches, after eviction, after scale
  changes, and after theme reload; and
- deterministic repeated runs on the same pinned environment.

Portable CI runs geometry, fixture-schema, comparison-tool, viewport, and
renderer-path tests. Host-sensitive raster tests run in a documented pinned
container/VM or on the reference Omarchy host and publish artifacts. CI must
report an explicit unsupported-environment skip; it must never silently update
references. Oracle reference updates require review of old/new metrics,
heatmaps, provenance, and an ADR/tolerance change when behavior changes.

### 8.1.5 Implement graphical scrollback as a viewport

The daemon remains the owner of canonical bounded history; the client owns only
viewport state. Replace the graphical client's zero-row attach request with a
configured bounded request and design paging if the current protocol snapshot
limit is too small for practical use. Do not increase a wire bound without
measuring frame size, authorization, resync, and memory consequences.

The client viewport must track:

- live-bottom versus history mode and offset from the newest logical row;
- available, returned, and omitted history counts;
- a stable top-row anchor while new output arrives;
- unseen-output state and an explicit return-to-live action; and
- clamping after history truncation, clear-history, resize/reflow, alternate
  screen transitions, process-incarnation changes, and resync.

When terminal mouse tracking is disabled, wheel input scrolls local history.
When tracking is enabled, preserve Foot-compatible mouse reports. Add and
document Foot-compatible keyboard navigation, including Shift+PageUp,
Shift+PageDown, and return-to-bottom behavior. Define precise behavior for the
alternate screen, `CSI 3 J`, application output while scrolled up, selection
and copy across the history/visible-grid boundary, URL hover, resize/reflow,
detach/reattach, history omission, and a history capacity of zero.

Rendering a historical viewport must not mutate daemon terminal state, PTY
size, cursor state, or the live semantic snapshot. Cursor and IME presentation
must be suppressed or transformed consistently while viewing history. New
output must continue to be consumed without forcing the viewport to jump.

### 8.1.6 Testing ladder and development sequence

Implement in reviewable vertical slices:

1. **Oracle inventory:** freeze provenance and make current mismatches visible
   without changing rendering.
2. **ASCII extraction:** add all-character metric/mask fixtures and comparator
   reports.
3. **Geometry contract:** centralize metrics, padding, scaling, and coordinate
   conversion with unit/property tests.
4. **Placement/raster correction:** fix one classified mismatch source at a
   time; regenerate reports but not references.
5. **Path equivalence:** compare full paint, semantic row damage, scroll-copy,
   cursor-only damage, cold/warm cache, eviction, and theme/scale invalidation.
6. **Viewport model:** implement pure scrollback arithmetic and state-machine
   tests before binding wheel/keyboard input.
7. **Protocol/render integration:** attach, page/resync if required, compose
   history plus live rows, then add selection and URL behavior.
8. **Wayland and end-to-end validation:** inject wheel/keyboard events and run
   real Foot/Splinterm scenarios on the reference host.
9. **Decision review:** amend ADR 0004, record intentional divergences, and
   obtain visual sign-off only after automated gates pass.

Required automated coverage:

- unit/property tests for metric rounding, residual padding, edge clearance,
  viewport offsets, anchors, clamping, trimming, and follow-live transitions;
- oracle differential tests for the complete ASCII/style/size/scale matrix;
- renderer integration tests proving all paint/cache/damage paths equivalent;
- protocol tests for zero, one, maximum, omitted, trimmed, cleared, stale, and
  resynchronized scrollback results;
- Wayland tests distinguishing local scrolling from application mouse reports;
- end-to-end tests that exceed history capacity, navigate and copy history,
  receive output while scrolled up, return live, clear history, resize/reflow,
  enter/leave alternate screen, detach, reattach, and force resync; and
- fuzz/property coverage for viewport arithmetic and bounded oracle image/metric
  parsing.

Every failure report must classify the first divergent layer: environment/font
selection, shaping, raster mask, metrics, placement/rounding, composition/color,
cache/damage, protocol history, viewport state, or input routing. Preserve the
smallest failing fixture and its artifacts. Do not tune a later layer to conceal
an earlier-layer mismatch.

### 8.1.7 Closure execution plan

The remaining work is organized as six dependency-ordered workstreams. Do not
run the final Hyprland sign-off until the deterministic renderer, history
identity, and CI gates are green. Each numbered item below closes one of the
known Phase 8.1 blockers.

#### Workstream A — authoritative final-buffer parity

##### A1. Final composited Foot-buffer comparison

**Purpose:** move from matching isolated glyph masks to matching the pixels that
are actually submitted for a terminal grid.

**Implementation:**

1. Extend the disposable pinned Foot patch with a test-only pre-submit dump at
   the completed render-target boundary. Emit width, height, stride, ARGB format,
   logical/buffer scale, grid dimensions, cell metrics, padding, cursor state,
   and frame identity beside the raw bytes.
2. Add a Splinterm exporter that renders the identical `TerminalSnapshot`
   through `SnapshotFrame` and `paint_snapshot` into a pre-Wayland ARGB buffer.
3. Define fixtures for the 95-character ASCII row, repeated narrow/wide forms,
   punctuation, leading/trailing spaces, cursor shapes, reverse video, dim, and
   edge cells. Include 80- and 240-column drift rows.
4. Add a strict ARGB comparator aligned by declared window and cell origins. It
   must report mismatch count, maximum channel delta, mismatch bounds, per-cell
   mismatch counts, edge-clearance deltas, and PNG/PPM heatmaps.
5. Compare the whole grid and independently crop every cell so a global failure
   can be reduced to the first divergent cell/layer.

**Acceptance:** default regular ASCII has exact cell/window geometry, no
cumulative drift, no clipped ink, and zero pixel mismatch except explicitly
recorded color-format conversions. Full-frame reference generation and
comparison run from one command without compositor screenshots.

**Files:** `tools/foot-oracle/patches/`, `tools/foot-oracle/`,
`crates/splinterm/src/renderer.rs`, a new final-buffer exporter, Spike 0016, and
ADR 0004.

##### A2. Bold/italic/bold-italic size and scale matrix

**Matrix:**

- faces: regular, bold, italic, bold italic;
- logical font sizes: 6, 12, current default 14, retained oracle profile 22, 32, 48, and 96 px;
- scales: 1×, 1.25×, 1.5×, and 2×;
- corpora: all printable ASCII, long drift rows, punctuation, combining text,
  box drawing, CJK, and fallback cases.

**Implementation:** parameterize fcft, isolated FreeType, production-cache, and
capture exporters by face pattern, logical size, and scale. Resolve styles from
the configured primary family rather than hard-coded JetBrains patterns. Record
face file/index/hash for every matrix cell. Run the complete matrix headlessly
for identity, metrics, masks, placement, fallback, and cache keys. Reserve
Wayland final-buffer runs for a reduced pairwise/boundary subset that proves
integration without replaying the entire Cartesian product.

**Acceptance:** every headless matrix cell passes exact face identity, metrics,
advance, cell placement, and grayscale alpha. Representative graphical boundary
cases confirm final composition. Color/fallback tolerances are explicit. No
style changes the terminal advance contract or causes cache identity reuse.

##### A3. Decorations

**Implementation sequence:**

1. Extend `splinterm-freetype` with safe owned decoration metrics matching
   fcft's baseline-relative underline and strike calculations.
2. Represent decoration spans separately from glyphs in `SnapshotFrame`, keyed
   by row, column range, color, style, position, and thickness.
3. Render single underline, double underline, strike, and undercurl through one
   shared row compositor used by full and incremental paths.
4. Clip decorations to their intended cell spans, including wide leaders,
   spacers, italic overhang, reverse/dim colors, and adjacent styled runs.
5. Lock every formula and scale-rounding boundary with exact vectors derived
   from pinned Foot source, then use a bounded graphical subset covering 1×, a
   fractional scale, a high integer scale, and both cursor focus presentations.

If the current wire attributes cannot distinguish single/double/curly
underline, extend terminal semantics and protocol explicitly; do not collapse
styles into one boolean while claiming parity.

**Acceptance:** source-derived baseline-relative coordinates, masks, ordering,
and thickness match Foot exactly; decorations do not leave gaps between adjacent
equal runs, cross unrelated cells, or differ between full and damaged-row
paints. Representative final-buffer cases prove integration without requiring
every source-derived formula at every graphical scale.

##### A4. Independent four-sided geometry

Replace symmetric `origin_x/origin_y` policy with reviewed types:

```text
CellGeometry { width, height, baseline, advance_rounding }
TerminalPadding { left, right, top, bottom }
WindowGeometry { cells, padding, residual_distribution, scale }
```

Use this contract for initial size, resize/grid calculations, hit-testing,
cursor and IME rectangles, selection, URL ranges, damage, final composition,
and captures. Define where odd residual pixels go instead of silently splitting
them symmetrically.

**Tests:** all four cell-to-ink clearances; all four window padding edges; zero,
asymmetric, odd, and oversized padding; edge glyphs; fractional scaling;
round-trip cell↔pixel conversion; 80/240-column drift.

**Acceptance:** no geometry call site independently rounds scale or derives
padding, and Foot differential reports zero unexplained edge delta.

#### Workstream B — renderer path invariants

##### B1. Scroll-copy, eviction, theme-reload, and cursor-only equivalence

Build a common test harness that starts from semantic state `S`, renders a clean
reference frame, then reaches the same state through each optimized path:

- full repaint;
- dirty-row repaint;
- forward/reverse scroll-copy plus exposed-row repaint;
- cursor-only movement/blink/style damage;
- glyph-cache cold, warm, forced eviction, and repopulation;
- scale invalidation and return to the original scale; and
- theme reload and return to the original palette.

Compare complete ARGB buffers byte-for-byte after each path. Add an explicit
raster-face budget/eviction policy or prove the finite face×scale bound and
report its memory separately.

**Acceptance:** equivalent semantic state always produces equivalent bytes;
optimized paths never retain stale cursor, decoration, theme, or copied pixels.
Cache metrics remain bounded and eviction cannot alter output.

#### Workstream C — durable scrollback identity and paging

##### C1. Stable daemon row identities

Content equality is not an identity. Replace overlap inference with daemon-owned
monotonic logical row IDs that survive ring movement but are invalidated when
resize/reflow creates new logical segmentation.

Add to snapshots/updates:

- history generation;
- oldest/newest available row ID;
- IDs on returned history rows; and
- explicit append, trim, clear, and reflow-generation transitions.

IDs must be scoped to Splint ID, process incarnation, and history generation.
Repeated identical rows must remain distinguishable. Clearing history or
incompatible reflow advances the generation and forces viewport/page reset.

**Acceptance:** property tests cover duplicate rows, wraparound, trimming,
clear, alternate screen, resize/reflow, stale incarnation, and revision gaps
without content-based anchor inference.

##### C2. Revision-bound paging beyond 16 rows

Add bounded protocol messages conceptually equivalent to:

```text
ScrollbackPageRequest {
  splint_id, incarnation, terminal_revision, history_generation,
  before_row_id, max_rows
}
ScrollbackPage {
  history_generation, oldest_available, newest_available,
  rows, has_older
}
```

Keep a small transfer bound per page and the existing frame-size limit. Reject
stale revision/generation/incarnation requests with an explicit resync response.
Authorize every page with `Observe + Scrollback`. The client page cache must be
row- and byte-bounded, deduplicate by row ID, and evict pages farthest from the
viewport without losing the anchor.

**Tests:** empty history; one row; exact page; multiple pages; repeated content;
concurrent append; ring trim between request/response; clear; stale generation;
malformed/oversized pages; cancellation; disconnect; resync; detach/reattach.

**Acceptance:** a user can navigate the configured history capacity rather than
only the newest 16 rows, while every request, response, cache, and allocation
remains bounded.

#### Workstream D — complete graphical history UX

##### D1. Visible unseen-output and return-to-live UI

Render a trusted, non-terminal-controlled overlay while detached showing:

- position/history status;
- bounded unseen-row count; and
- a clear return-to-live action.

Provide keyboard (`Shift+End`), mouse, and accessible action paths. Terminal
content must not be able to imitate the trusted indicator. Reduced-motion mode
must avoid attention-seeking animation.

**Acceptance:** new output does not jump a detached viewport; the indicator
updates without mutating daemon state; activation returns to live bottom,
clears unseen count, restores cursor/IME, and damages only required regions.

##### D2. Resize/reflow and page-boundary selection

Define policy before implementation:

- live resize follows daemon reflow and remains at bottom;
- detached resize anchors by stable logical row ID plus intra-row position;
- generation-changing reflow resets or remaps with an explicit user-visible
  outcome, never a silent wrong row;
- selection endpoints use row IDs and columns, not viewport array indices; and
- page eviction pins pages containing selection endpoints until copy completes
  or cancels selection explicitly.

Selection/copy and URL detection must work across loaded page boundaries without
logging terminal text. Alternate screen, clear-history, resync, and incarnation
change cancel or remap state according to documented rules.

**Acceptance:** deterministic tests cover grow/shrink, soft-wrap reflow, wide
cells, selection spanning three pages, output during selection, trim of an
endpoint, clear, resync, and reattach.

#### Workstream E — reproducibility, CI, and performance

##### E1. Pinned provenance and CI oracle

Create a machine-readable environment manifest containing:

- Foot/fcft commits and build options;
- FreeType/fontconfig versions and effective hinting/render policy;
- every font file/index/SHA-256 and fallback chain;
- logical size, DPI, scales, cell/padding policy, palette, and pixel format;
- Rust dependency versions; and
- relevant environment/configuration files.

The oracle must compare the live environment to the manifest and fail on drift.
CI must install or fetch checksum-pinned fonts, build the pinned oracle, run the
portable matrix subset, and upload summaries/heatmaps on failure. Jobs unable
to provide the pinned environment must emit an explicit named skip; they may not
silently pass or rewrite references.

**Acceptance:** a clean CI worker reproduces the required default parity result,
and reference updates require reviewed provenance plus old/new artifacts.

##### E2. Performance and memory baselines

Extend the Phase 4 benchmark to record, with host/software context:

- idle CPU and wakeups;
- 80×24 and 240×80 full and damaged frames;
- cold/warm/evicted glyph-cache latency;
- FreeType face creation and retained memory;
- continuous `yes` and large colored-file `cat`;
- output while detached deep in history;
- page fetch/cache/eviction latency and bytes;
- rapid resize/reflow;
- theme and scale invalidation; and
- detach/reattach snapshot/page restoration.

Define numeric regression budgets from the first accepted baseline. Measure
resident memory, SHM buffers, glyph bytes, raster faces, history pages, queue
high-water marks, and frame latency percentiles. A slow renderer must never
block daemon PTY consumption.

**Acceptance:** all stores have explicit bounds, no workload shows unbounded
growth, idle does not continuously redraw, and baseline artifacts are committed
or linked from the spike report.

#### Workstream F — real compositor validation

##### F1. Hyprland end-to-end sign-off

After A–E pass, run an isolated workspace-8 validation on the pinned Omarchy
host:

1. launch through `xdg-terminal-exec` and verify app ID/title;
2. render the final ASCII/style/scale fixtures and capture pre-submit evidence;
3. generate more history than one page and configured client cache capacity;
4. navigate with wheel and keyboard while mouse reporting is off;
5. enable application mouse tracking and verify wheel reports reach the app;
6. select/copy across pages, receive output while detached, and return live;
7. resize/reflow at 1×, 1.25×, 1.5×, and 2× where available;
8. clear history, enter/leave alternate screen, force resync, detach, and
   reattach;
9. run continuous output while sampling CPU, memory, frame pacing, and PTY
   responsiveness; and
10. close/reopen the client and confirm daemon-owned process continuity and
    resource cleanup.

Automate input/capture through a nested compositor where practical. Record the
exact commands, host manifest, logs without terminal bodies, benchmark output,
and reviewed screenshots/crops.

**Acceptance:** every scenario passes, no authority/privacy invariant regresses,
no unexplained visual differential remains, and the evidence is linked from the
Phase 8.1 exit record.

### Dependency order and review slices

Execute as buildable commits:

1. `Add final Foot buffer capture and comparator` (A1 foundation)
2. `Centralize cell and four-sided padding geometry` (A4)
3. `Render Foot-compatible terminal decorations` (A3)
4. `Run style size scale and drift matrix` (A2 + A1)
5. `Prove optimized renderer path equivalence` (B1)
6. `Add stable scrollback row identities` (C1)
7. `Add bounded revision-bound history paging` (C2)
8. `Add trusted detached-history status controls` (D1)
9. `Anchor reflow and selection across pages` (D2)
10. `Pin oracle environment and enable CI artifacts` (E1)
11. `Record renderer and history performance baselines` (E2)
12. `Validate Phase 8.1 under Hyprland` (F1)

A1/A4 and C1 can proceed in parallel. A2 depends on A1, A3, and A4. B1 depends
on the final compositor. C2 depends on C1. D1 depends on C2's viewport/cache
contract; D2 depends on C1 and C2. E1 begins with A1 and becomes required for
A2. E2 runs after B1 and C2. F1 is last.

For every slice, require formatting, strict Clippy, workspace tests, focused
property/differential tests, bounded malformed-input tests, updated provenance,
and no silent fixture regeneration.

### 8.1 exit gate

Phase 8.1 is complete and Phase 2 may proceed to packaging/release validation because:

- all 95 printable ASCII characters pass final-buffer pixel and placement gates
  at the pinned default configuration;
- the required style, size, and scale matrix passes its documented tolerances;
- all four cell-to-ink edges and all four terminal padding edges are measured;
- 80- and 240-column fixtures show no cumulative spacing drift;
- full and incremental renderer paths are equivalent and cache state does not
  alter final pixels;
- graphical scrollback navigation, anchoring, follow-live, selection, clearing,
  resize, alternate-screen, resync, and reattach behavior pass automated tests;
- oracle generation and mismatch reporting are reproducible with one documented
  command and publish reviewable artifacts;
- performance and memory remain bounded under continuous output while scrolled
  up and while generating the full parity matrix; and
- ADR 0004 names exact tolerances, environment constraints, renderer-stack
  consequences, and all intentional divergences from pinned Foot.

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

**Phase 9 status:** Complete. The private local `0.1.0.pre-1` Arch package owns
release client/daemon/PTY-helper binaries, on-demand user service, desktop and
AppStream metadata, icon, xdg launcher, theme generator, examples, and license
notices. It neither mutates user homes nor installs under `/usr/share/omarchy`.
The committed-source `makepkg` build ran workspace tests; isolated validation
covered paths/modes/dependencies, theme generation, protocol-mismatch restart,
client/daemon negotiation, and graceful socket cleanup. The package was not
installed or published. Evidence: [`artifacts/0018-packaging`](../spikes/artifacts/0018-packaging/README.md).

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
- unsupported Foot/config behavior is listed explicitly;
- Phase 8.1 ASCII pixel/placement, edge-spacing, renderer-path, and graphical
  scrollback gates pass against the pinned Foot oracle; and
- workspace formatting, strict Clippy, tests, visual fixtures, and package
  checks pass.

## Closure

Roadmap Phase 2 is complete. Phase 8.1 closed exact renderer/scrollback parity
and guarded Hyprland/Omarchy behavior; Phase 9 closed private package layout,
build, upgrade handling, documentation, and isolated release validation. Live
installation and public/AUR publishing remain explicit post-closure choices,
not prerequisites for the private prerelease milestone.
