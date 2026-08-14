# Code Size and Architecture Review

Date: 2026-08-14

## Scope

This review evaluates whether Splinterm contains excessive or unnecessarily
duplicated code, whether its structure supports local changes, and whether the
documented system design still matches the implementation.

The review covered workspace and crate manifests, the architecture document,
source and repository size, large files and functions, exact cross-file clone
signals, crate dependencies, implementation boundaries, recent change
concentration, and the standard non-graphical validation boundary. Two fresh
read-only reviewers independently checked the architectural and code-size
findings.

The existing `graphify-out/` graph was consulted as supporting evidence only. It
covers 21 files rather than the complete current repository, so its centrality
results were not treated as authoritative.

## Executive verdict

The system design remains fundamentally sound. Major crate, transport,
authority, and graphical boundaries are intentional, documented, and reflected
in the dependency graph.

The repository does not show broad copy-and-paste growth or an obvious surplus
of production code. Its main maintainability risk is concentrated orchestration:
a small number of very large dispatch, update, and rendering functions have
become mandatory edit and review points for otherwise separate behavior.

Summary:

- Architecture: sound, with one narrow client-to-daemon dependency to correct.
- Test and security discipline: strong.
- Cross-file duplication: low.
- Local changeability: good at crate boundaries, mixed inside orchestration
  modules.
- Primary risk: oversized request, event-reduction, drawing, and command
  dispatch pipelines.
- Runtime performance: not established by this review; performance optimization
  requires profiling and benchmark evidence rather than line-count reduction.

## Size profile

Approximate measurements taken from tracked files:

| Area | Size |
| --- | ---: |
| Active Rust, excluding one archived source copy | 141,803 lines |
| Approximate production Rust before trailing `cfg(test)` modules | 90,000–92,000 lines |
| Inline and external Rust tests | More than 45,000 lines |
| Tooling, primarily Python | 37,857 lines |
| All tracked code-like files, including tools and archived copies | About 196,000 lines |
| Exact cross-file clone candidates of at least 12 lines | About 595 duplicated lines |
| Tracked documentation and retained evidence | 2,596 files / 147.6 MiB |
| Git pack | 61.6 MiB |

Splinterm includes a terminal engine, persistent daemon, Wayland client,
protocol, remote transport, automation client, MCP server, image protocols,
packaging, benchmark harnesses, and Foot-oracle compatibility. The total size is
large but proportionate to the supported system.

An exact clone scan found 38 cross-file runs of at least 12 lines, representing
about 595 duplicated lines on one side. This is a heuristic rather than a proof
of semantic duplication, but it is sufficient to reject broad copy-and-paste as
the primary source of size.

## Findings

### High priority: decompose daemon request execution without weakening its gate

`crates/splinterd/src/main.rs:4721` begins
`handle_authorized_request`, an approximately 1,905-line function covering
access grants, topology and process mutations, history and search, controller
lifecycle, and terminal actions.

The single visible authorization boundary is valuable and should remain.
However, operation implementations for unrelated protocol families now collide
in one function. This increases merge conflicts and makes every operation
change carry a large security-review surface.

Recommended approach:

1. Retain one exhaustive top-level `Request` match.
2. Retain centralized authorization and resource tables.
3. Move operation bodies incrementally into private family handlers for access,
   topology lifecycle, history/search, control, and terminal actions.
4. Pass explicit borrowed context instead of introducing a generic service
   abstraction that hides authority or cleanup behavior.

This is primarily a changeability improvement; it may not materially reduce
line count.

### High priority: turn Wayland update and drawing into explicit phases

`crates/splinterm/src/wayland.rs:7163` begins the approximately 721-line
`apply_updates` pipeline. `crates/splinterm/src/wayland.rs:8083` begins the
approximately 879-line `draw` pipeline.

Together they coordinate topology and terminal update reduction, history and
search state, geometry, resize and focus reconciliation, frame preparation,
shared-memory backing, terminal and overlay composition, damage, capture, and
surface commit. Rendering, protocol-update, and overlay work therefore converge
on the same high-churn file and transactions.

The module already has focused submodules and grouped state structures. The
recommended change is not an arbitrary split of `wayland.rs` or a flattening of
`App` state. Instead:

1. Extract phase-oriented private helpers for focused-pane update reduction,
   prepared-frame reconciliation, SHM/backing synchronization, overlay
   composition, and commit finalization.
2. Keep `apply_updates` and `draw` as short, visibly ordered coordinators.
3. Preserve the documented atomic acquisition, damage, and commit sequence.
4. Retain Wayland object ownership in the Wayland layer.

### Medium priority: remove the UI crate's daemon-library dependency

`crates/splinterm/Cargo.toml` normally depends on `splinterd`, while the only
source use is `splinterd::inspect_policy_file` in
`crates/splinterm/src/app/local_service.rs:81` and `:86`. The API is exported by
`crates/splinterd/src/lib.rs`.

This couples the disposable graphical client to daemon implementation and its
transitive runtime dependencies for one shared security function. It weakens an
otherwise clean ownership boundary and expands rebuild and API-change scope.

Recommended approach:

1. Move the secure policy loader, typed representation, validation, and
   normalized inspection API into a narrow shared policy crate.
