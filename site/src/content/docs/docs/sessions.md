---
title: Sessions and persistence
description: Work with Lairs, Dojos, Splints, windows, detach, reopen, restore, and reset.
---

`splinterd` owns terminal processes and persistent topology. Graphical Splinterm processes can disconnect and be replaced without ending those sessions.

## New work and existing work

The normal desktop/XDG launch path always creates a fresh Lair with one Dojo and Splint. Reopening is intentionally separate:

```bash
splinterm sessions  # choose New Terminal or a recent running Dojo
splinterm reopen    # reopen the last remembered running Dojo
```

A graphical window may attach up to 32 distinct Dojos as local tabs, including Dojos from different Lairs. Opening a Dojo already present in that window activates its tab instead of duplicating it.

## Closing different things

These actions have deliberately different effects:

| Action | Result |
| --- | --- |
| Close a window | Detaches its local tabs and views |
| Close a tab | Detaches that Dojo from this window |
| Terminate a live Splint | Ends its process after explicit confirmation |
| Close an exited Splint | Removes the exited pane from topology |
| Close a Dojo | Removes it only when all affected Splints have exited |
| Restore | Starts a new process incarnation from saved metadata |

## Durable metadata

Metadata is stored under `$XDG_STATE_HOME/splinterm/`, falling back to `$HOME/.local/state/splinterm/`. Writes use an owner-only atomic primary and a previous-generation backup.

Splinterm persists identities, names, layout, focus hints, lifecycle state, and reviewed launch metadata. It does **not** persist terminal and scrollback bodies, clipboard data, PTY handles, grants, or controller tokens.

Startup quarantines malformed metadata and never executes a saved command automatically.

## Reset all sessions

Use the guarded command instead of moving state files manually:

```bash
splinterm reset       # interactive confirmation
splinterm reset --yes # explicit unattended confirmation
```

Reset moves the complete session database to a timestamped backup, restarts the canonical user service, waits for its configured socket, and leaves policy and configuration untouched.

:::caution
Reset affects the complete persistent topology. Inspect what is running before confirming it.
:::
