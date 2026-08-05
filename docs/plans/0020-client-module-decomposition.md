# Plan 0020: graphical client module decomposition

- **Status:** Proposed
- **Date:** 2026-08-04
- **Depends on:** stabilization of [Plan 0019](0019-dojo-tabs.md)
- **Architecture authority:** [Architecture](../architecture.md), [ADR 0001](../adr/0001-foot-rust-port.md), and [ADR 0003](../adr/0003-wayland-client-and-event-loop.md)
- **Behavioral authority:** Foot 1.27.0 commit `3c5b584b0eafa772eb4376fb6eaf6643399e190e`

## Decision

Decompose the oversized Splinterm graphical-client implementation into cohesive,
dependency-directed modules without changing terminal behavior, daemon ownership,
public automation contracts, renderer output, or Wayland lifecycle semantics.

The current files combine several independent systems:

| File | Total lines at planning time | Production code before tests |
| --- | ---: | ---: |
| `crates/splinterm/src/main.rs` | 7,963 | approximately 6,980 |
| `crates/splinterm/src/wayland.rs` | 12,505 | approximately 9,774 |
| `crates/splinterm/src/renderer.rs` | 9,203 | approximately 6,184 |

This plan treats file size as a symptom rather than the architectural problem.
The goal is not to create many small files. The goal is to establish explicit
ownership, narrow interfaces, one-way dependencies, independently testable state
reducers, and small composition roots.

Implementation must proceed as mechanical, validated extraction milestones.
Behavioral changes, optimization, visibility widening, and state redesign must
not be hidden inside file-movement commits.

## Goals

- Reduce `main.rs` to runtime setup and one application entry point.
- Separate platform-independent frontend message contracts from Wayland.
- Separate daemon/client orchestration from renderer and Smithay details.
- Make Wayland `App` a composition root over cohesive state objects rather than
  one structure whose methods directly manipulate approximately one hundred
  fields.
- Split rendering into settings, font, raster, frame, image, overlay, capture,
  and evidence subsystems while preserving exact Foot-derived behavior.
- Keep generic Window-local tab policy separate from both daemon task ownership
  and graphical tab presentation.
- Move unit tests with the implementation they exercise.
- Preserve bounded channels, caches, history, image residency, SHM buffers, and
  worker limits.
- Leave module boundaries suitable for later measurement-led optimization
  without claiming that file movement itself improves runtime performance.

## Non-goals

- Replacing the CPU renderer or adopting a GPU backend.
- Changing protocol, daemon topology, persistence, authorization, or controller
  semantics.
- Rewriting the active Window-local Dojo tab feature during decomposition.
- Modifying the canonical Foot checkout, comparison images, references, or
  tolerances.
- Broadly converting private implementation details into public APIs.
- Introducing traits, event buses, services, or dependency injection frameworks
  where one concrete implementation is sufficient.
- Running graphical tests without the separate approval required by
  `AGENTS.md`.
- Combining speculative performance changes with structural extraction.

## Architectural invariants

Every milestone preserves:

1. `splinterd` remains the sole owner of canonical topology, terminal state,
   PTYs, processes, and persistence.
2. The graphical client remains disposable presentation state.
3. Frontend messages remain bounded and carry no Wayland proxy objects.
4. The async protocol bridge knows no Smithay, SHM, or renderer implementation
   details.
5. Wayland handlers do not perform daemon topology operations directly; they
   emit bounded commands to the async owner.
6. Generic tab ordering and limits remain client-local and platform-independent.
7. The topology manager owns async task and subscription lifetimes; Wayland owns
   cached presentation state for each tab.
8. Only the active tab paints, reports focus, reconciles geometry, and claims
   controller ownership. Hidden tabs continue bounded semantic-update draining.
9. Renderer pixel output, rounding, fallback selection, glyph masks, image
   sampling, damage behavior, and cache invalidation remain unchanged during
   mechanical extraction.
10. Existing `lib.rs` re-exports remain compatible unless a separate reviewed
    API change explicitly replaces them.

## Target module layout

