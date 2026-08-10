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

## Font sizing and Wayland scale

The default `main.font-sizing-policy=output-scale` follows Wayland compositor output geometry, including fractional output scale. Choose `physical-dpi` when point-sized fonts should instead follow the output's reported mode and physical-size DPI.

`main.font-pixelsize` and `main.font-point-size` are mutually exclusive. Pixel-sized fonts remain fixed according to the selected sizing policy; point-sized fonts are converted from the effective DPI. See [Why native Wayland?](/docs/wayland/) for how scaling fits into the native client.

## Omarchy theme integration

With `main.theme` unset, Splinterm reads the active Omarchy theme from `${XDG_STATE_HOME:-~/.local/state}/omarchy/current/theme/`. The effective `foot.ini` supplies terminal colors, selection, alpha, and blur; `colors.toml` supplies the Omarchy UI accent.

Valid theme changes reload without restarting the daemon, shell, or window. A malformed or incomplete replacement retains the last valid palette.

Set an explicit JSON palette only to opt out of native discovery:

```ini
[main]
theme=~/.config/splinterm/theme.json
```

## Moving from Foot

Copy supported values rather than copying a complete `foot.ini`:

- Foot `font` maps to `main.font`.
- Foot `pixelsize=N` maps to `main.font-pixelsize=N`.
- Foot `size=N` maps to `main.font-point-size=N`.
- Foot `initial-window-size-chars` maps to columns and rows.
- Foot `shell`, scrollback lines, cursor style, alpha, and blur have bounded mappings.

Arbitrary bindings, notifications, server mode, URL modes, and advanced rendering controls are not currently supported through Splinterm configuration.

Theme alpha and blur are native Wayland presentation features. Blur is requested only when translucency is active, the theme enables it, and the compositor advertises compatible protocol support. Unsupported compositors continue without blur.
