# Human usage

This is the repository authority for operating Splinterm as a person: opening,
detaching, returning to, arranging, and deliberately ending persistent terminal
work. See [CLI reference](cli.md) for the complete command inventory and
[Automation](automation.md) for machine contracts.

## The four persistent concepts

- **Lair** — a named project or persistent session.
- **Dojo** — one persistent terminal layout inside a Lair.
- **Splint** — one terminal pane and process lifecycle inside a Dojo.
- **Window** — a disposable native Wayland view. It may display up to 32 Dojos as
  client-local tabs, including Dojos from different Lairs.

`splinterd` owns Lairs, Dojos, Splints, shells, layouts, terminal state, and
scrollback. `splinterm` displays and controls that state. Closing a Window or tab
detaches a view; it does not terminate the corresponding daemon resources.

## Start, detach, and return

Open a fresh terminal through the installed desktop entry or XDG launcher:

```bash
splinterm-xdg-terminal-exec
```

The normal launch creates a fresh Lair with one Dojo and one live Splint. Closing
the Window leaves the work running in `splinterd`.

Return through the native recent-Dojo workflow:

```bash
splinterm dojos
splinterm reopen
```

`dojos` opens the trusted Recent Dojos picker. Select a running Dojo or choose
New Terminal. `sessions` remains a compatibility alias for `dojos`. `reopen`
attaches the most recently remembered Dojo whose complete layout is still
running. Neither command silently restores an exited
process.

To inspect Lairs and Dojos without mapping a Window:

```bash
splinterm list
splinterm list --all
```

`list` emphasizes active Lairs; `--all` includes exited-only history and complete
topology. Stable IDs from list/topology output can select an exact attachment:

```bash
splinterm launch --splint-id SPLINT_ID
splinterm window --lair-id LAIR_ID --dojo-id DOJO_ID
```

A normal `splinterm launch` creates a fresh graphical Lair. `window` requires the
explicit Lair/Dojo pair and renders that saved layout.

## Windows, tabs, and the Dojo picker

A Window holds an ordered, client-local set of distinct Dojo tabs:

- `Ctrl+Shift+D` creates a Dojo in the active tab's Lair and opens it;
- `Ctrl+Tab` and `Ctrl+Shift+Tab` move through tabs;
- `Ctrl+Shift+Q` detaches the active tab and closes the Window if it was last;
- `Ctrl+Shift+B` toggles the Dojo tab strip without changing the tab set;
- `Ctrl+Shift+S` opens Recent Dojos inside the same Window;
- choosing an already-open Dojo activates its tab rather than duplicating it;
- choosing another running Dojo adds it without changing daemon topology; and
- choosing New Terminal creates a fresh Lair and opens its initial Dojo.

The trusted tab strip provides activation, a close target, and a `+` picker
action. It is visible by default; hiding it is local to the current Window,
reclaims its height for the pane grid, and does not persist or reappear when a
new tab opens. Right-clicking a visible tab opens a tab-targeted menu for Rename Tab,
Activate Tab, New Dojo, detach-only Close Tab, detach-only Close Other Tabs, and
confirmed Terminate Dojo. Opening the menu does not first activate its tab.

**Detach and terminate are different operations.** Closing tabs or Windows is
client-local. Terminating a Dojo is a named, confirmed daemon mutation and ends
its pane processes.

## Panes and layouts

The built-in application controls are:

