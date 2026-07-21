# Plan 0006: headless access and supported automation

- **Status:** In progress
- **Roadmap:** Phase 4 — Headless access and supported automation
- **Foundation:** [Plan 0004](0004-phase3-multiplexing.md), [ADR 0005](../adr/0005-trusted-consent-broker.md), [ADR 0006](../adr/0006-multiplexing-lifecycle.md)
- **Reference source:** Foot 1.27.0, commit `3c5b584b0eafa772eb4376fb6eaf6643399e190e`

## Goal

Turn Splinterm's private local control path into a supported, least-privileged
automation and headless-access surface without making the daemon a network
service:

1. `splinterd` runs as a documented user service on graphical and headless Linux
   hosts;
2. scripts and integrations use stable JSON/NDJSON contracts rather than human
   CLI text or private protocol details;
3. third-party access is explicit, resource-scoped, bounded, visible where a
   trusted UI exists, and inspectable afterward;
4. remote access is carried by authenticated SSH through a narrow relay or Unix
   socket forwarding; and
5. optional adapters, including MCP, remain disposable clients with no implicit
   authority.

Phase 4 does not turn terminal output into trusted instructions. All screen and
scrollback content remains untrusted data even when an automation client launched
the process that produced it.

## Current-state constraints

- `splinterd` is already Wayland-independent, binds only an owner-only Unix
  socket, verifies Linux peer credentials, and owns PTYs while no client is
  attached. What is missing is a supported headless service and authorization
  workflow, not a second daemon.
- Protocol v17 already has bounded framing, negotiation, request IDs, stable
  errors, cancellation, subscriptions, revisions, resynchronization, explicit
  Splint/incarnation identity, and per-Splint controller leases.
- The current `splinterm` CLI mixes human output with development-only terminal
  operations. Its serialized Rust DTOs are internal wire types, not a promised
  public CLI schema.
- Grant-once consent depends on a daemon-launched trusted Wayland surface. It
  fails closed on a headless host and grants are intentionally lost on daemon
  restart.
- Existing access scopes focus on terminal data and control. Metadata, spawn,
  topology mutation, restore, policy inspection, and audit inspection do not yet
  have a complete public authorization matrix.
- Audit records are bounded in memory and intentionally exclude terminal,
  clipboard, and input bodies, but there is no supported inspection or durable
  audit contract.
- The daemon sees the peer connected to its Unix socket. With a stdio relay that
  peer is the relay process, not the originating remote client; the design must
  not pretend otherwise.

## Product decisions fixed by this plan

1. **One daemon protocol:** local UI, CLI automation, editor integrations, relay,
   and MCP use the same versioned daemon operations and authorization checks.
   No adapter receives a private bypass.
2. **Public CLI boundary:** JSON/NDJSON envelopes and published JSON Schemas are
   the compatibility promise. Raw protocol frames and Rust enum layouts remain
   internal and may change with protocol versions.
3. **Output modes:** human output remains the interactive default. `--output
   json` emits exactly one JSON document; `--output ndjson` emits one bounded
   event per line and is required for subscriptions. Machine modes reserve
   stdout for schema-conforming data and send diagnostics to stderr.
4. **Compatibility:** every machine envelope carries a CLI schema name and major
   version. Additive fields are allowed within a major version; removal,
   semantic reinterpretation, or type changes require a new major version.
5. **Headless authorization:** absence of trusted graphical consent fails closed
   unless the user installs an explicit owner-only policy. Headless mode never
   enables the development attach bypass automatically.
6. **Policy identity:** persistent rules match a canonical executable identity,
   explicit operation scopes, resource selectors, and limits. Basenames, argv,
   client-supplied labels, and same-UID status alone are never sufficient.
   Exact identity representation and upgrade behavior must be settled by ADR and
   adversarial spike before implementation.
