# Plan 0028 reconnect hotfix investigation handoff

## Mission

Investigate the real-host Phase 4 failure in native remote graphical reconnect,
identify the root cause, propose the smallest safe fix, and produce a
validation-ready hotfix plan. Do not start with a speculative code change.
Localize the stall first, reproduce it non-graphically, add a regression test,
then recommend a bounded implementation.

The user requested an investigation and hotfix plan, not an automatic graphical
rerun. Real SSH-host changes and graphical testing remain approval-gated by
`AGENTS.md`.

## Repository state

Working directory: `/home/oldjobobo/Projects/splinterm`

Relevant commits:

```text
11462b6 Record blocked Holodeck validation
efebc7e Validate legacy Nerd Font dependency
25e4ef2 Support legacy JetBrains Nerd Font package
3475725 Add native remote graphical sessions
```

Unrelated pre-existing work must remain untouched:

```text
 M AGENTS.md
?? docs/remote-graphical-client-handoff.md
```

Plan and retained narrative:

- `docs/plans/0028-remote-graphical-client.md`
- Phase 4 subsection: `Phase 4 Holodeck attempt — 2026-08-07`

The local repository graph exists at `graphify-out/graph.json`, but its query for
this new path was broad and stale enough that direct source inspection remains
necessary.

## Current machine and Holodeck state

### Local workstation

- The workstation's Pacman-owned `/usr/bin/splinterm` was deliberately not
  replaced because its running daemon owns many live user Splints.
- Phase 4 used the exact validated client extracted from the newly built package
  into `/tmp`; that temporary client and launch scripts were removed during
  cleanup.
- `~/.config/splinterm/remotes.toml` now contains the normal `holodeck` profile.
- The original local focus/workspace was restored; workspace 8 / DP-2 was empty
  after cleanup.

### Holodeck

- Host alias: `holodeck`; account: `oldjobobo`; x86_64 Arch/Omarchy.
- Package `splinterm 0.1.0.pre-1` is installed and passed `pacman -Qkk` with zero
  altered files.
- `/usr/bin/splinterm`, `/usr/bin/splinterd`,
  `/usr/bin/splinterm-relay`, and `/usr/bin/splinterm-pty-child` are adjacent,
  root-owned, mode `0755`, and package-consistent.
- Installed relay SHA-256:
  `d09a5ec74b30126f4921fca9a7cf00abdfda0fce32f978ecf9a2abd8e8dd334b`.
- `splinterd.service` is active with no Lairs or test shells.
- The final policy is an owner-only daemon-level bootstrap rule for the exact
  relay digest. It grants only topology read plus daemon-level spawn/layout
  authority; it does not retain the temporary Lair or diagnostic CLI grants.
- Uploaded package/policy candidates were removed.
- Reversible reset backup:
  `/home/oldjobobo/.local/state/splinterm.reset-1786150469754`.
- Normal SSH uses the existing agent key. `SSH_AUTH_SOCK` locally was
  `/run/user/1000/ssh-agent.socket`.

Do not reconnect to Holodeck, alter its policy, install another package, create
topology, or run graphical tests without a new explicit approval covering the
new sequence.

## What succeeded on the real host

The first native remote Window worked end to end:

1. Agent-backed `remote check` negotiated one SSH child, the outer graphical
   relay, one logical channel, the private daemon protocol, `Ping`, and
   `ListLairs`.
2. A native remote Window mapped only on workspace 8 / DP-2 and was selected by
   exact Hyprland address before focus/input.
3. The Window rendered the remote shell and accepted:

   ```text
   printf 'SPLINTERM_PHASE4_INPUT_OK\n'; pwd; printf '%s\n' "$SHELL"
   ```

   It returned:

   ```text
   SPLINTERM_PHASE4_INPUT_OK
   /home/oldjobobo
   /bin/bash
   ```

4. Two hundred ordered lines rendered; detached scrollback and compositor-driven
   resize worked.
5. Unknown and changed host keys failed closed with distinct diagnostics against
   isolated fixtures.
6. Closing the local Window ended its client process and SSH lifetime while the
   exact remote shell PID remained alive.
7. A raw exact-frame probe sent a 16-byte `SPGR` v1 Hello and received the exact
   16-byte HelloAck:

   ```text
   53 50 47 52 00 01 02 00 00 00 00 00 00 00 00 00
   ```

This rules out a basic package mismatch, persistent host-key problem, relay
banner contamination, global policy denial, or inability to open any graphical
relay channel.

## Exact failure chronology

After closing the successful Window, the remote shell remained alive with the
same PID. Two bounded reopen attempts were made.

