# Spike 0027: native background-effect protocol

- **Status:** Complete
- **Date:** 2026-07-28
- **Plan:** [Native Wayland background blur](../plans/0013-native-background-blur.md)
- **Foot authority:** 1.27.0 commit `3c5b584b0eafa772eb4376fb6eaf6643399e190e`
- **Protocol authority:** `ext-background-effect-v1`, version 1

## Scope

This is the compile/runtime feasibility gate for Plan 0013 Slice 0. It adds no
Splinterm configuration key and does not connect the protocol to the production
Wayland application. The disposable example binds only the manager global and
therefore creates no surface or window.

## Foot 1.27.0 behavior

The canonical checkout at `~/Playground/foot` was clean at the pinned commit.
Its request flow is:

1. The registry-global handler in `wayland.c` binds manager version 1 and
   installs the capability listener.
2. The listener replaces `have_background_blur` from every `capabilities`
   event's blur bit.
3. Window creation calls `get_background_effect` whenever the manager exists,
   before consulting alpha or the configured `blur` value.
4. For `blur=yes` and translucent alpha, `wayl_win_alpha_changed` creates a
   temporary region, adds `(0, 0, INT32_MAX, INT32_MAX)`, calls
   `set_blur_region(region)`, and immediately destroys the region. The pending
   state is applied by the following main-surface commit.
5. For `blur=yes` with opaque alpha or fullscreen presentation, Foot calls
   `set_blur_region(NULL)`; the surface-associated effect object remains alive.
6. For `blur=no`, Foot makes no blur-region request, but the effect object made
   during window creation remains alive until window teardown.
7. Teardown destroys the effect proxy before the remaining Wayland surface
   proxies are released.

Steps 5 and 6 establish the fallback hazard in Plan 0013: an empty effect object
can exist even when Foot is not requesting visible blur. Splinterm must instead
create the object lazily only while blur is eligible and must destroy/commit it
when eligibility is lost. Foot's `INT32_MAX` region is behavioral evidence, not
geometry precedent; Splinterm will use checked logical surface dimensions.

## Generated Rust API

The lockfile already resolved `wayland-protocols` 0.32.12 transitively. The
workspace now declares that exact 0.32-series package directly with `client`
and `staging` features.

`crates/splinterm/examples/background_effect_protocol_spike.rs` compile-checks
these generated client operations without invoking them:

- `ExtBackgroundEffectManagerV1::get_background_effect`;
- `WlCompositor::create_region` and `WlRegion::add`;
- `ExtBackgroundEffectSurfaceV1::set_blur_region(Some(&region))`;
- immediate `WlRegion::destroy` after the copy-semantics request;
- effect `destroy`; and
- `WlSurface::commit`.

The manager dispatch receives `Capabilities { flags }` as
`WEnum<Capability>`. Known flags expose `Capability::Blur`; unknown raw bits can
be retained or ignored without rejecting a future compositor. The generated
event enum is non-exhaustive, so production dispatch must tolerate future event
variants.

The protocol XML confirms:

- capabilities are sent on bind and whenever they change;
- blur is bit `0x1`;
- one effect object may be associated with a surface;
- creating a duplicate raises `background_effect_exists`;
- blur-region state is surface-local, copied from the temporary region, and
  double-buffered until `wl_surface.commit`; and
- destroying the effect removes its regions on the next surface commit.

## Local compositor evidence

The non-window probe ran against:

- Hyprland `0.56.1`, commit `5c9377c15f85c50648f35ca5a213754f95b93ca0`;
- manager interface version `1`; and
- capability flags `0x1` (`blur=true`).

Observed probe output:

```text
ext_background_effect_manager_v1 version=1 flags=0x1 blur=true
```

`wayland-info` is not installed on this host, so the purpose-built registry
probe supplies the exact global and event evidence without opening a window.
No graphical isolation approval was used or required.

## Validation

```bash
cargo check -p splinterm --example background_effect_protocol_spike
cargo run -q -p splinterm --example background_effect_protocol_spike
```

Both commands passed, as did `cargo fmt --all --check` and
`git diff --check`. Strict Clippy with Rust 1.97 is currently blocked before the
spike can be isolated by existing warnings in unchanged production targets; no
lint policy was weakened and those unrelated files were not edited.

Slice 0 therefore establishes the generated request signatures, capability
event shape, request ordering, and teardown semantics needed before production
configuration or lifecycle work begins.

## Review

A fresh read-only review approved Slice 0 with no blockers or fixes worth doing
now. The reviewer confirmed that the dependency edge, Foot behavior, generated
request/event signatures, protocol teardown semantics, and manager-only runtime
probe are accurate and remain isolated from production configuration and
lifecycle code.

Residual risk: Hyprland's product version and commit are environmental evidence
reported separately by `hyprctl`; the protocol probe itself reports only the
advertised interface version and capability flags.
