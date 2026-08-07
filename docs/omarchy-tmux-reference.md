# Omarchy Tmux Reference

A local reference for the Basecamp/Omarchy tmux setup introduced by DHH. This documents the **stock Omarchy defaults**, then notes where this machine's live config differs.

Verified against the installed Omarchy package on **2026-08-07**:

- Omarchy: `4.0.0.r1512.gc992cdf-1`
- Default tmux config: `/usr/share/omarchy/config/tmux/tmux.conf`
- Default shell aliases: `/usr/share/omarchy/default/bash/aliases`
- Tmux layout functions: `/usr/share/omarchy/default/bash/fns/tmux`
- Live user config: `~/.config/tmux/tmux.conf`

## Quick start

```bash
t                 # Attach to an existing session, or create "Work"
```

The Omarchy prefix is **Ctrl+Space**. The standard **Ctrl+B** prefix also works.

Inside tmux:

```text
Prefix + ?        Show Omarchy's tmux keybinding reference
Prefix + q        Reload ~/.config/tmux/tmux.conf
Prefix + d        Detach (standard tmux binding)
```

`Prefix + key` means: press `Ctrl+Space`, release it, then press `key`.

## Omarchy-defined keybindings

### Config, help, and copy mode

| Key | Action |
|---|---|
| `Ctrl+Space` | Primary prefix |
| `Ctrl+B` | Secondary/standard tmux prefix |
| `Prefix + Ctrl+Space` | Send the prefix through to a nested tmux session |
| `Prefix + ?` | Open the Omarchy tmux keybinding viewer |
| `Prefix + q` | Reload `~/.config/tmux/tmux.conf` |
| `Prefix + [` | Enter copy mode (standard tmux binding) |
| Copy mode `v` | Begin selection |
| Copy mode `y` | Copy selection and leave copy mode |

Copy mode uses vi keys.

### Panes

| Key | Result |
|---|---|
| `Alt+Enter` | Split into top/bottom panes; new pane below |
| `Alt+Shift+Enter` | Split into left/right panes; new pane beside |
| `Alt+Escape` | Kill current pane |
| `Prefix + h` | Split into top/bottom panes; new pane below |
| `Prefix + v` | Split into left/right panes; new pane beside |
| `Prefix + x` | Kill current pane |
| `Prefix + z` | Toggle current-pane zoom/fullscreen (standard tmux binding) |
| `Ctrl+Alt+Arrow` | Focus the pane in that direction |
| `Ctrl+Alt+Shift+Arrow` | Resize in that direction by 5 cells |

Omarchy's labels call `split-window -v` a “vertical” split and `split-window -h` a “horizontal” split. The result descriptions above avoid that ambiguity.

### Windows

A tmux **window** is a tab inside a session.

| Key | Action |
|---|---|
| `Prefix + c` | Create a window in the current pane's directory |
| `Prefix + k` | Kill current window |
| `Prefix + r` | Rename current window |
| `Alt+1` … `Alt+9` | Switch directly to window 1–9 |
| `Alt+Left` / `Alt+Right` | Previous/next window |
| `Alt+Shift+Left` / `Alt+Shift+Right` | Move current window left/right |

Useful standard tmux bindings still available include `Prefix + w` (window chooser), `Prefix + n`/`p` (next/previous window), and `Prefix + &` (kill window with confirmation).

### Sessions

| Key | Action |
|---|---|
| `Prefix + C` | Create a session in the current pane's directory |
| `Prefix + K` | Kill current session |
| `Prefix + R` | Rename current session |
| `Prefix + P` / `Prefix + N` | Previous/next session |
| `Alt+Up` / `Alt+Down` | Previous/next session without prefix |
| `Prefix + s` | Open the standard tmux session chooser |
| `Prefix + d` | Detach from tmux |

## Shell alias

### `t`

Defined as:

```bash
alias t='tmux attach || tmux new -s Work'
```

Use it from a normal shell outside tmux. It attaches to an existing tmux session; if attachment fails because no server/session exists, it creates a session named `Work`.

## Tmux layout functions

These are **Bash functions**, not aliases. Run them **inside an existing tmux session**—normally after starting with `t`.

### `tdl` — Tmux Dev Layout

```bash
tdl <ai-command> [second-ai-command]
```

Examples:

```bash
tdl c               # Neovim + OpenCode + terminal
tdl cx              # Neovim + Claude Code + terminal
tdl cy              # Neovim + Codex + terminal
tdl c cx            # Neovim + OpenCode + Claude Code + terminal
```

Layout:

```text
┌──────────────────────┬───────────┐
│                      │ AI 1      │
│  $EDITOR .           │           │
│                      ├───────────┤  optional AI 2
│                      │ AI 2      │
├──────────────────────┴───────────┤
│ terminal (15% height)            │
└──────────────────────────────────┘
```

Behavior:

- Renames the window to the current directory's basename.
- Keeps all panes in the current directory.
- Starts `$EDITOR .` in the left pane.
- Starts the requested AI command in the right pane.
- Adds a second right-side AI pane when a second command is supplied.

### `tds` — Tmux Dev Square

```bash
tds
```

Creates four panes:

| Pane | Command |
|---|---|
| Top-left | `nvim .` |
| Top-right | `hunk diff --watch` |
| Bottom-left | Terminal |
| Bottom-right | `opencode` |

### `tdlm` — Tmux Dev Layout Multiplier

```bash
tdlm <ai-command> [second-ai-command]
```

Examples:

```bash
tdlm c
tdlm c cx
```

Creates one `tdl` window for every immediate subdirectory of the current directory. It also renames the session after the parent directory.

This is useful from a directory containing several projects or packages.

### `tsl` — Tmux Swarm Layout

```bash
tsl <pane-count> <command>
```

Examples:

```bash
tsl 4 c
tsl 3 cx
tsl 4 'codex --full-auto'
```

Creates the requested number of tiled panes in the current directory and runs the same command in every pane.

## Convenience AI aliases

These support the tmux layout functions:

| Alias | Installed command | Purpose |
|---|---|---|
| `c` | `opencode --auto` | OpenCode in automatic mode |
| `cx` | `claude --permission-mode bypassPermissions` after clearing the terminal | Claude Code with permission prompts bypassed |
| `cy` | `codex -s danger-full-access -a never` | Codex with full local access and no approval prompts |
| `ic` | `tdl c` | Dev layout with OpenCode |
| `ix` | `tdl cx` | Dev layout with Claude Code |
| `icx` | `tdl c cx` | Dev layout with OpenCode and Claude Code |

The `c`, `cx`, and `cy` modes intentionally relax safety/approval controls. Use them only in directories where that level of access is appropriate.

## Useful inspection commands

```bash
# Show the bindings parsed from your live tmux config
omarchy-menu-tmux-keybindings --print

# Show aliases exactly as Bash sees them
alias t c cx cy ic ix icx

# Show function definitions
type tdl tds tdlm tsl

# List sessions
tmux ls

# Reload the live config from inside tmux
tmux source-file ~/.config/tmux/tmux.conf
```

Inside tmux, `Prefix + ?` is the fastest reference.

## Stock defaults versus this machine's live config

This machine's `~/.config/tmux/tmux.conf` is customized and does **not** exactly match the current packaged default.

Important differences observed on 2026-08-07:

- Live `Prefix+h` runs `split-window -h` and live `Prefix+v` runs `split-window -v`; this is the reverse of the current Omarchy default documented above.
- Live config adds `Alt+Shift+Up/Down` to swap panes.
- Live config loads TPM (`tmux-plugin-manager`), which adds plugin-management bindings.
- Live status-bar styling is customized.
- The packaged default now includes binding descriptions and a reload confirmation.

To inspect only the packaged default without changing anything:

```bash
less /usr/share/omarchy/config/tmux/tmux.conf
```

Do not overwrite the live config merely to consult the defaults. Omarchy's refresh operation can replace config and should only be used deliberately.

## Known source caveat

At the time of verification, the installed/upstream `tdl` function ends with:

```bash
tmux select-pane -t "$opencode_pane"
```

but `tdl` defines `editor_pane`, `ai_pane`, and `ai2_pane`—not `opencode_pane`. The layout is created and commands are launched before this line, but the final focus-selection command may report an invalid/empty target. The likely intended target is `$editor_pane`.

## Sources

- Omarchy repository: <https://github.com/basecamp/omarchy>
- Current upstream tmux config: <https://github.com/basecamp/omarchy/blob/dev/config/tmux/tmux.conf>
- Current upstream aliases: <https://github.com/basecamp/omarchy/blob/dev/default/bash/aliases>
- Current upstream tmux functions: <https://github.com/basecamp/omarchy/blob/dev/default/bash/fns/tmux>
- DHH's Omarchy Manual, Hotkeys: <https://learn.omacom.io/2/the-omarchy-manual/53/hotkeys#tmux>
- Omarchy 3.4.0 release introducing the tailored tmux setup: <https://github.com/basecamp/omarchy/releases/tag/v3.4.0>
