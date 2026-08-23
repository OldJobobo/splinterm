---
title: Quickstart
description: Open a new Splinterm terminal, detach from work, and return to the running session.
---

This first workflow demonstrates Splinterm's central behavior: the graphical window can close while the daemon-owned terminal session continues running.

## 1. Install the public beta

Follow [Installation](/docs/install/) from an x86_64 Omarchy/Arch system.

## 2. Open a new terminal

Use the installed desktop entry or run:

```bash
splinterm-xdg-terminal-exec
```

A commandless launch creates a fresh **Lair**, one **Dojo**, and one **Splint**. It is persistent by default; `multiplexer.persistent-by-default=no` instead makes ordinary unnamed graphical Lairs belong to their Window. Creating or explicitly naming another Dojo promotes that Lair by default. A working directory is preserved exactly. If another application supplies a command through the XDG terminal contract, Splinterm preserves its structured argv without rebuilding shell text and always creates a transient client-bound Lair. Explicitly named and native command-bearing launches remain persistent.

## 3. Leave the work running

Start a recognizable process in the terminal, then close the graphical window. Closing the final tab detaches the client; it does not terminate the daemon-owned Dojo or its running Splints.

## 4. Return through Recent Dojos

Open the native Dojo picker:

```bash
splinterm dojos
```

Choose the running Dojo. `splinterm sessions` remains a compatibility alias. In a focused managed Splinterm window, **Ctrl+Shift+S** opens the same Recent Dojos workflow as trusted application chrome.

To reopen the last locally remembered running Dojo directly:

```bash
splinterm reopen
```

## Essential controls

| Action | Control |
| --- | --- |
| Command palette | Ctrl+Shift+P |
| Recent Dojos | Ctrl+Shift+S |
| New horizontal split | Ctrl+Shift+Enter |
| New vertical split | Ctrl+Shift+\\ |
| Move between panes | Ctrl+Shift+Arrow |
| Cycle tabs | Ctrl+Tab / Ctrl+Shift+Tab |
| New Dojo tab | Ctrl+Shift+D |
| Detach active tab | Ctrl+Shift+Q |
| Toggle tab strip | Ctrl+Shift+B |
| Search scrollback | Ctrl+Shift+F |
| Copy / paste | Super+C / Super+V (Ctrl+Shift+C/V and Ctrl+Insert/Shift+Insert aliases) |

Super shortcuts work only when the compositor delivers the chord to the Splinterm Window. When Omarchy classifies `com.oldjobobo.splinterm` as a terminal, it delivers universal copy/paste as `Ctrl+Insert`/`Shift+Insert`, which Splinterm accepts while preserving ordinary `Ctrl+C` terminal interrupt. While viewing historical output, plain Enter returns the focused pane to live output without submitting terminal input.

:::note[Detach is not restore]
Reopening attaches to processes that are still running. If a Splint has exited, starting it again from saved launch metadata is an explicit restore operation.
:::

The optional `omarchy-tmux` profile adds `Ctrl+Space` / `Ctrl+B` prefixes, trusted `Prefix+?` key help, and `Prefix+[` vi copy mode. See [Configuration and keymaps](/docs/configure/configuration/).

Next, learn the [core concepts](/docs/concepts/), read about [sessions and persistence](/docs/sessions/), or create complete layouts with [Dojo presets](/docs/presets/).
