# Plan 0028: Remote graphical client over SSH

- **Status:** In progress — Phases 1–3 complete and reviewed; Phase 4 Holodeck validation attempted and blocked by reconnect logical-channel timeout
- **Date:** 2026-08-07
- **Product goal:** provide the Splinterm equivalent of a local terminal emulator attaching through SSH to a remote tmux server
- **Depends on:** the accepted daemon-owned multiplexing, control, policy, image-security, headless-service, and SSH-relay work
- **Input:** [Remote graphical client handoff](../remote-graphical-client-handoff.md)
- **Security authorities:** [SSH stdio relay](../remote.md), [Architecture](../architecture.md), [Automation policy ADR](../adr/0007-supported-automation-policy.md), and [bounded terminal image ADR](../adr/0008-bounded-terminal-image-plane.md)

## Goal

A user with persistent Splinterm sessions on another Linux host must be able to
open the native Splinterm client locally, authenticate to that host through
OpenSSH, and attach to the remote daemon-owned topology.

The product analogy is exact:

```text
Conventional workflow
  local Foot -> SSH -> remote tmux -> remote sessions

Splinterm workflow
  local native Splinterm Window
    -> one authenticated SSH transport
    -> installed remote Splinterm graphical relay
    -> remote splinterd
    -> remote Lairs, Dojos, and Splints
```

The local Window renders and controls remote terminal state. The remote
`splinterd` remains the sole owner of remote PTYs, processes, topology,
scrollback, and persistence. Closing the local Window or losing SSH detaches the
view and releases connection-owned authority; it does not terminate remote
Splints.

The primary workflow is:

```bash
splinterm --remote wintermute
```

With a selected profile, absence of a subcommand opens the remote Recent
Sessions picker. Explicit forms remain available:

```bash
splinterm --remote wintermute sessions
splinterm --remote wintermute reopen
splinterm --remote wintermute window --lair-id LAIR_ID --dojo-id DOJO_ID
splinterm --remote wintermute launch
```

The local daemon remains the default when `--remote` is absent.

## Product decisions

### This is a native remote client, not a remote shell inside a local Splint

Splinterm does not create a local terminal and type `ssh` into it. It launches
OpenSSH as a hidden transport and presents the remote daemon's topology directly
through local native Wayland UI.

Consequences:

- existing remote Dojos appear in the local session picker;
- local tabs and panes are disposable views over remote persistent Dojos and
  Splints;
- input, resize, search, scrollback, and control ownership operate against the
  remote daemon;
- the remote host chooses remote shell defaults and interprets remote paths;
- the local renderer, theme, keymap, clipboard, IME, and Wayland integration
  remain local; and
- one native Window is bound to exactly one endpoint in the first release. A
  Window cannot mix local Dojos and Dojos from different remote profiles.

### The automation role is an authorization boundary, not an automation UX

Every remote protocol channel negotiates `ClientRole::Automation`. This prevents
an SSH-arriving process from claiming the installed local graphical client's
trusted-UI bypass. It does not make the user experience headless.

With persistent policy granting the required scopes, the native remote Window
may observe terminal state, subscribe to updates, acquire ordinary controller
ownership, send input and resize, search scrollback, and perform approved
remote topology mutations. Trusted-local-only operations remain unavailable.

### OpenSSH owns host and user authentication

Splinterm does not implement SSH cryptography, store passwords, parse private
keys, or copy credentials to the remote host. It invokes the installed OpenSSH
client with structured local argv.

OpenSSH continues to own:

- server host-key verification;
- public-key, certificate, agent, hardware-key, and password authentication;
- supported `~/.ssh/config` host aliases, proxies, and identity selection; and
- local terminal or `SSH_ASKPASS` prompting.

Splinterm owns only profile validation, safety-critical option overrides,
transport lifecycle, bounded diagnostics, and the fixed remote command.

### Authentication occurs once per local remote-client lifetime

A graphical Window opens multiple independent daemon protocol connections today
for topology, observation, control, and pane tasks. Running one independently
authenticated SSH process per connection would prompt password users repeatedly.
OpenSSH ControlMaster reuse would make the feature depend on the server's
`MaxSessions` limit and would fail for legitimate multi-pane/tab topologies.

The graphical path therefore uses one SSH process and one new bounded graphical
relay stream. That stream multiplexes independent logical byte channels. Each
logical channel maps to one independently validated Unix-socket connection to
remote `splinterd`.

