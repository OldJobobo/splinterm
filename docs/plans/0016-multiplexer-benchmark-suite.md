# Plan 0016: multiplexer benchmark suite

- **Status:** In progress
- **Date:** 2026-08-04
- **Benchmark foundation:** [Terminal benchmark suite](../benchmarks/terminal-benchmark-plan.md)
- **Lifecycle contract:** [ADR 0006](../adr/0006-multiplexing-lifecycle.md)

## Goal

Compare Splinterm's native multiplexing with tmux and Zellij without presenting
terminal-plus-multiplexer stacks as bare terminal emulators. The first matrix is:

1. `splinterm-native`;
2. `foot-bare`, as the rendering and nesting control;
3. `foot-tmux`; and
4. `foot-zellij`.

The initial topology curve is `single`, `two-columns`, and `four-grid`. A later
five-terminal by two-multiplexer cross-product is out of scope until this smaller
matrix proves that its boundaries and accounting are useful.

## Fairness and safety contracts

- Use the same Foot profile, font, geometry, child executable, workload bytes,
  locale, topology, and randomized block order for every applicable stack.
- Report native and nested stacks as stacks. Do not subtract Foot's measured
  overhead to manufacture a primary "multiplexer-only" score.
- Record both infrastructure resources and totals including every workload.
  Infrastructure includes terminal clients, multiplexer clients and servers,
  Splinterm's daemon, and equivalent helpers.
- Discover detached servers as explicit process roots; ancestry from the
  graphical terminal alone is insufficient.
- tmux uses a unique `TMUX_TMPDIR`, `-L` socket, session, and checked-in profile.
- Zellij uses a unique `ZELLIJ_SOCKET_DIR`, session, and checked-in profile.
- Ambient processes and default-namespace session counts are recorded only as
  counts. User session names are never retained. Ambient sessions are never
  included in resource totals or cleanup selectors.
- Cleanup targets only the exact benchmark namespace and session. A failed
  cleanup invalidates the sample.
- Graphical work remains opt-in and follows the workspace-8/DP-2 guardrails. One
  Splinterm-native smoke must pass before peer stacks or a matrix run.

## Boundaries

Every sample names one external boundary:

- topology request to all child readiness records;
- trigger to all PTY write-completion records;
- trigger to visible-marker screenshot polling approximation;
- targeted input to selected child receipt;
- targeted input to visible-marker screenshot polling approximation;
- outer resize dispatch to every pane's settled geometry;
- divider resize dispatch to affected panes' settled geometry;
- detach to client exit and reattach to all panes visible; or
- child exit to documented stack lifecycle state.

Screenshot polling is not compositor presentation or input-to-photon latency.
Per-pane readiness and completion records remain separate before an aggregate
`all` boundary is calculated.

## Cases

For each supported topology:

1. startup and topology readiness;
2. settled idle CPU, context switches, PSS/RSS, and process count;
3. simultaneous plain, ANSI, and Unicode output in every pane;
4. the existing twelve-step outer-window resize sequence;
5. deterministic equal-ratio divider changes;
6. active-pane targeted input;
7. detach and reattach continuity; and
8. process exit plus exact namespace cleanup.

The single-pane cases are controls. Multiplexer scaling claims must use the
change from one to two to four panes, not only absolute four-pane totals.

## Dependency-ordered milestones

### Milestone 0 — portable foundation (complete 2026-08-04)

- Probe exact tmux and Zellij executable paths, versions, hashes, ambient process
  counts, and default-namespace session counts without retaining session names.
- Add explicit stack identities to the reproducibility manifest.
- Add isolated tmux and Zellij namespace plans with exact cleanup commands.
- Define equal deterministic `single`, `two-columns`, and `four-grid` trees.
- Produce equivalent tmux creation actions and plugin-free Zellij KDL layouts.
- Add checked-in profiles that disable status/plugin UI, mouse integration,
  persistence, web service behavior, and automatic layout changes where the
  tool supports those controls.
- Define `splinterm.benchmark.multiplexer.v1`, including separate infrastructure
  and child-inclusive totals plus explicit process roles.
- Cover probes, privacy, namespace isolation, topology materialization, schemas,
  CLI output, and manifest compatibility in portable tests.

Validation:

```bash
ruff format --check tools/benchmark/multiplexers \
  tools/benchmark/multiplexing.py tools/benchmark/manifest.py tools/benchmark/run.py
ruff check tools/benchmark/multiplexers tools/benchmark/multiplexing.py \
  tools/benchmark/manifest.py tools/benchmark/run.py
python -m pytest -q tools/benchmark/test_benchmark.py
python tools/benchmark/run.py probe-multiplexers --require-all
python tools/benchmark/run.py manifest /tmp/splinterbench-manifest.json
python tools/benchmark/run.py validate /tmp/splinterbench-manifest.json
```

### Milestone 1 — headless orchestration (implemented and independently accepted 2026-08-04)

- Every pane receives a unique readiness channel and direct benchmark-child
  command. Trigger, completion, and input channels remain case-specific work for
  the graphical/output milestones.
