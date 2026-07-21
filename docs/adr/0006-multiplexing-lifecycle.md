# ADR 0006: persistent multiplexing lifecycle

- **Status:** Accepted
- **Date:** 2026-07-20
- **Plan:** [Persistent multiplexing](../plans/0004-phase3-multiplexing.md)

## Context

Phase 3 changes Splinterm from one daemon-owned terminal into a persistent local
multiplexer. Stable topology, disposable clients, concurrent observation, and
process relaunch must not weaken the existing daemon/client, authorization, or
Foot-parity boundaries.

A Splint's durable identity is not sufficient to identify a live process. A
relaunch keeps the Splint but replaces its PTY, process, terminal actor, grants,
and control lease. Structural edits also need an ordering domain independent of
terminal damage and scrollback revisions. Finally, metadata can survive daemon
loss, but kernel PTYs and process memory cannot.

## Decision

The daemon is the sole writer of topology, live-runtime state, and durable
metadata. It serializes each structural transaction and advances one monotonic
`TopologyRevision` exactly once for every committed structural or naming
change. Terminal revisions and history generations remain separate. Mutations
that carry a stale expected topology revision fail without partial changes.

Dojo, window, and Splint IDs are stable and independent of names and tree
positions. Every live Splint process has a nonzero incarnation. Relaunch retains
the Splint ID, allocates a new incarnation, and revokes all grants,
subscriptions, and control authority bound to the previous incarnation.

Control and terminal-size ownership are exclusive per Splint/incarnation, not
global. Different connections may control different Splints concurrently.
Observation, attach, focus, and reconnect never acquire or steal control.
Controller leases bind a daemon-owned connection ID. A bounded control
subscription reports subscriber-specific local/remote ownership without peer
identity. Transfer requests route only to the current owner, time out to denial,
and cancel when either participant disconnects. Acceptance atomically revokes
the old lease and grants a new requester lease; forced transfer requires a
separate trusted consent prompt. Keyboard focus, selection, viewport, search
query/cursor, and active pane are client-local.

Lifecycle operations are distinct:

- `KillSplint` ends the current process and retains the exited leaf and launch
  metadata.
- `CloseSplint` removes only an exited leaf and collapses its parent branch.
- Removing a live leaf requires an explicit kill-and-close operation.
- Closing a window's final leaf removes that window but retains its Dojo.
- `RelaunchSplint` starts the retained launch specification under a new
  incarnation.

Split ratios use a validated fixed integer unit from 1 through 999 out of 1000.
Names are trimmed, nonempty UTF-8, at most 128 bytes, with Dojo names unique
within the Lair. IDs remain the exact selector when titles are duplicated.

Durable state uses an owner-only, bounded, versioned schema written atomically.
It may contain topology, IDs, launch metadata, dimensions, exit metadata, and
opt-in relaunch intent. It never contains grants, controller tokens, PTY
handles, terminal grids, scrollback bodies, clipboard data, or a claim that a
process is still running. Startup loads every persisted leaf as exited and
restorable; executing a saved command always requires an explicit restore or
relaunch action.

Literal scrollback search is serialized by the terminal actor, scans retained
normal-screen rows without copying configured history, and applies hard query,
result, preview, cursor, and deadline bounds. Opaque continuation cursors are
valid only for the exact Splint/incarnation, terminal revision, and history
generation; output, trim, reflow, clear, resize, or relaunch forces restart.

No shared daemon lock may be held across PTY spawn, actor requests, process
shutdown, filesystem I/O, consent UI, or protocol writes. A split is reported
only after both spawn and topology insertion can commit; either-side failure
cleans up and leaves no addressable phantom leaf. A persistence failure cannot
be reported as a committed durable mutation.

## Consequences

- Concurrent clients can independently observe any Splint and can control
  different Splints without sharing focus or a global lease.
- Stable IDs support detach and later reattach, while incarnations make stale
  process authority fail explicitly.
- Topology compare-and-swap prevents lost structural updates without coupling
  topology ordering to terminal output.
- Kill, close, and relaunch have unambiguous process and tree effects.
- Daemon restart can restore validated metadata and permit explicit relaunch,
  but cannot claim process, PTY, exact terminal-state, or scrollback continuity.
- Structural transactions require coordination and bounded rollback paths, but
  PTY consumption remains isolated per live Splint.
- Graceful shutdown owns all connection tasks, drops nested writer/subscription
  tasks, then drains runtimes and persists once; a pinned signal future prevents
  aggregate SIGINT loss.