```text
one local Splinterm process
  -> one SSH authentication and child process
    -> one splinterm-relay --graphical-stdio process
      -> bounded channel multiplexer
        -> daemon Unix connection: topology
        -> daemon Unix connection: pane observation
        -> daemon Unix connection: pane control
        -> daemon Unix connection: additional pane tasks
```

This preserves current daemon connection ownership, request correlation,
subscription limits, cancellation, and controller-release behavior while
requiring only one SSH login.

### Existing automation relay compatibility remains unchanged

The current byte-transparent command remains exactly as documented:

```bash
/usr/bin/splinterm relay --stdio
```

It continues to carry one raw private-protocol connection for supported
automation clients. It does not gain multiplex framing and its wire behavior
must not change.

The graphical client uses a distinct fixed command:

```bash
/usr/bin/splinterm relay --graphical-stdio
```

The `splinterm` wrapper replaces itself with the adjacent installed
`splinterm-relay`, as it does for `--stdio`. The dedicated mode makes protocol
selection unambiguous and prevents an older raw relay client from interpreting
multiplexer bytes as daemon protocol frames.

## Non-goals

- Add a TCP, QUIC, WebSocket, or other network listener to `splinterd`.
- Reimplement SSH, SSH agent, known-hosts, or private-key storage.
- Automatically create keys, copy keys, accept host fingerprints, edit
  `authorized_keys`, change sshd configuration, or enable service lingering.
- Treat successful SSH login as Splinterm terminal authorization.
- Grant the remote graphical client trusted local UI identity.
- Transport image pixel bodies in the first release.
- Forward terminal-provided file paths, Kitty file/SHM sources, local sockets,
  arbitrary commands, or arbitrary SSH options.
- Mix local and remote Dojos or multiple remote profiles in one Window.
- Automatically reconnect and reacquire controllers after a dropped transport.
- Preserve a mapped local view across SSH or remote daemon failure. Remote
  Splints persist, but the affected local view may close.
- Promise Windows, macOS, non-OpenSSH clients, or non-Wayland graphical support.

## Current state and required seams

### Relay

`crates/splinterm-relay` currently carries one opaque byte stream between
stdin/stdout and one strictly validated daemon Unix socket. It validates the
socket path, owner, mode, connected UID, live daemon PID, and exact adjacent
`splinterd` executable. It keeps stderr separate and enforces non-terminal
stdin/stdout.

Required change:

- preserve raw `--stdio` behavior byte-for-byte;
- add a separately selected bounded graphical multiplexer;
- open and validate one daemon Unix connection per accepted logical channel;
- retain exact relay and daemon executable identity requirements; and
- close all logical channels promptly when SSH or the remote daemon disappears.

### Automation client connection

`crates/splinterm-automation-client::Connection` currently stores one concrete
Tokio `UnixStream`, chooses trusted or automation role at construction, and uses
an optional control-socket path for the trusted local image-content channel.

Required change:

- abstract protocol reading and writing from the concrete Unix stream;
- accept split async reader/writer transports;
- retain the local `Connection::connect()` constructor and behavior;
- add an automation constructor over one logical graphical-relay channel;
- carry explicit endpoint capabilities rather than inferring every behavior
  from socket presence; and
- retain current frame bounds, request IDs, queued-event bounds, cancellation,
  handshake, and version-mismatch behavior.

### Graphical orchestration

Direct `Connection::connect()` calls are spread across `app/sessions.rs`,
`app/window.rs`, `app/pane_bridge.rs`, and `app/topology_manager.rs`.
Graphical focus publication assumes a matching local trusted UI. Session recency
is stored in one local `recent-dojos.json`. Graphical mutations compile local
`LaunchParameters`, including local CWD and shell configuration.

Required change:

- resolve one endpoint before entering graphical command flows;
- pass a clonable endpoint connection factory through every graphical service;
- suppress trusted-only focus publication for remote endpoints;
- namespace recent-session state by endpoint identity;
- compile remote process creation through existing `AutomationLaunch` request
  variants; and
- preserve the local path exactly when no remote profile is selected.

### Images

The daemon already includes image metadata only for the exact matching trusted
local UI. An automation-role relay client receives text state without image
metadata, and image body retrieval rejects non-trusted connections.

Required change:

- encode `ImageTransport::LocalTrusted` versus `ImageTransport::Unavailable` in
  the endpoint contract;
