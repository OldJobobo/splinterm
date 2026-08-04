# Plan 0015: live appearance control over IPC

- **Status:** Proposed
- **Scope:** transient global appearance overrides for CLI, NDJSON, graphical clients, and MCP
- **Persistence:** daemon lifetime only
- **Base authority:** each graphical frontend's local Omarchy/config theme
- **Security authority:** [ADR 0007](../adr/0007-supported-automation-policy.md)
- **Related behavior:** [configuration](../configuration.md), [automation](../automation.md), and [MCP](../mcp.md)
- **Release decision:** do not advertise live appearance control until protocol, daemon, frontend, schema, MCP, security, graphical, and review gates pass

## Goal

Expose Splinterm colors, alpha, and blur through the existing owner-only IPC and
MCP surfaces so scripts can change every live graphical frontend immediately
without editing Omarchy, Splinterm configuration, or theme files.

The daemon owns only a global **override patch**. It does not own, import, or
report the effective frontend theme. Each frontend continues to resolve its own
local base theme, then applies the daemon patch last.

## Fixed product contract

### State and lifetime

`splinterd` owns one non-persistent state:

```text
AppearanceState {
    revision,
    patch
}
```

- revision `0` starts with an empty patch;
- state survives frontend replacement and applies to frontends opened later;
- state does not survive daemon restart;
- no operation writes a config, state, Omarchy, or theme file;
- no remote theme or terminal content is imported;
- no frontend reports rendered colors or compositor capability to the daemon.

### Patch fields

The patch may independently override:

- background;
- foreground;
- cursor;
- selection;
- URL color;
- UI accent;
- inactive pane border;
- active pane border;
- all 16 ANSI colors as one complete palette;
- default-background alpha; and
- blur requested state.

V1 does not support individual ANSI-slot clearing or replacement. A supplied
ANSI palette must contain exactly 16 colors and is committed as one value.

### Mutations

`set` accepts a nonempty partial patch. It first validates request-local shape
and values without locking. It then takes the non-awaiting appearance-store
mutex and, while holding that one lock, merges against the current patch,
validates the complete candidate, performs checked revision increment, commits,
and publishes with nonblocking queue operations. Subscription registration and
bootstrap snapshot capture use the same mutex. No snapshot/validate/relock gap
is permitted.

- malformed input changes nothing;
- an identical set succeeds with `changed=false` and no revision increment;
- `reset` clears the complete patch;
- resetting an empty patch succeeds with `changed=false`;
- individual fields cannot be cleared in v1;
- revision increment is checked and overflow rejects without mutation.

Set and reset are ephemeral, non-filesystem, idempotent mutations and do not
require a destructive `--yes` confirmation.

### Frontend precedence

Every graphical client retains separate values:

```text
base = local Omarchy or explicit JSON theme + local config overrides
override = latest daemon AppearancePatch
rendered = merge(base, override)
```

Both base reloads and override events recompute a complete `ResolvedTheme`.
Therefore:

- patched fields remain fixed across an Omarchy theme change;
- unpatched fields continue following the local theme;
- reset reveals the newest local base, not the base present when override began;
- base/patch event ordering converges to the same patch-last result;
- one complete rendered theme is delivered to Wayland per accepted update;
- the existing modal latest-theme deferral remains authoritative.

A daemon mutation response confirms daemon commit and publication. It does not
claim that every compositor has rendered the change.

### Read semantics

`appearance get` reports only:

- appearance revision;
- fields currently overridden; and
- omitted fields as inherited.

It must label this data as the **daemon override patch**. It must not call the
result effective, resolved, rendered, local, or remote theme state.

## User-facing API

### Human and machine CLI

```bash
splinterm appearance get
splinterm appearance set --alpha 0.92 --blur on
splinterm appearance set --background '#222222' --foreground '#c2c2b0'
splinterm appearance set --ansi '#000000,...exactly-16-colors...'
splinterm appearance reset

splinterm appearance get --output json
splinterm appearance set --alpha 0.92 --output json
splinterm appearance reset --output json
splinterm subscribe appearance --output ndjson
```

