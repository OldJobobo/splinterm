# Splinterm configuration and Foot migration

Phase 8 intentionally supports a small, explicit configuration surface. The
default path is `${XDG_CONFIG_HOME:-~/.config}/splinterm/config.ini`; set
`SPLINTERM_CONFIG` to test another file. Start from
[`config/splinterm/config.ini`](../config/splinterm/config.ini).

## Supported keys

| Section/key | Meaning | Range/default |
| --- | --- | --- |
| `main.font` | fontconfig pattern | JetBrains Mono Nerd Font Regular |
| `main.font-pixelsize` | configured pixel font size | 6–96; 14 |
| `main.font-point-size` | mutually exclusive point-size alternative | 6–96; unset |
| `main.font-size` | deprecated alias for `main.font-pixelsize` | unset |
| `main.font-sizing-policy` | `output-scale` or `physical-dpi` (no auto mode) | output-scale |
| `main.padding-left`, `padding-right`, `padding-top`, `padding-bottom` | independent logical padding edges | 0–10000; 12 each |
| `main.initial-columns`, `initial-rows` | requested initial grid | 2–480, 2–128; 80×24 |
| `main.shell` | shell executable used for an empty launch | login shell from the account |
| `main.login-shell` | use login-style argv[0] for the shell | yes |
| `main.title` | fixed window title; otherwise OSC title | unset |
| `main.app-id` | diagnostic only | fixed to `com.oldjobobo.splinterm` |
| `main.resize-delay-ms` | idle debounce before terminal reflow and PTY resize | 0–1000; 100 |
| `main.dpi-aware` | deprecated **legacy Splinterm** key: `yes` maps only to `output-scale`; `no` fails with migration guidance | unset |
| `main.theme` | explicit Splinterm JSON palette override; disables native Omarchy discovery | unset |
| `colors.alpha` | optional Foot-compatible override for theme background translucency | 0.0–1.0; unset (theme-owned) |
| `colors.blur` | optional native background-blur request | strict boolean; unset (theme-owned, otherwise `no`) |
| `scrollback.lines` | daemon terminal history budget | 0–1,000,000; 1000 |
| `cursor.style` | `block`, `beam`, or `underline` | block |
| `cursor.blink` | permit cursor blink | yes |
| `multiplexer.divider-style` | `line`, `frame`, or `none` pane chrome | line |
| `multiplexer.frame-title` | top-frame title source: `splint` or `none`; inert outside frame style | splint |

The protocol grid is bounded at 480×128. The shipped 14 px output-scale profile
uses the complete natural grid on validated 2560×1440 and 3840×2160 surfaces.
A smaller font or larger surface can exceed that envelope; Splinterm then keeps
the actual terminal top-left anchored, leaves remaining right/bottom pixels
outside terminal hit-testing, and emits one bounded `grid_capped` diagnostic for
the pane instead of silently treating the residual as terminal cells.

Malformed supported values fail startup. Unknown sections and keys print
line-numbered diagnostics. By default, palette roles come directly from the
active Omarchy theme's `colors.toml` and effective `foot.ini`; `[colors] alpha`
and `[colors] blur` are explicit user overrides. Whichever alpha source wins follows Foot's default
alpha mode: only cells whose background source is default are translucent;
explicit and reverse-video backgrounds remain opaque. When blur resolves to
`yes`, alpha is translucent, and the compositor advertises
`ext-background-effect-v1` blur capability, Splinterm requests native
compositor blur for the finite native Window region. Missing protocol support
or capability falls back to ordinary transparency with one bounded diagnostic;
opaque alpha and `blur=no` own no effect object. The protocol is still staging;
the validated initial target is Hyprland 0.56.1 or newer, while other
compositors require compatible version-1 blur capability. `alpha-mode=matching/all`
remains unsupported. Other `[colors]` options direct users to the active
Omarchy palette or an explicit `main.theme` JSON override. `[key-bindings]`
selects the typed built-in `splinterm` profile and an optional strict TOML
overlay, so keyboard dispatch and command-palette shortcut labels share one
action registry. This avoids claiming arbitrary `foot.ini` compatibility.

