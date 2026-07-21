# ADR 0007: supported automation policy and audit boundary

- **Status:** Accepted
- **Date:** 2026-07-23
- **Plan:** [Headless access and supported automation](../plans/0006-phase4-headless-automation.md)
- **Supersedes:** the persistent-policy deferral in [ADR 0005](0005-trusted-consent-broker.md)

## Context

Protocol v18 authenticates local Unix-socket peers and has scoped, incarnation-
bound grant-once consent for terminal access. It does not authorize topology
reads, process creation, restore, layout mutation, or audit inspection. The
current executable device/inode binding is suitable for one daemon lifetime but
not for persistent policy: replacement changes the inode, a path alone can be
replaced, and a basename, argv, client label, or same UID is not an identity.

Headless hosts cannot display the trusted consent client. Remote clients also
cannot be identified through a stdio relay: `splinterd` sees the local relay
process. Supported automation therefore needs an explicit persistent policy,
an exhaustive operation matrix, and inspectable audit metadata without turning
transport authentication into authorization.

## Decision

### Canonical executable identity

A persistent rule identifies one executable with both:

- an absolute, normalized path with no `.` or `..` components; and
- the SHA-256 digest of the exact regular executable file opened for the peer.

At connection acceptance the daemon obtains `SO_PEERCRED` and the Linux 6.5+
`SO_PEERPIDFD`, opens the peer's `/proc/<pid>/exe` target read-only and
close-on-exec, and derives path, device, inode, owner, mode, size, and digest from
that open file. It rejects a missing, non-regular, oversized, unreadable, or
changed-while-hashing executable. The open descriptor, not a later path lookup,
is authoritative.

[Spike 0020](../spikes/0020-persistent-executable-identity.md) established that
the pidfd must identify the same PID, remain alive across the bounded hash, and
be monitored for the connection lifetime. Peer exit closes the connection even
if its socket descriptor was passed elsewhere. Persistent-policy authorization
fails closed when `SO_PEERPIDFD` is unavailable; it never falls back to a reused
numeric PID. Hashing runs outside daemon locks on a bounded blocking worker.

Path and digest must both match. Device/inode remain useful connection audit
metadata but are not persisted as upgrade-stable identity. Basename, argv,
process title, environment, client-provided labels, parent process, and same UID
never contribute authority.

Package or executable replacement does not inherit authority. A newly opened
binary with a new digest is denied until the user explicitly updates and
validates the rule. Existing connections retain their accepted identity
snapshot, but every request is evaluated against the current policy generation;
removing the old digest revokes its subscriptions and controllers. A policy may
list a small bounded set of explicit digests during a staged upgrade. Wildcards,
directory trust, signer inheritance, and “current file at this path” matching
are excluded from v1.

This boundary reduces ambient authority; it cannot contain an already
compromised account. A malicious process with the user's full filesystem and
service-control authority can replace an owner-writable policy or arrange a
daemon restart. Operators needing protection from the service account itself
must provision a policy the account cannot modify using an external control
such as root ownership or MAC. Documentation must state this residual risk.

### Policy file and matching

The policy is a bounded, versioned document loaded from an explicitly configured
path. No policy file means no persistent third-party grants. The daemon opens
each path component without following symlinks and accepts only a regular file
owned by the daemon UID, mode `0600`, no hard links, and at most 256 KiB. A
separately administrator-provisioned read-only file may be root-owned, must not
be group/world writable, and follows the same no-symlink and size rules.

Rules contain a unique bounded ID, canonical executable identity, expiry,
closed operation scopes, exact resource selectors, and operation-specific
limits. The default scope is `topology_metadata_read`; no wildcard scope exists.
Exact Splint selectors may bind an incarnation or explicitly select the current
incarnation. Dojo/window selectors expand once to an explicit bounded Splint set
at authorization time. They do not include resources created later. A rule that
opts into future resources is deferred beyond policy v1.

Limits include, where applicable, maximum returned rows/results/bytes, maximum
live subscriptions, maximum spawn count, and deadline. Request protocol bounds
remain hard ceilings. A rule can only narrow them.

Reload parses and validates a complete candidate before publication. Invalid,
unsafe, expired, unreadable, or oversized policy produces a bounded diagnostic
and installs a deny-all persistent-policy generation; it never retains a
possibly broader old interpretation. Publication is atomic. Requests already
executing keep the authorization decision and limits captured at start; reload
cannot broaden them. Removal or narrowing cancels now-unauthorized
subscriptions, revokes affected controllers, and denies later requests.

