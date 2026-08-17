---
title: Configuration
description: Configure Splinterm fonts, geometry, shell behavior, scrollback, pane chrome, cursor, and theme overrides.
---

The default configuration path is:

```text
${XDG_CONFIG_HOME:-~/.config}/splinterm/config.ini
```

Start from the repository example at `config/splinterm/config.ini`. Set `SPLINTERM_CONFIG` to test another file without replacing your normal configuration.

## Minimal example

```ini
[main]
font=JetBrains Mono Nerd Font:style=Regular
font-pixelsize=14
font-sizing-policy=output-scale
padding-left=12
padding-right=12
padding-top=12
padding-bottom=12
initial-columns=80
initial-rows=24
login-shell=yes

[scrollback]
lines=1000

[multiplexer]
divider-style=line
frame-title=splint

[cursor]
style=block
blink=yes

[key-bindings]
profile=splinterm
# file=keybindings.toml
# prefix-timeout-ms=1000

[presets]
# file=presets.toml
allow-unrestricted-commands=no
```

Malformed supported values fail startup. Unknown sections and keys produce line-numbered diagnostics.

## Common settings

| Key | Meaning | Default or range |
| --- | --- | --- |
| `main.font` | fontconfig pattern | JetBrains Mono Nerd Font Regular |
| `main.font-pixelsize` | pixel font size | 6–96; 14 |
| `main.font-point-size` | alternative point size | 6–96; unset |
| `main.font-sizing-policy` | `output-scale` or `physical-dpi` | `output-scale` |
| `main.padding-*` | four independent logical edges | 0–10000; 12 |
| `main.initial-columns` | initial grid columns | 2–240; 80 |
| `main.initial-rows` | initial grid rows | 2–80; 24 |
| `main.shell` | executable for an empty launch | account login shell |
| `scrollback.lines` | daemon terminal history budget | 0–1,000,000; 1000 |
| `cursor.style` | `block`, `beam`, or `underline` | `block` |
| `multiplexer.divider-style` | `line`, `frame`, or `none` | `line` |
| `multiplexer.frame-title` | `splint` or `none` | `splint` |
| `key-bindings.profile` | `splinterm` or `omarchy-tmux` | `splinterm` |
| `key-bindings.file` | optional strict TOML overlay | unset |
| `key-bindings.prefix-timeout-ms` | prefix timeout in milliseconds | 250–5000; 1000 |
| `presets.file` | optional strict preset catalog | unset |
| `presets.allow-unrestricted-commands` | enable packaged `c`, `cx`, `cy` aliases | `no` |

## Font sizing and Wayland scale

The default `main.font-sizing-policy=output-scale` follows Wayland compositor output geometry, including fractional output scale. Choose `physical-dpi` when point-sized fonts should instead follow the output's reported mode and physical-size DPI.

`main.font-pixelsize` and `main.font-point-size` are mutually exclusive. Pixel-sized fonts remain fixed according to the selected sizing policy; point-sized fonts are converted from the effective DPI. See [Why native Wayland?](/docs/wayland/) for how scaling fits into the native client.

## Omarchy theme integration

With `main.theme` unset, Splinterm reads the active Omarchy theme from `${XDG_STATE_HOME:-~/.local/state}/omarchy/current/theme/`. The effective `foot.ini` supplies terminal colors, selection, alpha, and blur; `colors.toml` supplies standard Omarchy accent and background-ramp roles. The active-tab body uses `lighter_bg`, then Foot `bright0`, rather than terminal selection color.

Active tab text is independent from terminal selection text. Splinterm chooses whichever effective terminal background or foreground has higher WCAG contrast against the resolved active-tab body, preferring foreground on a tie. Native Omarchy themes need no Splinterm-specific roles. Explicit JSON themes may override the active-tab roles and use the same fallback rule against their own background and foreground roles.

Valid theme changes reload without restarting the daemon, shell, or window. A malformed or incomplete replacement retains the last valid palette.

Set an explicit JSON palette only to opt out of native discovery:

```ini
[main]
theme=~/.config/splinterm/theme.json
```

## Keymaps and the command palette

