---
title: MCP adapter
description: Install the optional Splinterm MCP adapter, connect a supported host, authorize the minimum required surface, and revoke it safely.
---

The current `splinterm-mcp` adapter is a local, bounded MCP stdio server supporting `2025-06-18` and `2025-11-25` (preferred). It presents a fixed catalog of 33 tools plus topology and terminal/control resources over the same daemon-owned topology used by the native client.

The adapter is an optional, separately identified third-party client—not a trusted part of `splinterd`. Installing or launching it grants no daemon authority. Unattended access requires owner-controlled policy for its exact executable identity, operations, resources, and limits. Interactive access can instead use `request_lair_access`: the trusted consent UI must receive explicit user approval before granting temporary access to the named Lair.

:::note
The optional MCP adapter is included in the stable 0.1.0 release. The host examples below document the currently validated local environment, not broad host compatibility or a stable API promise.
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

For unattended access, first configure `SPLINTERM_POLICY` in `~/.config/splinterm/daemon.env` using the repository's owner-only setup instructions:

https://github.com/OldJobobo/splinterm/blob/main/docs/headless.md#install-an-owner-only-policy

Configure this before starting the service if possible. Creating `policy.json` alone is not enough, and reloading does not update a running daemon's environment. Changing that environment requires a restart, which ends daemon-owned processes; save work and use a separate terminal first.

Edit `~/.config/splinterm/policy.json` with mode `0600`, preserving existing rules unless you intend to replace them. This observation-only example authorizes bounded reads and subscriptions for one current Splint incarnation:

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

## 4. Validate and activate

Validate and inspect the policy file offline:

```bash
chmod 600 "$HOME/.config/splinterm/policy.json"
splinterm policy validate "$HOME/.config/splinterm/policy.json"
splinterm policy inspect "$HOME/.config/splinterm/policy.json"
```

If the running daemon already has the correct `SPLINTERM_POLICY` environment, reload the edited JSON:

```bash
systemctl --user reload splinterd.service
journalctl --user-unit splinterd.service -n 30 --no-pager
```

For first startup or a changed environment, follow the service steps in the headless guide above instead. Check the journal for acceptance: `policy inspect` reads the file, not the daemon's published generation.

Policy-authorized operations fail closed if there is no policy, the digest differs, a scope is missing, the resource or incarnation is wrong, the revision is stale, destructive confirmation is missing, a limit is exceeded, or another client owns the controller. MCP transport access is not authority.

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
3. Run `splinterm policy validate "$HOME/.config/splinterm/policy.json"`, confirm the daemon started with the intended `SPLINTERM_POLICY`, and check its journal for policy acceptance.
4. Reconcile `unauthorized`, `stale_topology`, `stale_incarnation`, controller denial, timeout, and `resync_required`; do not retry blindly or broaden policy.
5. Keep stdout reserved for MCP frames. Bounded diagnostics are written to stderr.

Read [Bounded automation](/docs/automation/) for the shared policy, controller, audit, and untrusted-data model.
