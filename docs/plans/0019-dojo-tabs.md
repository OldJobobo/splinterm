# Plan 0019: Window-local Dojo tabs

- **Status:** Complete — implementation, full non-graphical validation, approved graphical matrix, bounded resource evidence, exact cleanup, and independent review pass
- **Date:** 2026-08-04
- **Reconciled:** 2026-08-09 against commits `9cc591f`, `2cc7d86`, `3ee2048`, later Plan 0017/0025/0028 evidence, and [`artifacts/0019-dojo-tabs/closure-2026-08-09/`](artifacts/0019-dojo-tabs/closure-2026-08-09/)
- **Depends on:** [Plan 0017](0017-inline-session-picker-overlay.md), [Plan 0018](0018-lair-dojo-topology-migration.md)
- **Architecture authority:** [ADR 0009](../adr/0009-topology-lair-dojo-migration.md)

## Reconciliation record — 2026-08-09

The original `Proposed` label became stale after `9cc591f` implemented the
bounded client-local tab model and `2cc7d86` documented it. Commit `3ee2048`
then extracted tab presentation and topology management without changing the
contract. Current README, architecture, configuration, and PRD surfaces all
identify Window-local Dojo tabs as implemented, and the client enforces the
32-tab bound.

Later guarded evidence exercises substantial Plan 0019 behavior. Plan 0025
recorded inactive/active tab targeting, detached close-other semantics, exact
captured identities, stale-pixel cleanup, and preservation of daemon-owned
Dojos and Splints. Plan 0028 recorded physical creation and switching of a
second Dojo/tab in local and remote native Windows. Current focused tests cover
bounded insertion, duplicate activation, stable reorder/close selection,
wrapped navigation, exact consumed shortcuts, tab-strip hit targets and compact
layout, active-tab selection color, and controller acquisition on active resize.

This reconciliation does not manufacture a historical completion claim. The
closure audit found genuine hidden-tab/resource and 32-tab capacity gaps. The
implementation now directly tests hidden semantic caching without frame rebuild
or resize/control commands, all-pane controller release, pane-task cancellation
and join, renderer image-lease release, transactional tab bounds, and bounded
connection saturation/recovery.

The approved guarded smoke and matrix pass 1/2/16/32 tabs, cross-Lair labels,
dark/light and opaque/translucent themes, normal/compact/minimal layouts, scales
1.2/1.5/2.4, hidden burst responsiveness, keyboard/pointer/picker actions, active
and final tab closure, tab-33 rejection, retained topology, memory/CPU/FD/thread
samples, and warm switch latency. Exact evidence and diagnostics are retained in
[`EVIDENCE.md`](artifacts/0019-dojo-tabs/closure-2026-08-09/EVIDENCE.md). Fresh
read-only reviewer `d4898b93` found two evidence blockers: initial frames taken
before first composition and a missing explicit clean-index record. The complete
smoke was rerun with committed terminal output before capture, exact cleanup
passed again, the early matrix frame is excluded from positive evidence, and
`git diff --cached --quiet` is retained. The review disposition records no
remaining blocker.

The matrix exposed that the daemon's old 32-connection admission limit made the
advertised 32-tab Window impossible because each graphical Splint currently
retains independent observation and control channels. With explicit user
approval, the still-hard limit is now 128; authorization and every
per-connection/per-resource bound remain unchanged. The independent reviewer
accepted this bounded connection/resource boundary.

## Goal

Allow one native Splinterm Wayland Window to present several daemon-owned Dojos
as application-owned tabs without making tabs or Windows part of persistent
daemon topology.

```text
splinterd topology                  disposable splinterm Window

Lair A                              ┌──────────────────────────┐
├── Dojo editor  ──────────────────▶│ editor | logs | notes  + │
└── Dojo logs    ──────────────────▶│                          │
Lair B                              │ active Dojo layout       │
└── Dojo notes   ──────────────────▶│ and its Splints          │
                                    └──────────────────────────┘
```

The persistent hierarchy remains:

```text
Topology → Lair → Dojo → Splint
```

A Window owns an ordered, bounded set of client-local references to Dojos and
one active tab. Opening, activating, reordering, or closing a tab must not
create, rename, restore, close, kill, or otherwise mutate its referenced Dojo
unless the user invokes a separate explicit topology operation.

## Product decisions

### Ownership and persistence

- Tabs are native presentation state owned by one running graphical client.
- Tabs are not added to `splinterm-core`, durable `topology.json`, daemon policy,
  public CLI JSON, MCP, audit identities, or child environment context.
