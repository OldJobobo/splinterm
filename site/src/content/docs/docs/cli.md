---
title: CLI reference
description: Find the human commands for opening, inspecting, arranging, restoring, and remotely accessing Splinterm.
---

The `splinterm` CLI serves both people and structured clients. Human output is the default. JSON and NDJSON are versioned machine contracts with separate authority rules; see [Bounded automation](/docs/automation/) before scripting them.

## Everyday graphical workflows

```bash
splinterm dojos                    # open Recent Dojos
splinterm reopen                   # reopen the last still-running Dojo
splinterm launch                   # create a fresh persistent graphical Lair
splinterm window --lair-id L --dojo-id D
```

`splinterm sessions` remains a compatibility alias for `dojos`. A Window is a disposable view and can hold several Dojos as local tabs.

## Inspect topology

```bash
splinterm ping
splinterm list
splinterm list --all
splinterm topology
splinterm inspect SPLINT_ID
```

`list` emphasizes active Lairs. `--all` includes exited-only history. IDs returned by these commands are stable topology identities, not window handles.

## Create and arrange work

```bash
splinterm new project --cwd "$HOME/src/project"
splinterm new-dojo LAIR_ID --name logs -- /usr/bin/journalctl -f
splinterm split SPLINT_ID --axis vertical --side second --ratio 600
splinterm ratio SPLINT_ID 500
splinterm rename-lair LAIR_ID NAME
splinterm rename-dojo DOJO_ID NAME
splinterm rename-splint SPLINT_ID TITLE
```

Arguments after `--` are executed directly as an argument vector. Splinterm does not rebuild them into shell text.

A new Lair starts with `Dojo 1`. If `new-dojo` omits `--name`, Splinterm chooses one greater than that Lair's highest exact `Dojo N` name without reusing gaps. Explicit names are preserved.

## Lifecycle

| Command | Effect |
| --- | --- |
| `kill SPLINT_ID` | End a live process but retain its Splint. |
| `restore SPLINT_ID` | Start one exited Splint from saved metadata. |
| `restore-dojo DOJO_ID` | Restore every exited Splint in a Dojo. |
| `restore-lair LAIR_ID` | Restore every exited Splint in a Lair. |
| `relaunch SPLINT_ID [-- ARGV...]` | Start a new incarnation, optionally with new argv. |
| `close SPLINT_ID` | Remove an exited Splint and collapse its layout branch. |
| `close-dojo DOJO_ID` | Remove a Dojo after every Splint has exited. |
| `reset` | Back up and clear all persistent topology, then restart cleanly. |

Restore is always explicit. Saved launch metadata is never executed automatically.

## Local configuration and presets

```bash
splinterm config check
splinterm keymap list
splinterm keymap show
splinterm keymap conflicts
splinterm preset list
splinterm preset show NAME
splinterm preset check
splinterm preset run NAME --cwd "$PWD" --dry-run
```

Read [Configuration and keymaps](/docs/configure/configuration/) and [Dojo presets](/docs/presets/) for the strict schemas and bundled Omarchy workflows.

## Remote profiles

```bash
splinterm remote list
splinterm remote inspect PROFILE
splinterm remote check PROFILE
splinterm --remote PROFILE
splinterm --remote PROFILE reopen
```

The first two commands are local-only. `remote check` performs bounded, read-only SSH, relay, and daemon probes without mapping a Window. See [Remote access](/docs/remote/).

## Machine output

```bash
splinterm --output json --schema-major 2 --timeout-ms 5000 topology
splinterm --output ndjson --schema-major 2 --timeout-ms 300000 \
  subscribe terminal SPLINT_ID
```

Machine mode never prompts. It uses stable schema-major-2 envelopes, explicit timeouts, structured exit categories, resource policy, incarnation checks, and resynchronization rules. Human-readable output and private daemon frames are not compatibility contracts.

For every flag and operation, the current executable's `--help` and repository [`docs/cli.md`](https://github.com/OldJobobo/splinterm/blob/main/docs/cli.md) remain authoritative.
