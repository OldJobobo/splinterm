# Plan 0025: Command palette and tab context menus

- **Status:** First expansion published and installed; second approved 31-command/tab-action expansion implemented, reviewed, and validated; awaiting publication, packaging, and installation decisions
- **Date:** 2026-08-06
- **Depends on:** [Plan 0017](0017-inline-session-picker-overlay.md), [Plan 0019](0019-dojo-tabs.md)

## Goal

Add two pieces of trusted, Window-local application UI:

1. a searchable command palette opened from a managed terminal Window; and
2. a compact context menu opened from a Dojo tab.

The first release is intentionally a design prototype with real actions, not a
complete command system. It should reuse the visual language and hard-won input
isolation of the Recent Sessions picker while staying small enough to refine
quickly.

```text
managed Splinterm Window

┌ editor ────────────────┬ logs ────────────────┬ + ┐
│                                                     │
│          ┌ COMMANDS                            ┐    │
│          │ > split_                            │    │
│          ├─────────────────────────────────────┤    │
│          │ › Split pane horizontally   Ctrl+⇧+↵│    │
│          │   Split pane vertically      Ctrl+⇧+\│    │
│          └─────────────────────────────────────┘    │
│                                                     │
└─────────────────────────────────────────────────────┘

right-click tab

┌ editor ─────────────────┬ logs ───────────────┬ + ┐
│                         ┌ editor ─────────────┐    │
│                         │ New Dojo            │    │
│                         │ Close Tab           │    │
│                         └─────────────────────┘    │
└─────────────────────────────────────────────────────┘
```

## Product decisions for the first cut

### Command palette

- Built-in opening chord: `Ctrl+Shift+P`.
- Available only in managed tabbed Windows. Legacy evidence, standalone picker,
  and trusted-consent hosts do not expose it.
- Search is active immediately; no separate focus step.
- Matching is case-insensitive substring matching over a stable command title
  and bounded keyword list. Ranking preserves declaration order in the MVP.
- Arrow keys navigate; Home/End jump; Enter executes; Escape closes; Backspace
  edits. Printable `event.utf8` appends to the bounded query.
- `J` and `K` are ordinary query characters, unlike the Recent Sessions picker.
- The first command set is deliberately limited to existing targeted topology
  operations:

| Command | Captured target | Existing dispatch |
| --- | --- | --- |
| `New Dojo` | active tab's `LairId` | `WindowTopologyCommand::NewDojo` |
| `Split pane horizontally` | active `DojoId` and focused `SplintId` | `WindowTopologyCommand::Split` |
| `Split pane vertically` | active `DojoId` and focused `SplintId` | `WindowTopologyCommand::Split` |

These three actions are enough to validate discovery, filtering, command
metadata, shortcut hints, keyboard execution, modal closure, and one action
whose target is a Lair plus two whose target is a Splint. They require no new
daemon request or protocol version.

The exact `LairId`, `DojoId`, and `SplintId` context is captured when the palette
opens. Execution must never retarget a command merely because asynchronous
state changed behind the overlay. If the target disappears, the existing
explicitly targeted operation may reject cleanly; the palette must not guess a
replacement.

### First post-approval palette expansion

The user approved the initial visual direction and explicitly opened the deferred
expansion gate. The first expanded inventory remains closed and application-owned:

| Category | Commands | Target behavior |
| --- | --- | --- |
| Session | Open recent sessions | Closes and reconciles the palette before requesting the existing trusted picker |
| Tab | New Dojo; Previous Dojo; Next Dojo; Close current tab | Captures exact Lair/current/neighbor Dojo identities; close remains detach-only |
| Pane | Horizontal/vertical split; focus left/right/up/down; close focused pane | Captures the focused Splint and exact directional destinations; close retains existing shell-termination semantics |
| View | Zoom in; zoom out; reset zoom | Reuses the existing client-local Foot-compatible zoom path |