### Reopen attempt 1

No Window mapped. Local launch log:

```text
Error: graphical relay protocol is incompatible: remote graphical relay: graphical relay magic is invalid
```

Artifact, if still present in the current machine session:

```text
/tmp/splinterm-phase4-window-reopen.log
```

A direct raw relay probe with closed stdin produced no stdout and the expected
stderr `graphical relay input closed before Hello`. The subsequent exact Hello
probe returned a byte-perfect HelloAck and no stderr. Therefore no persistent
remote shell/banner contamination was reproduced.

### Reopen attempt 2

After the exact Hello probe passed, one bounded reopen retry was allowed. Again,
no Window mapped. Local launch log:

```text
Error: remote transport failed: remote logical channel or daemon handshake timed out
```

Artifact, if still present:

```text
/tmp/splinterm-phase4-window-reopen2.log
```

This is the primary reproducible release blocker. The current timeout combines
logical-channel admission and private daemon negotiation, so it does not reveal
which stage stalled.

### Audit evidence correction

Do not repeat the earlier over-strong interpretation that audit proved the
failed reconnect was polling topology continuously.

The retained audit response reported:

```text
oldest_available_audit_id = 321
newest_available_audit_id = 1344
next_after_audit_id = 384
record_count = 64
```

The captured page was IDs `321–384`, timestamped 17:46:04–17:46:20, during the
successful Window. It showed normal authorized `inspect_topology` polling and no
denials on that page. It was the first page, not the newest page, and therefore
does not identify the reconnect stall. Holodeck was reset during cleanup, so
newest-page audit evidence is no longer available.

Potential artifacts:

```text
/tmp/splinterm-phase4-audit-reconnect.json
/tmp/splinterm-phase4-audit-reconnect.raw
/tmp/splinterm-phase4-relay-hello.bin
/tmp/splinterm-phase4-relay-hello.stderr
/tmp/splinterm-phase4-window-reopen.log
/tmp/splinterm-phase4-window-reopen2.log
```

Treat `/tmp` artifacts as opportunistic, not durable source-of-truth.

## Startup channel sequence to trace

One `ConnectionFactory::remote` creates one `RemoteSession`, one OpenSSH child,
and one `ClientMultiplexer`. Every factory clone opens another logical daemon
connection over that same multiplexer.

For one single-pane Window, `run_live_multipane_window` currently performs:

1. `initial_window_dojo_identity`:
   - opens a short-lived logical daemon connection;
   - sends `ListLairs`;
   - drops the connection on return.
2. `prepare_live_pane` for each Splint:
   - opens the retained observation/subscription connection;
   - performs `InspectSplint`, `RequestAccess`, `AuthorizationStatus`, and
     `Attach`;
   - opens a second retained control connection;
   - performs `InspectSplint` and optional control acquisition;
   - spawns controller and pane-subscription tasks.
3. `run_topology_manager`:
   - opens another retained connection;
   - polls `InspectTopology` every 250 ms.
4. `run_window` maps the Wayland Window after pane preparation.

The reported aggregate timeout may therefore originate in channel admission or
private daemon Hello for any of these connections. Determine the exact channel
ID and stage before changing lifecycle behavior.

## Primary code paths

### SSH session and aggregate timeout

`crates/splinterm/src/remote_session.rs`

- `RemoteSession::connect_with_program_and_timeout`
- `RemoteSession::connect_automation`
- `RemoteSession::connect_automation_inner`
- `SessionChannel`
- `RemoteLifetime::drop`
- `supervise_child`

Important current behavior:

```rust
tokio::time::timeout(
    self.operation_timeout,
    self.connect_automation_inner(),
)
```

wraps both:

```rust
self.multiplexer.open_channel().await
```

and:

```rust
Connection::connect_automation_transport(reader, writer).await
```

The first diagnostic hotfix should split these stages or attach stage context so
future failures say `open-channel admission timed out` versus `daemon Hello
timed out`. Preserve the original total deadline; do not accidentally double it.

### Endpoint factory

`crates/splinterm/src/endpoint.rs`

- `ConnectionFactory::remote`
- `ConnectionFactory::connect`

All clones share `Arc<EndpointKind::Remote(RemoteSession)>`; verify no clone or
background task outlives Window shutdown unexpectedly.

### Client multiplexer

`crates/splinterm-graphical-relay/src/client.rs`

- `ClientMultiplexer::negotiate`
- `ClientMultiplexer::open_channel`
- `dispatch_frame`
- `run_incoming_channel`
- `run_outgoing_channel`
- `ChannelGuard::drop`

