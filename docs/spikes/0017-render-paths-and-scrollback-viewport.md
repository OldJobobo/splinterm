# Spike 0017: Renderer path equivalence and scrollback viewport foundation

- **Status:** Closed — renderer paths and graphical scrollback accepted
- **Roadmap:** Phase 8.1

## Renderer path equivalence

The production snapshot cache now uses the exact FreeType grayscale bridge for
non-color faces. Tests additionally prove:

- cold and warm cache runs emit identical printable-ASCII metrics and alpha;
- full-frame paint and an all-rows semantic-damage paint are byte-identical for
  equivalent state, including backgrounds, glyphs, and cursor; and
- scale remains part of both glyph and raster-face cache identity.

The pinned oracle separately proves 95/95 exact fcft parity through both the
isolated bridge and the real production cache. Final Foot pre-compositor cell
composition and decoration geometry remain open gates.

## Scrollback authorization and attachment

The matching first-party Splinterm UI may now request the explicit
`Scrollback` scope together with `Observe`; clipboard and termination authority
remain excluded. Graphical attach and resynchronization request the protocol's
bounded history maximum instead of zero rows. The daemon still enforces the
scope and the existing 16-row transfer bound.

Protocol v9 added bounded scrollback replacement metadata to semantic updates.
Protocol v11 now assigns every daemon terminal row a monotonic identity before
it can enter history and carries a history generation, oldest/newest available
row IDs, and IDs on every returned history row. Update metadata also carries an
explicit `append { appended_rows, trimmed_rows }`, `clear`, `reflow`, or
`replace` transition. IDs move with circular row storage, distinguish duplicate
content, and remain stable through ring trims. Rows entering history are issued
IDs in chronological order even after partial-region scrolling. Clearing history,
RIS (`ESC c`), or resize/reflow advances the generation while preserving the
monotonic row-ID namespace; reflow re-identifies sparse and wrapped storage in
chronological order. Alternate-screen snapshots expose no normal-history rows.

Snapshot and update validation rejects missing, zero, duplicate, or
non-chronological history IDs, inconsistent bounds, IDs on visible row patches,
zero or regressing generations, non-advancing clear/reflow generations, and
payloads above the existing 16-row transfer cap. Per-subscription refreshes retain the
attach request's scrollback bound, so an observe-only zero-row subscription does
not receive history rows or row identities in later updates.

This is intentionally not a claim of complete graphical scrollback. Sixteen
rows remain the bounded bootstrap/update payload; practical history still needs
revision-bound paging before the cap can safely grow.

## Client viewport model

`ScrollbackViewport` introduces a renderer-independent client state model with:

- live-bottom versus detached offset;
- bounded up/down clamping;
- history-plus-live visible-row composition;
- unseen-row accounting while detached;
- anchor adjustment as available history grows; and
- return-to-live behavior for alternate screen and cleared history.

Pure tests cover clamping, row composition, new output while detached, duplicate
content, ring rollover, missing anchors, generation reset, explicit return to
live, alternate screen, and history clearing. Content-overlap inference has
been removed: the detached viewport now stores the exact top-row anchor ID,
repositions from that exact ID, and returns live rather than showing a wrong row
when that anchor is trimmed or the generation changes. Initial and replacement
snapshots are validated before rendering, and Splint/incarnation mismatches are
propagated rather than treated as newer snapshots. The Wayland client
renders the composed viewport, suppresses the live cursor while detached,
routes wheel input locally when application mouse tracking is disabled, retains
xterm mouse reports when tracking is enabled, and supports Shift+PageUp,
Shift+PageDown, and Shift+End. URL hover and selection-copy resolve against the
composed display rather than the hidden live grid.

## Revision-bound paging

Protocol v11 now provides bounded history-page requests keyed by Splint,
process incarnation, terminal revision, history generation, and the exclusive
`before_row_id` anchor. The daemon authorizes every request with both `Observe`
and `Scrollback`, rejects malformed bounds, and returns an explicit resync
response if revision or generation changes before or during extraction. Each
page remains capped at 16 rows and row IDs are selected chronologically without
materializing the configured history capacity.

Wheel accumulation now follows pinned Foot `input.c`: continuous axis distance
is multiplied and accumulated against the active renderer cell height,
`axis_value120` uses `120 / multiplier`, and discrete steps apply the multiplier
directly. The prior fixed 10-pixel threshold and arbitrary eight-line frame cap
were removed. Application mouse tracking retains multiplier 1 while local
history uses Foot's default multiplier 3. Setting `SPLINTERM_SCROLL_TRACE=1`
records input-to-commit, draw, cache, and page-batch timings without terminal
content. Initial traces exposed roughly 200 ms average scroll draws. Cache
retention alone did not materially change that result; the larger cause was the
`splinterm-test` launcher running the CPU renderer as an unoptimized debug
binary. The launcher now defaults to release binaries (with an explicit
`SPLINTERM_TEST_PROFILE=debug` override). The frame cache uses 4,096 entries as
a soft warm-cache target and drops unreferenced entries above it; an active
frame may temporarily require more referenced glyphs. Hard glyph-byte and face
budgets remain Slice 5/9 gates. A release-mode `splinterm-test` manual pass on
the reference Omarchy host accepted wheel responsiveness and bounded
multi-page history navigation as
good enough to stop interaction tuning for this slice; formal performance and
injected-input gates remain open.

