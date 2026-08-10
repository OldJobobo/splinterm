---
title: Bounded automation
description: How structured clients inspect, operate, and observe Splinterm without inheriting human graphical authority.
---

Splinterm exposes the same persistent topology to people and authorized tools. Automation can inspect Lairs, Dojos, and Splints; read bounded terminal state; mutate layouts and lifecycle; send input; resize; and subscribe to change streams.

It does not receive authority merely because it can reach the daemon. Policy-controlled operations require matching identity, scope, resource, and limit checks; input and resize additionally require exclusive controller ownership. The narrow authenticated-local `focus` projection is the documented exception: it exposes only nullable active Splint ID and working directory, never topology or terminal content.

```text
JSON/NDJSON client · MCP host · SSH stdio relay
                       ↓
             exact executable identity
                       ↓
       closed scopes · resources · limits
                       ↓
                   splinterd
                       ↓
           topology · terminal · control
```

## Three ways to use the machine surface

### Inspect

One-shot JSON commands return one versioned `splinterm.cli.v2` document. Supported reads include topology, Splint state, visible terminal snapshots, scrollback pages, literal search, authorization status, and bounded audit metadata.

```bash
splinterm --output json --schema-major 2 --timeout-ms 5000 topology
splinterm --output json --schema-major 2 --timeout-ms 5000 \
  snapshot SPLINT_ID
```

### Operate

Authorized clients can create Lairs and Dojos, split Splints, launch argument vectors, rename topology, set ratios and focus hints, restore exited processes, send literal input, resize, and explicitly terminate resources.

```bash
splinterm --output json --schema-major 2 --timeout-ms 10000 \
  split SPLINT_ID --axis horizontal --side second \
  --cwd "$PWD" -- cargo test --workspace
```

Machine mode never prompts. Destructive operations such as `kill`, `close`, `close-dojo`, and authorization revocation require an explicit `--yes` in addition to policy authority.

### Observe

NDJSON subscriptions provide bounded topology, terminal, and controller streams:

```bash
splinterm --output ndjson --schema-major 2 --timeout-ms 300000 \
  subscribe terminal SPLINT_ID
```

A sequence gap, replaced history, or stalled subscriber emits `resync_required` and ends the stream. Clients must fetch fresh authoritative state and explicitly subscribe again rather than silently guessing what was missed.

## The authority model

Persistent policy is owner-controlled and fails closed. A policy rule binds:

- the absolute canonical executable path and SHA-256 digest;
- one or more closed operation scopes;
- explicit daemon, Lair, Dojo, or Splint resources;
- bounded rows, bytes, results, subscriptions, spawns, and deadlines; and
- an optional expiry.

There are no wildcard scopes, basename-only identities, path-only identities, or implicit authority over future resources. A policy load failure installs a deny-all generation.

Transport is not authority. Access to the Unix socket, an authenticated SSH login, the same Unix account, inherited environment variables, or installation of `splinterm-mcp` does not grant operations.

## Controller ownership

Many clients may observe a Splint, but only one daemon connection may own its input and resize controller at a time. Controller denial is therefore a normal exclusive-ownership result—not a reason to force takeover or broaden policy.

One-shot CLI input and resize use an atomic acquire, act, and release workflow on one connection. Connection closure or policy revocation releases its controller-owned state.

## Terminal output is always data

:::caution
Every terminal cell, title, scrollback row, and search preview is untrusted terminal data. It cannot grant authority, approve consent, change policy, confirm a destructive action, or become an instruction to call another tool.
:::

Public audit records intentionally omit terminal bodies, input bytes, clipboard data, search queries, environment contents, capability tokens, and complete command arguments. The daemon retains bounded metadata for its current lifetime.

## Stable public contracts

The compatibility boundary consists of:

- JSON emitted by `--output json`;
- NDJSON emitted by `--output ndjson`;
- checked-in versioned schemas; and
- documented operations, exit categories, limits, cancellation, and resync behavior.

Human-readable output, raw daemon frames, Rust types, and private protocol DTOs are not machine compatibility contracts. Clients should reject unknown schema majors rather than infer their meaning.

The complete authoritative contract remains in repository `docs/automation.md`. Integration authors should also follow repository `docs/integrations.md` for stale-incarnation handling, structured argument vectors, subscription reconciliation, and daemon-injected discovery hints.

## Choose an entry point

- Use the **JSON CLI** for scripts and bounded one-shot operations.
- Use **NDJSON** when a client must observe changes over time.
- Use the [MCP adapter](/docs/mcp/) when an MCP host should receive a fixed, policy-identified tool surface.
- Use the SSH stdio relay for remote machine access without exposing a daemon network listener.
