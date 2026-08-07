---
title: Installation
description: Install or update the current Splinterm private prerelease on Arch Linux and Omarchy.
---

The validated installation target is an **x86_64 Omarchy system based on Arch Linux**. Splinterm is not yet a supported public release.

## Install the current committed package

From a Splinterm repository checkout:

```bash
./install.sh
```

The installer obtains the newest successfully built package for committed `main`, verifies its manifest and checksums, preserves a rollback copy, warns before stopping a running daemon, installs through Pacman, and verifies the packaged client identity.

Private-repository collaborators must authenticate GitHub CLI once:

```bash
gh auth login
```

:::caution
The installer operates on a clean committed `HEAD`. It does not package uncommitted worktree changes. Review the current worktree before installing.
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

Continue to the [quickstart](/docs/quickstart/) after installation.
