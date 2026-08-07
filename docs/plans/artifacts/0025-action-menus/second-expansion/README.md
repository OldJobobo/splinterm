# Plan 0025 second-expansion validation

Recorded on 2026-08-07 from the uncommitted second-expansion worktree using
release-profile development binaries, workspace 8 / `DP-2`, and the isolated
socket `/run/user/1000/splinterm-test/splinterd.sock`. Production daemon PID
`3194395` and installed production binaries were not used or changed.

## Non-graphical evidence

- `cargo test --workspace -- --test-threads=1`: passed; full output is in
  `workspace-tests.txt`.
- `cargo test -p splinterm --lib`: 268 passed, 1 ignored.
- Focused action-menu tests: 10 passed.
- Focused topology-manager tests: 7 passed.
- `cargo test -p splinterd --test end_to_end -- --test-threads=1`: 16 passed.
- `cargo check -p splinterm --all-targets`, `cargo fmt --all --check`, and
  `git diff --check`: passed.
- `cargo clippy -p splinterm --all-targets`: completed with the documented Rust
  1.97 warning baseline and no new action-menu correctness failure; output is in
  `clippy.txt`.

## Guarded graphical evidence

- `31-command-palette.png`: searchable closed built-in catalog with categories,
  selection, and shortcut hints.
- `rename-prompt.png`: trusted rename prompt prefilled from the captured Dojo.
- `inactive-tab-menu.png`: exact six-row tab-focused context menu on an inactive
  tab.
- `terminate-default-cancel.png`: named two-pane confirmation with Cancel
  selected by default. Enter left daemon topology byte-identical.
- `terminate-topology-before.txt` and `terminate-topology-after.txt`: affirmative
  termination removed the exact inactive Dojo and both captured live Splints
  while retaining the active Dojo and its running Splint.
- `terminate-reconciled-one-tab.png`: after frontend acknowledgement and repaint,
  only the retained active tab remains visible.

The first affirmative test exposed that `Request::CloseDojo` correctly rejects a
Dojo containing live Splints. The corrected path captures exact
`(SplintId, incarnation)` pairs, revalidates the complete set before every kill,
kills only captured live runtimes, refreshes topology, and closes the exact Dojo.
A later pass exposed stale inactive-tab pixels after successful state removal;
the `RemoveTab` handler now invalidates tab-label chrome and schedules a full
redraw.

A fresh read-only review found no identity-propagation or default-cancel blocker.
It requested bounded hardening for drift between serial kills, stale/not-found
close races, and request-sequence coverage. The implementation now revalidates
before every kill, retries only while the exact exited set remains, and settles
an already disappeared target without retargeting. Focused tests plus the
isolated multi-pane graphical sequence cover the resulting dispatch order and
reconciliation behavior.

Every graphical attempt used a freshly selected exact development PID/address
and verified workspace 8 / `DP-2` immediately before generated input. Cleanup
removed the isolated client, daemon, and socket; production PID `3194395`
remained active. Foot `0x55a31a544060` and cursor `(853,764)` were restored.

## Publication and installation

Commits `e51ebe2` and `94f5686` were pushed to `origin/main`. A temporary clean
worktree at committed `94f5686` produced and validated matched split packages
with the full PKGBUILD test suite. With explicit authorization, production
`splinterd` was stopped while it reported no active Lairs, both Pacman packages
were reinstalled through `pkexec`, and the service restarted as PID `86563`.
Package/installed hashes match, Pacman reports zero altered files, the running
daemon's sibling trusted client matches `/usr/bin/splinterm`, desktop validation
passes, and human-mode `splinterm list` still reports no active Lairs. The
rollback and exact hashes are in `installation-summary.txt`.
