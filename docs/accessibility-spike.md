# Semantic accessibility spike

Splinterm's custom renderer has no native widget tree. The accessibility seam is
therefore an independent AccessKit tree published on Linux through AT-SPI over
D-Bus by `accesskit_unix`.

This spike establishes the contract needed by the optional navigation explorer;
it does not enable or render the explorer.

## Contract

The semantic root is a Window with separate Terminal, SearchInput, Tree, and
polite Status nodes. Explorer entries are TreeItem nodes with stable
Window-local IDs and explicit levels:

- level 1: Lair;
- level 2: Dojo;
- level 3: Splint.

Expansion, selection, current target, availability, and pending state remain
independent. Pending items are busy and disabled and carry bounded status text.
Names, status, and search text reject control and bidi-formatting characters and
have fixed character limits. The semantic tree never contains terminal cells,
scrollback, parser state, image bodies, clipboard data, process data, or inferred
activity.

`SemanticNodeRegistry` allocates IDs from typed domain identities and never
reuses an ID during a Window lifetime. AT actions contain only semantic IDs and
typed operations; the calloop owner must resolve those IDs against its current
projection and reject stale or removed targets.

## Thread and event-loop boundary

AccessKit Unix handlers run on another thread. They may only enqueue a bounded
`SemanticAction` and invoke the supplied wake callback. They must never mutate
Wayland objects, renderer state, or application focus directly.

The intended runtime flow is:

```text
AT-SPI action
  -> AccessKit worker callback
  -> SemanticActionQueue + calloop Ping
  -> calloop-owned App drains typed actions
  -> current projection revalidation and state reduction
  -> one coalesced TreeUpdate
```

Focus and search requests replace older requests of the same class. Duplicate
activation/expansion requests are suppressed, and a full queue rejects further
non-coalescible actions. Semantic snapshots are staged by state changes rather
than raster damage; equivalent snapshots and intermediate states superseded in
the same loop turn do not publish updates. Every flush carries an explicit
`SemanticOwnerTurn`; a second flush with the same token remains pending until a
later turn. A dedicated polite, atomic Status
node carries result, empty, error, stale, and disconnected announcements without
churning item names.

## Wayland limitation

Wayland does not reveal a top-level Window's compositor-relative position.
Accordingly Splinterm must not call `Adapter::set_root_window_bounds` or claim
accurate global screen coordinates on Wayland. Local node geometry may be added
when explorer layout exists, but this limitation does not prevent AT-SPI role,
hierarchy, state, focus, or action exposure.

## Manual AT-SPI inspection

The non-shipping example keeps the synthetic contract available for inspection:

```bash
cargo run -p splinterm --example accessibility-spike
```

With the desktop accessibility bus enabled, inspect the process using an AT-SPI
inspector or screen reader. The bounded gate checks are:

1. Window, SearchInput, Tree, Status, and three TreeItem levels are discoverable.
2. `editor` is independently selected, current, and focused.
3. expand/collapse, focus, activation, and search-value actions arrive as typed
   queue events.
4. changing the status once produces one polite announcement; an equivalent
   update produces none.
5. no global bounds are claimed.

This manual inspection is graphical-environment validation and is not implied by
normal non-graphical test execution.