High-value lifecycle detail: `ChannelGuard::drop` removes the local route
immediately, then spawns an asynchronous task that drains queued data and sends
`CloseChannel`. Inspect ordering against a subsequent `OpenChannel`, runtime
shutdown, writer cancellation, and session drop. Do not assume this is the bug:
each failed reopen used a fresh process/SSH/multiplexer, so a prior local route
cannot directly survive into it. It may still expose incomplete remote daemon
cleanup or an independent within-startup open/drop/open race.

### Server relay

`crates/splinterm-relay/src/lib.rs`

- `run_graphical_streams_with_connector`
- `run_graphical_channel`
- `apply_channel_command`
- `finish_channel`
- `monitor_daemon_peer`

The server coordinator synchronously awaits `connect_validated` for every
`OpenChannel`, queues `ChannelOpened`, then starts the daemon bridge. Check
whether any connector or peer-identity path can block the entire coordinator
under real daemon/socket timing. Add a bounded, body-free event trace before
changing this ordering.

### Private protocol handshake

`crates/splinterm-automation-client/src/lib.rs`

- `Connection::connect_automation_transport`
- `Connection::connect_transport`

The client writes private `ClientFrame::Hello` only after outer channel admission
and waits for the daemon `ServerFrame::Hello`. Distinguish “Hello never left the
logical channel,” “relay never delivered it,” and “daemon accepted the Unix
socket but never returned Hello.”

### Window startup and shutdown

- `crates/splinterm/src/app/window.rs::run_live_multipane_window`
- `crates/splinterm/src/app/pane_bridge.rs::prepare_live_pane`
- `crates/splinterm/src/app/topology_manager.rs::run_topology_manager`
- pane/controller task cleanup in `pane_bridge.rs`

Confirm that close waits for/cancels every pane and topology task and releases
every `ConnectionFactory` clone before the process exits. Process exit should
still make SSH EOF authoritative; do not introduce reconnect or retry.

## Existing test gap

Current tests establish useful properties but do not reproduce this lifecycle:

- `one_fake_ssh_process_serves_multiple_automation_connections` opens two
  channels concurrently, pings, and drops everything once.
- graphical-relay tests route multiple channels, test fairness, reject limits,
  and fail unknown-channel traffic.
- fake SSH handles channel close frames and channel-local loss.

Missing coverage:

1. open a short-lived channel, drop it, immediately open retained observation and
   control channels, then a topology channel—the exact Window startup pattern;
2. close/drop the complete first session and prove its SSH child and all logical
   daemon connections are reaped;
3. create a second fresh `RemoteSession` and repeat the startup pattern against
   the same real daemon and retained shell identity;
4. include real outer codec, relay coordinator, Unix socket bridge, and private
   daemon Hello rather than a Python fixture that responds immediately;
5. delay selected acknowledgements/daemon Hellos to expose ordering races without
   broad timeouts.

## Ranked hypotheses

These are investigation leads, not accepted root causes.

### H1 — startup open/drop/open ordering stalls one logical channel

The Window startup intentionally creates a short-lived identity connection
before opening retained pane/control/topology connections. Current tests do not
exercise this exact ordering through the real relay and daemon. Inspect
`ChannelGuard::drop`, control-frame ordering, fair-data drain, and server channel
removal.

Counterpoint: the failed reconnect has a fresh multiplexer, so any explanation
must occur within its own startup sequence or through delayed remote cleanup.

### H2 — server coordinator blocks in per-channel validated Unix connection

`run_graphical_streams_with_connector` awaits `connector().await` inline. A real
socket/peer-identity operation that stalls would block later outer frames and
manifest as aggregate channel/daemon timeout. Instrument elapsed time and
channel ID around connector begin/success/reject.

### H3 — private Hello is queued but never forwarded or answered

Outer Hello/HelloAck is proven healthy. A channel can be admitted while its
private Hello stalls in client fair-data admission, server command delivery, Unix
socket write, or daemon response. Stage-specific tracing should locate the last
observed boundary.

### H4 — incomplete first-Window daemon connection cleanup affects the next client

The shell surviving is correct; subscriptions/controllers and protocol
connections must not survive. Verify daemon-side connection counts and release
metadata after closing the first client. Do not infer this solely from the
remote shell PID.

### H5 — first invalid-magic error is a separate diagnostic/race

The exact raw Hello consistently received a correct HelloAck, so persistent
stdout contamination is ruled out. Preserve the first error as evidence, but
focus hotfix acceptance on the reproducible logical-channel/private-handshake
timeout unless instrumentation reconnects the two symptoms.

## Investigation plan

### Milestone 1 — improve observability without changing behavior

