# Dojo presets

Splinterm preset files describe complete Dojo layouts without shell injection.
Milestone 5 supports strict static layouts and side-effect-free inspection. Atomic
creation arrives in Milestone 6; until then, `preset run` requires `--dry-run`.

## Configuration

```ini
[presets]
file=presets.toml
allow-unrestricted-commands=no
```

Relative paths resolve beside the selected `config.ini`. An explicitly selected
file must be readable and valid. The daemon never reads this path.

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

- `command="alias"` uses a catalog command alias;
- `shell=true` uses the configured direct login-shell launch policy.

Pane cwd values may be absolute or relative to the invocation root. Compilation
requires every final cwd to be an existing absolute directory. This client-side
check is not daemon authority; Milestone 6 repeats it at the atomic transaction
boundary.

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

## Inspection

```text
splinterm preset list
splinterm preset show personal-review
splinterm preset check
splinterm preset check ./other-presets.toml
splinterm preset run personal-review --cwd ~/Code/project --dry-run
```

Inspection is local-only. Dry-run expands placeholders, resolves cwd, validates
final launch parameters, and renders pane identities and split ratios. It does
not connect to the daemon, spawn a process, or mutate topology, and it omits full
argv from normal output.
