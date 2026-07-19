# Splinterm configuration and Foot migration

Phase 8 intentionally supports a small, explicit configuration surface. The
default path is `${XDG_CONFIG_HOME:-~/.config}/splinterm/config.ini`; set
`SPLINTERM_CONFIG` to test another file. Start from
[`config/splinterm/config.ini`](../config/splinterm/config.ini).

## Supported keys

| Section/key | Meaning | Range/default |
| --- | --- | --- |
| `main.font` | fontconfig pattern | JetBrains Mono Nerd Font Regular |
| `main.font-size` | logical pixel size | 6–96; 22 |
| `main.initial-columns`, `initial-rows` | requested initial grid | 2–240, 2–80; 80×24 |
| `main.shell` | shell executable used for an empty launch | login shell from the account |
| `main.login-shell` | use login-style argv[0] for the shell | yes |
| `main.title` | fixed window title; otherwise OSC title | unset |
| `main.app-id` | diagnostic only | fixed to `com.oldjobobo.splinterm` |
| `main.resize-delay-ms` | bounded delay before resize command | 0–1000; 0 |
| `main.dpi-aware` | respond to output/fractional scale | yes |
| `main.theme` | generated JSON role map | `~/.config/splinterm/theme.json` |
| `scrollback.lines` | daemon terminal history budget | 0–1,000,000; 1000 |
| `cursor.style` | `block`, `beam`, or `underline` | block |
| `cursor.blink` | permit cursor blink | yes |

Malformed supported values fail startup. Unknown sections and keys print
line-numbered diagnostics. `[colors]` options direct users to the generated
JSON and `[key-bindings]` options explain that the MVP bindings are not
remappable. This avoids claiming arbitrary `foot.ini` compatibility.

Built-in local bindings include Ctrl+Shift+C/V for copy/paste,
Ctrl+Shift+R to revoke active access, and Ctrl+Shift+L to release control.
Terminal key mappings otherwise follow the implemented Foot/xterm behavior.

## Migrating from Foot

Copy values rather than copying a whole `foot.ini`:

- Foot `font` → `main.font`; move a `size=` component to `main.font-size`.
- Foot `initial-window-size-chars` → `main.initial-columns` and
  `main.initial-rows`.
- Foot `shell` → `main.shell`; Splinterm never evaluates it as a shell command.
- Foot `scrollback.lines` and cursor style/blink map directly.
- Convert colors through the Omarchy generator below instead of pasting Foot's
  complete `[colors]` section.

Foot options outside the table—server mode, pad geometry, URL modes, arbitrary
bindings, notifications, and advanced rendering controls—are unsupported in
this MVP and produce diagnostics when represented as unknown keys.

## Theme role bridge

`tools/generate-omarchy-theme.py THEME/colors.toml` maps Omarchy `bg`, `fg`,
ANSI roles, accent, selection, and blue URL roles into a strict project-owned
JSON file. Writes are atomic. Splinterm polls that file every 500 ms, validates
all roles and colors, and repaints the terminal palette, cursor, selection, URL,
and focus chrome without restarting the daemon or shell. Invalid changes are
rejected and the last valid palette remains active; a missing startup file uses
the documented safe fallback in `config/splinterm/theme.json`.

The hook template at `config/omarchy/hooks/theme-set` can be copied into the
user hook directory after installing the generator. It reads Omarchy data but
does not modify `/usr/share/omarchy/` or any stock theme.
