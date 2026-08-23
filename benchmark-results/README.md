# Local benchmark workspace

This directory contains generated benchmark evidence and is intentionally kept
out of Git except for this retention guide. Do not use `git clean` here without
reviewing the paths below.

Current size at multiplexer-benchmark publication: approximately 70 MiB across
44 run directories.

## Canonical multiplexer publication sources — retain

These directories are the immutable local sources for the tracked compact
publication artifact in
`docs/benchmarks/artifacts/2026-08-05-plan0016-publication/`:

- `20260805T210644Z-milestone3-3w10m-seed-13372075-approved-retry/` — final
  36-warmup/120-measured multiplexer matrix with 183 source checksums.
- `20260805T215737Z-milestone4-five-terminal-idle-3w10m-seed-20260723-restart/`
  — final 15-warmup/50-measured five-terminal idle control.
- `20260805T220456Z-plan0016-publication-review/` — generated publication,
  provenance, implementation snapshots, and independent review.

Keep these until the complete raw evidence has a separately verified durable
archive or release attachment. The tracked compact artifact is not a replacement
for the raw reports when re-aggregation or forensic review is required.

## Selected diagnostic evidence — retain for catch-up analysis

- `20260805T165030Z-native-single-2000-line-ansi-diagnostic/` — original native
  ANSI graphical catch-up failure.
- `20260805T192101Z-native-ansi-revision-trace/` — retained revision trace.
- `20260805T192605Z-native-ansi-paced-subscription-smoke/` and
  `20260805T194009Z-native-ansi-reviewed-fix-final-smoke/` — paced catch-up
  development and reviewed-fix evidence.
- `20260805T201439Z-native-two-lifecycle-final-repetitions/`,
  `20260805T204112Z-native-two-controller-exit-race-repetitions/`, and
  `20260805T205655Z-native-two-controller-error-race-repetitions/` — focused
  lifecycle/controller race evidence.
- `20260805T212326Z-zellij-resize-focus-event-diagnostic/` — guarded compositor
  focus-event diagnostic.
- `20260805T164352Z-milestone3-development-matrix-smoke-complete-final/` — final
  complete one-sample safety smoke.

Future catch-up work may cite the ANSI before/after diagnostics for workload
design, but must establish fresh exact-binary stage traces rather than treating
these runs as current performance baselines.

## Superseded working runs

All other multiplexer-publication directories are intermediate smokes,
interrupted matrices, or attempts superseded by a later source/build state. They must not be included
in rankings or aggregated with the canonical matrix. They may be compressed or
removed only after the canonical sources and selected diagnostics above are
verified and a cleanup action is explicitly approved.

## Verification

```bash
(
  cd 20260805T210644Z-milestone3-3w10m-seed-13372075-approved-retry
  sha256sum --check SHA256SUMS
)
(
  cd 20260805T220456Z-plan0016-publication-review
  sha256sum --check SHA256SUMS
)
(
  cd ../docs/benchmarks/artifacts/2026-08-05-plan0016-publication
  sha256sum --check SHA256SUMS
  sha256sum --check SOURCE-SHA256SUMS
)
```

The five-terminal source directory did not originally contain a checksum
manifest. Its matrix hash is bound by publication `PROVENANCE.json`; the tracked
curation now retains a complete 68-file source manifest at
`docs/benchmarks/artifacts/2026-08-05-plan0016-publication/source-manifests/five-terminal-SHA256SUMS`.
