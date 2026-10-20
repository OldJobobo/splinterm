# MCP adapter setup and security

`splinterm-mcp` is a local, bounded MCP stdio server supporting `2025-06-18`
and `2025-11-25` (preferred). It is an
optional split package and a separately authorized third-party client, not a
trusted part of `splinterd`. Installing it grants nothing: the daemon continues
to deny operations until the owner installs an exact executable path/digest
policy.

## Install identity and launch

The split Arch package installs only `/usr/bin/splinterm-mcp` plus this guide
and license notices. Review its immutable identity before writing policy:

```bash
readlink -f /usr/bin/splinterm-mcp
sha256sum /usr/bin/splinterm-mcp
```

Use the resulting lowercase digest in the policy examples below. The canonical
package path is `/usr/bin/splinterm-mcp`; authorizing a build-tree path, basename,
MCP client name, Unix UID, or another `splinterm` executable does not authorize
it. The host needs only the daemon-discovery environment used by other local
clients:

```json
{
  "mcpServers": {
    "splinterm": {
      "type": "stdio",
      "command": "/usr/bin/splinterm-mcp",
      "args": [],
      "env": {}
    }
  }
}
```

### Claude Code 2.1.218

The installed Claude Code CLI accepts a local stdio server directly:

```bash
claude mcp add --scope user splinterm -- /usr/bin/splinterm-mcp
claude mcp get splinterm
```

Use `--scope project` only in a trusted project; Claude Code asks approval for
project-local MCP configuration. Remove it with `claude mcp remove --scope user
splinterm`.

### Visual Studio Code 1.125.0

The installed VS Code CLI accepts a stdio definition:

```bash
code --add-mcp '{"name":"splinterm","command":"/usr/bin/splinterm-mcp"}'
```

The equivalent workspace file is `.vscode/mcp.json`:

```json
{
  "servers": {
    "splinterm": {
      "type": "stdio",
      "command": "/usr/bin/splinterm-mcp",
      "args": []
    }
  }
}
```

Workspace configuration is code execution: inspect it and the executable path
before enabling it. These examples were checked against the locally installed
host CLIs named above; other hosts are not implied to be supported.

