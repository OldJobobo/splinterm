---
title: Upgrade and rollback
description: Upgrade Splinterm packages without stranding the installer, and understand recovery limits.
---

Use [Installation](/docs/install/) for a fresh install and [Current status](/docs/status/) for the shipped release. This guide applies to the documented x86_64 Omarchy/Arch target.

:::caution[Save work before replacing packages]
**0.1 has no live daemon-upgrade handoff.** Stopping or restarting `splinterd` ends its shells and commands. Save files, stop important jobs deliberately, and perform package operations from another terminal, such as Foot—not from a Splint.

Persistent layout and launch metadata are not running processes, terminal history, or application checkpoints. Restore is explicit and starts new processes; it does not recover unsaved application state.
:::

## Before upgrading

1. Review the target release notes and any migration instructions.
2. Inspect `splinterm list --all` and save work in every affected Lair.
3. Retain the currently installed package archives and note their versions. If the optional MCP package is installed, retain its matching version too.
4. Close existing Splinterm Windows after saving work. Closing a persistent Window alone does not stop its jobs.
5. Open an external terminal for the package operation and recovery checks.

Read-only package checks:

```bash
pacman -Q splinterm-bin splinterm-mcp-bin
command -v splinterm
/usr/bin/splinterm list
```

For source-built packages, query `splinterm` and `splinterm-mcp` instead. An absent optional MCP package is normal.

## Upgrade through the AUR

After saving all work, explicitly stop `splinterd.service` from the external terminal. Then use your normal full-system Arch upgrade workflow:

```bash
systemctl --user stop splinterd.service && yay -Syu
```

Update the installed main and optional MCP packages together; the adapter has an exact-version dependency. Do not mix source-built and prebuilt variants or incompatible versions. After successful installation, start the service:

```bash
systemctl --user start splinterd.service
```

## Upgrade with the release installer

From the external terminal and a reviewed repository checkout:

```bash
./install.sh
```

The installer selects a published versioned release, verifies manifest and package checksums, warns before stopping the daemon, installs through Pacman, and checks package/client identity. It does not select historical `edge-*` releases. An existing optional MCP installation is upgraded with its matching main package; a fresh installation does not opt into MCP.

The installer refuses a Splinterm-owned shell. Do not bypass that refusal or use `--yes` to suppress a warning you have not reviewed.

## Verify before returning to work

```bash
command -v splinterm
systemctl --user status splinterd.service
/usr/bin/splinterm list
/usr/bin/splinterm list --all
pacman -Qkk splinterm-bin
desktop-file-validate /usr/share/applications/com.oldjobobo.splinterm.desktop
```

Use the owning source-built package name for `pacman -Qkk` if applicable. The packaged client should resolve to `/usr/bin/splinterm`, adjacent to `/usr/bin/splinterd`; a user-local shadowing client does not acquire trusted graphical identity. Use normal human-mode `list` for this check, not `--output json`, which intentionally has separate automation authority.

**Reopen every existing client after replacing the binaries.** An old running client retains its old executable inode and no longer matches a newly installed daemon's trusted-UI sibling identity. See [Troubleshooting](/docs/troubleshooting/) if authorization fails.

Inspect exited topology before deciding what to restore. `reopen` returns to still-running Dojos; it never silently reruns saved commands.

## Roll back deliberately

The installer's emergency binary snapshot is for diagnosis and manual recovery. It is **not a package-consistent rollback**.

For a package-consistent downgrade:

1. Read the target release's migration guidance. Older binaries are not guaranteed to understand newer state or configuration.
2. Save current work and stop the daemon from an external terminal.
3. Preserve your state and configuration before changing versions; do not reset or delete them as a generic repair step.
4. Reinstall the retained, verified package archives using `sudo pacman -U`, supplying the matching main and optional MCP archives in the same transaction.
5. Restart the service, repeat the checks above, and reopen clients.

Do not copy only one old binary over a Pacman-owned installation or claim that a downgrade restores killed processes. If compatible archives or migration instructions are unavailable, stop and investigate rather than improvising a partial rollback.