7. **Policy defaults:** no policy file means no persistent third-party grants.
   Rules default to metadata-only, have optional expiry, and cannot grant more
   than their declared resources and limits. Policy reload is fail-closed and
   never broadens an already running request silently.
8. **Audit:** authorization decisions and sensitive operation outcomes are
   recorded as bounded metadata with monotonic IDs. Terminal rows, scrollback,
   clipboard bodies, input bytes, tokens, environment contents, and complete
   command arguments are never audited.
9. **Remote transport:** `splinterd` remains Unix-socket-only. The supported
   remote path is `splinterm relay --stdio` over SSH. Unix-socket forwarding may
   be documented only if its ownership, path, and peer-identity behavior pass the
   same security review.
10. **Relay trust:** SSH authenticates the host and login; it does not grant
    Splinterm scopes. The daemon authorizes the local relay executable under an
    explicit policy. Documentation states that this delegates the selected
    authority to callers able to invoke that relay under the account.
11. **Controller semantics:** observation, attachment, focus, or subscription
    never acquires input/resize control. Automation must explicitly acquire or
    request transfer under the Phase 3 lease rules.
12. **MCP:** `splinterm-mcp` is a separate stdio adapter, disabled unless
    installed/configured, and read-mostly by default. It is not required for the
    core Phase 4 completion gate.

## Non-goals

- a TCP, HTTP, WebSocket, or gRPC listener in `splinterd`;
- LAN discovery, firewall changes, automatic port forwarding, or custom SSH key
  management;
- treating same-user SSH access as blanket terminal authorization;
- process or PTY continuity across daemon/host restart;
- durable terminal or scrollback bodies;
- shell-string construction, implicit `sh -c`, or an unrestricted "run command"
  tool against an existing shell;
- collaborative typing, shared controller leases, or automatic takeover;
- persistent clipboard transport through the daemon;
- a general plugin runtime or a commitment to multiple editor plugins;
- public/AUR publication, Nix packaging, or sandboxed distribution; and
- changing the Foot-derived terminal or renderer behavior.

## Architectural invariants

- `splinterd` remains the sole writer of topology, runtime state, grants, policy
  state, and audit order.
- Transport does not confer authority. Every sensitive request is checked after
  transport authentication against exact operation, peer, resource,
  incarnation, and applicable limits.
- Public DTOs are explicit conversions; protocol/runtime structs are not
  serialized directly as CLI promises.
- All request, response, event, policy, relay, and audit allocations are bounded
  before use. Slow subscribers resynchronize rather than blocking PTYs.
- No daemon lock is held across policy filesystem I/O, consent UI, audit writes,
  PTY actor requests, relay I/O, or protocol writes.
- Policy and audit files use owner-only, no-symlink, bounded, atomic storage
  rules. Invalid policy never falls back to a broader previous interpretation.
- Terminal reads include exact Splint/incarnation/revision provenance,
  truncation/continuation metadata, and resynchronization state.
- Relay stdout carries protocol bytes only. Logs and diagnostics use stderr and
  never include terminal or input bodies.
- Closing a CLI, relay, editor, or MCP client cancels its subscriptions and
  releases its controllers without ending daemon-owned processes.

## Public capability direction

Before adding CLI output, define an operation-to-scope matrix. At minimum it
must distinguish:

- topology metadata read and topology subscription;
- visible-screen read and terminal-event subscription;
- scrollback read and search;
- input and resize/controller acquisition;
- structured process spawn/relaunch/restore;
- layout/name mutation;
- process termination and destructive close;
- grant/status/revocation inspection; and
- policy/audit inspection and administration.

Scopes are closed enums. New sensitive operations default to unauthorized until
the matrix assigns them. Broad wildcard grants are not part of the initial
policy format. Resource selectors use stable IDs; optional Dojo/window selectors
expand to an explicit bounded set at authorization time and do not silently
cover resources created later unless the rule says so conspicuously.

The public machine envelope should have this shape in principle:

```json
{
  "schema": "splinterm.cli.v1",
  "request_id": "1",
  "ok": true,
  "resource": {
    "splint_id": "…",
    "incarnation": 3,
    "terminal_revision": 42
  },
  "data": {},
  "truncated": false
}
```

Errors use the same envelope with a stable symbolic code, bounded message,
retryability, and current revision where relevant. NDJSON adds event sequence,
subscription identity, event type, and explicit `resync_required` records.
Terminal bytes that cannot be represented as semantic cells use an explicitly
named encoding; they are never inserted into JSON strings ambiguously.

## Execution checkpoint — resumed and closed after reboot

Recorded 2026-07-20 before an unrelated host reboot.

### Completed implementation

Slice 0 implementation is present in the worktree:

- ADR 0007 now records the Linux 6.5+ `SO_PEERPIDFD` requirement;
- `docs/spikes/0020-persistent-executable-identity.md` records the adversarial
  identity spike and accepted open-descriptor hashing algorithm;
- the five focused executable-identity tests pass;
- handwritten public v1 schemas exist under `dist/schemas/v1/` for one-shot CLI
  envelopes, NDJSON events, persistent policy, and audit records;
- five valid and five security-negative fixtures exist under
  `tests/automation/fixtures/`;
- `tools/automation/validate-contract-fixtures.py` validates those fixtures and
  is wired into CI; and
- `docs/automation.md` documents the draft public/private compatibility boundary.

Treat `AGENTS.md` as user-owned and do not alter it as part of this work. No
Phase 4 changes have been committed yet.

### Closure completed

Closed 2026-07-20. The intermittent shutdown-ordering race was repaired without
retries or filesystem-only serialization:

- process-exit observers now run under a daemon-owned `TaskTracker` that removes
  completed task outputs;
- shutdown stops and awaits connection tasks, closes the observer tracker,
  shuts down live runtimes, and awaits every observer with an explicit bounded
  timeout;
- final Lair persistence occurs only after observer reconciliation and while
  holding the topology transaction barrier;
- the socket is removed only after that final durable save; and
- the integration harness preserves and reports a bounded daemon stderr tail on
  failure.

The formerly failing serialized smoke passed:

```bash
cargo test -p splinterd --test end_to_end \
  explicit_restore_scopes_report_per_leaf_results \
  -- --exact --test-threads=1
```

The complete closure gate then passed: all ten contract fixtures validated
against four schemas, formatting and workspace Clippy passed with warnings
denied, the full workspace test suite passed, and all seven serialized
`splinterd` end-to-end tests passed. The local validator used an ephemeral
`uv --with jsonschema` environment because the host Python did not have the
CI-documented `jsonschema` dependency installed. Post-gate review confirmed
that `TaskTracker` removes completed tasks immediately, its closed-and-empty
wait semantics match the shutdown requirement, and every observer-registration
call site is behind a connection task drained before tracker closure.

No graphical test was required or run for this repair.

## Dependency-ordered implementation slices

### Slice 0 — security ADR, threat model, and contract fixtures (complete)

**Work**

- Write an ADR covering persistent policy identity, executable replacement and
  package upgrades, owner-only policy loading, relay trust, consent fallback,
  audit retention, and administrative operations.
- Threat-model malicious same-UID processes, copied/replaced executables, stale
  rules, symlink/path substitution, relay impersonation, SSH disconnects,
  terminal prompt injection, oversized streams, subscription stalls, and replay
  of stale resource/incarnation IDs.
- Define the operation-to-scope matrix and classify every current v17 request.
- Check in draft JSON Schemas and golden valid/invalid fixtures for one-shot
  responses, errors, subscription events, policy, and audit records.
- Decide whether schemas are handwritten or generated only after a small spike;
  generated schemas still require reviewed checked-in artifacts and compatibility
  tests.

**Likely files:** new `docs/adr/0007-supported-automation-policy.md`, this plan,
`docs/automation.md`, new schema/fixture directories under `dist/` or `tests/`.

