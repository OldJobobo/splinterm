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
local native splinterm
  ├─ trusted local endpoint ───────────────→ owner-only splinterd Unix socket
  └─ remote endpoint (native Window, one endpoint per lifetime)
       └─ one OpenSSH child
            └─ splinterm relay --graphical-stdio
                 └─ bounded logical channels
                      └─ independently validated remote splinterd Unix connections

splinterd → Topology → Lairs → Dojos → Splints
```

## Crate boundaries

### `splinterm-core`

Transport- and UI-independent state. A `Topology` catalogs named persistent
`Lair` sessions. Each `Lair` owns zero or more `Dojo` layouts, and each `Dojo`
owns a binary layout tree whose leaves are `Splint` terminal surfaces.

This crate must not depend on Wayland, async runtimes, PTYs, or a wire format.

### `splinterm-protocol`

Request/response types shared by both processes. The current development
protocol uses bounded length-prefixed JSON frames, version-range negotiation,
request IDs, peer-UID verification, stable errors, and explicit subscription
resynchronization. Protocol v27 carries closed access scopes, grant status,
revocation events, bounded direct command argv/working-directory/shell launch
fields, semantic terminal updates, topology and history generations, stable row
IDs, revision-bound scrollback/search pages, visible-row identity, and bounded
per-Splint control-status/transfer events. Command arguments
are never rebuilt as a shell string. Protocol DTOs remain separate from
terminal and daemon runtime structs.

### `splinterd`

Owns the authoritative Topology and all PTY file descriptors, child
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
selection, and viewport position remain client-local. A managed native Window
also owns an ordered, non-persistent set of at most 32 Dojo tabs and one active
tab. Tabs never enter `splinterm-core`, daemon topology, policy, audit, public
CLI/MCP data, or child context. The same Window may reference Dojos from
multiple Lairs, while duplicate opens activate the existing local tab.

Open tabs retain bounded terminal subscriptions. Hidden tabs drain semantic
updates without painting, blinking, resizing, focus reporting, or controller
ownership; activation installs cached frontend state and performs one active
geometry reconciliation. Closing a tab releases its client resources without
closing the referenced Dojo or Splints. The tab strip is trusted application
chrome, and one content rectangle below it is authoritative for pane layout,
terminal geometry, input mapping, overlays, IME, and damage.

Terminal images remain sparse protocol-independent content and placement
records in the daemon-owned terminal state. Trusted UI snapshots carry bounded
metadata only; immutable pixel bodies are fetched on demand through sealed
memfd or a separate bounded binary socket and retained under one renderer-wide
source budget. The client composes premultiplied BGRA into its existing CPU
backing/Wayland SHM path, clipped independently per pane. Public automation,
audit, and relay records never expose image bodies. See [images.md](images.md)
for the compatibility and resource matrix.

The graphical identity is fixed to `com.oldjobobo.splinterm` across Wayland and
desktop metadata. `splinterm launch` is the `xdg-terminal-exec` boundary.
Client-owned configuration controls renderer/window policy; a generated,
project-owned theme maps Omarchy roles and reloads without mutating daemon
terminal state or process lifetime.

A daemon `Dojo` is persistent topology, not a compositor-native surface.
Automation may create or mutate Dojos and their Splint trees without mapping a
native `Window`. Mapping, focusing, moving, resizing, or assigning a Window to a
compositor workspace requires a separate trusted graphical broker and is not
implied by topology mutation or the persisted default-focus hint.

### `splinterm-graphical-relay` and `splinterm-relay`

The shared graphical-relay crate owns only the bounded outer framing and the
client-side logical byte-channel router. It does not depend on or parse the
private daemon protocol. Exact magic/version, channel identities, data and
diagnostic sizes, channel count, per-channel queues, and aggregate queues fail
closed.

`splinterm-relay --stdio` remains one byte-transparent automation connection.
The distinct `--graphical-stdio` mode accepts outer channel requests and repeats
socket path, owner/mode, peer UID, pidfd lifetime, and exact adjacent daemon
executable validation for every channel. Daemon EOF is channel-local; validated
daemon process exit, SSH/stdin EOF, corrupt outer framing, or aggregate-bound
failure closes the complete session.

### Remote endpoint foundation

`remote.rs` owns strict credential-free profile parsing and structured OpenSSH
argv. `remote_session.rs` owns one SSH child, separate bounded stderr drain,
outer negotiation, cloneable logical-channel creation, shutdown/kill/reap, and
categorized transport failures. It never reads key material or stores password
or passphrase values.

`endpoint.rs` defines the behavior carried with a clonable connection factory:
local uses `TrustedUi`, trusted local image content, focus publication, local
launch semantics, trusted force-transfer authority, and the `local` recency
namespace; remote uses `RemoteInteractive`, unavailable image transport,
disabled focus publication, unavailable forced transfer, remote-daemon launch
defaults, and `remote-PROFILE` recency. OpenSSH authenticates the human remote
account; persistent automation policy is not part of this path. The daemon
accepts the role only from the adjacent relay running `--graphical-stdio`, while
raw `--stdio` remains `Automation`. Phase 2 routes CLI selection, session
discovery, Window startup, pane observation/control, topology reconciliation,
hidden tabs, history, search, mutations, and cleanup through clones of that one
factory. A Window therefore cannot retarget or mix local and remote identities
after startup.

## Client module boundaries

The `splinterm` binary entry point performs only Tokio runtime setup and calls
one private application entry point. Binary-owned orchestration lives under
`app/`:

| Area | Ownership |
| --- | --- |
| `app/commands.rs` | Leaf-only Clap grammar and value conversions |
| `app/cli.rs` | Output-mode selection and human command routing |
| `app/machine/` | Stable JSON/NDJSON requests, envelopes, deadlines, and subscriptions |
| `app/local_service.rs`, `consent.rs`, `sessions.rs` | Local policy/reset/relay services, trusted consent, session selection, and picker presentation |
| `app/remote_cli.rs` | Strict profile listing/inspection and non-mutating remote reachability checks |
| `app/session_catalog.rs` | Endpoint-aware local/remote-safe launch requests, namespaced recent-session state, and picker projections |
| `app/window.rs` | Graphical task startup, renderer configuration, image-cache setup, and single-/multi-pane lifecycle coordination |
| `app/pane_bridge.rs` | Bounded daemon-to-frontend pane subscriptions, control, resize, image leases, and resynchronization |
| `app/topology_manager.rs` | Per-Dojo async task ownership and topology reconciliation |
| `app/theme_watch.rs` | Bounded theme observation and publication |

Application services do not own Smithay objects, Wayland proxies, SHM buffers,
or renderer frames. As the terminal-window composition boundary, `app/window.rs`
configures the public renderer facade, creates the bounded shared image-content
cache, and maps `WindowOptions` through `run_window`. The session-picker path in
`sessions.rs` also configures the renderer and calls the same public window
facade for its transient trusted UI. The async pane and topology services
exchange frontend contracts, image leases, and protocol data without depending
on Wayland or renderer frames. Command grammar and session-catalog
helpers are neutral leaves, so machine/local-service dispatch and topology/window
orchestration have no reverse dependency on their callers.

Library-owned graphical code has one-way internal dependencies:

```text
frontend contracts ───────────────┐
                                  ▼
