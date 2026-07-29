# Plan 0014: first-class Herdr integration

- **Status:** Proposed
- **Product model:** Herdr is the agent runtime; Splinterm is the native secure presentation cockpit
- **Research:** [Herdr integration research](../herdr-integration-research.md)
- **Foundations:** [Architecture](../architecture.md), [client integrations](../integrations.md), [MCP](../mcp.md), [ADR 0006](../adr/0006-multiplexing-lifecycle.md), and [ADR 0007](../adr/0007-supported-automation-policy.md)
- **Minimum Herdr capability baseline:** released Herdr 0.7.2 terminal-session bridge, snapshot, schema, and event APIs
- **Release decision:** do not advertise first-class Herdr support until the bridge, controller, resynchronization, compatibility, security, packaging, and graphical gates pass

## Goal

Add an optional Herdr integration that lets an operator discover Herdr agents,
see their lifecycle and attention state in native Splinterm UI, and open a
Herdr-owned agent terminal as an ordinary Splinterm Splint without transferring
agent PTY ownership to `splinterd`.

The first release must preserve both projects' existing authority boundaries:
Herdr remains authoritative for agents and their terminal sessions; Splinterm
remains authoritative for its presentation topology, local terminal rendering,
and Splinterm-side authorization.

## User-visible outcome

With a supported `herdr` executable and a running Herdr session, the graphical
client provides an optional **Herdr Agents** surface that:

- lists live agents grouped by Herdr workspace and tab;
- displays Herdr-reported name, kind, lifecycle state, attention state, and
  worktree context;
- distinguishes `blocked`, unseen `done`, `working`, `idle`, and `unknown`;
- opens an agent terminal in a new or selected Splinterm Splint;
- focuses an already-open view instead of creating an accidental duplicate;
- supports read-only observation when input authority is unavailable;
- reports controller conflicts without silently taking control;
- offers explicit trusted takeover only after identifying the current target;
- marks a disconnected bridge separately from an exited Herdr agent;
- reconciles after Herdr or Splinterm client reconnect without guessing;
- remains absent when Herdr is not installed or the integration is disabled.

The ordinary Splinterm terminal path must behave exactly as before when the
integration is unused.

## Architecture decision

Use a separate optional executable, provisionally `splinterm-herdr`, as the
Herdr-specific process boundary. Do not add Herdr socket parsing, agent
detection, lifecycle classification, or bridge framing to `splinterd`.

```text
┌────────────── Splinterm graphical client ──────────────┐
│ trusted operator actions · agent catalog · view mapping│
└───────────────┬──────────────────────┬─────────────────┘
                │ Splinterm protocol   │ normalized catalog stream
                │                      │
┌───────────────▼───────────────┐  ┌───▼────────────────────────┐
│           splinterd           │  │ splinterm-herdr catalog    │
│ topology · bridge PTY · policy│  │ Herdr snapshot + events    │
└───────────────┬───────────────┘  └───┬────────────────────────┘
                │ PTY                   │ Herdr socket/CLI
┌───────────────▼───────────────┐      │
│ splinterm-herdr terminal      │      │
│ ANSI frames ↔ PTY input       │      │
└───────────────┬───────────────┘      │
                │ terminal-session NDJSON
┌───────────────▼──────────────────────▼─────────────────┐
│                      Herdr server                       │
│ agents · PTYs · lifecycle · prompt/wait · worktrees    │
└─────────────────────────────────────────────────────────┘
```

### Why a separate executable

A separate process:

- keeps Herdr-specific compatibility code out of `splinterd`;
- can be packaged independently;
- has an independently reviewable executable identity;
- fails without taking down the graphical client or daemon;
- permits protocol fixtures and fake-Herdr tests without graphical setup;
- preserves the option to remove or replace the integration;
- avoids linking or vendoring Herdr source;
- follows the existing `splinterm-mcp` precedent for an optional integration
  boundary.

The executable may contain shared library modules used by subprocess modes, but
only one installed binary is required initially.

## Fixed boundaries

### Herdr remains authoritative for agents