```text
crates/splinterm/src/
├── main.rs                         # runtime setup and app::run() only
├── app/                            # binary/application orchestration
│   ├── mod.rs                      # top-level command dispatch
│   ├── cli.rs                      # clap types
│   ├── local_service.rs            # reset, policy, relay
│   ├── human_output.rs
│   ├── consent.rs                  # trusted private consent protocol
│   ├── sessions.rs                 # picker, reopen, launch
│   ├── theme_watch.rs
│   ├── pane_bridge.rs              # daemon ↔ frontend pane messages
│   ├── topology_manager.rs
│   └── machine/
│       ├── mod.rs
│       ├── read.rs
│       ├── mutation.rs
│       ├── control.rs
│       ├── history.rs
│       ├── subscription.rs
│       └── output.rs
│
├── frontend/                       # platform-independent contracts
│   ├── mod.rs
│   ├── message.rs                  # WindowUpdate, WindowCommand, ThemeUpdate
│   ├── topology.rs                 # WindowTopology*, WindowDojoIdentity
│   └── options.rs                  # WindowOptions, WindowPaneOptions
│
├── renderer/
│   ├── mod.rs                      # small stable facade
│   ├── settings.rs                 # options, DPI, zoom, alpha
│   ├── fonts.rs                    # faces, fallback, metrics, glyph cache
│   ├── raster.rs                   # pixel blending and basic drawing
│   ├── text.rs                     # TextRow and ChromeText
│   ├── frame.rs                    # SnapshotFrame preparation
│   ├── compose.rs                  # rows, damage regions, scrolling
│   ├── images.rs                   # image preparation and sampling
│   ├── cursor.rs
│   ├── decorations.rs
│   ├── overlays/
│   │   ├── mod.rs
│   │   ├── picker.rs
│   │   └── history.rs
│   ├── capture.rs
│   └── evidence.rs                 # benchmarks, metrics, PPM output
│
├── wayland/
│   ├── mod.rs                      # run() and composition root
│   ├── app.rs                      # high-level orchestration
│   ├── state.rs                    # grouped top-level state
│   ├── pane_view.rs
│   ├── terminal_state.rs           # semantic update/history reducer
│   ├── damage.rs
│   ├── shm.rs
│   ├── clipboard.rs
│   ├── chrome.rs
│   ├── tabs.rs                     # graphical tab strip only
│   ├── picker.rs
│   ├── theme.rs
│   ├── effects.rs                  # Wayland adapter over reducer
│   ├── input/
│   │   ├── mod.rs
│   │   ├── keyboard.rs
│   │   ├── pointer.rs
│   │   ├── selection.rs
│   │   └── ime.rs
│   └── dispatch/
│       ├── mod.rs
│       ├── window.rs
│       ├── seat.rs
│       ├── keyboard.rs
│       ├── pointer.rs
│       ├── data_device.rs
│       └── output.rs
│
├── tab.rs                          # generic WindowTabSet policy
├── pane.rs                         # pure pane layout
├── viewport.rs                     # pure scrollback viewport
├── geometry.rs
└── background_effect.rs            # pure effect reducer
```

The exact final filenames may change when implementation reveals a more cohesive
boundary. Changes to this tree must preserve the dependency direction and avoid
replacing three large files with one differently named monolith.

## Dependency direction

```text
splinterm-core and splinterm-protocol types
                    ↓
geometry / pane / viewport / tab / background_effect
                    ↓
renderer internals and platform-independent frontend contracts
                    ↑                              ↑
          Wayland presentation            app protocol bridge
                    ↑                              ↑
                    └──── application composition ┘
                                   ↑
                                main.rs
```

The critical inversion is that application orchestration and topology management
import `frontend` contracts rather than importing them from `wayland`. Wayland
is a platform adapter and must not own the cross-thread application contract.

## Responsibility boundaries

### `main.rs` and `app`

`main.rs` should eventually contain only module declaration, runtime setup, and
`app::run()`. Binary-owned application modules contain Clap parsing, local
service operations, machine output, daemon connections, session startup, and
window task orchestration.

The machine client must not depend on Wayland, renderer, tabs, or graphical
topology management. Trusted consent framing remains a separate security-
sensitive module rather than ordinary session-launch code.

### `frontend`

`frontend` owns bounded data exchanged between the async application owner and
the graphical thread:

- window and pane construction options;
- terminal, authority, theme, and lifecycle updates;
- terminal input, resize, control, history, and transfer commands; and
- Window-local topology and tab commands.

