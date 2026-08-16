# Plan 0042: Beta1 wide Splint grid

- **Status:** Post-fix implementation and packaged graphical acceptance accepted; maintenance integration boundary passed with recorded oracle-host limitation
- **Date:** 2026-08-14
- **Product authority:** A maximized Splint uses the complete terminal-cell area
  of validated 1440p and 4K surfaces instead of silently stopping at the legacy
  240-column protocol ceiling
- **Release line:** `maint/0.1`
- **Depends on:** accepted four-sided geometry, bounded semantic terminal
  updates, exact protocol negotiation, daemon-owned PTY resize, and the new 0.1
  maintenance workflow
- **Converges with:** the planning artifact on
  `plan/0041-alpha3-4-active-tab-foreground` at
  `docs/plans/0041-alpha3-4-active-tab-foreground.md`, only after that branch is
  reviewed and integrated; implementation branches remain separate and serial
  at shared documentation/configuration files
- **Tracking note:** `TODO.md` reconciliation is intentionally deferred until
  the Plan 0041 branch's existing uncommitted `TODO.md` edit is integrated; do
  not create a concurrent writer conflict at that known convergence point

## Decision

Replace the legacy `240x80` maximum grid with a Beta1 contract that supports a
complete `480x128` terminal grid. This covers maximized 2560x1440 and 3840x2160
Splints using the shipped 14 px font profile with bounded headroom for ordinary
padding, pane chrome, and fractional compositor scale.

`480x128` is the Beta1 validated grid envelope, not a promise that every
combination of the currently parseable 6 px minimum font, arbitrary future 8K
surface, and every pane arrangement can grow without bound. If a surface/font
combination naturally exceeds the negotiated envelope, Splinterm must retain a
bounded trailing residual and emit one rate-limited diagnostic identifying the
negotiated grid cap. Silent clipping is no longer acceptable.

The wire change uses a new exact protocol version. A graphical client must fit
geometry to the server-advertised `maximum_columns` and `maximum_rows`, not only
to its compile-time constants. A pre-Beta1 server fails the exact-version
handshake. A same-version test or constrained endpoint may advertise smaller
valid grid limits, which the client respects instead of sending a rejected
resize.

Retain bounded frames. Beta1 may increase the exact-version fixed
`MAX_FRAME_BYTES` once, from 8 MiB to at most 16 MiB, only if exact adversarial
encoding evidence proves the largest accepted Beta1 snapshot and update fit.
The advertised `maximum_frame_bytes` must equal that fixed protocol constant;
it is not a separately negotiated transport value. If 16 MiB is insufficient,
stop Plan 0042 and request a separate scope/product decision for transactional
chunking rather than adding it automatically or raising the frame ceiling again.

The selected hard memory ceilings are 16 MiB per wire frame and per-subscriber
queued terminal payload, 64 MiB aggregate queued publication payload per Splint,
256 MiB aggregate queued/in-flight terminal publication payload in the daemon,
64 MiB semantic-plus-prepared terminal state per graphical pane, and 512 MiB
aggregate terminal presentation state per Window. Milestone 1 must add checked
byte accounting and fail-closed admission/backpressure at these boundaries.
Measured baseline and high-water values remain evidence, but cannot replace
these ceilings or silently widen them.

### Milestone 1 measured stop condition

`crates/splinterm-protocol/examples/grid-frame-envelope.rs` serializes exact
compact JSON shapes for 480x128 visible grids plus the permitted 16 snapshot
scrollback rows. The first measurement recorded:

| Cell profile | Snapshot | All-row update | Fits 16 MiB |
| --- | ---: | ---: | :---: |
| empty | 0.20 MiB | 0.18 MiB | yes |
| scalar | 1.06 MiB | 0.94 MiB | yes |
| 64-character composed | 9.36 MiB | 8.33 MiB | yes |
| fully styled scalar | 20.18 MiB | 17.94 MiB | no |
| fully styled 64-character composed | 28.48 MiB | 25.32 MiB | no |

The fully styled scalar case is valid under the current terminal-cell contract
and exceeds 16 MiB without relying on pathological composed text. The Plan 0042
stop-loss therefore triggered before changing protocol dimensions. The recorded
scope decision selected an exact-version compact terminal-cell attribute tuple
while retaining the 16 MiB frame ceiling and 480x128 target.

