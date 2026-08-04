# Plan 0018: Lair and Dojo topology migration

- **Status:** Complete
- **Date:** 2026-08-04
- **Supersedes terminology in:** [ADR 0006](../adr/0006-multiplexing-lifecycle.md)
- **Related UI work:** [Plan 0017](0017-inline-session-picker-overlay.md)

## Goal

Replace the persistent hierarchy

```text
Lair → Dojo → logical Window → Splint
```

with

```text
splinterd
└── Topology
    └── Lair
        └── Dojo
            └── Splint
```

`Topology` is the internal daemon-owned catalog. A Lair is the named persistent
session or project boundary. A Dojo is one persistent terminal layout whose
layout-tree leaves are Splints. `Window` is reserved for disposable native
Wayland presentation and is no longer a durable logical resource.

The migration must preserve existing topology, UUID values, launch metadata,
process incarnations, explicit restore behavior, authorization safety, and the
daemon's clone-mutate-persist-install-publish transaction ordering.

## Target model

```rust
pub struct Topology {
    revision: TopologyRevision,
    lairs: BTreeMap<LairId, Lair>,
}

pub struct Lair {
    pub id: LairId,
    pub name: String,
    pub dojos: Vec<Dojo>,
}

pub struct Dojo {
    pub id: DojoId,
    pub name: String,
    pub default_focus: SplintId,
    pub root: LayoutNode,
}
```

Invariants:

- One topology-wide monotonic revision and transaction domain remains authoritative.
- Lair IDs, Dojo IDs, and Splint IDs are stable UUID-backed identities.
- Lair names are trimmed, nonempty, at most 128 bytes, and globally unique.
- Dojo names are trimmed, nonempty, and at most 128 bytes. Duplicate Dojo names
  are allowed because existing logical Window titles are not unique.
- A Lair may contain no Dojos, preserving an existing named session after its
  final layout is removed.
- A Dojo owns one binary layout tree and one persisted default-focus hint.
- Native Wayland windows are client-local presentation and have no durable topology ID.
- Live PTYs and processes remain daemon-runtime state separate from durable topology.
- Startup never executes saved commands automatically; restore remains explicit.

## Exact legacy mapping

Persistence schema v2 converts losslessly to schema v3:

| Schema v2 | Schema v3 |
| --- | --- |
| root `Lair` | `Topology` |
| `Dojo` | `Lair` |
| `DojoId` UUID | `LairId` with the same UUID |
| `Dojo.name` | `Lair.name` |
| logical `Window` | `Dojo` |
| `WindowId` UUID | `DojoId` with the same UUID |
| `Window.title` | `Dojo.name` |
| `Window.root` | `Dojo.root` |
| `Window.default_focus` | `Dojo.default_focus` |
| `Splint` | unchanged |

A legacy Dojo containing several logical Windows becomes one Lair containing
several Dojos. An empty legacy Dojo becomes an empty Lair. UUID text remains
unchanged even though its semantic wrapper type changes.

## Naming and operation mapping

| Existing operation | Replacement |
| --- | --- |
| `ListDojos` | `ListLairs` |
| `CreateDojo` | `CreateLair` |
| `NewWindow` | `NewDojo` |
| `RestoreWindow` | `RestoreDojo` |
| `RestoreDojo` | `RestoreLair` |
| `CloseWindow` | `CloseDojo` |
| `RenameWindow` | `RenameDojo` |
| `RenameDojo` | `RenameLair` |
| `SetWindowDefaultFocus` | `SetDojoDefaultFocus` |
| `WindowStarted` | `DojoStarted` |
| `DojoCreated` | `LairCreated` |

The graphical `splinterm window` command may remain because it opens a native
Wayland window. Its persistent selectors become `--lair-id` and `--dojo-id`.
Presentation-side types such as `WindowOptions`, `WindowCommand`,
`WindowUpdate`, and `WindowGeometry` remain Window types.

## Compatibility policy

Splinterm is still a private prerelease, but durable user state must not be lost.
The migration therefore uses:

- automatic, validated, one-way durable metadata conversion from schema v2;
- a private protocol bump from v24 to v25;
- CLI JSON schema v2 rather than changing the documented v1 meaning in place;
- MCP schema v2 and new tool/resource names rather than ambiguous aliases;
- policy schema v2 with fail-closed rejection and explicit migration guidance;
- no compatibility alias in which `dojo_id` changes semantic level silently.

The old `lair.json` and backup are retained until a new topology document has
been written successfully. The new canonical names are `topology.json` and
`topology.json.previous`. Invalid data remains bounded, quarantined, and never
causes saved commands to execute.

## Policy resource mapping

The new policy hierarchy is:

```json
{"kind":"daemon"}
{"kind":"lair","lair_id":"..."}
{"kind":"dojo","dojo_id":"..."}
{"kind":"splint","splint_id":"...","incarnation":"current"}
```

Legacy semantic mapping is exact but must be explicit:

```text
old lair selector   → new daemon selector
old dojo selector   → new lair selector, preserving UUID
old window selector → new dojo selector, preserving UUID
old splint selector → new splint selector
```

The daemon selector authorizes only daemon/catalog-level resources. Lair and
Dojo selectors retain publication-time descendant expansion. Legacy
`{"kind":"lair"}` must never be silently reinterpreted as one named Lair.

## In-Splint context

The daemon-owned environment becomes:

```text
SPLINTERM_LAIR_ID
SPLINTERM_DOJO_ID
SPLINTERM_SPLINT_ID
SPLINTERM_SPLINT_INCARNATION
```

`SPLINTERM_WINDOW_ID` is removed. Caller-provided values for every reserved key
remain overridden. Integrations must reconcile these hints against current
public topology; they are not credentials.

## Dependency-ordered milestones

### Milestone 0 — decision record and baseline

- Record this plan and a superseding terminology ADR.
- Capture the dirty-worktree baseline and preserve unrelated Plan 0017/UI work.
- Add migration fixtures before changing the model.
- Do not run graphical validation.

Validation:

```bash
git diff --check
```

### Milestone 1 — core model and durable conversion

- Introduce `Topology`, `LairId`, named `Lair`, and layout-owning `Dojo`.
- Remove the durable logical `Window` and `WindowId` types.
- Rename aggregate errors and mutation/query methods by semantic level.
- Add schema-v3 `TopologyDocument` validation and explicit schema-v2 decoding.
- Preserve UUIDs, revision, tree shape, focus, launch metadata, and exited-only persistence.
- Add atomic `topology.json` storage with safe legacy import.

Validation:

```bash
cargo test -p splinterm-core
cargo fmt --all --check
git diff --check
```

### Milestone 2 — daemon and private protocol

- Move authoritative daemon state from `Lair` to `Topology`.
- Change containment and launch context to `(LairId, DojoId, SplintId)`.
- Preserve transaction serialization and rollback behavior.
- Bump private protocol v24 to v25.
- Replace logical Window requests, responses, mutation preflights, audit
  operations, provenance, and topology snapshots.
- Update startup restore, process-exit reconciliation, and topology publication.

Validation:

```bash
cargo test -p splinterm-protocol
cargo test -p splinterd --lib
cargo test -p splinterd --test end_to_end -- --test-threads=1
cargo fmt --all --check
git diff --check
```

### Milestone 3 — policy and authorization

- Add daemon/Lair/Dojo/Splint resource selectors.
- Preserve exact descendant snapshot and future-descendant behavior.
- Add policy schema v2 and fail-closed v1 diagnostics.
- Update consent, authorization, audit, and revocation identities.
- Verify no selector broadens authority during conversion.

Validation:

```bash
cargo test -p splinterd policy -- --test-threads=1
cargo test -p splinterd authorization -- --test-threads=1
cargo test -p splinterd --test end_to_end -- --test-threads=1
```

### Milestone 4 — CLI, picker, and native presentation

- Change human and machine selectors to Lair/Dojo identities.
- Keep Window vocabulary only for native presentation.
- Rename logical topology command variants to New/Switch Dojo.
- Feed one Dojo layout into each mapped native window.
- Migrate `recent-windows.json` to `recent-dojos.json` while preserving UUID values.
- Keep the picker initially flat with `Lair / Dojo` labels.
- Update the reference session-picker integration and package validation.

