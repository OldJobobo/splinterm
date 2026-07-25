# Phase 5 Slice 2 — reviewed Sixel differential matrix

This directory records the final guarded Splinterm-side matrix for the five
pinned Foot 1.27.0 Sixel fixtures. Foot commit
`3c5b584b0eafa772eb4376fb6eaf6643399e190e` remains the behavioral oracle.

## Result

All five cases are byte-exact for the compared 7×17 cell and report
`exact=true` plus `final_buffer_exact=true`:

- `opaque-red-column-1x6`
- `transparent-green-trailing-trim`
- `raster-attributes-repeat-red-3x6`
- `carriage-return-overlap-and-graphical-newline`
- `hls-primary-remap-red`

Every report records workspace 8 on DP-2, pre-map no-focus placement, the test
workspace never becoming active, the test window never becoming active,
placement remaining isolated, and verified cleanup. All cases used the same
reviewed release client:

`4afa748437e8299b4161082c4c2d64cec1956842e721fffc4286367a81b3246c`

## Validation

- `cargo test -p splinterm-terminal --test images` — 22 passed
- `cargo test -p splinterm --lib renderer::tests::` — 63 passed
- `cargo test -p splinterd --test end_to_end -- --test-threads=1` — 16 passed
- `cargo +nightly fuzz run terminal-advance -- -max_total_time=60` — 117,896 executions in 61 seconds, no crash
- `cargo fmt --all --check` — passed
- focused warning-denied Clippy for the changed Sixel test — passed
- two fresh read-only reviews — no correctness/evidence blocker after the local capture-condition lint fix

The workspace-wide warning-denied Clippy command remains blocked by unrelated
pre-existing benchmark/oracle worktree warnings. The broad contract validator
also reports a host Kitty-document hash drift before reaching its retained
Sixel checks; the five Sixel reports and their hashes were validated directly.
The serialized daemon gate passes; the non-serialized workspace aggregate had
two timeout failures while running the long daemon suite concurrently.

Earlier failed smoke directories remain beside this directory as an honest
record of harness development. They are not acceptance evidence.