Stable category labels join titles and keywords in case-insensitive filtering.
Unavailable previous/next or directional-focus destinations remain visible but
disabled; keyboard navigation skips them and pointer activation cannot run them.
The initial seven-row viewport, approved geometry, modal isolation, and exact
captured-authority rule remain unchanged.

The balanced post-approval context menu adds `Activate Tab`, horizontal and
vertical split against the clicked tab's captured focused Splint, and detach-only
`Close Other Tabs`. `Activate Tab` is disabled for the active tab; split actions
are disabled when no focused Splint was captured; close-others is disabled when
no other tab was captured. Batch detach carries both the exact retained Dojo and
the exact bounded set of other Dojos, and rejects cleanly if the retained target
disappears before execution.

### Tab context menu

- Open with right-click on a committed visible tab rectangle.
- The menu targets the clicked tab without first activating it.
- First menu actions:

| Action | Captured target | Existing dispatch |
| --- | --- | --- |
| `New Dojo` | clicked tab's `LairId` | `WindowTopologyCommand::NewDojo` |
| `Close Tab` | clicked tab's `DojoId` | `WindowTopologyCommand::CloseTab` |

- `Close Tab` retains current semantics: it detaches the client-local reference
  and never kills the Dojo or its Splints.
- Left-click activates a row only when press and release resolve to the same
  committed menu target.
- Clicking outside dismisses the menu and consumes that click. There is no
  click-through to terminal selection, paste, URLs, mouse reporting, dividers,
  or tabs.
- Right-clicking another visible tab while the menu is open retargets and
  repositions the menu in one consumed interaction.
- Arrow keys navigate; Enter executes; Escape closes. No typeahead is required
  for the two-item MVP.

### Second post-approval tab and palette expansion

The user approved a second bounded expansion after the first expanded slice was
installed. The tab context menu becomes explicitly Dojo/tab-oriented and keeps
six compact rows in this order:

1. `Rename Tab`;
2. `Activate Tab`;
3. `New Dojo`;
4. detach-only `Close Tab`;
5. detach-only `Close Other Tabs`; and
6. destructive `Terminate Dojo…`.

Horizontal and vertical split leave the tab menu and remain discoverable in the
command palette. `Rename Tab` is the primary first row in both surfaces.
Rename opens a trusted Window-local bounded editor prefilled from the exact
captured Dojo identity. The first printable edit replaces the prefilled value;
Backspace edits at Unicode scalar boundaries; Enter submits a nonempty name of
at most 128 UTF-8 bytes; Escape cancels. Controls and bidi-formatting characters
are rejected. Submission carries the captured `DojoId` and never resolves the
currently active tab again.

`Terminate Dojo…` never dispatches directly. It opens a second trusted popup
that names the captured Dojo and its captured pane count. `Cancel` is selected
by default. Arrow/Tab changes the decision, Enter activates the selected button,
and Escape always cancels. Confirmation retains the exact captured
`(SplintId, incarnation)` set, rejects pane-set or incarnation drift, sends
`Request::KillSplint` for each captured live runtime, then sends
`Request::CloseDojo` for the exact captured `DojoId` at the refreshed topology
revision. A disappeared target settles cleanly and must not retarget or expand
to a newly added pane. The popup owns keyboard, pointer, paste, IME, focus-report, selection,
URL, tab, divider, and terminal-mouse input just like the existing action
surfaces.

The command palette expands from 15 to this closed 31-command catalog:

| Category | Added commands |
| --- | --- |
| Session | New session |
| Tab | Rename current tab; Close other tabs; Terminate current Dojo… |
| Pane | Resize pane smaller; Resize pane larger |
| History | Search scrollback; Page up; Page down; Return to live |
| Control | Request control; Release control; Force control transfer; Revoke all access; Accept pending transfer; Deny pending transfer |