renderer ← geometry/config    wayland facade
   ▲                              │
   └──────── explicit frames ─────┘

app/cli → commands + machine/local/session/window services
app/window + sessions picker → renderer configuration + public wayland facade
app/pane_bridge + topology_manager → frontend contracts + daemon protocol
sessions + topology_manager → neutral session_catalog
```

`frontend/` owns platform-neutral window messages, options, picker state, and
topology contracts. `renderer/` owns immutable process resources, explicit
per-window render contexts, prepared frames, text/raster/image composition,
overlays, and deterministic capture. `wayland/` owns compositor state, SHM,
input/IME/clipboard, damage scheduling, graphical tabs and chrome, and thin
protocol dispatch. Its `App` composes cohesive platform, surface, presentation,
input, clipboard, panes, tabs, modal, and scheduling state rather than exposing
one flat mutable object. Generic tab policy remains in `tab.rs`; graphical tab
presentation remains in `wayland/tabs.rs`; daemon task ownership remains in
`app/topology_manager.rs`.

Only the established library facades are public. Reducers and dispatch seams use
private, `pub(super)`, or `pub(in crate::wayland)` visibility. Binary application
internals use `pub(in crate::app)`; only the single application entry point is
visible to the crate-root binary.

## Automation and coding-agent boundary

The supported path for a coding agent is a least-privileged automation client:
the stable JSON/NDJSON CLI or the separately packaged and authorized
`splinterm-mcp` stdio adapter. Both use the same daemon operations and policy checks;
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

PTY children receive daemon-overridden logical context hints for their Lair,
Dojo, Splint, and current incarnation. These values improve discovery for an
in-Splint agent but are not credentials and are never accepted as proof of
resource authority. Relaunch replaces the incarnation, and supported clients
reconcile every hint against current public topology before selection.

## Private prerelease deployment

The main Arch package installs four adjacent runtime executables—`splinterm`,
`splinterd`, `splinterm-relay`, and `splinterm-pty-child`—plus the public-CLI-only
`splinterm-session-picker` reference integration, an on-demand systemd user
service, xdg launcher/desktop metadata, icon/AppStream metadata, theme generator,
examples, and license notices. The launcher starts the daemon
and performs one bounded restart when protocol negotiation reveals a stale
pre-upgrade process. The headless-capable unit does not require graphical
display variables, preserves them for PTY children when the graphical session
provides them, strips the unsupported development authorization bypass, loads
only an optional owner-controlled environment file, uses SIGHUP for atomic
fail-closed policy reload, and uses SIGINT for graceful child/socket cleanup.
The optional `splinterm-mcp` split package adds only the independently
policy-identified stdio adapter, its setup guide, and license notices; installation
or MCP host configuration grants no daemon authority. Policy validation and inspection reuse the daemon's secure loader locally;
reload remains systemd control rather than an ordinary socket RPC. Package
scripts do not edit user homes, enable lingering, change SSH policy, or mutate
Omarchy-owned directories; see [headless.md](headless.md),
[integrations.md](integrations.md), [mcp.md](mcp.md), and
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
6. Headless automation uses the dedicated, exact-policy raw relay over
   authenticated SSH. Native human remote transport uses one authenticated SSH
   child with bounded logical channels negotiating `RemoteInteractive`; it does
   not consult automation policy or receive trusted image, forced-transfer, or
   graphical-focus authority. `splinterd` exposes no network listener.
7. Shutdown owns and drains connection tasks before runtime shutdown and final
   metadata persistence; one pinned signal future prevents lost SIGINT events.
8. Dojo mutation never claims native Window mapping or focus; Window-local tab
   operations never claim daemon topology mutation.
9. In-Splint context and terminal-derived data are discovery inputs, never
   authorization, consent, or executable instructions.
10. Future-descendant authority must be explicit and bounded; containment alone
    cannot silently broaden an automation rule.
