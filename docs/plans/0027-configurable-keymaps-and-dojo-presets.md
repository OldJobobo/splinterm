# Plan 0027: Configurable keymaps and Dojo presets

- **Status:** Active — Milestones 1–3 complete; Milestone 4 next
- **Date:** 2026-08-07
- **Depends on:** [Plan 0018](0018-lair-dojo-topology-migration.md), [Plan 0019](0019-dojo-tabs.md), [Plan 0025](0025-command-palette-and-tab-context-menus.md)
- **Primary compatibility reference:** [Omarchy Tmux Reference](../omarchy-tmux-reference.md)

## Goal

Make Splinterm feel immediately familiar to an Omarchy/tmux user without turning
the terminal into a tmux emulator or weakening Splinterm's explicit topology,
input-isolation, and process-launch contracts.

This plan delivers three related systems:

1. **Configurable keymaps** with strict parsing, conflict detection, prefix-key
   sequences, generated shortcut labels, and a built-in `omarchy-tmux` profile.
2. **Dojo presets** that create complete, named, persistent pane layouts from
   built-in or user-authored definitions without a chain of partially committed
   splits.
3. **Optional shell compatibility functions** for `t`, `tdl`, `tds`, `tdlm`,
   `tsl`, `ic`, `ix`, and `icx`, backed by Splinterm presets rather than tmux.

The intended result is that a DHH/Omarchy user can enable one profile, retain the
important Splinterm-specific commands, use familiar pane/window/session muscle
memory, and create the familiar development layouts with deterministic direct
process launches.

## Product translation

The domain mapping is exact and should be used consistently in code, docs, help,
and migration messages:

| tmux concept | Splinterm concept | Persistence and UI meaning |
| --- | --- | --- |
| server | `splinterd` | daemon that owns process and topology state |
| session | Lair | named persistent collection of Dojos |
| window | Dojo | named persistent layout, normally presented as a tab |
| pane | Splint | one terminal/process leaf in a Dojo layout |
| attached client | Window | disposable native client view over one or more Dojos |

A keymap profile changes input behavior. A Dojo preset creates topology. These
must remain separate concepts in configuration and implementation.

## Success criteria

The plan is complete when all of the following are true:

- Existing users receive the current Splinterm bindings with no configuration
  changes.
- `profile=omarchy-tmux` provides both `Ctrl+Space` and `Ctrl+B` prefixes and the
  direct Omarchy bindings documented in `docs/omarchy-tmux-reference.md`.
- Shortcut hints in the command palette and help surfaces are generated from the
  resolved keymap; no UI claims a stale hard-coded chord.
- A malformed custom keymap never partially captures keyboard input.
- Prefix state cannot leak across focus loss, modal entry, tab changes, config
  reload, or timeout.
- Built-in `tdl`, `tds`, `tdlm`, and `tsl` equivalents create coherent Dojo
  layouts with the documented working directories, commands, names, ratios,
  and focus.
- User-defined static layouts support arbitrary bounded binary pane trees.
- User-defined parameterized layouts support optional panes and bounded tiled
  repetition without evaluating shell source.
- Multi-pane and multi-Dojo preset materialization commits one topology revision
  or no topology revision; users never inherit half a preset because the third
  split failed.
- Existing daemon authority, automation roles, stable IDs, and direct-argv
  launch validation remain intact.
- Shell compatibility aliases/functions are opt-in and never overwrite an
  existing user alias or function silently.

## Non-goals

- Parse `~/.config/tmux/tmux.conf` or execute tmux commands.
- Import TPM plugins, tmux status-bar configuration, hooks, formats, or copy-mode
  scripts.
- Treat environment variables, shell aliases, names, rendered labels, or preset
  files as authority.
- Execute preset command strings through `sh -c` implicitly.
- Let terminal output register actions or keybindings.
- Make arbitrary user actions appear as trusted built-ins in the command palette.
- Change the public machine/automation contract in the first keymap milestone.
- Replace Hyprland or Omarchy desktop keybindings.
- Modify the canonical Foot oracle or terminal escape behavior.

## Current state and required seams

### Configuration

`crates/splinterm/src/config.rs` currently parses a deliberately small INI
subset. It recognizes `[key-bindings]` only to emit a diagnostic saying bindings
are not remappable. `AppConfig` has no keymap or preset fields. The default file
at `config/splinterm/config.ini` documents hard-coded bindings.

Required change:

- keep `config.ini` as the top-level compatibility and selection file;
- add strict dedicated TOML loaders for structured keymaps and presets;
- replace ignored `[key-bindings]` handling with profile/file selection;
- add a `[presets]` selection section;
- add local validation/inspection commands before enabling live reload.

### Keyboard classification and dispatch

`crates/splinterm/src/wayland/input/shortcuts.rs` contains pure hard-coded
classifiers for command palette, sessions, tabs, pane topology, pane focus, and
font zoom. Clipboard and several history/control chords are classified directly
in `crates/splinterm/src/wayland/dispatch/keyboard.rs` or modal handlers.

Required change:

- replace the family of chord-specific classifiers with one resolved keymap;
- preserve a closed `ActionId` enum and exhaustive dispatcher;
- keep modal-local keys separate from global keymaps;
- centralize press/repeat/release ownership;
- add a prefix state machine without parsing rendered strings.

### Command descriptors

`crates/splinterm/src/frontend/action_menu.rs` stores shortcut labels as
`&'static str` in `BUILT_IN_COMMANDS`.

Required change:

- descriptors retain stable command identity, title, category, and keywords;
- shortcut display becomes a projection from `ResolvedKeymap`;
- multiple chords may be displayed in priority order;
- compact rendering may show only the first chord, while help/CLI inspection
  shows all chords.

### Topology creation

`WindowTopologyCommand` in `crates/splinterm/src/frontend/topology.rs` and
`run_topology_manager` in `crates/splinterm/src/app/topology_manager.rs` support
one Lair, Dojo, or split mutation at a time. A split launches a default shell in
`env::current_dir()`, not necessarily the focused Splint's directory. The core
layout is already a binary `LayoutNode`, but no request creates a complete tree.