- never derive or open a `.content` socket for a remote endpoint;
- tolerate absent remote image planes and render text normally; and
- test that future metadata or capability regressions fail closed rather than
  attempting local image retrieval.

## Architecture

### Endpoint contract

Introduce an application-owned endpoint abstraction with behavior, not merely a
function returning bytes:

```text
ClientEndpoint
  Local
    protocol role: TrustedUi
    transport: owner-local Unix socket
    graphical focus publication: enabled
    image content: trusted local content channel
    launch semantics: local LaunchParameters
    recency namespace: local

  Remote(RemoteProfile, RemoteSession)
    protocol role: Automation
    transport: logical channel over one SSH graphical relay
    graphical focus publication: disabled
    image content: unavailable
    launch semantics: AutomationLaunch interpreted by remote daemon
    recency namespace: stable profile identity
```

A Window and every task it spawns receive clones of one `ConnectionFactory`.
For local endpoints, each `connect()` opens the current Unix socket. For remote
endpoints, each `connect()` opens a logical channel on the existing authenticated
`RemoteSession`.

The endpoint must never be selected from terminal content, daemon-provided
names, CWD values, or rendered picker labels.

### Graphical relay multiplexer

Add a small transport-only framing contract in a shared crate or module used by
the local graphical client and `splinterm-relay`. It is not the daemon private
protocol and must not leak into `splinterd`.

Required frame classes:

```text
Hello / HelloAck
OpenChannel / ChannelOpened / ChannelRejected
Data
HalfClose
CloseChannel
SessionError
```

Contract requirements:

- fixed magic and exact version negotiation before channels open;
- monotonically allocated nonzero channel IDs with no reuse during a session;
- bounded frame payloads fragmented independently of daemon protocol frames;
- a hard logical-channel maximum derived from the maximum supported native
  Window topology and its per-pane task count, with admission failure before
  exceeding that bound;
- bounded per-channel and aggregate queued bytes;
- fair scheduling so one output-heavy pane cannot starve control/topology
  channels;
- explicit half-close and close semantics;
- no parsing or rewriting of private daemon protocol payloads;
- channel-local failure where safe and session-wide failure for corrupt outer
  framing or violated aggregate bounds;
- SSH EOF closes every daemon connection;
- daemon channel EOF closes only the corresponding logical channel unless the
  validated daemon process itself exits; and
- no reconnect, retry, or implicit controller reacquisition.

The remote relay performs the existing socket, UID, mode, peer PIDFD, and exact
adjacent daemon executable checks for every opened channel. All daemon socket
peers still see the exact installed `splinterm-relay` executable.

### SSH process construction

Use `tokio::process::Command` or an equivalent direct process API. Never build a
local shell command string.

The safety-critical invocation includes fixed arguments equivalent to:

```text
ssh
  -T
  -o StrictHostKeyChecking=yes
  -o ClearAllForwardings=yes
  -o PermitLocalCommand=no
  -o RequestTTY=no
  -o EscapeChar=none
  [closed validated profile options]
  DESTINATION
  /usr/bin/splinterm relay --graphical-stdio
```

The remote command is one fixed application-owned literal. No terminal value,
profile field, hostname, username, path, or user-supplied option may alter it.
OpenSSH may transmit it through the remote account's normal command execution
path, but Splinterm never interpolates values into it.

The implementation must inspect actual OpenSSH behavior for conflicts with
`RemoteCommand`, `LocalCommand`, control sockets, and command-line overrides.
Tests use a fake SSH executable and record exact argv. Do not rely on quoting
assertions alone.

### SSH stderr and lifecycle

SSH stdout contains only graphical-relay bytes. SSH stderr is drained
concurrently into a bounded diagnostic ring and may also be presented through a
sanitized local error surface. It must never be merged into stdout or allowed to
fill a pipe and deadlock the transport.

The `RemoteSession` owns:

- the SSH child;
- child stdin, stdout, and stderr tasks;
- multiplexer reader/writer tasks;
- logical-channel routing state;
- cancellation and one terminal failure result; and
- asynchronous child termination and reaping.

Dropping the final session owner closes stdin, allows a short graceful exit,
then terminates and reaps a stuck child. No zombie, orphaned local SSH process,
or stale local runtime socket may remain. Closing it does not issue remote
Splint termination requests.

## Remote profile schema