- Closing a tab detaches its client-side subscriptions and control leases only.
  Its Dojo, Splints, PTYs, and processes continue under `splinterd`.
- Closing the final tab closes the native Window.
- Tab order and the active tab are not restored after the Window exits in the
  first release.
- The same Dojo may be mapped in separate native Windows, but appears at most
  once in a particular Window. Opening an already-present Dojo activates its
  existing tab.
- One Window may contain Dojos from different Lairs. This preserves the current
  Recent Sessions behavior rather than making a Lair an implicit native-window
  boundary.

### Tab creation and session selection

- Selecting a running Dojo from the inline Recent Sessions picker opens it as a
  new tab or activates its existing tab; it no longer replaces and destroys the
  current frontend.
- The picker's existing `New terminal` action continues to create a fresh Lair
  with its initial Dojo, then opens that Dojo as a tab.
- `Ctrl+Shift+D` and the tab-strip `+` action create a new Dojo inside the active
  tab's Lair and open it as the active tab. They do not create a fresh Lair.
- Restore remains explicit. Exited or partially exited Dojos are not silently
  restored merely because a tab reference is requested.

### Keyboard contract

The initial built-in bindings are:

| Binding | Application action |
| --- | --- |
| `Ctrl+Tab` | Activate the next Dojo tab, wrapping at the end. |
| `Ctrl+Shift+Tab` | Activate the previous Dojo tab, wrapping at the start. |
| `Ctrl+Shift+D` | Create and open a new Dojo in the active Lair. |
| `Ctrl+Shift+Q` | Detach and close the active tab; close the Window if it was the final tab. |
| `Ctrl+Shift+S` | Open Recent Sessions; selection opens or activates a tab. |

Existing bindings retained unchanged:

| Binding | Existing action that remains |
| --- | --- |
| `Ctrl+Shift+Arrow` | Move focus between Splints in the active Dojo. |
| `Ctrl+Shift+W` | Terminate and close the focused Splint in a managed window. |
| `Ctrl+Shift+T` | Request control transfer for the focused Splint. |

Sequential Splint traversal currently assigned to `Ctrl+Shift+Tab` is removed;
directional `Ctrl+Shift+Arrow` navigation remains. Plain `Tab`, terminal-defined
key input, and unrelated chords continue to reach the terminal normally.
Application-owned tab shortcuts are consumed on press, repeat, and release so
they cannot leak partial input to a terminal.

## UI contract

### Trusted tab strip

The tab strip is trusted application chrome rendered by `splinterm`, never
terminal content. It is painted above the active Dojo and reserves its own
logical surface rectangle. The pane compositor receives only the content
rectangle below the strip.

Managed multi-Splint Windows always reserve the tab strip, including when only
one tab is open. This makes tab creation discoverable and avoids changing PTY
rows merely because a second tab appears. Legacy direct single-Splint evidence
and consent windows remain outside this tabbed host.

Each visible tab shows a sanitized, clipped human label:

- the Dojo name when adjacent tabs belong to the same Lair or the label remains
  otherwise unambiguous;
- `Lair / Dojo` when Lair context is needed; and
- no UUID, terminal-provided OSC title, command output, or requester-controlled
  authority text.

The active tab must remain identifiable without color through position, fill,
frame/underline treatment, and text weight. The strip includes explicit `+` and
close hit targets. Middle-click may close a tab only when press and release
resolve to the same committed tab target. Drag reordering, detachable tabs,
animations, and a general widget framework are deferred.

When all tab labels do not fit, the strip keeps the active tab visible and uses
bounded horizontal paging rather than shrinking labels to unusable targets.
Pointer-wheel behavior over the strip may page tabs but must not reach terminal
history or mouse reporting. Compact windows retain keyboard tab switching and
at least the active-tab label, close action, and `+` action.

### Geometry and input isolation

Adding the strip changes the terminal content origin. Every dependent path must
use one authoritative content rectangle:

- split-tree layout and pane hit testing;
- terminal painting and damage;
- pointer-cell mapping and URL/selection coordinates;
- history and search overlays;
- IME cursor rectangles;
- clipboard and terminal mouse reporting;
- resize and terminal-grid calculation; and
- session-picker scrim and overlay composition.

Tab-strip presses belong to application chrome and must never become terminal
selection, paste, URL, or mouse-protocol input. A tab switch settles active
terminal pointer ownership and IME state using the same modal-safety principles
as Plan 0017 before installing the target frontend.

