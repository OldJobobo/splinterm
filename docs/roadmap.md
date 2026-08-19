# Engineering roadmap

> This roadmap records completed implementation phases and the dependency-ordered
> delivery path from public beta to a supported stable release. It is an
> engineering completion ledger and forward plan, not a product strategy,
> compatibility promise, or release date. Product direction is authoritative in
> the [`product roadmap`](product-roadmap.md); current maturity and validated
> scope are authoritative in [`status.md`](status.md);
> [`pre-planning-research.md`](pre-planning-research.md) preserves historical
> pre-implementation reasoning.

## Alpha3 and Beta 1 delivery record

Sections 0–2 and 5 below preserve the dependency order used to advance from
`v0.1.0-alpha2` through `v0.1.0-beta1`. Plans 0032–0036 and the Beta 1
performance gate are complete; their imperative wording records the accepted
plan rather than pending work. Sections 3–4 retain scoped post-alpha proposals
and remain subject to product decisions. Current release state remains
authoritative in [`status.md`](status.md).

### 0 — alpha3 release and planning hygiene (complete)

- Keep release claims aligned across the repository, website, AUR recipes, and
  immutable tags.
- Close the remaining packaged Dojo-picker acceptance item without conflating
  publication with installed graphical validation.
- Complete the bounded command-palette and keymap closure in
  [Plan 0033](plans/0033-alpha3-command-palette-and-keymap-closure.md) before
  constructing the `0.1.0-alpha3` candidate.
- Complete the explicit saved-workspace workflow in
  [Plan 0034](plans/0034-alpha3-saved-lair-layouts.md): saved Lairs retain Dojo
  trees, proportional split ratios, known structured launch recipes, launch
  working directories, and bounded geometry hints without checkpointing process
  state or automatically executing commands.
- Complete [Plan 0035](plans/0035-alpha3-scrollback-enter-safety.md): Enter while
  the focused Splint is in historical scrollback returns it to live output like
  `Shift+End`, consumes that physical key through release, and sends no PTY input.
- Complete [Plan 0036](plans/0036-alpha3-wayland-file-drop-path-insertion.md):
  bounded local regular-file drops insert deterministic shell-escaped paths into
  one exact controlled Splint without reading file bodies or submitting input.
- Reconcile Cargo, package, AUR, `.SRCINFO`, README, status, and release-note
  versions in one coherent alpha3 release-state commit.
- Construct the private candidate once, review its exact retained artifacts,
  and exercise promotion only to the protected approval pause unless publication
  receives separate explicit authorization.
- Finish Plan 0026's local website content and build/link validation before any
  separately approved preview or production deployment.
- Keep completed-plan closure facts separate from later release follow-ups.

### 1 — near-term correctness and Omarchy integration (complete)

1. Preserve exact theme-provided selection roles with an opaque selection
   background and separately repainted glyph and decoration foregrounds; do not
   derive, blend, or channel-swap the resolved theme colors.
2. Complete [Plan 0032](plans/0032-omarchy-screensaver-integration.md) through
   the Splinterm-owned package helper, explicit non-deleting user activation,
   non-graphical validation, and separately approved guarded graphical
   acceptance; no Omarchy source change or upstream submission is required.
3. Execute [Plan 0033](plans/0033-alpha3-command-palette-and-keymap-closure.md):
   close the `dojo.close-other-tabs` binding defect, add the bounded everyday
   command set, share safe `Super+C/V` terminal aliases and context-local
   `Super+C/V/X/Z` editing across built-in profiles, prove keymaps end to end,
   prevent registry drift, reconcile documentation, and record packaged
   graphical acceptance.
4. Execute [Plan 0035](plans/0035-alpha3-scrollback-enter-safety.md) so plain and
   keypad Enter return a historical focused Splint to live output without
   sending input; preserve modal precedence and normal live-view Enter behavior.

The screensaver work depends on the released transient XDG launch foundation in
[Plan 0029](plans/0029-transient-xdg-command-launches.md). Plan 0033 depends on
accepted [Plan 0025](plans/0025-command-palette-and-tab-context-menus.md) and
[Plan 0027](plans/0027-configurable-keymaps-and-dojo-presets.md). Keeping their
non-graphical milestones and guarded acceptance cases explicit prevents either
slice from becoming an unbounded UI redesign.

### 2 — lifecycle and desktop workflow (complete)

1. Execute [Plan 0034](plans/0034-alpha3-saved-lair-layouts.md) as the alpha3
   lifecycle slice: define Live, Detached, Saved, Restorable, Pinned, and
   Disposable states; expose exact-target save/preview/restore controls; and
   preserve existing explicit-execution and privacy boundaries.
2. Retain tree shape, split axes and ratios, default focus, Splint names, known
   structured launch argv, launch working directories, and bounded geometry
   hints. Do not infer interactive foreground applications or claim process,
   shell, editor, or terminal-body checkpointing.
3. Complete Plan 0034's bounded retirement, confirmed deletion, capacity, and
   picker-state requirements; keep cloning and reusable templates deferred.