Add a dedicated strict configuration file rather than expanding the loose
Foot-compatible INI parser with arbitrary SSH options:

```text
~/.config/splinterm/remotes.toml
```

Initial schema:

```toml
version = 1

[remotes.wintermute]
host = "wintermute"
# Optional; otherwise OpenSSH config/default chooses it.
user = "oldjobobo"
# Optional; default 22 or OpenSSH config.
port = 22
# Optional closed list; paths resolve locally.
identity_files = ["~/.ssh/id_ed25519"]
# Optional; default follows OpenSSH's normal known-hosts configuration.
known_hosts_file = "~/.ssh/known_hosts"
connect_timeout_seconds = 15
```

Rules:

- `serde(deny_unknown_fields)` or equivalent applies at every level;
- file and profile counts, strings, paths, and identity-file counts are bounded;
- profile names are stable local identifiers and cannot contain whitespace,
  separators, control characters, bidi controls, or leading `-`;
- host, user, and destination tokens cannot contain control characters,
  whitespace, shell metacharacter interpretation, or leading `-` ambiguity;
- ports and timeouts have conservative closed ranges;
- configured paths are local paths, expanded without invoking a shell, and
  rejected when ambiguous or unsafe;
- no `remote_command`, `proxy_command`, `local_command`, forwarding, arbitrary
  `options`, environment injection, or shell fragment field exists;
- an explicitly configured unreadable file or identity is an actionable error;
- `~/.ssh/config` remains usable for ordinary host aliases and supported proxy
  routing, but safety-critical command-line options override conflicting values;
- the implementation documents that administrator/user SSH configuration can
  still affect routing and authentication; and
- profile inspection prints resolved non-secret settings and exact redacted argv
  without reading or displaying private-key contents.

Add non-graphical commands:

```bash
splinterm remote list
splinterm remote inspect wintermute
splinterm remote check wintermute
```

`check` validates configuration, invokes the fixed relay, negotiates the
multiplexer and automation handshake, performs bounded non-mutating reachability
and topology-read probes, reports any policy denial encountered, and exits
without mapping a Window or mutating topology. It must not claim to enumerate
all resource-dependent authority from a single probe.

## Authentication behavior

### Agent-backed keys, certificates, and hardware keys

OpenSSH uses its normal identity and agent selection. Splinterm never requests
agent forwarding. Hardware touch and PIN behavior remain owned by OpenSSH and
its configured provider.

### Password and passphrase prompts from a terminal

Do not force `BatchMode=yes` on the one initial SSH process. When Splinterm has a
controlling terminal, OpenSSH may prompt through local `/dev/tty` even though
child stdin/stdout carry relay bytes. The session picker maps only after
transport and protocol negotiation succeed.

The user enters a password or key passphrase once for the complete local remote
client lifetime.

### Desktop launch and SSH askpass

When no controlling terminal exists, support OpenSSH's standard local
`SSH_ASKPASS` mechanism. Splinterm does not pass a password through argv,
configuration, environment values, relay frames, or logs.

First-release behavior:

- honor an explicitly configured environment/provider selected by the local
  desktop session;
- set the documented OpenSSH askpass requirement only when no controlling
  terminal is available;
- verify the askpass executable is an executable local file before launch;
- bound and sanitize any prompt text shown in Splinterm-owned diagnostics; and
- fail promptly and clearly when interactive authentication is required but no
  terminal or askpass provider is available.

A built-in native Splinterm askpass dialog is a separate security-reviewed
follow-up unless implementation review proves an external provider cannot
satisfy the supported Omarchy desktop workflow. Do not add an ad hoc password
field to the session picker.

### Host keys

Strict host-key verification is always enabled. Unknown and changed keys fail
closed. Initial trust is established explicitly through normal OpenSSH tooling
or a separately reviewed future onboarding UI. Splinterm does not run
`ssh-keyscan`, append `known_hosts`, or present “accept anything” behavior.

Required errors distinguish at least:

- unknown host key;
- changed host key;
- DNS/routing/connect timeout;
- no accepted authentication method;
- interactive authentication unavailable;
- remote command missing or package version incompatible;
- remote daemon unavailable;
- relay executable/daemon identity validation failure;
- graphical multiplexer version mismatch;
- private daemon protocol version mismatch; and
- Splinterm policy denial or insufficient scope.

## Remote-safe graphical behavior

### Session discovery and recency