Required change:

- resolve cwd from an exact captured Splint or explicit CLI `--cwd`;
- compile preset definitions into a complete bounded launch tree;
- add an atomic daemon request for one or more new Dojos;
- return stable IDs mapped to preset pane names;
- reconcile the resulting committed topology through the existing manager.

### Process launch

`LaunchParameters` already carries cwd, direct argv, shell choice, login-shell
policy, and scrollback size. This is the correct primitive. Presets must compile
to `LaunchParameters`; they must not type commands into a shell pane.

## Configuration architecture

### Top-level `config.ini`

Add these supported keys:

```ini
[key-bindings]
# Current behavior remains the default.
profile=splinterm
# Optional user overlay. Relative paths resolve from the Splinterm config dir.
file=keybindings.toml
# Valid range: 250..=5000.
prefix-timeout-ms=1000

[presets]
# Optional user definitions merged over bundled presets by fully qualified name.
file=presets.toml
# Dangerous built-in command aliases remain disabled until explicitly enabled.
allow-unrestricted-commands=no
```

Rules:

- Built-in profiles are `splinterm` and `omarchy-tmux`.
- Missing optional files mean “no user overlay,” not startup failure.
- An unreadable explicitly configured file is an error.
- Unknown profile names are errors with available names listed.
- `SPLINTERM_CONFIG` continues to select the top-level INI. Relative keymap and
  preset paths are resolved relative to that INI's directory, not process cwd.
- The selected files are client configuration. The daemon never reads arbitrary
  user keymap or preset paths.

### Keymap file: `keybindings.toml`

Use a versioned, ordered, strict format:

```toml
version = 1
inherits = "omarchy-tmux"

[[unbind]]
sequence = ["Prefix", "k"]

[[binding]]
sequence = ["Prefix", "k"]
action = "dojo.terminate-confirmed"

[[binding]]
sequence = ["Ctrl+Alt+Shift+Left"]
action = "pane.resize-left-5"

[[binding]]
sequence = ["Ctrl+Shift+G"]
action = "preset.open"
preset = "personal.review"
```

Parsing rules:

- `deny_unknown_fields` at every level.
- `version` is required and initially exactly `1`.
- `inherits` may name one built-in profile only; user-file inheritance chains are
  not allowed in v1.
- `sequence` has one direct chord or exactly two entries. `Prefix` is valid only
  as the first entry of a two-entry sequence.
- Built-in profiles define the actual prefix chords. The Omarchy profile expands
  `Prefix` to both `Ctrl+Space` and `Ctrl+B`.
- Modifiers use canonical names `Ctrl`, `Shift`, `Alt`, and `Super`. Accepted key
  names use a bounded XKB-facing vocabulary with documented aliases such as
  `Enter`, `Escape`, `Tab`, `PageUp`, `PageDown`, `Left`, `KP_Add`, digits, and
  printable ASCII punctuation.
- Letter case does not imply Shift; `Shift` must be explicit. The profile data
  encodes Omarchy's capital-letter shorthand as `Shift+C`, `Shift+K`, and so on.
- Shifted printable punctuation has canonical aliases: `?` normalizes to
  `Shift+/` and `&` normalizes to `Shift+7`. Supplying both the symbol alias and
  an extra `Shift` is a duplicate-modifier error. Human help renders the familiar
  `?` and `&` symbols, while conflict diagnostics include the normalized chord.
- Duplicate modifiers, empty chords, modifier-only second keys, sequences longer
  than two, and unknown keys are errors.
- Bindings are normalized before conflict detection.
- `unbind` must match an inherited normalized sequence. An unbind that matches
  nothing is a diagnostic, not silent success.
- Two enabled actions may not own the same normalized sequence in the same
  scope. Startup reports both source locations.
- Prefix chords may not also be direct bindings.
- Modal-local keys (`Escape`, navigation, confirmation keys, rename editor keys,
  consent keys while their modal is active) remain owned by their modal and are
  not globally remappable in v1.

### Preset file: `presets.toml`

Use named nodes rather than deeply nested inline TOML. A static example:

```toml
version = 1

[commands.editor]
kind = "editor-env"
fallback = ["nvim"]
append = ["."]

[commands.review]
kind = "argv"
argv = ["codex", "-a", "on-request"]

[presets.personal-review]
kind = "dojo"
display-name = "Review workspace"
name = "{cwd.basename}"
root = "main"
focus = "editor"

[presets.personal-review.nodes.main]
type = "split"
orientation = "columns"
ratio = 650
first = "editor"
second = "review"

[presets.personal-review.nodes.editor]
type = "pane"
command = "editor"
cwd = "{cwd}"
title = "editor"

[presets.personal-review.nodes.review]
type = "pane"
command = "review"
cwd = "{cwd}"
title = "review"
```

Terminology and validation:

- `orientation="columns"` means left/right and compiles to
  `Axis::Horizontal`.
- `orientation="rows"` means top/bottom and compiles to `Axis::Vertical`.
  User-facing formats never use ambiguous “horizontal split” wording.
- `ratio` is the first child's share in thousandths, `1..=999`.
- `root` and `focus` must resolve to existing nodes/panes.
- Node names are bounded ASCII identifiers. Preset display names remain bounded
  UTF-8 labels.
- The node graph must be one acyclic tree: no cycles, reused children,
  unreachable nodes, or more than 32 levels.
- A user preset is limited to 32 panes even though durable topology has a larger
  global bound. This matches one Window's practical tab/pane management budget
  and prevents accidental process storms.
- A pane either references a command or requests `shell=true`, never both.
- `argv` is a nonempty direct argv array. It is never shell-evaluated.
- `editor-env` parses `$EDITOR` with the closed compatibility lexer specified
  below, appends configured arguments, and falls back to the given direct argv.
- `{cwd}` and `{cwd.basename}` are the only v1 string placeholders. Expansion
  occurs before protocol construction and is bounded.

#### Closed compatibility command lexer

