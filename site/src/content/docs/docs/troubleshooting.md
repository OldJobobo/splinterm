---
title: Troubleshooting
description: Diagnose common local Splinterm installation, daemon, session, and configuration problems.
---

This page covers the first local checks. Splinterm is a public beta with a narrow validated Omarchy/Arch environment, so failures outside that target may not have a supported resolution.

## Check the installed command

```bash
command -v splinterm
command -v splinterd
```

On the packaged system, the trusted graphical client and daemon are adjacent under `/usr/bin`. A user-local `splinterm` earlier in `PATH` does not match the running system daemon's trusted-UI identity.

## Check the daemon

```bash
systemctl --user status splinterd.service
splinterm ping
```

Start the user service on demand if needed:

```bash
systemctl --user start splinterd.service
```

The default socket is `$XDG_RUNTIME_DIR/splinterm/splinterd.sock`. A development instance may use another path through `SPLINTERM_SOCKET`.

## A window exits as unauthorized

Check all of the following:

1. `command -v splinterm` resolves to the packaged client.
2. The running daemon executable is the packaged adjacent `splinterd`.
3. The client and daemon come from the same package build.
4. Old windows were reopened after replacing either executable.

Do not install a development client to an earlier user-local `PATH` entry and treat it as the packaged desktop client.

## Configuration fails at startup

Run with the intended file and read the line-numbered diagnostic:

```bash
SPLINTERM_CONFIG=/path/to/config.ini splinterm launch
```

Unknown keys and malformed values fail rather than being guessed. Compare the file with the [supported configuration](/docs/configure/configuration/).

## A Dojo is missing from Recent Dojos

The native picker opens only Dojos whose complete pane layout is still running. Exited Splints remain in persistent metadata, but starting them again requires explicit restore.

Inspect active and exited topology through the human CLI:

```bash
splinterm list
splinterm list --all
```

## Automation is denied

Socket access, the same Unix account, SSH login, or running inside a Splint does not grant automation authority. Persistent automation requires a valid owner-controlled policy for the exact executable identity, operation scopes, resources, and limits.

A controller denial can also be normal: one client may hold a Splint's exclusive input/resize lease while others continue observing.
