---
title: Remote access
description: Open native remote Splinterm windows or relay bounded automation through authenticated SSH.
---

Splinterm supports two distinct SSH workflows without exposing a daemon network listener:

- a **native graphical client** for a person using a remote account; and
- a **policy-scoped stdio relay** for machine automation.

Transport is not interchangeable authority. OpenSSH authenticates the human graphical workflow. Machine operations remain governed by exact executable policy.

## Configure a remote profile

Profiles live at `${XDG_CONFIG_HOME:-~/.config}/splinterm/remotes.toml`:

```toml
version = 1

[remotes.wintermute]
host = "wintermute"
user = "operator"
port = 22
identity_files = ["~/.ssh/id_ed25519"]
known_hosts_file = "~/.ssh/known_hosts"
connect_timeout_seconds = 15
```

The schema is strict. It does not accept arbitrary SSH options, commands, forwarding, environment, or shell fragments. Ordinary safe aliases, identities, certificates, agents, and proxy routing may still come from OpenSSH configuration; Splinterm supplies fixed safety overrides.

Inspect and probe without opening a Window:

```bash
splinterm remote list
splinterm remote inspect wintermute
splinterm remote check wintermute
```

`remote inspect` prints resolved non-secret settings and structured SSH argv. `remote check` performs bounded SSH, relay, daemon, `Ping`, and `ListLairs` probes without mutating topology.

## Open a native remote Window

```bash
splinterm --remote wintermute
splinterm --remote wintermute dojos
splinterm --remote wintermute reopen
splinterm --remote wintermute launch --working-directory /srv/project
splinterm --remote wintermute window --lair-id LAIR_ID --dojo-id DOJO_ID
```

Omitting the command opens the remote Recent Dojos picker. A Window stays bound to one endpoint and can discover, attach, split, control, search, and restore remote Dojos through one OpenSSH child. New remote panes briefly show a client-local `Opening remote pane…` placeholder while bounded protocol round trips complete.

OpenSSH may use the controlling terminal for passwords, key passphrases, PINs, or hardware tokens. Unknown or changed host keys fail closed. Splinterm does not run `ssh-keyscan`, accept unknown keys, or store credentials.

## Authority and boundaries

A native remote Window receives ordinary human multiplexer authority after SSH authenticates the remote account. It can request normal controller ownership but cannot use trusted-local forced takeover. It also does not publish local compositor focus or receive remote terminal image bodies.

SSH or relay loss closes the affected local views and releases connection-owned controllers. It sends no kill or close request, so daemon-owned remote Splints continue running.

:::note[Remote images]
Remote graphical sessions intentionally omit terminal image transfer. Unexpected remote image metadata fails the affected pane closed rather than opening a content channel.
:::

## Automation over SSH

The machine relay is a different entry point:

```bash
ssh -T \
  -o StrictHostKeyChecking=yes \
  ACCOUNT@HOST \
  /usr/bin/splinterm relay --stdio
```

It copies bounded private-protocol bytes over stdio. The remote daemon authorizes the exact installed `splinterm-relay` executable under an owner-controlled policy. SSH login, socket access, and Unix account identity do not grant machine operations.

Use a dedicated account or restricted key when relay callers must not inherit the account's other SSH capabilities. Relay stdout is protocol data only; diagnostics stay on stderr.

For the complete relay identity, profile schema, authentication, reconnect, and policy contract, read repository [`docs/remote.md`](https://github.com/OldJobobo/splinterm/blob/main/docs/remote.md).
