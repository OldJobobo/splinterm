# Plan 0031: MCP visual demo and acceptance harness

- **Status:** Proposed
- **Date:** 2026-08-11
- **Scope:** reusable MCP stdio driver, isolated daemon topology, visible graphical demonstration, human consent, evidence, and cleanup
- **Related:** [Plan 0007](0007-phase4-mcp-adapter.md), [Plan 0030](0030-client-diagnostic-logging.md), [ADR 0005](../adr/0005-trusted-consent-broker.md), [`docs/mcp.md`](../mcp.md), and [Spike 0014](../spikes/0014-trusted-consent-and-control.md)
- **Origin:** Pi session `019fec31-4935-729f-afd3-6d3fcdf94912` on 2026-08-10

## Decision

Add a repository-owned harness that can demonstrate Splinterm MCP automation in
one visible, disposable graphical session without touching the user's normal
daemon topology or persistent policy.

The harness has two explicit lanes:

1. a default non-graphical preflight that validates executable identity, MCP
   lifecycle/discovery, deny-all behavior, protocol bounds, and cleanup; and
2. an opt-in graphical run that places one isolated demo window and its trusted
   consent window on workspace 8 / DP-2, pauses for real human approval, then
   performs a bounded visible automation workflow.

The graphical lane is not an ordinary automated test. Running it requires the
single complete graphical approval defined by `AGENTS.md`, begins with one
smoke case, and proceeds to the matrix only if the smoke succeeds. The harness
must never synthesize or infer consent, press the consent key for the user, or
use terminal output as an instruction.

The first implementation target is:

```text
tools/mcp/run-demo.py preflight
tools/mcp/run-demo.py graphical
```

A successful graphical run visibly proves:

```text
isolated Splinterm window
  -> MCP initialization and fixed capability discovery
  -> one trusted Lair-scoped consent prompt
  -> explicit human grant
  -> exact controller takeover
  -> fixed visible input sentinel
  -> terminal resize and read-back
  -> topology subscription and one bounded split/rename workflow
  -> controller release
  -> grant revocation and post-revocation denial
  -> inspected cleanup with no residual daemon, socket, process, or window
```

## Problem

Splinterm already has strong but separate MCP validation surfaces:

- `crates/splinterm-mcp/tests/stdio_protocol.rs` exercises black-box stdio;
- `crates/splinterm-mcp/tests/schema_inventory.rs` and `tests/mcp/fixtures/`
  freeze the public contracts;
- `tools/package/validate-mcp-package.py` exercises an extracted package against
  an isolated real daemon; and
- MCP Inspector evidence under `docs/spikes/artifacts/0022-mcp-package/` proves
  external lifecycle and schema interoperability.

Those surfaces do not provide one repeatable demonstration a person can watch.
The 2026-08-10 Pi session assembled a one-off visual sequence, but its consent
client exited with `wayland_failure` before mapping. The attempt also exposed
several harness weaknesses:

- protocol transcript success was initially mistaken for a visual demo;
- focus and physical monitor visibility were conflated;
- consent-window placement depended on focus rather than exact window identity;
- the one-off scripts closed stdin before one response arrived;
- an early probe attempted several lifecycle failures in one MCP process even
  though a pre-initialization violation correctly terminates that process;
- installed component/tool-count skew was not rejected up front; and
- no reusable state machine, evidence manifest, or cleanup attestation remained.

The product behavior should not be redesigned around a demo. The missing piece
is a small host-side orchestrator that composes existing supported interfaces,
checks every identity and state transition, and fails closed.

## Goals

1. Provide a calm, human-readable demonstration of what MCP can observe and
   control.
2. Exercise the installed package or a coherent staged source build without
   modifying `/usr/bin`, Pacman state, user policy, or normal daemon topology.
3. Prove that the trusted consent surface is visible, correctly targeted, and
   genuinely approved by a person.
4. Reuse the same MCP host driver for non-graphical package validation where
   practical, avoiding a second drifting protocol client.
5. Record bounded machine-readable evidence for every phase, including failed
   preflight, denial, abort, and cleanup.
6. Make reruns deterministic enough to diagnose product failures rather than
   harness races.

## Non-goals

- replacing the black-box stdio, schema, real-daemon, package, Inspector, or
  conformance suites;
- granting unattended authority through interactive consent;
- adding HTTP, OAuth, prompts, sampling, elicitation, filesystem, clipboard, or
  arbitrary shell-string MCP surfaces;
