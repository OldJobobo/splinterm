# Dojo presets

Splinterm preset files describe complete Dojo layouts without shell injection.
Catalogs compile client-side, while non-dry runs materialize the complete layout
through one trusted, revision-checked daemon transaction. Splinterm ships five
bounded Omarchy workflows; an optional user file overlays them.

## Configuration

```ini
[presets]
file=presets.toml
allow-unrestricted-commands=no
```

Relative paths resolve beside the selected `config.ini`. An explicitly selected
file must be readable and valid. Without a file, the packaged Omarchy catalog
remains available. User preset names shadow packaged names. User command aliases
shadow packaged aliases only for user-owned presets and cannot rewrite bundled
layouts. The daemon never reads either catalog.

## Static schema

```toml
version = 1

[commands.editor]
kind = "editor-env"
fallback = ["nvim"]
append = ["."]

[commands.review]
kind = "argv"
argv = ["codex", "-a", "on-request"]

[presets.personal-review]
kind = "dojo"
display-name = "Review workspace"
name = "{cwd.basename}"
root = "main"
focus = "editor"

[presets.personal-review.nodes.main]
type = "split"
orientation = "columns"
ratio = 650
first = "editor"
second = "review"

[presets.personal-review.nodes.editor]
type = "pane"
command = "editor"
cwd = "{cwd}"
title = "editor"

[presets.personal-review.nodes.review]
type = "pane"
command = "review"
cwd = "{cwd}"
title = "review"
```

Every table rejects unknown fields. A catalog is limited to 256 KiB, 64 command
aliases, and 64 presets. Catalog, command, preset, and node names are bounded
ASCII identifiers. Display names and pane titles are bounded UTF-8. `{cwd}` and
`{cwd.basename}` are the only placeholders.

A split has two distinct named children. `columns` compiles to a left/right
branch and `rows` to top/bottom. Its ratio is the first child's share in
thousandths, from 1 through 999. Named nodes must form one acyclic tree: cycles,
reused children, missing children, orphans, depths over 32, and more than 32
panes fail validation. `root` must name a node and `focus` must name a reachable
pane.

A pane sets exactly one launch source:

- `command="alias"` uses a source-scoped catalog command alias;
- `parameter-command="name"` uses a resolved command parameter;
- `shell=true` uses the configured direct login-shell launch policy.

`when-parameter="name"` removes a pane when that parameter is absent. Its parent
split collapses when only one child remains; removing every pane or the focused
pane is an error. Parameters are declared as bounded `integer` or `command`
values and supplied with repeated `--param NAME=VALUE` arguments:

```toml
[[presets.swarm.parameter]]
name = "count"
type = "integer"
min = 1
max = 16
required = true

[[presets.swarm.parameter]]
name = "command"
type = "command"
required = true

[presets.swarm.nodes.root]
type = "grid"
count = "{count}"
pane-command = "{command}"
cwd = "{cwd}"
```

A grid uses stable row-major pane keys (`root.0`, `root.1`, …) and compiles into
a deterministic near-square binary tree. Integer ranges, final pane/depth
limits, and all launch bounds are checked after expansion.

Pane cwd values may be absolute or relative to the invocation root. Compilation
requires every final cwd to be an existing absolute directory. This client-side
check is not daemon authority; the daemon repeats it immediately before the
atomic topology commit.

## Direct execution

`kind="argv"` arrays are passed as direct argv. Characters such as `$`, `;`,
`*`, and `?` remain literal argument bytes; they are not evaluated by a shell.
Argument count and byte limits are the same as normal `LaunchParameters`.

`kind="editor-env"` parses `$EDITOR` with Splinterm's closed compatibility
lexer, then appends the configured direct arguments. An absent or whitespace-only
value uses `fallback`.

The compatibility lexer supports ASCII space/tab separators, single quotes,
double quotes, and bounded backslash escapes. It rejects malformed quoting,
newlines, NUL, non-UTF-8 input, leading `~`, and shell-evaluation characters:

```text
$ ` | & ; < > * ? [ ] # ( ) { }
```

Errors report a byte offset and bounded class without echoing the full
environment value. Splinterm never passes compatibility strings to `sh -c`.

## Packaged Omarchy presets

- `omarchy.t` opens the first running/reopenable Dojo in recent-session order.
  If none exists, it creates `Work` with one shell Dojo at the invocation cwd.
  An existing non-reopenable `Work` is reported for explicit restore/rename.
- `omarchy.tdl --param ai=COMMAND [--param ai2=COMMAND]` creates an 85/15
  editor/AI work area over a shell. The work area is 65/35; two AI panes split
  their column 50/50. Focus is always `editor`, correcting the stale upstream
  `opencode_pane` behavior.
- `omarchy.tds` creates the exact 2×2 editor, `hunk diff --watch`, shell, and
  `opencode` layout with editor focus.
- `omarchy.tdlm --param ai=COMMAND [--param ai2=COMMAND]` bytewise-sorts the
  immediate non-hidden real child directories, rejects zero or more than 32,
  atomically renames the captured Lair to the parent basename, and creates one
  `tdl` Dojo per child in one request. Symlinked directories are not followed;
  each real child's no-follow device/inode identity is revalidated before the
  request.
- `omarchy.tsl --param count=1..16 --param command=COMMAND` creates a deterministic
  near-square command swarm at the captured cwd and focuses `root.0`.

`editor` and `hunk-watch` are normal packaged aliases. These packaged aliases are
unrestricted and disabled by default:

```text
c   opencode --auto
cx  claude --permission-mode bypassPermissions
cy  codex -s danger-full-access -a never
```

They fail before topology mutation unless the INI explicitly sets
`allow-unrestricted-commands=yes`. The flags are never silently downgraded.
Direct command parameters still use the closed lexer and direct argv.

## Optional Bash integration

Splinterm can print or explicitly install guarded Bash functions for the
packaged workflows:

```bash
splinterm preset shell-init omarchy --shell bash
splinterm preset shell-install omarchy --shell bash
```

The functions are:

```text
s                 omarchy.t
sdl AI [AI2]      omarchy.tdl
sds               omarchy.tds
sdlm AI [AI2]     omarchy.tdlm
ssl COUNT COMMAND omarchy.tsl
```

Examples:

```bash
s
sdl c
sdl c cx
sds
sdlm cy
ssl 4 'codex -a never'
```

Arguments remain separate Bash values. In particular, `ssl` requires exactly
one quoted command string after the count; Splinterm's closed compatibility
lexer converts that string to direct argv and never evaluates it as shell
source. The generated functions use `command splinterm` to avoid recursive
aliases.

This integration deliberately differs from Omarchy's tmux shell layer:

- it uses `s`, `sdl`, `sds`, `sdlm`, and `ssl`, not `t`, `tdl`, `tds`, `tdlm`,
  or `tsl`;
- it does not define or replace `ic`, `ix`, or `icx`;
- `c`, `cx`, and `cy` remain preset-local command aliases rather than global
  shell aliases;
- layouts are compiled and committed as atomic Splinterm Dojos instead of tmux
  pane/window command sequences.

`preset shell-init` only prints the file. `preset shell-install` creates
`${XDG_CONFIG_HOME:-$HOME/.config}/splinterm/shell/omarchy.bash` with owner-only
permissions and refuses to replace any existing path. It never edits `.bashrc`.
After review, source the file explicitly:

```bash
source "${XDG_CONFIG_HOME:-$HOME/.config}/splinterm/shell/omarchy.bash"
```

Before defining anything, the sourced file checks every proposed name with
Bash's command resolver. If any name is already an alias, function, builtin, or
executable, it reports every conflict and defines none of the integration
functions. This check happens at source time because only the current shell can
reliably see its aliases and functions.

## Inspection

```text
splinterm preset list
splinterm preset show personal-review
splinterm preset check
splinterm preset check ./other-presets.toml
splinterm preset run personal-review --cwd ~/Code/project --dry-run
splinterm preset run personal-review --cwd ~/Code/project
splinterm preset run omarchy.tdl --param ai=opencode
splinterm preset run omarchy.tsl --param count=4 --param command='codex -a never'
```

Inspection and dry-run are local-only. Dry-run expands placeholders, resolves
cwd, validates final launch parameters, and renders pane identities and split
ratios. It does not connect to the daemon, spawn a process, or mutate topology,
and it omits full argv from normal output.

A non-dry run must execute through the installed local `splinterm` client. It
verifies the invoking `SPLINTERM_LAIR_ID`/`DOJO_ID`/`SPLINT_ID` hints against a
fresh topology snapshot, or uses the trusted graphical-focus record when those
hints are absent. The explicit `--cwd` changes the preset root but never chooses
a Lair by name or cwd. If neither exact source identifies a current managed
Splint, the command fails without guessing.

The daemon accepts the materialization request only from the executable-verified
trusted local client. Remote-interactive, automation, and MCP clients cannot use
it. It validates all Dojos, pane trees, launch bounds, focus keys, and cwd
directories before persisting one complete topology revision. Pre-commit failure
launches no process. After commit, an individual process-launch failure leaves
that exact pane durably exited while its complete layout and siblings remain.
The response maps each preset pane key to its daemon-assigned stable Splint ID;
the client verifies that mapping against the committed topology before reporting
success. It then opens the first committed Dojo in a new native Window and uses
the daemon-authored default focus. `--no-open` retains the fully reconciled
headless behavior for layout presets; `omarchy.t` always opens.
