# Plan 0044 Beta 2 graphical acceptance

- **Date:** 2026-08-19
- **Result:** PASS
- **Source tested:** `e6dba77cc99946c048d3142056f55ab6a68b9f11`
- **Target:** staged `0.1.0beta2-1` package on private socket, state, and configuration
- **Isolation:** workspace 8 / DP-2; exact PID/address targeting; no system installation

The approved guarded matrix proved:

1. commandless XDG, bare `splinterm launch`, Recent Dojos picker New, and
   in-Window New Terminal create Window-owned Lairs when
   `persistent-by-default=no`;
2. one Window can own two independently leased transient Lairs and closing it
   retires both;
3. creating a Dojo or explicitly renaming a Dojo promotes the complete Lair
   when `persist-on-tab-organization=yes`, and the promoted Lair survives
   Window close;
4. creating and renaming Dojos leaves the Lair transient when promotion is
   disabled, and Window close retires the complete organized Lair;
5. an explicit Lair name and a native command-bearing launch remain persistent;
6. command-bearing XDG remains client-bound even when persistence defaults on;
7. CaskaydiaMono resolves regular, bold, italic, and bold-italic from its own
   family; and
8. regular-only Audiowide maps successfully, emits three bounded style warnings,
   and reuses its regular face.

The first in-Window New Terminal attempt exposed an existing generated-name
collision: initial launch and immediate creation both used Unix seconds plus the
same PID. Commit `e6dba77` changed both generated-name sites to Unix nanoseconds.
The exact rebuilt package then created two distinct transient Lairs and retired
both on close.

Early harness failures were bounded and did not count as product failures:
empty-list guidance was initially parsed as a Lair, and the picker was initially
assumed to require a new compositor address. Instrumented diagnosis proved the
owner lease retired correctly; the unnecessary diagnostic code was reverted.
Accepted results count only UUID-prefixed Lair rows and allow the picker surface
to reuse its address.

Every input action followed a fresh exact-address focus check. Only isolated test
Windows were closed. After the matrix, workspace 8 was empty, all private daemon
and client processes were gone, and the recorded baseline focus, workspace,
monitor, and geometry were restored. User configuration, installed packages,
and the running system daemon were unchanged.

See `summary.json` for exact case outputs, package and executable identities,
baseline/final compositor state, and cleanup results. `SHA256SUMS` seals the
summary and original screenshots.