- modifying persistent Hyprland, Omarchy, terminal, or Splinterm configuration;
- testing native Wayland focus, move, or resize as an MCP capability;
- driving an existing user terminal or production daemon;
- treating a screenshot as protocol or authorization evidence;
- automatically approving the trusted consent prompt; or
- silently retaining demo topology after the run.

## User-facing workflow

### Non-graphical preflight

`preflight` performs no Wayland or window action. It:

1. resolves the selected `splinterm`, `splinterd`, and `splinterm-mcp` binaries;
2. records canonical path, device/inode, SHA-256, package/build version, and
   adjacency;
3. creates a unique private temporary root;
4. starts an isolated daemon with an isolated socket, home, config, runtime, and
   state directory and with no persistent automation policy;
5. starts the MCP adapter against that socket;
6. initializes exactly MCP `2025-11-25` and sends
   `notifications/initialized`;
7. records the discovered catalog count but validates required tool names and
   schemas instead of pinning the historical 32/33 count in the harness;
8. proves `splinterm.ping` succeeds and an unauthorized topology read fails
   closed;
9. runs protocol-negative cases in separate adapter processes; and
10. closes stdin, awaits bounded clean exit, stops the isolated daemon, removes
    only the owned temporary root, and writes a cleanup record.

The required catalog subset is:

```text
splinterm.ping
splinterm.inspect_topology
splinterm.request_lair_access
splinterm.acquire_control
splinterm.input
splinterm.resize
splinterm.read_terminal
splinterm.release_control
splinterm.revoke_access
splinterm.split_splint
splinterm.rename_splint
```

Additional tools remain discoverable and are recorded, but do not make the
harness fail merely because a reviewed catalog addition changes the count.
Removing or incompatibly changing a required tool does fail preflight.

### Graphical approval contract

Before `graphical` starts, the operator or supervising agent states and receives
approval for one bounded sequence containing:

- target: only harness-created windows on workspace 8 / DP-2;
- smoke: one normal demo window plus one trusted consent window;
- permitted actions: exact-address placement, optional focus of those windows,
  MCP input into the owned demo Splint, terminal resize, split/rename, controller
  release, grant revocation, and owned-window cleanup;
- matrix: the workflow below, conditional on smoke success; and
- cleanup: terminate only the isolated daemon and owned clients, remove the
  private root, restore original focus/workspace/cursor state where changed,
  and attest workspace 8 contains no harness window.

Approval is execution-scoped. A previous run or this plan is not approval to
manipulate the desktop.

### Graphical smoke

The harness first records:

- Hyprland version, instance signature, monitors, workspaces, clients, and active
  window;
- DP-2 geometry, transform, scale, and active workspace;
- current pointer position when available; and
- whether workspace 8 contains any unrelated window.

It refuses to start if DP-2 or workspace 8 cannot be identified, workspace 8
contains an unrelated window, or exact window selection cannot be guaranteed.
It does not move an unrelated window out of the way.

The smoke then:

1. starts the same isolated daemon arrangement proven by `preflight`;
2. launches one persistent native Lair named `MCP Visual Demo <short nonce>`
   through the selected adjacent `splinterm` binary;
3. observes the newly mapped PID/address and exact Lair/Dojo/Splint identities;
4. places only that exact address on workspace 8 / DP-2 without initial focus;
5. verifies process PID, executable device/inode, app ID, address, workspace,
   monitor, and topology identity;
6. starts one MCP request for Lair-scoped access; and
7. waits for the new consent client mapped by the isolated daemon's adjacent
   `splinterm` binary.

The consent window is selected by a conjunction of fresh evidence: new PID,
process ancestry under the isolated daemon, executable device/inode, app ID,
trusted title, and mapping time. Title alone is never authority. The harness
places only that exact address on workspace 8 / DP-2 and prints a clear prompt:

```text
Trusted Access Request is visible on DP-2.
Review the requester, Lair, and scopes.
Approve or deny it yourself; the harness will not send the decision key.
```

Requested smoke scopes are the minimum needed for the visible lane:

```text
topology_metadata_read
terminal_visible_read
input
resize
controller_transfer
```

A user denial is a valid fail-closed product outcome and produces a clean
`denied` report. It is not rewritten as infrastructure failure. Timeout,
authentication failure, `wayland_failure`, a wrong or absent consent window, or
a window on the wrong monitor fails the smoke and blocks the matrix.

### Visible matrix

