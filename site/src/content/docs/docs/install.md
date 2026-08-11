---
title: Installation
description: Install or update the current Splinterm public alpha on Arch Linux and Omarchy.
---

The validated installation target is an **x86_64 Omarchy system based on Arch Linux**. Splinterm is a public alpha, not a supported stable release; upgrades may change interfaces and end daemon-owned shells.

## Install the versioned AUR package

Install the main package from the [splinterm AUR package page](https://aur.archlinux.org/packages/splinterm) with an AUR helper, which also resolves its `xdg-terminal-exec` AUR dependency:

```bash
yay -S splinterm
```

The optional policy-scoped MCP adapter is a separate exact-version split package:

```bash
yay -S splinterm-mcp
```

`paru` may be used instead of `yay`. AUR availability does not expand the supported target or create stable compatibility and support-duration guarantees.

## Install the current edge package

For the newest successfully validated commit-bound package, clone the public repository and run the edge installer:

```bash
git clone https://github.com/OldJobobo/splinterm.git
cd splinterm
./install.sh
```

The installer obtains the newest successfully built package for committed `main`, verifies its manifest and checksums, preserves an emergency binary snapshot, warns before stopping a running daemon, installs through Pacman, and verifies the packaged client identity. The snapshot supports diagnosis and manual recovery; it is not a package-consistent rollback.

The repository, channel manifest, and edge release assets are public. GitHub CLI authentication is optional; the installer falls back to anonymous verified downloads.

:::caution
The default installer downloads the newest successfully validated public `main` edge package. Source mode operates on a clean committed `HEAD` and does not package uncommitted worktree changes. Review the current worktree before using source mode.
:::

## Build from committed source

To compile and package the current committed checkout locally:

```bash
./install.sh --source
```

Include the complete package test suite with:

```bash
./install.sh --source --check
```

## What installation does not change

The installer does not:

- make Splinterm your default terminal;
- edit Omarchy or Hyprland configuration;
- enable persistent systemd user lingering; or
- opt a fresh installation into the optional MCP package.

Continue to the [quickstart](/docs/quickstart/) after installation. If an MCP host needs bounded access, follow the separate [MCP adapter setup](/docs/mcp/); installing the adapter alone grants no authority.
