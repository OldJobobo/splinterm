# Plan 0033: Alpha3 command-palette and keymap closure

- **Status:** Implemented, non-graphically validated, and reviewed for `0.1.0-alpha3`; packaged graphical acceptance pending
- **Date:** 2026-08-12
- **Product authority:** The command palette remains a closed, trusted,
  application-owned catalog; resolved keymaps remain the authority for active
  shortcuts and their displayed labels
- **Depends on:** accepted command menus from Plan 0025 and accepted configurable
  keymaps and Dojo presets from Plan 0027

## Decision

Close the confirmed everyday-UI gaps between Splinterm's trusted command
palette, configurable action vocabulary, built-in `splinterm` and
`omarchy-tmux` keymaps, runtime dispatch, documentation, and validation before
publishing `0.1.0-alpha3`.

This is a correctness and discoverability closure, not an extensibility project.
Terminal output, plugins, presets, configuration files, and automation clients
must not register trusted palette commands.

## Confirmed baseline gaps

The alpha3 audit found:

1. `ActionId` and the palette's `BuiltInCommandId` catalog are maintained
   separately, with substantially more configurable actions than palette rows.
2. `dojo.close-other-tabs` has an application action and palette command but is
   omitted from the bindable registry and keyboard dispatch.
3. several implemented daily-driver actions are available through the richer
   Omarchy profile but are absent from the palette and therefore difficult to
   discover from the default Splinterm profile;
4. built-in keymap tests prove representative chords rather than an exhaustive
   binding-to-action-to-runtime matrix;
5. no invariant prevents future palette/configuration/runtime drift; and
6. usage documentation describes pane resizing as palette-only even though the
   default profile has direct resize bindings.

Every existing palette row has typed availability and dispatch handling. The
problem is incomplete registry alignment and discoverability, not a known dead
palette row.

## Alpha3 scope

### 1. Close the `dojo.close-other-tabs` wiring defect

- add `CloseOtherTabs` to the configurable/bindable action registry;
- route the resolved action through the same exact-target tab-close behavior as
  palette activation;
- preserve the non-destructive contract: closing Window-local tabs must not
  terminate their Dojos or Splints;
- expose the resolved shortcut label in the palette when a legal overlay binds
  the action; and
- add parser, overlay, runtime, stale-target, and documentation coverage.

No default chord is required if adding one would create a conflict. The action
must nevertheless be legal to bind and behave identically from keyboard and
palette paths.

### 2. Expand the curated command palette

Add trusted commands for the highest-value actions that are already implemented:

- Show Keybindings;
- Reload Configuration;
- Enter Copy Mode;
- Toggle Focused Pane Zoom;
- Choose Dojo;
- Move Dojo Left;
- Move Dojo Right;
- Choose Lair;
- Rename Current Lair;
- Previous Lair;
- Next Lair;
- Terminate Current Lair; and
- Detach Window.

Each command must have:

- a stable built-in command ID and human label;
- appropriate search terms;
- typed availability and dispatch;
- the resolved primary shortcut label when bound;
- modal input isolation and safe cancellation where applicable;
- exact captured-target behavior for resource-sensitive actions; and
- focused tests for enabled, disabled, stale, and dispatched states.

Showing **Show Keybindings** in the palette provides a default-profile path to
resolved binding help without inventing a conflicting global chord. A later
product decision may add another default chord if evidence shows it is needed.

### 3. Prove configurable-keymap behavior end to end

Add non-graphical, table-driven coverage that proves:

1. every binding in the built-in `splinterm` profile resolves to its declared
   `ActionId`;
2. every binding added or made primary by `omarchy-tmux` resolves to its declared
   `ActionId`, including prefix bindings;
3. every bindable action has a recognized runtime route or an explicit,
   reviewed reason for being modal-local;
4. a strict custom overlay can add, replace, and unbind representative direct
   and prefix bindings, and the resulting action reaches the same runtime
   dispatch as its built-in equivalent;
5. conflict, malformed-input, and unknown-action failures remain transactional
   and retain the last-known-good keymap;
6. configuration reload updates runtime resolution and palette/help shortcut
   labels together; and
7. focus loss, modal entry, timeout, and reload clear armed prefix state.

