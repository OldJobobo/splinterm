# Architecture

[ADR 0001](adr/0001-foot-rust-port.md) establishes Foot as the authoritative
implementation and behavioral foundation for Splinterm's Rust terminal port.
The boundaries below reorganize ownership for persistence; they do not replace
Foot with another terminal engine.

Splinterm has two lifetimes: the graphical client may come and go, while the
daemon owns terminal processes and session state. A third-party automation
client, including a coding agent or MCP adapter, is also disposable and receives
no authority from running inside a Splint.

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
resynchronization. Protocol v18 carries closed access scopes, grant status,
revocation events, bounded direct command argv/working-directory/shell launch
fields, semantic terminal updates, topology and history generations, stable row
IDs, revision-bound scrollback/search pages, visible-row identity, and bounded
per-Splint control-status/transfer events. Command arguments
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
input bodies. Per-Splint controller leases bind daemon-owned connection IDs.
Control observers receive subscriber-specific status without peer identity;
transfer requests are bounded, owner-decided, timeout-denied, and cancelled on
either disconnect. Forced transfer uses a separate trusted consent prompt.
Literal search runs inside the owning terminal actor with fixed query/result/
preview/deadline bounds and opaque cursors tied to incarnation, terminal
revision, and history generation. See
[ADR 0005](adr/0005-trusted-consent-broker.md) and
[ADR 0006](adr/0006-multiplexing-lifecycle.md).

### `splinterm`

This is the disposable Wayland terminal UI, responsible for presentation,
local input, clipboard/IME integration, and rendering state received from the
daemon. Its private consent mode renders fixed trusted application chrome and
never accepts requester-supplied terminal content. Normal windows keep active
authority, development bypass, controller state, pending transfer decisions,
and the local search surface visibly indicated. Search focus, query, match
selection, and viewport position remain client-local.

The graphical identity is fixed to `com.oldjobobo.splinterm` across Wayland and
desktop metadata. `splinterm launch` is the `xdg-terminal-exec` boundary.
Client-owned configuration controls renderer/window policy; a generated,
project-owned theme maps Omarchy roles and reloads without mutating daemon
terminal state or process lifetime.

A daemon `Window` is logical topology, not a compositor-native surface.
Automation may create or mutate logical windows and their Splint trees without
mapping a Wayland window. Mapping, focusing, moving, resizing, or assigning a
native window to a compositor workspace requires a separate trusted graphical
broker and is not implied by topology mutation or the persisted default-focus
hint.

## Automation and coding-agent boundary

The supported path for a coding agent is a least-privileged automation client:
today the stable JSON/NDJSON CLI, and eventually the separately authorized
`splinterm-mcp` adapter. Both use the same daemon operations and policy checks;
neither inherits trusted-UI authority. If an agent shells out to the general
`splinterm` CLI, the daemon authorizes that CLI executable, so every process able
to invoke that exact binary can exercise the rule's scopes. The dedicated MCP
binary exists to provide a narrower independently reviewable identity.

Splinterm provides topology, PTYs, terminal observation, process lifecycle, and
exclusive input/resize control. It does not itself provide semantic agent task
status, inter-agent messaging, readiness, completion, or result transport. A
higher-level orchestrator may build those semantics over structured process
launch and bounded observation, but terminal content remains untrusted data and
must never become authority or an automatic instruction.

PTY children will eventually receive daemon-overridden logical context hints for
their Dojo, window, Splint, and incarnation. These values improve discovery for
an in-Splint agent but are not credentials and are never accepted as proof of
resource authority.

## Private prerelease deployment

The Arch package installs four adjacent runtime executables—`splinterm`,
`splinterd`, `splinterm-relay`, and `splinterm-pty-child`—plus an on-demand
systemd user service, xdg launcher/desktop metadata, icon/AppStream metadata,
theme generator, examples, and license notices. The launcher starts the daemon
and performs one bounded restart when protocol negotiation reveals a stale
pre-upgrade process. The headless-capable unit does not require graphical
display variables, preserves them for PTY children when the graphical session
provides them, strips the unsupported development authorization bypass, loads
only an optional owner-controlled environment file, uses SIGHUP for atomic
fail-closed policy reload, and uses SIGINT for graceful child/socket cleanup.
Policy validation and inspection reuse the daemon's secure loader locally;
reload remains systemd control rather than an ordinary socket RPC. Package
scripts do not edit user homes, enable lingering, change SSH policy, or mutate
Omarchy-owned directories; see [headless.md](headless.md) and
[packaging.md](packaging.md).

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
6. Headless remote access uses the dedicated, exact-policy
   `splinterm-relay` transport over authenticated SSH; `splinterd` exposes no
   network listener and never attributes the remote caller as its local peer.
7. Shutdown owns and drains connection tasks before runtime shutdown and final
   metadata persistence; one pinned signal future prevents lost SIGINT events.
8. Logical window mutation never claims compositor-native mapping or focus.
9. In-Splint context and terminal-derived data are discovery inputs, never
   authorization, consent, or executable instructions.
10. Future-descendant authority must be explicit and bounded; containment alone
    cannot silently broaden an automation rule.
