# Spike 0030: native background-effect Wayland lifecycle

- **Status:** Complete
- **Date:** 2026-07-28
- **Plan:** [Native Wayland background blur](../plans/0013-native-background-blur.md)
- **Scope:** Plan 0013 Slice 3 only

## Production boundary

The disposable graphical client now binds
`ext_background_effect_manager_v1` version 1 opportunistically. The manager is
not required for startup and remains owned by the per-window `App`; the daemon,
PTY state, private Splinterm protocol, renderer pixels, and pinned Foot oracle
are unchanged.

Startup seeds the pure Slice 2 reducer from the resolved blur preference and
default-background alpha. Live accepted theme transitions remain intentionally
out of this slice and will update that desired state in Slice 4.

## Protocol translation

`crates/splinterm/src/wayland.rs` is the sole Wayland side-effect owner. It
translates ordered reducer actions as follows:

- `CreateEffect` calls `get_background_effect` for the main window surface;
- `SetBlurRegion` creates one temporary SCTK `Region`, adds the finite logical
  rectangle `(0, 0, width, height)`, sends `set_blur_region`, and drops the
  region immediately;
- `DestroyEffect` destroys and removes the sole effect proxy; and
- `CommitSurface` commits the window surface and then acknowledges that exact
  reducer commit reason.

The executor rejects duplicate creation, a region without an effect, a destroy
without an effect, and reducer/commit disagreement as clean client errors. It
does not allocate SHM, rebuild a terminal snapshot, repaint pixels, or request a
frame callback.

Initial and changed logical geometry is supplied from `WindowHandler::configure`
before the resize's terminal draw is scheduled. The region therefore uses
surface-local logical dimensions rather than scaled SHM dimensions. Capability
events are handled for the manager's full lifetime; known blur support and
unknown future bits are both preserved by conversion to the reducer's raw flag
state.

## Fallback and teardown

When the manager is absent, normal window creation continues. When the manager
reports no blur capability, or later loses it, the reducer's one-episode
fallback diagnostic and normal transparent rendering remain active. A bound
manager is not treated as capability-ready until its asynchronous
`capabilities` event arrives.

Capability gain can lazily create the effect after geometry exists. Capability
loss orders the bounded diagnostic, effect destruction, and protocol-only
surface commit. The reducer is acknowledged immediately after the commit
request, preventing duplicate recreation before removal is committed.

The optional manager is bound only after fallible pre-`App` setup is complete.
Every fallible event-loop exit is captured, then `App` asks the reducer to
destroy any active effect and destroys the manager before propagating the loop
result or dropping the remaining window fields. Surface teardown sends no
additional commit.

## Bounded metadata tracing

Setting `SPLINTERM_BACKGROUND_EFFECT_TRACE` enables metadata-only stderr lines
for manager version or absence, raw capability bits and known blur support,
object create/destroy, finite region dimensions, and reducer commit reason.
Fallback diagnostics remain bounded by the reducer even when tracing is off.
The trace formatter has focused tests and cannot include terminal body or
clipboard data.

## Non-graphical validation

Passed during implementation:

```bash
cargo test -p splinterm --lib background_effect -- --test-threads=1
env -u WAYLAND_DISPLAY -u WAYLAND_SOCKET cargo check -p splinterm --all-targets
cargo test -p splinterm --lib -- --test-threads=1
cargo fmt --all --check
git diff --check
```

The focused coverage includes reducer lifecycle tests, generated known/unknown
capability flag conversion, and bounded diagnostic/trace formatting. The full
library suite passed before review, and the no-compositor all-target build
proved the bindings and dispatch signatures without opening a window.

A bounded `cargo clippy -p splinterm --lib --no-deps` scan returned success and
reported no diagnostic in the new background-effect integration. The
repository's pre-existing Rust 1.97 warnings still prevent describing the full
workspace `-D warnings` command as passing; this slice neither weakened lint
policy nor expanded into unrelated cleanup.

No graphical client or compositor smoke test was launched. Those remain blocked
until the explicit Slice 5 approval gate.