1. Split or annotate the remote connection deadline into outer channel-admission
   and private daemon-Hello stages while preserving one total deadline.
2. Add opt-in, metadata-only tracing for:
   - session generation and child PID;
   - logical channel ID;
   - OpenChannel queued;
   - ChannelOpened/Rejected received;
   - private Hello write completed;
   - private Hello response received;
   - CloseChannel queued/received;
   - daemon Unix connector begin/success/failure;
   - channel and session cancellation.
3. Never log terminal bytes, input, payloads, credentials, controller tokens, or
   protocol bodies.
4. Add unit tests for stage-specific timeout classification.

Validation:

```bash
cargo test -p splinterm-graphical-relay
cargo test -p splinterm --test remote_session
cargo clippy -p splinterm-graphical-relay -p splinterm --all-targets --all-features -- -D warnings
```

### Milestone 2 — deterministic non-graphical reproduction

1. Add an integration fixture using a real `splinterd` and real
   `splinterm-relay` over stdio or a faithful in-process connector.
2. Model the exact single-pane startup sequence and complete teardown.
3. Repeat with a second fresh session against the same daemon while the first
   shell remains alive.
4. Assert all first-session channels/controllers/subscriptions are released and
   the second session completes every private Hello within the existing bound.
5. Add controlled delays around channel ack, connector, and private Hello to
   expose races deterministically.

Do not use a real SSH host or Wayland for this milestone.

### Milestone 3 — root-cause fix

Choose only after Milestones 1–2 identify the stuck boundary. Examples of
acceptable fix classes:

- make close/open control-frame ordering explicit;
- ensure dropped channels cannot retain fair-queue permits or block later data;
- guarantee server channel cleanup completes or is safely independent before
  later admission;
- move a blocking connector operation out of the coordinator while preserving
  bounded admission and monotonically increasing IDs;
- repair task/lifetime ownership so Window close deterministically releases all
  channel resources.

Do not “fix” this by:

- increasing timeouts;
- adding automatic reconnect/retry;
- weakening policy or executable identity;
- reducing channel/accounting bounds;
- making the relay parse private protocol payloads;
- changing raw `relay --stdio` behavior;
- accepting unknown host keys;
- collapsing independent logical connections into one daemon connection.

### Milestone 4 — regression and aggregate validation

Required focused coverage:

```bash
cargo fmt --all -- --check
cargo test -p splinterm-graphical-relay
cargo test -p splinterm-relay --lib
cargo test -p splinterm --test remote_session
cargo test -p splinterm --lib
git diff --check
```

Then run exact strict Clippy and the affected serialized daemon tests. If the
hotfix changes shared relay/client lifecycle, run the complete serialized
workspace matrix from Plan 0028.

Inspect the actual diff and obtain one fresh read-only review at the coherent
hotfix boundary.

### Milestone 5 — new approval-gated Holodeck validation

Only after non-graphical reproduction passes, the fix is reviewed, a clean
committed package is built, and exact artifacts exist:

1. request a new complete graphical approval under `AGENTS.md`;
2. back up and upgrade Holodeck's package/policy;
3. run one guarded open → close → same-PID persistence → reopen smoke first;
4. abort if that smoke fails;
5. only then continue the remaining password/askpass, mutation, transfer,
   SSH/relay/daemon-loss, local-regression, and cleanup matrix.

Do not reuse the prior approval after its abort.

## Hotfix acceptance criteria

A proposed hotfix is ready only when all are true:

1. A deterministic non-graphical regression test fails on `3475725` behavior and
   passes with the fix.
2. The exact stalled boundary is named with evidence.
3. One Window can close while its remote shell remains alive.
4. A second fresh SSH/relay session reopens that exact retained Dojo.
5. Every first-session logical channel, subscription, controller, task, and SSH
   child is reaped.
6. The fix preserves one SSH authentication per client lifetime, bounded/fair
   multiplexing, ordered shutdown, automation role, exact executable identity,
   no remote images, and raw relay compatibility.
7. Timeouts are not merely widened and no automatic retry/reconnect is added.
8. Focused tests, strict Clippy, required serialized matrices, diff hygiene, and
   fresh review pass.
9. A newly approved Holodeck graphical smoke passes before the rest of Phase 4
   resumes.

## Expected investigation deliverable

The next agent should return:

- the exact stage and channel ID that stalls;
- a minimal failing regression test;
- root-cause analysis tied to concrete ownership/order code;
- the smallest proposed code change and why it preserves invariants;
- files to change;
- focused and aggregate validation commands;
- rollback/residual risks;
- a dependency-ordered hotfix implementation plan;
- no graphical completion claim.
