# Plan 0019 closure evidence — 2026-08-09

## Scope and build identity

This artifact closes the Window-local Dojo tab contract against repository HEAD
`1e0a4adbe5c6875cc547d56acdbf546398c31b1b` plus the exact retained
`implementation.patch`. The release binaries used by the final guarded smoke and
matrix were built from that worktree:

- `splinterm`: `eb9452ad9fc70d9dcc94584907f8f6c68d5c15718d17df1dd95c8c730b00134b`;
- `splinterd`: `289548b27de3fa7a3627ddfa3de0f36cdd7fb8cb45e053b35ccdda287bfc9c75`;
- adjacent `splinterm-pty-child`: `dd9d96c97814d4b1f918f192b3354fd81981304bbc5050658e8a4eb28f82413f`.

The product change raises the daemon's still-hard private protocol connection
admission bound from 32 to 128. One graphical Splint currently retains separate
observation and control connections; the old global cap made the documented
32-tab Window bound unreachable at tab 15. The new cap provides bounded capacity
for one 32-tab Window, its topology/theme channels, transient human inspection,
and cleanup headroom. It does not change authorization, subscriptions per
connection, frames, queues, controllers, image budgets, or the separate
8-connection image-body transport limit.

Focused regressions additionally prove:

- hidden updates advance semantic cached state without rebuilding frames or
  emitting resize/control commands;
- deactivation releases controllers for every pane in the hidden tab;
- closing a tab drops renderer image leases so the shared cache can reclaim
  content;
- pane tasks are cancelled and joined before close cleanup returns;
- tab 33 is rejected before daemon creation; and
- exactly 128 connection permits can be admitted, saturation rejects the next
  admission, and releasing one permit restores one unit of capacity.

## Complete non-graphical validation

All commands passed; exact logs and a machine-readable summary are under
`validation/`:

- `cargo test --workspace -- --test-threads=1`;
- `cargo test -p splinterd --test end_to_end -- --test-threads=1` — 18 passed;
- `python -m pytest -q tools/automation/test_session_picker.py` — 14 passed,
  6 subtests passed;
- `cargo clippy --workspace --all-targets -- -D warnings`;
- `cargo fmt --all -- --check`;
- `git diff --check`; and
- `git diff --cached --quiet` — the index contained no staged changes.

Relevant current counts include 296 active Splinterm library tests with one
ignored manual timing harness, 50 Splinterm binary tests, 58 daemon library
tests, and 55 daemon binary tests.

## Guarded graphical authority and isolation

The user approved one complete smoke and conditional matrix. Every attempt used
release-profile development binaries with a private daemon, Unix socket, state,
configuration, clean-shell wrapper, and trace directory. Production binaries,
daemon, and historical topology were untouched.

Each Window mapped silently on workspace 8 / DP-2 with `no_initial_focus`, full
compositor opacity, floating geometry, and a freshly selected exact address/PID.
Before every generated input phase the harness required that same identity on
workspace 8 and monitor 1. Only the exact Window was focused. The current
pre-run user Window and cursor were restored after each complete run.

Final cleanup records prove workspace 8 empty, DP-2 scale 1.0/transform 0 and
unfocused, original focus and cursor restored, isolated processes stopped, and
all private runtime/config/state/launcher paths absent.

## Passed guarded smoke

`smoke-summary.json` records the exact assertions and six captures. After the
independent reviewer identified that the original initial frame preceded first
composition, the complete smoke was rerun. The corrected harness commits and
observes `ONE_TAB_OPAQUE_OK`, waits one additional second, and only then captures
`01-one-tab.png`. The image visibly shows the opaque terminal body, trusted tab
strip and controls, committed output, prompt, and cursor.

1. one-tab dark/opaque normal presentation at scale 1.2;
2. `Ctrl+Shift+D` created and opened one second Dojo without replacing the
   native Window;
3. `Ctrl+Tab` and `Ctrl+Shift+Tab` selected the expected terminals;
4. the inline picker activated an existing session without topology growth;
5. a hidden tab drained a 2,000-line burst while the active tab accepted and
   committed `ACTIVE_RESPONSIVE`;
6. activation immediately displayed `HIDDEN_DONE` from cached hidden state;
7. pointer activation and trusted close targets preserved the exact Window;
8. pointer close detached only the tab and left topology byte-identical; and
9. closing the last tab closed the Window while both Dojos and Splints remained
   Running.

The final smoke was rerun after the connection-cap fix and again after the
reviewer's first-composition finding; both passed with exact cleanup. The final
retained smoke set is the corrected post-review run.

## Passed matrix and resource evidence

`matrix-summary.json` records one exact Window across:

- dark opaque / light translucent / dark translucent themes;
- normal / compact / minimal layouts;
- scales 1.2 / 1.5 / 2.4;
- 1 / 2 / 16 / 32 tabs;
- two Lairs with ambiguous Dojo names, visibly rendered as cross-Lair labels;
- bounded paging that kept the active tab, close target, and `+` visible;
- active-tab removal; and
- final-tab Window closure.

