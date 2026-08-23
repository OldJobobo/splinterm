# Plan 0046: Separate Lair and Dojo pickers

- **Status:** Implemented and independently reviewed
- **Date:** 2026-08-22
- **Scope:** Native in-Window selector hierarchy and copy
- **Graphical validation:** Separately approval-gated

## Problem

The command palette exposes both **Choose Lair** and **Choose Dojo**, but they do
not currently represent different levels of the topology:

- **Choose Dojo** requests a current-Lair-filtered Dojo catalog; while
- **Choose Lair** requests the combined `LairDojo` catalog, which contains every
  reopenable Dojo across all Lairs.

Both routes reuse picker chrome whose visible heading and guidance are hardcoded
to Recent Dojos. Choosing a Lair therefore presents Dojos rather than Lairs and
makes users reason about two hierarchy levels at once.

## Product contract

Provide three distinct trusted workflows:

1. **Choose Lair** shows one row per reopenable persistent Lair. Selecting a row
   activates that Lair through its most recently attached Dojo, falling back to
   its first reopenable Dojo.
2. **Choose Dojo** shows only reopenable Dojos in the active Lair.
3. **Recent Dojos** remains a global cross-Lair recency workflow.

The picker copy and New action must identify the active level:

| Workflow | Heading | Guidance | New action |
| --- | --- | --- | --- |
| Choose Lair | Lairs | Switch to a Lair. | New Lair |
| Choose Dojo | Dojos | Switch to a Dojo in this Lair. | New Dojo |
| Recent Dojos | Recent Dojos | Open a recent running Dojo. | New Terminal |

A Lair row uses the Lair's user-visible name and summarizes its selected Dojo's
working directory and aggregate live pane state using the existing calm picker
layout. Terminal output does not influence labels, target choice, ordering, or
authority.

## Selection semantics

The Lair catalog includes only persistent Lairs with at least one reopenable
Dojo. It preserves daemon catalog order. For each Lair, target selection uses the
same rule as Previous/Next Lair navigation:

1. the most recently attached Dojo from that Lair in the current Window; then
2. the first reopenable Dojo in stable catalog order.

The captured `(LairId, DojoId)` remains identity checked by the existing open
path. An asynchronous topology change must fail or reconcile rather than
retarget a different Lair or Dojo.

## Implementation boundary

- Replace the combined selector kind with a pure Lair selector.
- Build one Lair item and representative Dojo target per eligible Lair.
- Carry selector presentation copy into native picker rendering instead of
  hardcoding Recent Dojos.
- Preserve standalone `splinterm dojos`, remote endpoint behavior, modal input
  isolation, pointer/keyboard selection, and existing topology mutation paths.
- Do not add the proposed Lair explorer sidebar, change tab ownership, or move
  Dojos between Lairs.

## Non-graphical validation

- exact palette and keymap dispatch tests;
- catalog tests for one row per Lair, current-Window recency preference,
  fallback, empty/ineligible Lairs, and current-Lair Dojo filtering;
- renderer/frontend tests for workflow-specific heading, guidance, count label,
  and New action;
- focused Rust formatting, tests, Clippy where practical, and `git diff --check`;
- fresh independent read-only review.

Graphical smoke or matrix work requires separate approval under the repository's
guarded graphical-testing rules.

## Validation record

- `cargo fmt --all -- --check` passed.
- `cargo clippy -p splinterm --all-targets -- -D warnings` passed.
- Focused selector catalog, command dispatch, picker copy, label sanitization, and
  New-action routing tests passed.
- `cargo test -p splinterm` passed the library, binary, and preceding integration
  suites before one remote-session fixture encountered a transient `ETXTBSY`
  while executing its temporary fake SSH file. The exact failed test passed on a
  bounded rerun.
- `git diff --check` passed.
- Fresh independent read-only review found that minimal presentation shortened
  `RECENT DOJOS` to `DOJOS`. The exception was removed, its regression assertion
  was corrected, and the focused renderer test and all static checks passed.
- Graphical validation was not performed because it remains separately
  approval-gated.