Human and machine command input use canonical `#rrggbb` colors and decimal
alpha `0.0..=1.0`. The private protocol may store alpha exactly as `u16`, using
the same conversion rule as existing config/theme code. Output colors are
canonical lowercase `#rrggbb`; public alpha remains decimal.

Public one-shot operation names:

- `appearance_get`;
- `appearance_set`; and
- `appearance_reset`.

The NDJSON stream emits:

1. `appearance_snapshot` as public sequence 1;
2. ordered `appearance_changed` events carrying the complete override state;
3. `resync_required` with stream `appearance` on sequence/revision gaps or
   bounded-subscriber overflow, then terminates cleanly.

Public CLI schema major remains v1 because existing operation outputs do not
change and appearance records are opt-in additions.

### MCP

Add exactly three tools:

- `splinterm.appearance_get`;
- `splinterm.appearance_set`; and
- `splinterm.appearance_reset`.

Annotations:

| Tool | Read-only | Destructive | Idempotent |
| --- | --- | --- | --- |
| `appearance_get` | yes | no | yes |
| `appearance_set` | no | no | yes |
| `appearance_reset` | no | no | yes |

MCP receives no persistent-theme, filesystem, network, arbitrary color-import,
or effective-rendering capability. MCP appearance subscriptions are deferred;
the three tools are the complete v1 MCP surface.

## Private protocol design

Raise the lockstep private protocol from v24 to v25.

Add renderer-independent DTOs under `splinterm-protocol`:

- `RgbColor { red, green, blue }`;
- `AppearancePatch` with optional fields;
- `AppearanceState { revision, patch }`; and
- mutation result `{ state, changed }`.

Every new appearance struct uses `#[serde(deny_unknown_fields)]`. An
`AppearanceSet` patch must contain at least one recognized field; an empty or
unknown-only object fails before mutation.

Add requests:

- `AppearanceGet`;
- `AppearanceSet { patch }`;
- `AppearanceReset`; and
- `SubscribeAppearance`.

Add responses for snapshot, mutation result, and subscription bootstrap. Add
subscription events for full-state change and resync-required. The subscription
bootstrap must register the subscriber and capture its snapshot under one lock:
a concurrent mutation appears either in the snapshot or as a later event, never
in neither.

No protocol DTO may depend on renderer, Wayland, or `splinterm::config` types.
Mixed v24/v25 components must fail through the existing incompatible-version
handshake rather than partially deserialize.

## Daemon architecture

Add a non-persistent `AppearanceStore` to `DaemonState`. It contains the current
state and a bounded subscriber hub.

Publication rules:

- validate request-local shape and values before locking;
- under one non-awaiting store lock, merge with current state, validate the
  candidate, checked-increment, commit, and `try_send` or mark resync;
- serialize publication by committed revision under that same lock;
- publish the full new state before responding;
- never block request dispatch on a slow subscriber;
- use a small bounded queue, initially capacity 8;
- when full, stop ordinary publication for that subscriber and coalesce the
  newest revision into one resync signal;
- include appearance subscriptions in ownership, detach, cleanup, and global
  subscription admission limits.

Appearance state is intentionally absent from `LairDocument`, metadata backup,
restore, and reset files. Policy reload must not mutate it. Daemon restart must
return it to revision 0 and an empty patch.

## Authorization and audit

Add dedicated automation scopes:

- `appearance_read`;
- `appearance_subscribe`; and
- `appearance_mutate`.

Authorization mapping:

| Request | Required scopes | Policy resource |
| --- | --- | --- |
| get | `appearance_read` | Lair |
| subscribe | `appearance_read`, `appearance_subscribe` | Lair |
| set/reset | `appearance_mutate` | Lair |

Add audit operations:

- `appearance_get`;
- `appearance_set`;
- `appearance_reset`; and
- `subscribe_appearance`.

