# ADR 0010: Multiplex native remote channels over one authenticated SSH process

- **Status:** Accepted — Phase 1 transport foundation implemented; graphical workflow routing remains Plan 0028 Phase 2
- **Date:** 2026-08-07
- **Plan:** [Plan 0028](../plans/0028-remote-graphical-client.md)
- **Input:** [Remote graphical client handoff](../remote-graphical-client-handoff.md)

## Context

A native Splinterm Window uses independent daemon connections for topology,
terminal observation, control, and pane tasks. Opening one SSH process for every
connection would repeat password or passphrase authentication. Depending on
OpenSSH ControlMaster would still consume one server SSH session per daemon
connection and would make supported Window topology depend on `MaxSessions`.

The product goal is the Splinterm equivalent of:

```text
local Foot -> SSH -> remote tmux -> persistent remote sessions
```

but rendered as one local native Splinterm Window attached directly to the
remote daemon-owned Lairs, Dojos, and Splints. `splinterd` must not gain a
network listener, and an SSH caller must not gain trusted-UI identity.

## Decision

Use one directly spawned OpenSSH child per remote-client lifetime. Its fixed
remote command is:

```text
/usr/bin/splinterm relay --graphical-stdio
```

That mode negotiates a separate bounded outer protocol and multiplexes a hard
maximum derived from 256 supported Splints, two retained channels per Splint,
and eight fixed/transient Window service channels. Channel IDs are nonzero,
monotonically allocated, and never reused. Every accepted channel opens
and repeats the existing owner, mode, UID, pidfd, and exact adjacent
`splinterd` executable validation before carrying opaque private-protocol bytes.
The daemon sees the installed `splinterm-relay` executable and every remote
channel negotiates `ClientRole::Automation`.

The outer protocol has exact magic and version plus `Hello`, `HelloAck`,
`OpenChannel`, `ChannelOpened`, `ChannelRejected`, `Data`, `HalfClose`,
`CloseChannel`, and `SessionError` frames. Data producers reserve shared and
per-channel byte permits before reading; permits survive through physical write.
A central round-robin scheduler services one frame per ready channel while a
separate bounded control queue prevents data from starving session control.
Per-channel drain barriers preserve data-before-half-close/close ordering. A
client close may cross a channel-local daemon EOF after the relay has retired the
same monotonically issued ID; that close is idempotent, while data, half-close,
or close for a future never-issued ID remains session-fatal. Data frames, queues,
diagnostics, and channel counts are bounded. Corrupt outer framing and
aggregate-bound violations fail the session; ordinary daemon EOF is
channel-local unless the validated daemon process exits.

The existing command remains byte-transparent and unchanged:

```text
/usr/bin/splinterm relay --stdio
```

`Connection` accepts split async reader/writer transports while preserving its
local Unix constructors, frame/request/event bounds, cancellation behavior, and
trusted local image-content path. A remote connection has no image socket path
and cannot negotiate trusted image retrieval.

OpenSSH owns keys, agents, certificates, hardware providers, passwords,
passphrases, host-key verification, aliases, and supported proxy routing.
Splinterm supplies structured argv, fixed safety overrides, bounded stderr
capture, standard `SSH_ASKPASS` validation, child cleanup, and categorized
errors. It stores no credentials and performs no host-key enrollment.

## Consequences

Password and passphrase users authenticate once for the complete remote session,
not once per pane. Multiple daemon connections remain independent without
consuming additional SSH sessions. Dropping the final session/channel owner
closes stdin, permits a short graceful exit, then terminates and reaps a stuck
child. Remote daemon-owned Splints are not terminated.

Persistent Splinterm policy remains a separate authorization boundary. A policy
for `/usr/bin/splinterm-relay` delegates its scopes to every SSH caller able to
execute that relay under the account. Dedicated accounts or administrator-owned
forced commands/restricted keys are recommended when that delegation is too
broad.

Phase 1 provides profiles, inspection, non-mutating `remote check`, transport,
automation channels, and an explicit endpoint capability factory. Routing native
Windows, remote-safe graphical mutations, recency namespaces, focus suppression,
and complete remote UI behavior remain Phase 2 and must not be described as
implemented yet.

## Rejected alternatives

- One SSH process per daemon connection: repeats authentication and scales poorly.
- OpenSSH ControlMaster as the application multiplexer: remains constrained by
  server SSH-channel policy and `MaxSessions`.
- A `splinterd` TCP listener: enlarges the daemon authentication and network
  attack surface.
- A custom SSH implementation or credential cache: duplicates established SSH
  security authority and creates secret-storage obligations.
- Trusted-UI negotiation through SSH: breaks the local executable/compositor
  trust boundary.
- Changing raw `relay --stdio`: risks existing automation compatibility.

## Validation

Phase 1 validation covers exact raw-relay compatibility, outer framing under
fragmentation and corruption, channel limits, queue backpressure, half-close,
channel/session EOF, daemon pidfd exit, anti-starvation behavior, split transport
handshake/cancellation, no remote image transport, exact fake-SSH argv, one fake
SSH process serving simultaneous daemon connections, child cleanup, strict
profiles, and read-only `remote check`.
