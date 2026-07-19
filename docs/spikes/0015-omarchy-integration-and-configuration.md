# Spike 0015: Omarchy integration and configuration

- **Status:** Implemented and validated on workspace 8
- **Plan:** [Plan 0002, Phase 8](../plans/0002-omarchy-terminal-mvp.md)

## Stable identity

The selected application ID is `com.oldjobobo.splinterm`. It is shared by the
Wayland app ID, desktop file and desktop launchable, icon, AppStream metadata,
and documented systemd user units under `dist/`.

## Launch contract

`dist/bin/splinterm-xdg-terminal-exec` is the stable terminal launcher and
executes `splinterm launch` without evaluating a shell string. The desktop file
advertises `X-TerminalArgExec=--` and
`X-TerminalArgDir=--working-directory=`. `splinterm launch` creates the MVP's
single daemon-owned Splint when none exists, then opens the native client. A
supplied command is carried as a bounded vector of arguments in protocol v8 and
passed directly to the PTY backend. Working directory is a separate path field.
If the daemon is unavailable, the launcher exits nonzero with a message naming
the user service and direct daemon command. Because the MVP still supports one
live Splint, an execute-command launch is rejected rather than silently
attaching when a process already exists.

## Theme bridge

`tools/generate-omarchy-theme.py` strictly maps Omarchy `colors.toml` roles to
the project-owned `theme.json` schema and writes atomically. The template hook
at `config/omarchy/hooks/theme-set` reads custom or stock Omarchy theme data and
writes only under the user's Splinterm config directory. It never changes
`/usr/share/omarchy`.

Open clients poll the generated file every 500 ms. Valid changes update default
foreground/background, ANSI 16, cursor, selection, URL, and trusted UI accent,
then invalidate the rendered frame without restarting the daemon or shell.
Invalid updates retain the last valid theme. Missing startup input uses the
safe bundled palette.

## Configuration subset

`config/splinterm/config.ini` and [the migration guide](../configuration.md)
define the supported subset: font family/size, initial grid, shell and login
behavior, fixed title, scrollback size, cursor style/blink, resize delay,
DPI-awareness, and generated theme path. Unsupported keys produce line-numbered
diagnostics; arbitrary Foot compatibility is not claimed.

## Automated validation

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
python tools/generate-omarchy-theme.py /path/to/colors.toml --output /tmp/theme.json
sh -n dist/bin/splinterm-xdg-terminal-exec config/omarchy/hooks/theme-set
```

Tests cover strict config/theme parsing, unsupported-key diagnostics, direct
argv preservation, bounded launch fields, cursor styles, theme repainting, and
the existing daemon lifecycle.

## Workspace 8 procedure

1. Build sibling `splinterd` and `splinterm` binaries and start an isolated
   daemon.
2. Generate a project-owned theme from the current Omarchy `colors.toml`.
3. Set `SPLINTERM_CONFIG` to an isolated config referencing that theme.
4. Switch directly with `hl.dsp.focus({ workspace = "8" })`, launch through
   `dist/bin/splinterm-xdg-terminal-exec`, and grant trusted access.
5. Confirm `hyprctl clients -j` reports class `com.oldjobobo.splinterm` and
   workspace 8.
6. Validate a cwd containing spaces and a command whose arguments contain shell
   metacharacters; confirm no interpolation occurs.
7. Rewrite the generated theme atomically and capture the same live shell before
   and after the repaint.
8. Close the client and confirm the daemon-owned process remains available.

## Recorded result

Validated on the installed Hyprland session on 2026-07-18 using an isolated
socket and config under `/tmp/splinterm-phase8`. The dist launcher opened the
trusted prompt on workspace 8; `hyprctl clients -j` reported class
`com.oldjobobo.splinterm`. After grant, the title was
`oldjobobo@wintermute:/tmp/splinterm phase8 cwd — local controller — EXTERNAL ACCESS ACTIVE`,
confirming the separate cwd contract preserved a path containing spaces.

The live theme file was replaced atomically while the shell remained attached.
Captured dominant background pixels changed from the generated current Omarchy
palette `(10, 12, 10)` to the fallback fixture palette `(14, 18, 22)` without a
new client or daemon PID. Ephemeral captures are
`/tmp/splinterm-phase8/before-theme.png` and `after-theme.png` and are not
committed.

A direct `/usr/bin/printf` launch received the literal argument
`$(touch /tmp/splinterm-phase8/SHOULD_NOT_EXIST); spaced argument`; the marker
was not created, proving no shell interpolation. Launching against a missing
socket exited 1 and named `com.oldjobobo.splinterm-daemon.service` plus the direct
`splinterd` recovery command. The isolated daemon remained alive throughout
the graphical theme run and was stopped explicitly after validation.