4. Execute [Plan 0036](plans/0036-alpha3-wayland-file-drop-path-insertion.md) as
   the Alpha3 desktop-input slice: accept bounded local `text/uri-list` regular
   files, preserve exact targeting and bracketed paste, insert no submission
   bytes, and retain body-free diagnostics.
5. Keep clipboard-image saving, directories, remote transfer, private
   file-manager MIME types, and drag-out outside Alpha3 pending separate
   destination, cleanup, confirmation, and privacy contracts.

### 3 — post-alpha3 user-customizable tabs

Deliver full user-defined tab customization in a scoped update after
`0.1.0-alpha3` and before the supported `1.0` release.

#### Identity

1. let users set presentation-only tab labels, optional bounded icons, and
   pinned/favorite presentation state;
2. distinguish a tab label from the persistent Dojo name so a Window-local
   customization never silently renames shared daemon topology; and
3. define whether selected identity overrides are Window-local, reusable by
   Dojo identity, or explicitly promoted to durable metadata.

#### Behavior

1. support deterministic ordering rules, initial active-tab selection, strip
   visibility policy, and user-chosen default tab actions;
2. allow per-tab shortcuts only for the closed, typed application action
   registry, with strict conflict detection and generated help/shortcut labels;
3. preserve exact-target, stale-revision, non-destructive detach, confirmation,
   modal-isolation, and 32-tab bounds; and
4. never let a tab customization execute arbitrary shell commands, register
   trusted actions, mutate shared topology implicitly, or steal another
   client's focus/control.

#### Appearance

1. let users customize active/inactive colors, dimensions, padding, separators,
   alignment, icon/label spacing, close indicators, and strip visibility;
2. provide global defaults plus bounded per-tab or per-Dojo overrides;
3. keep the active tab identifiable without color alone and preserve close/new
   targets, pointer hit regions, keyboard navigation, compact layouts,
   fractional scaling, and 1–32-tab overflow behavior; and
4. inherit the active theme by default, validate every override strictly, and
   fall back atomically to the last valid configuration rather than partially
   applying malformed appearance state.

Add schema, precedence, migration, conflict, renderer, pointer, keyboard, and
configuration tests plus separately approved guarded matrices across
identity/behavior combinations, dark/light, opaque/translucent,
active/inactive, compact, overflow, and multiple-scale cases.

This milestone does not permit arbitrary CSS, executable icon providers,
terminal-output-controlled chrome, arbitrary key-launched commands, or plugins
that inject trusted tab UI. Its exact schema, persistence scope, icon source
policy, action allowlist, ordering semantics, and theme precedence require a
focused implementation plan before work begins.

### 4 — automation confidence

- Implement [Plan 0031](plans/0031-mcp-visual-demo-harness.md) with the reusable
  non-graphical preflight, isolated topology, evidence manifest, and cleanup
  attestation first.
- Treat its human-consent graphical sequence as a separately approved acceptance
  activity, not an ordinary automated test.
- Keep proposed first-class Herdr and live-appearance IPC work outside the
  committed near-term path until their product decisions are made.

### 5 — beta performance gate (complete)

[Plan 0011](plans/0011-burst-output-memory-retention.md) recorded the original
no-go, and [Plan 0012](plans/0012-bounded-compact-publication-frames.md) retained
the rejected compact-frame experiment. [Plan 0043](plans/0043-beta1-sparse-publication-frames.md)
superseded that design and satisfied the gate. Before the Beta 1 claim, the
accepted replacement had to:

1. reproduce attribution against the current alpha baseline;
2. prove bounded exact reconstruction and client allocation non-graphically;
3. preserve the Foot oracle, protocol limits, and one-latest-snapshot delayed
   subscriber bound;
4. pass serial validation and independent review; and
5. run the guarded graphical comparison only after separate approval.

Do not convert a daemon-only memory win or a client/latency regression into a
beta success claim.

## Current forward path

### Supported stable release

Beta 1 satisfied its sparse-publication performance gate through
[Plan 0043](plans/0043-beta1-sparse-publication-frames.md). A supported stable
release still requires the project to:

1. define release channels, compatibility duration, supported environments,
   upgrade/rollback policy, and support/security-reporting processes;
2. validate public install, active-daemon refusal, upgrade, rollback, reset, and
   recovery journeys outside the maintainer-only workflow;
3. retain clean-build evidence and independent product/readability and technical
   reviews; and
4. reconcile every stable-release gate in [`status.md`](status.md).

### Broader distribution

Nix, Home Manager, tertiary artifacts, additional compositors, and sandboxed
packages follow explicit support boundaries. They must not broaden compatibility
claims before their package, daemon/socket, upgrade, and validation contracts
exist.

## Phase 0 — skeleton and research (complete)

- Rust workspace and CI
- Topology/Lair/Dojo/Splint domain model
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
initial private Arch prerelease package with coherent binaries, service,
desktop/theme integration, upgrade handling, documentation, and licenses. The
repository and immutable versioned GitHub/AUR packages are now public beta
surfaces; the early rolling edge channel has been retired, and stable-support
commitments remain post-milestone work.