## Client state model

Introduce explicit presentation models rather than adding another condition to
the current single-Dojo fields. Names are illustrative:

```rust
struct WindowTabSet {
    tabs: Vec<DojoTabView>,
    active: usize,
}

struct DojoTabView {
    lair_id: LairId,
    dojo_id: DojoId,
    lair_name: String,
    dojo_name: String,
    layout: LayoutNode,
    pane: PaneView,
    inactive_panes: Vec<PaneView>,
    pending_exited_splints: HashSet<SplintId>,
    frame_titles: HashMap<SplintId, CachedFrameTitle>,
    dirty_inactive_panes: HashSet<SplintId>,
}
```

Per-Dojo terminal frontend state moves into `DojoTabView`. Wayland globals,
SHM buffers, output/scale state, clipboard devices, theme, renderer generation,
modal picker state, and the native `Window` remain host-wide.

Do not use `DojoId` alone as the vector index. Keep stable identity separate
from order so closing or inserting a tab cannot redirect a pending action.
Pure `WindowTabSet` operations must cover open-or-activate, next/previous,
close-with-neighbor-selection, bounded insertion, and externally removed Dojos.

The initial per-Window tab bound is 32. Reaching the bound rejects only the new
open operation with an application-owned diagnostic; it does not evict another
tab or mutate daemon state. This bound must be centralized and tested.

## Async topology and subscription model

The current topology manager owns one `dojo_id`, one root, one pending focus,
and one set of pane tasks. Replace it with host state resembling:

```rust
struct ManagedWindowState {
    tabs: Vec<ManagedDojo>,
    active_dojo: DojoId,
}

struct ManagedDojo {
    lair_id: LairId,
    dojo_id: DojoId,
    root: LayoutNode,
    pending_focus: Option<PendingTopologyFocus>,
    pane_tasks: Vec<JoinHandle<Result<()>>>,
}
```

### Target every asynchronous operation

Every command and update that can outlive the current event-loop iteration must
carry stable target identity. In particular, split, close, ratio adjustment,
open, activate, and close-tab commands must identify their Dojo. A queued split
must never execute against a different Dojo merely because the user switched
tabs before the async manager received it.

Topology updates similarly identify the affected Dojo. The Wayland thread may
apply updates to a hidden tab's cached `DojoTabView`, but only the active tab can
schedule visible pane damage.

### Reconciliation

Use one `InspectTopology` result per manager poll to reconcile every open tab.
Do not issue one full topology request per tab. For each managed Dojo:

- reconcile its authoritative layout and revision-bound pending focus;
- prepare subscriptions only for newly added Splints;
- drop removed Splint frontends and their image leases;
- remove a tab if its Dojo was explicitly removed elsewhere; and
- close the Window only when no tabs remain.

A failure affecting one added or switched tab reports that tab's error and
leaves existing tabs usable. Daemon/protocol connection failure remains a
Window-wide shutdown condition.

### Hidden-tab updates, control, and resize

Open tabs retain bounded terminal subscriptions so activation can be immediate.
Hidden tabs continue draining and applying semantic updates, but do not paint,
blink cursors, emit focus reports, or schedule SHM commits.

On deactivation:

- settle pointer, selection, paste, and IME ownership;
- release controller leases held by every Splint frontend in that tab; and
- preserve its last acknowledged terminal dimensions and client-local viewport.

On activation:

- install the cached frontend without reconstructing snapshots;
- recompute the active Dojo against the current content rectangle;
- send at most one resize reconciliation per Splint for the new active geometry;
- acquire control lazily through existing input/resize behavior; and
- update title, authority, search, cursor, and trusted status from the newly
  active Splint.

Hidden tabs are not resized on every Window configure. This avoids background
`SIGWINCH` churn and controller acquisition for a Dojo the user cannot see.

## Resource and performance boundaries

Tabs multiply client-side snapshots, pane update queues, image leases, shaped
chrome text, and subscription/controller tasks. The first implementation must:

- preserve all existing bounded channel capacities;
- enforce the 32-tab Window bound before preparing pane subscriptions;
- keep one renderer-wide image-content byte budget across tabs;
- release image leases and cancel/join pane tasks when a tab closes;
- bound tab-label text caches to the visible paging neighborhood;
- avoid repainting terminal backing when only tab selection chrome changes;
- avoid rebuilding an inactive tab's snapshot frames on every active-tab draw;
- record tab-switch latency and retained-memory evidence before closure; and
- reject a tab open cleanly if complete pane preparation fails, leaving the
  source and other tabs unchanged.

