# Synthetic accessibility inspection harness

Splinterm's custom renderer has no native widget tree. The accessibility seam is
therefore an independent AccessKit tree whose bounded navigation projection is
published on Linux through AT-SPI over D-Bus by Splinterm's native adapter.
AccessKit Unix is not used because its AT-SPI translation does not preserve the
expanded/current states or distinct navigation actions required by this contract.

The production Window now publishes its real Explorer through this seam; see
`accessibility.md` for supported behavior and limits. This document describes only
the synthetic example. Running that example does not exercise production action
revalidation, restore confirmation, or the Window lifecycle, and does not establish
live architecture or packaged accessibility acceptance.

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
reuses a retired ID during a Window lifetime. Production callbacks capture an
owner-issued authority epoch along with the semantic ID and typed operation.
The calloop owner resolves them against its current projection and rejects stale
or removed targets. The synthetic harness merely prints and reduces fixture
actions; it has no production action authority.

## Thread and event-loop boundary

zbus invokes native AT-SPI handlers away from the calloop owner. They may only
enqueue a bounded `SemanticAction` and invoke the supplied wake callback. They
must never mutate Wayland objects, renderer state, or application focus directly.

The production runtime flow is:

```text
AT-SPI action
  -> native zbus callback
  -> SemanticActionQueue + calloop Ping
  -> calloop-owned App drains typed actions
  -> current projection revalidation and state reduction
  -> one coalesced latest-state publication
  -> cancellable native transport worker
```

Focus and search requests replace older requests of the same class. Duplicate
activation/expansion requests are suppressed, and a full queue rejects further
non-coalescible actions. Semantic snapshots are staged by state changes rather
than raster damage; equivalent snapshots and intermediate states superseded in
the same loop turn do not publish updates. Every flush carries an explicit
`SemanticOwnerTurn`; a second flush with the same token remains pending until a
later turn. A dedicated Status node and one polite AT-SPI announcement carry
result, empty, error, stale, and disconnected changes without churning item
names.

## Wayland limitation

Wayland does not reveal a top-level Window's compositor-relative position.
Accordingly Splinterm does not expose the AT-SPI Component interface or claim
accurate global screen coordinates. Explorer layout exists, but local geometry
and Component support remain unsupported. This limitation does not prevent AT-SPI role, hierarchy, state,
focus, or action exposure.

## Manual AT-SPI inspection

The non-shipping example keeps the synthetic contract available for inspection:

```bash
cargo run -p splinterm --example accessibility-spike
```

With the desktop accessibility bus enabled, inspect the process using an AT-SPI
inspector or screen reader. The bounded gate checks are:

1. Window, SearchInput, Tree, Status, and three TreeItem levels are discoverable.
2. `editor` is independently selected, current, focused, and collapsed;
   `shell` is busy and disabled; `offline` is disabled without being busy.
3. expand/collapse, focus, activation, and search-value actions arrive as typed
   queue events with their exact semantic node IDs.
4. changing the status once produces one polite announcement; an equivalent
   update produces none.
5. no Component interface or global bounds are exposed.

This manual inspection is graphical-environment validation and is not implied by
normal non-graphical test execution.