## Phase 3 — multiplexing (complete)

Detailed plans: [`plans/0004-phase3-multiplexing.md`](plans/0004-phase3-multiplexing.md) and the [`line/frame pane divider follow-up`](plans/0005-pane-divider-styles.md)

- Splint-tree editing and focus navigation
- Multiple Dojos per Lair
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
- Publication-snapshotted Lair/Dojo descendant policy semantics (complete)
- Inject non-authoritative Lair/Dojo/Splint/incarnation context into PTY children (complete)
- Public-CLI reference session picker and client-author examples (complete)
- Required full-capability `splinterm-mcp` adapter with supported-automation parity (complete)
- Reference in-Splint flow: discover, split, launch, observe, denied control, reconcile (complete)
- Logical topology automation remains separate from compositor-native window control
- No network listener in `splinterd` by default

## Native remote graphical client — Plan 0028

Detailed plan: [`plans/0028-remote-graphical-client.md`](plans/0028-remote-graphical-client.md)

- Phase 1 transport/authentication foundation implemented: strict remote
  profiles, exact OpenSSH argv, separate graphical relay mode, bounded logical
  channel multiplexer, transport-neutral protocol connections, one-child
  lifecycle, askpass validation, categorized diagnostics, explicit endpoint
  capabilities, and non-mutating `remote check`
- Existing byte-transparent `relay --stdio` compatibility retained
- Phase 2 native workflow implemented and non-graphically validated: global
  endpoint selection, remote Recent Sessions and Window attachment, pane/tab
  observation and control, history/search/resync, remote-safe launch mutations,
  endpoint recency namespaces, no-image enforcement, suppression of trusted
  graphical-focus publication, and trusted-only forced-transfer gating
- Human remote channels now negotiate `RemoteInteractive`; OpenSSH-authenticated
  graphical sessions receive normal terminal authority without automation policy,
  including immediate creation and attachment of new Lairs, Dojos, and Splints
- Fake-relay integration covers exact interactive identities, daemon denials,
  mismatched acknowledgements, and channel-local controller/subscription loss
- Phase 3 aggregate non-graphical closure passes, including strict workspace
  Clippy, affected package suites, and 18 serialized daemon end-to-end tests
- Phase 4 real-host closure passes: agent, terminal-password, and desktop
  `SSH_ASKPASS` authentication; native panes/tabs/search/scrollback/resize;
  ordinary control transfer; SSH/relay/daemon loss; persistence; isolated local
  regression; exact cleanup; and independent review are recorded under the Plan
  0028 closure artifact

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

Slices 0–2 are accepted: contracts, budgets, the bounded generic image
lifecycle, and Foot-compatible Sixel are implemented. The streaming DCS Sixel
path and bounded decoder match all five pinned semantic fixtures for whole,
every-split, and bytewise input. Foot cursor/scroller modes, XTSMGRAPHICS
replies, palettes, cell-aligned overwrite/underlay composition, and
resize/reflow behavior are implemented. The reviewed guarded matrix matches all
five retained Foot final-buffer cells byte-for-byte on workspace 8 / DP-2 with
no focus or placement violation, and a fresh 60-second ASan/libFuzzer run
completed 117,896 executions without a crash. Slice 3 is accepted: protocol v23
carries bounded image metadata and exact content
identities, with metadata exposed only to the executable-verified trusted UI.
The mandatory dedicated binary content channel, preferred sealed-memfd path,
bounded client source cache, and authoritative 64 MiB daemon byte admission are
implemented. Headless lifecycle, cleanup, stale-incarnation, cache-reuse, and
serialized daemon gates pass. Slice 4 is accepted with atomic bounded pane
source leases, stable-row placement projection, clipped premultiplied CPU image
composition, ADR z tiers, fractional geometry, conservative image damage,
full/incremental identity, and trusted-overlay precedence. Slice 5 is accepted
as the bounded practical Kitty static-image subset: selective streaming APC,
direct/chunked RGB/RGBA/PNG with zlib, transmit/display/query/visible-delete,
IDs and placements, crop/aspect/offset/cursor/z semantics, process-wide inbound
admission, exact fixture execution, and pinned `kitten icat`/Chafa trace replay.
Slice 6 is accepted with streaming inline-only iTerm2 OSC 1337 PNG support and
pinned self-contained fixtures. Its external Kitty input security spike rejects
ambient file, temporary-file, and POSIX-SHM names; those media remain bounded
`ENOTSUP` unless a future authenticated capability design is approved.
Placeholders, relative placement, multipart iTerm2 transfers, additional image
formats, and animation remain explicitly deferred. Slice 8's final guarded
Sixel/Kitty scale-and-pane matrix, no-image RSS gate, and one-tick p95 idle CPU
gate pass. Eager default-focus control acquisition now falls back to an
uncontrolled observer when the exclusive lease is already owned. Clean
committed main and MCP packages pass extracted runtime validation. Phase 5 is
complete; optional Slice 7 remains explicitly deferred.

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