After a real grant, the harness performs the following ordered steps against the
captured exact Lair/Splint/incarnation only:

1. **Observe:** inspect authorized topology and read the initial terminal state.
2. **Take control:** acquire exact input/resize control with explicit approved
   takeover because the graphical client may own the controller.
3. **Visible sentinel:** send one fixed, non-user-derived shell input string that
   prints a conspicuous line such as `SPLINTERM MCP CONTROL ACTIVE <nonce>`.
4. **Resize:** set a bounded known terminal grid size, then send fixed `stty size`
   input and read back the resulting semantic terminal state. The report calls
   this terminal-grid resize, not native Wayland window resize.
5. **Subscribe:** subscribe to `splinterm://topology` and record the next ordered
   update.
6. **Topology workflow:** after a second consent prompt or an initial explicitly
   approved expanded scope set, split the demo Splint with fixed structured argv,
   rename only the created child, and verify the topology notification and
   visible tab/pane state. No shell command string is built from user input.
7. **Read-back:** read the target terminal and verify only the fixed nonce/hash,
   provenance, revision, and trust label; do not retain arbitrary terminal text.
8. **Release:** release controller ownership and verify the control resource no
   longer reports local ownership.
9. **Revoke:** revoke the exact ephemeral grant and prove a fresh terminal read
   fails as `unauthorized`.
10. **Inspection checkpoint:** leave the isolated demo visible until the user
    selects Finish or a bounded timeout expires, then clean up.

The expanded topology workflow requires these additional consent scopes:

```text
topology_layout_mutate
topology_name_mutate
process_spawn
```

The implementation must choose one of two explicit UX contracts before the
first graphical acceptance run:

- request all smoke and matrix scopes in one clearly reviewed prompt; or
- use a minimal smoke prompt followed by a second clearly labeled matrix prompt.

It must not silently broaden the first request. The recommended default is two
prompts so the smallest visible control demonstration remains independently
useful.

## Binary and runtime modes

### Installed mode

Installed mode is the default graphical authority. It uses the coherent adjacent
runtime set containing the selected `splinterd` executable, normally
`/usr/bin/splinterm`, `/usr/bin/splinterd`, and `/usr/bin/splinterm-mcp`.

It does not install, replace, or modify those files. It launches a second
isolated daemon process with unique XDG directories and socket while inheriting
the minimum existing Wayland/session environment required to map the test
windows. The user's normal `splinterd.service` and topology remain untouched.

Preflight rejects:

- a shadowing `PATH` client;
- non-adjacent trusted UI binaries;
- package/build version skew;
- a running isolated daemon whose executable identity differs from the recorded
  selection; or
- a required MCP catalog/schema mismatch.

### Staged source mode

`--source` builds or accepts one coherent staged directory containing sibling
`splinterm`, `splinterd`, and `splinterm-mcp` binaries from the same source
identity. It never points a development client at the production daemon and
never claims installed-package validation.

The evidence records Git commit, dirty-state flag, patch digest when dirty,
compiler version, and each binary digest. Source mode must not use
`SPLINTERM_ENABLE_DEV_ATTACH` to bypass the consent behavior under test.

A build failure or filesystem quota failure is diagnosed before one bounded
retry. The harness never falls back silently from staged source mode to installed
mode.

## Harness architecture

### Shared MCP host driver

Extract or reuse a small Python stdio client with:

- newline-delimited JSON framing;
- monotonic request IDs;
- initialization state;
- response-ID matching while retaining notifications;
- per-request deadlines;
- exact process/EOF cleanup;
- bounded stdout/stderr capture;
- cancellation support; and
- redacted structured event output.

`tools/package/validate-mcp-package.py` should use this driver once parity is
proven. Refactoring the package validator must not weaken its existing package
layout, policy allow/deny, controller, subscription, or cleanup assertions.

Every lifecycle-invalid scenario starts a fresh MCP process. The driver does not
send later messages after an expected terminal protocol violation.

### Explicit state machine

The runner persists one state file after every transition:

```text
created
preflight_passed
daemon_ready
demo_mapped
consent_mapped
consent_granted | consent_denied
controller_acquired
sentinel_verified
matrix_complete
revoked
inspection
cleaning
clean
```

A transition records timestamp, process IDs, exact public resource IDs,
incarnation, topology/terminal revisions where relevant, and a fixed result
code. It does not record controller handles, transfer handles, capability data,
input bodies, policy bodies, arbitrary terminal text, environment values, or
private daemon protocol IDs.

