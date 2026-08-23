# Headless service and policy administration

`splinterd` owns Unix sockets, PTYs, persisted topology metadata, authorization,
and bounded audit metadata. It does not require or connect to Wayland or X11,
so the service works when `DISPLAY` and `WAYLAND_DISPLAY` are absent. When they
are present, the daemon preserves them for PTY children launched from graphical
sessions. The packaged unit removes the unsupported
`SPLINTERM_ENABLE_DEV_ATTACH` development bypass after loading its environment
file; graphical `splinterm window` clients remain separate processes.

## Service lifetime

The package installs but does not enable `splinterd.service`. Start it on demand:

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

## Aggregate resource guard

The packaged unit limits the complete daemon cgroup to 2,048 tasks and sets its
memory-high boundary to 75%. The task ceiling makes a recursive process spawn
fail before it approaches the user manager's much larger inherited ceiling.
`MemoryHigh` asks the kernel to reclaim and throttle under sustained pressure;
it is not a hard memory limit.

These settings cover `splinterd` and every process launched through its PTYs.
They reduce the blast radius of a runaway command, but they do not isolate one
Dojo from another. Per-Dojo or per-Splint cgroups require a separate process
ownership design.

Inspect the effective settings and current use without changing the service:

```bash
systemctl --user show splinterd.service \
  -p TasksCurrent -p TasksMax -p MemoryCurrent -p MemoryHigh -p MemoryMax
```

`MemoryCurrent` includes page cache charged to the service cgroup. Inactive file
cache is normally reclaimable under pressure, but it is neither immediately
free nor excluded from cgroup accounting. Do not use `drop_caches` as routine
Splinterm recovery.

## Install an owner-only policy

The unit optionally reads `%h/.config/splinterm/daemon.env`. `EnvironmentFile`
contents are not shell code and later interactive-shell exports do not alter an
already running user manager. `SPLINTERM_POLICY` must be an absolute canonical
path.

Create policy files without a permissive intermediate file:

```bash
install -d -m 700 "$HOME/.config/splinterm"
umask 077
policy_tmp=$(mktemp "$HOME/.config/splinterm/policy.json.XXXXXX")
cat >"$policy_tmp" <<'JSON'
{
  "schema": "splinterm.policy.v2",
  "rules": []
}
JSON
chmod 600 "$policy_tmp"
mv -f "$policy_tmp" "$HOME/.config/splinterm/policy.json"

printf 'SPLINTERM_POLICY=%s\n' "$HOME/.config/splinterm/policy.json" \
  >"$HOME/.config/splinterm/daemon.env"
chmod 600 "$HOME/.config/splinterm/daemon.env"
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

After editing, validate first and request a reload through the canonical unit:

```bash
splinterm policy reload
```

This command reports only that systemd delivered the reload request. Confirm the
result through the user journal or authorized bounded audit inspection:

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
