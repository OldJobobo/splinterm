# Headless service and policy administration

`splinterd` owns Unix sockets, PTYs, persisted topology metadata, authorization,
and bounded audit metadata. It does not require or connect to Wayland or X11,
so the service works when `DISPLAY` and `WAYLAND_DISPLAY` are absent. When they
are present, the daemon preserves them for PTY children launched from graphical
sessions. The packaged unit removes the unsupported
`SPLINTERM_ENABLE_DEV_ATTACH` development bypass after loading its environment
file; graphical `splinterm window` clients remain separate processes.

## Service lifetime

The package installs but does not enable `splinterd.service`. For first-time
automation setup, [configure the policy environment](#install-an-owner-only-policy)
before starting the service. Otherwise, start it on demand:

```bash
systemctl --user start splinterd.service
splinterm ping
```

The desktop launcher also starts it on demand. To start it whenever this account's
systemd user manager reaches `default.target`:

```bash
systemctl --user enable --now splinterd.service
```

A normal non-lingering user manager usually stops at logout, which stops the
daemon and its shells. Persistent post-logout service requires an administrator's
conspicuous decision:

```bash
sudo loginctl enable-linger ACCOUNT
```

The package and its scripts never enable lingering, modify SSH policy, create a
service account, or enable the unit. For a dedicated service account, the
administrator must provision the account, its home/runtime ownership, login or
lingering policy, and the exact automation executable policy independently.

## Resource guards and workload containment

The packaged daemon separates its control plane from terminal workloads before
commands execute:

```text
splinterd.service                  daemon control plane
app-splinterm.slice                all terminal workloads
└── one transient slice per Dojo
    └── one transient scope per live Splint
```

Both `splinterd.service` and the aggregate workload slice have `TasksMax=2048`
and `MemoryHigh=75%`. The independent daemon boundary keeps a workload failure
from consuming the daemon's task or memory-pressure budget. Transient Dojo
slices use 1024 tasks and 50% memory pressure; Splint scopes use 512 tasks and
25% memory pressure. These are task ceilings and soft memory-pressure
thresholds. `MemoryHigh` asks the kernel to reclaim and throttle under sustained
pressure; Splinterm does not install a `MemoryMax` hard kill boundary in this
release.

A new Splint is rejected if its exact helper PID cannot be verified inside its
scope while still blocked before target execution. Existing Dojos are not
retargeted or silently launched inside the daemon service. Direct development
launches use containment when a user manager is available and otherwise emit a
bounded warning; only the packaged service passes the strict
`--require-workload-cgroups` mode.

Inspect both boundaries without changing them:

```bash
systemctl --user show splinterd.service \
  -p TasksCurrent -p TasksMax -p MemoryCurrent -p MemoryHigh -p MemoryMax
systemctl --user status app-splinterm.slice
systemd-cgls --user-unit app-splinterm.slice
systemd-cgtop
```

`MemoryCurrent` includes page cache charged to each cgroup. Inactive file cache
is normally reclaimable under pressure, but it is neither immediately free nor
excluded from cgroup accounting. Do not use `drop_caches` as routine Splinterm
recovery.

The aggregate workload slice may be inactive when no Splints are running. Empty
transient Splint scopes and Dojo slices are collected after runtime cleanup.
Packaged Dojo slices are also `PartOf=splinterd.service`: stopping or failing the
daemon service stops its workload hierarchy, while a Splint or Dojo stopping
does not propagate failure back to the daemon.

## Install an owner-only policy

The unit optionally reads `%h/.config/splinterm/daemon.env`. `EnvironmentFile`
contents are not shell code and later interactive-shell exports do not alter an
already running user manager. `SPLINTERM_POLICY` must be an absolute canonical
path.

**First-time setup only:** the following block creates owner-only files and
refuses existing files or symlinks. If either file already exists, review it and
preserve its settings; add or update only the intended policy rules and
`SPLINTERM_POLICY` entry. Never replace an existing policy with the empty example
just to enable MCP.

```bash
(
  set -eu
  umask 077
  policy="$HOME/.config/splinterm/policy.json"
  env_file="$HOME/.config/splinterm/daemon.env"
  for file in "$policy" "$env_file"; do
    if [ -e "$file" ] || [ -L "$file" ]; then
      printf 'Refusing to replace %s; review the existing configuration.\n' "$file" >&2
      exit 1
    fi
  done
  install -d -m 700 "$HOME/.config/splinterm"
  set -C # Refuse files created by another process after the checks above.
  printf '{"schema":"splinterm.policy.v2","rules":[]}\n' >"$policy"
  printf 'SPLINTERM_POLICY=%s\n' "$policy" >"$env_file"
)
```

An empty rule list is the explicit deny-all starting point. Add only reviewed
absolute executable paths, SHA-256 digests, closed scopes, exact resources, and
bounded limits described in [automation.md](automation.md). Never grant a shell,
interpreter, writable executable, or broad wrapper unless all code it can execute
is intended to receive that authority.

Authorizing the general `splinterm` CLI delegates the selected rule to every
same-account process able to invoke that exact binary; running inside a Splint
does not narrow or grant that authority. This can support a supervised CLI-based
coding agent, but the optional `splinterm-mcp` split package uses its own exact
executable identity for a narrower production boundary; see [mcp.md](mcp.md).
Lair and Dojo rules snapshot only resources present when the policy generation
is published. To
authorize a newly created child, review the concrete resource, update policy,
reload the service, and reconnect; broad future-descendant authority is not part
of policy v2.

Validate and inspect the file offline through the daemon's exact secure loader:

```bash
splinterm policy validate "$HOME/.config/splinterm/policy.json"
splinterm policy inspect "$HOME/.config/splinterm/policy.json"
```

Validation enforces the daemon's owner, mode, hard-link, no-symlink, size, JSON,
and semantic rules. Inspection prints normalized validated JSON; it does not
query or mutate the running daemon.

After validating the file, choose the service action according to what changed:

- **Service not running:** `systemctl --user start splinterd.service` loads
  `daemon.env` and the configured policy.
- **Added or changed `SPLINTERM_POLICY` in `daemon.env`:** a running daemon needs
  a restart to receive the new environment. Save your work first. From Foot or
  another terminal not owned by `splinterd`, run
  `systemctl --user restart splinterd.service`. **This ends every daemon-owned
  process.** Reloading cannot change the running daemon's environment.
- **Only the policy JSON changed, at the already configured path:** reload
  without restarting shells:

```bash
splinterm policy reload
```

Reload reports only that systemd delivered the request. Offline `policy inspect`
prints the file, not the published generation. Confirm the result through the
user journal or authorized bounded audit inspection:

```bash
journalctl --user-unit splinterd.service -n 30 --no-pager
splinterm --output json audit --max-records 16
```

Reload is atomic and fail-closed. A rejected file installs a new deny-all
policy generation. Every reload disconnects automation-role clients and revokes
their connection-owned subscriptions, controller leases, and pending transfers;
automation clients must reconnect explicitly. Local and SSH-remote human
graphical clients do not use persistent policy and remain connected.

## Runtime, state, restart, and recovery

The default socket is `$XDG_RUNTIME_DIR/splinterm/splinterd.sock` and is removed
on clean shutdown. Runtime directories are per-login and are not backups. Durable
metadata defaults to `$XDG_STATE_HOME/splinterm`, or
`$HOME/.local/state/splinterm` when `XDG_STATE_HOME` is unset. Policy files remain
under user configuration and are not written by the daemon.

A daemon restart terminates daemon-owned child processes. The packaged unit
allows 90 seconds for the daemon's bounded 30-second HUP and 30-second TERM
process-group grace periods, exit reconciliation, final metadata save, and socket
removal before systemd may force cleanup.
Persisted topology and launch metadata may remain, but saved commands are never
automatically executed; restoration requires an explicit authorized `restore`,
`restore-dojo`, or `restore-lair` command. Explicitly Saved and Pinned Lairs are
protected from automatic retirement; only fully exited Disposable Lairs are
eligible for the daemon's bounded capacity-retirement policy. Saving or pinning
is metadata-only and never starts, stops, resizes, or detaches a process. Saved
split ratios are proportional layout authority, while stored rows and columns
are bounded launch hints rather than a promise of identical pixel geometry.
Audit retention is daemon-lifetime-only and resets after restart.

To terminate every daemon-owned shell, move the complete session database to a
timestamped backup, restart the service, and wait for its socket in one guarded
command:

```bash
splinterm reset
```

The interactive command asks for confirmation. Use `splinterm reset --yes` only
for an already-approved unattended reset. It reports the reversible backup path;
policy and user configuration remain untouched.

For a consistent backup, stop the service, copy the state and policy files while
preserving owner/mode, then start it again:

```bash
systemctl --user stop splinterd.service
# Copy ~/.local/state/splinterm and ~/.config/splinterm using trusted local tools.
systemctl --user start splinterd.service
```

Before package upgrades, stop the daemon if its shells may be discarded. Private
protocol compatibility does not promise process migration across versions. After
upgrade run `systemctl --user daemon-reload` and start the service. If startup
fails, inspect the user journal, verify `XDG_RUNTIME_DIR`/`HOME`, validate the
policy, and ensure no unsafe stale object occupies the socket path. Do not remove
unknown files or weaken ownership checks to force startup.

## Accepted 0.2 upgrade contract (not implemented)

The preceding restart procedure remains authoritative until the guarded
in-place re-exec contract in [ADR 0011](adr/0011-guarded-in-place-daemon-reexec.md)
is implemented and released. The accepted `0.2.0` target distinguishes four paths:

- **matching** — the installed and running build identities match;
- **compatible** — explicit handoff and checkpoint ranges overlap, so the next
  human launcher invocation performs a guarded in-place handoff automatically;
- **idle incompatible** — no live Splint can be lost, so a bounded restart is
  allowed; and
- **active incompatible** — launch blocks until the user explicitly confirms a
  destructive restart showing the exact affected Splint count.

The first upgrade from `0.1.x` to a handoff-capable `0.2.0` daemon is a separate
one-time bootstrap path because the running old daemon cannot gain handoff
support retroactively. It always reports the exact live Splint count and requires
confirmation, including when that count is zero. Package scriptlets only install
files and report the boundary; they do not restart or hand off a user's service.

During a compatible handoff, trusted graphical chrome reports **input paused**.
The exact sealed replacement-client snapshot restores the Window's bounded
ordered-tab, active-tab, and focused-pane record, recreates its per-Dojo connections, and
fully resnapshots before the previously active pane accepts typing again without
a click or tab switch. A transferred, stale, invalid, or conflicting controller-
resume claim remains view-only and uses the ordinary Request Control flow. Remote and automation clients must
reconnect and reauthenticate without inheriting old-generation authority.

This planned-upgrade continuity does not cover daemon crash, logout without
lingering, reboot, or host loss. Under ADR 0012, body-bearing handoff checkpoints
use only anonymous sealed memory-backed descriptors, and `0.2.0` durable recovery
remains recipe-only and stores no terminal grids, scrollback bodies, image
bodies, parser state, replies, or input. See
[ADR 0011](adr/0011-guarded-in-place-daemon-reexec.md) and
[ADR 0012](adr/0012-defer-durable-terminal-archives.md).