Built-in local bindings include Ctrl+Shift+C/V and Super+C/V for copy/paste,
Ctrl+Shift+S to open the native Recent Dojos picker, and Ctrl+Shift+P to open the
searchable command palette inside any focused managed terminal Window. The
palette is a curated trusted subset of the same closed action registry used by
the effective keymap. It includes resolved keybinding help, transactional
configuration reload, copy mode, focused-pane zoom, Dojo and Lair choosers,
Dojo reordering, Lair rename/navigation/termination, and clean Window detach in
addition to recent Dojos, tab navigation, splits, pane focus/closing/resizing,
tab-strip and font zoom, history/search, and bounded control actions. Force
control is trusted-local only and remains visibly disabled in remote Windows.
Typing filters
titles, categories, and keywords; arrows skip unavailable commands, Enter runs,
and Escape closes. Actions without an available captured target remain visible
but disabled. Right-clicking a visible Dojo tab opens a compact trusted menu
targeted to that tab without activating it first. Its six rows are Rename Tab,
Activate Tab, New Dojo, detach-only Close Tab, detach-only Close Other Tabs, and
Terminate Dojo. Rename opens a bounded prefilled editor. Termination opens a
named confirmation showing the captured pane count with Cancel selected by
default. Arrows navigate, Enter runs, and Escape or an outside click closes the
active surface.
Ctrl+Tab and Ctrl+Shift+Tab cycle Window-local Dojo tabs;
Ctrl+Shift+D creates and opens a Dojo in the active tab's Lair; Ctrl+Shift+Q
detaches the active tab and closes the Window when it was the final tab. These
application-owned chords are consumed on press, repeat, and release rather than
forwarded to the terminal process. Directional Ctrl+Shift+Arrow remains pane
navigation. In managed multi-Splint windows, Ctrl+Shift+W terminates and closes
the focused Splint; legacy direct single-Splint attachments leave that chord to
the terminal.
Ctrl+Shift+R revokes active access, and Ctrl+Shift+L releases control.
Ctrl+Shift+T requests transfer from the current controller; its trusted UI uses
Ctrl+Shift+Y/N to accept/deny, while Ctrl+Shift+U opens separate trusted
confirmation for forced takeover. Ctrl+Shift+F opens local literal scrollback
search; Enter submits, Ctrl+N/P navigates, and Escape closes the trusted search
surface. These control/search bindings are trusted application actions rather
than terminal-controlled callbacks. Foot-compatible runtime zoom uses
Ctrl+plus/equal/KP_Add and Ctrl+minus/KP_Subtract in 0.5-point steps; Ctrl+0/KP_0
resets the configured size. Terminal key mappings otherwise follow the
implemented Foot/xterm behavior.

## Keymap configuration

The top-level INI selects a built-in profile and optional overlay:

```ini
[key-bindings]
profile=splinterm
file=keybindings.toml
prefix-timeout-ms=1000
```

`file` is optional. Relative paths resolve beside the selected `config.ini`;
`~/` paths resolve through `HOME`. The timeout accepts 250–5000 milliseconds and
bounds an armed prefix sequence. Packaged profiles are `splinterm` and
`omarchy-tmux`; selecting an unknown profile is a startup error that lists both
available names. The Omarchy profile provides `Ctrl+Space` and `Ctrl+B` prefixes,
its direct and prefixed pane controls, exact five-cell directional resize,
client-local pane zoom, Dojo/Lair creation and selection, numeric and reordered
Window-local tabs, stable-ID trusted choosers, confirmed rename/termination,
Lair navigation, clean Window detach, a per-Window `Prefix+B` Dojo tab-strip
toggle, a generated searchable trusted `Prefix+?` help overlay, vi copy mode,
transactional config reload, and current-Lair lifecycle bindings:
`Prefix Shift+S` saves, `Prefix Shift+F` toggles pinned state,
`Prefix Shift+V` previews, and `Prefix Shift+O` opens the existing restore
confirmation path. These are Splinterm additions; the checked stock Omarchy tmux
profile defines no actions on those four shifted prefix keys. New Dojos and
Lairs inherit the focused Splint cwd.