The existing 15 Session/Tab/Pane/View commands remain. Availability is captured
when the palette opens: close-others requires captured other tabs; history return
requires a detached viewport; control commands reflect captured controller,
grant, and pending-transfer state; directional and adjacent-tab actions retain
their existing exact destinations. Execution revalidates only the captured
identity or token and never substitutes a newer active target.

Clipboard copy/paste remain outside this catalog because palette activation does
not own the fresh Wayland keyboard serial needed for correct clipboard
ownership. Restore/relaunch/kill-arbitrary-session actions remain deferred until
a separate exact-target picker exists. Shell, plugin, terminal-content, and
automation-provided actions remain prohibited.

### Remaining deferred expansion

The approved expansions do not authorize these broader surfaces:

- close-tabs-to-the-right, duplicate, rename, detach-to-window, or tab reordering
  commands;
- indiscriminately exposing every existing keyboard shortcut;
- nested menus, subcommands, recent-command ranking, fuzzy scoring, aliases
  editable by users, or configurable bindings;
- command extension APIs, plugins, shell commands, terminal-content-provided
  actions, or automation-provided actions;
- persistent palette history;
- animations, blur, glass effects, or a general widget toolkit; and
- daemon, protocol, public CLI, MCP, audit, policy, or topology-schema changes.

## Design direction: one family, two surfaces

The Recent Sessions picker is the visual reference, not a component to clone
wholesale. Extract only narrow shared color and text primitives where doing so
removes real duplication.

### Shared visual language

Both new surfaces should use:

- opaque theme-derived surfaces independent of terminal background alpha;
- the picker's contrast-corrected primary and secondary text;
- crisp one-pixel frames and a restrained offset shadow;
- selected rows identified by fill, accent rail, and `›`, not color alone;
- `ChromeText` shaping and clipping;
- logical-pixel layout converted deterministically by `scale_120`; and
- rectangular, calm TUI-inspired composition without rounded card stacks.

The existing `SessionPickerPalette` color derivation should become a narrowly
named trusted-overlay palette only if the resulting API remains honest for all
three surfaces. Do not create public theme keys for the MVP.

### Command palette geometry

- Position inside the authoritative content rectangle below the tab strip.
- Dim only the content rectangle; leave the trusted tab strip legible.
- Maximum width: 680 logical pixels.
- Preferred top offset: 48 logical pixels from the content origin, clamped for
  compact surfaces rather than vertically centered.
- Header/query area: 52 logical pixels.
- Result row: at least 48 logical pixels, with a 44-pixel minimum hit target.
- Show at most seven rows initially and page the filtered result set around the
  selected result.
- Normal rows show title on the left and the known built-in shortcut on the
  right. Compact mode drops the shortcut before truncating the title.
- An empty query shows all MVP commands. No matches shows one inert, clearly
  worded empty state and keeps Escape available.

Suggested visual copy:

```text
COMMANDS
> query

› Command title                              Shortcut
  Short optional explanation, only if layout proves it useful

↑↓ navigate   Enter run   Esc close
```

The initial implementation should prefer one-line rows. A second metadata line
must be earned by the first visual review rather than assumed.

### Tab context-menu geometry

- Anchor to the pointer release position associated with the clicked tab, then
  clamp the complete menu to the Window bounds.
- Prefer opening below the tab strip. Flip or shift when the right or bottom
  edge would clip it.
- Preferred width: 156 logical pixels; row height: 28. This tighter geometry
  replaces the original 220/44 proposal after the first visual review found the
  two-action menu oversized.
- Show the sanitized target tab label as a quiet header only if it improves
  target clarity in the first implementation. The two actions remain primary.
- Do not dim the Window for the context menu.
- Paint above tab chrome and terminal content as the topmost ordinary trusted
  application surface.

## Architecture

### Keep actions closed and application-owned

Introduce a small closed command identity rather than callbacks, strings, or
terminal-provided data:

```rust
enum BuiltInCommandId {
    NewDojo,
    SplitHorizontal,
    SplitVertical,
}

struct CommandPaletteContext {
    lair_id: LairId,
    dojo_id: DojoId,
    splint_id: SplintId,
}

struct TabMenuContext {
    lair_id: LairId,
    dojo_id: DojoId,
}
```

A pure descriptor projection supplies trusted title, keywords, and optional
shortcut text. A separate exhaustive dispatcher maps identity plus captured
context to the existing `WindowTopologyCommand`. Rendered labels are never
parsed back into authority or targets.

The tab menu may reuse a small `ActionMenuItem` descriptor shape, but it should
not pretend every command is valid in every host. Keep palette filtering,
context-menu targeting, and command execution explicit.

### Platform-neutral interaction state

Add a focused frontend module, likely `frontend/action_menu.rs`, containing pure
state and tests for:

- bounded query editing;
- filtering and stable order;
- selected-visible range reconciliation;
- keyboard navigation;
- hovered and pressed targets; and
- exact command identity independent of rendered text.

Suggested host state:

```rust
enum ActionSurface {
    CommandPalette(CommandPaletteUi),
    TabContextMenu(TabContextMenuUi),
}
```

Only one action surface may be open. It also cannot coexist with the Recent
Sessions picker, trusted consent, history-search input, a control-transfer
prompt, divider drag, pending tab/session switch, or picker reconciliation.
Centralize this exclusivity in one predicate instead of growing unrelated
boolean lists at each shortcut site.

Do not fold trusted consent into this enum. Its security and process-lifetime
contract remains separate.

### Layout and rendering seams

Add specialized pure layout outputs rather than a generic widget tree:

- `CommandPaletteLayout` with panel, query, list, footer, visible range, and row
  hit rectangles;
- `TabContextMenuLayout` with panel, optional header, rows, anchor identity, and
  row hit rectangles;
- `command_palette_layout(...)` and `tab_context_menu_layout(...)`;
- explicit hit-test functions; and
- specialized painters using shared trusted-overlay color/text primitives.

A likely home is `renderer/overlays/actions.rs`. Keep shaped text in bounded
renderer caches keyed by source, width, style, scale, and renderer generation.
Query edits may reshape the short query and changed visible rows, but must not
rebuild terminal `SnapshotFrame`s or clear terminal backing.

### Wayland lifecycle and composition

The command palette is a full input-modal overlay over the content rectangle.
The context menu is visually non-modal but input-capturing while open.

Opening either surface must:

1. settle current terminal pointer/selection ownership using the existing picker
   safety seam;
2. advance the input generation so stale paste completion cannot arrive later;
3. disable text-input-v3 and clear preedit for the MVP;
4. suppress terminal focus reports while the surface owns input;
5. clear incompatible pressed chrome/divider state;
6. capture exact action context; and
7. schedule transient chrome painting without mutating terminal backing.

Closing must restore normal IME/focus handling with at most one reconciled focus
report. Surface configure, scale, theme, and renderer-generation changes
invalidate action hit geometry and text caches. Activation is ignored until a
fresh layout has been painted and committed.

Terminal snapshots and hidden-tab updates continue to drain normally under both
surfaces. Topology changes may advance real frontend state, but cannot rewrite
captured command targets.

Composition order:

1. terminal panes and terminal-local overlays;
2. pane/history chrome;
3. trusted tab strip;
4. command-palette content scrim and panel, when open; then
5. tab context menu, when open.

Mutual exclusion means steps 4 and 5 do not normally coexist, but the ordering
remains explicit.

### Input dispatch order

Keyboard press handling should become:

1. trusted consent;
2. open action surface;
3. Recent Sessions picker;
4. action-surface opening shortcut;
5. existing tab and pane shortcuts;
6. history/search and terminal input.

Pointer frame handling should become:

1. open action surface;
2. Recent Sessions picker;
3. divider drag;
4. tab strip, including right-click menu opening;
5. terminal pointer behavior.

Every owned opening chord/button is consumed on press, repeat, and release. No
partial `Ctrl+Shift+P` bytes or right-click terminal mouse report may leak.

