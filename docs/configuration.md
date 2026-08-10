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
| `main.initial-columns`, `initial-rows` | requested initial grid | 2–240, 2–80; 80×24 |
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

Built-in local bindings include Ctrl+Shift+C/V for copy/paste, Ctrl+Shift+S
to open the native Recent Sessions picker, and Ctrl+Shift+P to open the
searchable command palette inside any focused managed terminal Window. The
palette groups 31 built-ins across sessions, tabs, panes, history, view, and
control. In addition to recent sessions, tab navigation, splits, pane focus,
closing, and font zoom, it can create a session, rename the current tab, detach
other tabs, open confirmed Dojo termination, resize a pane, search/page
scrollback, return to live output, request/release/force control, revoke captured
access grants, and accept or deny a captured pending transfer. Force control is
trusted-local only and remains visibly disabled in remote Windows. Typing filters
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
client-local pane zoom, generated-help guidance, and transactional config reload.
`Prefix+[` is listed explicitly as unavailable rather than captured for another
action; copy mode arrives in its later plan milestone. Dojo/Lair sequences also
remain assigned to their later milestone.

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
backslash, brackets, Plus/Equal/Minus, zero, and KP_0. Empty or duplicate
modifiers and unsupported keys fail with source context.

An overlay applies unbinds before bindings. An unmatched unbind is a diagnostic;
duplicate or semantically overlapping chords are errors naming both sources.
Only closed application actions can be selected—configuration cannot register
shell commands or callbacks. Bindable action IDs are:

```text
app.command-palette       session.recent
clipboard.copy            clipboard.paste
dojo.new                  dojo.previous
dojo.next                 dojo.close-tab
pane.split-below          pane.split-right
pane.focus-left           pane.focus-right
pane.focus-up             pane.focus-down
pane.close                pane.resize-smaller
pane.resize-larger        pane.resize-left-5
pane.resize-right-5       pane.resize-up-5
pane.resize-down-5        pane.zoom-toggle
app.binding-help          app.config-reload
terminal.send-prefix      history.search
history.page-up           history.page-down
history.return-live       view.zoom-in
view.zoom-out              view.zoom-reset
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
PROFILE` authenticates once and opens that profile's native Recent Sessions
picker; explicit `sessions`, `reopen`, `window`, and `launch` forms are also
available. Recency is namespaced by validated local profile identity. See
[remote.md](remote.md) for authentication, host-key, human authority,
remote-path, no-image, and disconnect behavior.

## Daily launch and session reopening

The normal desktop/XDG command remains `splinterm-xdg-terminal-exec`. Without a
command it creates a fresh persistent Lair with one Dojo. When an application
supplies `-- COMMAND...`, the same adapter creates a transient client-bound Lair
that is removed when the command exits or its owning Window disconnects. Native
`splinterm launch -- COMMAND...` remains persistent. Session reopening is
deliberately separate, and transient XDG commands never enter Recent Sessions:

```text
splinterm-sessions  → native Recent Sessions picker
splinterm-reopen    → last locally remembered running Dojo
```

The in-window Ctrl+Shift+S shortcut paints a trusted modal overlay over dimmed
live panes without creating another Wayland Window or replacing an existing tab.
Escape removes the overlay and presents the newest valid pane state. Choosing a
running session opens or activates its Dojo tab; New Terminal creates a fresh
Lair and opens its initial Dojo as a tab. One Window accepts at most 32 distinct
Dojo tabs, may mix Lairs, and does not restore tab order after exit. Tabs use a
sanitized Dojo label unless ambiguity requires sanitized `Lair / Dojo` context.
Closing a tab never closes its Dojo or Splints. The overlay adapts to compact and minimal
sizes, and vertical wheel or touchpad scrolling navigates hidden actions without
reaching terminal history or mouse reporting.

A suitable Omarchy convention is Super+Enter for the normal terminal command
and Super+Shift+Enter for `splinterm-sessions`. Splinterm does not modify the
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
Omarchy UI accent used by trusted surfaces and active pane chrome. Active tab
labels and close affordances use `selection-foreground`, falling back to the
terminal foreground when that Foot role is absent. `[colors-dark]` takes
precedence over legacy `[colors]`, while absent alpha defaults opaque and absent
blur defaults off.

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
`selection_foreground`, `pane_border`, and `pane_border_active` overrides;
`selection_foreground` falls back to the normal foreground for older JSON
themes. `tools/generate-omarchy-theme.py`
remains only an optional exporter for that override format.
