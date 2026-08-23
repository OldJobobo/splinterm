# SSH stdio relay

Splinterm supports remote automation through authenticated SSH without exposing a
TCP listener. The remote command `splinterm relay --stdio` replaces itself with
the dedicated `/usr/bin/splinterm-relay` transport. That process connects once
to the owner-only local daemon socket and copies opaque private-protocol bytes
between the socket and stdin/stdout.

The relay does not parse terminal content, frames, requests, responses, or
cancellation. It cannot mint authority or claim the SSH client's identity. The
remote client still negotiates the daemon protocol, owns request IDs, observes
revision/resynchronization rules, and sends cancellation frames itself. Image
pixel bodies remain restricted to the executable-verified trusted local UI and
its separate local content channel; public relay automation does not receive
those bodies or turn terminal-supplied Kitty paths/SHM names into host access.
See [images.md](images.md).

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
  "schema": "splinterm.policy.v2",
  "rules": [
    {
      "id": "ssh-topology-reader",
      "executable": {
        "path": "/usr/bin/splinterm-relay",
        "sha256": "REPLACE_WITH_REVIEWED_SHA256"
      },
      "scopes": ["topology_metadata_read"],
      "resources": [{"kind": "daemon"}],
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

## Native graphical client

The native remote Window uses a distinct transport:

```text
/usr/bin/splinterm relay --graphical-stdio
```

This mode is not byte-compatible with `--stdio`. It negotiates an exact bounded
outer protocol and maps independent logical channels to independently validated
daemon Unix connections. One local OpenSSH child therefore carries topology,
observation, control, and pane-task connections after one authentication. It
does not depend on OpenSSH ControlMaster or server `MaxSessions`.

Every logical daemon connection negotiates `ClientRole::RemoteInteractive`.
OpenSSH authenticates the human user and starts the installed graphical relay
under that remote Unix account; native remote Windows do not use automation
policy. The daemon accepts this role only from the adjacent
`splinterm-relay --graphical-stdio` process. It grants ordinary human terminal
and topology authority while withholding trusted-local compositor focus, image
content, and forced-control privileges. Closing SSH or the local session
releases connection-owned subscriptions and controllers but sends no Splint
termination request. Raw `relay --stdio` automation remains byte-compatible and
continues to negotiate `ClientRole::Automation` under persistent policy.

Select one profile globally to bind the complete native client lifetime to it:

```text
splinterm --remote PROFILE
splinterm --remote PROFILE dojos
splinterm --remote PROFILE reopen
splinterm --remote PROFILE window --lair-id LAIR_ID --dojo-id DOJO_ID
splinterm --remote PROFILE launch [--working-directory REMOTE_PATH] [-- ARGV...]
```

Omitting the subcommand after `--remote PROFILE` opens that endpoint's Recent
Dojo picker. Dojo discovery, tabs, pane snapshots and ordered updates,
resynchronization, scrollback/search, ordinary requested control/input/resize,
and ordinary lifecycle actions all use logical channels on the same SSH child.
New Lairs, Dojos, and Splints may be created, attached, controlled, and rendered
immediately without policy publication or Window restart. Trusted forced control
transfer remains visibly unavailable and is rejected before request construction.
A Window remains bound to exactly one endpoint. Local and remote recency files
are distinct, and profile names—not remote titles or CWDs—select the namespace.

Remote creation uses the existing remote-safe launch envelope: absent CWD and
argv cause the remote daemon to select its own home, shell, and defaults.
Split/relaunch inheritance is resolved by the remote daemon from the exact target
Splint. Because remote creation and attachment require several bounded protocol
round trips, a split immediately renders a noninteractive `Opening remote pane…`
placeholder before dispatching the mutation. One split may be pending per Dojo
tab. The client-local placeholder cannot receive focus or send terminal/topology
requests, never crosses the relay, and is replaced by the exact authoritative
Splint after attachment. A rejected mutation or unavailable/full command queue
removes only that placeholder and restores the original pane focus.

An explicit `--cwd`/`--working-directory` is an absolute remote path and
structured argv is never rebuilt as a shell string. Local shell settings and
local CWD are never default remote launch state. The envelope's historical Rust
type name is not an authorization role; remote graphical connections remain
human-interactive.

Profile inspection and reachability commands remain non-graphical:

```text
splinterm remote list
splinterm remote inspect PROFILE
splinterm remote check PROFILE
```

`check` starts the fixed graphical relay, negotiates one remote-interactive
channel, sends `Ping` and `ListLairs`, and exits without mapping a Window or
mutating topology. A successful check proves SSH, relay, daemon, and human-role
reachability.

## Remote profiles

Profiles live at `${XDG_CONFIG_HOME:-~/.config}/splinterm/remotes.toml`.
`SPLINTERM_REMOTES` selects an explicit file for isolated testing. The schema is
strict and versioned:

```toml
version = 1

[remotes.wintermute]
host = "wintermute"
user = "operator"                    # optional
port = 22                            # optional
identity_files = ["~/.ssh/id_ed25519"]
known_hosts_file = "~/.ssh/known_hosts"
connect_timeout_seconds = 15
```

Unknown fields, unsupported versions, more than 64 profiles, unsafe names,
ambiguous host/user tokens, zero ports, timeouts outside 1–300 seconds, more
than eight identity files, documents above 64 KiB, and path values above 4 KiB
fail closed. Explicit paths must be absolute or begin with `~/`, name readable
regular files, and contain no whitespace or control characters. Profiles cannot
supply arbitrary SSH options, commands, forwarding, environment, or shell
fragments.

`~/.ssh/config` may still select ordinary aliases, identities, certificates, and
proxy routing. Splinterm's command-line safety options override conflicting
`RemoteCommand`, `LocalCommand`, terminal, stdin, forwarding, and host-key
settings. `remote inspect` prints resolved non-secret settings and structured
argv; it never reads or prints private-key contents.

## Authentication and host keys

Splinterm directly spawns the installed `ssh` executable with piped protocol
stdin/stdout and separate bounded stderr. It does not force batch mode. When a
controlling terminal exists, OpenSSH may use `/dev/tty` for password, key
passphrase, PIN, or hardware-token interaction.

Without a controlling terminal, an existing `SSH_ASKPASS` provider remains
OpenSSH's authority. Splinterm requires that value to name an absolute executable
local file before setting `SSH_ASKPASS_REQUIRE=force`. It never places a password
or passphrase in argv, TOML, relay frames, diagnostics, or application storage.
Agent-backed authentication can still succeed without a prompt provider;
interactive authentication failures are reported rather than replaced with an
application password field. Post-connect SSH authentication and relay
negotiation are bounded by the profile timeout plus a 120-second human/hardware
interaction allowance. Logical-channel admission, private daemon handshake, and
each `remote check` probe use the validated profile timeout.

`StrictHostKeyChecking=yes` is always supplied. Unknown and changed host keys
fail closed. Establish trust explicitly with normal OpenSSH tooling; Splinterm
does not run `ssh-keyscan`, append known-hosts files, or offer accept-anything
behavior.

Bounded errors distinguish host-key changes/unknown keys, routing or timeout,
authentication failure, unavailable terminal/askpass interaction, missing remote
command, unavailable daemon, relay/daemon identity rejection, outer protocol
mismatch, private protocol mismatch, and generic transport loss. SSH
configuration can still affect routing and authentication, so inspect it with
standard OpenSSH tools when diagnosing aliases or proxies.

## Human authority and automation policy

The native graphical workflow is a human SSH session. After OpenSSH authenticates
the account, the graphical relay receives normal terminal-multiplexer authority
from that account's `splinterd`: session discovery, attachment, input, resize,
creation, splitting, naming, closing, restoration, and reconnection do not
require policy rules. Newly created topology is usable immediately.

Persistent exact-resource policy remains exclusively for machine clients using
`relay --stdio`, JSON/NDJSON, or MCP. Its snapshot behavior does not constrain a
native remote Window. Policy reload disconnects automation-role connections so
they re-evaluate the new generation; it does not disconnect local or remote
human graphical clients.

Remote snapshots must omit image metadata. The client never opens a `.content`
socket for a remote endpoint; unexpected remote image metadata fails the pane
closed before any body request. `PublishGraphicalFocus` is never sent remotely.
SSH/relay/channel loss shuts down the affected local views and drops their
connections, releasing subscriptions and controller leases. It sends no kill,
close, restore, or other process-lifecycle request, so daemon-owned remote
Splints continue running.