Post-change measurement reduced the fully styled scalar snapshot/update to
4.62/4.11 MiB. The first composed measurement used a 2-byte combining mark and
incorrectly suggested that every profile fit. Parent diff inspection corrected
the adversarial sequence to 63 permitted 4-byte supplementary-plane variation
characters after its base scalar. That valid profile measures 17.67/15.71 MiB
without attributes and 21.23/18.87 MiB with full attributes. Compact attributes
alone therefore do not satisfy the 16 MiB complete-state contract. The recorded
second-stage decision retains compact attributes and 16 MiB individual frames,
then adds bounded ordered terminal transactions: at most one in flight per
connection, 8 MiB maximum raw terminal-frame payload chunk, 32 MiB maximum
aggregate unchunked state, contiguous exact indices/count/length, and atomic
publication only after complete reassembly. Nesting, interruption, duplicates, reordering,
length/count mismatch, or aggregate overflow fail closed and require bounded
resynchronization. The earlier 16 MiB per-subscriber queued-payload ceiling is
superseded only by one 32 MiB in-flight transaction; multiple queued aggregate
transactions remain prohibited. The implemented transaction boundary now has
protocol and client integration coverage for oversized snapshots and updates,
corrupt base64, duplicate/reordered chunks, inconsistent lengths, aggregate
overflow, interruption, and EOF cleanup. Daemon encoding reserves 96 MiB of
transient admission per transaction beneath a 256 MiB global ceiling, so no
more than two encodes can retain unchunked plus base64-framed bodies at once. Final
acceptance still requires the complete validation and review boundaries below.

### Pinned default-profile geometry evidence

The non-graphical renderer fixture resolves JetBrains Mono Nerd Font Regular at
14 px output-scale sizing and records these exact uncapped grids:

| Scale (120ths) | Cell pixels | 2560x1440 | 3840x2160 |
| ---: | ---: | ---: | ---: |
| 120 | 13x30 | 195x47 | 293x71 |
| 150 | 17x38 | 186x46 | 280x70 |
| 180 | 20x44 | 190x48 | 286x72 |
| 240 | 26x59 | 195x48 | 293x72 |

All remain below 480x128. Pixel dimensions are asserted alongside grid cells so
scale is applied exactly once.

### Provisional 480x128 renderer evidence

A one-sample release run records a 96,816,384-byte tight-grid canvas,
142,045,184-byte process RSS after the profile, 1,206,703,370 ns cold-frame
preparation, 113,970,615 ns full paint, 9,126,676 ns one-row preparation, and
606,420 ns one-row paint. The canvas is Window-owned rather than per-pane
semantic state and remains under the 512 MiB Window ceiling. These values prove
the profile is measured; they do not establish a threshold. Multi-sample
acceptance must retain the existing 80x24 and 240x80 gates unchanged and report
the heavier maximum-grid costs honestly.

### Non-graphical implementation review and validation

Fresh protocol/security review `aa0f3c8c` and geometry/renderer review
`652435dc` rejected the first acceptance candidate. The owning writer fixed the
stale package v34 pin, transaction-wrapped nonterminal acceptance, pre-decode
base64 allocation, unaccounted publication queue, focused-pane limit loss,
missing cap diagnostic, missing 512 MiB Window presentation bound, and weak
480x128 report validation. Parent verification confirms each correction.

The post-fix boundary records 28 protocol tests, 40 automation-client tests,
379 Splinterm library tests with one manual timing harness ignored, 60 daemon
library tests, 72 daemon binary tests, 19 serial daemon integration tests, 13
serial remote-session tests, and 67 package/benchmark Python tests. Workspace
clippy passes for all targets with warnings denied; formatting and
`git diff --check` pass. The PTY integration suite requires building the adjacent
`splinterm-pty-child` helper first. No graphical test, installation, package
replacement, push, merge, or release action is included in this evidence.

### Packaged graphical acceptance candidate

The separately approved 2026-08-15 matrix first tested the exact package from
`dc8e1165968f66c17dd872bf6153b8eb1681650a`. It passed the guarded smoke,
exact 2560x1440 fullscreen case at `317x69`, right-edge output/cursor/pointer
selection, pane-local grids, eight rapid `228 -> 259 -> 228` crossings with
history retained, and a real remote graphical relay without cross-endpoint
leakage.

