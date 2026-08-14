---
title: Development
description: Build, test, and understand the Splinterm workspace without mixing contributor internals into user guidance.
---

Splinterm is a Rust workspace with separate crates for the graphical client, daemon, domain model, protocol, relay, MCP adapter, PTY boundary, and Foot-derived terminal kernel.

## Branch-first development

Keep the root `main` checkout for coordination and releases. Make each coherent
plan, feature, fix, or documentation milestone on a short-lived branch in a
dedicated sibling worktree:

```bash
git fetch origin
git worktree add ../splinterm-worktrees/my-change \
  -b feat/my-change origin/main
```

One writer owns each worktree. Dependent or overlapping changes remain serial;
read-only review may inspect the task worktree. Validate and review the actual
diff before opening a pull request, prefer squash merge, then remove the merged
worktree and branch. Release candidates and publication originate only from
reviewed `main` commits. See the repository's `CONTRIBUTING.md` and `AGENTS.md`
for the complete workflow and agent-specific stop-loss rules.

## Standard validation

Run from the repository root:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Changes to domain or protocol behavior should include focused tests and preserve crate boundaries.

## Isolated development instance

The `splinterm-test` helper builds and runs against an isolated owner-only socket:

```bash
./splinterm-test          # build, start/reuse the test daemon, open a client
./splinterm-test restart  # rebuild and restart after daemon/protocol changes
./splinterm-test ping     # build and verify the isolated daemon
./splinterm-test stop     # stop the isolated daemon
```

The helper intentionally labels and isolates its development authorization bypass. Do not use that bypass with the packaged daemon or normal user state.

## Workspace map

| Crate | Responsibility |
| --- | --- |
| `splinterm` | CLI, native Wayland frontend, input, and rendering |
| `splinterd` | authoritative topology, PTYs, persistence, and policy |
| `splinterm-core` | transport- and UI-independent domain model |
| `splinterm-protocol` | versioned private client-daemon transport |
| `splinterm-relay` | policy-identified SSH stdio transport |
| `splinterm-mcp` | optional policy-identified MCP adapter |
| `splinterm-pty` | Linux PTY and child-process boundary |
| `splinterm-terminal` | Foot-derived grid and streaming terminal kernel |

## Documentation boundaries

- User workflows belong in this site's main documentation.
- Machine contracts remain authoritative in `docs/automation.md` and checked-in schemas.
- Architectural decisions remain in `docs/adr/`.
- Plans and spikes record decisions and validation history; they are not user instructions.
- Retained benchmark and graphical evidence should not enter ordinary documentation search.

## Foot authority

Foot's pinned implementation remains the terminal behavior oracle. Do not modify the canonical Foot checkout or silently regenerate comparison references. Ported code must retain compatible licensing, exact provenance, and required notices in `THIRD_PARTY.md`.
