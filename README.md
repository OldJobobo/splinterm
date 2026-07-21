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
> Splinterm is a private prerelease. The Omarchy-native terminal MVP, headless
> multi-Splint lifecycle, and explicit durable metadata restore are validated.
> Persistent multi-window/pane multiplexing, explicit multi-client control
> transfer, and bounded scrollback search are validated. Supported third-party
> automation and public distribution remain future work.

## Workspace

```text
crates/
├── splinterm/           # interactive client and native Wayland frontend
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
`splinterd` owns a bounded registry of live shell actors that continuously consume
independent PTYs, track terminal state, and survive client disconnection. The local protocol
uses bounded framed messages, version negotiation, request IDs, peer-UID
verification, owner-only socket permissions, and explicit resynchronization.
The complete headless lifecycle is covered by an isolated real-daemon test.
Roadmap Phases 1, 2, and 3 are complete; the
[Omarchy-native terminal MVP plan](docs/plans/0002-omarchy-terminal-mvp.md)
links exact renderer, graphical sign-off, and private package evidence.
Headless multiplexing, crash-safe metadata restore, independently attachable
Dojo windows, and clipped multi-pane composition in one Wayland toplevel are implemented.

## Try the scaffold

For the current isolated development build, run:

```bash
./splinterm-test          # build, start/reuse the test daemon, and open Splinterm
./splinterm-test restart  # restart after protocol/daemon changes
./splinterm-test ping     # build and verify the isolated daemon
./splinterm-test stop     # stop the isolated daemon
```

The helper uses an owner-only socket under `$XDG_RUNTIME_DIR/splinterm-test`,
keeps daemon logs there, and enables the explicitly labeled development attach
bypass. It requires `pkg-config` and the FreeType development files.

Manual commands remain available:

```bash
cargo build

# Terminal 1 — normal access is granted through trusted graphical consent
cargo run -p splinterd

# Terminal 2
cargo run -p splinterm -- ping
cargo run -p splinterm -- new main
cargo run -p splinterm -- list       # shows stable Dojo, window, and Splint UUIDs
DOJO_ID=xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
WINDOW_ID=xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
SPLINT_ID=xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
cargo run -p splinterm -- send "$SPLINT_ID" $'printf "hello from the PTY\\n"\n'
cargo run -p splinterm -- snapshot "$SPLINT_ID"
cargo run -p splinterm -- split "$SPLINT_ID" --axis horizontal --side second
cargo run -p splinterm -- ratio "$SPLINT_ID" 650
cargo run -p splinterm -- rename-dojo "$DOJO_ID" work
cargo run -p splinterm -- rename-window "$WINDOW_ID" editor
# Convenience metadata only; connected clients retain their own actual focus.
cargo run -p splinterm -- window-focus-hint "$WINDOW_ID" "$SPLINT_ID"
cargo run -p splinterm -- rename-splint "$SPLINT_ID" shell
cargo run -p splinterm -- new-window "$DOJO_ID" --title logs
cargo run -p splinterm -- kill "$SPLINT_ID"       # prompts before termination
cargo run -p splinterm -- relaunch "$SPLINT_ID"   # replacement launch parameters
cargo run -p splinterm -- restore "$SPLINT_ID"    # saved launch metadata
cargo run -p splinterm -- restore-window "$WINDOW_ID"
cargo run -p splinterm -- restore-dojo "$DOJO_ID"
# `close` and `close-window` require every affected Splint to have exited.
cargo run -p splinterm -- kill "$SPLINT_ID" --yes # required for non-interactive use
cargo run -p splinterm -- close "$SPLINT_ID"
cargo run -p splinterm -- close-window "$WINDOW_ID"