The completed `120x64` endpoint fixture then exposed a direct-window defect:
`launch --splint-id` discarded negotiated dimensions and sent `309x66`.
Commit `6c03fb7f3365adef7b24b8afd5ffb460a0a2402a` now supplies the endpoint limits
to direct-window `WindowOptions` and shares the same conversion used by the
multi-pane path. The post-fix test boundary passes 379 library tests plus 101
binary tests, all integration tests including 14 remote-session tests,
all-target Clippy with warnings denied, formatting, and package validation.

The exact post-fix package emitted one bounded diagnostic and sent only
`120x64` / `960x1280` for a 2500x1362 pane. A real-cell selection positive
control persisted unchanged through a wholly residual x=1120..1274 drag, with
zero changed marker-crop pixels and no protocol input. Mozc displayed its popup
at column 119 inside the grid and committed UTF-8 `にほんご`; Fcitx was restored to
`keyboard-us`. No safe 4K output existed, so the plan-authorized non-graphical
4K proof remains the recorded limitation. Cleanup restored exact focus,
workspace, cursor, monitors, input method, and package state; Pacman reported
zero altered files. Fresh post-fix review `ae030b0b` returned **CLEAN**.
Evidence: [packaged graphical acceptance](../benchmarks/artifacts/2026-08-15-plan0042-packaged-graphical/summary.md).

### Maintenance integration boundary

The 2026-08-16 squash integration tree passed serialized workspace tests, 62
benchmark exporter tests, 39 Foot-oracle tests, workspace Clippy with warnings
denied, formatting, portable repository provenance, and `git diff --check`.
Plan 0042's intentional use of the existing workspace `base64` dependency added
one membership line to `Cargo.lock`; only the two synchronized Cargo.lock hashes
in `tools/foot-oracle/provenance.json` were updated.

Exact-host provenance could not execute because the machine had advanced from
pinned Fontconfig 2.18.2 and Rust/Cargo 1.91.0 to Fontconfig 2.18.3 and
Rust/Cargo 1.97.1. The approved integration exception retains this as an
explicit host limitation. The pinned Foot 1.27.0 commit, oracle patches,
fixtures, font identities, tolerances, and reference outputs remain unchanged.

## Confirmed baseline defect

The current protocol declares:

```rust
MAX_COLUMNS = 240
MAX_ROWS = 80
```

The graphical path passes those constants into
`WindowGeometry::fit_window()`. Natural geometry is calculated correctly, then
clamped before the resize command:

```text
Wayland pane rectangle
  -> SnapshotFrame::terminal_size()
  -> WindowGeometry::fit_window()
  -> min(natural columns, 240)
  -> WindowCommand::Resize
  -> daemon validation
  -> PTY + terminal grid resize
```

`WindowGeometry::from_parts()` deliberately assigns every unconsumed pixel to
the trailing right and bottom edges. At 2560x1440, a typical 9 px cell naturally
fits about 281 columns but is clamped to 240, leaving roughly 388 px on the right.
At 3840x2160 the same profile naturally fits about 424 columns and 118 rows, but
the current grid remains 240x80.

The application receives the clamped PTY size, so command output wraps or stops
at column 240. The renderer is displaying the authoritative grid; this is not a
late paint-only clipping error.

Existing tests explicitly approve the old behavior by asserting that a very
large surface returns `(MAX_COLUMNS, MAX_ROWS) == (240, 80)`. Performance and
graphical matrices likewise stop at 240x80. No accepted maximized 1440p or 4K
case proves full usable-width coverage.

## Beta1 behavior contract

1. The protocol advertises `maximum_columns = 480` and
   `maximum_rows = 128` under a new exact protocol version.
2. A local Beta1 client and daemon can resize one Splint to every grid in
   `2..=480` columns and `2..=128` rows that fits the compositor-provided pane
   rectangle.
3. After exact-version negotiation, the graphical client derives runtime
   geometry from the connected server's advertised grid limits. Compile-time
   constants remain absolute validation bounds, not the only runtime authority.