Policy validation and reload are local owner administration actions, not an
ordinary daemon RPC. The CLI may validate a file offline and request service
reload through the service manager, but it does not rewrite policy or bypass
normal filesystem controls.

### Consent, first-party UI, and relay

Graphical grant-once consent remains available and daemon-lifetime-only. If the
trusted consent surface is unavailable, a request without a matching policy
fails closed. Headless operation never enables the development bypass.

The matching packaged `splinterm` UI retains only the narrow implicit behavior
accepted in ADR 0005. Protocol v18 also requires that connection to declare the
`trusted_ui` handshake role. JSON/NDJSON connections declare `automation`, so
sharing the same executable inode cannot activate the graphical bypass. Public
automation modes, editor clients, relays, and MCP adapters are third parties even
when shipped together and require policy or interactive consent as the matrix
allows.

`splinterm relay --stdio` is authorized as its own exact executable identity.
SSH authenticates the host and login, not Splinterm operations. Because the
daemon sees the relay rather than the remote origin, a relay rule delegates its
listed resources, scopes, and limits to every caller able to invoke that exact
relay under the account. The relay cannot mint, forward, or claim a remote
identity. This delegation must be conspicuous in validation output and remote
access documentation.

### Closed operation scopes

Policy v1 uses these closed scopes:

- `topology_metadata_read` and `topology_subscribe`;
- `terminal_visible_read`, `terminal_subscribe`, `scrollback_read`, and
  `scrollback_search`;
- `controller_acquire`, `controller_transfer`, `input`, and `resize`;
- `process_spawn`, `process_restore`, and `process_terminate`;
- `topology_layout_mutate` and `topology_name_mutate`;
- `authorization_inspect`, `authorization_revoke`, and `audit_inspect`.

Clipboard scopes remain reserved internal consent scopes until a daemon
clipboard operation exists. Forced controller takeover is trusted-UI-only in
policy v1 and cannot be granted persistently. Policy administration is not a
socket scope. New requests and new sensitive behavior default to unauthorized
until this ADR's matrix is revised.

### Protocol v18 operation matrix

The public CLI is a composition of these protocol operations. Commands that need
parent IDs or a topology CAS first issue `InspectTopology`, so their CLI policy
rules also require `topology_metadata_read` with a Lair resource selector.
Terminal/control subscriptions use `InspectSplint` and require
`topology_metadata_read` for that Splint. These compound prerequisites are
intentional and visible rather than inherited from the trusted-UI bypass.

`own connection` means an unforgeable daemon-owned subscription, transfer, or
controller identifier created for that connection. Resource selectors are
checked in addition to every listed scope.

| v18 request | Required policy scope or authority |
| --- | --- |
| `Ping` | authenticated local peer; no resource data |
| `ListDojos` | `topology_metadata_read` |
| `InspectTopology` | `topology_metadata_read` |
| `SubscribeTopology` | `topology_subscribe` plus `topology_metadata_read` |
| `InspectSplint` | `topology_metadata_read` for the Splint |
| `RequestAccess` | graphical consent or an exact policy rule for every requested operation scope |
| `AuthorizationStatus` | `authorization_inspect` for the Splint |
| `RevokeAccess` | `authorization_revoke` for the grant's resource |
| `CreateDojo` | `process_spawn` and `topology_layout_mutate`; creation limit |
| `SplitSplint` | `process_spawn` and `topology_layout_mutate` for target; creation limit |
| `RelaunchSplint` | `process_spawn` for the exact Splint |
| `RestoreSplint` | `process_restore` for the exact Splint |
| `RestoreWindow` | `process_restore` for every expanded Splint |
| `RestoreDojo` | `process_restore` for every expanded Splint |
| `CloseSplint` | `topology_layout_mutate`; live process additionally requires `process_terminate` |
| `SetSplitRatio` | `topology_layout_mutate` for target |
| `NewWindow` | `process_spawn` and `topology_layout_mutate` for Dojo; creation limit |
| `CloseWindow` | `topology_layout_mutate` for every expanded Splint; each live process additionally requires `process_terminate` |
| `RenameDojo` | `topology_name_mutate` for Dojo |
| `RenameWindow` | `topology_name_mutate` for window |
| `SetWindowDefaultFocus` | `topology_layout_mutate` for window and Splint |
| `RenameSplint` | `topology_name_mutate` for Splint |
| `Attach` | `terminal_visible_read` and `terminal_subscribe`; also `scrollback_read` when rows are requested |
| `ScrollbackPage` | `terminal_visible_read` and `scrollback_read` |
| `SearchScrollback` | `terminal_visible_read`, `scrollback_read`, and `scrollback_search` |
| `AcquireControl` | `controller_acquire` plus at least one of `input` or `resize` |
| `SubscribeControl` | `terminal_visible_read` for Splint |
| `RequestControlTransfer` | `controller_transfer` plus at least one of `input` or `resize` |
| `DecideControlTransfer` | own pending transfer as current controller; no additional scope |
| `ForceControlTransfer` | trusted graphical UI confirmation only; no persistent policy scope |
| `ReleaseControl` | own controller; no additional scope |
| `Input` | own controller and `input` |
| `Resize` | own controller and `resize` |
| `Detach` | own subscription; no additional scope |
| `KillSplint` | `process_terminate` for exact Splint/incarnation |
| `AuditInspect` | `audit_inspect` for daemon-lifetime bounded metadata |

