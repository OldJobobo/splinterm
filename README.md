# Splinterm

**Splinterm** is a proposed Rust-based, Omarchy-native evolution of
[foot](https://codeberg.org/dnkl/foot): a fast Wayland terminal emulator with
persistent multiplexing built in.

- `splinterm` — terminal emulator and client
- `splinterd` — persistent background server
- **Lair** — the collection of persistent sessions
- **Dojo** — a workspace containing windows and panes
- **Splints** — individual terminal surfaces

> [!IMPORTANT]
> Splinterm is pre-alpha. This repository currently establishes the process,
> domain, and protocol boundaries; it is not yet a usable terminal emulator.

## Workspace

```text
crates/
├── splinterm/           # interactive client and, later, Wayland frontend
├── splinterd/           # persistent session daemon
├── splinterm-core/      # Lair/Dojo/window/splint state model
└── splinterm-protocol/  # versioned client-daemon wire protocol
docs/
├── adr/
│   └── 0001-foot-rust-port.md
├── architecture.md
├── pre-planning-research.md
└── roadmap.md
```

The initial daemon uses newline-delimited JSON over a Unix socket. That keeps
protocol development inspectable while terminal, PTY, persistence, and Wayland
work is brought online. The wire format is versioned from the start.

## Try the scaffold

```bash
cargo build

# Terminal 1
cargo run -p splinterd

# Terminal 2
cargo run -p splinterm -- ping
cargo run -p splinterm -- new main
cargo run -p splinterm -- list
```

The default socket is `$XDG_RUNTIME_DIR/splinterm/splinterd.sock`. Override it
for development with `SPLINTERM_SOCKET=/path/to/socket`.

## Research direction

[`docs/adr/0001-foot-rust-port.md`](docs/adr/0001-foot-rust-port.md)
records Foot as the authoritative implementation and behavioral foundation for
the Rust port. [`docs/pre-planning-research.md`](docs/pre-planning-research.md)
evaluates persistent multiplexing, Omarchy/Arch/Nix priorities, and a secure
AI-accessible local API before implementation dependencies are frozen.

## Development

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Foot lineage

Foot's source architecture is a primary reference for the emulator half of the
project, particularly its separation of terminal state, grid, renderer,
Wayland plumbing, and client/server mode. Splinterm does not currently contain
copied Foot source. Any future ported or adapted code must retain the relevant
MIT attribution and be recorded in `THIRD_PARTY.md`.

## License

MIT