4. Handshake rejects advertised grid limits below the 2x2 minimum or above the
   client's absolute Beta1 bounds. A valid same-version endpoint may advertise
   smaller grid limits, which remain endpoint-local and authoritative.
5. Local and remote graphical paths use the same endpoint-advertised dimensions.
   A client must never send a resize larger than its endpoint advertised.
6. Pre-Beta1 and Beta1 peers fail the existing exact-version compatibility
   boundary before limits or terminal state are consumed. Mixed-version
   operation and version-range negotiation are outside this patch.
7. The daemon validates dimensions before runtime access, resizes the PTY, then
   resizes terminal state and publishes one dimension-consistent update exactly
   as today.
8. The complete terminal grid remains top-left anchored with configured padding.
   Ordinary sub-cell residual pixels remain at the right/bottom edges. A large
   residual caused by the negotiated grid cap emits one bounded diagnostic.
9. A capped grid remains fully interactive only inside its actual terminal
   rectangle. Pointer hit testing, selection, IME cursor rectangles, images,
   damage, and pane chrome must not pretend residual pixels are terminal cells.
10. The pinned Beta1 display fixture—JetBrains Mono Nerd Font Regular,
    `font-pixelsize=14`, `font-sizing-policy=output-scale`, 12 px four-sided
    padding, and declared 120/150/180/240 scale cases—must record exact resolved
    cell metrics. Its validated 2560x1440 and 3840x2160 maximized single-Splint
    cases must not hit the Beta1 cap.
11. Split layouts independently size each visible Splint from its exact pane
    rectangle. Widening one pane must not resize or retarget another pane.
12. Hidden tabs retain their existing no-resize contract; activation performs
    one final negotiated geometry reconciliation.
13. Initial `main.initial-columns` and `main.initial-rows` accept the new bounded
    maxima. Existing values and persisted dimensions remain valid without
    migration.
14. Full snapshots, terminal updates, history pages, image placements, renderer
    allocations, and capture tools validate the same absolute grid envelope.
15. `maximum_frame_bytes` must exactly equal the Beta1 compile-time protocol
    constant. Zero, smaller, or over-absolute advertisements fail handshake.
    Encoding, decoding, queueing, and admission enforce the fixed limit and the
    quantitative aggregate memory ceilings above.
16. Raising the grid envelope must not broaden input, authorization, controller,
    connection, subscription, scrollback, image-body, or audit authority.

## Explicitly outside Beta1

- unbounded grids or a promise for every 6 px font and future 5K/8K surface;
- changing font metrics, padding semantics, compositor scaling, or pane ratios;
- centering capped grids or stretching glyph/cell widths to hide residuals;
- dynamic font shrinking to force a surface under the cap;
- increasing connection, subscription, scrollback, image, or controller limits;
- changing terminal reflow semantics beyond the larger accepted dimensions;
- altering the pinned Foot 1.27.0 oracle or accepted comparison images; and
- combining the implementation branch with Plan 0041's theme-role changes.

## Implementation milestones

### Milestone 1 — protocol and resource envelope

Expected areas:

- `crates/splinterm-protocol/src/lib.rs`
- `crates/splinterd/src/main.rs`
- `crates/splinterd/src/live.rs`
- protocol, daemon, relay, and fake-endpoint fixtures

Work:

- bump the exact private protocol version;
- raise absolute grid limits to 480x128 and advertise them in `ServerLimits`;
- add checked maximum-cell and maximum-row-patch calculations instead of
  scattering unchecked products;
- measure compact and adversarial full snapshots, all-row updates, 16-row
  history payloads, styled Unicode cells, and image-placement metadata at the
  new envelope;
- retain 8 MiB when evidence fits, otherwise raise once to no more than 16 MiB;
- require the advertised frame limit to equal the selected fixed protocol
  constant and enforce it in encoding, decoding, queueing, and admission;
- add checked per-subscriber, per-Splint, daemon-global, per-pane, and per-Window
  accounting for 16 MiB physical frames, 32 MiB logical transactions, 96 MiB
  transient daemon encoding admission, 256 MiB daemon-global admission, 64 MiB
  pane semantic state, and 512 MiB Window presentation state;
- transactionally chunk only oversized Attached/Snapshot/Update terminal frames,
  with one ordered atomic aggregate per connection; and