`Prefix+[` enters copy mode at the live cursor or current history viewport.
`h/j/k/l` and arrows move over visible and loaded historical rows; Home/End move
to line edges and PageUp/PageDown page within bounded loaded history while
requesting older bounded pages when needed. `v` anchors a selection, `y` copies
it to the Wayland clipboard with the triggering keyboard serial and exits, and
Escape cancels. Copy mode isolates terminal input, paste, pointer actions, IME,
and application mouse reporting, and cancels safely on focus, topology, pane
identity, or history-generation changes.

`Prefix+?` opens help for the effective resolved keymap. Type to search action
labels, configuration names, shortcuts, sources, and closed keywords; use
Up/Down or PageUp/PageDown to navigate, `Ctrl+U` to clear, and Escape once to
clear a non-empty query or again to close. The owned query never reaches the
terminal.

Outside copy mode, both built-in profiles map `Ctrl+Shift+C/V` and `Super+C/V`
to terminal-selection copy and the existing safe/bracketed paste. The
`omarchy-tmux` profile advertises `Ctrl+Shift+C/V` first so its generated help
remains usable when the compositor reserves Super chords; `splinterm` keeps
`Super+C/V` primary. Omarchy's universal binding injects
`Ctrl+Insert`/`Shift+Insert` into windows carrying its `terminal` tag, which
Splinterm accepts. Omarchy must classify `com.oldjobobo.splinterm` as a terminal;
without that classification its universal copy branch injects ordinary
`Ctrl+C`, which remains terminal interrupt. In copy mode, `Super+C` copies the active
selection and exits; `Super+V/X/Z` are consumed locally without pasting or
sending PTY input. These Super shortcuts work only when the compositor delivers
the chord to the Splinterm Window. Splinterm-owned command palette, keybinding
help, search, and rename fields accept effective `clipboard.copy` and
`clipboard.paste` bindings such as `Ctrl+Shift+C/V`; they also support
`Super+A/C/V/X/Z` selection/copy/paste/cut/undo and Shift+Left/Right selection. Their Unicode-safe undo
history is limited to 16 states and disappears with the field. Terminal-pane
`Super+X` and `Super+Z` are not claimed as universal cut or undo actions because
the running application owns its input buffer.

The overlay is versioned TOML and inherits one built-in profile:

```toml
version = 1
inherits = "splinterm"

[[unbind]]
sequence = ["Ctrl+Shift+P"]

[[binding]]
sequence = ["Ctrl+Alt+P"]
action = "app.command-palette"
```

Every table rejects unknown fields. A sequence contains one direct chord or
`["Prefix", "CHORD"]`; prefix sequences require a profile that defines prefixes.
Modifier names are `Ctrl`, `Shift`, `Alt`, and `Super` (`Control` and `Logo` are
accepted aliases). Letter case never implies Shift. Supported keys are letters,
Tab, Enter/KP_Enter, Escape, Space, slash/question, arrows, PageUp/PageDown, End,
backslash, brackets, ampersand, Plus/Equal/Minus, digits 0–9, and KP_0. Empty or
duplicate modifiers and unsupported keys fail with source context.

An overlay applies unbinds before bindings. An unmatched unbind is a diagnostic;
duplicate or semantically overlapping chords are errors naming both sources.
Only closed application actions can be selected—configuration cannot register
shell commands or callbacks. Bindable action IDs are:

```text
app.command-palette       session.recent
clipboard.copy            clipboard.paste
dojo.new                  dojo.previous
dojo.next                 dojo.close-tab
dojo.close-other-tabs      dojo.rename
dojo.terminate-confirmed
dojo.choose               dojo.select-1
dojo.select-2             dojo.select-3
dojo.select-4             dojo.select-5
dojo.select-6             dojo.select-7
dojo.select-8             dojo.select-9
dojo.move-left            dojo.move-right
lair.new                  lair.rename
lair.save                 lair.pin-toggle
lair.preview              lair.restore
lair.terminate-confirmed  lair.previous
lair.next                 lair.choose
window.detach             pane.split-below
pane.split-right          pane.focus-left           pane.focus-right
pane.focus-up             pane.focus-down
pane.close                pane.resize-smaller
pane.resize-larger        pane.resize-left-5
pane.resize-right-5       pane.resize-up-5
pane.resize-down-5        pane.zoom-toggle
view.toggle-tab-strip     app.binding-help
app.config-reload
copy-mode.enter           terminal.send-prefix      history.search
history.page-up           history.page-down
history.return-live       view.zoom-in
view.zoom-out             view.zoom-reset
control.request           control.release
control.force             control.accept-transfer
control.deny-transfer     access.revoke-all
```

Local inspection never contacts `splinterd`:

```text
splinterm config check
splinterm keymap list
splinterm keymap show [splinterm|omarchy-tmux]
splinterm keymap conflicts
```

`config check` parses the INI and selected overlay together. `keymap show` without
a name displays the effective keymap with source locations; with a name it shows
the packaged profile. Invalid configuration is never partially applied: Window
startup fails before mapping rather than falling back to a half-resolved keymap.

## Dojo presets

Splinterm always includes the bounded `omarchy.t`, `omarchy.tdl`, `omarchy.tds`,
`omarchy.tdlm`, and `omarchy.tsl` catalog. Select an optional strict user overlay
beside `config.ini`:

```ini
[presets]
file=presets.toml
allow-unrestricted-commands=no
```

A configured file must be readable and valid; it is never silently ignored.
Relative paths resolve beside `config.ini`. User preset names shadow packaged
names. User command aliases shadow packaged aliases only inside user-owned
presets; they cannot rewrite bundled Omarchy layouts.

`allow-unrestricted-commands` defaults to `no`. Setting it to `yes` explicitly
enables only the packaged `c`, `cx`, and `cy` aliases, whose full direct argv is
documented in [Dojo presets](presets.md). It never enables shell evaluation.

Version-1 catalogs define direct command aliases and bounded named node trees.
`columns` means left/right; `rows` means top/bottom. Ratios are the first child's
thousandths in `1..=999`. Panes may use a static alias, a typed command parameter,
or `shell=true`. Optional panes collapse their unary parent branch. A bounded
`grid` expands deterministically after integer/command parameter validation.
Trees reject cycles, reuse, orphans, invalid focus, depths over 32, and more than
32 final panes.

```text
splinterm preset list
splinterm preset show NAME
splinterm preset check [PATH]
splinterm preset run NAME --cwd PATH [--param NAME=VALUE]... --dry-run
splinterm preset run NAME --cwd PATH [--param NAME=VALUE]... [--no-open]
```

The optional Bash integration is generated from the packaged presets rather
than selected by another INI field:

```text
splinterm preset shell-init omarchy --shell bash
splinterm preset shell-install omarchy --shell bash
```

The installer uses
`${XDG_CONFIG_HOME:-$HOME/.config}/splinterm/shell/omarchy.bash`, creates it only
when absent, and never edits `.bashrc` or another startup file. The generated
file must be sourced explicitly and refuses to define any function when `s`,
`sdl`, `sds`, `sdlm`, or `ssl` already resolves to an alias, function, builtin,
or executable. It does not define or replace Omarchy's tmux shell names.

Inspection and dry-run are local and do not connect to `splinterd`. Dry-run
compilation checks final cwd directories and launch bounds and prints a topology
preview without showing full argv. A non-dry run verifies its exact invoking or
focused Splint context, then sends one trusted-local atomic preset request; it
never emulates a preset with a sequence of partial splits. Successful runs open
the first committed Dojo by stable ID unless `--no-open` is explicit. Remote, automation,
and MCP clients cannot invoke this private materialization request. See
[Dojo presets](presets.md) for the complete schema, bundled layouts,
parameter rules, direct-execution rules, and failure semantics.