2. Use that crate from both `splinterd` and `splinterm`.
3. Initially preserve `splinterd::inspect_policy_file` as a delegating
   compatibility wrapper.
4. Preserve all ownership, permissions, bounded-shape, and semantic checks.

### Medium priority: split topology-manager command families

`crates/splinterm/src/app/topology_manager.rs:1223` begins the approximately
559-line `handle_session_manager_command` function. It handles picker
operations, creation, navigation, restoration, renaming, retention,
termination, and tab lifecycle within one serialized switch.

Keep the single serialized manager loop, but delegate picker,
creation/materialization, Lair lifecycle, Dojo lifecycle, and tab lifecycle to
private handlers returning a common typed outcome.

### Medium priority: modularize automation-client internals

Before tests begin in `crates/splinterm-automation-client/src/lib.rs`, one module
contains independently evolving DTO projection, event and subscription
conversion, image transport/cache ownership, and connection framing and
cancellation.

Move these implementations into private `projection`, `events`, `image`, and
`connection` modules and re-export the existing public API unchanged. This
should reduce navigation and conflict cost without causing downstream churn.

### Low priority: centralize audit operation naming

The complete `AuditOperation` string mapping is duplicated in:

- `crates/splinterm-automation-client/src/lib.rs:1383`
- `crates/splinterm-mcp/src/dispatch.rs:1882`

The authoritative enum already lives in
`crates/splinterm-protocol/src/lib.rs:1372` with `snake_case` serialization.
Add an exhaustive canonical `AuditOperation::as_str()` in the protocol crate and
reuse it in both consumers. Keep exhaustive matching so new operations remain
compile-time checked.

### Low priority: conservatively extract MCP mutation response plumbing

`crates/splinterm-mcp/src/dispatch.rs:1325` begins an approximately 544-line
mutation dispatcher. Its explicit tool-to-request mapping is security relevant
and should stay visibly exhaustive.

Extract only per-resource-family response handling and repeated
preflight/revision plumbing. Do not replace the closed tool-name match with a
runtime registry or generic dispatch mechanism.

## Architecture assessment

The documented major boundaries still match implementation:

- `splinterm-core` remains independent of async runtimes, PTYs, wire protocol,
  and Wayland.
- `splinterm-protocol` depends on core rather than UI or runtime
  implementation.
- `splinterd` has no Wayland or Smithay dependency.
- `splinterm-graphical-relay` does not parse or depend on the private daemon
  protocol.
- Application services call the narrow `run_window` facade rather than owning
  Smithay objects.
- Internal APIs use private and scoped visibility deliberately.
- Protocol and authority decisions remain explicit and fail closed.

Recent change concentration supports the hotspot diagnosis. Of the last 100
commits, 39 changed Rust; those commits touched a median of three and a mean of
6.3 Rust files, with 15 touching at least five. `wayland.rs` appeared in 17 of
the 100 commits and `splinterd/src/main.rs` in 11. Some spread is expected for
protocol changes crossing daemon, client, and tests, but the two orchestration
roots remain recurring convergence points.

No blocker-level architectural failure was found.

## Code that should not be reduced merely to lower LOC

Do not compress or generalize the following without a separately justified
behavioral design:

- daemon authorization and resource mappings;
- protocol `Request` and `Response` definitions;
- MCP's closed tool mapping;
- the built-in keymap table;
- terminal and Foot-oracle regression coverage;
- image-protocol compatibility coverage;
- explicit resource limits and bounded validation; and
- the daemon's single visible authorization gate.

This verbosity supports security review, compatibility, and regression
protection. Removing it would improve a metric while making the system less
auditable.

The large preset-validation result produced by the first heuristic scan was a
false positive caused by naive brace tracking. The actual
`validate_catalog_with_commands` function is already bounded and delegates tree
validation and traversal; it does not justify a rewrite.

## Repository bulk

The working-tree size is dominated by retained benchmark, spike, oracle, and
graphical evidence under `docs/`, not production source. This affects clone,
search, and navigation ergonomics but is not demonstrated code-maintenance
bloat. Tooling consumes portions of the retained evidence and repository policy
requires preservation of historical artifacts.

Do not delete or silently regenerate this material. If repository ergonomics
become a material problem, evaluate Git LFS or versioned external object storage
with checked manifests, immutable provenance, offline behavior, and tooling
migration defined first.

## Recommended sequence

1. Extract the shared secure policy loader from `splinterd`.
2. Decompose daemon request-family implementations while preserving one
   exhaustive authorization gate.
3. Refactor Wayland update and draw into explicit ordered phases.
4. Split automation-client internals behind unchanged public exports.
5. Split topology-manager command families.
6. Centralize audit-operation string mapping.
7. Conservatively extract repeated MCP mutation response plumbing.
8. Adopt a review rule that new functions over roughly 200–300 lines require a
   documented cohesion, security, or generated-table rationale.

Safe deletion opportunities appear modest. The main return will come from
smaller change surfaces and clearer ownership, not from removing tens of
thousands of lines.

## Validation and review evidence

The following passed on the reviewed tree:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -- --test-threads=1
git diff --check
```

Two fresh read-only reviews completed successfully. Both found no blocker-level
architectural failure and independently identified the oversized daemon
request handler and client-to-daemon policy dependency. One review additionally
confirmed the Wayland update/draw pipelines as the largest UI changeability
risk.