The existing burst-output and compact-publication invariants remain in force.
Tabs must not reintroduce unbounded full-snapshot queues or delayed-subscriber
snapshot accumulation.

## Dependency-ordered milestones

### Milestone 0 — freeze behavior and pure contracts

Files:

- `docs/plans/0019-dojo-tabs.md`;
- focused tests in `crates/splinterm/src/wayland.rs` or a small client-local tab
  module if extraction is justified.

Work:

1. Record the shortcut and lifecycle decisions above.
2. Add pure tab-set tests before changing the live frontend.
3. Freeze the current one-Dojo Window, picker, pane-focus, topology-edit,
   controller, and resize behavior with focused regressions.
4. Preserve unrelated dirty renderer and pending-focus work in the active
   worktree.

Validation:

```bash
cargo test -p splinterm --lib tab
cargo test -p splinterm --lib pane_focus
cargo fmt --all --check
git diff --check
```

### Milestone 1 — targeted commands and multi-Dojo manager state

Files:

- `crates/splinterm/src/main.rs`;
- `crates/splinterm/src/wayland.rs`.

Work:

1. Add Dojo identity to asynchronous topology commands and updates.
2. Replace singular manager state with bounded `ManagedDojo` entries.
3. Reconcile all open Dojos from one topology snapshot.
4. Make open preparation transactional: publish a tab only after every initial
   pane frontend is ready.
5. Ensure close-tab cancellation drains or aborts only that tab's tasks.
6. Keep the graphical frontend singular until this manager seam is validated.

Validation:

```bash
cargo test -p splinterm --lib topology
cargo test -p splinterm --lib tab
cargo fmt --all --check
cargo clippy -p splinterm --all-targets -- -D warnings
git diff --check
```

### Milestone 2 — per-tab Wayland frontend state

File:

- `crates/splinterm/src/wayland.rs`.

Work:

1. Extract per-Dojo pane, layout, focus, title-cache, pending-exit, and damage
   state into `DojoTabView`.
2. Introduce `WindowTabSet` and active-tab accessors.
3. Apply hidden-tab updates without visible damage.
4. Implement activation/deactivation control, focus, resize, IME, pointer, and
   search reconciliation.
5. Preserve session-picker deferred-update ordering when its target or source
   tab changes while the modal is open.

Validation:

```bash
cargo test -p splinterm --lib tab
cargo test -p splinterm --lib topology
cargo test -p splinterm --lib session_picker
cargo test -p splinterm --lib control
cargo fmt --all --check
git diff --check
```

### Milestone 3 — trusted tab strip layout and painting

Files:

- `crates/splinterm/src/wayland.rs`;
- `crates/splinterm/src/renderer.rs` only for narrow reusable chrome text or
  paint primitives;
- `crates/splinterm/src/geometry.rs` if the authoritative content-rectangle seam
  belongs there.

Work:

1. Add pure tab-strip layout with bounded paging and non-overlapping hit targets.
2. Reserve the strip in managed Window geometry from initial configure onward.
3. Paint the strip as application chrome outside terminal backing.
4. Keep the active tab visible and cache only bounded visible label text.
5. Route pointer, wheel, scale, configure, and damage through committed strip
   geometry.
6. Begin with conservative full-strip damage; optimize only after correctness.

Validation:

```bash
cargo test -p splinterm --lib tab_strip
cargo test -p splinterm --lib geometry
cargo test -p splinterm --lib renderer
cargo fmt --all --check
git diff --check
```

### Milestone 4 — keyboard, pointer, and picker integration

Files:

- `crates/splinterm/src/wayland.rs`;
- `crates/splinterm/src/main.rs`;
- `crates/splinterm/src/session_picker.rs`.

Work:

1. Implement the approved keyboard contract.
2. Move sequential pane traversal off `Ctrl+Shift+Tab`; preserve directional
   pane focus.
3. Add open-or-activate behavior for picker selections.
4. Add current-Lair Dojo creation for `Ctrl+Shift+D` and `+`.
5. Keep picker `New terminal` as fresh-Lair creation.
6. Implement tab close buttons and optional same-target middle-click close.
7. Ensure every tab action is consumed without terminal input leakage.

Validation:

```bash
cargo test -p splinterm --lib tab
cargo test -p splinterm --lib session_picker
cargo test -p splinterm --lib key
python -m pytest -q tools/automation/test_session_picker.py
cargo fmt --all --check
git diff --check
```

