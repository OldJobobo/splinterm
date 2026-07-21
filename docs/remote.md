# SSH stdio relay

Splinterm supports remote automation through authenticated SSH without exposing a
TCP listener. The remote command `splinterm relay --stdio` replaces itself with
the dedicated `/usr/bin/splinterm-relay` transport. That process connects once
to the owner-only local daemon socket and copies opaque private-protocol bytes
between the socket and stdin/stdout.

The relay does not parse terminal content, frames, requests, responses, or
cancellation. It cannot mint authority or claim the SSH client's identity. The
remote client still negotiates the daemon protocol, owns request IDs, observes
revision/resynchronization rules, and sends cancellation frames itself.

## Security boundary

SSH authenticates the host and login account. It does **not** authorize a
Splinterm operation. `splinterd` sees and authorizes the exact local
`splinterm-relay` executable, not the remote machine, SSH key, command line, or
human identity.

A policy rule for the relay delegates its listed scopes, resources, and limits
to every caller who can invoke that relay under the account. Use a dedicated
account or an SSH forced-command/restricted-key configuration when callers must
not receive the account's other capabilities. Those SSH controls are
administrator-owned and are never installed or modified by Splinterm.

The helper has a distinct executable identity from the normal `splinterm` CLI.
Package upgrades change its digest and intentionally require explicit policy
review. Obtain the installed identity with trusted local tools:

```bash
realpath /usr/bin/splinterm-relay
sha256sum /usr/bin/splinterm-relay
```

A minimal read-only rule is:

```json
{
  "schema": "splinterm.policy.v1",
  "rules": [
    {
      "id": "ssh-topology-reader",
      "executable": {
        "path": "/usr/bin/splinterm-relay",
        "sha256": "REPLACE_WITH_REVIEWED_SHA256"
      },
      "scopes": ["topology_metadata_read"],
      "resources": [{"kind": "lair"}],
      "limits": {"max_results": 64, "deadline_ms": 5000}
    }
  ]
}
```

Create and validate the owner-only policy using the workflow in
[headless.md](headless.md), then request a service reload. Never grant a shell,
interpreter, writable relay copy, wildcard resource, or broader scope merely to
avoid maintaining exact policy.

## Connect through SSH

Pin and verify the server host key before unattended use. A fixed invocation is:

```bash
ssh -T \
  -o StrictHostKeyChecking=yes \
  -o UserKnownHostsFile="$HOME/.ssh/known_hosts" \
  ACCOUNT@HOST \
  /usr/bin/splinterm relay --stdio
```

`-T` is required: the relay rejects terminal stdin or stdout. Keep the remote
command fixed; do not interpolate terminal content or untrusted parameters into
an SSH command string. Relay stdout contains protocol bytes only. Bounded,
body-free diagnostics use stderr and must not be merged into stdout.

The relay requires an absolute normalized socket path. The socket must be a
non-symlink Unix socket owned by the account with mode `0600`, its immediate
canonical directory must be owner-only, the connected peer UID must match, and
a Linux 6.5+ `SO_PEERPIDFD` must bind the live peer to the exact opened
`splinterd` executable adjacent to the relay. The relay monitors that pidfd for
its complete connection lifetime. Unsafe `SPLINTERM_SOCKET` overrides and
same-UID substitute listeners fail closed
and are never used as fallback to another endpoint.

## Lifetime and failure behavior

Input EOF half-closes the daemon write direction and continues draining daemon
output. Daemon EOF, restart, socket failure, broken stdout, SSH death, or relay
death closes the relay connection promptly. The daemon then cancels that
connection's subscriptions and releases its controllers and pending transfers;
daemon-owned shells continue running.

A relay never reconnects automatically after daemon restart. Start a new SSH
relay and renegotiate explicitly. A user service normally ends at logout unless
an administrator enables lingering as described in [headless.md](headless.md).
Splinterm does not enable lingering, alter SSH policy, create keys, or install a
forced command.

The relay uses two fixed 16 KiB copy buffers plus bounded kernel pipe/socket
buffers. Backpressure stalls producers instead of creating an unbounded queue.
Malformed and oversized frames are forwarded unchanged; the daemon remains the
single protocol parser and rejects them under its normal frame limits.

Unix-socket forwarding is not a supported alternative in this slice. Its path
expansion, ownership, cleanup, and peer-identity behavior have not passed the
same review as the stdio relay.