Use one project-owned lexer for `$EDITOR` and quoted compatibility command
strings accepted by `tsl`; do not delegate to a shell or a provider-specific
parser.

Grammar and failures:

- ASCII space and tab separate arguments outside quotes. Leading/trailing
  separators are ignored.
- Single quotes preserve every enclosed byte except NUL/newline until the next
  single quote. A single quote cannot be escaped inside single quotes.
- Double quotes preserve enclosed spaces. Inside double quotes, backslash may
  escape only `"` and `\\`.
- Outside quotes, backslash escapes exactly the next non-newline byte.
- `''` and `""` preserve an empty argument.
- Unclosed quotes, trailing backslash, empty input, empty `argv[0]`, NUL, CR, LF,
  and non-UTF-8 input are hard errors.
- After unquoting, reject shell-evaluation metacharacters anywhere in an
  argument: `$`, backtick, `|`, `&`, `;`, `<`, `>`, `*`, `?`, `[`, `]`, `#`,
  `(`, `)`, `{`, and `}`. Reject a leading `~` as implicit home expansion.
- There are no comments, variable/command/arithmetic expansion, redirects,
  pipes, control operators, globbing, brace expansion, tilde expansion, or
  process substitution. Rejected syntax is never treated literally.
- Report the byte offset and error class without echoing secrets from the full
  environment value.

The output is a direct argv vector subject to the same argument-count, per-item,
and aggregate-byte bounds as ordinary `LaunchParameters`. Tests use the same
lexer for `$EDITOR` and `tsl` compatibility strings so the two paths cannot
drift.
- cwd must resolve to an existing absolute directory. Relative pane cwd values
  resolve beneath the invocation's captured root cwd.
- User command aliases shadow bundled aliases only inside that user's preset
  file. They do not create shell aliases or global command-palette entries.

### Parameterized user presets

After static presets are accepted, add two bounded node forms without changing
v1's direct-execution rule:

```toml
[[presets.personal-swarm.parameter]]
name = "count"
type = "integer"
min = 1
max = 16
required = true

[[presets.personal-swarm.parameter]]
name = "command"
type = "command"
required = true

[presets.personal-swarm.nodes.root]
type = "grid"
count = "{count}"
pane-command = "{command}"
```

An optional pane uses `when-parameter="ai2"`. If the parameter is absent, the
compiler removes that leaf and collapses its now-unary branch. A `grid` node is
compiled into a deterministic balanced binary tree. No general expression
language, loops, includes, or embedded scripting are allowed.

## Resolved keymap model

Add a platform-neutral module, likely `crates/splinterm/src/keymap.rs`:

```rust
struct KeyChord {
    modifiers: ModifierMask,
    key: KeyIdentity,
}

struct KeySequence {
    first: SequenceStart,
    second: Option<KeyChord>,
}

enum ActionId {
    CommandPalette,
    SessionPicker,
    ClipboardCopy,
    ClipboardPaste,
    TabNext,
    TabPrevious,
    TabIndex(u8),
    TabMoveLeft,
    TabMoveRight,
    DojoNew,
    DojoRename,
    DojoCloseTab,
    DojoTerminateConfirmed,
    LairNew,
    LairRename,
    LairPrevious,
    LairNext,
    LairTerminateConfirmed,
    WindowDetach,
    PaneSplitBelow,
    PaneSplitRight,
    PaneClose,
    PaneFocus(FocusDirection),
    PaneResize { direction: FocusDirection, cells: u16 },
    PaneZoomToggle,
    CopyModeEnter,
    FontZoomIn,
    FontZoomOut,
    FontZoomReset,
    HistorySearch,
    ControlRequest,
    ControlRelease,
    ControlForce,
    AccessRevoke,
    TransferAccept,
    TransferDeny,
    BindingHelp,
    ConfigReload,
    SendPrefix,
    PresetOpen(PresetName),
}

struct ResolvedBinding {
    sequence: KeySequence,
    action: ActionId,
    source: BindingSource,
}

struct ResolvedKeymap {
    prefixes: Vec<KeyChord>,
    bindings: Vec<ResolvedBinding>,
    direct_index: HashMap<KeyChord, ActionId>,
    prefix_index: HashMap<KeyChord, ActionId>,
    display_by_action: HashMap<ActionKey, Vec<DisplayChord>>,
}
```

`ActionId` remains closed and exhaustive. Configuration selects actions; it
cannot provide callbacks or Rust/shell snippets.

## Prefix state machine

Add `PrefixState` to Window-local input state:

```rust
enum PrefixState {
    Idle,
    Armed {
        prefix: KeyChord,
        raw_code: u32,
        deadline: Instant,
    },
}
```

Behavior:

1. A matching prefix press is consumed, records the exact prefix and deadline,
   and never reaches terminal input.
2. Prefix repeat is consumed and does not re-arm the deadline.
3. Prefix release is consumed through the existing raw-code ownership set.
4. The next non-modifier press before the deadline resolves through the prefix
   index, executes once, and returns to idle.
5. Unknown second keys are consumed and produce one quiet bounded status message;
   they are not forwarded after the prefix has claimed the sequence.
6. `SendPrefix` emits the primary prefix's terminal encoding exactly once.
7. Timeout returns to idle. The prefix itself is not replayed.
8. Focus loss, modal opening, tab activation, Window shutdown, keymap reload, and
   keyboard capability loss clear the state.
9. Modal input always has priority over global direct and prefix bindings.
10. Press/repeat/release behavior is covered by pure tests so no action fires on
    autorepeat unless that action explicitly opts into repeat.

## Built-in keymap profiles

### `splinterm`

Encode the current documented bindings as data with behavior parity. This is a
migration, not a redesign. Existing shortcut tests become table-driven tests
against this profile.

### `omarchy-tmux`

This profile inherits every non-conflicting Splinterm safety/productivity chord,
then adds the Omarchy tmux vocabulary. It does not read the live tmux config.
The packaged reference in `docs/omarchy-tmux-reference.md` is the behavioral
source of truth.

#### Config, help, and copy mode