Splinterm must not independently infer agent state from terminal pixels,
process names, titles, or prompt text. The integration consumes Herdr's
snapshot and events and labels the resulting metadata as Herdr-reported.

Splinterm must not implement:

- its own Claude/Codex/Pi screen manifests;
- agent-native session resume;
- prompt completion heuristics;
- worktree lifecycle duplication;
- agent-result interpretation;
- automatic actions triggered by terminal prose.

### `splinterd` remains agent-semantic-free

The daemon may persist a bounded external presentation reference attached to a
Splint, but it does not connect to Herdr, classify agents, maintain Herdr event
state, or grant Herdr authority.

The reference is presentation metadata, not truth. A graphical client validates
it against a fresh normalized Herdr snapshot before showing live status.

### PTY ownership is not shared

A Herdr agent continues to run under a Herdr-owned PTY. The Splinterm Splint owns
only the adapter process's PTY:

```text
splinterd → adapter PTY → splinterm-herdr → Herdr terminal-session stream
                                         → Herdr-owned agent PTY
```

The adapter decodes Herdr ANSI frames into its stdout and forwards raw user input
as Herdr terminal commands. Neither daemon adopts the other's PTY.

### Automation authority does not cross the bridge

Splinterm policy determines who may create, observe, or control the adapter's
Splint. It does not constrain Herdr's same-user socket after the adapter connects.

The adapter must expose a closed, reviewed operation set. It must not:

- proxy arbitrary Herdr API methods;
- expose the Herdr socket through Splinterm MCP;
- accept a shell command string;
- execute terminal-derived text;
- treat a Splinterm context variable as authority;
- acquire or force Herdr control because a caller controls the outer Splint;
- claim that Splinterm policy scopes apply inside Herdr.

A headless or MCP control surface is deferred until the local graphical workflow
has passed its security and lifecycle gates.

## External compatibility contract

### Herdr discovery

Discovery order:

1. an explicit configured path;
2. `herdr` resolved through the graphical client's reviewed environment;
3. unavailable.

The integration does not download, update, or install Herdr. Missing Herdr is a
non-error unless the operator invokes the integration.

On startup, `splinterm-herdr` must:

1. run a bounded version/capability probe;
2. obtain and validate the Herdr API schema or known capability response;
3. reject missing terminal-session, snapshot, or event capabilities;
4. report the detected version and unsupported capability clearly;
5. avoid starting a Herdr server merely to populate an empty catalog unless the
   operator explicitly requests it.

Version strings alone are insufficient. Capability validation is authoritative.

### Catalog protocol

`catalog` mode emits a Splinterm-owned, newline-delimited public contract,
provisionally `splinterm.herdr.event.v1`:

- one bootstrap record with source version, session identity, and normalized
  workspaces/tabs/agents;
- ordered lifecycle records;
- explicit disconnected, incompatible, and resync-required records;
- no terminal body, prompt text, clipboard data, environment, socket path, full
  argv, capability material, or controller identifiers.

Every record is bounded and rejects unknown schema major or operation. The
normalizer keeps Herdr's public IDs as strings and does not infer relationships
missing from a snapshot.

A connection loss ends the stream. The graphical client discards live catalog
state, marks existing views disconnected, launches a new bounded catalog
process, fetches a fresh snapshot, and then resumes events. It does not apply
missed events to stale state.

### Terminal bridge protocol

`terminal` mode takes an explicit Herdr session plus pane/terminal target and an
explicit mode: `observe` or `control`.

It must:

- launch structured argv for `herdr terminal session observe/control`;
- place its Splinterm PTY in raw mode;
- keep Herdr subprocess stdin/stdout on pipes rather than another interactive
  PTY;
- cap each NDJSON record before allocation;
- validate record type and base64 before decoding;
- cap decoded frame size and aggregate queued output;
- write decoded ANSI bytes to adapter stdout without interpreting them;
- translate PTY input bytes to `terminal.input` using base64 form;
- translate `SIGWINCH` dimensions to bounded `terminal.resize` commands;
- map explicit scroll actions only when a public Splinterm input path can
  distinguish them from terminal application input;