## Remote profile configuration

Remote SSH endpoints use a separate strict TOML file rather than the
Foot-compatible INI parser:

```text
${XDG_CONFIG_HOME:-~/.config}/splinterm/remotes.toml
```

```toml
version = 1

[remotes.wintermute]
host = "wintermute"
user = "operator"                    # optional; OpenSSH default otherwise
port = 22                            # optional; OpenSSH config/default otherwise
identity_files = ["~/.ssh/id_ed25519"]
known_hosts_file = "~/.ssh/known_hosts"
connect_timeout_seconds = 15
```

The schema rejects unknown fields at every level. Profile names, host/user
tokens, counts, document size, paths, ports, and timeouts are bounded. Explicit
files must be readable regular local files; path expansion never invokes a
shell. There is deliberately no arbitrary option, forwarding, environment,
proxy-command, local-command, or remote-command field. Ordinary safe alias,
identity, certificate, agent, and proxy routing configured in OpenSSH remains
available, while Splinterm supplies fixed safety overrides and the fixed remote
command.

Use `splinterm remote list` and `splinterm remote inspect PROFILE` for local-only
validation. `splinterm remote check PROFILE` additionally starts SSH and performs
bounded non-mutating relay/daemon reachability probes. `splinterm --remote
PROFILE` authenticates once and opens that profile's native Recent Dojos
picker; explicit `dojos`, `reopen`, `window`, and `launch` forms are also
available. Recency is namespaced by validated local profile identity. See
[remote.md](remote.md) for authentication, host-key, human authority,
remote-path, no-image, and disconnect behavior.

## Daily launch and Dojo reopening

The normal desktop/XDG command remains `splinterm-xdg-terminal-exec`. Without a
command it creates a fresh persistent Lair with one Dojo. When an application
supplies `-- COMMAND...`, the same adapter creates a transient client-bound Lair
that is removed when the command exits or its owning Window disconnects. These
command-bearing XDG windows start with the Dojo tab strip hidden; the normal
strip-toggle action can still reveal it. Native `splinterm launch -- COMMAND...`
remains persistent. Dojo reopening is
deliberately separate, and transient XDG commands never enter Recent Dojos:

```text
splinterm-dojos     → native Recent Dojos picker
splinterm-reopen    → last locally remembered running Dojo
```

`splinterm-sessions` and `splinterm sessions` remain compatibility aliases.

The in-window Ctrl+Shift+S shortcut paints a trusted modal overlay over dimmed
live panes without creating another Wayland Window or replacing an existing tab.
Escape removes the overlay and presents the newest valid pane state. Choosing a
running Dojo opens or activates its tab; New Terminal creates a fresh
Lair and opens its initial Dojo as a tab. One Window accepts at most 32 distinct
Dojo tabs, may mix Lairs, and does not restore tab order after exit. Tabs use a
sanitized Dojo label unless ambiguity requires sanitized `Lair / Dojo` context.
Closing a tab never closes its Dojo or Splints. The overlay adapts to compact and minimal
sizes, and vertical wheel or touchpad scrolling navigates hidden actions without
reaching terminal history or mouse reporting.

A suitable Omarchy convention is Super+Enter for the normal terminal command
and Super+Shift+Enter for `splinterm-dojos`. Splinterm does not modify the
user's Hyprland configuration automatically. The picker opens only Dojos whose
complete pane layout is still running; restoring exited processes
remains an explicit lifecycle command.

## Migrating from Foot

Copy values rather than copying a whole `foot.ini`:

- Foot `font` → `main.font`. Foot `pixelsize=N` →
  `main.font-pixelsize=N`; Foot `size=N` → `main.font-point-size=N`.
  A `size=` or `pixelsize=` embedded in `main.font` is rejected because the
  face/style pattern cannot become a second sizing authority.
