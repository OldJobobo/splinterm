# Plan 0028 Phase 4 closure evidence — 2026-08-09

## Build and installation

- Fix commit: `6a7263d` (`Preserve human control channels after transfer timeout`).
- Validated package SHA-256: `2884fd09f8d0d91818ed74acaeec92a4463707775d056c805a90b5e2c3446d44`.
- Installed on Wintermute and Holodeck through Pacman.
- Both hosts reported `splinterm: 42 total files, 0 altered files`.
- Matching installed executable SHA-256 values on both hosts:
  - `splinterm`: `1c579a1ac16ccb91c47c6b996afb4c660037b7de1bbe79e7db80531878233cbf`
  - `splinterd`: `41bb5c7b0bcaeea73a900d7a7f9cefc8392ee884bab5d6c3bb3a74b397bf4`
  - `splinterm-relay`: `33f50a884764d8fe7c17fbf3807443231d7d92086b5ba774363cfac1e99a8937`
- `splinterm remote check holodeck` passed before and after the matrix.

The package builder ran with checks enabled and reported `Validation was successful` and `Package validation passed`. This included the complete package test/check surface. Focused pre-package validation also passed:

- `cargo test -p splinterm-automation-client` — 35 passed;
- `cargo test -p splinterd --lib` — 58 passed;
- `cargo test -p splinterd --bin splinterd` — 54 passed;
- `cargo test -p splinterm --lib` — 293 passed, 1 ignored manual harness;
- `cargo test -p splinterm --test remote_session` — 13 passed;
- strict Clippy for `splinterd`, `splinterm-automation-client`, and `splinterm`;
- `cargo fmt --all -- --check`; and
- `git diff --check`.

Fresh read-only security/lifecycle review run `f0b233ca` found no blocker or fix worth doing now. It confirmed that transfer expiry still removes server-side authority while only Automation connections consume adapter-handle revocation disconnects, and that clean EOF remains distinct from truncated private frames.

Final read-only closure review run `f30bada9` compared the Phase 4 matrix and acceptance criteria against this artifact, the six screenshots, the fix, retained prior evidence, package/install validation, and cleanup. It found no blocker or required evidence gap and concluded: `may mark complete`.

## Guarded graphical matrix

Target: Wintermute native client to `oldjobobo@holodeck`, with test windows isolated on workspace 8 / DP-2 and selected by fresh exact Hyprland address before input. The original Foot focus was restored after each case.

Passed evidence:

1. Agent-backed `remote check` and one-authentication native launch.
2. Native remote creation mapped one Window without changing focus.
3. Remote shell semantics rendered `CWD=/home/oldjobobo` and `SHELL=/usr/bin/bash`.
4. Two hundred ordered lines rendered; detached scrollback and physical-key search found `ORDER_150`.
5. Compositor resize changed the remote PTY from `48×115` to `35×84`.
6. Physical `Ctrl+Shift+Enter` created a live second Splint; `SPLIT_MATRIX_OK` rendered.
7. Physical `Ctrl+Shift+D` created a second Dojo/tab; tab switching and input passed.
8. A second native client observed the exact two-pane Dojo.
9. A 15-second ordinary control-transfer timeout left the RemoteInteractive controller channel and process alive with no EOF/partial-frame diagnostic.
10. A second ordinary transfer request was accepted on the matching left pane; `TRANSFER_OK` rendered in the new controller, then release completed.
11. Password-only authentication used a temporary strict SSH alias with public keys disabled. One native client mapped and owned exactly one SSH child carrying the fixed graphical relay command.
12. Desktop no-TTY authentication used `/usr/lib/seahorse/ssh-askpass`; the local dialog mapped on workspace 8, one native client mapped, and it owned exactly one SSH child.
13. Terminating one exact local SSH child closed only its client; a sibling client and all three remote Splints remained healthy.
14. Terminating one exact newly created remote `splinterm-relay --graphical-stdio` process closed only its client, emitted bounded `splinterd closed the connection` diagnostics, preserved the sibling client, and left all Splints Running.
15. Stopping Holodeck `splinterd` closed the remaining remote client. Restart retained one coherent topology with three `Exited(129)` Splints, and `remote check` passed.
16. Local graphical regression used isolated packaged adjacent binaries, socket, daemon, and state. `LOCAL_REGRESSION_OK` rendered; closing its Window left its isolated Splint Running. The isolated daemon/state were then removed without touching Wintermute's historical topology.
17. Prior retained Phase 4 evidence covers unknown and changed host-key rejection, reconnect/reopen, role/protocol rejection, no-image authority, and raw relay compatibility.

## Defect found and corrected during closure

The initial transfer attempt timed out because acceptance was sent from a different pane than the request. Timeout also exposed that `connection_revocations`—intended to invalidate Automation adapter handles—disconnected `TrustedUi` and `RemoteInteractive` connections. That killed the human pane-control channel and clean EOF was misleadingly reported as a partial frame.

Commit `6a7263d` limits connection-revocation disconnect handling to `Automation` while leaving expiry/revocation of server-side authority unchanged. It also reports empty EOF as connection closure and reserves partial-frame diagnostics for buffered truncation. Focused tests, complete package validation, read-only review, and the corrected real-host timeout/accept matrix passed.

## Cleanup

- Both exact remote test Windows and all authentication/failure-injection clients exited.
- Holodeck's sole Plan 0028 test Lair was guarded-reset; final state: active daemon, zero Lairs.
- Workspace 8 / DP-2 was empty.
- Original Foot focus `0x55a31a480a10` was restored.
- Temporary password SSH alias, temporary Splinterm profile, helper scripts, staged remote packages, and isolated local runtime/state were removed.
- Wintermute's pre-existing historical topology was not reset or modified by the local regression.
- Rollback snapshots were retained under each host's `~/.local/state/` for package recovery.

## Screenshot manifest

See `SHA256SUMS`. Successful captures retained here:

- `smoke.png`
- `search.png`
- `multipane.png`
- `resize.png`
- `transfer-fixed.png`
- `local-regression.png`
