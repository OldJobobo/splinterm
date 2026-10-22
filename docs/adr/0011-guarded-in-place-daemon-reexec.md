# ADR 0011: guarded in-place daemon re-exec for compatible upgrades

- **Status:** Accepted for `0.2.0`; implementation pending; remembered agent grants across upgrade amended by [ADR 0014](0014-easy-agent-access.md) (amendment below)
- **Date:** 2026-08-14

## Context

Splinterm keeps daemon-owned shells and terminal state alive when graphical
clients disconnect, but replacing the installed package does not replace an
already-running `splinterd` image. Restarting the daemon ends its child
processes and kernel PTYs. Starting an unrelated replacement daemon also cannot
recover the old daemon's ordinary child-reaping authority.

The upgrade contract must preserve exact process and PTY identity without
claiming continuity across daemon crashes, logout, reboot, host loss, or an
incompatible build. It must also preserve the adjacent daemon/client executable
identity used by trusted graphical clients and must never let package scriptlets
restart arbitrary user services.

## Decision

### Compatibility and initiation

A handoff is compatible only when the running and installed daemons explicitly
negotiate overlapping private protocol, handoff protocol, terminal-checkpoint,
and descriptor-manifest schema ranges. Membership in the `0.2.x` release series
alone is not a compatibility promise.

On the next human launcher invocation, a fully compatible handoff occurs
automatically after bounded preflight, including when live Splints exist. A
package scriptlet never initiates it. After the `0.1.x` bootstrap boundary, an
idle daemon may restart automatically. An incompatible daemon with live Splints
is blocked until the user explicitly
confirms a destructive restart that reports the exact affected Splint count.
The first `0.1.x` to handoff-capable `0.2.0` transition necessarily uses that
one confirmed bootstrap restart because the running `0.1.x` daemon cannot adopt
a protocol retroactively.

### Process and descriptor ownership

The running daemon preserves its PID and child-parent relationship by replacing
itself through descriptor-based `execveat(..., AT_EMPTY_PATH)`. Path lookup is
not repeated after preflight, but an open regular-file descriptor is not treated
as immutable: a privileged in-place write could change the bytes behind it.

After no-follow, owner, mode, package-adjacency, source device/inode, and digest
validation, the old generation copies each forward daemon/client image into a
separate memfd created with `MFD_ALLOW_SEALING | MFD_EXEC`, rehashes the complete
copy, and applies
`F_SEAL_WRITE | F_SEAL_GROW | F_SEAL_SHRINK | F_SEAL_SEAL`. It verifies the
complete seal set before compatibility preflight or quiescence and executes only
those sealed snapshots. Source metadata and the sealed snapshot's device, inode,
digest, size, and seal set are recorded distinctly. Source mutation, truncation,
replacement, or deletion after sealing cannot change the executed bytes.

At startup, the running generation similarly creates and retains a mutually
compatible sealed rollback snapshot pair from `/proc/self/exe` and its validated
adjacent client, before package paths can change. If either
forward or rollback image cannot be copied, rehashed, sealed, or executed from
its immutable snapshot, handoff is blocked. An ordinary writable package-file
descriptor is never a forward, rollback, or trusted-client execution authority.

`splinterm-pty` must expose an adoptable Linux session representation based on
validated child PID, process group, session identity, and an explicit owned PTY
descriptor set. Adoption must retain direct child reaping, foreground/process-
group signaling, resize, and ordered bidirectional I/O. Exactly one userspace
reader may own each PTY stream before and after the handoff; accidental adoption
of both a master and an independent cloned reader is forbidden.

The listening socket descriptor survives exec and is re-registered by the new
generation. Existing accepted protocol connections, automation sessions, consent
grants, subscriptions, transfer requests, and audit cursors do not survive.
Before exec, the old daemon creates at most one explicitly allowlisted handoff-
continuation socketpair for each eligible trusted local Window. The
daemon opens and retains a pidfd for the already-authenticated client process,
groups only old connections proven to originate from that same process, enables
per-message credentials on the continuation channel, and carries the pidfd plus
its expected process/executable identity through daemon exec. This is not a
surviving protocol connection: its closed contract carries only the final
adopted-or-rolled-back outcome and, after adoption, one resume ticket. It carries
no terminal, input, mutation, subscription, or general authentication traffic.
Unknown or unclaimed inherited descriptors are closed before admission reopens.

### Handoff and rollback boundary

The old generation remains authoritative while it:

1. validates the exact forward and rollback source pairs and materializes their
   complete sealed executable snapshots;
2. fences structural mutation, controller transfer, resize, and new input;
3. linearizes each terminal actor's PTY reads, writes, replies, resize, parser,
   publication, and child-exit observation;
4. captures one bounded canonical checkpoint and descriptor manifest; and
5. completes every pre-exec operation that can fail.