The Recent Sessions picker reads the selected remote daemon's Lairs and Dojos.
Persist local recency under a namespace derived from the exact local profile
identity, not remote-provided names. Renaming a profile intentionally creates a
new recency namespace unless an explicit migration is implemented.

Remote CWD and title strings remain untrusted display data and pass through the
existing picker sanitization and bounds.

### Creating remote processes

Remote graphical creation must not send local `AppConfig.shell`, local process
CWD, or other local host paths through trusted-local `LaunchParameters`.

Use the existing automation variants:

- `CreateLairAutomation`;
- `SplitSplintAutomation`;
- `RelaunchSplintAutomation`; and
- `NewDojoAutomation`.

Default remote creation uses `AutomationLaunch { cwd: None, argv: [] }`, causing
the remote daemon to select its configured/default shell.

For pane and Dojo creation from an existing remote view, an inherited CWD must
come from an exact captured remote Splint identity and its remote CWD. An
explicit `--cwd` under `--remote` is documented as a path on the remote host.
It is not canonicalized or existence-checked locally. Direct command argv is
transported as structured argv and never rebuilt as a shell string.

Any UI action whose implementation has only trusted-local launch semantics must
be disabled remotely until converted. It must not silently fall back to local
semantics.

### Focus, control, and consent

Do not send `PublishGraphicalFocus` over a remote endpoint. It is trusted-local
state and does not describe compositor focus on the remote host.

Ordinary policy-authorized controller acquisition, transfer requests, input,
resize, release, and control-status subscriptions remain available. Forced
trusted-UI transfer and remote trusted consent are not implied. If another
client owns a controller, the remote Window follows the existing ordinary
request/deny/accept workflow within granted policy.

Remote operation requires persistent policy. It must not depend on a remote
Wayland consent prompt or silently enable development terminal access.

### Policy recipe

Documentation must provide a reviewed policy template for the exact installed
`/usr/bin/splinterm-relay` digest. The complete interactive workflow may require:

- topology metadata read and subscribe;
- terminal visible read and subscribe;
- scrollback read and search;
- controller acquire and transfer;
- input and resize;
- authorization inspect;
- topology layout and name mutation; and
- process spawn, restore, or terminate only when the operator deliberately
  enables the corresponding UI actions.

Provide a read-only profile first and a full interactive profile second. Do not
use wildcard resources or silently broaden scopes to make tests pass. State
clearly that every SSH caller able to execute the relay under that Unix account
receives the relay executable's configured Splinterm authority. Recommend a
dedicated account or administrator-owned forced-command/restricted-key setup
when that delegation is too broad.

### Images

Remote snapshots and updates render text and safely omit image pixels. The
remote endpoint does not request image bodies, connect to a local `.content`
socket, interpret remote file/SHM references, or weaken the existing trusted
image plane. Remote image transport remains a separate future security design.

## Execution plan and approval model

Implementation proceeds autonomously through three non-graphical phases. Their
checks are engineering acceptance criteria, not user confirmation gates. The
parent implementation agent may read, edit, run non-graphical tests, apply
review findings, and continue between them without asking for approval.

There is one planned user approval boundary: the real-host graphical validation
in Phase 4. Outside that boundary, request user input only if work encounters a
genuine product/security decision, destructive action, publication, or material
scope/cost increase listed under Stop gates.

### Phase 1 — transport and authentication foundation

Deliver as one dependency-ordered implementation phase:

1. Correct the PRD and architecture contract, record the Foot + SSH + tmux user
   story, and add an ADR for one-authentication graphical relay multiplexing.
2. Lock the existing raw `relay --stdio` behavior and define the separate fixed
   `relay --graphical-stdio` mode.
3. Add strict `remotes.toml` parsing, endpoint resolution, redacted profile
   inspection, and pure SSH argv/environment construction.
4. Implement the bounded graphical relay multiplexer: version negotiation,
   logical channel open/data/half-close/close, fairness, per-channel and
   aggregate bounds, daemon socket validation, and session cleanup.
5. Refactor `splinterm-automation-client::Connection` to accept split async
   transports while preserving the current local Unix constructor and trusted
   image behavior.
6. Implement one authenticated `RemoteSession` owning the SSH child, bounded
   stderr drain, multiplexer tasks, cloneable logical-channel factory,
   cancellation, termination, and child reaping.
7. Support agent/key authentication, terminal password/passphrase prompting,
   standard `SSH_ASKPASS`, strict host keys, and categorized failures.
