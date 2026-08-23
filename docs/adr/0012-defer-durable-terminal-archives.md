# ADR 0012: defer durable terminal-body archives beyond 0.2.0

- **Status:** Accepted
- **Date:** 2026-08-14

## Context

Splinterm's durable saved-workspace contract stores validated topology, launch
recipes, working directories, proportional layout, and bounded geometry hints.
It deliberately excludes terminal grids, scrollback bodies, image bodies,
clipboard data, input, process memory, and shell state.

Plan 0037 considered adding an owner-only, default-off terminal archive so a
saved Lair could display historical terminal content after reboot. Such an
archive would persist sensitive output that may contain credentials, private
messages, paths, source code, logs, and command results. It would require
separate decisions for retention, deletion, export, trusted-human and machine
read authority, image handling, storage bounds, corruption, migration, and
possibly encryption key management.

The primary `0.2.0` objective is compatible planned-upgrade continuity for live
processes and PTYs. Combining that ownership and rollback work with a new
durable sensitive-data surface would enlarge both the implementation and review
boundary.

## Decision

`0.2.0` does not persist terminal grids, scrollback bodies, image bodies, parser
state, terminal reply queues, or input as part of saved Lairs or reboot
restoration. Recipe-only restoration remains the default and only supported
reboot/daemon-loss behavior. Restored leaves are exited/restorable, and launching
a new process remains explicit and allocates a new incarnation.

The canonical terminal checkpoint used for a planned in-place daemon handoff is
not a durable archive. Any body-bearing checkpoint exists only in an anonymous,
sealed, bounded memory-backed descriptor; it has no filesystem pathname and no
named owner-only fallback. It is excluded from the durable session database,
backups, public and private observation APIs, audit records, diagnostics, crash
uploads, argv, and environment. Normal adoption or rollback closes it, while
candidate crash, kill, or service cleanup closes the last process-owned
descriptors without leaving a named checkpoint artifact.

Durable terminal archives are recorded as a possible `0.3.0` product milestone,
not a commitment. Any future proposal requires a separate plan and privacy ADR
that decide, before implementation:

- explicit opt-in scope and whether a global owner default is allowed;
- maximum bytes per Splint, Lair, account, and capture;
- retention, expiry, deletion, backup, and export behavior;
- trusted-human read authority and whether all machine access remains forbidden;
- image-body omission or storage policy;
- truncation, generation labeling, corruption, and schema migration behavior;
- archive behavior before and after explicit relaunch;
- encryption claims and key ownership, if encryption is proposed; and
- amendments to `FR-PERSIST-05`, ADR 0006, and the public saved-Lair contract.

No future implementation may infer commands, authority, process identity, or
relaunch intent from archived terminal content.

## Consequences

- `0.2.0` can focus its security, lifecycle, and validation budget on live
  compatible handoff.
- Reboot and daemon-loss documentation remains simple and honest: layouts and
  recipes may survive, but processes, PTYs, terminal presentation, and
  scrollback do not.
- Existing saved-workspace files retain their current privacy boundary.
- Abrupt handoff failure may lose live continuity, but it cannot leave a named
  body-bearing checkpoint behind.
- Users do not receive historical terminal presentation after reboot in
  `0.2.0`.
- A later archive design remains possible without making its schema or privacy
  policy an accidental `0.2.x` compatibility obligation.