- preserve exact-version mismatch behavior across local and graphical SSH relay
  endpoints.

Focused tests must prove:

- 480x128 is accepted and either dimension plus one is rejected before runtime
  access;
- server limits and every fixture advertise exact new values;
- checked grid products cannot overflow;
- the largest accepted complete payload fits the selected frame strategy;
- one byte over each frame and aggregate memory limit fails closed;
- zero, smaller, and over-absolute advertised frame limits fail handshake;
- zero, below-2x2, and over-absolute advertised grid limits fail handshake;
- a valid smaller same-version grid advertisement remains endpoint-local; and
- protocol-previous/current combinations fail with the existing stable
  compatibility category rather than hanging or producing a generic resize
  failure.

**Milestone gate:** do not proceed to graphical geometry until every accepted
complete 480x128 state fits the selected fixed frame bound and checked queue/
presentation accounting enforces the selected quantitative ceilings. If the
16 MiB stop-loss cannot pass, stop Plan 0042 for a separate scope decision.

### Milestone 2 — negotiated graphical geometry

Expected areas:

- `crates/splinterm/src/frontend/options.rs`
- `crates/splinterm/src/app/pane_bridge.rs`
- `crates/splinterm/src/app/topology_manager.rs`
- `crates/splinterm/src/renderer/frame.rs`
- `crates/splinterm/src/geometry.rs`
- `crates/splinterm/src/wayland.rs`
- `crates/splinterm/src/wayland/terminal_state.rs`

Work:

- carry endpoint-advertised grid limits into each pane's frontend state;
- fit terminal geometry to `min(client absolute bound, server advertised bound)`;
- preserve local/remote endpoint ownership and prevent limits from one endpoint
  or tab leaking into another;
- keep duplicate-resize suppression, queue backpressure retry, hidden-tab
  behavior, pane-local rectangles, scale reconciliation, and active-controller
  semantics unchanged;
- distinguish ordinary sub-cell residuals from residual caused by a negotiated
  grid cap for bounded diagnostics; and
- preserve exact pointer, selection, IME, image, chrome, and damage clipping to
  the actual grid.

Focused tests must prove:

- deterministic 2560x1440 and 3840x2160 single-Splint geometry pins JetBrains
  Mono Nerd Font Regular, 14 px output-scale sizing, 12 px padding, declared
  scale, exact resolved cell metrics, and the expected natural grid without a
  cap residual;
- 480x128 succeeds, 481/129 never leaves the client;
- a valid same-version 240x80 advertisement is respected;
- protocol-previous/current peers fail before consuming advertised limits;
- invalid advertised frame/grid limits fail at local and relay handshakes;
- local and remote endpoint limits remain isolated;
- fractional scales and output transitions preserve logical cell counts and do
  not double-apply scale;
- multiple panes use their own rectangles and negotiate independently;
- hidden tabs do not resize and activation reconciles exactly once;
- capped pointer/selection/IME coordinates reject trailing residual pixels; and
- resize queue saturation retries the newest dimensions without advancing the
  acknowledged size early.

### Milestone 3 — renderer, configuration, and performance boundary

Expected areas:

- `crates/splinterm/src/config.rs`
- `config/splinterm/config.ini`
- renderer frame/compose/damage/image tests
- `crates/splinterm/src/bin/final-buffer-capture.rs`
- performance and benchmark tooling
- `docs/configuration.md`

Work:

- replace hard-coded 240/80 configuration bounds with the protocol constants or
  one shared checked contract;
- ensure allocations and index products remain checked at 480x128;
- extend full/dirty/scroll rendering, image clipping, selection, and capture
  coverage to the new envelope without regenerating Foot references;
- retain the existing 80x24 and 240x80 benchmarks as regression controls and add
  480x128 as the Beta1 large-grid profile;
- record warm full prepare, one-row prepare, full paint, one-row paint, resize
  reflow, encoded bytes, queued bytes, and resident-memory evidence; and
- document the validated display/font envelope and explicit capped-grid
  diagnostic rather than claiming unbounded resolution support.

Acceptance requires no unbounded allocation, overflow, panic, frame-resync loop,
or idle work proportional to the maximum grid. Large-grid performance must be
reported honestly; do not weaken existing 80x24 or 240x80 gates to make the new
profile pass.