The tests should use pure resolution and extracted dispatch reducers wherever
possible. They must not require graphical input injection merely to prove the
closed action matrix.

### 4. Share safe Super clipboard/edit shortcuts

Make the Omarchy-style desktop shortcuts available in the default `splinterm`
profile wherever their meaning is safe and unambiguous:

| Context | `Super+C` | `Super+V` | `Super+X` | `Super+Z` |
| --- | --- | --- | --- | --- |
| Normal terminal pane | copy terminal selection | safe/bracketed clipboard paste | pass through to terminal application | pass through to terminal application |
| Splinterm-owned editable field | copy selected field text | paste bounded text | cut selected field text | bounded local undo |
| Copy mode | copy the active copy-mode selection through the established clipboard path | no paste into PTY | no destructive edit | no unrelated undo |

Requirements:

- move `Super+C` and `Super+V` into the shared built-in base keymap so
  `splinterm` and `omarchy-tmux` resolve them identically without duplicate
  bindings;
- preserve `Ctrl+Shift+C/V` aliases in the default profile;
- keep owned-field `Super+C/V/X/Z` context-local and available under both
  profiles;
- never claim terminal-pane `Super+X/Z` as universal cut/undo, because those
  chords belong to the foreground terminal application when no Splinterm-owned
  field is active;
- ensure copy-mode desktop shortcuts cannot leak bytes or paste into the PTY;
- preserve modal precedence, clipboard generation checks, bounded field undo,
  Unicode-safe selection, safe/bracketed paste, and terminal selection
  ownership; and
- document that compositor-reserved Super chords work only when the compositor
  delivers them to the Splinterm Window.

Add a table-driven context/profile matrix for press, repeat, release, clipboard
availability, empty/nonempty selection, stale clipboard generation, owned-field
closure, copy-mode exit/cancellation, and terminal passthrough.

### 5. Add drift-prevention invariants

Add tests or static assertions that fail when:

- palette command IDs or labels are duplicated or empty;
- a palette shortcut action is neither bindable nor explicitly documented as
  palette-only;
- a palette row lacks availability or typed dispatch behavior;
- a built-in binding resolves to a different action than its profile declares;
  or
- documented built-in profile/action inventories diverge from the effective
  registry.

The palette is a **curated subset** of the closed action registry, not an
exhaustive action browser. Intentional exclusions must be explicit in tests or
near the catalog rather than accidental omissions.

### 6. Reconcile user documentation

Update the configuration and usage documentation to:

- describe the palette as a curated trusted catalog using the same action IDs
  and resolved shortcut labels as keymaps;
- list `dojo.close-other-tabs` as bindable;
- document the newly discoverable palette commands;
- document the shared `Super+C/V` terminal aliases and context-local owned-field
  `Super+C/V/X/Z` behavior without mislabeling terminal `Super+X/Z`;
- correct the default pane-resize shortcut guidance;
- distinguish the default `splinterm` profile from `omarchy-tmux` additions;
- explain that generated binding help reflects the effective resolved keymap;
  and
- keep preset topology creation separate from keymap action dispatch.

Remove fixed palette-command counts from current product claims unless a test
updates them from one authoritative catalog.

## Explicitly outside alpha3

- user-, plugin-, shell-, terminal-output-, or automation-defined trusted
  palette commands;
- binding arbitrary named presets or shell commands to keys;
- launching arbitrary presets from the trusted command palette;
- palette rows for numeric Dojo selection 1–9;
- palette rows for raw `terminal.send-prefix`;
- capturing terminal-pane `Super+X/Z` as application-wide cut/undo;
- separate palette rows for all four exact five-cell directional resizes;
- a new compatibility promise for tmux configuration or plugins;
- palette categories, plugin APIs, fuzzy-ranking redesign, or other broad menu
  polish; and
- graphical automation beyond the separately approved guarded acceptance below.

Clipboard commands and low-level control-transfer actions remain eligible for a
later curated-menu decision; their omission is not an alpha3 blocker because
existing direct controls remain documented and operational.

## Validation milestones

### Milestone 1 — registry and keyboard correctness

- close the `CloseOtherTabs` bindable/runtime gap;
- share safe `Super+C/V` terminal aliases across built-in profiles and prove the
  context-local `Super+C/V/X/Z` matrix;