Authorization uses `PolicyResource::Lair`. Appearance audit records retain the
current `resource: null` projection used for Lair-global operations; v1 does not
add a Lair `AuditResource` variant. Audit records remain body-free and must not
record patch values, base themes, or effective themes. Update policy/audit closed
enums and scope bounds from 18 to 21 while preserving all existing policy
fixtures.

The frontend uses the established trusted graphical-client authority path. MCP,
CLI machine clients, and relay clients remain subject to executable identity,
scope, resource, and limit policy. SSH transport does not import identity or
colors and grants no appearance authority by itself.

## Frontend architecture

Add one dedicated appearance subscription connection per graphical frontend.
Complete its race-free initial snapshot and merge it into the local base before
both `renderer::configure` and every initial `WindowOptions.theme`, then map the
Wayland surface. A future frontend must never initialize alpha-dependent
renderer/buffer state or map a frame from the unpatched base first.

Replace direct theme-watcher-to-Wayland delivery with a small coordinator:

```text
local theme watcher ── base ─────┐
                                 ├─ appearance reducer ─ complete ResolvedTheme ─ Wayland
appearance subscription ─ patch ─┘
```

The coordinator owns the latest base and daemon state. It rejects regressing
appearance revisions and applies only validated full states.

On stream gap or explicit resync:

1. stop applying incremental appearance events;
2. fetch or resubscribe to an authoritative snapshot;
3. resume only after revision reconciliation.

On connection loss, retain the last patch for at most 2.5 seconds while making
up to 50 config-independent reconnect attempts at 50 ms intervals. Resume event
processing only after an authoritative subscription snapshot. If that window
expires, clear the transient patch, render the local base, keep the graphical
window alive, and continue low-frequency one-second reconciliation attempts. A
later authoritative snapshot always replaces the retained or cleared state; a
restarted daemon therefore reveals revision 0/empty state. Paused-time tests
must cover timeout clearing and both same-daemon and restarted-daemon recovery.

Wayland continues receiving only complete `ResolvedTheme` values. Existing
color redraw classification, alpha buffer reconciliation, blur object lifecycle,
and session-picker deferred-theme behavior stay unchanged.

## Implementation milestones

### Milestone 1: protocol vocabulary and wire validation

**Files**

- `crates/splinterm-protocol/src/lib.rs`

**Work**

- add appearance DTOs, requests, responses, events, scopes, and audit operations;
- add strict validation and checked revision behavior;
- bump private protocol to v25;
- add golden round-trip and incompatible-version tests.

**Validation**

```bash
cargo test -p splinterm-protocol
cargo fmt --all -- --check
git diff --check
```

**Gate**

Independent protocol/security review must approve representation, no-op revision
semantics, unknown-field handling, and v25 behavior before daemon work proceeds.

### Milestone 2: daemon store, hub, authorization, and audit

**Files**

- `crates/splinterd/src/main.rs`
- `crates/splinterd/src/authorization.rs`
- `crates/splinterd/src/policy.rs`
- `crates/splinterd/src/audit.rs`
- `crates/splinterd/tests/end_to_end.rs`

**Work**

- implement atomic get/set/reset and race-free subscription bootstrap;
- implement bounded publication and explicit resync;
- wire detach, cleanup, limits, request dispatch, and resources;
- add dedicated authorization/audit mappings;
- prove no persistence across daemon restart.

**Required tests**

- initial revision 0/empty state;
- partial accumulation and whole ANSI replacement;
- invalid atomic rejection and revision overflow;
- same-value/no-op mutation behavior;
- multiple subscribers and future subscriber snapshot;
- deterministic subscribe-versus-set race covering both legal interleavings;
- slow-subscriber overflow to newest resync revision;
- scope/resource denial and trusted-UI separation;
- audit metadata excludes theme bodies;
- restart clears appearance state.

**Validation**

```bash
cargo test -p splinterd --lib
cargo test -p splinterd --test end_to_end -- --test-threads=1
```