## Dependency-ordered milestones

### Milestone 0 — pure contracts and locked MVP inventory

Files:

- `docs/plans/0025-command-palette-and-tab-context-menus.md`;
- new focused frontend tests; and
- `wayland/input/shortcuts.rs` tests.

Work:

1. Freeze the three palette commands and two tab-menu actions above.
2. Add the closed identities, captured contexts, descriptor projections, pure
   filtering/navigation state, and exact shortcut classification.
3. Bound the query to 256 UTF-8 bytes and 128 Unicode scalars; reject control and
   bidi-formatting characters and edit only at scalar boundaries.
4. Test zero matches, one match, all matches, selection preservation, query
   shrink/growth, and stale captured targets.

Validation:

```bash
cargo test -p splinterm --lib action_menu
cargo test -p splinterm --lib shortcut
cargo fmt --all --check
git diff --check
```

### Milestone 1 — command palette layout and static painter

Files:

- `crates/splinterm/src/renderer/overlays/actions.rs`;
- `crates/splinterm/src/renderer/overlays/mod.rs`;
- narrow picker-palette/text extraction only where justified; and
- renderer tests.

Work:

1. Implement deterministic normal/compact palette geometry below the tab strip.
2. Reuse the picker's theme-derived contrast rules.
3. Shape and cache the header, query, visible command titles, shortcuts, footer,
   and empty state.
4. Paint the static palette over a deterministic backing fixture.
5. Keep all hit rectangles half-open and non-overlapping. Palette rows remain
   at least 44 logical pixels high; the visually reviewed two-action tab menu
   uses compact 28-pixel rows.

Validation:

```bash
cargo test -p splinterm --lib command_palette
cargo test -p splinterm --lib renderer
cargo fmt --all --check
cargo clippy -p splinterm --all-targets -- -D warnings
git diff --check
```

### Milestone 2 — live command palette with three commands

Files:

- `frontend/action_menu.rs`;
- `wayland.rs` and focused `wayland/dispatch/` modules;
- `wayland/input/shortcuts.rs`; and
- `docs/configuration.md` after behavior is real.

Work:

1. Open `Ctrl+Shift+P` only through the centralized availability predicate.
2. Implement query editing, filtering, keyboard navigation, pointer hover,
   same-target activation, outside-panel consumption, and Escape dismissal.
3. Capture exact active Lair/Dojo/Splint context on open.
4. Exhaustively dispatch the three commands through existing topology commands.
5. Preserve modal IME, paste, pointer, focus-report, configure, theme, and
   topology-update safety.
6. Add bounded diagnostics for unavailable or stale execution without leaving a
   stuck overlay.

This is the first coherent implementation checkpoint. Stop here for design
review before adding commands or building the tab menu.

Validation:

```bash
cargo test -p splinterm --lib command_palette
cargo test -p splinterm --lib session_picker
cargo test -p splinterm --lib topology
cargo fmt --all --check
cargo clippy -p splinterm --all-targets -- -D warnings
git diff --check
```

### Milestone 3 — tab context-menu layout and two live actions

Files:

- `renderer/overlays/actions.rs`;
- `wayland/tabs.rs`;
- `wayland/dispatch/pointer.rs`;
- action-surface state/tests; and
- `docs/configuration.md`.

Work:

1. Expose right-button classification only within the Wayland implementation.
2. Open only from a committed visible tab target and capture its exact Lair/Dojo
   identity.
3. Implement anchored/clamped geometry, hover, keyboard navigation,
   same-target activation, outside-click dismissal, and right-click retargeting.
4. Dispatch `New Dojo` and `Close Tab` without activating or retargeting the
   clicked tab first.
5. Ensure every menu pointer event is consumed before divider, tab activation,
   or terminal routing.
6. Verify final-tab closure follows existing Window lifecycle and never gains
   topology-kill semantics.

Stop again for design review before any menu expansion.

