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
├── splinterm-protocol/  # versioned client-daemon wire protocol
├── splinterm-pty/       # Linux PTY and child-process boundary
└── splinterm-terminal/  # Foot-derived grid and streaming VT kernel
docs/
├── adr/
│   ├── 0001-foot-rust-port.md
│   └── 0002-linux-pty-backend.md
├── plans/
│   └── 0001-terminal-kernel.md
├── architecture.md
├── pre-planning-research.md
└── roadmap.md
```

The initial daemon uses newline-delimited JSON over a Unix socket.
`splinterm-terminal` contains the Foot-derived cell/grid model, streaming VT
kernel, borrowed semantic snapshots, monotonic revisions, and bounded update
replay. `splinterm-pty` provides the tested Linux PTY/process boundary.
`splinterd` now owns one live shell actor that continuously consumes its PTY,
tracks terminal state, and survives client disconnection. The local protocol
uses bounded framed messages, version negotiation, request IDs, peer-UID
verification, owner-only socket permissions, and explicit resynchronization.
Persistence and Wayland remain to be connected.

## Try the scaffold

```bash
cargo build

# Terminal 1 — terminal access is deliberately opt-in during development
SPLINTERM_ENABLE_DEV_ATTACH=1 cargo run -p splinterd

# Terminal 2
cargo run -p splinterm -- ping
cargo run -p splinterm -- new main
cargo run -p splinterm -- list
cargo run -p splinterm -- send $'printf "hello from the PTY\\n"\n'
cargo run -p splinterm -- snapshot
```

The default socket is `$XDG_RUNTIME_DIR/splinterm/splinterd.sock`. Override it
for development with `SPLINTERM_SOCKET=/path/to/socket`.

## Research direction

[`docs/adr/0001-foot-rust-port.md`](docs/adr/0001-foot-rust-port.md)
records Foot as the authoritative implementation and behavioral foundation for
the Rust port. [`docs/pre-planning-research.md`](docs/pre-planning-research.md)
evaluates persistent multiplexing, Omarchy/Arch/Nix priorities, and a secure
AI-accessible local API before implementation dependencies are frozen. The
[first implementation plan](docs/plans/0001-terminal-kernel.md) defines the
Foot-derived terminal kernel and detachable one-Splint vertical slice.

## Development

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Optional parser fuzzing (requires cargo-fuzz)
cargo fuzz run terminal-advance
```

## Foot lineage

Foot's source architecture is the authoritative foundation for the emulator
half of the project. `splinterm-terminal` begins the Rust translation of Foot's
terminal representations; translated modules record the pinned source file and
commit in their documentation. Ported or adapted code retains the relevant MIT
attribution and is recorded in `THIRD_PARTY.md`.

## License

MIT