Audit inspection is paginated and requires `audit_inspect`.
Cancellation is connection-local and needs no separate scope, but can cancel
only the caller's request or subscription.

### Audit contract and retention

The daemon assigns monotonic, nonzero audit IDs for its lifetime and retains the
newest 1,024 records in memory. Inspection is cursor-based and page-bounded; a
cursor older than retention returns an explicit gap and the oldest available
ID. IDs and records restart with the daemon and the API is labeled
`daemon_lifetime`. Durable audit is deferred until retention, rotation, crash
recovery, and privacy can be evaluated independently.

Records contain timestamp, audit ID, policy generation/rule ID when applicable,
peer UID, bounded executable path and digest, operation, resource IDs and
incarnation, requested scopes, decision, bounded symbolic reason, and bounded
outcome metadata. They never contain terminal rows or bytes, scrollback or
clipboard bodies, input bytes, capability tokens, environment contents, search
queries, or complete command arguments. Spawn/restore records may contain an
argument count and redacted executable basename only.

Record authorization grants, denials, revocations, expiry, policy match/reject,
controller transfer, spawn/restore, topology mutation, termination, and policy
reload outcome. Audit insertion is ordered by the daemon and occurs without
holding policy, topology, PTY, protocol-writer, or consent locks. Audit loss or
inspection backpressure cannot block PTY consumption; a retention gap is
reported rather than hidden.

## Threat-model conclusions

- Copied executables fail the canonical-path check; replaced executables fail the
  digest check. Writable policy/account compromise remains an explicit residual
  risk rather than a false promise.
- Symlink, path-component, hard-link, ownership, mode, size, parse, and race
  failures deny the entire persistent-policy generation.
- Stale rules fail on expiry, missing resources, incarnation mismatch, or policy
  generation change. Selector expansion is bounded and snapshot-based.
- Relay impersonation gains nothing unless it matches the exact relay path and
  digest; callers who can invoke the authorized relay receive its explicitly
  delegated authority by design.
- SSH disconnect, relay EOF, or client death cancels subscriptions and releases
  controllers without killing daemon-owned processes.
- Terminal prompt injection remains untrusted returned data. It cannot approve
  consent, alter policy, create a scope, or become an audit reason.
- Frames, policy, selectors, streams, pages, queues, relay buffers, and audit are
  bounded before allocation. Slow consumers receive resynchronization or are
  disconnected.
- Request IDs, stable resource IDs, incarnation, revisions, ownership, policy
  generation, and cursors prevent replay from silently targeting new state.

## Consequences

- Headless access is possible only after an explicit least-privileged policy is
  installed; missing graphical consent otherwise denies access.
- Package upgrades intentionally require policy digest review instead of
  silently transferring authority to replacement code.
- Relay and adapter identities remain disposable clients with no bypass.
- The first implementation must add scopes and authorization checks before
  exposing machine CLI contracts.
- Audit inspection is useful but intentionally not durable in policy v1.
- The executable snapshot and policy-open algorithms require adversarial tests
  before implementation; failure to make either race-safe is a stop gate.
