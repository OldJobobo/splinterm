# Herdr integration research

- **Research date:** 2026-07-29
- **Splinterm baseline:** `main` at `b7f0a621fc584e1a68de20e5c9267c1109654fc9`
- **Herdr source baseline:** `master` at `73d92004f50d3f5fafe64e0f9b7fddbcf4d99965`
- **Released Herdr baseline:** 0.7.5
- **Purpose:** determine whether Herdr can serve as Splinterm's agent runtime while Splinterm serves as its native secure presentation cockpit

## Executive finding

The projects can work together without merging their terminal engines or sharing
PTY ownership. Herdr can remain authoritative for agent processes, names,
lifecycle, worktrees, prompt-and-wait, and terminal sessions. Splinterm can
present those sessions as native Wayland Splints and retain its own topology,
rendering, controller, policy, consent, and audit boundaries.

The strongest integration seam is Herdr's released terminal-session bridge:

```text
herdr terminal session observe <target> --cols N --rows N
herdr terminal session control <target> --cols N --rows N
```

These commands emit newline-delimited `terminal.frame` records containing
base64-encoded ANSI bytes. Control mode accepts newline-delimited
`terminal.input`, `terminal.resize`, `terminal.scroll`, and `terminal.release`
commands. That provides an explicit bridge protocol instead of relying on
screen scraping, shell prompt parsing, or nested interactive keybindings.

A first-class integration remains a non-trivial product feature. Agent
discovery, lifecycle presentation, controller conflicts, reconnect behavior,
version compatibility, trust labeling, packaging, and the boundary between
Herdr's same-user API and Splinterm's least-privileged automation all require an
explicit design. [Plan 0014](plans/0014-first-class-herdr-integration.md) records
that design.

## Primary sources

### Herdr