- send `terminal.release` during orderly control shutdown;
- terminate and reap the Herdr child on bridge exit;
- close cleanly on `terminal.closed`, malformed input, oversized input,
  incompatible schema, broken pipe, or Herdr server disconnect.

Unknown Herdr records fail closed for the current schema major. Diagnostics go
to stderr and never contaminate the terminal byte stream.

### Controller composition

A writable view requires both:

1. control of the outer Splinterm Splint; and
2. control of the inner Herdr terminal session.

The adapter starts without `--takeover`. If Herdr reports a controller conflict,
the bridge remains closed or falls back to an explicitly requested observer
view. It must not retry takeover.

A takeover flow is allowed only through trusted graphical UI:

1. identify the Herdr session and target;
2. state that another Herdr controller will be disconnected;
3. ask the operator to confirm;
4. launch a new control bridge with one `--takeover` attempt;
5. report success or failure without retrying.

The terminal itself cannot trigger or approve this flow.

## Splinterm presentation model

### Herdr agent catalog

The graphical client owns transient catalog state. Add a dedicated module rather
than expanding the Wayland reducer with protocol parsing.

The first surface is a compact native panel or switcher, not a second full Herdr
TUI. Each row includes:

- sanitized agent name;
- agent kind;
- semantic status;
- unseen completion indicator;
- workspace/tab context;
- worktree label when available;
- view state: closed, observing, controlling, disconnected, or stale.

Suggested attention order:

1. blocked;
2. unseen done;
3. working;
4. idle;
5. unknown or disconnected.

This is a presentation default, not task priority or authority.

### Trusted and untrusted presentation

Herdr state belongs in ordinary application chrome but is explicitly marked as
reported by an external runtime. Herdr-provided names, paths, titles, custom
labels, and status labels are sanitized, length-bounded, and never interpolated
into fixed consent copy.

The trusted Splinterm consent surface must not display arbitrary Herdr terminal
content. A controller takeover prompt uses fixed application text plus sanitized
session and target identifiers.

### Durable view reference

After the first bridge works, add a small persisted reference to Splint metadata:

```text
kind: herdr_terminal_v1
session: bounded session identifier
terminal_id: bounded Herdr terminal identifier
pane_id: optional last-observed Herdr pane identifier
mode: observe | control
```

Agent aliases and statuses are not persisted as authority. On client reconnect,
the reference is matched against a fresh catalog. Missing or changed targets are
shown as stale; the client never selects another agent automatically.

The bridge launch command remains explicit saved launch intent. A `splinterd`
restart does not automatically rerun it. Existing explicit restore semantics
remain unchanged.

## Configuration

Add an optional section to Splinterm configuration:

```ini
[herdr]
enabled=no
binary=herdr
session=default
```

Requirements:

- disabled by default during the experimental release;
- strict known-key parsing with line-numbered diagnostics;
- `binary` is either a bare executable resolved normally or an absolute path;
- no shell expansion, command string, plugin marketplace, auto-install, or
  auto-update behavior;
- session selection is explicit when more than one session is available;
- no socket path configuration in the first release unless Herdr's public CLI
  cannot select the required session safely.

A later stable release may enable discovery by default while keeping all control
and takeover actions explicit.

## Failure and recovery behavior

| Failure | Required behavior |
| --- | --- |
| Herdr missing | Hide inactive catalog; show actionable diagnostic when invoked |
| Unsupported Herdr | Report detected version/capabilities; do not start bridge |
| Snapshot malformed | Reject catalog bootstrap; no partial state |
| Event gap/disconnect | Mark catalog stale; fetch full snapshot before resuming |
| Herdr terminal missing | Mark mapped view stale; do not retarget |
| Agent occupant replaced | Refresh metadata; retain terminal identity; clear stale agent claim |
| Herdr controller busy | Offer observer or explicit trusted takeover |
| Splinterm controller lost | Stop forwarding input immediately |
| Herdr controller lost | Stop forwarding input and mark view observing/disconnected |
| Oversized frame | Terminate bridge with bounded diagnostic |
| Invalid base64/record | Terminate bridge; do not pass partial bytes |
| Bridge exits | Splint process exits normally; Herdr agent remains alive |
| Graphical client exits | `splinterd` and bridge may remain; reconnect from authoritative state |
| `splinterd` restarts | Bridge ends; saved intent remains; explicit restore/reconnect required |
| Herdr server restarts | Bridge ends; catalog resnapshots; views become reconnectable if targets return |