8. Add `splinterm remote list`, `remote inspect`, and the non-mutating
   `remote check` command.

Internal acceptance checks:

- parsers reject unknown, ambiguous, unbounded, or unsafe profile values;
- fake SSH records exact structured argv and proves no shell interpolation;
- one fake SSH process serves multiple simultaneous logical channels and causes
  only one authentication interaction;
- outer framing handles fragmentation, coalescing, queue overflow, channel EOF,
  session EOF, daemon death, cancellation, and cleanup;
- raw relay tests remain unchanged and passing;
- local automation-client behavior remains unchanged; and
- untrusted transports cannot retrieve image content.

Fresh read-only architecture/security review occurs after this coherent phase.
Actionable in-scope findings are fixed by the parent without another user gate.

#### Phase 1 implementation record — 2026-08-07

Implemented strict profiles and inspection, exact OpenSSH planning, separate raw
and graphical relay dispatch, bounded/versioned outer framing, derived channel
admission, permit-backed per-channel and aggregate byte accounting, round-robin
data scheduling, ordered half-close/close, repeated daemon executable
validation, transport-neutral daemon connections, one-child `RemoteSession`,
bounded stderr and diagnostics, terminal/askpass handling, startup and operation
deadlines, child termination/reaping, categorized failures, explicit endpoint
capabilities, and non-mutating `remote check`.

Retained tests cover corrupt/fragmented/coalesced framing, channel/queue bounds,
client and relay fairness, half-close and EOF, validated daemon death, raw relay
compatibility, split transport cancellation and no-image behavior, strict
profiles, exact fake-SSH argv, one fake SSH process serving simultaneous daemon
connections, stalled negotiation timeout/reap, and read-only check probes.
Affected-crate formatting, strict Clippy, protocol, relay, automation-client,
Splinterm library/binary, and remote integration tests pass. Two fresh read-only
reviewers reported aggregate-accounting/fairness and timeout/bidi findings; the
parent fixed all four and the second/final review round found no remaining
blocker or fix worth doing now.

Phase 1 completion does not claim native remote Window routing or real-host
validation. Those remain Phases 2 and 4 respectively.

### Phase 2 — complete native remote workflow

Deliver as one product phase:

1. Route global `--remote PROFILE` through a clonable endpoint factory and make
   `splinterm --remote PROFILE` open the remote Recent Sessions picker.
2. Thread the endpoint through sessions, windows, pane bridge, topology manager,
   and every current direct `Connection::connect()` site.
3. Namespace recent Dojos by stable local profile identity.
4. Implement remote topology/session reads, terminal attach, text snapshots,
   ordered updates, resynchronization, scrollback, and search.
5. Disable trusted-only remote graphical-focus publication and make remote image
   transport explicitly unavailable.
6. Implement ordinary controller acquisition/status/transfer, input, resize,
   release, pane focus, tabs, and hidden-tab synchronization.
7. Convert remote New, launch, split, new Dojo, and relaunch behavior to the
   existing automation request variants and remote-safe `AutomationLaunch`.
8. Inherit CWD only from exact captured remote state; treat explicit remote
   `--cwd` as a remote path and never send local shell/CWD defaults.
9. Make rename, ratio, close, restore, and termination actions endpoint- and
   policy-aware; trusted-only actions remain visibly unavailable.
10. Package both relay modes and update profiles, authentication, policy,
    disconnect, troubleshooting, image, README, CLI, architecture, PRD, and
    roadmap documentation.

Internal acceptance checks:

- fake remote sessions cover read-only, interactive, and denied policy profiles;
- exact Splint/incarnation/controller identities bind input and resize;
- channel loss cannot retarget another pane;
- disconnect releases subscriptions/controllers while remote Splints remain
  running;
- incompatible fake local/remote homes prove local shell and CWD never leak into
  default remote launches;
- one Window remains bound to one endpoint;
- remote endpoints never request image bodies or open a `.content` socket; and
- local trusted graphical behavior remains unchanged.

Fresh read-only implementation/security review occurs after this coherent
phase. Actionable in-scope findings are fixed and revalidated by the parent
without another user gate.

### Phase 3 — autonomous non-graphical closure

