# Dojo presets

Splinterm preset files describe complete Dojo layouts without shell injection.
Static catalogs compile client-side, while non-dry runs materialize the complete
layout through one trusted, revision-checked daemon transaction.

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

## Inspection

```text
splinterm preset list
splinterm preset show personal-review
splinterm preset check
splinterm preset check ./other-presets.toml
splinterm preset run personal-review --cwd ~/Code/project --dry-run
splinterm preset run personal-review --cwd ~/Code/project
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
success.