Validation:

```bash
cargo test -p splinterm --lib tab_context_menu
cargo test -p splinterm --lib tab_strip
cargo test -p splinterm --lib topology
cargo fmt --all --check
cargo clippy -p splinterm --all-targets -- -D warnings
git diff --check
```

### Milestone 4 — polish only what the review proves necessary

Potential in-scope refinements:

- spacing, panel position, selected-row treatment, shortcut alignment, context
  header presence, compact behavior, and copy;
- exact damage regions after correctness is proven;
- bounded text-cache tuning; and
- documentation/screenshots after separately approved graphical validation.

Do not use this milestone as permission to expand the command inventory.

Final non-graphical validation:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
git diff --check
```

### Milestone 5 — rename and destructive-confirmation contracts

1. Add exact captured Dojo name/id/count prompt state with bounded rename editing.
2. Add `WindowTopologyCommand::RenameDojo` and `TerminateDojo` manager paths
   backed by existing `Request::RenameDojo`, `Request::KillSplint`, and
   `Request::CloseDojo` messages.
3. Add deterministic rename and confirmation layouts, CPU painting, hit testing,
   default-cancel selection, and destructive-state styling.
4. Prove stale/disappeared targets reject without fallback and termination never
   runs before affirmative popup activation.

### Milestone 6 — tab-focused menu and 31-command palette

1. Replace tab-menu split rows with primary rename and confirmed termination.
2. Add the 16 approved palette commands and exact captured availability state.
3. Reuse existing local search/history/control/ratio behavior rather than
   synthesizing terminal key input.
4. Preserve committed geometry, same-target pointer activation, modal isolation,
   IME/focus reconciliation, and bounded text caches across nested prompts.

### Milestone 7 — expanded closure

Run focused action/prompt/renderer/topology tests, the full workspace validation
ladder, fresh read-only review, and separately approved isolated graphical smoke.
Do not install or advertise this expansion until confirmation default-cancel,
rename bounds, inactive-tab exact targeting, and daemon termination semantics
all have recorded evidence.

## Non-graphical test contract

Focused tests must cover:

- exact `Ctrl+Shift+P` classification, repeat/release consumption, and blocking
  states;
- query byte/scalar bounds, Unicode scalar deletion, control/bidi sanitation,
  case-insensitive matching, stable ranking, and empty results;
- selection staying visible across filter and viewport changes;
- command dispatch retaining captured Lair/Dojo/Splint identity;
- normal, compact, narrow, and short layouts at scales 120, 150, and 240;
- clamped context menus at all four Window edges;
- half-open non-overlapping hit targets;
- press-inside/release-outside cancellation;
- outside-click dismissal without click-through;
- right-click retargeting without tab activation;
- no keyboard, paste, selection, URL, IME, terminal mouse, history-wheel, tab,
  or divider input leakage;
- configure/scale/theme/renderer-generation layout invalidation;
- current terminal and hidden-tab updates continuing beneath transient chrome;
- no terminal frame/backing rebuild for selection-only movement; and
- final-tab close retaining existing detach-only semantics.

## Graphical validation boundary

This plan does not authorize graphical testing. After Milestone 2 is complete
and non-graphical validation passes, request approval for one guarded isolated
smoke on workspace 8 / `DP-2` before any matrix.

The first smoke should test only:

1. open palette with `Ctrl+Shift+P`;
2. type `split`, navigate, and dismiss without execution;
3. reopen and execute one split command;
4. verify terminal input did not receive the query or chord;
5. verify placement, focus preservation, and cleanup.

After Milestone 3, a separately approved context-menu smoke should test:

1. right-click active and inactive tabs;
2. outside-click dismissal;
3. `New Dojo` against an inactive tab's Lair;
4. `Close Tab` without Dojo/Splint termination; and
5. wrong-target, focus, placement, and cleanup abort conditions.

Only after both smokes pass should an approved matrix cover compact/normal
sizes, scales 120/150/240, dark/light/translucent themes, edge clamping, multiple
tabs, keyboard-only operation, and pointer press/release cancellation.

## Expanded-slice closure evidence

The post-approval palette and balanced context-menu expansions were validated on
2026-08-06 from commit `61827ff`. Durable screenshots and the bounded validation
report are stored under
[`artifacts/0025-action-menus/`](artifacts/0025-action-menus/README.md).

Non-graphical closure passed `cargo test --workspace`,
`cargo fmt --all --check`, and `git diff --check`. Strict Rust 1.97
`cargo clippy --workspace --all-targets -- -D warnings` remains blocked by the
recorded pre-existing workspace warning set; the run found no action-menu-specific
correctness failure.

The authorized isolated release-profile sequence on workspace 8 / `DP-2`
recorded:

1. bounded `split` search, Escape dismissal, and a horizontal split that changed
   isolated topology from one running Splint to two;
2. inactive- and active-tab six-action menus, including disabled `Activate Tab`
   selection skipping on the active tab;
3. outside-click dismissal without topology mutation;
4. horizontal split against the inactive tab's captured Splint while the second
   tab remained active; and
5. `Close Other Tabs` settling to one retained client tab while the isolated
   daemon retained all three captured Dojos and four running Splints.

The last result records detach-only semantics rather than a daemon kill. The
isolated daemon, client, socket, and virtual pointer were removed, and the
pre-test Foot focus and cursor position were restored. A fresh read-only source
review reported no source defect or fix worth doing now; its formal attestation
was rejected only because its sandbox could not read the supplied `/tmp`
manifest/diff artifacts, and that evidence-delivery limitation remains explicit
in the durable report.

Publication and installation completed later on 2026-08-06. Commits `61827ff`
and `b515caa` were pushed to `origin/main`; clean committed `b515caa` produced the
validated split package set. With explicit authorization, the running production
daemon was stopped, terminating its 14 running Splints across two active Lairs,
and matched `splinterm` and `splinterm-mcp` packages were installed through
Pacman. Post-install verification recorded:

- package/installed `splinterm` SHA-256
  `78ad6e24171b713946f6f154cc93bd51d845c883cd68eb180c776f9e580a51e7`;
- package/installed `splinterd` SHA-256
  `994681a520a0c922faa13d2803a5f8b0b5538105e0d55fc65dd5cf085212ff60`;
- package/installed `splinterm-mcp` SHA-256
  `d747bd615731d2d68d00608fccc2200094ee60b2e2f4c1823f69b6de2d5c0819`;
- active `splinterd.service`, with the running executable device/inode matching
  `/usr/bin/splinterd`;
- `/usr/bin/splinterm` and `/usr/bin/splinterm-mcp` command resolution;
- `pacman -Qkk splinterm splinterm-mcp`: zero altered files;
- valid `com.oldjobobo.splinterm.desktop`; and
- normal human-mode `/usr/bin/splinterm list`: no active Lairs.

The pre-install rollback is
`~/.local/state/splinterm/rollback/20260806-194513-pre-b515caa`.

## Second-expansion implementation record

The second approved tab-focused/31-command expansion was implemented on
2026-08-07 without touching the unrelated image-spike benchmarking work.
Non-graphical validation recorded:

- `cargo check -p splinterm --all-targets` passed;
- `cargo test -p splinterm --lib`: 268 passed, 1 ignored;
- `cargo clippy -p splinterm --all-targets` passed with the existing warning
  baseline and no newly reported warning in the changed action paths;
- `cargo test --workspace -- --test-threads=1` passed;
- `cargo fmt --all --check` and `git diff --check` passed; and
- two fresh read-only reviews (protocol/topology and modal/UI lifecycle) reported
  no blocker or fix worth doing now.

A parallel workspace run first exposed one timing-sensitive unchanged MCP stdio
assertion; the exact test passed immediately in serialized isolation, and the
subsequent serialized full workspace run passed.

Separately authorized isolated graphical validation on workspace 8 / `DP-2`
recorded the searchable 31-command catalog, prefilled rename, the exact six-row
inactive-tab menu, default-cancel termination, and inactive-tab targeting without
activation. The first affirmative attempt exposed that `Request::CloseDojo`
rejects a Dojo containing live Splints. The corrected command therefore captures
exact `(SplintId, incarnation)` pairs, revalidates the complete set before every
serial kill, kills only captured live runtimes, refreshes topology, and closes the
exact Dojo with bounded stale/not-found reconciliation. A later pass exposed
stale tab-strip pixels after successful inactive-tab removal; explicit tab-label
cache invalidation and a full redraw fixed the final frontend reconciliation.

The final guarded case removed the exact inactive two-pane Dojo and both captured
Splints, retained the active Dojo and running Splint, and repainted to exactly one
visible tab. Durable evidence is stored under
[`artifacts/0025-action-menus/second-expansion/`](artifacts/0025-action-menus/second-expansion/README.md).
A fresh read-only review found no captured-identity or default-cancel blocker and
requested bounded drift/race hardening; those findings were applied before the
final full workspace and graphical passes. Every isolated process was removed,
production daemon PID `3194395` remained active, and the original Foot focus and
cursor were restored. Publication, packaging, and installation have not been
authorized or performed for this second expansion.

## Stop gates

Stop for user input before:

- changing the opening binding or making bindings configurable;
- adding actions beyond the closed 31-command palette and six-row tab menu;
- exposing shell, plugin, terminal-content, automation, or requester-provided
  commands;
- adding further destructive Dojo/Splint lifecycle operations to either surface;
- changing tab close from detach-only behavior;
- enabling IME composition inside palette search rather than the bounded direct
  UTF-8 MVP path;
- introducing a generic widget, popup, menu, or command-extension framework;
- changing daemon/protocol/public automation contracts;
- adding graphical effects or animation; or
- running graphical validation without explicit approval.

## Acceptance criteria

The initial feature slice was accepted when:

1. `Ctrl+Shift+P` opens a polished searchable palette below the trusted tab
   strip in a managed terminal Window.
2. The palette initially exposed exactly the three approved commands and
   dispatched them to captured stable identities through existing topology
   commands; the later approved expansion preserves that targeting rule.
3. Right-clicking a visible tab opens a clamped trusted menu targeting that tab,
   with exactly `New Dojo` and `Close Tab`.
4. Context actions never activate or retarget a different tab implicitly.
5. Neither surface leaks keyboard, pointer, paste, IME, URL, selection, history,
   divider, or terminal mouse input.
6. Selection and activation use explicit committed hit geometry and remain
   identifiable without color.
7. Theme, scale, configure, and async frontend updates reconcile without stale
   hits or terminal-frame replacement.
8. Normal and compact surfaces remain usable at supported scales.
9. Focused and workspace-wide non-graphical validation passes.
10. Separately authorized graphical evidence records both guarded smokes before
    any expanded matrix.
11. The two explicit design-review stops occur before command/menu expansion.
12. Independent review reports no blocker or fix worth doing now, with recorded
    validation and review evidence as required by `AGENTS.md`.
13. The second approved expansion exposes exactly the documented 31-command
    catalog and six-row tab-focused context menu.
14. Rename is prefilled, UTF-8 bounded, control/bidi sanitized, nonempty, and
    dispatches only the captured `DojoId`.
15. Termination always opens a named confirmation with captured pane count and
    Cancel selected by default; only affirmative activation kills the exact
    captured Splint incarnations and then sends `Request::CloseDojo`.
16. Detach-only close actions remain distinct from daemon-side Dojo termination.
17. History and control actions reject a changed focused `SplintId`; pending
    transfer decisions retain the captured transfer token and access revocation
    retains the captured grant set.
18. Clipboard and arbitrary saved-session actions remain excluded.
