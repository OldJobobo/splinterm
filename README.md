# Splinterm

**Splinterm** is a proposed Rust-based, Omarchy-native evolution of
[foot](https://codeberg.org/dnkl/foot): a fast Wayland terminal emulator with
persistent multiplexing built in.

- `splinterm` — terminal emulator and client
- `splinterd` — persistent background server
- **Topology** — the daemon's complete persistent session catalog
- **Lair** — a named persistent session or project
- **Dojo** — one persistent terminal layout within a Lair
- **Splint** — an individual terminal pane/process

> [!IMPORTANT]
> Splinterm is a private prerelease. The Omarchy-native terminal MVP, headless
> multi-Splint lifecycle, and explicit durable metadata restore are validated.
> Persistent multi-window/pane multiplexing, explicit multi-client control,
> stable local JSON/NDJSON automation, headless policy administration, bounded
> audit inspection, dedicated SSH stdio relay, daemon-injected logical context,
> and public-CLI reference session picker are validated. The full-capability
> `splinterm-mcp` implementation, optional split package, extracted-package
> runtime, host interoperability, and approved stdio fallback evidence are
> validated. Core Phase 4 is complete; public distribution remains open.

## Workspace

```text
crates/
├── splinterm/           # interactive client and native Wayland frontend
├── splinterd/           # persistent session daemon
├── splinterm-core/      # Topology/Lair/Dojo/Splint state model
├── splinterm-protocol/  # versioned client-daemon wire protocol
├── splinterm-relay/     # dedicated policy-identified SSH stdio transport
├── splinterm-mcp/       # optional policy-identified MCP stdio adapter
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
Dojos, and clipped multi-pane composition in one Wayland Window are implemented.

## Install on Arch/Omarchy

From a clean clone or pull, build and install the current committed version with:

```bash
./install.sh
```

The script installs missing package dependencies, builds and validates the Arch
package, and asks before installing it. It does **not** change the default
terminal or edit Omarchy configuration. It never opts a fresh installation into
the optional MCP package; when MCP is already installed, it is upgraded to keep
the package versions matched. Use `./install.sh --check` to include the full
package test suite, or `--yes` for an already-approved unattended installation.

## Daily use

The normal desktop entry and `xdg-terminal-exec` path always open a fresh Lair
with one Dojo and Splint, running the requested command or configured shell when none
is supplied. Reopening is a separate, non-destructive action:

```bash
splinterm sessions  # choose New Terminal or a recent running Dojo
splinterm reopen    # reopen the last locally remembered running Dojo
```

From any focused managed Splinterm terminal, **Ctrl+Shift+S** opens Recent
Sessions as trusted application chrome over dimmed live panes in the current
Window. Escape removes the overlay without replacing the terminal frontend.
**Ctrl+Shift+P** opens a searchable command palette with the initial New Dojo,
horizontal split, and vertical split actions. Right-clicking any visible Dojo
tab opens a compact trusted menu with New Dojo and detach-only Close Tab
actions targeted to that exact tab. Choosing a running session opens
it as a Window-local Dojo tab, or activates its
existing tab; **New Terminal** creates a fresh Lair and opens its initial Dojo as
a tab. These application-owned actions are not forwarded to terminal processes.
Tabs normally show the sanitized Dojo name; when that would be ambiguous, they
show the sanitized `Lair / Dojo` label instead.

The native Recent Sessions picker shows human Lair/Dojo names, starting
directory, Splint count, and running state without exposing UUIDs. Use arrow keys
or J/K to select, Enter to open, N for a fresh terminal, Escape to cancel, or
click an entry. Compact and minimal windows page adaptively; vertical wheel or
touchpad scrolling moves through actions when every row cannot fit. Reopening maps a daemon-owned Dojo into a native Window when its complete
Splint layout is still running; it does not create, relaunch, or restore a
process. Dojos with exited Splints remain available through the explicit
`restore`, `restore-dojo`, and `restore-lair` commands.

Packaged desktop actions expose **New Terminal**, **Recent Sessions**, and
**Reopen Last Session**. Omarchy users can bind `splinterm-sessions` to a global
shortcut such as Super+Shift+Enter while leaving the normal terminal shortcut
on `splinterm-xdg-terminal-exec`.

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
# Active Lairs only, with stable Lair, Dojo, and Splint UUIDs.
cargo run -p splinterm -- list
# Include exited-only Lairs and complete topology details.
cargo run -p splinterm -- list --all
LAIR_ID=xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
DOJO_ID=xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
SPLINT_ID=xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
cargo run -p splinterm -- send "$SPLINT_ID" $'printf "hello from the PTY\\n"\n'
cargo run -p splinterm -- snapshot "$SPLINT_ID"
cargo run -p splinterm -- split "$SPLINT_ID" --axis horizontal --side second
cargo run -p splinterm -- ratio "$SPLINT_ID" 650
cargo run -p splinterm -- rename-lair "$LAIR_ID" work
cargo run -p splinterm -- rename-dojo "$DOJO_ID" editor
# Convenience metadata only; connected clients retain their own actual focus.
cargo run -p splinterm -- dojo-focus-hint "$DOJO_ID" "$SPLINT_ID"
cargo run -p splinterm -- rename-splint "$SPLINT_ID" shell
cargo run -p splinterm -- new-dojo "$LAIR_ID" --name logs
cargo run -p splinterm -- kill "$SPLINT_ID"       # prompts before termination
cargo run -p splinterm -- relaunch "$SPLINT_ID"   # replacement launch parameters
cargo run -p splinterm -- restore "$SPLINT_ID"    # saved launch metadata
cargo run -p splinterm -- restore-dojo "$DOJO_ID"
cargo run -p splinterm -- restore-lair "$LAIR_ID"
# `close` and `close-dojo` require every affected Splint to have exited.
cargo run -p splinterm -- kill "$SPLINT_ID" --yes # required for non-interactive use
cargo run -p splinterm -- close "$SPLINT_ID"
cargo run -p splinterm -- close-dojo "$DOJO_ID"

# Open one native Window with the selected daemon-owned Dojo as its initial tab.
# A Window may attach up to 32 distinct Dojos, including Dojos from other Lairs.
# Window/tab attach and detach never create, kill, restore, or close a Dojo.
cargo run -p splinterm -- window --lair-id "$LAIR_ID" --dojo-id "$DOJO_ID"
```

For the packaged systemd user service, reset every session through one guarded
command instead of manually moving state files:

```bash
splinterm reset       # prompts before resetting
splinterm reset --yes # explicit unattended confirmation
```

Durable metadata is stored under `$XDG_STATE_HOME/splinterm/`, falling back to
`$HOME/.local/state/splinterm/`. Writes use an owner-only atomic primary plus a
previous-generation backup. Startup quarantines malformed metadata and never
runs a saved command automatically. `splinterm reset` atomically moves the
complete session database to a timestamped backup, restarts the canonical user
service, waits for its configured socket, and leaves policy and configuration
untouched. Restored layouts retain their stable IDs, but every explicitly
restored process receives a new incarnation. Terminal and
scrollback bodies, clipboard data, PTY handles, grants, and controller tokens
are never persisted.

The `window` command keeps bounded subscriptions for every Splint in up to 32
Window-local Dojo tabs. It renders only the active Dojo's persisted binary layout
tree below trusted tab chrome, sends UTF-8 and essential terminal keys only to
the client-local focused Splint, and derives each active PTY/grid size from its
pane rectangle. Hidden tabs continue draining bounded semantic updates but do
not paint, blink, resize, emit focus reports, or retain controller leases. Pane observers do not acquire controller
leases merely by attaching or receiving focus. First input acquires the focused
pane's exclusive connection-owned lease, applies its remembered geometry, and
then delivers input in order; explicit release or disconnect relinquishes it.
Function/navigation/keypad keys,
xterm modifiers, application cursor/keypad modes, xkb compose, focus reporting,
and exact snapshot colors are supported. Protocol v18 streams bounded semantic
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
Ctrl+Shift+L releases the local controller. Directional pane focus uses
Ctrl+Shift+Arrow. Ctrl+Tab and Ctrl+Shift+Tab cycle Window-local Dojo tabs;
Ctrl+Shift+D creates a Dojo in the active Lair; Ctrl+Shift+Q detaches the active
tab and closes the native Window when it was the final tab. Tab order is not
restored after the Window exits. Ctrl+Shift+Enter splits horizontally,
Ctrl+Shift+\\ splits vertically, Ctrl+Shift+W terminates and closes the focused
Splint (or directly removes it when already exited), and Ctrl+Shift+[ / ]
adjusts its parent ratio. Multi-Splint windows use
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
interpolation. Open clients read and safely reload the active Omarchy Quattro
`colors.toml` and effective `foot.ini` directly, with no hook or generated
runtime palette. Project-owned configuration lives in `config/`; see
[`docs/configuration.md`](docs/configuration.md) for the supported Foot subset
and migration guide.

Bounded static terminal images are implemented through one daemon-owned image
plane: Foot-compatible Sixel, the documented practical Kitty RGB/RGBA/PNG
subset, and inline-only iTerm2 PNG. Pixel bodies use a trusted on-demand binary
channel rather than public automation JSON. External Kitty file/SHM transports,
placeholders, and animation remain unsupported or deferred; see
[`docs/images.md`](docs/images.md) for the exact compatibility matrix, limits,
and evidence.

The default socket is `$XDG_RUNTIME_DIR/splinterm/splinterd.sock`. Override it
for development with `SPLINTERM_SOCKET=/path/to/socket`. The packaged daemon is
Wayland-independent and supports on-demand or persistent systemd user-service
operation with explicit owner-controlled policy. See
[`docs/headless.md`](docs/headless.md) for policy validation/reload, logout and
lingering behavior, service accounts, backups, upgrades, and recovery. Remote
automation uses the dedicated, policy-scoped SSH stdio relay documented in
[`docs/remote.md`](docs/remote.md). Client authors and in-Splint tools should use
the checklist, safe `jq` examples, and packaged reference picker in
[`docs/integrations.md`](docs/integrations.md).

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

# Optionally export an Omarchy palette as an explicit portable JSON override
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
