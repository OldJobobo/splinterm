# Plan 0010 bounded release closure

This publication closes Plan 0010 against clean committed source
`34f2839e74f991abc1ea5b60298f33fb94d57280`, the immutable focused performance
publications produced by its descendant plans, a complete Arch package build,
a current sanitizer-backed parser fuzz run, and installed-system validation.

## Release validation

- `PKGBUILD check()` passed 750 Rust tests with one intentional ignored manual
  benchmark and zero failures.
- Package validation passed 14 Python tests plus core and optional MCP extracted
  runtime checks.
- `terminal-advance` completed 102,081 executions in 61 seconds with no crash,
  timeout, or sanitizer finding.
- Core and already-installed MCP packages were installed through Pacman after
  rollback copies were saved.
- `/usr/bin/splinterm` and `/usr/bin/splinterd` are root/Pacman owned.
- The restarted daemon's device, inode, and SHA-256 exactly match the installed
  `/usr/bin/splinterd`.
- Human-mode `/usr/bin/splinterm list`, desktop validation, and Pacman integrity
  checks pass; Pacman reports 42 files and zero alterations.

## Performance evidence

- [Plan 0016 publication](../2026-08-05-plan0016-publication/README.md) owns the
  guarded multiplexer and five-terminal evidence.
- [Plan 0022](../2026-08-06-plan0022-history-catchup/README.md) owns stage-trace
  correlation and the live-history fast path.
- [Plan 0023](../2026-08-06-plan0023-ansi-history-throughput/README.md) owns the
  reducer optimization and focused graphical ANSI confirmation.
- [Resize preparation evidence](../2026-08-06-plan0024-resize-preparation/README.md)
  proves that resize has no duplicate expensive content preparation worth changing.

No new broad graphical matrix was run for installation. Existing evidence is
preserved rather than reinterpreted as presentation timing.

## Explicit limitations

Plan 0010's original provisional table predates Plans 0016 and 0022–0024. This
closure does not fabricate a new 5-warmup/30-sample all-lane matrix, graphical
control binary, or presentation timestamp. Microsecond-scale zero-history
confidence remains unresolved; Plan 0022's original all-pane diagnostic remains
above 50 ms p95; screenshot polling remains coarse; and broad Rust 1.97 Clippy
style debt remains outside this performance closure.

These residuals do not invalidate the accepted bounded changes, complete package
suite, exact installed identities, or published focused gates. Any future
performance claim requires a new focused plan and new correlated evidence.

## Contents

- `summary.json`: machine-readable closure result.
- `PROVENANCE.json`: exact commands, source, packages, and retained evidence.
- `installed-validation.txt`: installed identity and integrity record.
- `review.md`: independent closure decision.
- `SHA256SUMS`: compact publication integrity.

Raw package, fuzz, and installation logs remain locally retained. Rollback
binaries remain under the raw installation directory and must not be deleted
without approval.
