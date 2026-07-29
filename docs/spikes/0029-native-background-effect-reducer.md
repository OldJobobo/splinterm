# Spike 0029: native background-effect reducer

- **Status:** Complete
- **Date:** 2026-07-28
- **Plan:** [Native Wayland background blur](../plans/0013-native-background-blur.md)
- **Scope:** Plan 0013 Slice 2 only

## Boundary

`crates/splinterm/src/background_effect.rs` is a side-effect-free desired-state
reducer. It imports no Wayland client or generated protocol type, owns no proxy,
and opens no connection or surface. Slice 3 will translate its ordered actions
into protocol requests.

The reducer records:

- requested blur and default-background alpha;
- manager availability and all advertised capability bits;
- the latest validated logical surface size;
- `Absent`, `Active`, or `DestroyPending` effect ownership;
- the last region represented by emitted actions;
- a pending enable, disable, or resize commit;
- which missing-protocol fallback diagnostic was emitted for the current
  rejection episode; and
- whether the surface is still alive.

## Actions and ordering

Full eligibility requires requested blur, alpha below `u16::MAX`, manager
availability, the blur capability bit, a live surface, and validated logical
geometry.

Enabling emits exactly:

1. `CreateEffect`;
2. `SetBlurRegion(LogicalSize)`; and
3. `CommitSurface(Enable)`.

An active resize emits one finite region update followed by
`CommitSurface(Resize)`. Disabling or losing eligibility emits
`DestroyEffect` followed by `CommitSurface(Disable)` and enters
`DestroyPending`. No effect can be recreated until `surface_committed()`
acknowledges that removal and returns the lifecycle to `Absent`.

When translucent blur is requested without the manager or its blur capability,
the reducer emits one `Diagnostic` action for that rejection episode. Repeated
reconciliation and alternation between missing-manager and missing-capability
states remain silent; fully resolving support or disabling the request resets
the bound. Capability loss while active orders the diagnostic before the
effect destroy and required removal commit.

Reconciliation emits no protocol lifecycle or region action while a commit is
pending, and emits nothing after the surface is destroyed. This models the
production owner's required request ordering while allowing a later
implementation to coalesce the reducer's required commit with an
already-scheduled surface commit.

## Geometry and teardown

`LogicalSize` accepts only positive dimensions representable as signed 32-bit
Wayland region widths and heights. Invalid zero, negative, and oversized values
return a typed error without replacing the last valid size. The reducer never
uses SHM dimensions or `INT32_MAX` as an unbounded region surrogate.

Surface destruction is idempotent. An `Active` lifecycle emits one final effect
destroy action; `Absent` emits nothing; and `DestroyPending` emits nothing
because its destroy request has already been produced. No commit is requested
for a surface being torn down.

## Validation

Passed:

```bash
cargo test -p splinterm --lib background_effect::tests -- --test-threads=1
cargo test -p splinterm --lib -- --test-threads=1
cargo fmt --all --check
git diff --check
```

Results:

- focused reducer: 8 passed;
- complete Splinterm library: 181 passed; and
- formatting and whitespace checks: passed.

A bounded `cargo clippy -p splinterm --lib --no-deps` scan returned success and
reported no diagnostics in `background_effect.rs`. Existing diagnostics in
unchanged production modules remain the separately recorded Rust 1.97 baseline
boundary; lint policy was not weakened.
