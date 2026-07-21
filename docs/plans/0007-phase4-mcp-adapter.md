# Plan 0007: required full-capability MCP adapter

- **Status:** Planned; stable JSON/NDJSON CLI implemented, blocked on the
  future-descendant policy decision and reusable non-Wayland client extraction
  in [Plan 0006](0006-phase4-headless-automation.md)
- **Roadmap:** Phase 4 — Headless access and supported automation, Slice 6
- **Foundation:** [Plan 0006](0006-phase4-headless-automation.md),
  [ADR 0007](../adr/0007-supported-automation-policy.md), and
  [`docs/automation.md`](../automation.md)
- **Protocol target:** Model Context Protocol `2025-11-25`
- **Required outcome:** Phase 4 cannot close without this adapter

## Goal

Ship a separate `splinterm-mcp` stdio server that gives MCP clients bounded
parity with every operation supported for third-party automation through the
same daemon capability checks as every other client.

The adapter must make terminal-derived content useful without presenting it as
instruction, consent, or authority. It must not turn MCP transport access,
inherited in-Splint context, or logical resource containment into Splinterm
access; expose the private daemon protocol; inherit graphical-client authority;
or add a network listener.

## Research baseline

Research was performed on 2026-07-21 against primary sources:

- [MCP specification `2025-11-25`](https://modelcontextprotocol.io/specification/2025-11-25),
  the latest dated release at research time;
- [MCP lifecycle](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle),
  including initialization, version/capability negotiation, cancellation,
  shutdown, and timeout requirements;
- [MCP transports](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports),
  including newline-delimited stdio and stdout/stderr requirements;
- [MCP tools](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)
  and the corresponding
  [TypeScript schema](https://github.com/modelcontextprotocol/specification/blob/main/schema/2025-11-25/schema.ts);
- [MCP security best practices](https://modelcontextprotocol.io/specification/2025-11-25/basic/security_best_practices),
  especially local-server consent, sandboxing, and stdio isolation;
- the official [Rust SDK](https://github.com/modelcontextprotocol/rust-sdk),
  whose current stable release was
  [`rmcp-v2.2.0`](https://github.com/modelcontextprotocol/rust-sdk/releases/tag/rmcp-v2.2.0);
- the official [MCP conformance framework](https://github.com/modelcontextprotocol/conformance)
  and [MCP Inspector](https://github.com/modelcontextprotocol/inspector); and
- the [OWASP prompt-injection prevention guidance](https://cheatsheetseries.owasp.org/cheatsheets/LLM_Prompt_Injection_Prevention_Cheat_Sheet.html),
  particularly structured separation of instructions and untrusted data,
  least privilege, output validation, and adversarial testing.

The Rust SDK was changing quickly at research time, including active work on a
future `2026-07-28` draft lifecycle. This plan deliberately targets the latest
dated specification rather than that draft and pins the SDK exactly until a
reviewed upgrade changes both together.

## Fixed product and architecture decisions

### One small, direct, third-party process

`crates/splinterm-mcp` is a separate workspace package and installs one
`splinterm-mcp` executable. It connects directly to the owner-only daemon Unix
socket with `ClientRole::Automation`.

It must not:

- spawn `splinterm --output json` for tool calls;
- depend on the graphical `splinterm` crate and its Wayland/font/render stack;
- proxy raw daemon frames;
- listen on TCP, HTTP, WebSocket, or another network transport; or
- remain resident after its MCP client closes stdio.

Spawning the CLI would make `splinterm`, rather than `splinterm-mcp`, the peer
whose executable path and digest the daemon authorizes. That would erase the
adapter's independent least-privileged policy identity and delegate the selected
scopes to any same-account process able to invoke that CLI.

### Extract the reusable non-Wayland client first

Before the MCP package lands, move the reusable connection, cancellation,
public projection, cursor, error, and bounds logic from
`crates/splinterm/src/automation.rs` into a small non-Wayland workspace library,
provisionally `crates/splinterm-automation-client`.

Both the CLI and MCP adapter depend on that library. The library may depend on
`anyhow`, `serde`, `serde_json`, Tokio, `splinterm-core`, and
`splinterm-protocol`; it must not depend on Wayland, rendering, font, clipboard,
or graphical consent modules.

The Rust API remains an internal implementation boundary. Compatibility is
promised by the checked-in CLI and MCP JSON Schemas, not by Rust enum layouts or
private protocol DTOs.

### Logical resources, graphical windows, and agent context

MCP window tools operate on daemon logical `Window` resources and Splint layout
trees. They do not map, focus, move, resize, close, or assign a
compositor-native Wayland window. `set_window_default_focus` remains a persisted
logical presentation hint. A future compositor broker is a separate trusted UI
surface and is not part of MCP v1.

When an MCP host is launched from inside a Splint, it may inherit
`SPLINTERM_DOJO_ID`, `SPLINTERM_WINDOW_ID`, `SPLINTERM_SPLINT_ID`, and
`SPLINTERM_SPLINT_INCARNATION` after Plan 0006's launch-context slice lands.
These optional values are initial-selection hints only. The adapter may validate
them against an authorized topology read, but it must not use them as authority,
consent, proof of ancestry, an implicit default mutation target, or a reason to
broaden a policy. Missing, malformed, stale, or unauthorized hints are ignored
and surfaced explicitly; the adapter never guesses another Splint.

MCP provides terminal and topology primitives, not a semantic multi-agent
supervisor. Agent readiness, task state, inter-agent messaging, completion, and
result transport remain host/orchestrator responsibilities. Terminal output can
be observed as untrusted data but cannot itself trigger a follow-up tool call in
the server.

### Future-descendant authority must be decided first

ADR 0007 and `docs/automation.md` say Dojo/window selectors do not silently
cover descendants created later. The current `ResourceSelector::matches`
implementation lets a Dojo selector dynamically match later windows and Splints,
and a window selector dynamically match later Splints. That mismatch is a
release blocker for MCP lifecycle and control tools because it can turn
authority over an existing logical container into unintended authority over
agent-created children.

Before MCP Slice 6 starts, either:

1. repair matching to the accepted bounded snapshot semantics and require an
   explicit policy update before controlling a newly created resource; or
2. accept a new ADR defining conspicuous bounded future-descendant authority,
   including selector syntax, creation lineage, limits, expiry, revocation,
   audit representation, upgrade behavior, and adversarial tests.

Do not preserve the current behavior accidentally. A Lair grant used to create a
Dojo never implies later terminal authority over that Dojo, and context
environment values never substitute for a resource selector.

### MCP version, SDK, and capabilities

The first release:

- negotiates MCP `2025-11-25` and rejects unsupported negotiated versions;
- uses the stateful `initialize` → response → `notifications/initialized`
  lifecycle;
- uses the official Rust SDK pinned as `rmcp = "=2.2.0"`;
- enables only the minimum server, macro/schema, stdio transport, tools, and
  resources features;
- advertises static `tools` plus subscribable `resources`, with no list-change
  notification because the catalog is fixed for MCP v1;
- does not advertise prompts, roots, sampling, elicitation, completions,
  structured logging, tasks, or experimental capabilities; and
- marks every tool with task support `forbidden`.

A dependency spike must prove the exact feature set, Rust 1.85 compatibility,
newline-delimited stdout purity, lifecycle negotiation, resource subscriptions,
cancellation hooks, and clean EOF shutdown before production tool code is
accepted. If `rmcp 2.2.0` cannot meet those gates without broad HTTP/OAuth/client
dependencies, stop for an architecture decision rather than silently
implementing a second MCP stack.

### Full supported-automation parity

MCP v1 exposes every operation ADR 0007 permits for third-party automation:
metadata and terminal reads, subscriptions, search, access requests and
inspection, revocation, audit inspection, structured process creation and
lifecycle, layout and name mutation, controller acquisition and transfer,
input, resize, close, and termination.

Authority is not granted by advertising a tool. Every call still passes through
the daemon's exact executable identity, policy scope, resource/incarnation,
revision, controller, limit, confirmation, and audit checks. A read-only policy
can discover mutation tools but cannot execute them successfully.

MCP v1 does not expose trusted-UI-only forced control takeover, direct policy
file administration, clipboard operations that do not exist in the daemon, or
arbitrary shell-string execution. Those are not supported third-party
automation operations. Adding one requires changing the underlying ADR or daemon
capability matrix, not merely editing the MCP adapter.

### Tools and subscribable resources

MCP v1 uses:

- tools for explicit bounded reads, mutations, authorization, controller, and
  lifecycle operations; and
- resources for topology, terminal, and control state that clients may read and
  subscribe to using MCP's native resource-update mechanism.

It exposes no prompts. Terminal-derived resource content has the same explicit
untrusted-data label, provenance, schema validation, and bounds as tool results.
Resource subscriptions never acquire terminal control or expand policy scope.

## Public MCP v1 contract

### Tool naming and annotations

Names are stable, lowercase, dot-separated ASCII identifiers under the MCP
128-character recommendation. Every tool sets reviewed annotations matching its
actual behavior:

- observation tools: `readOnlyHint: true`, `destructiveHint: false`,
  `idempotentHint: true`;
- additive or reversible mutation tools: `readOnlyHint: false` and
  `destructiveHint: false`, with `idempotentHint` true only for set-to-value
  operations;
- close, kill, and revoke tools: `readOnlyHint: false`,
  `destructiveHint: true`; and
- every tool: `openWorldHint: false` because its authority is confined to the
  local Splinterm daemon and explicit resources.

Annotations are hints only; the daemon policy remains authoritative. Tool
descriptions are static reviewed strings and never contain terminal, title,
query, policy, audit, or client-provided text.

### Tool inventory

#### Observation, authorization, and audit

| Tool | Input | Required Splinterm scope or authority |
| --- | --- | --- |
| `splinterm.ping` | empty object | authenticated peer |
| `splinterm.list_dojos` | empty object | `topology_metadata_read` |
| `splinterm.inspect_topology` | empty object | `topology_metadata_read` |
| `splinterm.inspect_splint` | `splint_id` | `topology_metadata_read` |
| `splinterm.read_terminal` | `splint_id` | `terminal_visible_read`, `terminal_subscribe` |
| `splinterm.read_scrollback` | `splint_id`, optional `cursor`, optional `max_rows` | `terminal_visible_read`, `scrollback_read` |
| `splinterm.search_scrollback` | `splint_id`, `query`, optional `case_sensitive`, `cursor`, `max_matches` | `terminal_visible_read`, `scrollback_read`, `scrollback_search` |
| `splinterm.request_access` | `splint_id`, exact closed `scopes` | graphical consent or matching policy for every requested scope |
| `splinterm.authorization_status` | `splint_id` | `authorization_inspect` |
| `splinterm.revoke_access` | `grant_id`, `confirm: true` | `authorization_revoke` for the grant resource |
| `splinterm.inspect_audit` | optional `cursor`, optional `max_records` | `audit_inspect` |

#### Process and topology lifecycle

| Tool | Input | Required Splinterm scope or authority |
| --- | --- | --- |
| `splinterm.create_dojo` | `name`, optional `cwd`, structured `argv` | `process_spawn`, `topology_layout_mutate`; creation limit |
| `splinterm.split_splint` | target, axis, side, ratio, optional `cwd`, structured `argv` | `process_spawn`, `topology_layout_mutate`; creation limit |
| `splinterm.new_window` | `dojo_id`, title, optional `cwd`, structured `argv` | `process_spawn`, `topology_layout_mutate`; creation limit |
| `splinterm.relaunch_splint` | `splint_id`, optional `cwd`, structured `argv` | `process_spawn` for the exact Splint |
| `splinterm.restore_splint` | `splint_id` | `process_restore` for the exact Splint |
| `splinterm.restore_window` | `window_id` | `process_restore` for every expanded Splint |
| `splinterm.restore_dojo` | `dojo_id` | `process_restore` for every expanded Splint |
| `splinterm.close_splint` | `splint_id`, `confirm: true` | `topology_layout_mutate`; live process also needs `process_terminate` |
| `splinterm.close_window` | `window_id`, `confirm: true` | `topology_layout_mutate`; each live process also needs `process_terminate` |
| `splinterm.kill_splint` | `splint_id`, exact incarnation, `confirm: true` | `process_terminate` for exact Splint/incarnation |
| `splinterm.set_split_ratio` | target `splint_id`, ratio | `topology_layout_mutate` |
| `splinterm.rename_dojo` | `dojo_id`, name | `topology_name_mutate` |
| `splinterm.rename_window` | `window_id`, title | `topology_name_mutate` |
| `splinterm.rename_splint` | `splint_id`, title | `topology_name_mutate` |
| `splinterm.set_window_default_focus` | `window_id`, `splint_id` | `topology_layout_mutate` for both resources |

#### Controller and terminal mutation

| Tool | Input | Required Splinterm scope or authority |
| --- | --- | --- |
| `splinterm.acquire_control` | `splint_id`, exact incarnation, requested `input`/`resize` modes | `controller_acquire` plus each requested operation scope |
| `splinterm.request_control_transfer` | `splint_id`, exact incarnation, requested modes | `controller_transfer` plus each requested operation scope |
| `splinterm.decide_control_transfer` | opaque pending-transfer handle, `accept` or `deny` | adapter-owned current controller and pending transfer |
| `splinterm.release_control` | opaque controller handle | adapter-owned controller |
| `splinterm.input` | `splint_id`, exact incarnation, UTF-8 `text`, optional opaque controller handle | own controller and `input`; without a handle use atomic acquire/input/release |
| `splinterm.resize` | `splint_id`, exact incarnation, columns, rows, optional opaque controller handle | own controller and `resize`; without a handle use atomic acquire/resize/release |

No tool accepts a shell command string. Process creation takes a bounded argv
array and passes it directly. An empty argv means the configured shell under the
same reviewed CLI semantics.

UUID inputs use canonical hyphenated lowercase strings. Unknown properties are
rejected. Numeric page limits are positive integers and are capped by the lower
of the MCP contract and negotiated daemon limit. Continuation cursors and
adapter handles are opaque strings; clients must not construct or edit them.

Close, kill, and revoke reject missing or false `confirm` before connecting to
the daemon. Confirmation is explicit intent, not authorization; policy and
resource checks still run. Search queries, input text, cwd, and argv are never
echoed in results, errors, tracing, or audit.

`read_terminal` always detaches its temporary subscription before returning.
Observation never acquires a controller. Adapter controller and transfer handles
wrap daemon-owned IDs, are unguessable, bound to one MCP process and daemon
connection, capped at eight live handles, and are never serialized into audit or
other public contracts.

### Resource and subscription inventory

MCP v1 exposes one fixed resource and two templates:

| Resource | Read/subscribe authority | Contents |
| --- | --- | --- |
| `splinterm://topology` | `topology_metadata_read`; subscription also needs `topology_subscribe` | Current closed topology snapshot, public sequence, revision, and resync state. |
| `splinterm://splints/{splint_id}/terminal` | `terminal_visible_read`, `terminal_subscribe` | Current visible snapshot with exact incarnation/revision/history provenance and trust label. |
| `splinterm://splints/{splint_id}/control` | `topology_metadata_read`, `terminal_visible_read` | Subscriber-specific control status with no private controller/transfer IDs. |

On subscribe, the adapter opens the corresponding daemon subscription, emits
MCP `notifications/resources/updated` for ordered changes, and makes the latest
closed state available through `resources/read`. Public sequence numbers are
adapter-owned and start at one. A daemon gap, stall, revocation, incarnation
replacement, or history replacement publishes one final `resync_required`
state and closes that subscription; the client must explicitly resubscribe.
Resource reads and subscriptions never acquire control.

### Schemas and result envelope

Check in reviewed JSON Schema 2020-12 documents under
`dist/schemas/mcp/v1/`, with valid and security-negative fixtures under
`tests/mcp/fixtures/`. All object schemas are closed with
`additionalProperties: false` unless the MCP specification itself requires an
open metadata object.

Each successful tool returns `structuredContent` conforming to its declared
`outputSchema`. The common object shape is:

```json
{
  "schema": "splinterm.mcp.v1",
  "tool": "splinterm.read_terminal",
  "ok": true,
  "resource": {},
  "data": {},
  "truncated": false,
  "content_trust": "untrusted_terminal_data"
}
```

Rules:

- `content_trust` is `untrusted_terminal_data` for terminal snapshots,
  scrollback, and search previews, and `trusted_metadata` for daemon-produced
  topology/status/audit metadata.
- Terminal provenance includes Dojo ID, window ID, Splint ID, incarnation,
  terminal revision, and history generation.
- Pagination and truncation are explicit. Stale or replaced history returns a
  closed resync result and no continuation cursor.
- Audit results state `daemon_lifetime` retention and expose explicit retention
  gaps.
- No result contains private daemon request, subscription, controller, or
  transfer IDs, raw protocol variants, terminal bytes, input bodies,
  environment data, complete argv, capability tokens, or the search query.
  Controller tools may return only adapter-issued opaque handles under their
  closed schemas.

For clients that do not consume `structuredContent`, return one `TextContent`
block containing the same bounded envelope serialized as compact JSON, as
recommended by the MCP tools specification. Do not add explanatory prose before
or after that JSON. Both representations count toward the response byte ceiling.

### Errors

Use MCP protocol-level errors only for protocol problems such as an unknown tool,
invalid tool arguments, invalid initialization, or an unsupported MCP version.

Daemon, authorization, stale-state, timeout, resource-limit, and not-found
failures are tool results with `isError: true` and a closed
`splinterm.mcp.v1` error object containing:

- stable symbolic code;
- bounded sanitized message;
- `retryable` boolean;
- resource/revision details only when the reviewed contract permits them; and
- no terminal, input, query, argv, policy-body, or private-protocol content.

Do not retry automatically. In particular, `retryable: true` describes the
condition; it does not grant permission to repeat an operation.

## Bounds, concurrency, cancellation, and lifecycle

Freeze these adapter-level limits before implementation:

- maximum inbound MCP line: 256 KiB, including JSON-RPC framing;
- maximum complete MCP tool response: 1 MiB across structured and text content;
- maximum concurrent tool calls per process: 4;
- default daemon deadline: 5 seconds;
- configurable daemon deadline range: 100 milliseconds through 30 seconds;
- `max_rows` and `max_matches`: default 64, maximum 256;
- audit page: default 64, maximum 256;
- structured argv: at most 256 entries and 64 KiB total encoded bytes;
- input text: at most the lower of 64 KiB and the negotiated daemon limit;
- at most 8 live adapter controller/transfer handles;
- at most 16 live MCP resource subscriptions;
- each terminal subscription retains at most one bounded current projected
  state needed to apply ordered updates; and
- no unbounded queue or result cache; retained subscription state is discarded
  on resync, unsubscribe, revocation, daemon loss, or process exit.

The dependency spike must confirm whether the SDK enforces the inbound line
limit before allocation. If not, wrap stdin in a bounded newline codec before
passing messages to the SDK.

Stateless tool invocations get their own automation connection. This isolates
poisoned request correlation, cancellation, and authorization accounting while
the process-wide semaphore enforces concurrency.

Controller tools and resource subscriptions retain dedicated daemon connections
only for the lifetime of their adapter-issued handle or MCP subscription. They
are stored in bounded registries, never shared across MCP processes, and are
revoked and closed on explicit release/unsubscribe, resync, policy reload,
daemon failure, or stdio shutdown. Atomic input/resize calls without a handle use
one connection for acquire, action, and best-effort release.

Extend the extracted client with a cancellation-aware request API. MCP
`notifications/cancelled`, tool future cancellation, deadline expiry, and stdio
EOF must all send one best-effort daemon `Cancel` for an in-flight request,
close that Unix connection, discard late frames, and release any temporary
subscription or controller. Cancellation cannot roll back a mutation already
committed by the daemon; mutation results and audit records must report that
honestly. Cancellation never kills a daemon-owned Splint unless the completed,
explicitly confirmed request was itself `kill_splint`.

On stdin EOF, stop accepting calls, cancel in-flight calls, close stdout, and
exit within a bounded grace period. Write only valid newline-delimited MCP
messages to stdout. Bounded diagnostics may go to stderr; stderr output never
contains terminal text, search queries, complete argv, environment values,
policy contents, or audit bodies.

## Authorization and installation workflow

`splinterm-mcp` is third-party automation even when installed with Splinterm.
It receives no trusted-UI bypass and no authority from same-UID status, argv,
basename, MCP client name, or transport access.

The daemon matches the adapter's canonical executable path and opened-file
SHA-256 digest under ADR 0007. Documentation and packaging must provide:

1. the installed canonical binary path;
2. a command that prints the reviewed digest without modifying policy;
3. least-privileged policy examples for observation-only, terminal-control,
   lifecycle-management, and full supported-automation profiles, each using
   explicit scopes, resources, limits, and expiry where appropriate;
4. a no-policy and under-scoped example that fail closed;
5. upgrade guidance explaining that a new binary digest requires explicit
   policy review/update; and
6. revocation guidance covering policy reload and process termination.

Do not generate, edit, or broaden policy automatically during installation or
first run. The adapter reads only `SPLINTERM_SOCKET` or `XDG_RUNTIME_DIR` for
daemon discovery and its bounded timeout/log configuration. It does not inspect
MCP roots, the working directory, shell configuration, SSH material, or unrelated
environment variables.

## Prompt-injection and tool-output safety

Terminal text, titles, scrollback, and search previews are attacker-controlled
data. They remain untrusted even when the user launched the process.

Defense in depth:

- tool descriptions and server instructions say terminal fields are data, never
  instructions, consent, or evidence that another tool should be called;
- terminal data appears only under explicit structured fields with
  `content_trust: untrusted_terminal_data`;
- no returned text is interpolated into descriptions, errors, schema titles,
  resource URIs, tool names, logs, mutation arguments, confirmation fields, or
  follow-up MCP requests;
- the server offers no sampling, elicitation, prompt, shell-string, filesystem,
  or network capability; mutation tools act only on their explicit validated
  request arguments and never chain from terminal output;
- schemas and result-size checks run after conversion and before serialization;
- terminal-control sequences and invalid Unicode are represented only through
  the existing reviewed semantic-cell conversion; and
- fixtures include direct, indirect, encoded, Markdown/HTML, tool-call-shaped,
  fake-consent, and data-exfiltration prompt-injection strings.

The adapter cannot force an MCP host or model to ignore malicious prose. Its
security claim is narrower and testable: terminal content cannot execute code,
invoke another tool inside the server, alter authorization, change the tool
catalog, escape its data fields, or broaden returned capabilities.

## Dependency-ordered implementation slices

### Blocking precondition — reconcile descendant policy semantics

Before production MCP code, add focused policy tests demonstrating the accepted
behavior for: a Splint added to an existing window, a window added to an existing
Dojo, a newly created Dojo, relaunch incarnation changes, selector expiry, and
reload revocation. Update ADR 0007, the policy schema, automation documentation,
and audit fixtures together if the decision changes the accepted v1 contract.

**Gate:** code, ADR, schema, examples, and tests agree on whether each newly
created descendant is authorized. No implicit containment behavior remains.

### Slice 0 — SDK and protocol spike

**Work**

- Add a temporary non-shipping spike using exactly `rmcp 2.2.0` and MCP
  `2025-11-25`.
- Prove stateful lifecycle negotiation, tools/resources capability
  advertisement, static `tools/list`, resource templates/read/subscribe,
  closed argument rejection, structured output, tool errors, cancellation
  observation, stdin EOF shutdown, and stdout purity.
- Measure the dependency tree and verify Rust 1.85, workspace lint, license, and
  minimal-feature compatibility.
- Determine and test the bounded stdin wrapper required to reject a line before
  allocating beyond 256 KiB.

**Gate**

A black-box stdio test passes every behavior above. Record the exact Cargo
feature set and dependency/license inventory. Stop if the official SDK requires
unneeded HTTP/OAuth/client features or cannot expose cancellation safely.

### Slice 1 — extract the non-Wayland automation client

**Work**

- Create `crates/splinterm-automation-client`.
- Move connection/framing, typed errors, deadlines, cursors, projections, and
  bounds from the graphical crate without changing the CLI JSON contract.
- Add cancellation-token support and guaranteed temporary-subscription cleanup.
- Point `splinterm` CLI at the extracted crate.

**Gate**

Existing CLI fixtures and subprocess tests remain compatible, crate dependency
inspection shows no graphical dependencies, and focused cancellation tests prove
one best-effort daemon cancel followed by connection disposal.

Do not begin this extraction until the active Slice 2 work and its P0
authorization/schema review findings are closed or explicitly rebased into this
slice.

### Slice 2 — freeze MCP schemas and fixtures

**Work**

- Add common, per-tool input, per-tool output, and error schemas under
  `dist/schemas/mcp/v1/`.
- Add valid fixtures for every tool and error class.
- Add invalid fixtures for unknown fields, malformed UUID/cursor/handle,
  missing provenance or confirmation, excessive limits, private daemon IDs, raw
  bytes, query/input/argv echo, open-ended data, false trust labels, invalid
  controller ownership, uncommitted mutation results, and oversized text.
- Extend the fixture validator without weakening existing CLI schemas.

**Gate**

Every valid fixture passes and every security-negative fixture fails for the
intended reason. Schema inventory is exact and duplicate field definitions are
factored without opening objects accidentally.

### Slice 3 — tools/resources MCP server skeleton

**Work**

- Create `crates/splinterm-mcp` with a small `main.rs`, `server.rs`, `tools.rs`,
  `dto.rs`, and `limits.rs`.
- Implement bounded stdio, initialization, static tool/resource discovery,
  resource templates and subscription registry, semaphore, sanitized errors,
  stderr-only tracing, EOF shutdown, and no-policy failure.
- Advertise only the fixed capabilities, annotations, and resource behavior in
  this plan.

**Gate**

A subprocess test proves stdout contains only valid MCP messages; unsupported
versions/capabilities and malformed/oversized lines fail closed; no HTTP listener
or unrelated capability is present.

### Slice 4 — metadata, status, and audit tools

**Work**

- Implement ping, Dojo list, topology inspection, Splint inspection, access
  requests, authorization status/revocation, and audit inspection.
- Apply exact policy scopes, limits, cursor behavior, retention labels, output
  schemas, and tool-error mapping.

**Gate**

Mock-daemon and real-daemon policy tests prove exact allow/deny behavior,
resource scoping, explicit revoke confirmation, audit pagination/gaps, no
private-field leakage, and no trusted UI authority.

### Slice 5 — bounded terminal tools and subscribable resources

**Work**

- Implement one-shot visible snapshot attach/detach, scrollback pagination, and
  literal search.
- Implement topology, terminal, and control resources with read, subscribe,
  update notification, unsubscribe, and explicit terminal resync behavior.
- Preserve exact provenance, public sequence, opaque continuation,
  stale/resync results, truncation, semantic-cell conversion, and query
  non-echo.
- Apply explicit trust labels and final serialized-size checks.

**Gate**

Tests cover exited/replaced Splints, incarnation mismatch, history replacement,
stale/tampered cursor, wide/combining cells, invalid Unicode representation,
maximum pages, oversized output, timeout, cancellation, sequence gaps,
revocation, unsubscribe, resubscribe, and detach cleanup.

### Slice 6 — lifecycle, topology mutation, and process tools

**Work**

- Implement structured create, split, new-window, relaunch, restore, close,
  kill, ratio, rename, and default-focus tools only after the descendant-policy
  blocking gate is closed.
- Reuse the CLI's topology compare-and-swap, exact affected-resource,
  incarnation, partial multi-restore, and committed-revision semantics.
- Validate cwd and bounded argv without constructing a shell string.
- Require `confirm: true` for close and kill before connecting; retain exact
  daemon policy and audit checks after confirmation.

**Gate**

Every lifecycle/topology operation has allow, deny, newly-created-descendant,
stale revision, not-found/incarnation, limit, partial-result,
cancellation-before-commit, cancellation-after-commit, confirmation, and
body/argv non-echo coverage. Successful mutations return only committed
identities/revisions and produce resource-complete audit records. Creating a
resource does not silently grant observation or control of it.

### Slice 7 — controller, input, resize, and transfer tools

**Work**

- Implement bounded opaque controller and pending-transfer handle registries.
- Implement acquire, transfer request/decision, release, atomic input, handled
  input, atomic resize, and handled resize.
- Preserve daemon connection ownership and exact input/resize scopes; never
  expose private IDs or forced takeover.
- Release all controller and transfer state on explicit release, cancellation,
  revocation, policy reload, daemon loss, MCP EOF, or process exit.

**Gate**

Tests cover controller contention, deny/accept/timeout transfer, wrong-process
and stale handles, mode mismatch, incarnation replacement, input/resize limits,
read-policy denial, cancellation, client death, and complete cleanup. Terminal
text cannot manufacture a handle, confirmation, or follow-up mutation.

### Slice 8 — adversarial security and lifecycle closure

**Work**

- Add prompt-injection fixtures across every terminal-derived field.
- Attempt every mutation with no policy, each wrong scope/resource/incarnation,
  malformed confirmation, stale revision, forged handle, and read-only policy.
- Test four concurrent calls, a fifth waiting/cancelling, stalled daemon, daemon
  restart, MCP client death, stdin EOF, broken stdout, repeated malformed
  requests, full subscription registry, and full controller registry.
- Confirm logs/audit omit terminal, search, input, environment, argv, policy,
  and token bodies while retaining exact mutation resource/outcome metadata.

**Gate**

Every supported third-party operation is reachable with exact authority and
denied without it; a read policy cannot be used for input/control/process
operations; malicious terminal text remains inert quoted data; every process,
connection, controller, transfer, subscription, and task is cleaned up.

### Slice 9 — packaging, client setup, and conformance evidence

**Work**

- Install `splinterm-mcp` separately or through an explicit package option.
- Document exact launch configuration for at least two supported MCP hosts,
  canonical path/digest policy setup, upgrades, revocation, limits, trust labels,
  and troubleshooting.
- Add the official MCP Inspector as a manual interoperability check.
- Run the official conformance server suite where it supports the shipped stdio
  profile. Do not add an HTTP transport merely to satisfy a conformance runner;
  retain a black-box stdio harness for requirements the upstream runner cannot
  exercise.
- Update `THIRD_PARTY.md` and package license/dependency evidence.

**Gate**

A clean installed environment launches the adapter from documented client
configuration, negotiates `2025-11-25`, lists exactly the 32 fixed tools and
three resource forms, performs authorized read, mutation, controller, and
subscription scenarios, denies each under-scoped counterpart, survives
cancellation, and exits without residue when the host disconnects. A reference
in-Splint host uses non-authoritative context hints to select its starting
resource, then demonstrates bounded split, structured child launch, observation,
controller denial handling, and resync reconciliation without claiming native
Wayland window focus or semantic agent supervision.

## Validation contract

After each slice, run the smallest package-specific tests. Before closing the
adapter run:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p splinterd --test end_to_end -- --test-threads=1
python tools/automation/validate-contract-fixtures.py
cargo tree -p splinterm-mcp
```

Also run, with versions pinned in CI or documented tooling:

- the black-box stdio MCP protocol suite;
- supported official MCP conformance scenarios;
- MCP Inspector initialization, tool-list, successful-call, tool-error, and
  cancellation checks; and
- one installed-package policy allow/deny scenario in an isolated no-Wayland
  environment.

No graphical test is required unless trusted graphical consent behavior changes.
If a graphical test becomes necessary, follow the workspace 8 / DP-2 isolation
rules in `AGENTS.md` and run one guarded case before any matrix.

## Required evidence

Phase 4 MCP evidence must record:

- MCP specification version and exact `rmcp` version/features;
- checked-in tool and result schemas plus fixture counts;
- supported host/client versions tested;
- executable canonical path and digest used by policy tests;
- allow/deny scope and confirmation matrix for every tool and resource;
- mutation commit/revision, partial-result, controller, and audit evidence;
- maximum request/result bytes, concurrency, timeout, page, handle, and
  subscription limits;
- cancellation and process/socket/controller/transfer/subscription cleanup
  results;
- prompt-injection and secret/private-field leakage results;
- dependency and license inventory; and
- all deferred capabilities, especially trusted forced takeover, direct policy
  administration, prompts, HTTP transport, sampling, elicitation, and arbitrary
  shell execution.

## Definition of done

This plan is complete when an installed, policy-authorized `splinterm-mcp`
process provides the 32 fixed tools and three subscribable resource forms over
bounded MCP stdio, covering every operation supported for third-party
automation, while unconfigured and under-scoped processes fail closed. Reads,
mutations, subscriptions, and controller operations preserve exact provenance,
resource/incarnation identity, revisions, limits, confirmation, ownership,
commit state, descendant-authority semantics, and audit outcomes. An in-Splint
coding-agent host can select its validated logical context and orchestrate a
bounded child Splint flow without ambient authority, compositor-control claims,
or server-side semantic agent supervision. Every terminal result carries an
untrusted-data label. The adapter exposes no arbitrary shell, filesystem,
network, prompt, sampling, elicitation, trusted forced-takeover, or policy-write
capability; cancellation and disconnect leave no task, transfer, subscription,
controller, child, or socket residue; and the recorded interoperability, schema,
authorization, mutation, prompt-injection, packaging, and full non-graphical
gates pass.

## Stop gates

Stop and request a new architecture decision if implementation requires:

- authorizing the spawned `splinterm` CLI instead of `splinterm-mcp`;
- retaining dynamic descendant matching without an accepted explicit ADR and
  closed policy/schema/audit contract;
- treating inherited Dojo/window/Splint/incarnation hints as authority or an
  implicit mutation target;
- representing logical `new_window` or default-focus mutation as compositor
  mapping or native focus control;
- a network transport or OAuth flow for the required local adapter;
- exposing raw daemon/protocol DTOs as MCP contracts;
- treating terminal content as instructions, prompts, consent, or trusted
  metadata;
- adding trusted forced takeover, direct policy-file administration, arbitrary
  shell strings, filesystem access, sampling, elicitation, or prompts to MCP v1;
- exposing a mutation without its exact policy/resource/revision/controller,
  confirmation, commit-result, and audit contract;
- retaining search/input bodies or completed tool results beyond a request, or
  retaining terminal state/authority outside the bounded live subscription and
  handle registries or beyond the MCP process lifetime;
- weakening executable path/digest policy, resource scopes, controller rules,
  schemas, or response bounds;
- tracking the undated/future MCP draft without a reviewed migration; or
- replacing the official SDK with a custom protocol implementation without a
  documented SDK-spike failure and separate security review.
