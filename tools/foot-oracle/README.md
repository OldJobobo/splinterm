# Foot semantic oracle

This directory defines how Splinterm will compare its Rust terminal port against
the pinned Foot reference implementation.

## Reference

- Project: Foot
- Version: 1.27.0
- Commit: `3c5b584b0eafa772eb4376fb6eaf6643399e190e`
- Local source: `${FOOT_SOURCE:-$HOME/Playground/foot}`
- License: MIT

See `provenance.json` and the repository's `THIRD_PARTY.md`.

## Build

Build the reference outside the source checkout:

```bash
tools/foot-oracle/build-reference.sh
```

The default build directory is `/tmp/splinterm-foot-build`. Override it with
`FOOT_BUILD=/path`. The script verifies the exact reference commit unless
`ALLOW_FOOT_REVISION_MISMATCH=1` is set deliberately.

The minimal reference build disables documentation, themes, terminfo, utmp, and
grapheme clustering. It still builds Foot's normal test target and terminal
libraries.

## Why an oracle adapter is needed

Text output is not enough to establish terminal compatibility. Two terminal
engines can display the same text while disagreeing about:

- cell attributes and color source;
- cursor position and last-column flag;
- soft wraps versus hard line breaks;
- terminal modes and scroll regions;
- alternate-screen state;
- replies written back to the PTY;
- row metadata used during resize and reflow.

Foot does not currently ship a machine-readable semantic state dumper. Before
Splinterm claims parity, a test-only adapter must expose normalized state from
the pinned Foot implementation.

## Adapter design

The preferred approach is a maintained patch applied only to a disposable Foot
worktree or build copy:

1. create a detached worktree at the pinned commit;
2. apply patches from `tools/foot-oracle/patches/`;
3. build a `foot-state-dump` test executable;
4. feed it a fixture's dimensions, configuration, and input bytes;
5. emit canonical JSON matching `fixtures/terminal/v1/schema.md`;
6. compare that JSON with the Rust terminal snapshot.

The adapter may add test-only constructors or accessors, but it must not change
terminal behavior. Its patch must never be applied to the canonical
`~/Playground/foot` checkout.

If a full terminal constructor remains too coupled to Wayland, implement the
adapter incrementally:

1. direct C harnesses for grid algorithms;
2. a parser/handler harness with a minimal test terminal;
3. a headless compositor-backed Foot process for remaining integration state;
4. screenshot comparison only for behavior that is inherently graphical.

## Canonical output

The oracle output will use the fixture schema and normalize implementation-only
values. It must include at least:

- dimensions;
- cursor and last-column flag;
- complete visible rows;
- hard/soft linebreak metadata;
- non-default cell attributes;
- relevant modes;
- title changes and terminal replies.

It must not include pointers, allocation sizes, renderer buffers, timestamps,
or other nondeterministic data.

## Initial fixture status

The first five fixtures are source-reviewed expectations derived from the pinned
Foot implementation. They validate corpus structure today, but are marked
`source_reviewed`, not `oracle_verified`. Their status may be promoted only
after `foot-state-dump` produces matching semantic output.

Validate fixture structure with:

```bash
python tools/foot-oracle/validate-fixtures.py
```

## Rules

- Never silently update expected output to match Rust.
- Record intentional Splinterm divergences explicitly in the fixture.
- Keep raw input byte-exact using lowercase hexadecimal.
- Compare arbitrary input chunkings against the same final state.
- Treat Foot as the authority until an accepted Splinterm ADR documents a
  divergence.