## Packaging and licensing

Create an optional package split for `splinterm-herdr`, following the independent
adapter precedent used by `splinterm-mcp`.

The package:

- contains only Splinterm-owned code, schemas, docs, and notices;
- declares `herdr` as an optional external runtime dependency;
- does not vendor, download, or redistribute Herdr;
- does not claim compatibility beyond tested Herdr versions;
- exposes a package-level smoke command that verifies capability without creating
  or controlling an agent.

Before any bundle or hard dependency is proposed, review the exact Herdr release
license. Herdr 0.7.5 is AGPL-3.0-or-later with a commercial option, while current
`master` is Apache-2.0 after the 2026-07-22 relicensing change.

## Non-goals for the first release

- replacing Herdr's own TUI;
- moving Herdr agents into Splinterm-owned PTYs;
- implementing Herdr agent detection in Splinterm;
- proxying the complete Herdr socket API;
- adding Herdr tools directly to `splinterm-mcp`;
- prompting agents or waiting for task completion from Splinterm automation;
- automatic agent fan-out or worktree creation;
- automatic controller takeover;
- durable terminal bodies beyond existing Splinterm behavior;
- compositor workspace placement;
- Windows or macOS support;
- image-protocol parity until terminal-session frame behavior is verified;
- bundling or installing Herdr.

## Expected files

### New adapter

- Create: `crates/splinterm-herdr/Cargo.toml`
- Create: `crates/splinterm-herdr/src/main.rs`
- Create: `crates/splinterm-herdr/src/catalog.rs`
- Create: `crates/splinterm-herdr/src/bridge.rs`
- Create: `crates/splinterm-herdr/src/herdr.rs`
- Create: `crates/splinterm-herdr/src/protocol.rs`
- Create: `crates/splinterm-herdr/tests/catalog_stdio.rs`
- Create: `crates/splinterm-herdr/tests/terminal_bridge.rs`
- Create: `crates/splinterm-herdr/tests/fake_herdr.rs`

### Public contracts

- Create: `dist/schemas/v1/splinterm-herdr-event.schema.json`
- Create: `dist/schemas/v1/splinterm-herdr-fixtures/`
- Modify: `tools/automation/validate-contract-fixtures.py`

### Core and private protocol

- Modify: `crates/splinterm-core/src/model.rs`
- Modify: `crates/splinterm-protocol/src/lib.rs`
- Modify: `crates/splinterd/src/main.rs`
- Modify: `crates/splinterd/src/persistence.rs` if the current persistence boundary
  remains separate from `main.rs`

These changes are limited to the bounded presentation reference and launch
intent. Herdr lifecycle does not enter daemon state.

### Graphical client

- Create: `crates/splinterm/src/herdr.rs`
- Create: `crates/splinterm/src/herdr_panel.rs`
- Modify: `crates/splinterm/src/config.rs`
- Modify: `crates/splinterm/src/main.rs`
- Modify: `crates/splinterm/src/renderer.rs`
- Modify: `crates/splinterm/src/pane.rs` only if mapped-view chrome cannot remain
  isolated in the Herdr panel module

### Packaging and documentation