- Foot `dpi-aware=no` → `main.font-sizing-policy=output-scale`: a 96-DPI font
  is scaled with compositor output scale. Foot `dpi-aware=yes` →
  `main.font-sizing-policy=physical-dpi`: points use the most recently entered
  Wayland output's mode/physical-size DPI and pixel sizes remain fixed. Missing,
  invalid, or unreasonable output data falls back to 96 DPI with provenance.
  Splinterm intentionally has no `auto` value.
- Foot `initial-window-size-chars` → `main.initial-columns` and
  `main.initial-rows`.
- Foot `shell` → `main.shell`; Splinterm never evaluates it as a shell command.
- Foot `scrollback.lines` and cursor style/blink map directly.
- Foot `alpha` and `blur` are imported together from `[colors-dark]`, or from
  legacy `[colors]` when no dark section exists. `[colors-light]` is ignored
  because Splinterm has no light-theme selection state. Use `[colors] alpha`
  and `[colors] blur` only for explicit Splinterm overrides.
- Convert colors through the Omarchy generator below instead of pasting Foot's
  complete `[colors]` section.

The Foot mapping above is separate from migration of Splinterm's old key.
Legacy Splinterm `main.dpi-aware=yes` meant “follow compositor scale” and maps
only to `output-scale`. Legacy Splinterm `dpi-aware=no` forced the whole surface
to 1×, has no behavior-preserving mapping, and fails with a targeted message.
Using the legacy and new policy keys together is rejected.

Wayland `surface_scale_120` always follows compositor output geometry and is
never disabled by font policy. Font resolution records the configured unit,
policy, observed/sizing DPI provenance, compositor scale, effective 26.6 size,
and final pixel size.

Foot options outside the table—server mode, pad geometry, URL modes, arbitrary
bindings, notifications, and advanced rendering controls—are unsupported in
this MVP and produce diagnostics when represented as unknown keys.

## Native Omarchy theme integration

With `main.theme` unset, Splinterm reads the active Quattro theme directly from
`${XDG_STATE_HOME:-~/.local/state}/omarchy/current/theme/`. The effective
`foot.ini` supplies the terminal foreground/background, ANSI 16, cursor,
selection background and foreground, alpha, and blur; `colors.toml` supplies the
standard Omarchy `accent` and `lighter_bg` roles used by application chrome. The
selected-Dojo-tab body uses `lighter_bg` when present and otherwise Foot
`bright0`; it never borrows the terminal selection background. Its label and
close affordance use whichever effective Foot background or foreground has
higher WCAG contrast against that resolved tab body, preferring foreground on
an exact tie. Native Omarchy themes do not need Splinterm-specific palette
roles.

The tab-strip and selected-tab backgrounds both inherit the terminal alpha
while preserving their resolved colors. The selected-tab underline remains the
opaque UI accent. Terminal selections continue to use Foot's independent
`selection-foreground`, falling back to the terminal foreground when that Foot
role is absent. `[colors-dark]` takes precedence over legacy `[colors]`, while
absent alpha defaults opaque and absent blur defaults off.

Splinterm fingerprints the active directory plus both source files every 500 ms.
This detects Omarchy's atomic current-theme directory replacement and applies a
valid palette through the existing live theme channel without restarting the
daemon, shell, or Wayland window. A transiently incomplete replacement or
malformed live theme retains the last valid palette and reports one bounded
diagnostic. If Omarchy state is absent at startup, Splinterm uses its bundled
safe fallback.

No theme hook, generated file, or manual integration step is required. Setting
`main.theme=/path/to/theme.json` explicitly opts out of Omarchy discovery for
portable or isolated use. The strict JSON schema retains optional
`selection_foreground`, `active_tab_background`, `active_tab_foreground`,
`pane_border`, and `pane_border_active` overrides. `selection_foreground` falls
back to the normal foreground and `active_tab_background` falls back to ANSI
color 8 for older JSON themes. A missing `active_tab_foreground` uses the
same contrast algorithm against the JSON palette's resolved `background` and
`foreground`; malformed explicit values fail theme loading. The optional
`tools/generate-omarchy-theme.py` exporter derives these JSON roles from the
standard Omarchy background ramp and foreground/background endpoints.