The tab-33 request emitted the bounded application diagnostic and left daemon
topology byte-identical. Closing one active tab and then every remaining
client-local tab also left all 32 daemon Dojos and 32 Splints Running. The raw
final topology is retained in `matrix-summary.json`.

The matrix harness's original
`01-one-tab-dark-opaque-normal-scale120.png` was also captured before first
composition. It remains retained as honest execution provenance but is excluded
from positive acceptance evidence. The corrected post-review smoke image is the
one-tab visual authority. Matrix resource sampling and all later composed
2/16/32-tab captures remain valid.

### Retained client resources

| Tabs | RSS | Idle CPU ticks / 2 s | Threads | FDs |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 36,496 KiB | 0 | 4 | 20 |
| 2 | 44,744 KiB | 0 | 4 | 22 |
| 16 | 47,916 KiB | 0 | 4 | 50 |
| 32 | 56,592 KiB | 0 | 4 | 82 |

The 32-tab client retained 20,096 KiB more RSS than the one-tab sample, remained
at four threads, and consumed zero measured idle CPU ticks. File descriptors
increased by the expected two retained protocol connections per added Splint;
the daemon admission fix preserves a hard global ceiling.

### Warm switch latency

Raw stage-v2 traces are retained under `trace/`. Each required count has 14
`tab_switch` samples (the explicit 12-switch sample plus adjacent activation
records):

| Tabs | Median | p95 | Maximum |
| ---: | ---: | ---: | ---: |
| 2 | 3.188 ms | 5.057 ms | 5.057 ms |
| 16 | 3.019 ms | 4.212 ms | 4.212 ms |
| 32 | 3.152 ms | 4.890 ms | 4.890 ms |

The separate ~200 ms wall samples intentionally include a fixed 180 ms harness
settle interval and are retained only as execution provenance, not product
latency.

## Harness diagnostics retained honestly

The final executed harnesses are under `harness/`. Earlier attempts were stopped,
diagnosed, and cleaned before retry:

1. the first one-tab screenshots preceded the first committed composition; the
   corrected full smoke waits for committed terminal output before capture;
2. embedded newline text did not synthesize a physical Return;
3. an asynchronous configured shell made immediate text markers nondeterministic,
   so the final harness used a clean-shell wrapper and committed tab targets;
4. the generic no-focus wait oracle rejected explicitly approved exact focus;
5. raw JSON marker searching ignored cell-separated terminal text;
6. a 2,000-line burst scrolled the echoed command out, so completion correctly
   required the retained output marker once;
7. first cross-Lair picker navigation selected the already-open session; the
   corrected documented `j, j, Enter` selected Beta;
8. the pre-fix daemon repeatedly stopped accepting probes at tab 15, exposing
   the real 32-connection product blocker;
9. the first successful post-fix matrix reporting pass expected nested trace
   fields, while stage-v2 records flatten metrics at the root.

No invalid attempt is used as positive product evidence. Every attempt closed
its exact Window/processes and restored workspace, monitor, focus, cursor, and
private paths; one cursor mismatch was explicitly identified by the user as
concurrent pointer movement and was not represented as product cleanup evidence.

## Acceptance mapping

1. Ordered bounded Dojo tabs: pure tab tests plus 1/2/16/32 graphical matrix.
2. Client-local only: pointer/keyboard/all-tab close left daemon topology and
   Running processes unchanged.
3. Picker open-or-activate: smoke unchanged-count and same-Window assertion.
4. Keyboard contract/input ownership: focused shortcut tests and smoke actions;
   Plan 0017 modal leak evidence remains applicable.
5. Close never kills Dojos/Splints: smoke, 32-tab matrix, and retained Plan 0025
   close-other evidence.
6. Explicit async Dojo identity: targeted topology tests plus Plan 0025 inactive
   mutation evidence.
7. Hidden bounded state/no paint/resize/blink/controller: new cache/no-frame/no-
   command and all-controller-release tests, hidden burst, zero idle ticks.
8. Trusted-strip geometry/input/IME/resize/damage: current geometry, pointer,
   IME, resize, damage suites and scale/size captures.
9. Resource release and transactional bound: task-join, image-lease, controller,
   tab-bound, connection-admission tests and tab-33 matrix.
10. Focused/workspace validation: complete retained logs pass.
11. Guarded graphical evidence: corrected final smoke and matrix pass with exact
    cleanup; the pre-composition matrix frame is explicitly excluded.
12. Independent review: fresh reviewer `d4898b93` accepted the bounded connection
    and resource boundary, then identified the first-composition screenshot and
    clean-index evidence blockers. Both requested bounded corrections are
    retained in `review/disposition.md`; no reviewer finding remains unresolved.

Plan 0019 satisfies its closure boundary and may be marked complete.