- Modify: root `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: Arch packaging metadata under `packaging/`
- Modify: `README.md`
- Modify: `docs/architecture.md`
- Modify: `docs/configuration.md`
- Modify: `docs/integrations.md`
- Modify: `docs/packaging.md`
- Create: `docs/herdr.md`
- Create: an ADR freezing external-runtime and authority boundaries before the
  graphical integration is advertised

Exact packaging paths must be confirmed against the current package layout at
implementation time.

## Dependency-ordered implementation slices

### Slice 0 — freeze Herdr bridge evidence

Build a disposable non-production harness against the separately installed Herdr
binary.

Record:

- exact `terminal.frame` and `terminal.closed` records;
- input, resize, scroll, and release command records;
- behavior for malformed commands and unsupported versions;
- controller conflict and takeover responses;
- snapshot and event ordering;
- reconnect behavior after Herdr client/server interruption;
- maximum observed record/frame sizes during normal agent TUI output;
- whether Kitty, Sixel, hyperlinks, synchronized output, focus, mouse, and
  clipboard-relevant sequences survive the stream;
- process and file descriptor cleanup.

Use a fake terminal target first. Graphical validation requires separate approval
under `AGENTS.md` and runs only on workspace 8 / monitor DP-2.

**Gate:** documented fixtures and a go/no-go decision prove that Herdr's released
bridge can carry the required text terminal semantics without screen scraping.
No production configuration or UI is added.

### Slice 1 — adapter protocol and fake-Herdr tests

Add `splinterm-herdr` with capability probing and normalized catalog output.
Develop against a deterministic fake Herdr process before live integration.

Tests cover:

- unknown schema and record rejection;
- bounded line and decoded-frame sizes;
- invalid base64;
- stdout purity and stderr diagnostics;
- ordered bootstrap/events;
- disconnect and resync-required behavior;
- cancellation and child reaping;
- absent and incompatible Herdr;
- sanitized and bounded metadata.

**Gate:** all adapter tests pass without a running daemon, Herdr server, or
Wayland display. The binary cannot control a terminal yet.

### Slice 2 — read-only terminal bridge

Implement `observe` mode only. Decode validated ANSI frames into an ordinary
Splinterm-owned PTY. Do not add input, resize ownership, or takeover.

Test with fake Herdr fixtures, an isolated real `splinterd`, and a separately
installed supported Herdr. Confirm that bridge termination does not terminate
the Herdr target.

**Gate:** a Herdr-owned terminal renders in an ordinary Splint, disconnects
cleanly, and remains unambiguously read-only.

### Slice 3 — writable bridge and controller composition

Add raw PTY input, resize propagation, release, and controller-loss behavior.
Keep takeover absent.

Tests cover:

- binary and UTF-8 input;
- bracketed paste bounds;
- resize coalescing and invalid dimensions;
- outer controller loss;
- inner controller loss;
- partial writes and backpressure;
- bridge shutdown and child reaping;
- no input forwarding in observe or stale state.

**Gate:** control requires both layers, loss of either stops input immediately,
and no code path requests takeover.

### Slice 4 — graphical catalog and open/focus workflow

Add the opt-in client configuration, normalized catalog subscriber, native agent
panel, and trusted operator actions.

Start with client-local mappings. Opening an agent creates a structured bridge
argv. Selecting an already mapped live target focuses its Splint rather than
creating a duplicate.

Non-graphical tests cover reducer state, ordering, sanitation, selection,
disconnect, resnapshot, duplicate prevention, and stale-target handling.

After approval, graphical tests cover:

- panel layout at normal, narrow, and scaled sizes;
- blocked/done/working/idle/unknown states;
- keyboard and pointer navigation;
- opening, focusing, observing, and closing views;
- ordinary terminal behavior with integration disabled;
- placement and focus isolation on workspace 8 / DP-2.

**Gate:** the operator can discover and open agents without the full Herdr TUI,
and no terminal content enters trusted consent copy.

### Slice 5 — durable view references and reconnect

Add the bounded `herdr_terminal_v1` presentation reference to Splint metadata and
private protocol/persistence. Increment the private protocol version.

Tests cover:

- round-trip persistence;
- missing, oversized, and malformed references;
- stale Herdr targets;
- bridge incarnation replacement;
- client reconnect;
- explicit restore after daemon restart;
- no automatic rerun;
- no authority or policy expansion from the reference.

**Gate:** graphical client restart reconstructs mapped views from authoritative
Splinterm topology plus a fresh Herdr snapshot without guessing or duplicating
views.

### Slice 6 — explicit trusted takeover

Add one graphical takeover action. Keep it unavailable to public CLI, JSON,
NDJSON, MCP, terminal escape sequences, and catalog records.

Tests cover fixed copy, sanitized target labels, denial, timeout, one-shot
success, controller replacement, cleanup, and absence from machine interfaces.

Graphical validation verifies that terminal output cannot visually spoof the
prompt and that cancelling leaves the existing controller untouched.

**Gate:** takeover occurs only after one explicit trusted decision and is never
retried automatically.

### Slice 7 — packaging, documentation, and release matrix

Add optional packaging and user documentation. Validate separately installed
Herdr versions spanning the supported minimum through current stable.

The release matrix includes:

- capability probe;
- catalog bootstrap/events;
- observe and control;
- resize and input;
- Herdr server restart;
- Splinterm client reconnect;
- controller conflict;
- unsupported version;
- no-Herdr startup;
- package install/uninstall;
- license and notice review.

**Gate:** extracted-package tests pass, unsupported versions fail clearly, no
Herdr binary is bundled, and ordinary Splinterm package behavior is unchanged.

### Slice 8 — separate decision for headless orchestration

After local graphical release evidence exists, decide whether to expose any
Herdr-aware operation to third-party automation.

This requires a new ADR and threat model. Candidate operations should begin with
read-only catalog inspection and opening an observer bridge. Prompting, waiting,
control, worktree mutation, and takeover are not implied by completing the local
integration.

**Gate:** no headless Herdr authority is added under this plan without a separate
approved architecture decision.

## Validation commands

Run focused checks as each slice lands, then the complete non-graphical gate:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
uv run --with jsonschema python tools/automation/validate-contract-fixtures.py
python -m unittest tools/automation/test_session_picker.py
git diff --check
```