**Exit gate:** no persistent policy, relay, or public machine output lands until
review can answer exactly which identity and scope authorizes every operation,
including on a host with no graphical session.

### Slice 1 — complete authorization and inspectable audit (complete)

**Work**

- Extend closed access scopes and centralize an exhaustive request-to-scope
  authorization table in `splinterd`; remove scattered implicit assumptions.
- Separate trusted first-party UI behavior from third-party policy decisions.
- Add bounded policy parsing, validation, owner/mode checks, atomic reload, and
  explicit diagnostics. Begin with exact stable resource IDs and conservative
  limits; add broader selectors only with tests.
- Make audit records wire DTOs and add paginated inspection by monotonic cursor.
  Record grant, deny, revoke, expiry, policy match/reject, controller transfer,
  spawn/restore, topology mutation, and termination outcomes without bodies or
  complete argv.
- Define retention and restart semantics honestly. If durable audit is selected,
  use a separately bounded append/checkpoint design; otherwise label the API as
  daemon-lifetime-only.

**Likely files:** `crates/splinterm-protocol/src/lib.rs`,
`crates/splinterd/src/{main,consent}.rs`, a new focused policy/audit module,
`crates/splinterd/tests/end_to_end.rs`.

**Gate:** a matrix test proves every sensitive request is denied with no consent
or policy, narrowly allowed by its exact rule, denied outside resource/scope/
limit, revoked on policy removal or incarnation change, and represented by a
body-free audit record.

**Completion evidence (2026-07-20):**

- all daemon requests pass through one exhaustive operation-scope table and one
  exact request-to-resource translator; connection-owned and trusted-UI
  authority remain separate from persistent policy;
- persistent identity uses `SO_PEERPIDFD`, verifies it against `SO_PEERCRED`,
  hashes the bounded opened executable outside daemon locks, and monitors peer
  exit for the connection lifetime;
- policy loading walks every path component with `O_NOFOLLOW`, enforces bounded
  owner/mode/schema/rule validation, publishes complete generations atomically,
  and installs deny-all on rejection;
- exact path/digest, scope, resource/incarnation, returned row/result/byte,
  live-subscription, deadline, and cumulative spawn limits are enforced;
- `SIGHUP` reload disconnects existing clients and revokes all connection-owned
  controllers, transfers, and subscriptions before the new generation serves
  requests;
- audit wire DTOs and `AuditInspect` provide monotonic cursor pages over the
  newest 1,024 daemon-lifetime records with explicit retention gaps; records
  include bounded identities and symbolic outcomes but no terminal, input,
  search, environment, capability, or complete argv bodies; and
- post-implementation review closed future-request trusted bypass, exact grant
  resource selection, `Detach` audit coverage, per-authority reload revocation,
  cumulative spawn/subscription accounting, and serialized response/event byte
  ceilings.

The guarded serialized smoke and complete non-graphical gate passed:

```bash
cargo test -p splinterd --test end_to_end \
  explicit_restore_scopes_report_per_leaf_results \
  -- --exact --test-threads=1
uv run --with jsonschema python tools/automation/validate-contract-fixtures.py
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p splinterd --test end_to_end -- --test-threads=1
```

No graphical test was required or run for Slice 1.

### Slice 2 — stable JSON/NDJSON CLI

**Work**

- Add a reusable non-Wayland automation client boundary rather than growing more
  ad hoc branches in the graphical `main.rs`.
- Add global output mode, schema-major selection, timeout, and cancellation
  options. Preserve human prompts only in human mode; destructive machine calls
  require explicit confirmation flags.
- Cover topology/list/inspect, lifecycle and layout mutation, bounded snapshot,
  scrollback/search, authorization status/revoke, controller operations, input,
  and audit inspection according to the scope matrix.
- Add NDJSON terminal/topology/control subscriptions with ordered events,
  provenance, bounded queue behavior, clean cancellation, and explicit resync.
