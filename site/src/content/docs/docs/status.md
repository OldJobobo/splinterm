---
title: Current status
description: The shipped Splinterm release, validated target, capabilities, and limits.
---

**Splinterm 0.1.0 is the first stable release for the documented target.**

[`v0.1.0`](https://github.com/OldJobobo/splinterm/releases/tag/v0.1.0) was published on September 5, 2026. The prebuilt `splinterm-bin` and optional `splinterm-mcp-bin` AUR packages, and their source-built alternatives, are **`0.1.0-1`**.

These guides describe that shipped release. Development on `main` may contain features that are not yet available in packages.

## Supported target, not a universal promise

- x86_64 Omarchy/Arch Linux, running native Wayland under the documented Hyprland environment.
- Headless `splinterd` does not require a graphical environment; its packaged and remote workflows retain the documented 0.1 limitations.
- Stable 0.1.0 is not an LTS promise or a 1.0 compatibility contract. Future 0.x releases may change interfaces with documented migration.
- Other distributions, compositors, architectures, and package formats are not current compatibility promises.

## What you can use today

| Area | Shipped behavior |
| --- | --- |
| Native Wayland terminal | Keyboard, pointer, clipboard, IME, scaling, and damage-driven rendering on the documented target |
| Lairs, Dojos, and Splints | Persistent workspaces, pane layouts, and Window-local Dojo tabs |
| Terminal lifetime | Persistent by default; optional Window-owned Lairs; command-bearing XDG launches start client-bound, with owning-client tab-organization promotion when enabled |
| Returning to work | Recent Dojos and reopen for running work; explicit restore for exited processes |
| Keymaps and local controls | Built-in `splinterm` and `omarchy-tmux` profiles, binding help, command palette, and vi copy mode |
| Dojo presets | Atomic layouts and optional collision-safe Bash helpers |
| Omarchy integration | Native theme following, live system-monospace family following when `main.font` is unset, and opt-in desktop integration |
| Remote access | Native graphical clients over profile-bound SSH; remote image transfer excluded |
| Automation | Policy-scoped JSON/NDJSON, SSH stdio relay, and an optional separately identified MCP adapter |
| Images | Documented Sixel, practical Kitty static-image, and inline iTerm2 PNG subsets |
| Installation | Versioned GitHub releases and AUR packages |

## Important boundaries

Closing a Window leaves **persistent** work running while `splinterd` and its processes remain alive. A Window-owned, unpromoted Lair ends with its owning Window. A daemon restart or reboot does not preserve running processes or terminal history. See [Sessions and persistence](/docs/sessions/).

**0.1 has no live daemon-upgrade handoff.** Save application work before upgrading and follow [Upgrade and rollback](/docs/packaging/).

Splinterm is **security-conscious**, not absolutely secure. [Automation](/docs/automation/) is constrained by executable identity, explicit scopes, resource limits, controller ownership, revocation, and bounded audit metadata. Terminal output is always untrusted data and cannot grant authority.

Creating or mutating a Dojo does not map, focus, move, or resize a native Wayland Window. Read [Why native Wayland?](/docs/wayland/) for the direct-compositor benefits and explicit non-claims.

## Release evidence and direction

The [published release](https://github.com/OldJobobo/splinterm/releases/tag/v0.1.0) and [publication record](https://github.com/OldJobobo/splinterm/blob/258b05180dffadd01def52f0ef18f8c2ee220b8b/docs/status.md) record the stable release boundary. [Source at the release tag](https://github.com/OldJobobo/splinterm/tree/v0.1.0) identifies shipped behavior; older maturity text in individual tagged documents is historical, not a newer release claim.

The [public roadmap](/docs/roadmap/) describes intended improvements, not additional shipped capabilities.