Adapter-specific tests should remain directly runnable:

```bash
cargo test -p splinterm-herdr
cargo test -p splinterm-herdr --test catalog_stdio
cargo test -p splinterm-herdr --test terminal_bridge
```

Graphical tests are not covered by these commands and remain governed by
`AGENTS.md` workspace, monitor, placement, focus, and approval requirements.

## Acceptance criteria

The integration is first-class only when all of the following are true:

1. A supported Herdr session appears as a native, optional agent catalog.
2. A selected Herdr agent can be observed in a normal Splint.
3. Writable control composes both controller layers and stops on either loss.
4. No automatic takeover exists.
5. Blocked and unseen-done agents are distinguishable without reading terminal
   prose.
6. Client reconnect begins from fresh authoritative snapshots at both layers.
7. Stale targets are surfaced and never silently retargeted.
8. Herdr termination, bridge termination, agent termination, and controller loss
   are distinct user-visible states.
9. Terminal frames and Herdr metadata remain untrusted data.
10. The daemon does not classify agents or connect to Herdr.
11. Splinterm policy is not represented as Herdr authorization.
12. No Herdr source or binary is bundled without a reviewed license decision.
13. Integration-disabled startup and ordinary terminal behavior are unchanged.
14. Non-graphical, graphical, package, compatibility, and security evidence are
    recorded.
15. Documentation explains both the benefit and the authority boundary in plain
    language.

## Risks and stop conditions

Stop for an architecture decision if any spike shows that:

- Herdr terminal-session output cannot reproduce required text terminal behavior;
- frame size or cadence cannot be bounded without visible corruption;
- control mode cannot distinguish controller conflict and disconnect reliably;
- Herdr does not offer a capability/schema boundary stable enough to support;
- the integration requires parsing human Herdr output;
- native status requires interpreting terminal content in Splinterm;
- durable mapping requires `splinterd` to become a Herdr client;
- graphical takeover cannot remain isolated from terminal-controlled content;
- packaging would require unreviewed redistribution of Herdr;
- a public automation surface would grant materially broader Herdr authority than
  its Splinterm scopes communicate.

## Deferred possibilities

After the first-class presentation path is stable, later proposals may consider:

- worktree-to-Dojo navigation;
- Herdr notification routing into Splinterm chrome;
- prompt-and-wait from a separately authorized orchestrator;
- read-only Herdr catalog resources in MCP;
- richer agent attention filters;
- remote Herdr runtime with local Splinterm presentation;
- a Herdr plugin that can request Splinterm view actions through a narrow channel;
- a compositor broker for opening a mapped agent view on a requested workspace.

None of these are required to prove the core product relationship.