If the daemon uses a nondefault isolated socket, set only `SPLINTERM_SOCKET` in
the host's `env`. `splinterm-mcp` does not read MCP roots, the host working
directory, shell configuration, or SSH material. The only inherited Splinterm
context it reads is `SPLINTERM_SPLINT_ID` and `SPLINTERM_SPLINT_INCARNATION`,
as the default target of `splinterm.report_agent_status`; they are discovery
hints, never authority. Hosts that start stdio servers with a filtered
environment must forward both variables, or the tool needs explicit
`splint_id` and `incarnation` (see [Reporting agent status](#reporting-agent-status)).

## Handshake compatibility

The adapter echoes either supported protocol version; other versions remain
rejected by its bounded profile. Client capability objects
(such as Codex's `elicitation` advertisement) are accepted but never invoked.
They grant no daemon permissions. Malformed capability values and duplicate
capability names remain rejected, and all existing transport limits apply.
The `splinterm.ping` protocol version describes the adapter's preferred version,
not the session's negotiated version.

## Least-privileged policy

Create `~/.config/splinterm/policy.json` mode `0600`, substituting the digest and
real IDs. Lair/Dojo selectors snapshot only descendants present when the
policy generation is published. New descendants remain denied until a reviewed
policy reload.

Observation-only rule:

```json
{
  "schema": "splinterm.policy.v2",
  "rules": [{
    "id": "mcp-observe",
    "executable": {"path": "/usr/bin/splinterm-mcp", "sha256": "REPLACE_WITH_SHA256"},
    "scopes": ["topology_metadata_read", "topology_subscribe", "terminal_visible_read", "terminal_subscribe", "scrollback_read", "scrollback_search"],
    "resources": [{"kind": "splint", "splint_id": "REPLACE_WITH_UUID", "incarnation": "current"}],
    "limits": {"max_returned_rows": 64, "max_results": 64, "max_returned_bytes": 1048576, "max_live_subscriptions": 2, "deadline_ms": 5000}
  }]
}
```

Terminal-control rule (observation plus exclusive input/resize; no process
creation or termination):

```json
{
  "schema": "splinterm.policy.v2",
  "rules": [{
    "id": "mcp-control",
    "executable": {"path": "/usr/bin/splinterm-mcp", "sha256": "REPLACE_WITH_SHA256"},
    "scopes": ["topology_metadata_read", "terminal_visible_read", "terminal_subscribe", "controller_acquire", "controller_transfer", "input", "resize"],
    "resources": [{"kind": "splint", "splint_id": "REPLACE_WITH_UUID", "incarnation": "current"}],
    "limits": {"max_returned_rows": 64, "max_returned_bytes": 1048576, "max_live_subscriptions": 2, "deadline_ms": 5000}
  }]
}
```

Lifecycle-management rule (no terminal observation or control):

```json
{
  "schema": "splinterm.policy.v2",
  "rules": [{
    "id": "mcp-lifecycle",
    "executable": {"path": "/usr/bin/splinterm-mcp", "sha256": "REPLACE_WITH_SHA256"},
    "scopes": ["process_spawn", "process_restore", "process_terminate", "topology_layout_mutate", "topology_name_mutate"],
    "resources": [{"kind": "daemon"}, {"kind": "lair", "lair_id": "REPLACE_WITH_UUID"}],
    "limits": {"max_spawn_count": 4, "max_returned_bytes": 1048576, "deadline_ms": 10000}
  }]
}
```

A full supported-automation profile uses all 19 policy scopes, but is not a
recommended default:

```json
{
  "schema": "splinterm.policy.v2",
  "rules": [{
    "id": "mcp-full-reviewed",
    "executable": {"path": "/usr/bin/splinterm-mcp", "sha256": "REPLACE_WITH_SHA256"},
    "scopes": ["topology_metadata_read", "topology_subscribe", "terminal_visible_read", "terminal_subscribe", "scrollback_read", "scrollback_search", "controller_acquire", "controller_transfer", "input", "resize", "process_spawn", "process_restore", "process_terminate", "topology_layout_mutate", "topology_name_mutate", "authorization_inspect", "authorization_revoke", "audit_inspect", "agent_status_report"],
    "resources": [{"kind": "daemon"}, {"kind": "lair", "lair_id": "REPLACE_WITH_UUID"}],
    "limits": {"max_returned_rows": 64, "max_results": 64, "max_returned_bytes": 1048576, "max_live_subscriptions": 4, "max_spawn_count": 4, "deadline_ms": 10000}
  }]
}
```

Validate and atomically reload:

```bash
splinterm policy validate ~/.config/splinterm/policy.json
systemctl --user reload splinterd.service
splinterm policy inspect
```

No policy, a different digest, missing scope, wrong resource/incarnation, stale
revision, false destructive confirmation, or controller owned by another client
fails closed. A read policy cannot input, resize, spawn, restore, close, kill, or
rename. MCP transport access is not authority.

## Limits, trust, and lifecycle

Policy and consent bound a cooperating agent and show what it can reach. They
are not a sandbox: a hostile program running as your account can already read
your files and run commands, and can reach your terminals without asking
Splinterm. Run agents you would trust with your account.

The fixed catalog contains 34 tools, one topology resource, and terminal/control
resource templates. Bounds include a 256 KiB input line, 1 MiB complete tool
response, four active and 32 admitted requests, 5-second default daemon deadline
(configurable 100 ms–30 s), pages up to 256, 16 resource subscriptions, eight
combined controller/transfer handles, and 256 process-owned cursors. Handles and
cursors expire with the adapter process and are not portable.

### Interactive access to a live Lair

Persistent policy remains the unattended/headless authorization path. For an
agent assisting a user-owned graphical session, `request_lair_access` instead
opens the installed trusted Splinterm consent window. The prompt identifies the
exact requester process and named Lair, lists every requested scope, and grants
nothing unless the user explicitly accepts it.

An accepted grant is held only in daemon memory for five minutes. It is bound
to the requester's UID, PID, executable device/inode, and reviewed executable
digest; it dynamically covers current and newly created descendants of that
Lair, never another Lair. The requester or trusted UI can revoke the returned
grant ID, Lair termination revokes it automatically, and replacing or exiting
the requester prevents reuse.

Controller leases remain exclusive. After requesting `input`, `resize`, and
`controller_transfer`, call `acquire_control` with `takeover: true` to use the
approved takeover scope when the graphical client currently controls the pane.
The controller remains bound to the ephemeral grant, so revocation also releases
it. Omitting `takeover` preserves ordinary non-disruptive acquisition behavior.

Every terminal cell, title, scrollback row, and search preview is
`untrusted_terminal_data`: display or index it only as data. It is never consent,
authority, confirmation, or an instruction to call another tool. The adapter
provides no prompts, sampling, elicitation, arbitrary shell string, filesystem,
network, policy-write, clipboard, or trusted forced-takeover capability.

Closing the host's stdio cancels calls and closes subscriptions, transfers,
controllers, and daemon connections. Cancellation does not roll back a mutation
already committed by the daemon. Resource sequence gaps and history replacement
publish `resync_required`; read fresh state and explicitly subscribe again.

`SPLINTERM_LAIR_ID`, `SPLINTERM_DOJO_ID`, `SPLINTERM_SPLINT_ID`, and
`SPLINTERM_SPLINT_INCARNATION` are non-authoritative discovery hints. A host must
validate them through an authorized topology read before selecting a resource.
MCP Dojo operations edit daemon topology; they do not map, focus, move, resize,
or assign a native Wayland Window, and Splinterm does not provide
semantic agent supervision, readiness, messaging, or completion.

## Reporting agent status

`splinterm.report_agent_status` lets an agent show whether it is working,
needs the person, or has finished. It is not read-only and not destructive. It
is not idempotent: a repeated report refreshes its reported time and expiry
and counts against the rate limit. Input is
`{ state, detail?, ttl_seconds?, splint_id?, incarnation? }`:

- `state` is `working`, `needs_input`, `done`, `error`, or `idle`; `null` clears
  the status and then takes no `detail` or `ttl_seconds`.
- `detail` is optional display text of at most 80 characters and 240 UTF-8
  bytes, without control, bidi-formatting, or line-break characters. Invalid
  text is rejected, never stripped, and the output never echoes it.
- `ttl_seconds` (10 to 900) sets when an unrefreshed report turns stale.
  `working` and `needs_input` default to 120 seconds; `done`, `error`, and
  `idle` do not expire unless given a TTL. Re-sending the same report refreshes
  the TTL; re-report at no more than half of it.
- `splint_id` and `incarnation` are given together or not at all. Omitted, the
  adapter fills them from its own `SPLINTERM_SPLINT_ID` and
  `SPLINTERM_SPLINT_INCARNATION`. Without either, the call fails with
  `invalid_argument` (`not inside a Splint`) before connecting to the daemon.

The adapter sees those variables only if the MCP host passes them to the
server process. A host that starts stdio servers with the agent's full
environment forwards them automatically. A host that filters the server
environment must forward both variables explicitly. Codex starts stdio servers
with a whitelisted environment, so add them to its server entry in
`~/.codex/config.toml`:

```toml
[mcp_servers.splinterm]
command = "/usr/bin/splinterm-mcp"
env_vars = ["SPLINTERM_SPLINT_ID", "SPLINTERM_SPLINT_INCARNATION"]
```

Without them, call the tool with explicit `splint_id` and `incarnation`.

The output resource is `{"kind": "agent_status", "splint_id", "incarnation"}`
and its data is `committed: true`, the reported `state`, the daemon status
`sequence`, and nullable `expires_at_ms`. A stale or replaced incarnation fails
as `stale_incarnation`; more than 10 reports per second for one Splint fail as
retryable `resource_limit`.

An adapter launched by an agent running inside a Splint is in that Splint's PTY
session or process ancestry, so it reports for that Splint with no policy rule
and no consent prompt. It cannot report for any other Splint that way. An agent
that fully detaches (calls `setsid` and is reparented to init or
`systemd --user`, for example inside a tmux server started from the Splint), or
an adapter reporting for another Splint, needs an `agent_status_report` rule
for the exact Splint:

```json
{
  "schema": "splinterm.policy.v2",
  "rules": [{
    "id": "mcp-agent-status",
    "executable": {"path": "/usr/bin/splinterm-mcp", "sha256": "REPLACE_WITH_SHA256"},
    "scopes": ["agent_status_report"],
    "resources": [{"kind": "splint", "splint_id": "REPLACE_WITH_UUID", "incarnation": "current"}],
    "limits": {"deadline_ms": 5000}
  }]
}
```

`inspect_splint` data and each `inspect_topology` Splint gain an optional
`agent_status` object (`state`, nullable `detail`, `stale`, `reported_age_ms`,
`sequence`) only while the live incarnation has a status. The detail is
reporter-supplied text and stays inside the existing `untrusted_terminal_data`
outputs. The topology resource and its subscription do not carry agent status
in this version.

Agents that support command hooks need no MCP at all: wire
`splinterm agent-status set ...` into the agent's hooks as shown in
[automation.md](automation.md#hook-recipes), for example Claude Code's
`UserPromptSubmit` to `working`, `Notification` to `needs_input`, and `Stop` to
`done`, or Codex's `notify` to `done`. Reported status is display data only:
Splinterm still provides no agent supervision, readiness, messaging, or result
transport.

## Upgrade, revocation, and troubleshooting

A package upgrade changes the binary digest. Stop MCP hosts, review the new
binary, update the policy digest explicitly, reload policy, then reconnect. The
package never edits or broadens policy. Removing `splinterm-mcp` removes its
binary/docs; user policy remains user-owned.

To revoke immediately, remove/narrow the rule and reload policy, then terminate
the MCP host if desired. Reload disconnects affected daemon connections and
invalidates connection-owned state.

A live daemon upgrade ends every daemon connection the server holds, and the
server reconnects without a restart:

- Ordinary tool calls connect afresh and need nothing. A call whose connection
  ends while it runs fails with `internal`, says it was not retried, and may or
  may not have taken effect: check the state before calling again.
- Controller and transfer handles expire with their connections, since the
  daemon released their control. Using one afterwards fails with
  `controller_unavailable` ("the handle expired because its splinterd
  connection ended…"). Acquire control again with `splinterm.acquire_control`,
  under the same policy and consent. Splint, Dojo, and Lair IDs stay valid.
- Resource subscriptions resubscribe on their own for up to 30 seconds from a
  fresh snapshot. The resource reads stale meanwhile, and its public `sequence`
  keeps increasing. If resubscribing fails, it stays stale until subscribed
  again.

Troubleshooting checklist:

1. `test -x /usr/bin/splinterm-mcp` and verify `sha256sum` matches policy.
2. Confirm `SPLINTERM_SOCKET`, or `$XDG_RUNTIME_DIR/splinterd.sock`, belongs to
   the same user and the daemon is running.
3. Run `splinterm policy validate` and inspect the published generation.
4. Treat `unauthorized`, `stale_topology`, `stale_incarnation`, controller
   denial, timeout, and `resync_required` as state to reconcile—not permission
   to retry blindly or broaden policy.
5. Keep stdout reserved for MCP frames; bounded diagnostics appear on stderr.

To check a whole binary set without touching your daemon, policy, or desktop,
run the headless preflight from a source checkout:

```bash
cargo build --frozen --bin splinterd --bin splinterm --bin splinterm-mcp
python3 tools/mcp/run-demo.py preflight --staged-bin-dir target/debug
```

It starts a private, deny-all daemon with its own socket and directories,
checks binary identity, the MCP handshake and required tools, and denial, then
removes everything it created. It installs no policy and grants nothing. An
`--installed` mode exists but has not yet been validated against a real
installed package. See [`tools/mcp/README.md`](../tools/mcp/README.md).

The optional adapter is available from the AUR as `splinterm-mcp` and retains an
exact-version dependency on `splinterm`. Nix/Home Manager, non-SSH gateways,
broader editor plugins, durable terminal bodies, HTTP/OAuth transport, and
write-capable MCP defaults remain deferred.