| Omarchy input | Splinterm action | Required implementation |
| --- | --- | --- |
| `Ctrl+Space` | primary prefix | prefix state machine |
| `Ctrl+B` | secondary prefix | prefix state machine |
| `Prefix+Ctrl+Space` | send primary prefix | terminal encoding action |
| `Prefix+?` | binding help | generated trusted help overlay |
| `Prefix+q` | reload keymap/presets | transactional local reload |
| `Prefix+[` | enter copy mode | keyboard history/copy mode |
| copy mode `v` | begin selection | copy-mode-local action |
| copy mode `y` | copy and leave | Wayland clipboard publication using retained serial policy |

The first release may ship the profile before copy mode only if `Prefix+[` is
shown as unavailable rather than silently mapped to a different behavior. The
profile is not declared complete until vi copy mode passes its own milestone.

#### Panes

| Omarchy input | Splinterm action |
| --- | --- |
| `Alt+Enter` | split below |
| `Alt+Shift+Enter` | split right |
| `Alt+Escape` | close focused Splint using existing safe close semantics |
| `Prefix+h` | split below |
| `Prefix+v` | split right |
| `Prefix+x` | close focused Splint |
| `Prefix+z` | toggle client-local focused-pane zoom |
| `Ctrl+Alt+Arrow` | focus Splint in that direction |
| `Ctrl+Alt+Shift+Arrow` | resize toward that direction by 5 cells |

Directional resize must use the authoritative `PaneLayout` branch path and
current cell geometry. It computes the appropriate ancestor and a new bounded
`SplitRatio`, then sends `WindowTopologyCommand::SetRatio`. It must not pretend a
fixed ratio delta equals five cells.

Pane zoom is client-local presentation state. It hides sibling allocations for
painting/hit-testing and reports focus only for the zoomed Splint; it does not
rewrite durable `LayoutNode` topology.

#### Dojos (tmux windows)

| Omarchy input | Splinterm action |
| --- | --- |
| `Prefix+c` | create Dojo in focused Lair using focused Splint cwd |
| `Prefix+k` | open confirmed Dojo termination prompt |
| `Prefix+r` | open rename-current-Dojo prompt |
| `Alt+1` … `Alt+9` | activate visible Window-local Dojo index 1–9 |
| `Alt+Left` / `Alt+Right` | previous/next Dojo tab |
| `Alt+Shift+Left` / `Alt+Shift+Right` | reorder current Window-local tab |
| `Prefix+w` | open a trusted Dojo chooser |
| `Prefix+n` / `Prefix+p` | next/previous Dojo tab |
| `Prefix+&` | open confirmed Dojo termination prompt |

Tab reordering remains Window-local and non-persistent, matching current tab
semantics. Numeric selection resolves the committed visible tab order and never
targets a hidden or stale identity.

#### Lairs (tmux sessions)

| Omarchy input | Splinterm action |
| --- | --- |
| `Prefix+Shift+C` | create Lair using focused Splint cwd |
| `Prefix+Shift+K` | open confirmed Lair termination prompt |
| `Prefix+Shift+R` | open rename-current-Lair prompt |
| `Prefix+Shift+P` / `Prefix+Shift+N` | previous/next Lair |
| `Alt+Up` / `Alt+Down` | previous/next Lair |
| `Prefix+s` | open trusted Lair/Dojo chooser |
| `Prefix+d` | detach the native Window without terminating Dojos/Splints |

Previous/next Lair uses a captured ordered catalog. If the destination Lair has
an already attached Dojo, activate its most recently active attached Dojo;
otherwise open its persisted default/recent Dojo as a tab. Never infer authority
from a Lair name.

Lair termination is a new destructive trusted prompt. It captures the exact
Lair ID and every `(SplintId, incarnation)` beneath it, defaults to Cancel, and
rejects topology drift rather than killing newly added processes.

#### Splinterm bindings retained by the Omarchy profile

The profile keeps these unless the user explicitly unbinds them:

- `Ctrl+Shift+P`: command palette;
- `Ctrl+Shift+S`: recent sessions;
- `Ctrl+Shift+C/V`: copy/paste;
- `Ctrl+plus/equal/minus/0` and keypad equivalents: font zoom;
- `Ctrl+Shift+F`: scrollback search;
- `Ctrl+Shift+T/L/U/R/Y/N`: control and access workflows;
- `Shift+PageUp/PageDown/End`: history navigation.

Where an Omarchy binding supersedes an existing Splinterm chord, the profile's
resolved table is authoritative and `splinterm keymap show` reports the source.

## Generated help and diagnostics

Add local human-facing commands:

```text
splinterm config check
splinterm keymap list
splinterm keymap show [splinterm|omarchy-tmux]
splinterm keymap conflicts
splinterm preset list
splinterm preset show NAME
splinterm preset check [PATH]
```

Output rules:

- default output is grouped, compact, and human-readable;
- `keymap show` groups direct, prefix, modal, and retained Splinterm bindings;
- source detail (`built-in`, inherited, user file and line) is secondary;
- validation prints all independent errors in one pass when safe;
- no raw UUIDs or full paths dominate normal output;
- `NO_COLOR` is respected;
- no command reports a reload as successful until parse, conflict resolution,
  and atomic state replacement succeed.

`Prefix+?` renders the same resolved data in a trusted overlay. The CLI and UI
must share descriptor projection rather than duplicate tables.

## Transactional reload

`Prefix+q` reloads only the current Window's keymaps and presets in the first
release. `splinterm config check` validates the same files from a separate CLI
process but does not claim to mutate an already running Window. Font, geometry,
shell, and theme source remain startup-owned or use their existing channels.

Reload algorithm:

1. Read both files into new buffers.
2. Parse and validate independently.
3. Resolve inheritance, unbinds, conflicts, preset references, and command
   aliases into immutable `ResolvedKeymap` and `PresetCatalog` values.
4. If any error occurs, keep both previous values and show one bounded failure
   with `splinterm config check` as the next action.
5. If valid, clear prefix/copy-mode state and atomically replace both values on
   the Wayland event-loop thread.
