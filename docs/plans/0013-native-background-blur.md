# Plan 0013: native Wayland background blur

- **Status:** Draft — implementation not started
- **Release decision:** Do not advertise native blur until protocol, fallback, graphical, and review gates pass
- **Behavioral authority:** Foot 1.27.0 commit `3c5b584b0eafa772eb4376fb6eaf6643399e190e`
- **Protocol authority:** `ext-background-effect-v1`, version 1, from `wayland-protocols` staging
- **Initial compositor target:** Hyprland 0.56.1 or newer
- **Related decisions:** [ADR 0004](../adr/0004-font-and-cpu-renderer.md) and [configuration guide](../configuration.md)

## Decision

Add opt-in native background blur to Splinterm through
`ext-background-effect-v1`. Blur remains presentation state owned entirely by
the disposable graphical client. It must not alter daemon terminal state, the
private Splinterm wire protocol, renderer pixel semantics, PTY behavior, or the
pinned opaque Foot oracle.

The active Omarchy theme owns the default blur choice through its `foot.ini`.
A user may explicitly override that choice in Splinterm's `[colors]` section.
Splinterm requests blur only when all three conditions hold:

1. resolved `blur` is enabled;
2. resolved default-background alpha is below `1.0`; and
3. the compositor advertises the protocol's blur capability.

If any condition is false, Splinterm must own no background-effect object for
the surface. This preserves compositor fallback behavior and avoids Foot 1.27's
problematic `blur=no` pattern, where an empty effect object can suppress the
compositor's ordinary transparent-window blur path.

## Why this is a separate plan

The existing alpha slice intentionally implemented only Foot's default alpha
mode. At that time the installed Hyprland release did not implement
`ext-background-effect-v1`, and alpha needed no new Wayland object lifecycle.
Native blur does. It introduces a staging protocol global, asynchronous
capability changes, double-buffered surface state, resize-sensitive regions,
live theme transitions, and compositor-specific graphical evidence.

This plan does not expand into `alpha-mode=matching` or `alpha-mode=all`.
Those modes change which terminal cells are translucent and require their own
renderer/oracle work. Native blur here operates behind the already-supported
default-alpha pixels.

## User-visible contract

### Configuration

Support one new optional key:

```ini
[colors]
blur=yes
```

Accepted values are the existing strict booleans. Invalid values fail startup
with a line-numbered diagnostic.

Precedence is:

1. explicit `${XDG_CONFIG_HOME}/splinterm/config.ini` `[colors] blur`;
2. generated `theme.json` blur imported from the active theme's `foot.ini`;
3. `no` when neither source specifies a value.

Splinterm's user INI preserves the parser's existing last-assignment-wins
behavior: a later `[colors] blur` overrides an earlier one, just as a later
`[colors] alpha` does. Tests must state that result rather than vaguely testing
"duplicates."

The Omarchy bridge selects one Foot section deterministically: prefer
`[colors-dark]`; otherwise use legacy `[colors]`; do not import
`[colors-light]` because Splinterm currently has no light-theme selection
state. Within the selected section, the last assignment to a key wins. Alpha
and blur must be parsed from that same selected section; they may not be mixed
from different sections. An absent alpha defaults to `1.0`, and absent blur
defaults to `no`. Generation stays atomic, and a theme switch must update both
values in one validated `theme.json` replacement.

### Runtime behavior

| Resolved alpha | Resolved blur | Protocol/capability | Required behavior |
| --- | --- | --- | --- |
| `1.0` | any | any | no effect object; opaque rendering unchanged |
| `<1.0` | `no` | any | no effect object; ordinary compositor policy may apply |
| `<1.0` | `yes` | missing | transparent rendering continues; one bounded diagnostic; no startup failure |
| `<1.0` | `yes` | present, no blur capability | same graceful fallback as missing capability |
| `<1.0` | `yes` | blur capability present | create one effect object, set the full logical surface region, commit |

Blur algorithm, radius, passes, noise, brightness, vibrancy, and performance
remain compositor policy. Splinterm exposes no fake client-side blur strength.

## Protocol architecture

### Dependency boundary

Add a direct workspace dependency on `wayland-protocols` matching the resolved
0.32 series with `client` and `staging` features. Do not depend on its current
transitive presence through Smithay Client Toolkit. Keep first-party unsafe code
forbidden.

Expected files:

- `Cargo.toml`
- `crates/splinterm/Cargo.toml`
- `Cargo.lock`

Use the generated client types under
`wayland_protocols::ext::background_effect::v1` rather than vendoring XML or
generating project-local bindings.

### Client-owned state

Add a small project-owned background-effect controller, preferably isolated
from the large Wayland application reducer. It must separate pure desired-state
decisions from Wayland proxy side effects.

Required conceptual state:

