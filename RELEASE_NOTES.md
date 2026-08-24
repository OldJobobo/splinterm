# Splinterm 0.1.0 Beta 2 — Persistence With an Exit

Splinterm remains persistent by default. Beta 2 adds a deliberate alternative:
an ordinary unnamed graphical terminal can belong to its Window and disappear
when that Window closes. The moment you organize that terminal into a named or
multi-tab workspace, Splinterm can make the complete Lair durable automatically.

Beta 2 also includes the startup font-family correction for newly opened
Windows. Selecting CaskaydiaMono, another Omarchy terminal family, or an
explicit Fontconfig family no longer leaves bold and italic resolution tied to
JetBrains Mono.

This release also separates terminal workloads from the daemon's resource
failure boundary. Packaged Splints now run inside nested systemd user units:
the aggregate Splinterm workload slice contains one slice per Dojo and one scope
per Splint. The daemon keeps its own independent task and memory-pressure guard.

## Configurable terminal lifetime

The new settings are:

```ini
[multiplexer]
persistent-by-default=yes
persist-on-tab-organization=yes
```

Both default to `yes`, preserving Beta 1 behavior for existing configurations.

Set `persistent-by-default=no` when normal terminals should behave like
Window-owned terminals rather than detached multiplexer sessions. It applies to:

- the commandless desktop/XDG launch used by Omarchy's terminal icon and
  `SUPER+ENTER`;
- bare `splinterm launch`;
- **New** in the Recent Dojos picker; and
- in-Window **New Terminal**.

Closing the owning Window terminates the processes and removes the complete
unpromoted Lair. It is not saved, restored, placed in Recent Dojos, or selected
by `reopen`.

Explicit durable intent still wins. Named Lairs, `splinterm new NAME`,
`splinterm launch --name NAME`, native command-bearing launches, presets,
restore/relaunch, remote creation, automation, and MCP creation remain
persistent. Generated collision-resistant Lair names and the initial `Dojo 1`
label are implementation identities, not explicit naming.

## Organize first, keep it afterward

With `persist-on-tab-organization=yes`, either of these actions atomically
promotes a Window-owned Lair to persistent:

- creating another Dojo tab; or
- explicitly naming or renaming a Dojo tab.

Promotion includes every Dojo, Splint, and running process in the Lair. It is
permanent for that Lair. Once promoted, closing the Window detaches normally and
the Lair can appear in Recent Dojos.

Set the option to `no` when even organized or multi-tab terminals should remain
Window-owned. In that mode, all tabs in the transient Lair are removed together
when its Window closes.

Promotion is an owner-only topology transaction. Lifetime, tab creation or
rename, persistence, lease removal, revision advancement, and publication
commit together. A stale revision, wrong owner, invalid name, runtime admission
failure, or persistence failure leaves the original transient Lair unchanged.

Command-bearing `splinterm-xdg-terminal-exec -- COMMAND...` retains its existing
client-bound contract regardless of these settings.

## Startup font-family correction

New clients now resolve regular, bold, italic, and bold-italic from the family
selected by the configured regular Fontconfig pattern. The application default
is now:

```ini
[main]
font=monospace:style=Regular
```

That follows the active Omarchy Fontconfig terminal-family selection. An
explicit `main.font` remains authoritative.

A styled face is accepted only when it belongs to the selected regular family,
represents the requested weight and slant, and has compatible terminal-cell
metrics. When no compatible style exists, Splinterm warns and deliberately
reuses the regular face instead of refusing to open a Window.

This corrects startup for families such as CaskaydiaMono and for regular-only
families. Already-open Windows still retain immutable renderer resources; live
font-family replacement remains planned for the 0.2 line.

## Workload isolation and daemon protection

The packaged daemon starts only after it proves that the user systemd manager
can create, verify, and remove the transient units required for workload
placement. Each terminal command remains blocked in the PTY helper until its
process has been moved into and verified inside the exact Splint scope.
Placement failure prevents the target command from executing.

The packaged boundaries are:

- `splinterd.service`: `TasksMax=2048`, `MemoryHigh=75%`;
- aggregate `app-splinterm.slice`: `TasksMax=2048`, `MemoryHigh=75%`;
- each Dojo slice: `TasksMax=1024`, `MemoryHigh=50%`; and
- each Splint scope: `TasksMax=512`, `MemoryHigh=25%`.

`MemoryHigh` applies reclaim pressure rather than terminating the unit at a hard
byte ceiling. Beta 2 deliberately sets no `MemoryMax`; a hard-memory limit
requires measured follow-up evidence. These boundaries limit a runaway terminal
workload's impact on the daemon and neighboring Dojos, but they do not claim to
fix unrelated client-side panics.

Upgrading from Beta 1 still restarts the 0.1 daemon and ends its active Dojos.
Perform the upgrade from Foot or another terminal not owned by `splinterd`, then
reopen Splinterm windows after the package replacement.

## Compatibility and boundaries

- Existing configuration omission remains persistent.
- Persistent topology, restore, history, remote, automation, MCP, and preset
  contracts are unchanged.
- Transient authority is restricted to the trusted local graphical owner.
- Terminal output cannot select lifetime, trigger promotion, or retain a Lair.
- Beta 1 tags and packages remain immutable.
- Beta 2 still targets x86_64 Omarchy/Arch Linux with native Wayland.

## Validation boundary

The Beta 2 implementation passed serialized workspace, package,
release-tooling, portable Foot-provenance, documentation, independent review,
real-systemd workload-placement, local installation, and guarded staged-package
graphical boundaries before publication.
