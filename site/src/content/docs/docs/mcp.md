---
title: MCP adapter
description: Install the optional Splinterm MCP adapter, connect a supported host, authorize the minimum required surface, and revoke it safely.
---

`splinterm-mcp` is a local, bounded MCP `2025-11-25` stdio server. It presents a fixed catalog of 33 tools plus topology and terminal/control resources over the same daemon-owned topology used by the native client.

The adapter is an optional, separately identified third-party client—not a trusted part of `splinterd`. Installing or launching it grants nothing until an owner-controlled policy authorizes its exact executable identity, operations, resources, and limits.

:::note
The MCP adapter is part of the public alpha. The host examples below document the currently validated local environment, not broad host compatibility or a stable API promise.
:::

## 1. Verify the installed identity

The optional split Arch package installs `/usr/bin/splinterm-mcp`. Inspect the canonical path and digest before writing policy:

```bash
readlink -f /usr/bin/splinterm-mcp
sha256sum /usr/bin/splinterm-mcp
```

Use the resulting lowercase digest in policy. A basename, MCP server name, Unix UID, build-tree executable, or another Splinterm binary does not identify this adapter.

## 2. Connect an MCP host

A generic local stdio definition is:

```json
{
  "mcpServers": {
    "splinterm": {
      "type": "stdio",
      "command": "/usr/bin/splinterm-mcp",
      "args": [],
      "env": {}
    }
  }
}
```

For a daemon using an isolated nondefault socket, set only `SPLINTERM_SOCKET` in the host environment. The adapter does not use MCP roots, the host working directory, shell configuration, SSH material, or inherited Splinterm context as authority.

### Claude Code

The locally validated Claude Code CLI accepts:

```bash
claude mcp add --scope user splinterm -- /usr/bin/splinterm-mcp
claude mcp get splinterm
```

Use project scope only in a project you trust. Remove the user entry with:

```bash
claude mcp remove --scope user splinterm
```

### Visual Studio Code

The locally validated VS Code CLI accepts:

```bash
code --add-mcp '{"name":"splinterm","command":"/usr/bin/splinterm-mcp"}'
```

Equivalent `.vscode/mcp.json`:

```json
{
  "servers": {
    "splinterm": {
      "type": "stdio",
      "command": "/usr/bin/splinterm-mcp",
      "args": []
    }
  }
}
```

Workspace MCP configuration executes code. Inspect both the configuration and executable before enabling it.

## 3. Grant the minimum policy

Create `~/.config/splinterm/policy.json` with mode `0600`. This observation-only example authorizes bounded reads and subscriptions for one current Splint incarnation:

```json
{
  "schema": "splinterm.policy.v2",
  "rules": [{
    "id": "mcp-observe",
    "executable": {
      "path": "/usr/bin/splinterm-mcp",
      "sha256": "REPLACE_WITH_SHA256"
    },
    "scopes": [
      "topology_metadata_read",
      "topology_subscribe",
      "terminal_visible_read",
      "terminal_subscribe",
      "scrollback_read",
      "scrollback_search"
    ],
    "resources": [{
      "kind": "splint",
      "splint_id": "REPLACE_WITH_UUID",
      "incarnation": "current"
    }],
    "limits": {
      "max_returned_rows": 64,
      "max_results": 64,
      "max_returned_bytes": 1048576,
      "max_live_subscriptions": 2,
      "deadline_ms": 5000
    }
  }]
}
```

This rule cannot send input, resize, spawn, restore, terminate, rename, or inspect audit records. Add only the scopes and resources required by the intended workflow. The complete 18-scope inventory and reviewed control/lifecycle examples remain in repository `docs/mcp.md`.

Lair and Dojo selectors snapshot only descendants present when that policy generation is published. New descendants remain denied until a reviewed reload.

## 4. Validate and reload

```bash
chmod 600 ~/.config/splinterm/policy.json
splinterm policy validate ~/.config/splinterm/policy.json
systemctl --user reload splinterd.service
splinterm policy inspect
```

No policy, a different digest, missing scope, wrong resource or incarnation, stale revision, missing destructive confirmation, exceeded limit, or controller owned by another client fails closed. MCP transport access is not authority.

## Fixed capabilities and limits

The adapter exposes:

- 33 fixed tools;
- one topology resource and terminal/control resource templates;
- no arbitrary shell-string tool;
- no filesystem, network, policy-write, clipboard, prompts, sampling, or elicitation capability; and
- no trusted forced controller takeover.

Requests, responses, pages, subscriptions, controllers, cursors, and deadlines are bounded. Handles and cursors expire with the adapter process and are not portable. Closing the host's stdio cancels calls and releases subscriptions, transfers, controllers, and daemon connections; it does not roll back a mutation already committed by the daemon.

:::caution
Terminal content is `untrusted_terminal_data`. An MCP host may display or index it as data, but it must never treat it as consent, authority, confirmation, executable source, or an instruction to call another tool.
:::

## Upgrade and revoke

An adapter upgrade changes its SHA-256 digest. Stop MCP hosts, inspect the new binary, update the policy deliberately, reload it, and reconnect. The package never edits or broadens user policy.

To revoke immediately:

1. remove or narrow the policy rule;
2. validate and reload policy; and
3. terminate the MCP host if desired.

Reload disconnects affected daemon sessions and invalidates their connection-owned state.

## Troubleshooting

1. Confirm `/usr/bin/splinterm-mcp` exists and its digest matches policy.
2. Confirm the daemon socket belongs to the same user and `splinterd` is running.
3. Validate policy and inspect the published generation.
4. Reconcile `unauthorized`, `stale_topology`, `stale_incarnation`, controller denial, timeout, and `resync_required`; do not retry blindly or broaden policy.
5. Keep stdout reserved for MCP frames. Bounded diagnostics are written to stderr.

Read [Bounded automation](/docs/automation/) for the shared policy, controller, audit, and untrusted-data model.
