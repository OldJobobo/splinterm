# Splinterm glossary

## Session model

### Lair

The complete collection of persistent sessions managed by one `splinterd`
daemon. A Lair contains Dojos, their windows, and all associated Splints.

### Dojo

A named workspace within the Lair. A Dojo groups related logical windows, such
as those belonging to one project or task.

### Window

A persistent logical terminal layout within a Dojo. A Window contains one or
more Splints arranged in a layout tree. It exists independently of any native
Wayland window displaying it.

### Splint

An individual terminal pane within a Window. A Splint has a stable ID, terminal
state, launch metadata, and a process lifecycle.

### Layout tree

The horizontal and vertical split structure that arranges Splints inside a
Window. Each branch records its axis and split ratio; each leaf is a Splint.

### Topology

The complete logical structure of the Lair: Dojos, Windows, Splints, layout
trees, names, IDs, and lifecycle metadata.

## Programs and presentation

### `splinterd`

The background daemon that owns PTYs, child processes, terminal state, logical
topology, durable metadata, authorization state, and the local Unix socket.
Sessions can continue running while no graphical client is attached.

### `splinterm`

The graphical Wayland terminal client and command-line interface. It connects
to `splinterd` to display logical Windows or perform supported operations.

### Native window

A compositor-managed Wayland toplevel displaying one logical Splinterm Window.
Opening, closing, moving, or focusing a native window is separate from changing
the daemon's logical topology.

### PTY

A pseudoterminal owned by `splinterd` that connects a Splint's shell or process
to its terminal state.

## Lifecycle

### Attach

Display and observe an existing logical Window or Splint through a client.
Attaching does not create or restart its process.

### Detach

Stop displaying a logical Window without terminating its Splints or their
processes.

### Incarnation

A positive number identifying one process lifetime within a stable Splint.
Relaunching or restoring a Splint preserves its Splint ID but creates a new
incarnation, preventing stale automation from targeting the replacement.

### Relaunch

Start a new process incarnation for an exited Splint, optionally with reviewed
replacement launch parameters.

### Restore

Explicitly start an exited Splint from its saved launch metadata. Restoring can
operate on one Splint or the exited Splints in a Window or Dojo.

## Control and authorization

### Controller

The client currently holding the exclusive lease to send input or resize a
particular Splint. Observing or focusing a Splint does not itself grant control.

### Control transfer

The explicit process by which another client requests a Splint's controller
lease and the current controller accepts or denies that request.

### Grant

Temporary, revocable authority approved through Splinterm's trusted graphical
consent UI. Grants are bounded by requester, resource, scopes, incarnation,
expiry, and daemon lifetime.

### Policy

An owner-controlled JSON document granting an exact automation executable
specific operation scopes over selected resources with explicit limits and an
optional expiry. No policy means no persistent third-party automation grants.

### Scope

A named authorization capability, such as reading topology, observing terminal
content, sending input, spawning a process, changing layout, or terminating a
process.

### Resource selector

A policy entry identifying the Lair or an exact Dojo, Window, or Splint. Dojo
and Window selectors snapshot their existing descendants when the policy is
published; they do not automatically authorize future descendants.

### Trusted UI

Application-controlled Splinterm chrome rendered separately from terminal
content. Consent, authority indicators, and control-transfer decisions use this
surface so terminal output cannot spoof them.

## Automation and remote access

### Automation client

A script, editor integration, CLI machine-mode process, relay, or MCP adapter
using Splinterm's supported structured operations. Socket access and Unix user
identity alone do not authorize it.

### JSON/NDJSON contract

The stable machine-readable CLI interface. JSON represents one-shot operations;
NDJSON represents bounded subscriptions and event streams.

### Relay

The dedicated `splinterm-relay` SSH stdio transport. It connects remote callers
to the local daemon without making `splinterd` a network service and receives
only the authority assigned to its exact executable identity.

### MCP adapter

The `splinterm-mcp` stdio server that exposes supported Splinterm
automation operations as MCP tools and resources. It remains subject to normal
daemon policy, resource, controller, confirmation, and audit checks.

### In-Splint context

The `SPLINTERM_DOJO_ID`, `SPLINTERM_WINDOW_ID`, `SPLINTERM_SPLINT_ID`, and
`SPLINTERM_SPLINT_INCARNATION` values injected into a Splint's child process.
They are discovery hints, not credentials or authority.