- The three topologies materialize and inspect in isolated tmux and Zellij
  sessions without a graphical terminal. Zellij inventory permits only its
  suppressed internal link record, never visible plugin UI.
- Native Splinterm topology orchestration runs against an isolated daemon,
  socket, and state directory through the supported JSON CLI.
- Server/daemon and workload PIDs bind exact process incarnations and form
  non-overlapping role sets. Linux process-tree discovery now reads every thread's
  `children` file so worker-thread PTY spawns are not omitted.
- All implementations use short benchmark-owned Unix socket roots, then verify
  exact server exit, workload exit, socket removal, and unchanged ambient counts.
- The randomized nine-case development matrix passed for Splinterm, tmux, and
  Zellij across `single`, `two-columns`, and `four-grid`. The pre-existing
  default Zellij namespace retained the same one-session count throughout; no
  session names were recorded.

Evidence: [2026-08-04 headless matrix](../benchmarks/artifacts/2026-08-04-multiplexer-headless/summary.md).
Independent follow-up review accepted exact PID/start-tick server capture, guarded
fallback forest discovery, strict process-role accounting, artifact currency, and
verified cleanup across all nine cases.
These readiness values validate orchestration boundaries and are not a
performance ranking.

Execution gate passed: topology, readiness, role accounting, ambient isolation,
and cleanup are headlessly verified for all three multiplexer implementations.

### Milestone 2 — guarded two-pane smoke (completed 2026-08-04)

After explicit approval for the bounded graphical sequence:

1. `splinterm-native` passed the `two-columns` idle/readiness smoke;
2. workspace 8 / DP-2 placement, no focus, process accounting, host-state
   preservation, and exact cleanup passed;
3. the gated `foot-tmux` and `foot-zellij` peer smokes then passed; and
4. all three cases produced current implementation snapshots, strict reports,
   screenshots, and checksums.

Evidence: [2026-08-04 guarded graphical smoke](../benchmarks/artifacts/2026-08-04-multiplexer-graphical-smoke-2/summary.md).
This is topology, isolation, and cleanup evidence, not a performance ranking.

### Milestone 3 — development matrix

Run three warmups and ten measured randomized samples over all four stacks and
three topologies. Establish startup, idle, output, resize, targeted input,
detach/reattach, and lifecycle reports. Retain raw records, execution order,
profiles, executable identities, implementation snapshots, and checksums.

Implementation and the guarded development gate completed 2026-08-05. The
measured runner now covers all ten cases in one isolated stack/topology cell;
the matrix persists an immutable seeded plan, exact resume identity, raw
per-pane records, infrastructure and workload-inclusive resources, source and
profile snapshots, executable identities, cleanup evidence, summaries, and
checksums. Independent Foot uses one window per pane at equivalent aggregate
geometry; its divider and detach cases are explicit not-applicable results.

A randomized one-sample smoke with seed `13372075` passed all 12 stack/topology
cells, 111 measured operations, nine explicit not-applicable operations, schema
and semantic validation, and exact cleanup. This is implementation and safety
evidence, not a performance ranking.

The initial full-scale ANSI blocker is resolved. Revision tracing showed the
daemon publishing large intermediate ANSI grids faster than the wire/client
could consume them. Terminal subscriptions now pace admitted frames at 33 ms
while compact mailboxes coalesce output; revocation and expiry remain
preemptive, lag fails closed, and final update/exit ordering bypasses pacing.
A guarded 2,000-line native smoke reduced ANSI visibility from a 60-second
timeout to under one second. Two fresh read-only review rounds rejected the
first security-sensitive draft, then approved the corrected implementation.

Full-matrix execution also exposed and fixed native detach/lifecycle races:
managed windows now use topology authority rather than transient layout
materialization, final resync-plus-exit bypasses impossible post-exit resync,
and controller completion no longer races terminal lifecycle observation.
Repeated native single/two-column regressions and exact cleanup pass.

The required Milestone 3 evidence run remains pending. The latest fresh 3+10
attempt passed 18 cells, including all native ANSI and lifecycle cases reached,
then stopped when a Foot/Zellij divider operation activated reserved workspace
8. Owned windows, processes, and namespace were removed, but the host-state
cleanup assertion correctly remained invalid because focus entered the test
workspace. Do not retry the graphical matrix without a fresh approved guarded
sequence.

### Milestone 4 — current baseline and publication review

Rerun the current bare five-terminal baseline before interpreting the new
matrix. Existing July development artifacts predate later Splinterm performance
and resize work. Record independent review and bounded graphical evidence before
claiming the multiplexer lane complete.

## Stop gates

Stop for a new decision if the implementation requires:

- including ambient user sessions in measurements or cleanup;
- changing persistent user tmux or Zellij configuration;
- comparing only native Splinterm against a nested peer without Foot-bare
  controls;
- inferring detached-server cost from terminal ancestry;
- using focus or global synthetic input to drive graphical samples;
- widening the first matrix to every terminal host; or
- running a graphical smoke or matrix without the repository's explicit
  isolation approval and gates.