Splinterm binds only a closed registry of application actions. Keyboard dispatch, command-palette labels, tab menus, and generated help therefore describe the same resolved keymap. Configuration cannot register shell commands or callbacks.

Inspect the active configuration without contacting the daemon:

```bash
splinterm config check
splinterm keymap list
splinterm keymap show
splinterm keymap show omarchy-tmux
splinterm keymap conflicts
```

The default `splinterm` profile provides the controls in the [quickstart](/docs/quickstart/). The `omarchy-tmux` profile adds familiar `Ctrl+Space` and `Ctrl+B` prefixes, pane and tab workflows, local pane zoom, stable-ID choosers, `Prefix+B` tab-strip toggling, trusted `Prefix+?` key help, transactional configuration reload, and `Prefix+[` vi copy mode. `dojo.close-other-tabs` is available to strict overlays even though neither profile claims a default chord.

The command palette is a curated trusted subset of this closed registry. It projects shortcut labels from the effective resolved keymap and exposes binding help, reload, copy mode, pane zoom, Dojo/Lair workflows, and Window detach without allowing configuration, plugins, or terminal output to register commands.

Both profiles provide terminal `Super+C/V`, retain `Ctrl+Shift+C/V`, and accept Omarchy's terminal-tagged `Ctrl+Insert`/`Shift+Insert`. Omarchy must classify `com.oldjobobo.splinterm` as a terminal; without that classification its universal copy branch may inject ordinary `Ctrl+C`, which remains terminal interrupt. Splinterm-owned fields use local `Super+C/V/X/Z`, while terminal-pane `Super+X/Z` remain application-owned. These Super shortcuts work only when the compositor delivers the chord to the Splinterm Window. In copy mode, move with `h/j/k/l`, arrows, Home/End, or PageUp/PageDown. Press `v` to begin selecting, `y` or `Super+C` to publish to the Wayland clipboard and exit, or Escape to cancel. `Super+V/X/Z`, pointer input, paste, and IME text are consumed locally and never forwarded to the terminal application.

While a focused pane is viewing historical output, plain Enter or keypad Enter returns it to live output without sending terminal input. A later Enter pressed while already live submits normally.

A strict overlay can unbind and replace closed actions:

```toml
version = 1
inherits = "splinterm"

[[unbind]]
sequence = ["Ctrl+Shift+P"]

[[binding]]
sequence = ["Ctrl+Alt+P"]
action = "app.command-palette"
```

Unknown actions, malformed chords, duplicate bindings, and semantic conflicts fail startup with source context rather than partially applying.

## Dojo presets

The optional `[presets]` file describes complete named pane trees. Splinterm always ships the bounded `omarchy.t`, `omarchy.tdl`, `omarchy.tds`, `omarchy.tdlm`, and `omarchy.tsl` workflows. Inspection and dry-run are local; real runs commit the complete layout atomically.

Read [Dojo presets](/docs/presets/) for catalog syntax, commands, parameters, and the optional collision-safe `s`, `sdl`, `sds`, `sdlm`, and `ssl` Bash helpers.

## Remote profiles

SSH endpoints use a separate strict TOML file at `${XDG_CONFIG_HOME:-~/.config}/splinterm/remotes.toml`. Profile configuration cannot inject arbitrary SSH options or remote commands.

```bash
splinterm remote list
splinterm remote inspect PROFILE
splinterm remote check PROFILE
```

Read [Remote access](/docs/remote/) for the profile schema and graphical versus automation authority.

## Moving from Foot

Copy supported values rather than copying a complete `foot.ini`:

- Foot `font` maps to `main.font`.
- Foot `pixelsize=N` maps to `main.font-pixelsize=N`.
- Foot `size=N` maps to `main.font-point-size=N`.
- Foot `initial-window-size-chars` maps to columns and rows.
- Foot `shell`, scrollback lines, cursor style, alpha, and blur have bounded mappings.

Arbitrary bindings, notifications, server mode, URL modes, and advanced rendering controls are not currently supported through Splinterm configuration.

Theme alpha and blur are native Wayland presentation features. Blur is requested only when translucency is active, the theme enables it, and the compositor advertises compatible protocol support. Unsupported compositors continue without blur.