- requested blur from resolved theme/config;
- current default-background alpha;
- manager availability;
- last advertised capability bits;
- current logical surface size;
- effect lifecycle: `Absent`, `Active`, or `DestroyPending`;
- whether a surface commit is required; and
- bounded one-shot diagnostics already emitted.

There may be at most one `ext_background_effect_surface_v1` object for the main
`wl_surface`. Splinterm uses server-side decorations, so no CSD or popup surface
is included in this plan. One multi-pane window still owns one Wayland surface
and one blur region.

### Global and capability handling

Bind manager version 1 opportunistically during Wayland startup. The global is
not required for the window to open.

Handle every `capabilities` event rather than treating the first event as
immutable:

- capability gained: reconcile desired state and lazily create the effect;
- capability lost: remove the effect on a surface commit and continue with
  ordinary transparency;
- unknown capability bits: preserve forward compatibility and ignore them;
- manager absent: retain desired state so a future architecture can support
  registry hotplug without changing configuration semantics.

No panic, protocol error, busy loop, or repeated log spam is acceptable when the
manager or capability is absent.

### Effect lifecycle and commit ordering

The protocol state is double-buffered. Reconciliation must happen before the
surface commit that presents the corresponding state.

Enabling:

1. prove `alpha < 1.0`, `blur=yes`, and blur capability;
2. create exactly one effect object;
3. create a temporary `wl_region` for the complete logical surface;
4. set the blur region, then destroy the temporary region;
5. commit the surface even when no terminal pixels changed.

Disabling or losing capability:

1. destroy the effect proxy; setting `set_blur_region(NULL)` alone is not
   sufficient because it leaves the surface-associated effect object alive;
2. commit the surface so protocol-defined effect removal takes effect;
3. return to `Absent` without leaving an empty effect object alive;
4. do not recreate an effect in the same transition before removal has been
   committed, or the compositor may raise `background_effect_exists`.

Repeated reconciliation with unchanged state must emit no request and schedule
no commit. Surface destruction must release the effect before or with the rest
of the Wayland proxies without sending requests to an inert surface.

### Blur region

Use a finite region covering the current **logical surface size**, not
`INT32_MAX`. The region is expressed in surface-local coordinates and must not
use SHM buffer dimensions. This avoids overflow-sensitive compositor paths and
keeps integer/fractional output scaling independent from effect geometry.

Update the region before the commit that applies any logical resize. Required
cases include initial configure, ordinary resize, maximization/fullscreen,
output-scale changes that alter logical size, and multi-pane layout changes that
resize the window. Zero or overflowing dimensions must fail safely before a
protocol request.

Do not request per-cell or damage-shaped blur regions. Opaque explicit cell
backgrounds naturally hide blur, while a stable whole-surface region avoids
protocol traffic on terminal damage.

## Theme and live-reload integration

Extend the strict generated theme schema with a boolean blur value. Keep the
field backward compatible through a default of `false`, but make malformed
present values fail validation.

A live theme transition must reconcile protocol state in the graphical event
loop:

- `no -> yes` with translucent alpha creates and commits the effect;
- `yes -> no` removes and commits it;
- alpha `1.0 -> <1.0` activates a requested effect;
- alpha `<1.0 -> 1.0` removes it;
- changing colors without changing effective blur eligibility emits no protocol
  request;
- a rejected theme file leaves the previous alpha, blur, and protocol state
  intact.

The theme watcher must communicate desired state to the Wayland owner; it must
not manipulate Wayland proxies from a background thread.

Expected files:

- `crates/splinterm/src/config.rs`
- `crates/splinterm/src/main.rs`
- `crates/splinterm/src/wayland.rs`
- optional new `crates/splinterm/src/background_effect.rs`
- `tools/generate-omarchy-theme.py`
- generator/config/Wayland tests

## Dependency-ordered implementation slices

### Slice 0 — freeze authorities and add a protocol spike

Before production edits:

- record the exact Foot blur request sequence for `alpha<1`, `blur=yes`;
- record Foot behavior for opaque alpha and `blur=no`;
- verify the generated Rust manager, capability, surface, and region APIs;
- confirm Hyprland 0.56.1 advertises version 1 and blur capability;
- write a minimal disposable client or compile-only harness proving generated
  bindings and dispatch signatures without modifying production lifecycle.

**Gate:** the spike establishes request ordering, event shape, and teardown
semantics. No production config key is accepted yet.

### Slice 1 — configuration and theme schema

Implement strict parsing, precedence, generator import, backward-compatible JSON
defaulting, and live theme model propagation. Do not bind the Wayland protocol
in this slice.

Tests must cover:

- `yes`, `no`, invalid, and absent values;
- explicit duplicate user keys using the last assignment;
- explicit override versus generated theme;
- `[colors-dark]` preferred over legacy `[colors]`, `[colors]` fallback when no
  dark section exists, and `[colors-light]` ignored;
- last assignment within the selected Foot section;
- alpha and blur imported atomically from the same selected section;
- malformed startup theme using the safe fallback in both single- and
  multi-pane launch paths with one bounded diagnostic;
- malformed live reload preserving the previous resolved theme with one bounded
  diagnostic; and
- themes without `blur` remaining disabled.

**Gate:** all config/generator tests pass and documentation can describe the
value without claiming the compositor effect works yet.

### Slice 2 — pure effect-state reducer

Implement and exhaustively test a side-effect-free reducer that turns requested
blur, alpha, capability, object state, and logical size into bounded actions such
as create, set finite region, remove, commit, or no-op.

Tests must cover:

- every row of the runtime behavior table;
- repeated no-op reconciliation;
- capability gain/loss/re-gain;
- live alpha and blur toggles in both orders;
- resize while active and resize while inactive;
- disable followed by immediate re-enable without duplicate objects;
- zero, negative, and overflow-sensitive geometry rejection;
- surface destruction in every lifecycle state.

**Gate:** no Wayland proxy is needed to prove lifecycle and ordering decisions.

### Slice 3 — Wayland binding and object lifecycle

Wire the manager, capability dispatch, temporary `wl_region`, effect proxy, and
surface commits into the production client. Preserve all existing fractional
scale, viewport, SHM, input, IME, frame-callback, and close behavior.

Add bounded diagnostic tracing suitable for tests. It may report manager,
capability, lifecycle transition, region dimensions, and commit reason, but no
terminal body or clipboard content.

**Gate:** focused library tests, strict Clippy, and a no-compositor build pass.
A protocol-disabled or capability-disabled launch remains behaviorally identical
to current clean HEAD.

### Slice 4 — live theme reconciliation and resize correctness

Connect accepted theme updates and logical resize events to the controller.
Ensure protocol-only commits neither repaint the terminal nor create a frame
callback loop. Coalesce a blur-region change with an already-required surface
commit when possible.

Tests must prove:

- no extra SHM allocation or terminal snapshot rebuild for a blur-only toggle;
- no idle wakeup after state settles;
- one region update per accepted logical-size change;
- rejected theme reload preserves active effect state;
- multi-pane windows retain one effect object and whole-window region;
- close during a pending toggle cleans up deterministically.

**Gate:** non-graphical lifecycle and existing renderer/Wayland tests pass with
no change to opaque final-buffer hashes.

### Slice 5 — guarded graphical smoke

This slice requires explicit user approval under the repository graphical-test
guardrails.

Before requesting approval, perform a non-graphical feasibility check for a
Hyprland-supported compositor-output capture that can observe an inactive
workspace without activating it, focusing its window, or violating DP-2
placement. If no such mechanism exists, record that boundary and keep automated
acceptance protocol-based; do not switch the user to workspace 8 merely to
obtain a screenshot.

On inactive workspace 8 / DP-2 only:

1. install temporary pre-map placement and no-focus rules before launch;
2. snapshot workspace, monitor, focus, pointer, rule, and process state;
3. launch one exact release candidate with `alpha<1` and `blur=yes`;
4. require placement on workspace 8 / DP-2 without focus;
5. capture bounded client protocol diagnostics proving manager bind, blur
   capability, one effect creation, a finite region, and commit;
6. collect compositor-visible evidence only if the pre-smoke feasibility check
   proved a guardrail-compliant inactive-workspace capture path;
7. prove cleanup leaves no window, process, temporary rule, workspace, monitor,
   or focus residue;
8. abort before any matrix on placement, focus, protocol, render, or cleanup
   failure.

Because compositor blur is not present in the client SHM buffer, a client-only
framebuffer capture is not visual evidence. When inactive-workspace compositor
capture is unavailable, the automated gate is the exact protocol lifecycle plus
placement/cleanup evidence. Visual quality is then a separate user-observed
manual check in an ordinary user-launched Splinterm window; the agent must not
manufacture visual evidence by violating workspace isolation.

**Gate:** the exact native-blur protocol lifecycle passes for one case, and any
claimed compositor-visible result has a guardrail-compliant evidence path.

### Slice 6 — approved graphical differential matrix

Run only after Slice 5 succeeds, under the same single graphical approval:

- Splinterm translucent + blur disabled: no effect object;
- Splinterm translucent + blur enabled: one active finite region;
- Splinterm opaque + blur enabled: no effect object;
- live `no -> yes -> no` without reopening the window;
- live alpha `1.0 -> translucent -> 1.0` with blur requested;
- ordinary resize while active;
- fractional-scale lane already supported by the test monitor workflow;
- multi-pane window using exactly one effect object;
- Foot 1.27 translucent + blur enabled as protocol and visual reference.