6. Rebuild shortcut labels and help caches without rebuilding terminal frames.
7. Show a short “Bindings reloaded” status with counts.

A file watcher may be added later, but explicit reload is required first because
it is deterministic and matches Omarchy's `Prefix+q` contract.

## Dojo preset runtime model

### Client-side catalog and compiler

Add likely modules:

- `crates/splinterm/src/preset.rs`: schemas, built-ins, merge, validation;
- `crates/splinterm/src/preset/compiler.rs`: placeholders, optional nodes,
  grids, and binary-tree compilation;
- `crates/splinterm/src/app/presets.rs`: CLI/runtime resolution and human output.

The compiler produces a neutral tree whose leaves contain a stable local pane
key and `LaunchParameters`. IDs are assigned only at the daemon transaction
boundary.

```rust
enum PresetLayoutLaunch {
    Pane {
        key: PresetPaneKey,
        title: String,
        launch: LaunchParameters,
    },
    Split {
        orientation: PresetOrientation,
        ratio: SplitRatio,
        first: Box<Self>,
        second: Box<Self>,
    },
}

struct DojoLaunchSpec {
    name: String,
    focus: PresetPaneKey,
    root: PresetLayoutLaunch,
}
```

### Exact context capture

Preset invocation accepts an explicit `--cwd`; otherwise it resolves cwd and
Lair from the invoking/focused Splint. Add process environment hints such as
`SPLINTERM_LAIR_ID`, `SPLINTERM_DOJO_ID`, and `SPLINTERM_SPLINT_ID` when the
daemon launches a Splint, but treat them only as lookup hints. The client must
re-read topology and verify that the hinted Splint exists and belongs to the
claimed Dojo/Lair before mutation.

If hints are absent, a human invocation may use the daemon's graphical-focus
record. It must display the selected Lair/Dojo in the preview. If neither source
is unambiguous, fail with a next action; do not guess from cwd or names.

### Atomic protocol mutation

Add a private human-client request rather than issuing sequential split
requests:

```rust
enum PresetTarget {
    NewLair { name: String },
    ExistingLair { lair_id: LairId, rename: Option<String> },
}

Request::MaterializePreset {
    expected_topology_revision: TopologyRevision,
    target: PresetTarget,
    dojos: Vec<DojoLaunchSpec>,
}

Response::PresetMaterialized {
    lair_id: LairId,
    dojo_ids: Vec<DojoId>,
    panes: Vec<PresetPaneIdentity>,
    topology_revision: TopologyRevision,
}
```

Protocol and daemon rules:

- This request is initially accepted only from the executable-verified trusted
  human client role. Do not expose an automation/MCP equivalent accidentally.
- Validate the full tree, names, cwd values, argv, ratios, focus keys, pane
  counts, Dojo count, topology revision, and target Lair before mutation.
- Repeat cwd validation at the daemon boundary immediately before draft commit:
  every final expanded path must be absolute, UTF-8/NUL-safe under the existing
  wire byte bound, and resolve through `fs::metadata` to an existing directory.
  If any leaf fails, reject the entire request before persistence or process
  launch. If a directory disappears after this check, use the documented
  post-commit spawn-failure semantics rather than retargeting another path.
- Bound one request to 32 new Dojos, 32 panes per Dojo, 128 total new panes,
  depth 32, existing protocol argv/cwd byte limits, and frame-size limits.
- Build a draft topology with stable IDs and validate persistence bounds.
- Commit the complete topology once and advance one revision.
- Start each committed Splint through the existing launch path.
- A post-commit process-launch failure leaves that exact Splint durably exited
  with a bounded error; it never removes sibling panes or exposes a partial
  layout tree.
- If validation or persistence fails before commit, launch no process and change
  no topology.
- Audit one aggregate human preset operation plus bounded per-process outcomes.
- Return the mapping from preset pane keys to stable IDs so the client can focus
  the requested pane without positional guessing.

Implement the core mutation by cloning/validating a draft or by adding an
explicit transaction method to `splinterm-core`; do not call public one-step
mutators repeatedly and then attempt best-effort rollback.

## Built-in Omarchy Dojo presets

Built-ins are compiled from the same schema as user presets where possible. The
dynamic `tdlm` and `tsl` planners may use closed generators that emit the same
neutral `DojoLaunchSpec` type.

### Command aliases

| Alias | Direct argv behavior | Risk |
| --- | --- | --- |
| `editor` | `$EDITOR` parsed to direct argv, fallback `nvim`, append `.` | normal |
| `c` | `opencode --auto` | unrestricted mode; opt-in |
| `cx` | `claude --permission-mode bypassPermissions` | unrestricted mode; opt-in |
| `cy` | `codex -s danger-full-access -a never` | unrestricted mode; opt-in |
| `hunk-watch` | `hunk diff --watch` | normal |

Unrestricted aliases are present in the catalog but disabled until
`allow-unrestricted-commands=yes`. A disabled alias yields a clear validation
error before topology mutation. Do not silently downgrade its flags.

### `omarchy.t`

Equivalent intent: attach to an existing session, otherwise create `Work`.

- List current topology and feed it through the existing recent-Dojo/session
  catalog ordering (`collect_sessions` plus `recent_dojo_ids`).
- Select the first running/reopenable Dojo from that catalog, regardless of its
  Lair name. This is Splinterm's deterministic equivalent of bare `tmux attach`
  selecting an existing attachable session.
- Open or activate that exact `(LairId, DojoId)`; never resolve the target again
  from its rendered name.
- Only when no running/reopenable Dojo exists, create a `Work` Lair with one
  shell Dojo at invocation cwd.
- Duplicate Lair names are already prohibited by core topology. If an inactive
  non-reopenable `Work` Lair blocks creation, report that exact conflict and
  offer its restore/rename options; do not create a differently named surprise.
- This workflow does not require multi-pane materialization.

### `omarchy.tdl`

Parameters: required `ai`, optional `ai2`.

Creates a new Dojo in the captured Lair. It does not rewrite or terminate the
invoking Dojo.

