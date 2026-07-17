# Architecture

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

Request/response types shared by both processes. The scaffold uses
newline-delimited JSON so early protocol changes are easy to inspect. Framing
and encoding may change later, but protocol versions must remain explicit.

### `splinterd`

Owns the authoritative Lair and, eventually, all PTY file descriptors, child
processes, scrollback, and durable snapshots. Clients should be disposable
without ending sessions. The daemon must not depend on Wayland and should run
on headless Linux hosts such as `neuromancer`.

### `splinterm`

Today this is a small control client. It will become the Wayland terminal UI,
responsible for presentation, local input handling, clipboard/IME integration,
and rendering state received from the daemon.

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
