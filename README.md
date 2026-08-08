<div align="center">
  <img src="assets/icons/splinterm-icon-glyph.svg" width="112" height="112" alt="Splinterm glyph">

# Splinterm

**A persistent, security-conscious terminal substrate for humans and bounded automation.**

[Website](https://splinterm.com/) · [Documentation](https://splinterm.com/docs/) · [Quickstart](https://splinterm.com/docs/quickstart/) · [Current status](https://splinterm.com/docs/status/)

</div>

Splinterm combines a native Wayland terminal with a headless daemon that keeps shells, layouts, and terminal state alive when graphical clients disconnect. Close a window, come back later, and the work is still running.

Humans use that persistent topology through native windows, tabs, and panes. Authorized tools can reach the same sessions through bounded JSON/NDJSON, SSH relay, and MCP interfaces. Splinterm is built in Rust from [Foot](https://codeberg.org/dnkl/foot)'s terminal behavior and designed first for Omarchy and Arch Linux.

> [!IMPORTANT]
> **Status: advanced private prerelease.** Core terminal emulation, persistent sessions, multiplexing, native Wayland presentation, Arch packaging, and bounded automation workflows are implemented and validated. The currently validated target is x86_64 Omarchy/Arch Linux. Public distribution, compatibility guarantees, and a support policy have not yet been released.
>
> See the [current status](https://splinterm.com/docs/status/) for the exact capability and availability boundaries.

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
| Window-local Dojo tabs | Implemented and validated |
| Multi-client controller transfer | Implemented and validated |
| JSON/NDJSON automation | Implemented and validated |
| SSH stdio relay | Implemented and validated |
| MCP adapter | Implemented and validated |
| Sixel, practical Kitty static images, and inline iTerm2 PNG | Documented supported subsets |
| Arch/Omarchy package | Private prerelease package validated |
| Public distribution | Not released |
| Nix and broader distributions | Planned |

For limitations and release gates, read [Current status](https://splinterm.com/docs/status/). Exact image support is documented in [`docs/images.md`](docs/images.md).

## Install

The validated installation target is **x86_64 Omarchy/Arch Linux with native Wayland**. From a Splinterm repository checkout:

```bash
./install.sh
```

The installer downloads the newest successful `main` package, verifies its manifest and checksums, preserves a rollback copy, installs through Pacman, and verifies the packaged client identity. Private-repository collaborators must authenticate GitHub CLI once with `gh auth login`.

To build and package the current committed checkout locally:

```bash
./install.sh --source
```

Add `--check` to run the complete package test suite. The installer deliberately packages a clean committed `HEAD`; it does not include uncommitted worktree changes.

Installation does **not** change your default terminal, edit Omarchy or Hyprland configuration, enable systemd user lingering, or install the optional MCP package on a fresh system.

Read the complete [installation guide](https://splinterm.com/docs/install/).

## Start using it

Open a fresh terminal from the installed desktop entry or the XDG terminal launcher:

```bash
splinterm-xdg-terminal-exec
```

The normal launch creates a new Lair with one Dojo and one Splint. Closing the window detaches the graphical client while `splinterd` keeps the session running.

Return through the native session picker:

```bash
splinterm sessions  # choose a running Dojo or start a new terminal
splinterm reopen    # reopen the last locally remembered running Dojo
```

Inside a managed Splinterm window, these controls cover the essential workflow:

| Action | Control |
| --- | --- |
| Command palette | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd> |
| Recent Sessions | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>S</kbd> |
| Split horizontally | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Enter</kbd> |
| Split vertically | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>\</kbd> |
| Move between panes | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Arrow</kbd> |
| Cycle Dojo tabs | <kbd>Ctrl</kbd>+<kbd>Tab</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Tab</kbd> |
| New Dojo tab | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd> |
| Detach active tab | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Q</kbd> |
| Search scrollback | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>F</kbd> |
| Copy / paste | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>C</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>V</kbd> |

Reopening attaches to processes that are still running. Starting an exited process again from saved launch metadata is an explicit **restore** operation.

Continue with the [quickstart](https://splinterm.com/docs/quickstart/) or [sessions and persistence](https://splinterm.com/docs/sessions/).

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
| Manage persistence, restore, and reset | [Sessions and persistence](https://splinterm.com/docs/sessions/) |
| Configure the terminal | [Configuration](https://splinterm.com/docs/configure/configuration/) |
| Check maturity and availability | [Current status](https://splinterm.com/docs/status/) |
| Troubleshoot an installation | [Troubleshooting](https://splinterm.com/docs/troubleshooting/) |
| Contribute to the project | [Development guide](https://splinterm.com/docs/development/) |

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

## License

Splinterm is available under the [MIT License](LICENSE).