- add registry invariants;
- pass focused keymap, configuration, shortcut, and tab lifecycle tests;
- run formatting, strict Clippy for affected crates, and `git diff --check`.

### Milestone 2 — palette discoverability

- add the bounded command set and typed dispatch paths;
- prove availability, stale-target behavior, shortcut projection, and modal
  isolation non-graphically;
- reconcile usage/configuration/PRD/status claims;
- run affected-crate tests and independent read-only review.

### Milestone 3 — built-in and custom-keymap closure

- pass the exhaustive built-in `splinterm` and `omarchy-tmux` binding matrix;
- pass representative strict-overlay add/replace/unbind/reload tests;
- pass one serial workspace validation on the coherent alpha3 release state;
- build release packages with package tests disabled only after that serial run.

### Milestone 4 — packaged graphical acceptance

After separate approval under the repository graphical-testing rules, use the
installed adjacent trusted client and daemon in one guarded sequence to verify:

1. the command palette displays and dispatches the newly added everyday actions;
2. shortcut labels match the effective `splinterm` profile;
3. Show Keybindings displays the effective resolved help rows;
4. a bounded custom overlay changes one safe test binding and both dispatch and
   displayed labels update after reload;
5. the `omarchy-tmux` profile exposes and dispatches representative direct and
   prefix actions, including binding help, copy mode, Dojo/Lair navigation, and
   pane zoom;
6. both profiles support terminal `Super+C/V`, owned-field `Super+C/V/X/Z`, and
   safe copy-mode behavior while terminal `Super+X/Z` remain application-owned;
7. close-other-tabs remains Window-local and non-destructive; and
8. all temporary configuration, topology, windows, focus, workspace, monitor,
   and geometry are restored or explicitly reported.

Abort on wrong-window input, unexpected focus movement, unrelated topology
mutation, configuration rollback failure, or incomplete cleanup.

## Implementation evidence (2026-08-13)

- `dojo.close-other-tabs` is part of the closed bindable registry and routes
  keyboard and palette activation through the same exact-target, Window-local,
  non-destructive tab behavior.
- The curated palette includes the bounded everyday command set for effective
  keybinding help, configuration reload, copy mode, pane zoom, Dojo and Lair
  navigation/management, saved-Lair actions, and Window detachment. Availability,
  shortcut projection, stale-target handling, and typed dispatch have focused
  coverage.
- The built-in `splinterm` and `omarchy-tmux` profiles, prefix bindings, strict
  overlay add/replace/unbind/reload behavior, and catalog/registry drift
  invariants are exercised non-graphically.
- Terminal `Super+C/V`, Omarchy's terminal-tagged Insert translation,
  owned-field `Super+C/V/X/Z`, copy-mode isolation, and terminal-pane
  `Super+X/Z` passthrough are covered under both profiles.
- Usage, configuration, PRD, and status authorities describe the resolved
  keymap, curated palette, shared clipboard controls, and corrected pane-resize
  guidance.
- On the coherent pre-release worktree, `cargo fmt --all --check`, strict
  workspace Clippy, `cargo test --workspace`, release/package tooling tests,
  site check/build, shell validation, and `git diff --check` pass.
- A fresh read-only correctness review confirmed the bindable exact-target
  close-other-tabs route, closed typed action/keymap matrix, and evidence scope,
  with no blockers or fixes worth doing now. Optimized installed-package
  graphical acceptance remains required before closure.

## Alpha3 acceptance

Plan 0033 is complete only when:

- the confirmed `dojo.close-other-tabs` defect is closed;
- the bounded palette additions are implemented and documented;
- built-in and configurable keymaps pass the non-graphical end-to-end matrix;
- the context-sensitive `Super+C/V/X/Z` matrix passes under both built-in
  profiles without capturing terminal-pane `Super+X/Z`;
- registry drift invariants pass;
- focused and serial validation evidence is recorded;
- a fresh read-only review has no unresolved blockers; and
- separately approved packaged graphical acceptance is recorded.

Candidate construction and publication remain separate release operations. This
plan does not authorize installation, graphical testing, pushing, candidate
dispatch, promotion approval, AUR publication, or release publication.