- Publish checked-in schemas, examples, exit-code table, compatibility policy,
  and shell-safe invocation guidance. Keep stdout pristine in machine modes.

**Likely files:** `crates/splinterm/src/main.rs`, new
`crates/splinterm/src/automation.rs` or a small dedicated client crate,
`crates/splinterm-protocol`, `dist/schemas/`, `docs/automation.md`.

**Gate:** golden tests validate every emitted document against the published
schema; a subprocess test proves stdout contains only JSON/NDJSON while warnings
remain on stderr; older v1 fixtures remain byte/semantics compatible after
additive changes.

### Slice 3 — supported headless service and policy workflow

**Work**

- Make the packaged systemd user service usable without Wayland variables or a
  running compositor and document on-demand versus persistent service lifetime.
- Add CLI commands to validate, inspect, and reload policy without exposing a
  policy-write RPC to ordinary automation clients. Provide secure file creation
  examples rather than editing policy automatically.
- Document login/logout behavior, administrator-approved lingering, service
  accounts, runtime/state directories, backups, upgrades, and recovery. Never
  enable lingering or modify SSH/system policy from package scripts.
- Add a headless integration harness with an empty graphical environment that
  launches the daemon, loads an isolated policy, restores/starts selected
  Splints explicitly, exercises authorized automation, restarts the daemon, and
  verifies honest process-loss/policy/audit semantics.

**Likely files:** `dist/systemd/splinterd.service`, packaging docs/artifacts,
`docs/headless.md`, `tools/` headless harness, daemon integration tests.

**Gate:** on an isolated no-Wayland environment, supported CLI automation works
only under the installed policy; consent-required requests fail closed; daemon
restart never auto-runs saved commands; shutdown reaps children and leaves no
socket/process residue.

### Slice 4 — SSH stdio relay

**Work**

- Add `splinterm relay --stdio` as a byte-transparent, bounded, full-duplex bridge
  between stdin/stdout and one validated local daemon socket.
- Preserve protocol negotiation, request IDs, cancellation, backpressure, EOF,
  half-close, and daemon errors without parsing terminal output or minting
  authority in the relay.
- Reject TTY stdin/stdout by default, reserve stderr for bounded diagnostics, set
  safe descriptor inheritance, and terminate promptly on either-side failure.
- Document host-key verification, exact SSH command construction, policy needed
  for the relay identity, logout/service lifetime, and the authority delegated to
  anyone able to invoke the relay under that account.
- Spike Unix-socket forwarding separately. Publish it only if permissions,
  cleanup, path expansion, and peer identity are no weaker or more confusing
  than stdio relay.

**Likely files:** `crates/splinterm/src/main.rs` or a dedicated adjacent binary,
relay module/tests, `docs/remote.md`, packaging completions/man pages.

**Gate:** a localhost-SSH or transport-equivalent integration test survives
large frames, stalled readers, cancellation, daemon restart, relay/SSH death,
and malformed lengths; unauthorized operations remain denied and cleanup leaves
no child, task, or socket residue.

### Slice 5 — reference editor/client integrations

**Work**

- Publish a client-author checklist covering schema negotiation, stable IDs,
  deadlines, cancellation, resync, controller ownership, untrusted terminal
  content, and revocation.
- Ship one narrow reference integration, preferably an editor task/session picker
  that lists topology and opens an existing selected window or starts a direct
  argv launch through the CLI. It must not parse human output or copy the daemon
  protocol implementation.
- Add shell and `jq` examples for common read-only workflows and explicit,
  confirmed mutation. Avoid shell interpolation of command arrays.
- Test the reference integration against schema fixtures and one isolated daemon;
  keep editor-specific UX outside the daemon and protocol crates.

**Gate:** the reference client uses only documented public CLI contracts, handles
not-found/stale/resync/denied outcomes, and requires no development bypass or
private Rust API.

