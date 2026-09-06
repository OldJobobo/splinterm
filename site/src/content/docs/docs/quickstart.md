---
title: Quickstart
description: Open a new Splinterm terminal, detach from work, and return to the running session.
---

Open a terminal, split it into panes, close its Window, and return to the same running work. This workflow uses the default **persistent** lifetime.

:::caution[Check the lifetime before closing]
If you set `multiplexer.persistent-by-default=no`, an ordinary unnamed Lair belongs to its Window and closing that Window ends its processes unless the Lair has been promoted. Command-bearing XDG launches start client-bound and end with their command or owning Window unless promoted. See [Terminal lifetime](/docs/configure/configuration/#terminal-lifetime).
:::

## 1. Install Splinterm

Follow [Installation](/docs/install/) from an x86_64 Omarchy/Arch system.

## 2. Open a new terminal

Use the installed desktop entry or run:

```bash
splinterm-xdg-terminal-exec
```

A commandless launch creates a **Lair** (your workspace), one **Dojo** (its terminal layout), and one **Splint** (a terminal pane). The Lair is persistent by default. Native `splinterm launch -- COMMAND...` is also persistent; an application-supplied XDG command instead starts in a client-bound Lair. While unpromoted, that Lair ends when its initial command exits or its owning Window disconnects.

## 3. Split the Dojo

With the default `splinterm` keymap, press **Ctrl+Shift+Enter** for a horizontal split or **Ctrl+Shift+\\** for a vertical split. Each pane is a Splint. Use **Ctrl+Shift+Arrow** to move between them; **Ctrl+Shift+D** creates another Dojo, shown as a tab.

## 4. Leave persistent work running

Start a recognizable process in a Splint, then close the graphical Window. With the default persistent lifetime, this detaches the view without terminating its Dojos or Splints. The daemon must remain running: restarting it or rebooting ends processes.

## 5. Return through Recent Dojos

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
| Copy / paste | Ctrl+Shift+C / Ctrl+Shift+V (Super+C/V and Ctrl+Insert/Shift+Insert aliases) |

Super shortcuts work only when the compositor delivers the chord to the Splinterm Window. When Omarchy classifies `com.oldjobobo.splinterm` as a terminal, it delivers universal copy/paste as `Ctrl+Insert`/`Shift+Insert`, which Splinterm accepts while preserving ordinary `Ctrl+C` terminal interrupt. While viewing historical output, plain Enter returns the focused pane to live output without submitting terminal input.

:::note[Detach is not restore]
Reopening attaches to processes that are still running. If a Splint has exited, starting it again from saved launch metadata is an explicit restore operation.
:::

The optional `omarchy-tmux` profile adds `Ctrl+Space` / `Ctrl+B` prefixes, trusted `Prefix+?` key help, and `Prefix+[` vi copy mode. See [Configuration and keymaps](/docs/configure/configuration/).

Next, learn the [core concepts](/docs/concepts/), read about [sessions and persistence](/docs/sessions/), or create complete layouts with [Dojo presets](/docs/presets/).
