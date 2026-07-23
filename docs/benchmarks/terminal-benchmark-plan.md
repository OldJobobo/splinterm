# Terminal benchmark suite plan

## Purpose

The comparative suite measures Splinterm, Foot, Kitty, Ghostty, and Alacritty at
common external boundaries. It does not compare Splinterm's internal renderer
timers directly with whole competitor processes, and it does not collapse
performance, correctness, and features into one "winner" score.

Foot 1.27.0 commit `3c5b584b0eafa772eb4376fb6eaf6643399e190e`
remains Splinterm's behavioral oracle. Comparative measurements do not replace,
modify, or regenerate that reference evidence.

## Measurement lanes

### Common end-to-end performance

Run identical workloads through each terminal at 80x24 and 240x80:

- cold and warm launch to benchmark-child readiness;
- idle CPU, context switches, RSS, shared memory, and process count;
- plain text, SGR-heavy, scrolling, and Unicode output;
- bounded and effectively disabled scrollback profiles;
- deterministic resize sequences;
- memory retention after repeated output and clear cycles; and
- child exit and terminal shutdown behavior.

Every result must identify whether its boundary is child readiness, PTY write
completion, visible-marker detection, or compositor presentation. These are not
interchangeable latency measurements.

### Latency

The initial suite may measure launch-to-child and PTY write completion. A later
lane may measure output-to-visible with a high-contrast marker and guarded,
tightly cropped screenshots. Screenshot polling is an approximation and must be
reported as such.

Input-to-child and input-to-visible require equal synthetic input delivery. On
this development system, benchmark windows may never receive focus. Those cases
therefore remain deferred until a nested compositor or virtual-input mechanism
can inject input without violating the workspace guard.

### Correctness and capabilities

Correctness is reported independently from speed:

- the repository's Foot-derived VT fixtures and final-buffer oracle;
- parser fuzzing;
- Unicode width, combining, emoji, SGR, alternate-screen, cursor, erase, resize,
  title, and hyperlink cases;
- crash, timeout, and malformed-sequence handling; and
- a feature matrix for graphics protocols and other non-common capabilities.

Unsupported features are marked unsupported, not scored as zero-performance
runs.

### Splinterm architecture

Splinterm-only measurements remain a separate lane:

- daemon and client resource totals;
- detach and reattach latency;
- process continuity after client closure;
- snapshot, paging, and bounded update performance;
- multiple Splints and viewers; and
- queue, history, and backpressure bounds.

The existing Phase 9 benchmark remains the primary internal diagnostic:

```bash
python tools/performance/run-phase9-baseline.py OUTPUT_DIR
```

## Fairness contract

A publishable run must use the same host, compositor, monitor, scale, font files,
font size, geometry, palette, locale, child command, and workload bytes. Disable
transparency, blur, background images, ligatures, bells, URL detection, and
shell integration where each terminal permits it. Record every exception.

Use release or distribution builds, record binary hashes and complete version
output, and save normalized configuration files with the result. Run terminals
sequentially in randomized blocks. Keep cold and warm samples separate. Retain
all raw samples and never silently remove outliers.

Single-PID RSS is not a fair primary metric. Each run should eventually use a
transient cgroup and report both terminal-infrastructure resources and totals
including the benchmark child. Splinterm infrastructure includes `splinterd`,
the graphical client, and its PTY helper. Competitor helpers must be counted by
the same rule.

## Statistics

Development smoke runs use three warmups and ten samples. Publishable runs use
five warmups and thirty samples per case, randomized block ordering, and report
median, p95, min, max, median absolute deviation, and a documented confidence
interval. Thermal or background-load preflight failures invalidate a run rather
than becoming unexplained outliers.

## Result layout

```text
benchmark-results/TIMESTAMP/
├── manifest.json
├── configs/
├── raw/TERMINAL/CASE/SAMPLE.json
├── summary.json
└── summary.md
```

`tools/benchmark/result-schema.json` defines the first portable record shape.
Schema versions are append-only within a major version.

## Graphical isolation

All graphical cases follow the repository guardrails already implemented by
`tools/performance/run-phase9-graphical.py`:

- inactive workspace 8 on DP-2;
- pre-map placement and no initial focus;
- one guarded case before a matrix;
- continuous placement and user-workspace checks;
- immediate abort on any placement or focus violation; and
- verified cleanup after every case.

A benchmark command must not make graphical execution the default.

## Implementation milestones

1. **Portable foundation:** result schema, host manifest, terminal probes,
   deterministic workload child, process-tree and cgroup readers, aggregation,
   and unit tests.
2. **Guarded idle smoke (implemented):** Splinterm and Foot adapters, one
   fixed-size idle case, child-inclusive process-forest accounting, separate
   child-ready/window-map boundaries, and cleanup verification.
3. **Five-terminal baseline (implemented):** randomized startup/idle,
   trigger-gated plain/ANSI/Unicode, twelve-step resize, and mixed-output
   retention blocks are implemented for all adapters. Output records keep
   PTY-write and screenshot-visible boundaries separate.
4. **Correctness report:** reuse Foot fixtures and add only portable external
   observations where terminal-private state is unavailable.
5. **Presentation and input latency:** implement only after a common, auditable
   nested-compositor or virtual-input boundary exists.

## Current commands

The portable milestone is intentionally non-graphical:

```bash
python tools/benchmark/run.py probe
python tools/benchmark/run.py probe --json
python tools/benchmark/run.py manifest /tmp/splinterbench-manifest.json
python tools/benchmark/run.py validate /tmp/splinterbench-manifest.json
python tools/benchmark/run.py summarize /tmp/splinterbench-manifest.json
python tools/benchmark/run.py sample-process $$ --json
python tools/benchmark/workloads/bench-child.py plain --lines 1000 >/dev/null
```

The probe reports availability; it does not launch terminal windows.

The guarded milestone runs exactly one terminal per invocation and refuses to
operate unless workspace 8 is already empty, inactive, and assigned to DP-2:

```bash
python tools/benchmark/run-graphical-idle.py /tmp/splinterbench-idle --terminal splinterm
python tools/benchmark/run-graphical-idle.py /tmp/splinterbench-idle --terminal foot
python tools/benchmark/run-graphical-matrix.py /tmp/splinterbench-matrix \
  --warmup-runs 3 --samples 10 --seed 20260723
python tools/benchmark/run-output-matrix.py /tmp/splinterbench-output \
  --warmup-runs 3 --samples 10 --seed 20260724 --lines 2000
python tools/benchmark/run-resize-matrix.py /tmp/splinterbench-resize \
  --warmup-runs 3 --samples 10 --seed 20260725
python tools/benchmark/run-retention-matrix.py /tmp/splinterbench-retention \
  --warmup-runs 3 --samples 10 --seed 20260726 --lines 5000
```

Run Splinterm first as the guarded smoke. Run Foot only after the first result
records `valid: true` and `cleanup_verified: true`. This milestone measures
launch-to-child-ready and launch-to-window-map separately; neither is described
as input-to-photon latency.