The candidate validates and stages every terminal, PTY, listener, policy, and
publication object before reading a post-checkpoint PTY byte. The irreversible
adoption commit occurs immediately before that first read. Before the commit, a
handled adoption failure may execute the sealed rollback snapshot with the
unchanged descriptors and rollback manifest. After the commit, restoring the
older checkpoint is forbidden because doing so could duplicate or reorder PTY
bytes. A candidate crash, kill, or hang outside the cooperative pre-read
rollback path is a daemon crash and carries no `0.2.0` live-continuity promise.

### Smooth local client recovery

During handoff, existing Windows show trusted application chrome stating that
Splinterm is upgrading and input is paused. Input is not queued into the PTY or
silently redirected while authority is fenced.

Before exec, each eligible trusted local Window receives the sealed forward
client snapshot descriptor and its client end of the bounded continuation
channel through its authenticated old connection. The Window also creates one anonymous
sealed, bounded resume record for its client-local state: its ordered set of at
most 32 open Dojo IDs, active tab, focused Splint per open Dojo, and the mapping
from those Dojos to the exact old connections. The record contains no terminal,
scrollback, clipboard, selection, search-query, IME, input, environment, consent,
or credential body and grants no authority; every ID remains a hint that the new
client must validate against its authoritative resnapshot. Selection, search,
IME composition, and open transient overlays cancel rather than cross exec.

The Window holds the sealed executable snapshot descriptor, continuation
endpoint, and resume-record FD close-on-exec, displays the input-paused state,
and performs no path lookup. Cooperative rollback tells it to close all three artifacts and resume or
reconnect to the old generation.

Successful adoption tells the Window to close the stale UI connection and
relaunch through the sealed snapshot descriptor. Immediately before that client
exec, it clears `FD_CLOEXEC` only on fixed continuation-channel and resume-record
slots; the sealed executable descriptor remains close-on-exec after serving as
the `execveat` target. The replacement client validates and closes the resume
record, recreates the ordered tabs and per-Dojo connections, and performs full
resnapshots before restoring the active tab and focused pane. It closes every
inherited artifact on an unexpected slot, schema, bound, ID, message, peer,
timeout, or reconnect failure.

After the replacement client establishes trusted identity and completes those
resnapshots, it uses the inherited continuation channel to obtain one bounded,
single-use, generation-bound resume ticket. Every continuation message carries
kernel-supplied sender credentials. The adopted daemon requires the sender PID
to be the same still-live process tracked by the inherited pidfd, requires the
replacement ordinary connections to come from that process, and revalidates that
`/proc/<pid>/exe` has the exact device/inode and digest identity of the sealed
forward client snapshot before issuing or accepting a claim. Passing the
endpoint to another process therefore cannot transfer the claim.

The ticket is bound to that process identity, exact old connection set, new
daemon generation, immutable client-snapshot identity, and pre-fence controlled
Splint incarnations. It expires after a short fixed deadline, cannot be
persisted, and cannot restore control until the replacement client's ordinary
trusted connections, validated Window resume record, and completed resnapshots are
correlated.

A successful claim restores only the prior local human controller disposition;
it does not preserve the old connection ID, consent, policy, automation,
subscription, remote, or audit authority. If the claim is invalid, expired, or
conflicts with a valid new owner, the pane remains visibly view-only and offers
the existing Request Control workflow. The normal successful path restores
input to the previously active pane without a click, tab switch, or manual
refocus.

Remote graphical clients reconnect and reauthenticate without a local resume
ticket. Remote package administration remains an operator-owned SSH workflow,
not part of this ADR.

## Consequences

- Compatible upgrades can preserve the same shell PIDs, PTYs, terminal
  incarnations, and daemon PID while replacing daemon code.
- Compatibility is explicit and negotiated; an individual `0.2.x` upgrade may
  still be blocked rather than silently destroying work.
- The first `0.1.x` to `0.2.0` transition remains a clearly reported destructive
  boundary.
- The local interactive path resumes smoothly, but stale connection authority
  still fails closed.
- The checkpoint, continuation channel, client pidfd, sealed client-snapshot
  descriptor, Window resume record, and resume ticket are internal, bounded,
  generation-specific contracts rather than public automation APIs.
- In-place re-exec narrows but cannot eliminate the post-exec crash interval.
  Crash continuity would require a separately approved persistent PTY broker or
  subreaper architecture.

## Validation requirements

Implementation must proceed through bounded dependency-ordered milestones. At
minimum, retained evidence must prove:

- shell PID, session, process group, PTY identity, reaping, signaling, resize,
  one-reader ownership, and byte ordering survive forward adoption and rollback;
- unsupported, truncated, duplicate, mismatched, or oversized manifests fail
  before publication;