**Gate**

Concurrency and authorization review before any public client integration.

### Milestone 3: automation client and relay compatibility

**Files**

- `crates/splinterm-automation-client/src/lib.rs`
- `crates/splinterm-relay/tests/stdio.rs`
- protocol-version expectations in packaging validation

**Work**

- add typed appearance one-shot/subscription helpers;
- preserve correlation, sequence, cancellation, and bounded-queue behavior;
- map change/resync/disconnect failures explicitly;
- update v25 expectations while leaving relay transport-transparent.

**Validation**

```bash
cargo test -p splinterm-automation-client
cargo test -p splinterm-relay
```

### Milestone 4: frontend base-plus-patch reducer

**Files**

- `crates/splinterm/src/config.rs`
- `crates/splinterm/src/main.rs`
- `crates/splinterm/src/wayland.rs`

**Work**

- add pure patch merge/conversion helpers;
- separate local base publication from rendered theme delivery;
- subscribe before initial map;
- integrate reducer into managed and single-window paths;
- preserve theme changes across frontend/session replacement;
- implement bounded resubscription and stale-patch clearing.

**Required tests**

- every patch field and whole ANSI replacement;
- base reload under active patch;
- base/patch arrival in both orders converges identically;
- reset reveals newest base;
- two local bases produce different rendered themes from one patch;
- startup snapshot precedes both renderer configuration and initial
  `WindowOptions.theme` in every graphical entry path;
- gap, regression, resync, disconnect, daemon restart, and recovery;
- modal deferral and alpha/blur reconciliation.

**Validation**

```bash
cargo test -p splinterm --lib --bin splinterm
cargo clippy -p splinterm --no-deps --lib --bin splinterm -- -D warnings
```

**Gate**

Frontend lifecycle and Wayland review before public CLI exposure.

### Milestone 5: CLI JSON/NDJSON contracts

**Files**

- `crates/splinterm/src/main.rs`
- `crates/splinterm/src/automation.rs`
- `crates/splinterm-automation-client/src/lib.rs`
- `crates/splinterm/tests/automation_cli.rs`
- `dist/schemas/v1/cli-envelope.schema.json`
- `dist/schemas/v1/cli-event.schema.json`
- valid/invalid automation fixtures

**Work**

- add human get/set/reset and appearance subscription commands;
- add closed JSON one-shot and NDJSON event projections;
- canonicalize colors and alpha;
- preserve one-document stdout and current exit categories;
- document inherited fields and commit-versus-render semantics.

**Required tests**

- exact human, JSON, and NDJSON records;
- empty/partial/full/no-op/reset states;
- malformed colors, alpha, ANSI length, denial, timeout, and v24 mismatch;
- resync record then clean stream termination;
- no stdout diagnostics and no filesystem modifications;
- all prior public fixtures remain valid and unchanged.

**Validation**

```bash
cargo test -p splinterm --test automation_cli -- --test-threads=1
uv run --with jsonschema python tools/automation/validate-contract-fixtures.py
```

### Milestone 6: policy schemas, ADR, and automation documentation

**Files**

- `dist/schemas/v1/policy.schema.json`
- `dist/schemas/v1/audit-record.schema.json`
- `docs/adr/0007-supported-automation-policy.md`
- `docs/automation.md`

**Work**

- add three scopes and four audit operations to closed contracts;
- document Lair-global resource semantics, bounds, no persistence, and no
  effective-theme reporting;
- validate old policy/audit fixtures and new allow/deny examples.

**Gate**

Security/schema review must reconcile protocol, daemon tables, JSON schema, and
ADR line by line.

### Milestone 7: MCP tools

**Files**

- `crates/splinterm-mcp/src/tools.rs`
- `crates/splinterm-mcp/src/dispatch.rs`
- `crates/splinterm-mcp/src/server.rs`
- `crates/splinterm-mcp/tests/schema_inventory.rs`
- `crates/splinterm-mcp/tests/stdio_protocol.rs`
- `dist/schemas/mcp/v1/common.schema.json`
- new appearance tool input/output schemas
- `docs/mcp.md`

