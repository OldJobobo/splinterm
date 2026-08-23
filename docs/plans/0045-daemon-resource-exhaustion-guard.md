# Plan 0045: Daemon resource-exhaustion guard

- **Status:** Implemented and independently reviewed in
  [PR #34](https://github.com/OldJobobo/splinterm/pull/34); awaiting integration
- **Date:** 2026-08-22
- **Scope:** Immediate aggregate service guard and packaged defaults
- **Follow-up:** Per-Dojo or per-Splint cgroup isolation

## Incident

At 2026-08-22 18:57 PDT, `systemd-oomd` killed
`splinterd.service`. Systemd recorded 19 GB peak service memory, 25.3 GB peak
service swap, and 38,074 killed processes while host memory and swap were above
the 90% kill threshold. The service restarted automatically, but its
daemon-owned processes were gone and the graphical clients lost their live
Dojos.

The triggering workload was not a daemon allocator leak. A migration test
created a `python3` stub, saved a mise shim as the supposed real interpreter,
and then invoked that shim under a `PATH` which resolved `python3` back to the
stub. The resulting recursive process storm ran inside a Splint. Because the
PTY helper and all of its descendants inherit `splinterd.service`'s cgroup, the
workload and daemon shared one resource boundary. `systemd-oomd` therefore
selected and killed the complete service cgroup.

## Hotfix decision

Add aggregate limits to the packaged user unit:

```ini
TasksMax=2048
MemoryHigh=75%
```

`TasksMax` is the primary incident guard. It makes a recursive spawn fail with
resource exhaustion before approaching the user manager's inherited 38,271-task
ceiling. `MemoryHigh` is a reclaim and throttling boundary which gives the
kernel earlier pressure feedback; it is not a hard memory ceiling.

Do not add an aggregate `MemoryMax` in this hotfix. All Splints still share the
service cgroup, so a hard service-level memory event can damage or terminate
unrelated Dojos. Do not claim that these aggregate settings provide per-Dojo
isolation.

## Runtime mitigation

The incident host received the equivalent transient properties without
restarting the daemon:

```bash
systemctl --user set-property --runtime splinterd.service \
  TasksMax=2048 \
  MemoryHigh=75%
```

The effective cgroup reported `pids.max=2048` and
`memory.high=25130590208`; `splinterd.service` remained active and its current
Dojo remained listed. These transient properties disappear when the user
manager restarts or when explicitly reverted.

## Implementation

1. Add the two limits to `dist/systemd/user/splinterd.service`.
2. Require the exact settings in the package validator.
3. Document that the limits cover the aggregate daemon and terminal workload,
   how to inspect them, and why `MemoryHigh` may include reclaimable page cache.
4. Keep the triggering migration-test correction in its owning Omarchy
   repository; do not mix that unrelated source change into this branch.

## Validation

Run the following non-graphical checks:

```bash
systemd-analyze verify --user dist/systemd/user/splinterd.service
python tools/package/validate-package.py --help
python -m unittest discover -s tools/package -p 'test_systemd_unit.py'
git diff --check
```

Use the repository's actual focused package command if the validator does not
expose the assumed test module. Inspect the complete diff before independent
review. Do not reproduce the original recursive workload. Any exhaustion probe
must use a separate transient user scope with a small task ceiling, memory
ceiling, and timeout; graphical testing is out of scope.

## Validation result

The implementation passed the following non-graphical checks on 2026-08-22:

- `python -m unittest discover -s tools/package -p 'test_*.py' -v` — 15 tests
  passed;
- `systemd-analyze verify --user dist/systemd/user/splinterd.service` — passed
  with no diagnostics;
- `python -m py_compile tools/package/validate-package.py
  tools/package/test_systemd_unit.py` — passed; and
- `git diff --check` — passed.

A fresh read-only reviewer inspected the complete scoped files and parent-supplied
`origin/main` diff. The review decision was `PASS`, with no blocker or fix worth
doing now. It retained the aggregate-cgroup and non-hard-memory-boundary limits
below as residual risks.

## Acceptance

- The packaged unit has an aggregate 2,048-task ceiling and 75% memory-high
  boundary.
- Package validation fails if either setting is absent.
- Unit syntax and focused package tests pass.
- Documentation does not describe page cache as free memory or the aggregate
  guard as per-Dojo isolation.
- Independent review finds no blocker.

## Rollback

The live transient guard can be removed without restarting the daemon:

```bash
systemctl --user set-property --runtime splinterd.service \
  TasksMax=infinity \
  MemoryHigh=infinity
```

A packaged rollback removes the two unit properties, runs
`systemctl --user daemon-reload`, and leaves restart timing to the user because
a daemon restart terminates daemon-owned processes on the current release.

## Architectural follow-up

Move terminal workloads out of the daemon control plane's resource boundary.
Each Dojo or Splint should receive its own delegated cgroup with a bounded task
count and configurable memory policy. Exhaustion must affect only the offending
workload while the daemon reconciles its exit and keeps unrelated topology
alive. That work requires a separate plan covering transient-unit ownership,
PTY spawn and adoption, daemon handoff, cleanup, persistence, and upgrade
compatibility.
