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
| `main.resize-delay-ms` | bounded delay before resize command | 0–1000; 0 |
| `main.dpi-aware` | deprecated **legacy Splinterm** key: `yes` maps only to `output-scale`; `no` fails with migration guidance | unset |
| `main.theme` | generated JSON role map | `~/.config/splinterm/theme.json` |
| `colors.alpha` | Foot-compatible default-background translucency | 0.0–1.0; 1.0 |
| `scrollback.lines` | daemon terminal history budget | 0–1,000,000; 1000 |
| `cursor.style` | `block`, `beam`, or `underline` | block |
| `cursor.blink` | permit cursor blink | yes |
| `multiplexer.divider-style` | `line`, `frame`, or `none` pane chrome | line |
| `multiplexer.frame-title` | top-frame title source: `splint` or `none`; inert outside frame style | splint |

Malformed supported values fail startup. Unknown sections and keys print
line-numbered diagnostics. `[colors] alpha` follows Foot's default alpha mode:
only cells whose background source is default are translucent; explicit and
reverse-video backgrounds remain opaque. `alpha-mode` and blur are not yet
supported. Other `[colors]` options direct users to generated `theme.json`, and
`[key-bindings]` options explain that MVP bindings are not remappable. This
avoids claiming arbitrary `foot.ini` compatibility.

Built-in local bindings include Ctrl+Shift+C/V for copy/paste,
Ctrl+Shift+R to revoke active access, and Ctrl+Shift+L to release control.
Ctrl+Shift+T requests transfer from the current controller; its trusted UI uses
Ctrl+Shift+Y/N to accept/deny, while Ctrl+Shift+U opens separate trusted
confirmation for forced takeover. Ctrl+Shift+F opens local literal scrollback
search; Enter submits, Ctrl+N/P navigates, and Escape closes the trusted search
surface. These control/search bindings are not terminal-controlled and are not
currently remappable. Foot-compatible runtime zoom uses Ctrl+plus/equal/KP_Add and
Ctrl+minus/KP_Subtract in 0.5-point steps; Ctrl+0/KP_0 resets the configured
size. Terminal key mappings otherwise follow the implemented Foot/xterm behavior.

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

## Theme role bridge

`tools/generate-omarchy-theme.py THEME/colors.toml` maps Omarchy `bg`, `fg`,
ANSI roles, accent, selection, muted pane border, active pane border, and blue
URL roles into a strict project-owned JSON file. Writes are atomic. Splinterm
polls that file every 500 ms, validates all roles and colors, and repaints the
terminal palette, cursor, selection, URL, and focus chrome without restarting
the daemon or shell. The optional `pane_border` role defaults to a midpoint of
background and foreground, while `pane_border_active` defaults to `ui_accent`,
so older generated themes remain valid. Invalid changes are rejected and the
last valid palette remains active; a missing startup file uses the documented
safe fallback in `config/splinterm/theme.json`.

After installing the generator, Omarchy 4 users should copy
`config/omarchy/hooks/theme-set.d/10-splinterm.sh` into
`~/.config/omarchy/hooks/theme-set.d/`. The legacy single-hook template remains
at `config/omarchy/hooks/theme-set`. Both read the active Omarchy state/theme
directory and never modify `/usr/share/omarchy/` or a stock theme.