| Action | Default control |
| --- | --- |
| Command palette | `Ctrl+Shift+P` |
| Recent Dojos | `Ctrl+Shift+S` |
| Split below | `Ctrl+Shift+Enter` |
| Split right | `Ctrl+Shift+\` |
| Focus pane | `Ctrl+Shift+Arrow` |
| Close focused managed pane | `Ctrl+Shift+W` |
| Resize pane smaller/larger | Command palette |
| New Dojo tab | `Ctrl+Shift+D` |
| Previous/next Dojo tab | `Ctrl+Shift+Tab` / `Ctrl+Tab` |
| Detach active tab | `Ctrl+Shift+Q` |
| Toggle Dojo tab strip | `Ctrl+Shift+B` (`Prefix+B` in `omarchy-tmux`) |
| Search scrollback | `Ctrl+Shift+F` |
| Page history | `Shift+PageUp` / `Shift+PageDown` |
| Return to live output | `Shift+End` |
| Copy / paste | `Ctrl+Shift+C` / `Ctrl+Shift+V` |
| Release control | `Ctrl+Shift+L` |
| Request control transfer | `Ctrl+Shift+T` |
| Accept / deny transfer | `Ctrl+Shift+Y` / `Ctrl+Shift+N` |
| Open forced-takeover confirmation | `Ctrl+Shift+U` |
| Revoke active access | `Ctrl+Shift+R` |
| Zoom in / out / reset | `Ctrl++` / `Ctrl+-` / `Ctrl+0` |

Application-owned chords are consumed rather than forwarded to the PTY. In a
legacy direct single-Splint attachment, `Ctrl+Shift+W` remains terminal input;
it closes only a focused Splint in a managed multi-pane Window.

The command palette exposes the same typed action registry used by keyboard
bindings and labels unavailable actions rather than guessing a target. Type to
filter, use arrows to navigate, Enter to run, and Escape to close.

See [Configuration](configuration.md#keymap-configuration) for the strict built-in
profile and TOML overlay. Configuration can bind only closed application actions;
it cannot register shell commands or callbacks.

With the `omarchy-tmux` profile, `Prefix+B` toggles the Dojo tab strip,
`Prefix+?` opens trusted read-only help generated from the resolved keymap, and
`Prefix+[` enters client-local vi copy mode.
Move with `h/j/k/l`, arrows, Home/End, or PageUp/PageDown; press `v` to begin a
selection, `y` to publish it to the Wayland clipboard and exit, or Escape to
cancel. Copy mode never forwards these keys, pointer input, paste, or IME text to
the terminal application. Outside copy mode, `Super+C/V` provide terminal
copy/paste. Splinterm-owned command-palette, search, and rename fields also offer
bounded local selection, cut, paste, and undo; terminal `Super+X/Z` remain owned
by the running application.

The same profile ships atomic `omarchy.t`, `omarchy.tdl`, `omarchy.tds`,
`omarchy.tdlm`, and `omarchy.tsl` presets. Optional Bash functions use the
separate `s`, `sdl`, `sds`, `sdlm`, and `ssl` names and install only through an
explicit collision-safe workflow. See [Presets](presets.md) for exact behavior.

## Pointer, selection, clipboard, and URLs

- Click a pane to focus it; click a tab to activate it.
- Drag terminal text to select. Selection is client-local presentation state.
- `Ctrl+Shift+C` copies the active selection and `Ctrl+Shift+V` pastes clipboard
  text through the controlled terminal input path.
- Primary selection, URL recognition/activation, and pointer reporting follow the
  implemented Foot-derived terminal behavior and current control ownership.
- Right-clicking trusted tab chrome opens the tab menu; terminal-controlled
  content cannot paint or activate trusted chrome.

Clipboard and primary-selection contents are terminal data, not authority.
Pasting requires the graphical client to hold terminal input control.

## Scrollback and search

`Ctrl+Shift+F` opens local literal scrollback search. Enter submits, `Ctrl+N` and
`Ctrl+P` navigate matches, and Escape closes the trusted search surface. Search
queries are not injected into the terminal. History paging remains bounded and
preserves follow-live behavior when returning with `Shift+End`.

The CLI can also read bounded history pages and search without mapping a Window:

```bash
splinterm scrollback SPLINT_ID --max-rows 32
splinterm search SPLINT_ID 'literal text'
```

Use opaque continuation cursors returned by machine output rather than inventing
positions. See [CLI reference](cli.md#terminal-observation-and-input).

## Control ownership

Only one connection controls a live Splint at a time. Other Windows may remain
observers. Request/release/accept/deny operations are explicit, and forced
takeover is a separate trusted-local confirmation. A remote graphical Window may
request normal control but cannot use trusted-only forced takeover.

Closing a controlling Window releases its connection-owned lease. A later client
must acquire control explicitly; observation never silently becomes input
authority.

## Exit, restore, and relaunch

A Splint has a stable ID across process exits. Each new process under that leaf
has a new positive **incarnation**. This prevents stale clients from silently
retargeting a replacement process.

- `kill SPLINT_ID` ends a live process but retains its Splint leaf.
- `restore SPLINT_ID` starts an exited Splint from saved launch metadata.
- `restore-dojo DOJO_ID` and `restore-lair LAIR_ID` restore all exited leaves in
  the selected saved scope.
- `relaunch SPLINT_ID [-- ARGV...]` starts a new incarnation, optionally with new
  direct argv.
- `close SPLINT_ID` removes an exited leaf and collapses its layout branch.
- `close-dojo DOJO_ID` removes a Dojo only after every Splint has exited.

Restore is never automatic. Saved commands are inert metadata until a person or
properly authorized tool requests restoration. Human `kill` prompts unless
`--yes` is supplied; `close` and `close-dojo` remove only already-exited topology
without another interactive prompt. Machine forms require `--yes` for all three.
Use `--yes` only for an already-approved unattended operation, not as a substitute
for intent.

## Reset and daemon lifetime

The daemon owns all running shells. Stopping it, installing a private-protocol-
incompatible version, or resetting it ends those processes. A package upgrade
also replaces the trusted client inode: close and reopen every existing
Splinterm Window after replacement so it uses the exact client sibling adjacent
to the running daemon. To back up and clear
every session in one guarded workflow:

```bash
splinterm reset
```

The command confirms, stops the daemon, moves persistent session state to a
reported backup, and restarts cleanly. It does not remove policy or user
configuration. Read [Headless operation](headless.md) before service, backup,
policy, or recovery work.

## Remote human use

Remote profiles are strict local TOML records. Inspect and probe one without
mapping a Window:

```bash
splinterm remote list
splinterm remote inspect PROFILE
splinterm remote check PROFILE
```

Open the profile's native picker or an explicit workflow with:

```bash
splinterm --remote PROFILE
splinterm --remote PROFILE dojos
splinterm --remote PROFILE reopen
```

OpenSSH authenticates the human account. The graphical relay does not create a
public daemon listener, and machine automation remains separately policy-scoped.
Remote images and trusted-only forced takeover are intentionally unavailable.
See [Remote access](remote.md).

## Configuration and troubleshooting

The normal configuration path is
`${XDG_CONFIG_HOME:-~/.config}/splinterm/config.ini`. Validate it and the selected
keymap without contacting the daemon:

```bash
splinterm config check
splinterm keymap conflicts
```

Use [Configuration](configuration.md) for supported keys and Omarchy theme
integration, [Headless operation](headless.md) for daemon/service failures, and
[Current status](status.md) for validated environment and compatibility limits.
