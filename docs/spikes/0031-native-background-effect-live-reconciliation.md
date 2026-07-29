# Spike 0031: native background-effect live reconciliation

- **Status:** Complete
- **Date:** 2026-07-28
- **Plan:** [Native Wayland background blur](../plans/0013-native-background-blur.md)
- **Scope:** Plan 0013 Slice 4 only

## Accepted theme updates

The Wayland owner now applies accepted `WindowUpdate::Theme` values to both the
renderer theme and the pure background-effect reducer. The theme watcher still
validates files before sending this message, so a rejected reload sends no
update and leaves the prior alpha, blur request, palette, and effect lifecycle
unchanged.

Active and inactive multi-pane update streams share one application path. The
window continues to own one theme, one main `wl_surface`, one reducer, and at
most one background-effect proxy regardless of pane count.

Theme updates are classified into independent work:

- a blur-only change reconciles effect state but requires no renderer pixel
  rebuild;
- alpha or palette changes rebuild pixels through the existing theme path; and
- an identical theme performs neither kind of work.

A blur-only update therefore does not mutate terminal snapshots, rebuild a
`SnapshotFrame`, allocate or attach SHM, request a frame callback, damage the
surface, or schedule a draw. If the reducer requires a lifecycle change, the
owner sends the protocol requests and a protocol-only surface commit
immediately.

## Commit coalescing

When a theme change already requires new pixels, the owner records that effect
reconciliation is required but does not generate protocol actions while a
frame callback may delay drawing. Immediately before the actual SHM attach and
surface commit, `draw` reconciles the latest alpha, blur request, capability,
and logical geometry. Any reducer commit is then deferred only within that
single synchronous draw invocation and acknowledged directly after the same
`wl_surface.commit` request.

The deferred commit marker never survives an event-loop turn. A separate pure
reconciliation scheduler preserves draw-bound desired state until `draw`
begins, so later blur-only updates and capability events cannot consume an
alpha or geometry transition against old pixels. Capability-only changes remain
immediate when no draw-bound transition exists.

## Logical resize behavior

Initial and changed `WindowHandler::configure` values validate and replace the
reducer's finite logical size, then queue reconciliation for the draw already
required by the configure. Multiple configure events delayed behind a frame
callback retain only the latest logical dimensions. The reducer emits no region
request for terminal damage, pixel-only theme changes, output scale changes
that preserve logical size, or settled state.

The draw path generates the finite region immediately before the commit that
also applies the latest viewport destination and buffer. This preserves
surface-local geometry while coalescing the resize effect commit with existing
render work.

## Close and settled state

Closing before a queued draw reaches reconciliation owns no new proxy. Closing
after draw-time creation but before commit follows the existing fatal-error
cleanup and destroys the active proxy. Closing with a removal commit pending
uses the reducer's `DestroyPending` teardown rule and does not send a duplicate
destroy or an inert-surface commit.

After a blur-only protocol commit or a draw-coalesced commit is acknowledged,
repeated reconciliation emits no action, schedules no draw, and adds no timer
or polling source.

## Non-graphical validation

Passed during implementation before review:

```bash
cargo test -p splinterm --lib wayland::tests::blur_only_theme_updates_reconcile_immediately_without_queuing_pixel_work -- --test-threads=1
cargo test -p splinterm --lib wayland::tests::draw_bound_effect_reconciliation_survives_later_updates_and_capabilities -- --test-threads=1
cargo test -p splinterm --lib background_effect -- --test-threads=1
cargo test -p splinterm --lib -- --test-threads=1
env -u WAYLAND_DISPLAY -u WAYLAND_SOCKET cargo check -p splinterm --all-targets
cargo fmt --all --check
git diff --check
```

Focused owner-scheduler coverage proves blur-only changes reconcile immediately
without queuing the draw path, while alpha and palette changes retain their
required pixel semantics. It also proves a queued draw reconciliation survives
later blur-only and capability updates, is consumed exactly once by draw, and
is cleared by teardown. Existing reducer coverage proves settled no-op behavior,
one finite region transition per reconciled logical size, multi-step alpha/blur
toggles, and deterministic destruction from `Absent`, `Active`, and
`DestroyPending`.
The startup/live theme watcher tests continue to prove malformed reload
suppression and preservation of the last accepted complete `ResolvedTheme`.

A bounded `cargo clippy -p splinterm --lib --no-deps` scan returned success with
no diagnostic in the Slice 4 integration. The existing Rust 1.97 warning
baseline remains outside this slice and lint policy was not weakened.

No graphical client or compositor test was launched. Graphical validation
remains blocked until the explicit Slice 5 approval gate.