```text
root: rows, ratio 850
├─ work: columns, ratio 650
│  ├─ editor: editor command, cwd=root
│  └─ ai-column
│     ├─ ai1: selected AI command, cwd=root
│     └─ ai2: optional second AI command, cwd=root
└─ terminal: configured login shell, cwd=root
```

- Without `ai2`, `ai-column` collapses to `ai1`.
- With `ai2`, `ai-column` is rows at ratio 500.
- Dojo name is `{cwd.basename}`.
- Initial focus is `editor`, intentionally correcting the likely stale
  `opencode_pane` focus bug documented in the Omarchy reference.
- The bottom shell receives 15% height.

### `omarchy.tds`

Creates one square Dojo:

```text
root: rows 500
├─ top: columns 500
│  ├─ editor: nvim .
│  └─ diff: hunk diff --watch
└─ bottom: columns 500
   ├─ terminal: login shell
   └─ ai: opencode
```

All panes use the captured root cwd. Initial focus is `editor`.

### `omarchy.tdlm`

Parameters: required `ai`, optional `ai2`.

- Capture the current directory as parent.
- Enumerate immediate non-hidden child directories in deterministic bytewise
  filename order; do not follow symlinked directories unless explicitly opted
  in later.
- Reject an empty set with a human message.
- Reject more than 32 children before mutation.
- Rename the captured Lair to the parent basename as part of the same preset
  transaction.
- Create one `tdl` Dojo per child, named after that child and rooted there.
- Open/focus the first created Dojo after the daemon acknowledges the complete
  transaction.

### `omarchy.tsl`

Parameters: `count` in `1..=16` and required command alias/direct argv.

- Create one Dojo named `{cwd.basename}-swarm`.
- Create `count` panes at the captured cwd, each running the same direct argv.
- Compile a deterministic near-square tiled binary tree. Choose
  `columns=ceil(sqrt(count))`, assign panes row-major, omit unused final cells,
  and derive branch ratios from occupied row/column counts.
- Focus the first pane.
- A quoted compatibility command string is tokenized to direct argv by the
  closed compatibility lexer above and rejects shell-evaluation syntax; it is
  never evaluated by a shell.

## CLI and optional shell compatibility

### Preset CLI

```text
splinterm preset list
splinterm preset show omarchy.tdl
splinterm preset check
splinterm preset run omarchy.t
splinterm preset run omarchy.tdl --param ai=c
splinterm preset run omarchy.tdl --param ai=c --param ai2=cx
splinterm preset run omarchy.tds
splinterm preset run omarchy.tdlm --param ai=cy
splinterm preset run omarchy.tsl --param count=4 --param command=c
splinterm preset run personal-review --cwd ~/Code/project
```

Before mutation, human output shows a compact preview:

```text
Preset   omarchy.tdl
Target   Work / my-project
Root     ~/Code/my-project
Panes    editor · c · shell
Layout   editor + ai above, shell below (85/15)

Creating Dojo…
Created  my-project — 3 panes
```

Normal successful execution remains concise. `--dry-run` validates and previews
without connecting for mutation. Destructive replacement is not part of v1;
there is no confirmation for additive new-Dojo creation.

### Shell compatibility

Package, but do not auto-source, a Bash integration generated from stable preset
commands. Provide:

```text
splinterm preset shell-init omarchy --shell bash
```

It prints functions for:

- `t` → `omarchy.t`;
- `tdl`, `tds`, `tdlm`, `tsl` → corresponding preset commands;
- `ic`, `ix`, `icx` → `tdl c`, `tdl cx`, `tdl c cx`.

Rules:

- The installer may offer an explicit opt-in to write a dedicated
  `~/.config/splinterm/shell/omarchy.bash` file.
- It does not edit `.bashrc` automatically in the first release.
- It checks and reports existing functions/aliases named `t`, `tdl`, `tds`,
  `tdlm`, `tsl`, `ic`, `ix`, or `icx` before installation.
- Global `c`, `cx`, and `cy` aliases are not installed by Splinterm. Those names
  remain preset-local command aliases to avoid commandeering common shell names.
- Generated functions preserve argument boundaries and use `command splinterm`
  to avoid recursive aliases.
- Bash integration receives `shellcheck`; other shells are separate follow-ups.

## Trusted UI additions

### Binding help

Reuse command-palette visual primitives but create a read-only help model:

- generated from `ResolvedKeymap`;
- grouped by Prefix, Panes, Dojos, Lairs, History, View, and Control;
- searchable when practical;
- shows unavailable actions distinctly;
- cannot execute commands from rendered labels;
- Escape closes and all terminal input remains isolated while open.

### Dojo and Lair choosers

`Prefix+w` and `Prefix+s` need exact-target trusted pickers rather than
position/name parsing. Extend the existing session-picker family with captured
IDs. Selecting a row opens/activates that exact target. Stale targets reject
cleanly.

### Copy mode

Implement as a dedicated client-local state over loaded history:

- `Prefix+[` enters at the live cursor/current viewport;
- vi movement is bounded to loaded rows and can request bounded older pages;
- `v` anchors selection;
- `y` publishes selected UTF-8 and exits;
- Escape exits without copying;
- terminal mouse reporting, paste, tabs, dividers, IME, and application input are
  isolated while active;
- clipboard publication must obey current Wayland serial/ownership constraints.

Do not label search mode as copy mode; ship the real interaction or report it as
unavailable.

## Implementation milestones

### Milestone 1 — action registry and current-profile parity

**Status:** Complete (`7abd969`)

Files:

- `crates/splinterm/src/keymap.rs` (new);
- `crates/splinterm/src/wayland/input/shortcuts.rs`;
- `crates/splinterm/src/wayland/dispatch/keyboard.rs`;
- `crates/splinterm/src/frontend/action_menu.rs`;
- `crates/splinterm/src/config.rs`;
- `config/splinterm/config.ini`;
- `docs/configuration.md`.

Work:

