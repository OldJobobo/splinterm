---
title: Current status
description: What is implemented, validated, limited, planned, and unreleased in Splinterm.
---

Splinterm **0.1.0 is the first stable release**, published on 2026-09-05 for x86_64 Omarchy/Arch Linux with native Wayland under Hyprland. Source, documentation, GitHub releases, and AUR packages are public. Broader platform compatibility and a support lifetime are not promised; future 0.x releases may change interfaces with documented migration.

Stable release notes and assets:

https://github.com/OldJobobo/splinterm/releases/tag/v0.1.0

The newer **0.1.1-rc.2**, published on 2026-09-15, is a prerelease, not a new stable release. Check the release listing for subsequent publications:

https://github.com/OldJobobo/splinterm/releases

## What that means

- The product runs and has a normal graphical daily-use path.
- Core terminal, persistence, multiplexing, packaging, and automation milestones have recorded validation.
- The current target is narrow: x86_64 Omarchy/Arch Linux with native Wayland.
- Installation uses the versioned AUR package, the matching GitHub release, or a committed source checkout.
- Broader distribution and long-term compatibility promises have not been released.

## Capability summary

| Area | Current state |
| --- | --- |
| Native Wayland presentation | Keyboard, pointer, clipboard, IME, scaling, and damage-driven rendering validated on the documented Hyprland target |
| Persistent sessions and explicit restore | Implemented and validated |
| XDG command lifecycle | Commandless launches remain persistent; command-bearing launches use trusted client-bound transient Lairs |
| Pane layouts and multiple Dojos | Implemented and validated |
| Window-local Dojo tabs, tab strip, and context menus | Implemented and validated |
| Configurable keymaps, searchable help, and current-Lair controls | Implemented and non-graphically validated; packaged graphical acceptance pending |
| Atomic Dojo presets and optional Bash helpers | Implemented and validated |
| Vi copy mode and trusted local field editing | Implemented and validated |
| Multi-client controller transfer | Implemented and validated |
| JSON/NDJSON automation | Implemented and validated |
| SSH stdio relay | Implemented and validated |
| [Native remote graphical client](/docs/remote/) | Implemented and validated with profile-bound OpenSSH transport; remote image transfer is excluded |
| [MCP adapter](/docs/mcp/) | Implemented and validated as an optional, separately policy-identified package |
| Sixel, practical Kitty static images, inline iTerm2 PNG | Documented supported subsets |
| Arch/Omarchy package | Versioned GitHub release and AUR packages validated |
| Public source and versioned builds | Available |
| [AUR packages](https://aur.archlinux.org/packages/splinterm-bin) | Prebuilt `splinterm-bin`, source-built `splinterm`, and optional MCP split packages; this channel can include release candidates, not only stable releases |
| Stable release | 0.1.0 for the documented platform |
| Broader compatibility and support lifetime | Not promised |
| Nix and broader distributions | Planned |

## Important boundaries

Splinterm is **security-conscious**, not absolutely secure. [Automation](/docs/automation/) is constrained by executable identity, explicit scopes, resource limits, controller ownership, revocation, and bounded audit metadata. Terminal output is always untrusted data and cannot grant authority.

**Splinterm 0.1 does not support live daemon upgrade handoff.** Stopping or replacing the daemon ends its child processes; saved topology does not checkpoint applications. Save your work and upgrade from another terminal, then reopen Splinterm Windows. See [Installation](/docs/install/).

Persistent topology is also separate from graphical presentation. Creating or mutating a Dojo does not map, focus, move, or resize a native Wayland window. Read [Why native Wayland?](/docs/wayland/) for the direct-compositor benefits, comparison model, and explicit non-claims.

## Before depending on it

Review the [public roadmap](/docs/roadmap/) and the exact specialist documentation for the feature you intend to use. Stable 0.1.0 does not expand the documented compatibility scope; repository [`docs/status.md`](https://github.com/OldJobobo/splinterm/blob/main/docs/status.md) remains authoritative.
