---
title: Quickstart
description: Open a new Splinterm terminal, detach from work, and return to the running session.
---

This first workflow demonstrates Splinterm's central behavior: the graphical window can close while the daemon-owned terminal session continues running.

## 1. Install the prerelease

Follow [Installation](/docs/install/) from an x86_64 Omarchy/Arch system.

## 2. Open a new terminal

Use the installed desktop entry or run:

```bash
splinterm-xdg-terminal-exec
```

The normal launch path creates a fresh **Lair**, one **Dojo**, and one **Splint**. If you provide a command or working directory through the XDG terminal contract, the launcher preserves those arguments without rebuilding them as shell text.

## 3. Leave the work running

Start a recognizable process in the terminal, then close the graphical window. Closing the final tab detaches the client; it does not terminate the daemon-owned Dojo or its running Splints.

## 4. Return through Recent Sessions

Open the native session picker:

```bash
splinterm sessions
```

Choose the running Dojo. In a focused managed Splinterm window, **Ctrl+Shift+S** opens the same Recent Sessions workflow as trusted application chrome.

To reopen the last locally remembered running Dojo directly:

```bash
splinterm reopen
```

## Essential controls

| Action | Control |
| --- | --- |
| Command palette | Ctrl+Shift+P |
| Recent Sessions | Ctrl+Shift+S |
| New horizontal split | Ctrl+Shift+Enter |
| New vertical split | Ctrl+Shift+\\ |
| Move between panes | Ctrl+Shift+Arrow |
| Cycle tabs | Ctrl+Tab / Ctrl+Shift+Tab |
| New Dojo tab | Ctrl+Shift+D |
| Detach active tab | Ctrl+Shift+Q |
| Search scrollback | Ctrl+Shift+F |
| Copy / paste | Ctrl+Shift+C / Ctrl+Shift+V |

:::note[Detach is not restore]
Reopening attaches to processes that are still running. If a Splint has exited, starting it again from saved launch metadata is an explicit restore operation.
:::

Next, learn the [core concepts](/docs/concepts/) or read about [sessions and persistence](/docs/sessions/).