Validation:

```bash
cargo test -p splinterm
python -m pytest -q tools/automation/test_session_picker.py
cargo fmt --all --check
git diff --check
```

### Milestone 5 — stable automation and MCP v2

- Add CLI JSON schema v2 with Lair/Dojo topology and provenance.
- Replace MCP tools and resources with Lair/Dojo terminology.
- Update closed schemas, schema inventory hashes, fixtures, and contract validators.
- Preserve untrusted-output handling, exact resource identity, confirmation,
  mutation preparation, and bounded aggregate restore behavior.
- Verify the relay remains byte-transparent and topology-agnostic.

Validation:

```bash
cargo test -p splinterm-automation-client
cargo test -p splinterm-mcp
cargo test -p splinterm-relay
uv run --with jsonschema python tools/automation/validate-contract-fixtures.py
```

### Milestone 6 — canonical documentation and closure

Update canonical current-state documentation:

- `GLOSSARY.md`
- `README.md`
- `docs/architecture.md`
- `docs/configuration.md`
- `docs/automation.md`
- `docs/mcp.md`
- `docs/integrations.md`
- `docs/headless.md`
- `docs/remote.md`
- `docs/roadmap.md`
- ADR 0006 and ADR 0007 through superseding notes

Historical plans, spikes, benchmark artifacts, and recorded evidence remain
historical unless they claim current behavior outside their dated context.

Final non-graphical validation:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p splinterd --test end_to_end -- --test-threads=1
uv run --with jsonschema python tools/automation/validate-contract-fixtures.py
python -m pytest -q tools/automation/test_session_picker.py
git diff --check
```

## Stop gates

Stop for a product or scope decision if implementation would require:

- merging multiple legacy layouts into one Dojo rather than the specified one-to-one conversion;
- changing global topology revision semantics;
- making Dojo names unique and therefore renaming migrated user layouts;
- silently reinterpreting a legacy policy resource;
- retaining an alias whose ID field changes semantic level;
- changing process restore, grant, controller, or terminal ownership behavior;
- broad edits to historical benchmark evidence; or
- graphical testing without the repository's separately approved workspace-8/DP-2 sequence.

## Acceptance

The migration is complete only when:

1. schema-v2 durable state converts losslessly to schema v3;
2. the old and new UUID values are proven identical in migration tests;
3. every supported public topology surface uses Lair/Dojo/Splint terminology;
4. Window remains only a native presentation concept in current code and docs;
5. policy migration fails closed and does not broaden authority;
6. focused and workspace-wide non-graphical validation passes;
7. independent review finds no blocker or fix worth doing now; and
8. recorded validation and review evidence exist, as required by `AGENTS.md`.

## Completion evidence

Non-graphical validation completed on 2026-08-04:

- `cargo check --workspace --all-targets`
- `cargo test --workspace -- --test-threads=1`
- `cargo test -p splinterd --test end_to_end -- --test-threads=1` — 16 passed
- strict Clippy for `splinterm-core`, `splinterm-protocol`, `splinterd`,
  `splinterm-automation-client`, `splinterm-mcp`, and `splinterm-relay`
- `uv run --with jsonschema python tools/automation/validate-contract-fixtures.py`
  — 35 valid/39 invalid automation fixtures and 86 valid/30 invalid MCP fixtures
- `python -m pytest -q tools/automation/test_session_picker.py tools/benchmark/test_graphical_multiplexer.py`
  — 19 passed plus 6 subtests
- `cargo fmt --all --check`
- `git diff --check`

The first independent read-only review found one blocker: explicit-cwd
`NewDojoAutomation` still consulted an existing Dojo and failed for an empty
Lair. The daemon resolver was corrected and covered by an empty-Lair regression.
A second fresh read-only review reran the focused migration checks and returned
**ACCEPT**, with no blockers or fixes worth doing now.

No graphical validation was authorized or run. Workspace-wide Clippy reaches
pre-existing dirty renderer/Wayland work and reports unrelated new-toolchain
style lints there; all migration-owning non-presentation crates pass strict
Clippy.