### Milestone 5 — lifecycle, bounds, and performance closure

Files:

- focused Splinterm tests and benchmark tooling;
- evidence under a new dated `docs/benchmarks/artifacts/` or
  `docs/spikes/artifacts/` directory only when measurements are actually run.

Work:

1. Exercise 1, 2, 16, and 32 tabs with mixed Splint counts.
2. Verify hidden tabs drain bounded updates without rendering or resize churn.
3. Verify closing tabs releases controllers, image leases, and tasks.
4. Measure warm tab-switch latency, idle CPU, and retained client memory.
5. Run burst output in a hidden tab and prove the active tab remains responsive.
6. Verify duplicate-open activates rather than allocating another tab.

Validation:

```bash
cargo test -p splinterm
cargo test --workspace
cargo fmt --all --check
cargo clippy -p splinterm --all-targets -- -D warnings
git diff --check
```

### Milestone 6 — current-state documentation and reviewed closure

Files:

- `README.md`;
- `GLOSSARY.md`;
- `docs/architecture.md`;
- `docs/configuration.md`;
- `docs/integrations.md` where picker behavior is described.

Document:

- that tabs are Window-local references rather than persistent topology;
- exact create, activate, close, and last-tab behavior;
- all approved bindings in plain language;
- cross-Lair tab labels;
- the distinction between closing a tab, closing a Splint, and closing a Dojo;
- the 32-tab bound; and
- the non-restored first-release tab order.

Final non-graphical validation:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p splinterd --test end_to_end -- --test-threads=1
python -m pytest -q tools/automation/test_session_picker.py
git diff --check
```

## Graphical validation boundary

This plan does not authorize graphical testing. When separately approved, use
one guarded smoke case before any matrix. The approval request must identify the
isolated workspace-8/DP-2 target, permitted focus/input actions, cleanup, and the
complete matrix.

The smoke must verify:

- correct isolated placement and preserved user focus;
- tab-strip initial geometry with one tab;
- `Ctrl+Shift+D`, `Ctrl+Tab`, `Ctrl+Shift+Tab`, and `Ctrl+Shift+Q`;
- pointer activation and close without terminal input leakage;
- picker open-or-activate behavior;
- hidden-tab output followed by immediate activation;
- no process termination when closing a tab; and
- complete cleanup and focus restoration.

Only after the smoke succeeds may the approved matrix cover compact/normal
sizes, scales 120/150/240, light/dark/translucent themes, 1/2/many tabs,
cross-Lair labels, hidden-tab burst output, active-tab removal, and last-tab
Window closure. Any wrong-target input, placement, focus, or cleanup failure
aborts the sequence.

## Stop gates

Stop for user input before implementation expands into any of the following:

- persisting Window tab sets or tab order;
- adding tabs to daemon topology, policy, audit, CLI JSON, MCP, or child context;
- restricting a Window to one Lair;
- permitting duplicate tabs for one Dojo in the same Window;
- silently restoring exited Splints when opening a tab;
- changing `Ctrl+Shift+W`, `Ctrl+Shift+T`, or directional pane-navigation meaning;
- changing controller-transfer semantics rather than releasing hidden-tab
  leases through existing supported operations;
- introducing drag-and-drop, tear-off tabs, animations, or a general widget
  framework;
- materially widening client memory or queue bounds;
- modifying the canonical Foot oracle or graphical references; or
- running graphical validation without separate approval.

## Acceptance criteria

The feature is complete only when:

1. one native Window can host an ordered bounded set of Dojo tabs;
2. tabs remain entirely client-local and daemon topology is unchanged;
3. picker selection opens or activates without destroying another tab;
4. the approved keyboard bindings work and do not leak terminal input;
5. closing a tab never kills or closes its Dojo or Splints;
6. every queued edit and update is applied to its explicitly identified Dojo;
7. hidden tabs retain current bounded state without painting, resizing, blinking,
   or holding controller authority;
8. active-tab geometry, pointer mapping, IME, resize, and damage account for the
   trusted strip;
9. tab and image resources are released on close and the 32-tab bound is
   enforced transactionally;
10. focused and workspace-wide non-graphical validation passes;
11. separately authorized graphical evidence passes the guarded smoke and
    approved matrix; and
12. independent review reports no blocker or fix worth doing now, with recorded
    validation and review evidence as required by `AGENTS.md`.
