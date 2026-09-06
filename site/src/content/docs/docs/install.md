---
title: Installation
description: Install the shipped Splinterm release on Arch Linux and Omarchy.
---

The validated installation target is an **x86_64 Omarchy system based on Arch Linux, using native Wayland under Hyprland**. See [Current status](/docs/status/) for the shipped version and support boundary.

Already installed? Read [Upgrade and rollback](/docs/packaging/) before replacing packages. Upgrades can end daemon-owned shells; stable 0.1 does not provide live daemon handoff.

## Install the prebuilt AUR package

Install the recommended [splinterm-bin AUR package](https://aur.archlinux.org/packages/splinterm-bin). It downloads verified prebuilt x86_64 binaries and resolves the `xdg-terminal-exec` AUR dependency without compiling Splinterm locally:

```bash
yay -S splinterm-bin
```

The optional policy-scoped MCP adapter is a separate exact-version prebuilt package:

```bash
yay -S splinterm-mcp-bin
```

Source-built `splinterm` and `splinterm-mcp` packages remain available. Migrating from them prompts once to approve replacement by the conflicting `-bin` packages. `paru` may be used instead of `yay`. AUR availability does not expand the supported target or create a long-term-support or 1.0 compatibility promise.

## Install the current versioned release directly

Run the installer from **another terminal, such as Foot**, not a shell inside Splinterm. It refuses a Splinterm-owned shell because stopping the daemon would terminate the installer itself.

For the newest published versioned package, clone the public repository and run:

```bash
git clone https://github.com/OldJobobo/splinterm.git
cd splinterm
./install.sh
```

The installer selects the newest published SemVer `v…` release, verifies the GitHub-recorded candidate-manifest digest and exact package checksums, preserves an emergency binary snapshot, warns before stopping a running daemon, installs through Pacman, and verifies the packaged client identity. The snapshot supports diagnosis and manual recovery; it is not a package-consistent rollback.

The repository and versioned release assets are public. GitHub CLI authentication is optional; the installer falls back to anonymous verified downloads.

:::caution
The default installer downloads only a published versioned release; it never selects historical `edge-*` releases or an arbitrary `main` commit. Source mode operates on a clean committed `HEAD` and does not package uncommitted worktree changes. Review the current worktree before using source mode.
:::

## Build from committed source

Source mode packages the clean committed checkout, which may differ from the shipped release. Select and review the intended commit first, then run from an external terminal:

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

To opt into the reversible Omarchy default-terminal, terminal-tag, and screensaver integration explicitly:

```bash
splinterm integration omarchy enable
```

Continue to the [quickstart](/docs/quickstart/) after installation. If an MCP host needs bounded access, follow the separate [MCP adapter setup](/docs/mcp/); installing the adapter alone grants no authority.