On startup, a run owns a cryptographically random nonce and a private directory
mode `0700`. Every socket, process, window, and artifact must be attributable to
that nonce before cleanup can act on it.

### Window coordination

Use current Hyprland 0.55+ JSON inspection and Lua dispatchers. Do not use legacy
hyprlang commands or interpolate terminal-controlled title text into Lua.

For every window action:

1. refresh `hyprctl -j clients`;
2. select by exact process and harness identity;
3. require one match;
4. target the fresh address;
5. perform the least action; and
6. re-query the postcondition.

Prefer event-socket observation over blind sleep/poll loops. A bounded readiness
backoff may be used for daemon/socket startup, but window identity comes from
compositor events plus a fresh JSON snapshot.

The harness creates no persistent Hyprland rule. Temporary placement must be
process/address-specific and removed during cleanup even after failure.

### Diagnostics

When consent does not map or exits early, the harness gathers only bounded,
sanitized evidence:

- isolated daemon exit/status and fixed diagnostic codes;
- `splinterm diagnostics --last-exit` from the isolated state root where
  available;
- exact child status and process ancestry;
- Hyprland clients/events around the expected mapping; and
- socket/process cleanup state.

It does not discard the trusted consent client's failure behind a generic
`consent_denied` conclusion. It also does not expose private capability frames
or raw terminal/error bodies. A failed expensive graphical attempt is not
repeated until the failure is diagnosed and the next attempt is stated.

## Cleanup contract

Cleanup runs on success, denial, timeout, assertion failure, Ctrl+C, MCP EOF, or
supervisor termination where the process can still execute handlers.

In order, it:

1. stops accepting new harness actions;
2. best-effort releases any owned controller;
3. best-effort revokes the exact ephemeral grant;
4. closes MCP stdin and awaits bounded adapter exit;
5. requests graceful shutdown of the isolated daemon;
6. verifies every owned child and descendant exited, then escalates only against
   exact owned PIDs if the grace period expires;
7. verifies the isolated socket is absent;
8. verifies no window matching an owned PID/address remains;
9. removes temporary Hyprland placement state;
10. restores original focus/workspace/pointer state where the approved sequence
    changed it and the original target still exists;
11. verifies unrelated windows, production daemon identity, and production
    topology are unchanged; and
12. removes only the nonce-owned temporary root after exporting the evidence
    selected for retention.

The runner never kills by process name, broad app ID, title, workspace, or user.
If exact cleanup cannot be proven, it stops destructive cleanup, reports the
remaining exact identities, and does not claim success.

## Evidence layout

Each run writes to a caller-selected directory or, by default, a private
runtime directory that is summarized before deletion:

```text
mcp-demo-<UTC>-<nonce>/
  manifest.json
  state.json
  report.json
  summary.md
  identities.json
  mcp-events.jsonl
  hyprland-before.json
  hyprland-after.json
  cleanup.json
  diagnostics/
  screenshots/          # only when graphical capture was approved
  SHA256SUMS
```

The final report distinguishes:

- `passed`;
- `denied_by_user`;
- `preflight_failed`;
- `consent_ui_failed`;
- `workflow_failed`;
- `cleanup_incomplete`; and
- `aborted`.

Screenshots are local evidence and are not committed automatically. The consent
prompt contains requester identity and PID; any publication requires explicit
curation and a separate decision. Reports retain only fixed sentinels and hashes,
not arbitrary terminal content.

## Dependency-ordered milestones

### Milestone 1 — stdio driver and preflight

- Add the shared MCP host driver and typed event/report model.
- Implement installed/staged binary identity checks.
- Implement isolated daemon startup and deny-all MCP preflight.
- Run each terminal protocol-negative case in a fresh adapter process.
- Add unit/subprocess tests for response matching, early stdin close, timeout,
  malformed output, process exit, and cleanup.

**Gate:** preflight passes repeatedly without Wayland, touches no persistent
policy/topology, and leaves no socket or process. Existing package validation
continues to pass after any driver reuse.

### Milestone 2 — owned graphical smoke

- Add guarded Hyprland preflight and exact-address placement.
- Create the isolated native demo Lair/window.
- Observe and place the consent window by exact process ancestry and identity.
- Pause for human grant/deny without synthetic input.
- Collect Plan 0030 diagnostics on pre-map failure.
- Implement complete signal/error cleanup and state restoration.

