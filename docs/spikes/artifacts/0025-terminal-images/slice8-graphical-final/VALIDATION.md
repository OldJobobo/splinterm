# Non-graphical closure validation

Recorded against the same worktree used for the final guarded matrix.

## Passed

- `cargo test --workspace -- --test-threads=1`
  - complete serialized workspace suite passed;
  - daemon end-to-end gate: 16/16;
  - terminal image gate: 23/23;
  - renderer library: 163/163;
  - CLI, automation, protocol, PTY, relay, MCP, and documentation tests passed.
- `cargo +nightly fuzz run terminal-advance -- -max_total_time=60`
  - 103,027 executions in 61 seconds;
  - 2,711 edges and 14,468 features at completion;
  - no crash, timeout, or sanitizer finding.
- `cargo test --manifest-path tools/image-spike/Cargo.toml`
  - 9/9 passed.
- warning-denied Clippy for `tools/image-spike`: passed.
- `python -m unittest tools.automation.test_session_picker`
  - 14/14 passed.
- `cargo fmt --all --check`, focused `git diff --check`, Python compilation,
  final artifact checksum validation, and pinned Foot cleanliness: passed.

## External/worktree notes

- `python tools/image-spike/validate_contracts.py` stops on installed Kitty
  document hash drift. Pinned retained fixture/capture hashes remain unchanged.
- Workspace warning-denied Clippy reaches pre-existing Rust 1.91 style lints
  outside the Phase 5 closure edits.

## Follow-up control review

Eager and later lazy control acquisition treat `ControllerUnavailable` as the
expected observer fallback. A second window therefore remains attachable
without control when another client owns the exclusive lease. Input and resize
attempts while ownership remains unavailable are dropped without terminating
the control subscription, so explicit transfer/takeover remains available.
Successful acquisition and all non-ownership errors retain their prior
behavior. Focused unit tests cover successful acquisition, ownership conflict,
and propagation of other protocol errors.

## Package closure

`tools/package/build-local-package.sh` builds the clean committed source into
split `splinterm`, `splinterm-mcp`, and debug packages. Extracted runtime
validation passes for the main and MCP packages, including service/unit files,
private relay behavior at protocol version 23, installed licenses/provenance,
and `usr/share/doc/splinterm/images.md`.
