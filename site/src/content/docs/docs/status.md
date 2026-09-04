---
title: Current status
description: What is implemented, validated, limited, planned, and unreleased in Splinterm.
---

Splinterm is a **public beta**. Source, documentation, and immutable versioned GitHub and AUR packages are public. Substantial core behavior is implemented and validated, while the validated target remains narrow and stable compatibility guarantees have not been released.

[`v0.1.0-rc.2`](https://github.com/OldJobobo/splinterm/releases/tag/v0.1.0-rc.2) is the current public prerelease and is distributed through both AUR package bases as `0.1.0rc.2-1`. It retains RC1 while closing two live-font review findings: accepted staging remains synchronized with watcher probes, and cached FreeType faces share immutable staged mappings rather than copying complete font files.

RC2 passed complete non-graphical validation, independent source review, guarded packaged live-font replacement and invalid-candidate rollback acceptance, candidate construction, protected promotion, and exact-asset publication verification. Ongoing daily-use soak remains focused on repeated font and scale changes, theme refresh for new Splints, package upgrade and rollback, and long-running terminal workloads.

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
| Graphical terminal lifecycle | Ordinary unnamed commandless launches are configurable persistent or Window-owned; command-bearing XDG launches remain client-bound |
| Pane layouts and multiple Dojos | Implemented and validated |
| Window-local Dojo tabs, tab strip, and context menus | Implemented and validated |
| Configurable keymaps and Omarchy controls | Implemented and validated |
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
| [AUR packages](https://aur.archlinux.org/packages/splinterm-bin) | Recommended prebuilt `splinterm-bin`; source-built `splinterm` is also available |
| Stable support and broader compatibility | Not released |
| Nix and broader distributions | Planned |

## Important boundaries

Splinterm is **security-conscious**, not absolutely secure. [Automation](/docs/automation/) is constrained by executable identity, explicit scopes, resource limits, controller ownership, revocation, and bounded audit metadata. Terminal output is always untrusted data and cannot grant authority.

Persistent topology is also separate from graphical presentation. Creating or mutating a Dojo does not map, focus, move, or resize a native Wayland window. Read [Why native Wayland?](/docs/wayland/) for the direct-compositor benefits, comparison model, and explicit non-claims.

## Before depending on it

Review the [public roadmap](/docs/roadmap/) and the exact specialist documentation for the feature you intend to use. Public beta availability is not a new compatibility guarantee; repository [`docs/status.md`](https://github.com/OldJobobo/splinterm/blob/main/docs/status.md) remains authoritative.
