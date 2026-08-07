# Plan 0025 expanded action-menu validation

Recorded on 2026-08-06 from commit `61827ff` using the isolated development
socket and release-profile client on workspace 8 / `DP-2`. Production binaries
and the production daemon were not used by the graphical sequence.

## Non-graphical evidence

- `cargo test --workspace`: passed.
- `cargo fmt --all --check`: passed.
- `git diff --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: blocked by the
  recorded pre-existing Rust 1.97 warning set outside this slice. The run found
  the known `too_many_lines`, `collapsible_if`, `struct_excessive_bools`,
  `manual_is_multiple_of`, and test-only `useless_vec` findings; it did not
  identify an action-menu-specific correctness failure.

## Guarded graphical evidence

- `palette-filter-split.png`: `Ctrl+Shift+P`, bounded `split` search, category
  labels, shortcut hints, selected row, and modal placement. Escape dismissed
  without query leakage; a later horizontal-split execution changed the
  isolated topology from one running Splint to two.
- `context-inactive-tab.png`: six-action menu anchored to an inactive tab with
  `Activate Tab` enabled.
- `context-active-tab.png`: the same compact menu on the active tab, with
  `Activate Tab` disabled and selection skipping to `New Dojo`.
- `inactive-split-retains-active-tab.png`: splitting the inactive tab increased
  that exact Dojo's Splint count while the second tab stayed active.
- `close-other-tabs-detached.png`: after `Close Other Tabs`, the client retained
  one tab while the isolated daemon still retained all three captured Dojos and
  four running Splints, proving detach-only behavior.

Outside-click dismissal completed without topology mutation. The close-others
frontend acknowledgement arrived after the first immediate strip capture; the
settled client state was one tab. One test-operator `End` key was intentionally
ignored because tab menus support arrow navigation only, so Enter created an
extra isolated Dojo; the corrected four-Down sequence then exercised
`Close Other Tabs`. All isolated processes were terminated, the socket was
removed, and the pre-test Foot focus and cursor position were restored.

A fresh read-only source review found no source defect or fix worth doing in the
expanded implementation. Its formal attestation remained rejected because the
reviewer's sandbox could not read the supplied `/tmp` manifest/diff artifacts;
that evidence-delivery limitation is recorded rather than represented as a
source acceptance.