- replacing, deleting, truncating, or rewriting either source pathname after
  snapshot sealing cannot change the exact executable pair used for handoff or
  trusted local relaunch, while every write/grow/shrink/seal-change attempt
  against an executable snapshot fails;
- every pre-exec failure returns to the unchanged old generation;
- every handled post-exec failure before the first new PTY read rolls back
  without duplicated or lost bytes;
- old clients cannot input, resize, mutate, or reclaim authority after fencing;
- continuation channels reject unexpected inherited slots, sender credentials,
  pidfd/process/executable mismatches, excess messages or descriptors, replay,
  and every operation outside adopted-or-rolled-back outcome delivery and one
  ticket, then close after claim or expiry;
- Window resume records reject unsupported schemas, excess tabs/connections,
  unknown or mismatched IDs, bodies outside the closed field set, corruption,
  replay, and cross-Window use; rollback, expiry, crash cleanup, multi-Window,
  and multi-connection tests preserve no body-bearing artifact and restore only
  each Window's ordered tabs, active tab, focus, and prior controller map;
- a valid local resume ticket restores the active pane without user action,
  while invalid, replayed, transferred, remote, stale, or conflicting tickets
  fail closed; and
- package, launcher, rollback, downgrade, and interrupted-upgrade paths never
  silently terminate active Splints or continue an unsupported mixed generation.

## Implementation note: the control claim (Plan 0061 M3.16, 2026-10-20)

The resume-ticket claim is implemented in two steps, because a Window holds
one control connection per pane and a controller lease is bound to exactly
one connection:

1. **One continuation claim per Window.** After an adoption, the relaunched
   Window, executing the sealed forward client snapshot, sends one fixed-size
   claim on its continuation channel naming the generation it reached. The
   adopted daemon requires kernel `SCM_CREDENTIALS` (a sender without
   `CAP_SYS_ADMIN` cannot name another PID; the kernel refuses it with
   `EPERM`) from the daemon's UID and the held Window's PID, the pinned
   pidfd still alive with the recorded start time, and `/proc/<pid>/exe` at
   the pinned snapshot's device and inode. That executable check is by
   device and inode only, never by content hash (AGENTS.md §4.4, the
   2026-10-18 exception), where the decision above speaks of
   "device/inode and digest identity": the pinned inode is the sealed
   snapshot the candidate verified against its package record, and its
   bytes cannot change, so a copy with the same digest is refused. Kernel
   credentials naming PID 0 (a sender outside the daemon's PID namespace)
   are refused. It answers with one grant carrying
   a 256-bit ticket from `getrandom` and closes the channel. Any refusal
   closes the channel with nothing sent and drops that Window and its
   placeholders. A rollback daemon pins no snapshot and issues no ticket.
2. **One redemption per pane.** Each pane's new control connection sends
   `ClaimHandoffControl { ticket, splint }`, naming its Splint and the
   incarnation it attached to (an optional field within protocol 42). The
   daemon repeats the process checks against that connection's peer, then
   restores the named Splint's controller only when the Window held it at
   the fence, the named, recorded, and live incarnations agree, and no
   controller holds it now. The lease is connection-bound and never
   grant-bound; consent rules are unchanged. Anything else is skipped and
   answered with no controller, never forced.

The ticket is held only in the daemon's memory, compared in full, bound to
its Window's process, the snapshot identity, and the generation, and ends with
the 30 s hold. It is single-use per held controller: each placeholder is
restored at most once, a second presentation for the same Splint is a replay,
and a presentation from any other process ends the ticket. Every step, the
continuation claim, each redemption, every refusal, and the release at
expiry, runs under the hold's one mutex, so a claim either completes before
the release or finds nothing. A Window that never claims loses its
placeholders at expiry, and its panes acquire control the ordinary way.

## Amendment: remembered agent grants (ADR 0014)

[ADR 0014](0014-easy-agent-access.md), accepted for `0.2.0` and implemented on
`main`, adds remembered agent grants: "Always for this workspace" answers
kept in a daemon-owned store. They are durable user state, like policy, not
handoff state, and this ADR treats them as follows:

- They are not written into the checkpoint, descriptor manifest, continuation
  channel, or resume record.
- An adopted generation, and a rolled-back one, loads the store fresh from disk
  through the ordinary validating loader before admission reopens. A store
  that fails validation denies remembered authority in that generation and
  does not block or roll back the handoff. A generation that does not support
  the store's version treats it as unavailable and never rewrites it.
- Accepted connections, automation sessions, agent controller leases,
  just-this-once grants, "while this workspace is open" grants, and pending
  consent requests end at the fence, as the decision above requires for
  consent grants. A "while this workspace is open" grant ends with its daemon
  generation even though the Lair survives.
- An agent's new connection to the adopted generation is checked against the
  remembered identities again. Loading a definition never restores an old
  connection's authority, and the resume ticket never restores an agent's
  controller.
