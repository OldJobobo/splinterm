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

Build the unmodified reference outside the source checkout:

```bash
tools/foot-oracle/build-reference.sh
```

Build the patched semantic oracle in a disposable worktree:

```bash
tools/foot-oracle/build-oracle.sh
```

Then compare every fixture with Foot:

```bash
tools/foot-oracle/run-fixtures.py
```

The default build directories are `/tmp/splinterm-foot-build` and
`/tmp/splinterm-foot-oracle-build`. The scripts verify the exact reference
commit. The minimal builds disable documentation, themes, terminfo, utmp, and
grapheme clustering while retaining Foot's normal tests and terminal code.

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

Foot does not currently ship a machine-readable semantic state dumper. The
maintained test-only patch in `patches/0001-semantic-state-dump.patch` adds one
to the pinned reference build without changing the canonical Foot checkout.

## Adapter design

The oracle uses a maintained patch applied only to a disposable Foot worktree:

1. create a detached worktree at the pinned commit;
2. apply `patches/0001-semantic-state-dump.patch`;
3. build the patched Foot executable;
4. launch it with an exact fixture payload and oracle-only logical grid size;
5. dump normalized JSON matching `fixtures/terminal/v1/schema.md` after parser
   input is consumed;
6. compare that JSON with the fixture and, later, the Rust terminal snapshot.

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

The first five fixtures are `oracle_verified` against the pinned Foot build:
printable text, soft wrapping, cursor positioning, erase-line, and basic SGR.
The current adapter requires a running Wayland compositor because it exercises
the patched Foot executable. A future fully headless C test harness may remove
that requirement.

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