- define closed actions/chords/sequences;
- encode current Splinterm bindings as `splinterm` profile;
- route existing behavior through one resolver/dispatcher;
- generate palette shortcut labels;
- preserve exact press/repeat/release and modal behavior.

Gate:

- current binding behavior and command hints are byte/identity equivalent;
- no user configuration enabled yet;
- focused Rust tests and full `cargo test -p splinterm --lib` pass.

### Milestone 2 — strict custom keymap and inspection CLI

**Status:** Complete (`7abd969`)

Work:

- add TOML dependency at the narrow owning crate;
- parse/normalize/merge/unbind/conflict-check;
- add `config check` and `keymap` human commands;
- enable `[key-bindings]` selection;
- add source-aware diagnostics.

Gate:

- malformed files never partially apply;
- golden tests cover every supported key spelling and error class;
- old `config.ini` continues to load without diagnostics about missing files.

### Milestone 3 — prefix engine and Omarchy direct/pane bindings

**Status:** Complete

Recorded evidence:

- validation: `cargo test -p splinterm --lib --no-fail-fast` (303 passed,
  1 ignored), `cargo test -p splinterm --test keymap_cli --no-fail-fast`
  (3 passed), Clippy with `-D warnings`, formatting, and `git diff --check`;
- review: one fresh read-only review identified unsafe nested directional
  resize selection, modifier cancellation, and missing `Prefix+[` unavailable
  guidance; all three findings were corrected and the full validation gate was
  rerun.

Work:

- add prefix state/deadline lifecycle;
- add `Ctrl+Space`, `Ctrl+B`, send-prefix, help placeholder, and reload;
- add direct pane split/close/focus bindings;
- add exact five-cell directional resize;
- add client-local pane zoom.

Gate:

- pure sequence tests cover focus/modal/timeout/repeat/release resets;
- no prefix byte leaks to the PTY except explicit send-prefix;
- pane geometry tests prove five-cell resize direction at multiple scales.

### Milestone 4 — Dojo/Lair bindings and trusted selectors

Work:

- add numeric tab selection and Window-local tab reorder;
- use focused Splint cwd for new Dojo/Lair;
- add rename/termination prompts for current targets;
- add Dojo and Lair chooser models;
- add previous/next Lair behavior and Window detach.

Gate:

- every action captures stable IDs;
- destructive operations default Cancel and reject drift;
- closing/detaching never kills durable topology accidentally.

### Milestone 5 — static preset schema and compiler

Work:

- implement strict `presets.toml` schema;
- add command alias and placeholder resolution;
- compile named nodes to a validated binary launch tree;
- add `preset list/show/check/run --dry-run`.

Gate:

- cycle, reuse, orphan, depth, count, cwd, argv, focus, and ratio tests pass;
- direct TOML `argv` arrays are proven through tests containing shell
  metacharacters that remain literal arguments, while compatibility strings use
  the stricter closed lexer and reject them.

### Milestone 6 — atomic preset protocol and daemon materialization

Files likely include:

- `crates/splinterm-protocol/src/lib.rs`;
- `crates/splinterm-core/src/model.rs` and `layout.rs`;
- `crates/splinterd/src/main.rs` plus audit/authorization modules;
- `crates/splinterd/tests/end_to_end.rs`;
- `crates/splinterm/src/app/topology_manager.rs`;
- `crates/splinterm/src/frontend/topology.rs`.

Work:

- add neutral launch tree protocol types;
- add one-revision draft topology transaction;
- launch committed leaves and return pane-key/ID map;
- reconcile/open created Dojos in the native Window.

Gate:

- stale revision, malformed tree, persistence failure, and authorization denial
  produce zero topology changes and zero launched processes;
- accepted request produces one revision and a complete tree;
- one simulated spawn failure leaves one exited leaf and intact siblings.

### Milestone 7 — built-in Omarchy layouts

Work:

- define `omarchy.t`, `tdl`, `tds`, `tdlm`, and `tsl`;
- add opt-in unrestricted command aliases;
- implement optional-pane and deterministic-grid compilation;
- add exact cwd/name/focus behavior.

Gate:

- golden `LayoutNode` fixtures match every reference layout;
- `tdlm` ordering and bounds are deterministic;
- unrestricted aliases cannot run without explicit opt-in;
- the stale upstream `opencode_pane` focus behavior is not reproduced.

### Milestone 8 — shell integration

Work:

- generate Bash compatibility functions;
- add conflict inspection and opt-in installer path;
- document exact differences from tmux behavior.

Gate:

- `shellcheck` passes;
- Bats or shell tests prove argument preservation for one/two AI commands and
  quoted swarm command strings;
- installation never edits or replaces an existing alias silently.

### Milestone 9 — copy mode and complete Omarchy profile

Work:

- implement vi copy mode and clipboard handoff;
- replace unavailable `Prefix+[` profile entry;
- finish generated `Prefix+?` help.

Gate:

- keyboard-only selection spans visible and loaded historical rows;
- `v`, `y`, Escape, paging, focus loss, and topology changes are bounded;
- the Omarchy profile can be documented as complete for all reference bindings
  that have a meaningful Splinterm equivalent.

### Milestone 10 — packaging, installation, and graphical acceptance

Work:

- package examples, built-in profile docs, preset schema, and optional shell
  integration;
- update README/configuration/packaging docs;
- build/install coherent adjacent client and daemon binaries when the protocol
  milestone requires both.

Gate:

- `./install.sh` packages only committed source as currently required;
- trusted UI identity is validated with adjacent `/usr/bin` device/inode rules;
- non-graphical CLI/config/preset checks pass;
- graphical tests run only under a separately approved guarded matrix;
- existing Splinterm Windows are reopened after a client replacement.

## Test strategy

### Pure keymap tests

- canonical parsing for every modifier/key alias;
- direct and prefix sequence normalization;
- inheritance and unbind precedence;
- duplicate/conflict source reporting;
- prefix timeout, repeat, release, modal, focus, and reload lifecycle;
- display chord generation and command-palette projection;
- parity fixture for every current Splinterm binding;
- parity fixture for every Omarchy reference binding.