It owns no Smithay types, Wayland proxies, renderer frames, daemon connections,
or task handles.

### `renderer`

`renderer::mod` remains a narrow facade. Most internal types stay private or
`pub(super)`. Higher rendering layers may depend on primitives and fonts;
primitives must not depend on frames, overlays, captures, or Wayland.

The first decomposition preserves current process-global initialization and
cache ordering. A later, separate milestone may introduce explicit ownership:

```text
RendererResources
├── immutable font faces
├── shared glyph and image caches
└── raster resources

RenderContext
├── output DPI and surface scale
├── font zoom
├── background alpha
└── per-window presentation state
```

That state change requires focused tests and review; it is not part of the
mechanical file moves.

### `wayland`

Wayland `App` remains the platform composition root, but its state should be
grouped by ownership rather than exposed through many mutable field getters:

```rust
struct App {
    platform: PlatformState,
    surface: SurfaceState,
    presentation: PresentationState,
    input: InputState,
    clipboard: ClipboardState,
    panes: PaneWorkspace,
    tabs: TabPresentationState,
    modal: ModalState,
    scheduling: RenderSchedule,
}
```

Subsystems expose operations such as `apply_update`, `prepare_frame`,
`encode_key`, `begin_read`, `activate`, and `schedule_draw`. Smithay callbacks
should translate one protocol event, call a subsystem operation, and schedule a
redraw or bounded command. They should not directly coordinate unrelated state.

### Tabs

Tab ownership remains deliberately split:

- `tab.rs` owns generic ordering, bounds, active identity, close, and reorder
  policy;
- `wayland/tabs.rs` owns trusted strip layout, hit testing, painting, and cached
  graphical state; and
- `app/topology_manager.rs` owns per-Dojo subscriptions, task cancellation, and
  daemon reconciliation.

No shared object may span Wayland proxies, renderer frames, daemon connections,
and async task handles.

## Dependency-ordered milestones

### Milestone 0 — stabilize and record the baseline

Complete or coherently checkpoint Plan 0019 before moving tab-sensitive code.
Record the current non-graphical test, lint, formatting, and diff-check baseline.
Do not revert or overwrite the active tab implementation.

### Milestone 1 — renderer picker and primitives

Extract the current picker presentation, raster primitives, and `ChromeText`
into `renderer/overlays/picker.rs`, `renderer/raster.rs`, and
`renderer/text.rs`. Preserve the existing renderer facade and move focused tests
with their implementations.

This is the lowest-risk first implementation milestone.

### Milestone 2 — remaining renderer subsystems

Extract settings, fonts/cache, frame preparation, decorations, cursor, images,
composition, history overlays, capture, and evidence in dependency order.
Preserve all Foot-derived calculations and comparisons exactly.

Do not replace renderer globals in this milestone.

### Milestone 3 — frontend contracts

Move `WindowUpdate`, `WindowCommand`, `ThemeUpdate`, `WindowTopologyCommand`,
`WindowTopologyUpdate`, `WindowDojoIdentity`, `WindowOptions`, and
`WindowPaneOptions` out of `wayland.rs` and into `frontend` modules. Preserve
existing crate-root re-exports.

This milestone establishes the dependency direction needed by both later
`main.rs` and Wayland work.

### Milestone 4 — pure Wayland-independent reducers

Extract terminal update/history reduction, keyboard encoding, selection and URL
logic, pointer/history classification, shortcut reducers, damage calculations,
and bounded clipboard I/O. These modules accept plain values and contain no
Wayland proxy objects.

### Milestone 5 — low-risk application services

Move human output, theme watching, local reset/policy/relay operations, consent,
and session selection/launch out of `main.rs`.

### Milestone 6 — machine client

Split JSON/NDJSON operations by read, mutation, control, history, subscription,
and output responsibilities. Keep common connection and envelope helpers in the
smallest shared parent module that avoids cycles.

### Milestone 7 — pane protocol bridge

Extract attachment, resynchronization, authority loading, controller leases,
resize batching, image leases, and pane subscriptions. Its Wayland-facing API is
only bounded `frontend` messages.

### Milestone 8 — Wayland state composition

Group `App` fields into cohesive owned subsystems and move state together with
its behavior. Do not create a facade containing dozens of mutable getters or a
collection of modules that all mutate the entire `App` directly.

