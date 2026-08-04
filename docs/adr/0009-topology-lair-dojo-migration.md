# ADR 0009: Topology, Lair, and Dojo hierarchy

- **Status:** Accepted
- **Date:** 2026-08-04
- **Plan:** [Lair/Dojo topology migration](../plans/0018-lair-dojo-topology-migration.md)
- **Amends:** [ADR 0006](0006-multiplexing-lifecycle.md), [ADR 0007](0007-supported-automation-policy.md)

## Context

The original durable hierarchy used one daemon-owned `Lair` catalog containing
named Dojos, logical Windows, and Splints. That overloaded “Window”: the same
word described both persistent daemon topology and native Wayland presentation.
It also prevented multiple named Lairs from being represented explicitly.

## Decision

The persistent hierarchy is:

```text
Topology → Lair → Dojo → Splint
```

`Topology` is the daemon-owned catalog and retains one global monotonic
`TopologyRevision`. A Lair is a named persistent session or project. A Dojo is
a named persistent layout with one binary Splint tree and a default-focus hint.
“Window” is reserved for a compositor-managed native Wayland toplevel. Mapping,
closing, focusing, moving, or resizing a Window does not itself mutate a Dojo.

The schema-v2 identity conversion is lossless:

- the former named Dojo UUID becomes the Lair UUID;
- the former logical Window UUID becomes the Dojo UUID;
- Splint UUIDs, launch metadata, tree shape, focus, lifecycle metadata, and the
  global topology revision remain unchanged.

Durable metadata is schema v3 in `topology.json`. Startup may decode schema-v2
`lair.json`, commit canonical schema-v3 metadata atomically, and must not delete
the legacy source before that write succeeds.

The private protocol is version 25. Public CLI and MCP contracts are version 2
and expose only Lair/Dojo/Splint identities. Child context is
`SPLINTERM_LAIR_ID`, `SPLINTERM_DOJO_ID`, `SPLINTERM_SPLINT_ID`, and
`SPLINTERM_SPLINT_INCARNATION`; `SPLINTERM_WINDOW_ID` is not exported.

Persistent policy v2 uses `daemon`, `lair`, `dojo`, and `splint` selectors. The
legacy policy-v1 `lair` selector meant the singleton global catalog, so v1 is
rejected rather than silently reinterpreted. Lair and Dojo selectors continue
to snapshot only descendants present when the generation is published.

## Consequences

- Persistent resource names match the product hierarchy without colliding with
  Wayland terminology.
- Existing schema-v2 durable state migrates without changing stable identity or
  restore behavior.
- Mixed protocol versions fail negotiation rather than changing ID meaning.
- CLI, MCP, policy, audit, fixture, integration, and child-environment users
  must adopt their v2/v25 contracts together.
- Native presentation code may continue to use `Window`, `WindowCommand`, and
  related Wayland names because those types no longer describe durable state.