### Pure preset tests

- static binary trees and focus resolution;
- columns/rows to internal Axis translation;
- optional branch collapse;
- grid generation for counts 1 through 16;
- `tdl`, one/two-AI variants, `tds`, `tdlm`, and `tsl` golden trees;
- deterministic directory ordering;
- placeholder and `$EDITOR` handling;
- compatibility lexer quoting, escaping, empty arguments, rejection offsets,
  and every prohibited expansion/operator character;
- shell metacharacters are rejected rather than evaluated or silently retained;
- every count/depth/bytes/path/risk bound.

### Core/protocol/daemon tests

- request round trip and frame bounds;
- authorization rejects automation role;
- stale revision is side-effect free;
- relative, missing, non-directory, oversized, and NUL-containing leaf cwd
  values are rejected daemon-side before persistence;
- one-revision complete insertion;
- stable pane-key/ID response mapping;
- persistence round trip of complete layouts;
- startup/spawn failure semantics;
- aggregate audit records contain bounded metadata, never terminal bodies.

### Client integration tests

- configured shortcut dispatches the same exact captured command as palette
  activation;
- dynamic shortcut labels update after reload;
- new Dojo/Lair cwd comes from captured Splint, not GUI process cwd;
- preset-created Dojos reconcile all panes before activation;
- chooser and destructive prompt reject stale targets;
- tab reorder stays local and does not alter daemon topology.

### CLI and shell tests

- calm grouped output and actionable errors;
- `--dry-run` is side-effect free;
- keymap/preset inspection works without a running daemon;
- shell functions preserve spaces and argv boundaries;
- existing alias conflicts are reported and retained;
- `NO_COLOR` output remains legible.

### Graphical matrix

After explicit approval under the repository graphical-testing rules:

1. smoke one isolated managed Window on workspace 8 / DP-2;
2. verify prefix help and one prefix action;
3. verify direct pane split/focus/resize/zoom;
4. verify Dojo index/navigation/reorder;
5. verify Lair chooser and detach semantics;
6. create and inspect `tdl`, `tds`, and `tsl` layouts;
7. verify copy mode and modal input isolation;
8. restore focus/workspace and remove test topology.

Abort on wrong-target input, focus failure, placement failure, or cleanup failure.

## Compatibility and migration

- Default profile remains `splinterm`; no current chord changes automatically.
- Existing `[key-bindings]` entries that previously produced “not remappable”
  diagnostics become supported only when they use the new documented keys.
- Do not reinterpret old arbitrary Foot binding syntax as Splinterm action IDs.
- The `omarchy-tmux` profile is bundled, versioned with Splinterm, and tested
  against the checked-in reference. It is not silently changed based on the
  machine's live tmux customization.
- User files are versioned. Unsupported versions fail with migration guidance.
- A keymap/preset reload is all-or-nothing and retains last-known-good state.
- Protocol additions require coherent adjacent client/daemon packaging; do not
  install only the client once `MaterializePreset` is introduced.
- Public JSON/NDJSON automation schemas remain unchanged until a separate plan
  explicitly designs preset authority and stable machine envelopes.

## Documentation deliverables

- Update `docs/configuration.md` with file locations, schema, precedence, reload,
  diagnostics, and full action vocabulary.
- Keep `docs/omarchy-tmux-reference.md` as the stock-source reference and add a
  separate “Splinterm mapping” table rather than rewriting historical facts.
- Add `docs/presets.md` with static and parameterized examples.
- Add checked-in examples:
  - `config/splinterm/keybindings.toml`;
  - `config/splinterm/presets.toml`;
  - generated Omarchy Bash integration.
- Update command palette documentation to state that hints are resolved, not
  hard-coded.
- Document semantic differences: additive new Dojos instead of mutating the
  invoking shell, direct argv instead of shell injection, Window-local tab
  order, explicit destructive confirmation, and disabled unrestricted AI modes
  until opt-in.

## Risks and controls

| Risk | Control |
| --- | --- |
| Invalid binding captures common input | parse and resolve complete keymap before atomic activation; retain last known good |
| Prefix leaks or eats unrelated keys | explicit timeout/state reset rules and press/repeat/release tests |
| UI shortcut hints drift | derive all hints/help from `ResolvedKeymap` |
| Preset creates half a layout | one daemon topology transaction, not sequential split rollback |
| Preset executes unexpected shell syntax | direct argv only; no implicit `sh -c` |
| AI aliases bypass safety unexpectedly | explicit unrestricted-command opt-in and preflight failure |
| Cwd/name identifies the wrong target | stable ID capture and topology revalidation |
| Lair termination kills new processes | capture exact IDs/incarnations and reject drift |
| User preset starts too many processes | strict per-Dojo/request bounds and dry-run preview |
| Omarchy changes upstream | update the checked-in reference deliberately and test profile diff |
| Client-only install breaks new protocol | package/install adjacent coherent client and daemon after protocol milestone |

## Approval boundaries during implementation

Separate approval is required before:

- expanding the closed action registry beyond this plan;
- exposing presets to automation/MCP policy;
- allowing arbitrary shell evaluation or plugin-provided actions;
- installing unrestricted global `c`, `cx`, or `cy` aliases;
- replacing Pacman-owned binaries;
- restarting/replacing the daemon for protocol deployment;
- running the guarded graphical matrix.

Routine source edits, unit/integration tests, and non-graphical validation remain
within an approved implementation request.

## Definition of done

Do not call this plan complete until:

- every milestone has recorded validation evidence;
- `cargo fmt --all -- --check`, focused Clippy under repository policy, full
  workspace tests, protocol/schema tests, shell tests, and `git diff --check`
  pass or have explicitly bounded unrelated failures;
- the Omarchy binding and preset parity fixtures cover every row in
  `docs/omarchy-tmux-reference.md`;
- a fresh read-only review approves keymap safety, preset transactionality,
  direct-execution semantics, docs, and packaging;
- the separately approved graphical matrix passes;
- packaged installation evidence confirms coherent trusted client/daemon
  identity where the new protocol is used.
