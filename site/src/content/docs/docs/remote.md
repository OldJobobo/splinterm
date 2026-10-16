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

OpenSSH may use the controlling terminal for passwords, key passphrases, PINs, or hardware tokens. Without a terminal it uses an ssh-agent key first, then your own `SSH_ASKPASS`, and only then Splinterm's own prompt in the connection status Window. Unknown or changed host keys fail closed. Splinterm does not run `ssh-keyscan`, accept unknown keys, or store credentials.

## Open a remote without a terminal

`splinterm remotes`, the **Remote Dojos** desktop action, **Open Remote…** in the command palette, and the **Remote Dojos…** row of the local Recent Dojos picker open one chooser of the profiles in `remotes.toml`. Choosing a profile starts a separate `splinterm --remote PROFILE` Window process with no inherited stdio and no controlling terminal, so OpenSSH cannot prompt on a terminal. An agent key or your own `SSH_ASKPASS` is preferred; when neither can answer, Splinterm asks in the status Window.

Every graphical remote launch first shows a status Window ("Connecting to …") while OpenSSH authenticates. On success it closes and the remote's picker opens; on failure it shows the categorized cause, a one-line remedy, the newest sanitized OpenSSH lines, and the agent or `SSH_ASKPASS` setup, with R or Enter to retry and C, Q, or Escape to close. OpenSSH's error output stays in that Window. Started from a terminal, OpenSSH may still prompt on that terminal, because Splinterm forces `SSH_ASKPASS` only when stdin is not a terminal. If the Window closes before connecting, `splinterm` exits with status 1. The client diagnostics log under `~/.local/state/splinterm/logs/` records only the failure category.

When OpenSSH needs a password, passphrase, or PIN and nothing else can answer, the status Window shows OpenSSH's prompt text and a masked password field: Enter submits, and Escape cancels the attempt. The field never echoes; the screen and accessibility text say only whether it is empty. Yes/no questions take Y or N. Splinterm's own executable answers OpenSSH as `SSH_ASKPASS` through a private per-attempt socket and token, accepts prompts only from that attempt's OpenSSH, wipes answers after use, and never logs them. Pasting into the field is allowed for password managers, and the status Window does not use an input method. Escape, closing the Window, or two unanswered minutes end the attempt by stopping OpenSSH first, so it never sends an empty password. Non-graphical commands such as `remote check` never show this prompt.

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