**Gate:** one explicitly approved smoke shows the correct consent prompt on
workspace 8 / DP-2, grant and denial each fail or succeed honestly, and cleanup
attestation proves no residue. A smoke failure blocks the matrix.

### Milestone 3 — visible control workflow

- Acquire exact control with approved takeover.
- Send and verify the fixed visible sentinel.
- Resize the terminal grid and verify `stty size` through semantic read-back.
- Release control and verify local ownership clears.
- Revoke the grant and verify post-revocation denial.

**Gate:** the window visibly changes only through the intended terminal actions;
MCP results preserve exact provenance/trust labels; no body/handle leaks into
evidence; cleanup passes.

### Milestone 4 — topology/subscription showcase

- Add the separately consented topology/spawn scopes.
- Subscribe before mutation.
- Split with fixed structured argv and rename only the created child.
- Reconcile the notification, topology revision, visible layout, and child
  identity.
- Add the bounded inspection checkpoint.

**Gate:** one approved matrix records ordered notification and visible topology
proof, then revokes and cleans the complete isolated Lair.

### Milestone 5 — documentation and retained evidence

- Document commands, approval boundaries, installed/source modes, expected UI,
  denial, diagnostics, artifacts, and cleanup.
- Add one sanitized successful preflight artifact.
- Add one reviewed graphical smoke/matrix artifact only after explicit approval
  to retain it.
- Run one fresh read-only security/acceptance review over the coherent harness.

**Gate:** a new operator can run preflight without desktop effects and can
understand exactly what must be approved before the graphical lane. Recorded
review has no unresolved blocker.

## Required validation

Non-graphical validation before any graphical review:

```bash
python -m pytest tools/mcp/tests
python -m py_compile tools/mcp/run-demo.py tools/mcp/mcp_host.py
cargo test -p splinterm-mcp --all-targets -- --test-threads=1
python tools/automation/validate-contract-fixtures.py
python tools/package/validate-mcp-package.py <extracted-root>
git diff --check
```

Use the repository's actual Python test command if the harness follows an
existing unittest convention instead of pytest. Do not add a new test framework
only for this plan.

Before a graphical launch:

```bash
hyprctl version
hyprctl -j status
hyprctl -j instances
hyprctl -j monitors all
hyprctl -j workspaces
hyprctl -j clients
hyprctl -j activewindow
hyprctl configerrors
```

Graphical acceptance requires one user-approved smoke followed by the already
approved conditional matrix. Validation records:

- exact binary and package/build identities;
- isolated socket/state roots;
- window PID/address/workspace/monitor identity;
- consent target and fixed scope names;
- grant/deny result without capability material;
- controller, resize, terminal provenance, notification, revision, revocation,
  and post-revocation denial results;
- original and restored compositor state; and
- exact cleanup attestation.

No graphical pass may be claimed from screenshots alone.

## Definition of done

This plan is complete when:

1. `preflight` is a repeatable non-graphical gate for coherent installed and
   staged source binaries;
2. the graphical runner refuses unsafe workspace/window conditions before
   mapping anything;
3. one explicit approval covers a documented smoke and conditional matrix;
4. the user sees and personally decides the trusted consent prompt;
5. the visible workflow demonstrates bounded observation, control, resize,
   subscription, topology mutation, release, revocation, and fail-closed denial;
6. the runner records bounded privacy-safe evidence after every phase;
7. denial, timeout, `wayland_failure`, wrong-window detection, and interruption
   all clean up honestly;
8. production daemon topology, persistent policy, installed files, unrelated
   windows, and user processes remain unchanged;
9. focused automated gates and one retained graphical acceptance run pass; and
10. fresh read-only review records no unresolved security, cleanup, or evidence
    blocker.

## Stop gates

Stop and request a new decision if implementation would require:

- editing or reloading persistent user automation policy;
- replacing Pacman-owned binaries merely to run the harness;
- using the production daemon or an existing user Lair/Splint;
- synthesizing consent approval;
- driving input into a window not created and exactly identified by the harness;
- relying on title, workspace, app ID, or focus alone as window authority;
- enabling `SPLINTERM_ENABLE_DEV_ATTACH` for the behavior under test;
- adding persistent Hyprland/Omarchy configuration;
- retaining raw terminal contents, private handles, capability frames, policy
  bodies, argv, or environment data in artifacts;
- broad-killing by process name or workspace during cleanup;
- adding HTTP/OAuth or another MCP transport for convenience; or
- repeating a failed graphical attempt before diagnosing it and stating one
  bounded correction.