The Wayland client prefetches up to four bounded pages (64 rows) before upward
navigation reaches the oldest loaded row, then delivers the batch to the
Wayland thread as one cache update. It validates page identity, deduplicates by
row ID, prepends
older rows without moving the exact viewport anchor or forcing an unchanged
viewport redraw, and preserves
loaded pages across same-generation append/replacement updates. The cache is
bounded to 4,096 rows and 16 MiB and remains one contiguous newest-history
window; upward paging stops at that client budget rather than evicting the
newest edge and creating an unpageable gap. Daemon history remains independently
bounded and authoritative. Initial snapshots and update payloads remain capped
at 16 wire rows.

## Slice 0 closure evidence

- **Release interaction sign-off:** the reference Omarchy-host pass used the
  release-default `./splinterm-test restart` workflow with
  `SPLINTERM_SCROLL_TRACE=1`. Wheel navigation remained responsive beyond the
  16-row attachment payload and continued through four-page prefetch batches.
  The earlier roughly 200 ms debug-build draw samples did not reproduce as an
  interaction blocker in release mode. These observations close interaction
  checkpoint C1/C2 only; they are not the Phase 8.1 performance baseline.
- **Automated paging evidence:** the daemon end-to-end scenario now walks four
  consecutive 16-row pages, rejects overlap, exercises stale revision and stale
  generation responses, and proves that output between bound requests forces a
  resync. Protocol tests reject malformed identity, bounds, ordering, and empty
  history metadata. Client tests enforce request identity/cursor correlation and
  both row and byte cache budgets.
- **Lifecycle check (2026-07-18, Linux 7.1.3-arch2-1 x86_64, Rust/Cargo 1.91.0):**
  `./splinterm-test ping && ./splinterm-test stop` left no matching debug or
  release daemon in `/proc` and removed both the isolated socket and PID file.
  `stop` now escalates after its bounded wait and fails rather than reporting
  success if a matching daemon survives.

No trace contains terminal content. The exact numeric release performance
budgets and committed host manifests remain Slice 9 work.

## Slice 1 final-buffer closure evidence

The clean pinned-host command
`tools/foot-oracle/run-final-buffer-comparison.py
/tmp/splinterm-final-buffer-clean-retest --workspace 8` rebuilt and tested Foot,
preflighted the pinned commit, patch/font/native-library/Cargo.lock identities,
and passed all 16 regular-12px/1× fixtures byte-for-byte. The matrix covers
printable ASCII, spacing and punctuation, narrow/wide runs, 80/240-column drift,
edge cells, reverse, dim, conceal, and hidden/block/beam/underline cursor states.
The aggregate report records 16/16 exact with zero maximum channel delta.

Matrix automation exposed and fixed three observable Foot contracts: rows
compose right-to-left when overhang masks overlap, dim foreground intensity is
two-thirds, and focused block/underline cursors are opaque with Foot's one-pixel
underline geometry. The Foot patch now uses contextual hunks, a capture marker,
and a clean disposable build; stale patched binaries are not closure evidence.

## Evidence archive

Machine-readable summaries and per-case capture metadata for the Plan 0003
Slice 1–4 closure runs are committed under
[`artifacts/0017/`](artifacts/0017/README.md). The original `/tmp` run
directories (raw ARGB buffers, heatmaps, per-glyph JSONL) are volatile and
reproducible from the documented per-slice commands.

## Phase 8.1 closure

Plan 0003 completed the remaining work with trusted bounded detached-history
status and keyboard/pointer return-to-live controls; generation/row-ID/column
selection identity across pages; explicit selection cancellation on reflow;
row/byte-bounded paging and endpoint retention; exact renderer-path/cache
output; forced resync and detach/reattach continuity; release performance
budgets; and guarded Hyprland/Omarchy sign-off. Machine-readable Slice 9 and 10
reports are archived beside the earlier parity summaries under
[`artifacts/0017/`](artifacts/0017/README.md).

Post-sign-off user validation also fixed and covered focus-loss history repaint,
application-keypad text input used by Neovim, `TERM=xterm-256color`, Omarchy 4
native theme hooks, and Foot-compatible default-background alpha. Fcitx5/Mozc
provided a live `text-input-v3` preedit, candidate selection, PTY commit, and
return to US input. Search and durable scrollback across daemon restart remain
Roadmap Phase 3 work, not Phase 8.1 gaps.
