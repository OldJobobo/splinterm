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
Omarchy palette or an explicit `main.theme` JSON override, and `[key-bindings]`
options explain that MVP bindings are not remappable. This
avoids claiming arbitrary `foot.ini` compatibility.

Built-in local bindings include Ctrl+Shift+C/V for copy/paste and Ctrl+Shift+S
to open the native Recent Sessions picker inside any focused managed terminal
Window. Ctrl+Tab and Ctrl+Shift+Tab cycle Window-local Dojo tabs;
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
surface. These control/search bindings are not terminal-controlled and are not
currently remappable. Foot-compatible runtime zoom uses Ctrl+plus/equal/KP_Add and
Ctrl+minus/KP_Subtract in 0.5-point steps; Ctrl+0/KP_0 resets the configured
size. Terminal key mappings otherwise follow the implemented Foot/xterm behavior.

## Daily launch and session reopening

The normal desktop/XDG command remains `splinterm-xdg-terminal-exec` and always
creates a fresh Lair with one Dojo. Session reopening is deliberately separate:

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
selection, alpha, and blur; `colors.toml` supplies the Omarchy UI accent used by
trusted surfaces and active pane chrome. `[colors-dark]` takes precedence over
legacy `[colors]`, while absent alpha defaults opaque and absent blur defaults
off.

Splinterm fingerprints the active directory plus both source files every 500 ms.
This detects Omarchy's atomic current-theme directory replacement and applies a
valid palette through the existing live theme channel without restarting the
daemon, shell, or Wayland window. A transiently incomplete replacement or
malformed live theme retains the last valid palette and reports one bounded
diagnostic. If Omarchy state is absent at startup, Splinterm uses its bundled
safe fallback.

No theme hook, generated file, or manual integration step is required. Setting
`main.theme=/path/to/theme.json` explicitly opts out of Omarchy discovery for
portable or isolated use. The strict JSON schema retains `pane_border` and
`pane_border_active` overrides; `tools/generate-omarchy-theme.py` remains only
an optional exporter for that override format.
