---
title: Dojo presets
description: Inspect, customize, and run complete Dojo layouts without shell injection or partial topology changes.
---

Dojo presets turn a named layout into one atomic Splinterm topology transaction. The client validates and compiles the complete pane tree before the daemon persists it or launches a process. Presets use direct argument vectors; they are not shell scripts.

## Bundled Omarchy workflows

Splinterm ships five workflows:

| Preset | Layout |
| --- | --- |
| `omarchy.t` | Reopen the first running Dojo, or create a one-shell `Work` Lair. |
| `omarchy.tdl` | Editor and one or two AI panes over a shell. |
| `omarchy.tds` | 2×2 editor, watched diff, shell, and `opencode` layout. |
| `omarchy.tdlm` | One `tdl` Dojo per immediate non-hidden real child directory. |
| `omarchy.tsl` | A deterministic 1–16 pane command swarm. |

Inspect and preview them without contacting the daemon:

```bash
splinterm preset list
splinterm preset show omarchy.tdl
splinterm preset check
splinterm preset run omarchy.tdl --cwd "$PWD" --param ai=opencode --dry-run
```

Run a preset with:

```bash
splinterm preset run omarchy.tdl --cwd "$PWD" --param ai=opencode
splinterm preset run omarchy.tdl --param ai=c --param ai2=cx
splinterm preset run omarchy.tsl --param count=4 --param command='codex -a never'
```

Successful layout runs open their first Dojo in a native Window. Add `--no-open` to keep a layout headless after full reconciliation; `omarchy.t` always opens.

:::caution[Unrestricted aliases are opt-in]
The packaged `c`, `cx`, and `cy` aliases carry deliberately unrestricted agent flags. They fail before mutation unless `[presets] allow-unrestricted-commands=yes` is explicit in `config.ini`. Splinterm never silently weakens those flags.
:::

## Optional Bash helpers

Generate the integration for review, or install it to a dedicated file:

```bash
splinterm preset shell-init omarchy --shell bash
splinterm preset shell-install omarchy --shell bash
source "${XDG_CONFIG_HOME:-$HOME/.config}/splinterm/shell/omarchy.bash"
```

The helpers use a separate collision-safe namespace:

```text
s                 omarchy.t
sdl AI [AI2]      omarchy.tdl
sds               omarchy.tds
sdlm AI [AI2]     omarchy.tdlm
ssl COUNT COMMAND omarchy.tsl
```

Installation refuses to replace an existing file and never edits `.bashrc`. When sourced, the file defines nothing if any proposed name is already an alias, function, builtin, or executable.

## Add a user catalog

Select an optional strict TOML catalog in `config.ini`:

```ini
[presets]
file=presets.toml
allow-unrestricted-commands=no
```

A catalog defines direct command aliases and bounded pane/split trees. User preset names may shadow bundled names. The schema rejects unknown fields, cycles, reused nodes, missing children, unsafe command strings, trees deeper than 32, and layouts over 32 panes.

```toml
version = 1

[commands.editor]
kind = "editor-env"
fallback = ["nvim"]
append = ["."]

[presets.review]
kind = "dojo"
display-name = "Review workspace"
name = "{cwd.basename}"
root = "main"
focus = "editor"

[presets.review.nodes.main]
type = "split"
orientation = "columns"
ratio = 650
first = "editor"
second = "shell"

[presets.review.nodes.editor]
type = "pane"
command = "editor"
cwd = "{cwd}"
title = "editor"

[presets.review.nodes.shell]
type = "pane"
shell = true
cwd = "{cwd}"
title = "shell"
```

`{cwd}` and `{cwd.basename}` are the only placeholders. Pane commands are direct argv, or values parsed by Splinterm's closed compatibility lexer; they are never passed to `sh -c`.

Non-dry runs are trusted-local human operations. Remote, automation, and MCP clients cannot invoke private preset materialization. For the complete schema and failure semantics, read repository [`docs/presets.md`](https://github.com/OldJobobo/splinterm/blob/main/docs/presets.md).