# Exactly one native toplevel for the selected daemon window.
# Opening/closing the UI only attaches/detaches; it does not create, kill, restore,
# focus, or otherwise alter another daemon window.
cargo run -p splinterm -- window --dojo-id "$DOJO_ID" --window-id "$WINDOW_ID"
```

Durable metadata is stored under `$XDG_STATE_HOME/splinterm/`, falling back to
`$HOME/.local/state/splinterm/`. Writes use an owner-only atomic primary plus a
previous-generation backup. Startup quarantines malformed metadata and never
runs a saved command automatically. Restored layouts retain their stable IDs,
but every explicitly restored process receives a new incarnation. Terminal and
scrollback bodies, clipboard data, PTY handles, grants, and controller tokens
are never persisted.

The `window` command keeps one ordered daemon subscription per pane, renders a
persisted binary window tree into one clipped backing buffer, sends UTF-8 and
essential terminal keys to the client-local focused pane, and derives each
PTY/grid size from its pane rectangle. Pane observers do not acquire controller
leases merely by attaching or receiving focus. First input acquires the focused
pane's exclusive connection-owned lease, applies its remembered geometry, and
then delivers input in order; explicit release or disconnect relinquishes it.
Function/navigation/keypad keys,
xterm modifiers, application cursor/keypad modes, xkb compose, focus reporting,
and exact snapshot colors are supported. Protocol v17 streams bounded semantic
row, scroll, cursor, mode, palette, dimension, and title updates. The client
coalesces damage to Wayland frame callbacks, incrementally prepares changed
rows, scroll-copies reusable backing pixels, submits row damage, and uses a
bounded scale-specific glyph cache. Pointer selection, regular and primary
clipboard, bounded safe bracketed paste, application mouse reporting, and
user-gesture-only HTTP(S) opening are supported. Paired fractional-scale/
viewport rendering, `text-input-v3` preedit and commit, inactive-IME compose
fallback, focus indication, and reduced-motion cursor behavior are implemented. Protocol v7 replaces the
normal development grant with a daemon-launched trusted Wayland consent client,
scoped five-minute grant-once authority, explicit revocation, and visible
active-authority/controller indication. Ctrl+Shift+R revokes active grants and
Ctrl+Shift+L releases the local controller. Pane focus uses Ctrl+Shift+Arrow or
Ctrl+Shift+Tab (Shift+Tab reverses traversal). Ctrl+Shift+Enter splits
horizontally, Ctrl+Shift+\\ splits vertically, Ctrl+Shift+W closes an exited
pane, and Ctrl+Shift+[ / ] adjusts its parent ratio. Multi-Splint windows use
trusted box-drawing chrome configured by `[multiplexer] divider-style=line`,
`frame`, or `none`. Frame mode optionally displays the sanitized daemon-owned
Splint title with `frame-title=splint`; terminal OSC titles cannot spoof it.
Active and inactive borders follow the live-reloaded `pane_border_active` and
`pane_border` theme roles. Ctrl+Shift+T requests control from another client;
the current owner accepts with Ctrl+Shift+Y or denies with Ctrl+Shift+N, and a
request times out closed. Ctrl+Shift+U starts the separate trusted confirmation
for a forced transfer. Ctrl+Shift+F opens the trusted local literal-search
surface; Enter searches case-insensitively, Ctrl+N/P navigates matches, and
Escape closes it. Search results and opaque cursors are bounded and invalidated
by terminal revision or history-generation changes. The
`SPLINTERM_ENABLE_DEV_ATTACH=1` bypass remains available only for isolated
development and is prominently labeled in the window title.

Phase 8 uses the stable identity `com.oldjobobo.splinterm`. The files under
`dist/` provide the desktop entry, icon, AppStream metadata, systemd user unit,
`xdg-terminals.list` entry, and `splinterm-xdg-terminal-exec` launcher. The
launcher preserves command arguments and working directory without shell
interpolation. Project-owned configuration and Omarchy theme templates live in
`config/`; open clients safely reload generated theme roles. See
[`docs/configuration.md`](docs/configuration.md) for the supported Foot subset
and migration guide.

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

# Complete isolated daemon/PTY detach/reattach/resync lifecycle
cargo test -p splinterd --test end_to_end -- --test-threads=1

# Human-paced Phase 1 persistence walkthrough
# (Foot is only the presenter for this headless milestone.)
tools/run-phase1-demo.py

# Roadmap Phase 2 native Wayland renderer preview
cargo run -p splinterm -- window

# Workspace-8-safe human review launcher
tools/run-wayland-window-demo.py

# Generate a project-owned theme from an Omarchy colors.toml
python tools/generate-omarchy-theme.py /path/to/colors.toml --output /tmp/theme.json

# Exercise the xdg-terminal-exec-compatible contract without installing it
PATH="$PWD/target/debug:$PATH" dist/bin/splinterm-xdg-terminal-exec --working-directory "$PWD"

# Initial renderer and font-stack evidence
cargo run --release -p splinterm --example cpu-shm-benchmark
cargo run -p splinterm --example font-stack-spike

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