Run focused checks throughout Phases 1 and 2, then run the complete closure set:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p splinterm-protocol
cargo test -p splinterm-automation-client
cargo test -p splinterm-relay --lib
cargo test -p splinterm-relay --test stdio
cargo test -p splinterm --lib
cargo test -p splinterm --test automation_cli
cargo test -p splinterd --test end_to_end -- --test-threads=1
cargo test --workspace --all-targets --all-features -- --test-threads=1
git diff --check
```

Add the shared graphical-relay codec crate to focused commands if Phase 1
introduces one.

Phase 3 completion evidence (2026-08-07): the exact formatting and workspace
strict-Clippy commands passed; every focused protocol, automation-client, raw
relay, graphical-relay, Splinterm library, and automation CLI suite passed; all
16 serialized daemon end-to-end tests passed; the exact serialized
workspace/all-targets/all-features matrix passed with only the documented manual
pane timing harness ignored; `git diff --check` passed; and one fresh read-only
closure review found no behavioral blocker. Its two local lint-allow reason
findings were applied and revalidated. No real SSH host or graphical Window was
used.

Retain evidence for exact fake SSH argv, key/agent behavior, one password or
askpass interaction per session, host-key/authentication failures, multiplexer
bounds/fairness, logical-channel concurrency, policy denials, protocol mismatch,
child/task/channel reaping, remote no-image behavior, unchanged local trusted
behavior, and unchanged raw automation relay behavior.

This phase requires no user confirmation. Diagnose failures before bounded
retries, apply accepted review findings, and continue until the non-graphical
acceptance boundary passes or a Stop gate is actually triggered.

### Phase 4 — single user-approved real-host graphical validation

This is the only planned approval gate. Before running it, request one approval
for the complete bounded sequence required by `AGENTS.md`, identifying the
remote host, target Window, permitted focus/input actions, guarded smoke,
remaining matrix, and cleanup plan.

After approval, run one guarded smoke and continue through the approved matrix
without asking again if the smoke succeeds:

1. read-only `remote check`;
2. agent-backed key authentication;
3. password authentication from a controlling terminal;
4. desktop `SSH_ASKPASS` authentication when a supported provider is installed;
5. unknown and changed host-key rejection using isolated known-hosts fixtures;
6. remote session picker and existing Dojo open;
7. multi-pane text output and ordered updates;
8. input, resize, scrollback, search, and normal controller transfer;
9. remote split/new Dojo/new Lair using remote shell and CWD semantics;
10. local Window close followed by proof that remote Splints remain running;
11. reconnect and reopen;
12. SSH kill, relay kill, and daemon loss behavior;
13. local graphical regression smoke; and
14. cleanup, focus/workspace restoration, and remote test topology removal only
    when that destructive cleanup was included in the approval.

Abort on wrong-target input, focus/placement failure, unexpected password
repetition, remote process loss, or cleanup failure. Completion requires
recorded evidence and independent review, not additional progress approvals.

#### Phase 4 Holodeck attempt — 2026-08-07

The approved guarded sequence used `oldjobobo@holodeck` and isolated workspace 8
on DP-2. Commit `3475725` supplied the feature, followed by packaging
compatibility commits `25e4ef2` and `efebc7e`. The validated Arch package was
installed on an initially empty x86_64 Arch/Omarchy host. The deployment exposed
and fixed the package's unnecessarily new-only JetBrains Nerd Font dependency;
Pacman integrity, adjacent executable identities, daemon startup, exact relay
digest policy, and an agent-backed `remote check` then passed.

Recorded successful evidence:

- unknown and changed host keys failed closed with distinct diagnostics against
  isolated known-hosts fixtures;
- one native remote Window mapped only on workspace 8 / DP-2 and was selected by
  exact Hyprland address before focus or input;
- the remote shell rendered ordered text, accepted input, and reported
  `/home/oldjobobo` and `/bin/bash`, proving remote CWD/shell semantics;
- 200 ordered output lines, detached scrollback, and compositor-driven Window
  resize rendered correctly;
- closing the local Window ended its client/SSH lifetime while the exact remote
  shell PID remained alive; and
- the raw relay accepted an exact `SPGR` v1 Hello and returned the byte-perfect
  16-byte HelloAck.

The reconnect acceptance boundary failed. The first reopen attempt reported an
invalid graphical-relay magic. A direct exact-frame probe then ruled out remote
shell/banner contamination. The one bounded retry reached the relay but timed
out waiting for a logical channel or daemon handshake. A bounded audit query
reported retained IDs through `1344`, but captured only the first page
(`321–384`), whose timestamps belong to the successful Window's normal topology
polling. That page contained no denials and does not identify which reconnect
stage stalled; no stronger audit claim is retained. The matrix was aborted before
password-only/SSH_ASKPASS authentication, split/new Dojo/new Lair, controller
transfer, SSH/relay/daemon-loss, and local graphical-regression cases. Synthetic
Ctrl-shortcut injection was also inconclusive; physical-key search validation
was not claimed.

Cleanup removed every test Lair/Splint, uploaded package fixture, temporary host
keys, test windows, and local launch scripts; restored the original workspace
and focus; and left Holodeck with the validated package, active empty daemon,
normal local `holodeck` profile, and a narrow daemon-only exact-digest relay
bootstrap policy. Holodeck retained the reversible reset backup at
`/home/oldjobobo/.local/state/splinterm.reset-1786150469754`. Phase 4 is not
complete and the feature must not be described as reconnect-validated.

## Stop gates

Stop and request a product/security decision if implementation would require:

- storing or caching SSH passwords or private-key passphrases;
- accepting unknown host keys automatically;
- arbitrary profile-supplied SSH options or remote commands;
- adding a daemon network listener;
- weakening exact relay/daemon executable identity;
- exposing image bodies or trusted image metadata remotely;
- granting trusted-UI role over SSH;
- silently broadening persistent policy;
- invoking a remote shell to rebuild command argv;
- sending local shell paths or CWD as default remote launch state;
- changing raw `relay --stdio` compatibility;
- mixing endpoints in one Window before a separate design; or
- retrying/reconnecting in a way that could reacquire control or repeat a
  mutation ambiguously.

Stop and diagnose before retrying any failed real-host, aggregate, expensive, or
graphical command.

## Acceptance criteria

The feature is complete only when all of the following are demonstrated:

1. `splinterm --remote PROFILE` authenticates once and presents the selected
   remote daemon's sessions in a local native Window.
2. Agent-backed keys work through normal OpenSSH selection without Splinterm
   reading key material.
3. A password-only user receives one local terminal or `SSH_ASKPASS` prompt for
   the remote-client lifetime, not one prompt per pane or daemon connection.
4. Unknown or changed host keys fail closed and are distinguishable from user
   authentication failure.
5. One SSH child carries the independent daemon connections required by a
   supported multi-pane/tab Window without depending on OpenSSH `MaxSessions`.
6. Remote connections negotiate automation role and never receive trusted local
   UI bypass.
7. Policy-authorized text output, panes, tabs, input, resize, scrollback, search,
   and ordinary controller ownership work against exact remote identities.
8. New remote terminals use remote daemon defaults; local shell and CWD do not
   leak into default remote launches.
9. Closing or crashing the local client, SSH, or relay leaves remote Splints
   running while releasing connection-owned subscriptions and controllers.
10. SSH failure, relay failure, daemon loss, policy denial, channel failure,
    multiplexer mismatch, and daemon protocol mismatch fail clearly without
    corrupting local or remote state.
11. Remote endpoints never request image bodies or open a local image-content
    socket.
12. Raw SSH automation relay behavior and all local trusted graphical behavior
    remain unchanged.
13. Profiles, authentication, policy delegation, host-key setup, disconnect
    semantics, and image limitations are documented accurately.
14. Non-graphical evidence, operator-gated graphical evidence, and independent
    review are recorded before the feature is described as implemented or
    validated.

## Documentation updates

At implementation milestones, update:

- `docs/PRD.md` — add a P0 native remote graphical requirement distinct from
  remote automation and mark its state honestly;
- `docs/architecture.md` — show local Unix, remote SSH, graphical relay
  multiplexer, automation role, and remote daemon channels;
- `docs/remote.md` — cover profiles, authentication, fixed graphical command,
  policy, failure, disconnect, and image behavior while retaining raw automation
  relay documentation;
- `docs/configuration.md` — document `remotes.toml` and strict validation;
- `docs/automation.md` — preserve the distinction between public automation and
  native remote graphical use;
- `README.md` and user-facing CLI documentation — add the basic remote sessions,
  reopen, window, and launch workflow;
- `docs/roadmap.md` — track native remote graphical delivery separately from the
  already complete SSH automation relay; and
- packaging documentation — state remote package/version and adjacent executable
  requirements.

Do not claim the feature is implemented or validated until its corresponding
code, retained evidence, and independent review exist.