### Slice 6 — optional read-mostly MCP adapter

This slice is non-blocking for core Phase 4 completion.

**Work**

- Create a separate `splinterm-mcp` stdio process that invokes the supported
  client boundary and requests the same capabilities as any third party.
- Default tools to topology metadata, bounded visible-screen reads, bounded
  search, and audit/status inspection. Omit input, termination, arbitrary shell
  execution, policy administration, and forced takeover by default.
- Return provenance, truncation, stale/resync, and explicit untrusted-content
  labels with every terminal read. Tool descriptions must not imply terminal
  prose is instruction or consent.
- Require explicit user configuration to expose any mutating tool and preserve
  daemon consent/policy/controller checks. The adapter stores no reusable daemon
  authority beyond its process lifetime.
- Add protocol conformance, cancellation, output-bound, prompt-injection fixture,
  and denied-capability tests; package the adapter separately or as an opt-in
  component.

**Gate:** an unconfigured adapter cannot mutate a Splint; a read grant cannot be
used for input or process control; malicious terminal text remains quoted data
in the MCP result and cannot create a tool call or broaden authority.

### Slice 7 — closure, documentation, and package evidence

- Run the complete authorization matrix, schema compatibility suite, isolated
  daemon lifecycle suite, headless service scenario, and one guarded relay
  scenario. No graphical matrix is required unless trusted consent UI itself
  changes.
- Record queue/memory/CPU behavior for idle NDJSON subscriptions, high terminal
  output, stalled relay/client, audit retention, and policy reload. Preserve the
  Phase 2/3 baseline for unrelated terminal/rendering paths.
- Update architecture, roadmap, README, packaging, service, security, automation,
  headless, and remote documentation. Clearly separate local consent,
  user-installed headless policy, SSH authentication, Splinterm authorization,
  and controller ownership.
- Record deferred work: non-SSH gateways, durable terminal bodies, broader editor
  plugins, public distribution, Nix, and write-capable MCP defaults.

## Validation contract for every implementation slice

Run the smallest relevant unit/package tests after each dependency-ordered
change. Before closing a code slice run:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p splinterd --test end_to_end -- --test-threads=1
```

Public schema changes additionally run fixture validation and compatibility
tests. Relay/headless tests use isolated runtime, state, config, and home
directories with bounded timeouts and explicit process/socket cleanup. Run one
guarded case before any remote matrix. Graphical tests, if needed, follow the
workspace-8/DP-2 isolation rules in `AGENTS.md`.

## Phase 4 definition of done

Core Phase 4 is complete when a user can install an explicit least-privileged
policy on a no-Wayland host, manage persistent Splints through documented stable
JSON/NDJSON CLI contracts, inspect bounded authorization audit metadata, and use
the same contracts remotely through an SSH stdio relay. Every operation remains
resource/scoped, controller-exclusive, revision-aware, bounded, cancellable, and
fail-closed; terminal content is labeled untrusted; `splinterd` exposes no
network listener; and package/service documentation states logout, lingering,
upgrade, and process-loss behavior honestly.

A reference editor/client integration is required. The read-mostly MCP adapter is
an optional follow-up and does not block this definition of done.

## Stop gates

Stop and request a new architecture decision if implementation requires:

- a network listener or remote credentials inside `splinterd`;
- treating SSH login, same UID, executable basename, argv, or client labels as
  sufficient authorization;
- a headless policy that silently grants all resources or future resources;
- exposing raw internal protocol/Rust DTOs as the stable public CLI promise;
- bypassing consent/policy/controller checks in relay, editor, or MCP clients;
- logging terminal, scrollback, clipboard, input, token, environment, or complete
  command bodies;
- constructing shell command strings from automation parameters;
- persisting terminal bodies or reusable adapter authority;
- changing the pinned Foot oracle or renderer behavior; or
- broad editor/MCP work before authorization, schemas, headless service, and
  relay gates pass.
