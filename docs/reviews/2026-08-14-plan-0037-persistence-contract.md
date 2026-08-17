# Plan 0037 Persistence Contract Review

Date: 2026-08-14

## Scope

A fresh read-only reviewer evaluated the Plan 0037 documentation milestone on
branch `docs/0037-persistence-contract` against the approved product decisions,
existing PTY and multiplexing ADRs, current architecture, PRD, and headless
operating contract.

The reviewed files were:

- `TODO.md`;
- `docs/PRD.md`;
- `docs/architecture.md`;
- `docs/headless.md`;
- `docs/plans/0037-0.2-persistence-and-upgrade-handoff.md`;
- `docs/adr/0011-guarded-in-place-daemon-reexec.md`; and
- `docs/adr/0012-defer-durable-terminal-archives.md`.

The review was documentation-only. It did not authorize or perform production
implementation, installation, daemon replacement, graphical testing, commit,
push, merge, release, or publication.

## Accepted product decisions

- `0.2.0` durable recovery remains recipe-only. Durable terminal-body archives
  are deferred and recorded only as a possible, non-committed `0.3.0` program.
- The next human launcher invocation performs a handoff automatically only when
  the running and installed builds explicitly negotiate compatible protocol,
  checkpoint, and descriptor ranges.
- `0.2.x` release-series membership alone is not a compatibility promise.
  Incompatible active upgrades block until an exact-count destructive fallback
  is explicitly confirmed.
- The first `0.1.x` to handoff-capable `0.2.0` transition requires one confirmed
  bootstrap restart, including when the reported live Splint count is zero.
- Normal compatible handoff visibly pauses input and restores the original
  Window's ordered tabs, active tab, focused pane, and eligible controller
  disposition after pinned-client relaunch and full resnapshot without requiring
  a click or tab switch.

## Initial findings and resolution

The first pass returned two blockers and four fixes worth doing immediately:

1. **Window-local state was not preserved across client exec.** Resolved with a
   bounded anonymous sealed Window resume record carrying only ordered Dojo IDs,
   active tab, focused Splints, and exact old-connection mapping. Multi-Window,
   multi-connection, rollback, expiry, cleanup, cross-Window, and no-body gates
   were added.
2. **Continuation and resume claims lacked a non-transferable process binding.**
   Resolved by binding the old and replacement client to one inherited monitored
   pidfd, kernel-supplied per-message credentials, exact old connection set,
   pinned executable identity, trusted replacement connections, and daemon
   generation. Transfer, replay, mismatch, and conflict fail closed.
3. **Bootstrap confirmation was ambiguous.** Resolved by making the first
   `0.1.x` to `0.2.0` restart mandatory and confirmed even when idle; automatic
   idle restart begins only after that boundary.
4. **Abrupt failure could leave a named body-bearing checkpoint.** Resolved by
   requiring anonymous sealed memory-backed descriptors with no named fallback
   and crash, kill, hang, and service-cleanup evidence.
5. **The PRD could imply current conditional upgrade continuity.** Resolved by
   stating explicitly that no then-currently released alpha package upgrade
   preserved processes and that negotiated continuity is an unimplemented
   `0.2.0` target.
6. **Listener recreation contradicted listener preservation.** Resolved by using
   listener descriptor adoption/re-registration consistently.

The same reviewer performed a bounded follow-up against those fixes and returned
**APPROVE** with no unresolved blocker or fix worth doing now.

## Pull-request review follow-up

A later pull-request review identified that retaining an open regular-file
executable descriptor does not freeze its bytes against a privileged in-place
rewrite. The contract now rejects ordinary writable package-file descriptors as
execution authority. Forward and rollback daemon/client images must be copied
into independently rehashed executable memfds with the complete
`F_SEAL_WRITE | F_SEAL_GROW | F_SEAL_SHRINK | F_SEAL_SEAL` set before preflight
or quiescence. Only those immutable snapshots may be executed or establish
trusted-client identity. The implementation gates now require source-copy race,
source rewrite/truncation, seal, writable-mapping, exact-exec, rollback, and
trusted-authority evidence.

## Validation evidence

The parent validation after the fix pass recorded:

```text
git diff --check                                      PASS
cargo fmt --all --check                               PASS
relative Markdown links: 105 targets / 7 files       PASS
changed non-Markdown files                            0
stale decision/finding terminology scan               0 matches
```

## Verdict

Plan 0037's product contract, guarded in-place re-exec ADR, and archive-deferral
ADR are accepted for implementation planning. Production implementation remains
pending and must proceed through the plan's milestone gates.

Residual implementation risks remain explicit: adoptable PTY ownership, exact
Window/connection correlation, compositor focus restoration, checkpoint and
descriptor ABI correctness, fault injection, package rollback, and the bounded
post-commit crash interval. These are implementation evidence requirements, not
unresolved documentation decisions.