### Milestone 4 — Beta1 integration and stable-0.1 handoff

- implement on a short-lived branch from reviewed `origin/maint/0.1`;
- merge the accepted patch into `maint/0.1` and forward-port it to `main` through
  a separate reviewed branch;
- after the concrete Plan 0041 branch is reviewed, decide in that branch whether
  its release metadata should be retargeted to Beta1 before integration;
- serialize shared `config.rs`, `wayland.rs`, `TODO.md`, status, and release-file
  edits with Plan 0041;
- reconcile Beta1 Cargo/package/provenance versions only in a dedicated release
  integration commit after both patches reach acceptance;
- construct and promote the Beta1 candidate only from `maint/0.1` under the new
  maintenance workflow; and
- record residual display/font combinations that remain capped as explicit
  Beta1 limitations and inputs to later beta or final-0.1 planning.

Beta1 is not stable `0.1.0`. Before final `0.1.0`, the project must use Beta1
field evidence to decide whether 480x128 is the supported stable envelope or
whether a separately planned transport/renderer expansion is justified. Do not
raise limits again during release integration.

## Non-graphical validation

Focused commands, adjusted to exact test names during implementation:

```bash
cargo test -p splinterm-protocol
cargo test -p splinterd resize_limits
cargo test -p splinterm terminal_size
cargo test -p splinterm geometry
cargo test -p splinterm wayland::terminal_state
cargo test -p splinterm --test remote_session
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

At the coherent maintenance boundary:

```bash
cargo test --workspace -- --test-threads=1
python -m pytest -q tools/benchmark/test_benchmark.py
python tools/foot-oracle/check-provenance.py
python -m pytest -q tools/foot-oracle/test_*.py
```

Run release/package/site checks required by `CONTRIBUTING.md` only at integration
or candidate boundaries. Record exact failures and isolated reruns; do not retry
an expensive failed command before diagnosis.

## Packaged graphical acceptance

After separate approval for the complete guarded sequence and an approved
adjacent packaged Beta1 client/daemon installation:

1. use an isolated Window on workspace 8 / DP-2 with the shipped font and
   padding profile;
2. maximize or set an exact 2560x1440 content surface and prove output, cursor,
   selection, pointer targeting, and IME reach the final natural column;
3. repeat at exact 3840x2160 when the approved monitor/scale path can provide it
   without disturbing unrelated displays; otherwise retain a non-graphical 4K
   proof and state the graphical limitation;
4. exercise one vertical and one horizontal two-pane layout and prove each PTY
   receives its exact local grid;
5. resize rapidly across the legacy 240-column boundary and back, proving reflow,
   history continuity, no resync loop, and no stale right-edge pixels;
6. attach through one bounded remote graphical endpoint and prove advertised
   limit negotiation without cross-endpoint leakage;
7. exercise a deliberately capped synthetic profile and prove one bounded
   diagnostic plus correct residual hit-testing; and
8. restore focus, workspace, monitor mode/scale, geometry, configuration,
   package state, daemon state, and test topology.

Abort on input reaching the wrong Window, unrelated output reconfiguration,
partial terminal state, protocol loop, lost history, incorrect PTY dimensions,
stale pixels, or incomplete cleanup.

## Beta1 acceptance

Plan 0042 is complete only when:

- maximized validated 1440p and 4K default-profile Splints are no longer capped
  by the legacy 240-column limit;
- 480x128 is enforced consistently across protocol, daemon, PTY, client,
  renderer, configuration, images, capture, and tests;
- runtime geometry respects endpoint-advertised limits;
- the selected fixed frame strategy has exact payload evidence and the selected
  16/64/256/64/512 MiB memory ceilings are enforced;
- 80x24, 240x80, and 480x128 correctness/performance evidence is recorded;
- focused and serial validation plus fresh protocol/security and
  geometry/renderer reviews are recorded;
- separately approved packaged graphical acceptance is recorded; and
- the Beta1 release state is integrated, promoted, distributed, and recorded
  only under its separate release authorizations.

This plan does not authorize implementation, pushing, installation, graphical
testing, candidate dispatch, promotion approval, AUR publication, or release
publication.
