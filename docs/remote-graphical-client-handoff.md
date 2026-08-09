# Handoff: Remote Graphical Splinterm Client

## Purpose

Implement the missing remote graphical-client workflow and correct the product
documentation so it clearly distinguishes remote graphical use from remote
automation.

A user running `splinterd` on another host should be able to open the native
Splinterm client locally and attach it to that remote daemon through the existing
SSH stdio relay.

Example UX:

```bash
splinterm --remote wintermute sessions
splinterm --remote wintermute window --lair-id LAIR_ID --dojo-id DOJO_ID
```

`wintermute` should resolve through a named, strictly parsed remote profile. The
local daemon remains the default when `--remote` is absent.

## Product-documentation correction

The current documentation contains a gap:

- `docs/PRD.md` promises remote operation without exposing a daemon network
  listener;
- `docs/remote.md` documents the SSH stdio relay as an automation transport;
- the original graphical MVP explicitly excluded remote graphical operation;
  and
- no later requirement explicitly restores remote graphical attachment as a
  native-client feature.

Correct the PRD to require both:

1. policy-authorized remote automation through the SSH relay; and
2. a native local graphical client that can use that relay to present and
   control a remote daemon-owned terminal topology.

The requirements must continue to state that `splinterd` exposes no TCP listener
and owns no SSH credentials.

## Feature contract

When a remote profile is selected, Splinterm must:

- spawn `ssh` with structured arguments and no shell interpolation;
- require non-terminal SSH operation;
- execute the fixed remote command `/usr/bin/splinterm relay --stdio`;
- negotiate the existing private Splinterm protocol over child stdin/stdout;
- show the remote session picker and open selected remote Dojos;
- render terminal text snapshots and ordered updates;
- support panes, tabs, input, resize, scrollback, search, and normal controller
  ownership when authorized;
- preserve remote Splints when the local client or SSH transport exits;
- report SSH failure, policy denial, and protocol mismatch clearly; and
- keep SSH stderr separate from protocol stdout.

A dropped SSH connection may close the affected local view. It must not imply
that the remote Splints were terminated.

## Security boundary

The remote graphical client must not impersonate the trusted local UI.

The remote daemon sees `/usr/bin/splinterm-relay` as its peer. It must authorize
that exact installed executable through persistent policy. SSH authenticates the
host and account but does not authorize terminal operations.

Remote profiles must:

- use strict host-key verification by default;
- represent SSH options as structured configuration;
- reject unsafe or ambiguous profile values;
- keep the remote relay command fixed;
- never derive commands or options from terminal content; and
- document the authority delegated to callers who can invoke the relay under the
  selected account.

The implementation must use the automation client role for remote connections.
The local Unix-socket path must retain its existing trusted-UI identity and
behavior.

## Implementation approach

The existing relay and private protocol already provide the required remote byte
transport. Do not add a daemon TCP listener or replace the relay.

Refactor the client connection boundary from a concrete local `UnixStream` to a
connection factory capable of opening either:

```text
Local endpoint
  -> Unix socket
  -> TrustedUi protocol role

Remote endpoint
  -> ssh child stdin/stdout
  -> /usr/bin/splinterm relay --stdio
  -> Automation protocol role
```

The factory must be clonable because the graphical client currently opens
separate daemon connections for observation, control, topology management, and
pane tasks. Each connection must own and reap its SSH child cleanly.

Likely implementation areas:

- `crates/splinterm-automation-client/src/lib.rs`
  - abstract the control transport;
  - preserve the local Unix-socket constructor;
  - support split child stdin/stdout;
  - retain request correlation, cancellation, bounds, and handshake behavior.
- `crates/splinterm/src/app/commands.rs`
  - add the remote-profile selection option.
- `crates/splinterm/src/app/cli.rs`
  - resolve the endpoint and pass a connection factory into graphical flows.
- `crates/splinterm/src/app/window.rs`
- `crates/splinterm/src/app/pane_bridge.rs`
- `crates/splinterm/src/app/topology_manager.rs`
- `crates/splinterm/src/app/sessions.rs`
  - replace direct `Connection::connect()` calls with the selected factory.
- client configuration modules under `crates/splinterm/src/config/`
  - define and validate named remote profiles.

The relay and daemon protocol should remain unchanged unless implementation
reveals a concrete missing protocol capability.

## Images

Remote graphical attachment must not weaken the existing terminal-image security
boundary.

Image pixel bodies currently use a separate trusted local content channel and
sealed memfd or bounded binary transfer. Relay clients intentionally do not
receive those bodies. The initial remote graphical path should therefore render
text and safely omit unavailable image pixels rather than attempting to open a
local image-content socket for a remote endpoint.

Remote image transport is outside this feature. It requires its own reviewed
security design before changing relay or image-content authority.

## Documentation changes

Update:

- `docs/PRD.md`
  - add an explicit remote graphical-client requirement;
  - distinguish it from remote automation;
  - record the implementation state honestly until validation is complete.
- `docs/remote.md`
  - document named-profile setup, the graphical command examples, relay policy,
    host-key behavior, disconnect semantics, and the image limitation.
- `docs/architecture.md`
  - show local Unix-socket and remote SSH-relay client transports.
- `docs/configuration.md`
  - document the strict remote-profile schema.
- README or user-facing CLI documentation
  - include the basic remote sessions/window workflow.

Do not claim the feature is implemented or validated until code, evidence, and
independent review exist.

## Acceptance criteria

The feature is complete when:

1. `splinterm --remote PROFILE sessions` presents sessions from the selected
   remote daemon.
2. A selected remote Dojo opens in the local native Wayland client.
3. Text output, panes, tabs, input, resize, scrollback, search, and controller
   ownership work through the SSH relay within granted policy.
4. Closing the local window leaves remote Splints running.
5. SSH EOF, relay death, daemon loss, policy denial, and protocol mismatch fail
   clearly without corrupting local or remote state.
6. The local client path retains existing behavior.
7. Remote connections do not claim trusted local UI authority.
8. Remote connections do not attempt to retrieve image bodies through the local
   content socket.
9. The PRD and remote documentation accurately describe the delivered behavior
   and limitations.

## Validation

Routine tests must use a fake SSH executable or child process rather than a real
network host. The harness should record exact argv and expose protocol bytes over
stdin/stdout.

Cover:

- strict profile parsing and SSH argv construction;
- fixed remote relay command;
- protocol stdout and diagnostic stderr separation;
- handshake success and version mismatch;
- session/topology reads;
- attach, snapshot, update ordering, and resynchronization;
- control acquisition, input, resize, and release;
- policy denial and insufficient-scope errors;
- SSH EOF, daemon EOF, relay death, cancellation, and child reaping;
- remote no-image behavior; and
- unchanged local Unix-socket behavior.

Run the focused non-graphical validation appropriate to the changed files,
including:

```bash
cargo test -p splinterm-automation-client
cargo test -p splinterm --lib
cargo test -p splinterm --test automation_cli
cargo test -p splinterm-relay --lib
cargo test -p splinterm-relay --test stdio
git diff --check
```

If protocol or daemon authorization code changes, also run:

```bash
cargo test -p splinterm-protocol
cargo test -p splinterd --test end_to_end -- --test-threads=1
```

A real `wintermute` smoke remains operator-gated. Any graphical test requires the
separate approval and isolation required by `AGENTS.md`.