**Work**

- add and dispatch the three fixed tools;
- add six strict appearance tool input/output schemas;
- keep the discoverable resource-schema inventory at exactly three and use the
  existing daemon resource for appearance tool results;
- update the exact tool inventory from 32 to 35 and reviewed schema inventory
  from 69 to 75;
- make `common.schema.json` the only pre-existing MCP schema whose hash changes:
  its closed `scope` enum gains the three appearance scopes and its closed
  `tool_name` enum gains the three appearance tools; preserve every other
  pre-existing schema hash;
- test policy denial, idempotence, malformed input, bounds, and daemon failures.

**Validation**

```bash
cargo test -p splinterm-mcp --test schema_inventory
cargo test -p splinterm-mcp --test stdio_protocol -- --test-threads=1
cargo test -p splinterm-mcp
```

### Milestone 8: integrated non-graphical closure

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -- --test-threads=1
uv run --with jsonschema python tools/automation/validate-contract-fixtures.py
git diff --check
```

The known repository-wide Clippy baseline must be handled explicitly rather
than silently weakening warnings. Record focused race, overflow, schema, MCP,
and protocol-version evidence. Complete fresh protocol/security/concurrency and
public-contract reviews before graphical testing.

### Milestone 9: guarded graphical smoke

This milestone requires separate approval under the repository graphical-test
rules.

- workspace 8 only;
- monitor DP-2 only;
- workspace 8 inactive before launch;
- pre-map placement and no-focus rules;
- never switch the user to workspace 8 or focus test windows;
- abort and clean up immediately on placement/focus violation.

Use an isolated daemon/socket/config and two frontends with distinct local base
themes:

1. map frontend A with empty override;
2. set colors, ANSI, alpha, and blur and verify one complete transition;
3. map frontend B and prove the current patch is applied before initial display;
4. reload A's local base and prove only inherited fields change;
5. reset and prove each frontend returns to its own newest base;
6. record appearance revisions, screenshots, placement/focus evidence, and full cleanup.

A failed smoke blocks any broader graphical matrix.

## Non-goals

- persistent appearance state;
- theme/config/Omarchy file writes;
- effective or rendered frontend-state reporting;
- remote-color import;
- frontend rendering acknowledgements;
- per-client, window, pane, Dojo, or Splint overrides;
- per-slot ANSI mutation;
- individual-field clearing;
- terminal escape-sequence color control through this API;
- MCP appearance subscriptions;
- HTTP/network listeners;
- frame-perfect simultaneous compositor presentation.

## Principal risks

1. **Initial-map race:** a frontend must receive the authoritative subscription
   snapshot before mapping.
2. **Lost base:** storing only a merged theme would make reload/reset incorrect;
   base and patch must remain separate.
3. **Slow subscriber:** publication must remain bounded and surface resync.
4. **Stale patch after daemon restart:** disconnect recovery must reconcile or
   clear the transient patch without killing the usable frontend.
5. **Exhaustive security drift:** request, scope, resource, audit, schema, docs,
   and projection tables must change together.
6. **Lockstep upgrade:** protocol v25 requires daemon/client/package agreement.
7. **Public naming ambiguity:** every response must say override patch, never
   effective theme.
8. **Alpha unit drift:** human/public decimal and private exact representation
   need explicit conversion tests.

## Completion criteria

This plan is complete only when:

- all nine milestone gates have recorded evidence;
- no appearance operation modifies a filesystem theme/config path;
- current and future frontends converge to patch-last behavior;
- daemon restart clears overrides;
- authorization and audit contracts fail closed;
- CLI and MCP schemas are reviewed and validated;
- existing public outputs remain compatible;
- graphical smoke passes under workspace/monitor/focus isolation; and
- final independent review records no blocker or fix worth doing now.
