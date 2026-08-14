<div align="center">
  <img src="assets/icons/splinterm-icon-glyph.svg" width="112" height="112" alt="Splinterm glyph">

# Splinterm

**A persistent, security-conscious terminal substrate for humans and bounded automation.**

[Website](https://splinterm.com/) · [Documentation](https://splinterm.com/docs/) · [Quickstart](https://splinterm.com/docs/quickstart/) · [Product roadmap](docs/product-roadmap.md) · [Current status](docs/status.md)

</div>

Splinterm combines a native Wayland terminal with a headless daemon that keeps shells, layouts, and terminal state alive when graphical clients disconnect. Close a window, come back later, and the work is still running.

Humans use that persistent topology through native windows, tabs, and panes. Authorized tools can reach the same sessions through bounded JSON/NDJSON, SSH relay, and MCP interfaces. Splinterm is built in Rust from [Foot](https://codeberg.org/dnkl/foot)'s terminal behavior and designed first for Omarchy and Arch Linux.

> [!IMPORTANT]
> **Status: public alpha.** Source, immutable versioned GitHub and AUR packages, and documentation are public. Core terminal emulation, persistent sessions, multiplexing, native Wayland presentation, Arch packaging, and bounded automation workflows are implemented and validated for the current x86_64 Omarchy/Arch Linux target. The alpha may make breaking changes; broader compatibility guarantees and stable support have not been released.
>
> See the repository-authoritative [current status](docs/status.md) for the exact capability and availability boundaries.

## Why Splinterm

### Your shell outlives its window

`splinterd` owns the terminal processes, layouts, and session metadata. A Wayland window is a disposable view into that state—not the owner of it. Detaching a client does not end the work beneath it.

### One topology for people and tools

Native windows, the human CLI, structured clients, the SSH relay, and the MCP adapter all operate on the same persistent sessions. Automation does not live in a separate, less capable terminal world.

### Authority is explicit

Automation is constrained by exact executable identity, explicit scopes, bounded resources and messages, controller ownership, consent, revocation, and body-free audit metadata. Terminal output remains untrusted data; it cannot grant authority or become an automatic instruction.

### Terminal behavior has an oracle

Foot is Splinterm's behavioral foundation, not just visual inspiration. The terminal kernel is a Rust translation grounded in a pinned Foot implementation, with provenance retained in [`THIRD_PARTY.md`](THIRD_PARTY.md).

## What works today

| Area | Current state |
| --- | --- |
| Native Wayland terminal | Implemented and validated |
| Persistent sessions and explicit restore | Implemented and validated |
| Pane layouts and multiple Dojos | Implemented and validated |
| Omarchy keymap, presets, and optional Bash helpers | Implemented and validated |
| Vi copy mode and context-local desktop editing | Implemented and validated |
| Window-local Dojo tabs | Implemented and validated |
| Multi-client controller transfer | Implemented and validated |
| JSON/NDJSON automation | Implemented and validated |
| SSH stdio relay | Implemented and validated |
| MCP adapter | Implemented and validated |
| Sixel, practical Kitty static images, and inline iTerm2 PNG | Documented supported subsets |
| Arch/Omarchy package | Versioned GitHub release and AUR packages validated |
| Public source and versioned builds | Available |
| AUR packages | Prebuilt `splinterm-bin` and source-built `splinterm`, both `0.1.0alpha3.1-1` |
| Stable support and broader compatibility | Not released |
| Nix and broader distributions | Planned |

For limitations and release gates, read [Current status](docs/status.md). Exact image support is documented in [`docs/images.md`](docs/images.md).

## Install

The validated installation target is **x86_64 Omarchy/Arch Linux with native Wayland**. The recommended AUR packages download verified prebuilt binaries and do not compile locally:

```bash
yay -S splinterm-bin
# Optional policy-scoped MCP adapter:
yay -S splinterm-mcp-bin
```

The source-built alternatives are `splinterm` and `splinterm-mcp`. `paru` may be used instead of `yay`. All packages remain alpha software with no stable compatibility or support-duration guarantee.

For the newest published versioned release package, clone the public repository and run:

```bash
git clone https://github.com/OldJobobo/splinterm.git
cd splinterm
./install.sh
```

The release installer selects the newest published SemVer `v…` release, verifies its GitHub-recorded manifest digest and package checksums, preserves an emergency binary snapshot, installs through Pacman, and verifies the packaged client identity. The snapshot supports diagnosis and manual recovery; it is not a package-consistent rollback. GitHub CLI authentication is optional, and anonymous public downloads are supported.

To build and package the current committed checkout locally, run the installer
from Foot or another terminal not owned by `splinterd`:

```bash
./install.sh --source
```

Add `--check` to run the complete package test suite. The installer deliberately packages a clean committed `HEAD`; it does not include uncommitted worktree changes.

Installation does **not** change your default terminal, edit Omarchy or Hyprland configuration, enable systemd user lingering, or install the optional MCP package on a fresh system. After installation, `splinterm integration omarchy enable` explicitly configures the complete reversible user-level default-terminal, terminal-tag, and screensaver integration. Trusted graphical authority requires the client to be the exact device/inode sibling adjacent to the running `/usr/bin/splinterd`. After an upgrade replaces `/usr/bin/splinterm`, close and reopen every existing Splinterm window: an already-running client retains the old inode and is no longer the trusted sibling.

Read the complete [installation guide](https://splinterm.com/docs/install/).

## Start using it

Open a fresh terminal from the installed desktop entry or the XDG terminal launcher:

```bash
splinterm-xdg-terminal-exec
```

A commandless desktop/XDG launch creates a persistent Lair with one Dojo and one Splint. Closing its window detaches the graphical client while `splinterd` keeps the session running. When another application asks the XDG terminal to host a command, Splinterm instead creates a transient client-bound Lair: command exit or owner-window disconnect terminates its processes and removes it from topology. Native `splinterm launch -- COMMAND...` remains persistent.

Return through the native Dojo picker:

```bash
splinterm dojos   # choose a running Dojo or start a new terminal
splinterm reopen  # reopen the last locally remembered running Dojo
```

`splinterm sessions` remains a compatibility alias for `splinterm dojos`.

Inside a managed Splinterm window, these controls cover the essential workflow:

| Action | Control |
| --- | --- |
| Command palette | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd> |
| Recent Dojos | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>S</kbd> |
| Split horizontally | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Enter</kbd> |
| Split vertically | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>\</kbd> |
| Move between panes | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Arrow</kbd> |
| Cycle Dojo tabs | <kbd>Ctrl</kbd>+<kbd>Tab</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Tab</kbd> |
| New Dojo tab | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd> |
| Detach active tab | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Q</kbd> |
| Search scrollback | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>F</kbd> |
| Copy / paste | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>C</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>V</kbd> |

The optional `omarchy-tmux` profile adds familiar `Ctrl+Space` / `Ctrl+B`
prefixes, `Prefix+?` resolved-key help, and `Prefix+[` vi copy mode. In copy
mode, navigate with vi keys or arrows, press `v` to select, `y` to publish to the
Wayland clipboard, or Escape to cancel. Outside copy mode, `Super+C/V` provide
terminal copy/paste; Splinterm-owned fields additionally provide bounded local
selection, cut, and undo without claiming universal terminal `Super+X/Z`.
Packaged atomic Dojo presets and optional collision-safe Bash helpers cover the
standard Omarchy `t`, `tdl`, `tds`, `tdlm`, and `tsl` workflows under
Splinterm's separate `s*` shell namespace.

Reopening attaches to processes that are still running. Starting an exited process again from saved launch metadata is an explicit **restore** operation.

Continue with the [quickstart](https://splinterm.com/docs/quickstart/), repository [human usage guide](docs/usage.md), or website [Dojos and persistence](https://splinterm.com/docs/sessions/).

## How it works

```text
                         native windows
                    ┌──────────┴──────────┐
                    │ disposable views    │
                    └──────────┬──────────┘
                               │ attach / detach
                         ┌─────▼─────┐
                         │ splinterd │
                         └─────┬─────┘
                               │ owns
Topology
└── Lair: project or persistent session
    ├── Dojo: terminal layout
    │   ├── Splint: shell or process
    │   └── Splint: shell or process
    └── Dojo: another layout
        └── Splint: shell or process
```

- **Topology** — the daemon's complete persistent session catalog.
- **Lair** — a named project or persistent session.
- **Dojo** — one persistent terminal layout inside a Lair.
- **Splint** — an individual terminal pane and process lifecycle.
- **Window** — a native Wayland view that may display multiple Dojos as local tabs.

Window and tab lifetimes are separate from Dojo and Splint lifetimes. Closing a view detaches it; terminating a process is an explicit, guarded action.

Read [Core concepts](https://splinterm.com/docs/concepts/) for the user model or [`docs/architecture.md`](docs/architecture.md) for system ownership and boundaries.

## Automation and remote access

Splinterm exposes a deliberately bounded automation surface rather than making its private daemon protocol public:

- **JSON/NDJSON CLI** for versioned one-shot operations and subscriptions.
- **SSH stdio relay** for remote automation without a network listener in `splinterd`.
- **Native remote client** for a profile-bound graphical workflow over an authenticated relay.
- **MCP adapter** as an optional, separately packaged and policy-identified integration.

Machine access does not inherit human graphical authority and remains governed by automation policy. Native remote Windows are different: OpenSSH authenticates the human account, and the installed graphical relay receives normal terminal-multiplexer authority without automation policy.

Authoritative references:

- [`docs/automation.md`](docs/automation.md) — public JSON/NDJSON contracts, policy, and exit behavior
- [`docs/remote.md`](docs/remote.md) — SSH relay, remote profiles, and authority boundaries
- [`docs/mcp.md`](docs/mcp.md) — MCP installation, policy, and host setup
- [`docs/integrations.md`](docs/integrations.md) — integration-author checklist and safe client workflows
- [`dist/schemas/v2/`](dist/schemas/v2/) — checked-in public machine schemas

## Configuration

Splinterm uses `${XDG_CONFIG_HOME:-~/.config}/splinterm/config.ini` for its focused configuration surface. It supports fonts and sizing, shell behavior, scrollback, cursor settings, pane chrome, keymap overlays, and explicit theme overrides.

On Omarchy, Splinterm can read the active theme's effective `foot.ini` and `colors.toml` directly and safely reload valid palette changes without restarting the daemon or shell.

See the [configuration guide](https://splinterm.com/docs/configure/configuration/) for supported keys, keymap inspection, Omarchy integration, and Foot migration.

## Documentation

| If you want to… | Start here |
| --- | --- |
| Install and evaluate Splinterm | [Installation](https://splinterm.com/docs/install/) |
| Open, detach, and return to work | [Quickstart](https://splinterm.com/docs/quickstart/) |
| Understand Lairs, Dojos, Splints, and windows | [Core concepts](https://splinterm.com/docs/concepts/) |
| Manage persistence, restore, and reset | [Dojos and persistence](https://splinterm.com/docs/sessions/) |
| Configure the terminal | [Configuration](https://splinterm.com/docs/configure/configuration/) |
| Check maturity and availability | [Current status](docs/status.md) |
| Use windows, tabs, panes, and restore safely | [Human usage](docs/usage.md) |
| Find CLI commands and machine-output boundaries | [CLI reference](docs/cli.md) |
| Troubleshoot an installation | [Troubleshooting](https://splinterm.com/docs/troubleshooting/) |
| Contribute to the project | [Contributing](CONTRIBUTING.md) |

Specialist contracts and design records remain in [`docs/`](docs/). Plans, spikes, benchmarks, and retained artifacts are development history and evidence—not the primary user guide.

## Development

Splinterm is a Rust workspace. The normal non-graphical validation is:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

For an isolated development daemon and client:

```bash
./splinterm-test          # build, start or reuse the test daemon, and open Splinterm
./splinterm-test restart  # rebuild and restart after daemon or protocol changes
./splinterm-test ping     # build and verify the isolated daemon
./splinterm-test stop     # stop the isolated daemon
```

Read [`CONTRIBUTING.md`](CONTRIBUTING.md) and the [development guide](https://splinterm.com/docs/development/) before changing domain, protocol, renderer, or Foot-derived behavior.

## Design authority and lineage

Splinterm's emulator half is derived from Foot's architecture and behavior. Translated or adapted code records its source provenance and retains the relevant MIT attribution. See [`docs/adr/0001-foot-rust-port.md`](docs/adr/0001-foot-rust-port.md), [`THIRD_PARTY.md`](THIRD_PARTY.md), and [`docs/pre-planning-research.md`](docs/pre-planning-research.md).

## Support Splinterm

If Splinterm is useful to you, you can [support its continued development on Ko-fi](https://ko-fi.com/oldjobobo).

## License

Splinterm is available under the [MIT License](LICENSE).