### Milestone 9 — Wayland protocol dispatch

Move Smithay and protocol `Dispatch` implementations only after state ownership
is explicit. Keep handlers thin and retain delegate macro and registry behavior.

### Milestone 10 — tab presentation and topology manager

After Plan 0019 behavior is stable, extract graphical tab presentation and the
async topology manager independently. Preserve active/hidden subscription,
focus, controller-release, resize, and cleanup semantics.

### Milestone 11 — explicit renderer contexts

Evaluate replacing renderer globals with shared immutable resources and
per-window mutable context. Retain this change only if focused correctness,
multi-window ownership, cache, and performance evidence support it.

### Milestone 12 — architecture closure

Update architecture documentation, remove obsolete compatibility shims, inspect
module visibility, and record final review and validation evidence. Do not claim
completion until both validation evidence and recorded review exist.

#### Closure record — 2026-08-05

Plan 0020 is complete through Milestone 12. The accepted closure reduces
`main.rs` to a six-line Tokio composition root; separates leaf command grammar,
CLI dispatch, machine clients, neutral session-catalog helpers, session UI,
graphical window lifecycle, pane protocol bridging, topology task ownership,
and theme observation; extracts private Wayland chrome painting; and documents
the final frontend, renderer-context, Wayland, and application boundaries.
Application internals are limited to `pub(in crate::app)` or narrower except for
the single crate-root `app::run` entry point. The Wayland public facade remains
`run` plus the existing bracketed-paste export.

Recorded non-graphical validation:

- normalized source equivalence passed for all 18 mechanically moved command,
  window-lifecycle, session-catalog, and chrome functions;
- `cargo test -p splinterm` passed 237 library tests, 35 binary tests, 15
  automation CLI integration tests, and 7 policy CLI integration tests;
- `cargo check -p splinterm --all-targets` passed;
- strict all-target Clippy passed with `-D warnings`, wildcard-import checks, and
  only the recorded baseline allowances for `collapsible-if`,
  `manual-is-multiple-of`, and `useless-vec`;
- `cargo fmt --all -- --check` and `git diff --check` passed; and
- dependency and visibility audits found no reverse imports from machine/local
  services into CLI dispatch, no session/topology/window cycle, no graphical
  dependencies in the async pane or topology services, and no new public
  Wayland or crate export.

Two fresh final reviews were recorded. The behavior/lifecycle review accepted
CLI and machine contracts, bounded queues, pane resynchronization, controller
and image cleanup, topology/theme task shutdown, renderer behavior, chrome
painting, public exports, and Wayland teardown with no blockers. The initial
architecture review identified command/session dependency cycles and incomplete
window dependency documentation; after neutral `commands` and
`session_catalog` leaves and corrected documentation were added, a fresh
second-round architecture review accepted ownership, dependency direction,
visibility, documentation, and every Plan 0020 completion criterion with no
blockers.

A full-workspace attempt during Milestone 11 retained two unrelated timing
failures in unchanged `splinterd` end-to-end tests
(`parent_policy_snapshot_excludes_new_splint_until_reload` and
`phase8_detach_reattach_overflow_resync_and_cleanup`). Daemon unit tests and the
bounded workspace excluding `splinterd` passed; no daemon or protocol files were
changed by Milestones 11–12. No graphical validation was authorized or required
for these behavior-preserving architecture changes.

## Test strategy

Move unit tests with implementation by default. This preserves private testing
and prevents architectural damage from making internal details public merely so
integration tests can reach them.

Keep renderer pixel, font, image, and Foot-oracle tests adjacent to renderer
implementation. Keep pure state-machine and input-encoding tests adjacent to the
extracted reducers.

Add integration tests only for stable seams where they provide value:

- daemon protocol bridge to `WindowUpdate`;
- topology-manager command/update transitions;
- public final-buffer capture behavior; and
- subprocess-level JSON/NDJSON CLI contracts where already practical.

Do not move the complete monolithic test suites into integration tests. Wayland
dispatch wiring is primarily compile-checked; behavioral logic should be tested
through extracted non-graphical reducers. Graphical validation remains a
separately approved activity.

## Validation gates

Run after every coherent extraction milestone:

```bash
cargo fmt --all -- --check
cargo test -p splinterm --lib --bin splinterm
cargo clippy -p splinterm --all-targets -- -D warnings
git diff --check
```

Run additional focused tests for the module being moved. Renderer milestones
must retain the relevant deterministic capture, image, and Foot comparison
evidence already required by the owning feature plans. Full workspace tests are
required at acceptance boundaries where shared protocol or crate exports change.

Graphical tests are not authorized by this plan. If a later milestone changes
runtime behavior rather than only structure, request approval for the complete
bounded graphical sequence under `AGENTS.md` before running it.

## Code organization rules

- Prefer cohesive modules of roughly 400–1,200 lines over arbitrary micro-files.
  Larger algorithmically cohesive modules are acceptable when splitting them
  would create circular or leaky APIs.
- Target `main.rs` at fewer than approximately 50 lines.
- Keep facade `mod.rs` files small and intentional, preferably below
  approximately 300 lines.
- Use private or `pub(super)` visibility by default.
- Prefer concrete state and functions over single-implementation traits.
- Avoid shared mutable access across subsystem boundaries.
- Preserve bounded queues and backpressure; moving code is not permission to
  widen limits.
- Keep pure reducers independent of async runtimes, Wayland, and renderer state.
- Keep application orchestration independent of Smithay and SHM.
- Keep Wayland dispatch independent of daemon connection mechanics.
- Use one behavior-preserving concern per commit.

## Principal risks

### Frontend contracts currently belong to Wayland

`main.rs` consumes message and option contracts re-exported from `wayland.rs`.
Extracting application orchestration first could produce an `app ↔ wayland`
cycle. Milestone 3 must precede the pane bridge and topology-manager split.

### Active tab work crosses threads

Plan 0019 currently spans generic tab policy, graphical cached state, and async
task ownership. Moving it before behavior stabilizes risks controller leaks,
missed hidden-tab updates, incorrect focus reporting, or cancellation races.
Tab-sensitive extraction remains late in the sequence.

### `PaneView` is a coupling hub

`PaneView` currently combines snapshots, frames, history, search, selection,
input state, authority, controller status, image leases, and update channels.
Merely moving it would move the monolith. Define narrow operations before
considering further internal decomposition.

### Renderer globals encode ordering

Configuration, output DPI, zoom, alpha, faces, and caches currently rely on
process-global initialization and invalidation ordering. Preserve that ordering
through mechanical extraction; redesign ownership only in its own milestone.

### Excessive visibility can erase the benefit

A decomposition that makes most types `pub(crate)` or exposes mutable fields has
not established useful boundaries. Review every visibility expansion and prefer
facade operations over raw access.

## Suggested commit sequence

1. `Refactor renderer picker into dedicated module`
2. `Refactor renderer raster and text primitives`
3. `Split renderer frame composition subsystems`
4. `Extract frontend window contracts from Wayland`
5. `Extract terminal update and input reducers`
6. `Split machine client from main`
7. `Extract client theme and consent services`
8. `Extract pane protocol bridge`
9. `Group Wayland application state`
10. `Split Wayland input and clipboard handlers`
11. `Split Wayland protocol dispatch`
12. `Extract tab presentation and topology management`
13. `Replace renderer globals with explicit contexts`
14. `Document and review final client architecture`

Each commit must compile and pass its focused non-graphical validation. If one
mechanical extraction is too large to review reliably, split it at a cohesive
subsystem boundary rather than retaining a partially connected intermediate
architecture.

## Completion criteria

This plan is complete only when:

- `main.rs` is a small entry point;
- frontend contracts no longer live under the Wayland implementation;
- renderer and Wayland have intentional facades and one-way internal
  dependencies;
- Wayland `App` composes cohesive state objects rather than directly owning one
  flat set of unrelated state;
- async protocol and topology code contain no renderer, Smithay, SHM, or Wayland
  proxy dependencies;
- generic tab policy, graphical tab presentation, and daemon task ownership are
  visibly separate;
- existing public exports and machine contracts remain compatible or have a
  separately approved migration;
- focused and aggregate validation pass;
- `git diff --check` passes;
- architecture documentation reflects the final boundaries; and
- recorded review confirms no blocker in ownership, cleanup, protocol,
  renderer, or Wayland lifecycle behavior.