For each case retain exact binary/config identities, Wayland request summaries,
Hyprland version, monitor geometry, placement/focus checks, and cleanup status.
Retain screenshots or compositor captures only when Slice 5 proved a
compliant inactive-workspace capture path. Otherwise record the protocol matrix
and a separate user manual visual note without presenting it as automated pixel
evidence.

A rotated-output lane is not implicitly authorized. If needed, request separate
approval for one bounded DP-2 transform/restore sequence. Never test on DP-3,
move a test window there, or use the user's active portrait workspace.

**Gate:** every matrix case matches the runtime behavior table, no stale effect
survives disable/opaque transitions, and Foot/Splinterm visual behavior is
consistent within their documented alpha-mode difference.

### Slice 7 — review, documentation, and release decision

After all implementation and evidence gates:

- update `docs/configuration.md` and the sample config;
- amend ADR 0004 to move native blur from unsupported to opt-in supported;
- document compositor capability fallback and staging-protocol status;
- record exact commands and evidence paths in a new spike/evidence note;
- run one fresh read-only protocol/lifecycle review and one bounded final review
  if fixes are required;
- do not mark this plan complete until review and recorded evidence exist.

## Non-graphical validation ladder

After the relevant slices:

```bash
python -m pytest -q tools/benchmark/test_benchmark.py -k omarchy_theme_generator
cargo test -p splinterm --lib
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -- --test-threads=1
git diff --check
```

Add additional focused generator cases to the existing benchmark test module or
move that focused coverage to a clearly named dedicated module in one deliberate
change; do not leave overlapping harnesses.

Add focused tests for the background-effect reducer and Wayland dispatch. The
full workspace run is a closure gate, not a substitute for focused failures.

## Performance and resource gates

Native blur must not:

- allocate additional SHM buffers or backing framebuffers;
- repaint terminal pixels solely because capability state changes;
- add a polling thread, timer, or idle wakeup;
- create more than one manager binding or effect object per window;
- send region updates for terminal damage;
- retain temporary `wl_region` objects after `set_blur_region`;
- increase opaque/blur-disabled startup RSS beyond measurement noise; or
- regress the accepted graphical idle and resize behavior.

Compositor GPU cost is external policy and must be reported rather than hidden.
If the feature causes unacceptable Hyprland-specific GPU or visual regressions,
disable it by configuration and preserve graceful fallback; do not implement a
client-side blur shader in this plan.

## Failure and fallback policy

- Missing global or capability: continue transparently and diagnose once.
- Opaque alpha: do not create an effect and do not warn.
- Invalid user config: fail startup through existing strict INI rules.
- Missing or malformed generated startup theme: use the existing safe fallback
  palette in both single- and multi-pane paths and emit one bounded diagnostic.
- Malformed live theme reload: preserve the last accepted alpha, blur, palette,
  and effect state and emit one bounded diagnostic.
- Wayland protocol error or compositor disconnect: follow the existing clean
  client-termination boundary; never affect daemon-owned shells.
- Resize geometry error: reject the region transition without sending malformed
  dimensions; preserve the last valid state or terminate cleanly if surface
  correctness cannot be proven.
- Graphical isolation or cleanup failure: abort the sequence and block the
  matrix.

## Anti-shortcut rules

Do not:

- treat transparent ARGB output alone as native-blur support;
- parse `blur=yes` before the protocol path exists and silently ignore it;
- keep an empty effect object alive when blur is disabled;
- use `INT32_MAX` without documenting and reviewing the overflow tradeoff;
- express regions in physical SHM pixels;
- add Hyprland window rules as a substitute for the client protocol;
- implement `alpha-mode=matching/all` inside this scope;
- weaken or regenerate the pinned Foot final-buffer references;
- claim rotated-monitor validation from a landscape DP-2 test;
- run graphical tests before focused tests, diff inspection, and explicit
  approval; or
- call the plan complete without recorded validation and independent review.

## Completion criteria

This plan is complete only when all of the following are true:

- direct staging protocol bindings compile with first-party unsafe code still
  forbidden;
- strict config, theme precedence, and live reload are documented and tested;
- lazy object ownership preserves ordinary compositor fallback when inactive;
- capability gain/loss, resize, toggle, close, and compositor-disconnect cleanup
  boundaries are deterministic;
- opaque and blur-disabled output remains byte-identical to clean HEAD;
- the guarded smoke and approved matrix pass on workspace 8 / DP-2;
- Foot differential protocol evidence is retained;
- resource/idle gates pass;
- documentation and ADR accurately describe support and limitations;
- `git diff --check`, formatting, strict Clippy, and the serial workspace suite
  pass; and
- fresh review records no unresolved blocker.
