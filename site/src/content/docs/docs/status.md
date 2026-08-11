---
title: Current status
description: What is implemented, validated, limited, planned, and unreleased in Splinterm.
---

Splinterm is a **public alpha**. Source, documentation, and verified commit-bound edge packages are public. Substantial core behavior is implemented and validated, while the supported target remains narrow and stable compatibility guarantees have not been released.

## What that means

- The product runs and has a normal graphical daily-use path.
- Core terminal, persistence, multiplexing, packaging, and automation milestones have recorded validation.
- The current target is narrow: x86_64 Omarchy/Arch Linux with native Wayland.
- Installation uses public alpha edge packages or a committed source checkout.
- AUR publication is being prepared; broader distribution and long-term compatibility promises have not been released.

## Capability summary

| Area | Current state |
| --- | --- |
| Native Wayland presentation | Keyboard, pointer, clipboard, IME, scaling, and damage-driven rendering validated on the documented Hyprland target |
| Persistent sessions and explicit restore | Implemented and validated |
| XDG command lifecycle | Commandless launches remain persistent; command-bearing launches use trusted client-bound transient Lairs |
| Pane layouts and multiple Dojos | Implemented and validated |
| Window-local Dojo tabs | Implemented and validated |
| Multi-client controller transfer | Implemented and validated |
| JSON/NDJSON automation | Implemented and validated |
| SSH stdio relay | Implemented and validated |
| [MCP adapter](/docs/mcp/) | Implemented and validated as an optional, separately policy-identified package |
| Sixel, practical Kitty static images, inline iTerm2 PNG | Documented supported subsets |
| Arch/Omarchy package | Public alpha edge package validated |
| Public source and edge builds | Available |
| AUR package | Preparing for alpha publication |
| Stable support and broader compatibility | Not released |
| Nix and broader distributions | Planned |

## Important boundaries

Splinterm is **security-conscious**, not absolutely secure. [Automation](/docs/automation/) is constrained by executable identity, explicit scopes, resource limits, controller ownership, revocation, and bounded audit metadata. Terminal output is always untrusted data and cannot grant authority.

Persistent topology is also separate from graphical presentation. Creating or mutating a Dojo does not map, focus, move, or resize a native Wayland window. Read [Why native Wayland?](/docs/wayland/) for the direct-compositor benefits, comparison model, and explicit non-claims.

## Before depending on it

Review the repository roadmap and the exact specialist documentation for the feature you intend to use. Public alpha availability is not a new compatibility guarantee; repository [`docs/status.md`](https://github.com/OldJobobo/splinterm/blob/main/docs/status.md) remains authoritative.
