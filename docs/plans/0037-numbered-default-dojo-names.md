# Plan 0037: Numbered default Dojo names

- **Status:** Implementation complete and reviewed for the current `0.1.0-alpha` patch line; release integration pending
- **Date:** 2026-08-14
- **Product authority:** Implicitly named Dojos use short, predictable per-Lair labels; explicit and persisted names remain user-owned
- **Depends on:** accepted Lair/Dojo topology, topology revisions, graphical Dojo tabs, and `new-dojo` CLI creation

## Decision

Replace generated default Dojo labels such as `terminal` and
`terminal-<timestamp>` with `Dojo 1`, `Dojo 2`, and so on.

Numbering is local to one Lair. An implicitly named Dojo uses one greater than
the highest existing name that exactly matches `Dojo N`, where `N` is a
positive decimal integer. Gaps are not reused: if a Lair contains `Dojo 1` and
`Dojo 3`, the next implicit name is `Dojo 4`.

Explicit names remain exact after existing normalization and validation.
Existing persisted Dojos are not migrated or renamed. Collision-resistant Lair
names such as `terminal-<timestamp>-<pid>` also remain unchanged because they
identify Lairs rather than Dojo tabs.

## Behavior contract

- A newly created persistent or transient Lair starts with `Dojo 1`.
- Creating an unnamed Dojo in that Lair chooses `max(existing numbered Dojos) + 1`.
- Exact names such as `Dojo 7` participate in numbering, including names set by
  an explicit create or rename operation.
- Custom names, different capitalization, malformed suffixes, zero, signs,
  whitespace variants, and numeric overflow do not participate.
- If the highest recognized number cannot be incremented safely, implicit
  creation fails with a bounded error rather than wrapping or generating a
  duplicate fallback.
- An explicit CLI `--name` bypasses default-name generation.
- MCP, automation, preset, and protocol requests that already require an
  explicit name retain that contract.
- Name derivation and the create request use the same listed topology revision;
  concurrent topology mutation is rejected through the existing stale-revision
  boundary.

## Implementation milestones

### Milestone 1 — shared numbering contract and initial Dojo

- Add one shared pure helper for deriving the next numbered Dojo name from a
  Lair's current Dojos.
- Change the initial Dojo created by `Lair::new` and `Lair::transient` from
  `terminal` to `Dojo 1`.
- Test empty, contiguous, gapped, custom, malformed, explicitly high, and
  overflow cases.
- Preserve legacy persistence fixtures and prove old `terminal` names still load
  without migration.

### Milestone 2 — graphical and CLI implicit creation

- Replace timestamp-derived graphical tab names with the shared helper.
- Derive the name from the target Lair in a `ListLairs` response and use that
  response's topology revision for creation.
- Make `new-dojo --name` optional. Resolve omission through the same helper in
  both human and machine execution paths while preserving explicit names.
- Keep generated Lair names collision-resistant and rename misleading internal
  helper terminology where it otherwise suggests those values are Dojo names.
- Test omitted and explicit CLI behavior plus local and remote-interactive
  request construction.

### Milestone 3 — documentation and acceptance

- Document the numbered omitted-name behavior in the source and website CLI
  references.
- Run focused affected-crate tests, the non-graphical workspace boundary,
  formatting, strict affected-crate Clippy, and `git diff --check`.
- Inspect the actual diff and obtain one fresh read-only review before claiming
  completion.

## Validation

```bash
cargo test -p splinterm-core
cargo test -p splinterm
cargo test --workspace
cargo fmt --all --check
cargo clippy -p splinterm-core -p splinterm --all-targets -- -D warnings
git diff --check
```

No graphical test is required for acceptance: topology responses and focused
unit/integration tests can prove the generated names. Packaging, installation,
version bumps, publication, and release tagging remain separate release-boundary
work.

## Implementation evidence (2026-08-14)

- New persistent and transient Lairs now start with `Dojo 1`; legacy persisted
  names remain unchanged when loaded.
- One core helper recognizes only canonical positive `Dojo N` names, advances
  past the maximum without reusing gaps, ignores malformed and over-`u64`
  suffixes, and fails closed at `u64::MAX`.
- Graphical creation, human `new-dojo`, remote-interactive creation, and machine
  JSON creation derive implicit names from the exact listed topology revision.
  Explicit names bypass derivation. Generated Lair names remain unchanged.
- Focused request tests cover implicit and explicit names for local,
  remote-interactive, and machine requests, including exact revision retention.
- `cargo test -p splinterm-core` passed. Post-review `cargo test -p splinterm
  --lib --bin splinterm` passed with 370 library tests passed, one manual timing
  harness ignored, and 99 binary tests passed.
- Strict affected-crate Clippy, formatting, `git diff --check`, and the complete
  website build/link validation passed.
- The workspace run passed affected work but encountered unrelated timing
  failures in two MCP stdio tests; both passed exact isolated retries. Full
  `splinterm` integration runs also exposed the existing fake-SSH `ETXTBSY`
  fixture race; the exact failed case passed in isolation. No remote or MCP code
  was changed for this patch.
- Fresh read-only review found no correctness blocker and requested stronger
  request-construction coverage. That coverage was added; bounded follow-up
  review confirmed the finding resolved and the patch ready.