- [Product site](https://herdr.dev/)
- [Source repository](https://github.com/ogulcancelik/herdr)
- [Documentation](https://herdr.dev/docs/)
- [Concepts](https://herdr.dev/docs/concepts/)
- [Agents](https://herdr.dev/docs/agents/)
- [Agent automation](https://herdr.dev/docs/agent-automation/)
- [Socket API](https://herdr.dev/docs/socket-api/)
- [Persistence and remote access](https://herdr.dev/docs/persistence-remote/)
- [Plugins](https://herdr.dev/docs/plugins/)
- [Changelog](https://github.com/ogulcancelik/herdr/blob/master/CHANGELOG.md)
- [License on `master`](https://github.com/ogulcancelik/herdr/blob/master/LICENSE)

### Splinterm

- [Architecture](architecture.md)
- [Automation](automation.md)
- [MCP adapter](mcp.md)
- [Client integrations](integrations.md)
- [Remote access](remote.md)
- [Headless operation](headless.md)
- [Roadmap](roadmap.md)
- [ADR 0005: trusted consent broker](adr/0005-trusted-consent-broker.md)
- [ADR 0006: multiplexing lifecycle](adr/0006-multiplexing-lifecycle.md)
- [ADR 0007: supported automation policy](adr/0007-supported-automation-policy.md)

## Product boundary comparison

| Concern | Herdr | Splinterm |
| --- | --- | --- |
| Primary product | Agent-aware terminal workspace manager | Native Wayland terminal, persistent multiplexer, and secure automation substrate |
| Persistent owner | Herdr server | `splinterd` |
| Process/terminal unit | Herdr pane and terminal | Splint and process incarnation |
| Workspace hierarchy | Session → workspace → tab → pane | Topology → Lair → Dojo → Splint |
| Agent identity | First-class named live agent | No semantic agent registry |
| Agent lifecycle | `idle`, `working`, `blocked`, `done`, `unknown` | Process lifecycle only |
| Orchestration | Start, prompt, wait, read, focus, worktree workflows | Structured process/topology/control primitives |
| Presentation | TUI hosted by an outer terminal | Native disposable Wayland client |
| Automation trust | Owner-only socket with broad same-user authority | Exact executable digest, scopes, resources, bounds, consent, revocation, and audit |
| External protocol | CLI plus broad local NDJSON socket API | Stable JSON/NDJSON CLI plus bounded MCP adapter |
| Terminal bridge | Read-only observer and exclusive controller streams | Normal PTY ingestion and semantic terminal publication |

The projects overlap at multiplexing but specialize at different layers. Herdr
knows what an agent is doing; Splinterm knows which process and terminal a client
may observe or control.

## Verified Herdr capabilities relevant to integration

### Agent state and orchestration

Herdr detects supported agents through foreground process discovery, terminal
screen manifests, terminal signals, and optional agent-native integrations. It
publishes agent identity, native session references when available, and semantic
status. Current agent operations include:

- `agent.list` and `agent.get`;
- `agent.start`;
- atomic `agent.prompt` with optional wait;
- server-owned `agent.wait` pinned to the resolved pane occupant;
- `agent.read`, `agent.send_keys`, `agent.rename`, and `agent.focus`;
- declarative transient Agent views;
- worktree creation, opening, and removal.

`done` is an attention state: an idle agent has completed work that the operator
has not yet seen. This is a presentation concept rather than a process state and
is valuable for a cockpit UI.

### Snapshot and events

`session.snapshot` returns version/protocol metadata, current focus, workspaces,
tabs, panes, layouts, agents, and worktree provenance. Clients are expected to
bootstrap from that snapshot and then subscribe to events. Relevant events
include pane updates, agent-status changes, layout changes, scroll changes, and
worktree lifecycle. On reconnect or suspected staleness, the client fetches a
fresh snapshot rather than inferring state.

The CLI exposes the snapshot as:

```text
herdr api snapshot
```

The bundled API schema is inspectable through `herdr api schema`.

### Direct human attachment

Herdr supports direct interactive attachment:

```text
herdr agent attach <target>
herdr terminal attach <terminal_id>
```

One writable direct attachment owns terminal input and resize. `--takeover`
replaces an existing controller. Direct attach is useful for manual compatibility
testing, but it is not the preferred first-class Splinterm boundary because it
combines rendering, input, resize, and keybinding behavior in one interactive
client.

### Machine terminal-session bridge

Herdr 0.7.2 introduced bridge-oriented commands:

```text
herdr terminal session observe <target> [--cols N] [--rows N]
herdr terminal session control <target> [--takeover] [--cols N] [--rows N]
```

Observed contract:

- stdout is newline-delimited JSON;
- `terminal.frame` carries base64-encoded ANSI bytes;
- `terminal.closed` ends the stream;
- observe mode has no input, resize, scroll, or takeover authority;
- control mode reads newline-delimited commands on stdin;
- commands include `terminal.input`, `terminal.resize`, `terminal.scroll`, and
  `terminal.release`;
- only one controller may own input and resize;
- multiple observers may coexist;
- controller takeover is explicit.

This is the narrowest stable seam for making a Herdr-owned terminal appear as a
Splinterm Splint.

### Persistence

Herdr distinguishes several persistence levels:

- client detach leaves the server and pane processes alive;
- ordinary full server restart restores structure but not live processes;
- optional history replay restores display state, not process state;
- supported agents may resume through native session identifiers;
- experimental live handoff can preserve supported live PTYs.

Splinterm also preserves daemon-owned processes across graphical client detach,
but a `splinterd` restart ends those processes and explicit restore relaunches
saved commands. A bridge process ending must not be confused with the Herdr-owned
agent ending.

## Verified Splinterm capabilities relevant to integration

Splinterm already provides the required presentation substrate:

- daemon-owned persistent Splints;
- native Wayland rendering and pane composition;
- structured argv process creation;
- stable Lair/Dojo/Splint IDs and process incarnations;
- bounded visible terminal, scrollback, search, and subscriptions;
- one exclusive controller per live Splint;
- explicit controller transfer;
- JSON/NDJSON automation contracts;
- a separate 32-tool MCP adapter;
- peer UID and executable-digest policy;
- exact operation scopes and resource selectors;
- trusted graphical consent, revocation, and body-free audit;
- explicit treatment of terminal content as untrusted data.

Splinterm intentionally does not provide agent identity, task state, readiness,
completion, inter-agent messaging, or result aggregation. The proposed
integration preserves that boundary: `splinterd` does not become an agent
classifier.

## Integration shapes considered

### 1. Nest the full Herdr TUI in one Splint

**Feasibility:** high.

This requires no source integration and is a useful smoke test. Splinterm hosts
one Herdr client, while the Herdr server owns every inner agent terminal.
Splinterm sees one terminal surface rather than individual agents.

This is coexistence rather than first-class integration. It duplicates
multiplexer navigation, retains nested keybindings, and does not let Splinterm
present individual agents as native Splints.

### 2. Launch direct Herdr attachments in separate Splints

**Feasibility:** high.

Each Splint runs `herdr agent attach` or `herdr terminal attach`. Herdr owns the
real agent PTY and Splinterm arranges the attachment clients. This is useful
immediately, but direct attach is human-oriented and does not provide a clean
machine boundary for lifecycle reconciliation or trusted presentation.

### 3. Bridge Herdr terminal-session streams into ordinary Splints

**Feasibility:** high, with bounded engineering work.

A dedicated adapter launches `herdr terminal session observe/control`, decodes
Herdr frame records, writes ANSI bytes to its Splinterm-owned PTY, reads raw PTY
input, and emits Herdr control commands. `SIGWINCH` becomes `terminal.resize`.
Splinterm's existing terminal engine and renderer process the resulting ANSI as
untrusted terminal data.

This model gives each Herdr terminal a native Splinterm presentation surface
without sharing the actual agent PTY.

### 4. Make `splinterd` own Herdr agents while Herdr observes them

**Feasibility:** low with current Herdr architecture.

Herdr's agent APIs operate on Herdr-owned panes and terminals. It does not adopt
arbitrary PTYs owned by another multiplexer. Supporting this model would require
a new external-terminal backend in Herdr or substantial shared-runtime work.

### 5. Merge both topology and persistence models

**Feasibility:** low and not justified.

Both systems have authoritative IDs, layouts, focus, controller, resize, and
persistence behavior. Combining them would create dual-writer ambiguity and
would weaken the clear lifetime boundaries in both projects.

## Recommended division of responsibility

### Herdr owns

- agent process and PTY;
- agent kind, name, native session, and lifecycle;
- prompt submission and server-owned waits;
- worktree orchestration;
- pane-agent detection and diagnostics;
- terminal-session controller arbitration.

### Splinterm owns

- native Wayland presentation;
- Lair/Dojo/Splint placement;
- rendering and local input;
- Splinterm process incarnation and controller state;
- policy, consent, revocation, and audit for Splinterm operations;
- explicit creation and removal of presentation bridges.

### The adapter owns

- Herdr version/schema negotiation;
- Herdr snapshot and event normalization;
- mapping Herdr terminal identities to presentation Splints;
- ANSI frame decoding and bounded transport;
- PTY raw mode and resize translation;
- controller-conflict reporting and explicit takeover requests;
- reconnect and resynchronization behavior;
- clear trust labels for Herdr-reported metadata.

## Security analysis

### Authority does not compose automatically

A narrow Splinterm authorization does not become a narrow Herdr authorization.
Herdr's owner-only socket is a broad same-user control surface. A bridge process
that can reach it may be able to inspect or control the wider Herdr session even
when Splinterm authorizes that bridge for only one Splint.

Therefore:

- no document or UI may claim that Splinterm policy scopes protect operations
  inside Herdr;
- the adapter must expose only its reviewed operations and never proxy arbitrary
  Herdr API requests;
- raw Herdr socket access must not be exposed through Splinterm MCP;
- automated `--takeover` is prohibited;
- first-party graphical actions and third-party headless automation remain
  separate authorization surfaces;
- terminal frames remain untrusted data despite originating from Herdr;
- Herdr names, titles, state labels, and paths are sanitized and visually marked
  as external runtime data;
- Herdr terminal bytes never become consent, policy, commands, or executable
  source.

### Controller composition

There are two independent controllers:

1. a Splinterm controller owns input and resize for the bridge process's Splint;
2. the bridge may own Herdr input and resize for the Herdr terminal.

Input is possible only while both are held. Loss of either controller must stop
forwarding. A Herdr control conflict is a normal outcome and must not trigger
forced takeover. Takeover requires an explicit trusted graphical decision and
must identify the affected Herdr target.

### Process identity and replacement

The Splinterm bridge process has a normal Splint incarnation. Herdr terminal and
agent identity are separate. A bridge restart may reconnect to the same Herdr
terminal, while a Herdr agent occupant may be replaced inside that terminal.
The UI must display these identities separately and refresh agent metadata from
Herdr events. It must not silently convert a stale agent alias into authority
for an unrelated replacement.

## Compatibility and licensing

The bridge-oriented terminal-session commands are present in released Herdr
0.7.2 and later. The integration should negotiate a minimum version and validate
machine records rather than relying only on a version string.

Herdr's licensing changed after 0.7.5:

- released 0.7.5 source is AGPL-3.0-or-later with a commercial option;
- current `master` is Apache-2.0 after a 2026-07-22 relicensing commit.

Splinterm should not vendor Herdr source or bundle a Herdr binary until the
license of the exact distributed release is reviewed. The initial integration
can execute a separately installed `herdr` binary and consume documented process
protocols, keeping Splinterm's MIT source independent.

## Product conclusion

The proposal is feasible because Herdr already exposes the exact missing seam:
a machine-oriented rendered-terminal session with separate observe and control
modes. Splinterm does not need to reinterpret Herdr's internal PTY or duplicate
its agent detection. It needs a bounded adapter, a native agent catalog, durable
presentation references, explicit controller composition, and honest security
language.

The resulting product model is:

> Herdr is the agent runtime. Splinterm is the native secure presentation
> cockpit.
