# Architecture

[ADR 0001](adr/0001-foot-rust-port.md) establishes Foot as the authoritative
implementation and behavioral foundation for Splinterm's Rust terminal port.
The boundaries below reorganize ownership for persistence; they do not replace
Foot with another terminal engine.

Splinterm has two lifetimes: the graphical client may come and go, while the
daemon owns terminal processes and session state.

```text
┌──────────────── splinterm ────────────────┐
│ Wayland frontend · input · renderer · UI  │
└───────────────────┬───────────────────────┘
                    │ versioned Unix socket
┌───────────────────▼───────────────────────┐
│                 splinterd                 │
│ protocol · Lair state · PTYs · persistence│
└───────────────────┬───────────────────────┘
                    │
        Lair → Dojos → Windows → Splints
```

## Crate boundaries

### `splinterm-core`

Transport- and UI-independent state. A `Lair` owns named `Dojo` workspaces. A
`Dojo` owns windows, and each window owns a binary layout tree whose leaves are
`Splint` terminal surfaces.

This crate must not depend on Wayland, async runtimes, PTYs, or a wire format.

### `splinterm-protocol`

Request/response types shared by both processes. The current development
protocol uses bounded length-prefixed JSON frames, version-range negotiation,
request IDs, peer-UID verification, stable errors, and explicit subscription
resynchronization. Protocol v12 carries closed access scopes, grant status,
revocation events, bounded direct command argv/working-directory/shell launch
fields, semantic terminal updates, history generations and stable row IDs,
revision-bound scrollback pages, and visible-row identity. Command arguments
are never rebuilt as a shell string. Protocol DTOs remain separate from
terminal and daemon runtime structs.

### `splinterd`

Owns the authoritative Lair and, eventually, all PTY file descriptors, child
processes, scrollback, and durable snapshots. Clients should be disposable
without ending sessions. The daemon must not depend on Wayland and should run
on headless Linux hosts such as `neuromancer`.

For ungranted terminal access, the daemon launches a disposable sibling
`splinterm consent` process over a private inherited Unix socket. Random
one-use capability material and the bounded grant-once/deny exchange stay on
that channel. In-memory grants bind peer UID/PID/executable identity,
Splint/incarnation, and explicit scopes. Revocation releases tied controllers
and subscriptions; bounded audit metadata excludes terminal, clipboard, and
input bodies. See [ADR 0005](adr/0005-trusted-consent-broker.md).

### `splinterm`

This is the disposable Wayland terminal UI, responsible for presentation,
local input, clipboard/IME integration, and rendering state received from the
daemon. Its private consent mode renders fixed trusted application chrome and
never accepts requester-supplied terminal content. Normal windows keep active
authority, development bypass, and controller state visibly indicated.

The graphical identity is fixed to `com.oldjobobo.splinterm` across Wayland and
desktop metadata. `splinterm launch` is the `xdg-terminal-exec` boundary.
Client-owned configuration controls renderer/window policy; a generated,
project-owned theme maps Omarchy roles and reloads without mutating daemon
terminal state or process lifetime.

## Private prerelease deployment

The Arch package installs all three adjacent runtime executables, an on-demand
systemd user service, xdg launcher/desktop metadata, icon/AppStream metadata,
theme generator, examples, and license notices. The launcher starts the daemon
and performs one bounded restart when protocol negotiation reveals a stale
pre-upgrade process. The unit uses SIGINT for the daemon's graceful child/socket
cleanup path. Package scripts do not edit user homes, terminal preferences, or
Omarchy-owned directories; see [packaging.md](packaging.md).

## Foot reference map

The local Foot source at `~/Playground/foot` suggests useful subsystem seams:

| Foot area | Splinterm destination |
| --- | --- |
| `client.c`, `server.c`, `client-protocol.h` | protocol + daemon transport |
| `terminal.c`, `commands.c`, `csi.c`, `osc.c`, `dcs.c` | future terminal engine crate |
| `grid.c` | future screen/scrollback model |
| `slave.c`, `spawn.c`, `reaper.c` | daemon PTY/process ownership |
| `render.c`, `shm.c` | client renderer |
| Wayland/input/IME modules | client platform layer |

Splinterm should preserve this separation while changing the ownership model:
terminal processes and canonical screen state belong to `splinterd`, not the
Wayland client's lifetime.

## Near-term invariants

1. A client crash must not terminate a splint.
2. Persistent state has one writer: `splinterd`.
3. Domain types do not know about rendering or transport.
4. The protocol is versioned and rejects incompatible peers.
5. Ported Foot code retains MIT attribution and provenance.
6. Headless remote access uses an authenticated relay such as SSH; `splinterd`
   does not expose a network listener by default.
