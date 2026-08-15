# Plan 0041: Beta 1 active-tab foreground contrast

- **Status:** Implementation, non-graphical validation, and review complete; packaged graphical acceptance pending for `0.1.0-beta.1`
- **Date:** 2026-08-14
- **Product authority:** Active Dojo-tab text has its own optional theme role and
  never borrows terminal selection text color implicitly
- **Depends on:** native Omarchy theme discovery, strict JSON theme resolution,
  and the accepted exact-color selected-tab rendering path

## Decision

Add an optional `active_tab_foreground` role to Splinterm's native Omarchy and
JSON theme inputs. Active Dojo-tab labels and close affordances use the resolved
role. Terminal selection keeps using `selection_foreground`; changing tab text
must not change selected terminal glyphs.

When `active_tab_foreground` is absent, choose whichever of the resolved theme
`background` or `foreground` has the higher WCAG contrast ratio against the
resolved `active_tab_background`. Prefer `foreground` on an exact tie. This is a
deterministic compatibility fallback, not a promise to synthesize a new color
or force every legacy palette above a minimum ratio.

An explicit `active_tab_foreground` remains an exact theme-owned RGB value. It is
validated but not contrast-corrected, blended, or replaced. An invalid explicit
value rejects the candidate theme atomically instead of silently using the
fallback.

For Dispatch, the intended native Omarchy roles are:

```toml
active_tab_background = "#e6c93a"
active_tab_foreground = "#141d23"
```

The active tab therefore uses dark `#141d23` text on yellow `#e6c93a` at
approximately 10.39:1 contrast, while Foot's tan `selection-foreground` remains
unchanged for terminal selection.

## Confirmed baseline defect

Before this patch, the native path resolved `active_tab_background` from
`colors.toml` but resolved `selection_foreground` from Foot. `tab_foreground()`
then used `selection_foreground` for active tabs. Dispatch consequently renders
`#b69f80` text on `#e6c93a`, approximately 1.55:1 contrast.

An explicit `~/.config/splinterm/theme.json` does not affect the running client
unless `main.theme` selects it. With `main.theme` unset, native Omarchy discovery
is authoritative, so changing only that generated JSON is not a runtime fix.
Changing Dispatch's Foot selection foreground would couple unrelated terminal
selection and application-chrome semantics and is outside this patch.

## Behavior contract

1. Extend `ResolvedTheme` with a complete `active_tab_foreground` value and give
   the bundled default theme an explicit readable value.
2. Extend strict JSON `ThemePalette` with optional `active_tab_foreground`.
   Resolve an explicit valid `#RRGGBB` exactly; otherwise compute the fallback
   only after `background` and `active_tab_background` are resolved.
3. In native Omarchy discovery, read optional `active_tab_foreground` only from
   `colors.toml`. Keep `selection-foreground` sourced from effective Foot
   `[colors-dark]` or legacy `[colors]` exactly as before.
4. Resolve `active_tab_background` first, retaining its existing fallback to the
   terminal selection background. Then resolve the missing foreground from the
   theme background/foreground endpoint with higher contrast against that final
   active-tab background.
5. Use the WCAG sRGB relative-luminance and contrast-ratio calculation. Compare
   ratios without rounding; documentation-only displayed ratios may be rounded.
6. Prefer normal theme `foreground` on an exact contrast tie. Do not use
   `selection_foreground`, `ui_accent`, ANSI colors, hard-coded black/white, or a
   generated intermediate color as the fallback.
7. `tab_foreground()` uses `active_tab_foreground` only for active tab labels and
   their close affordances. Inactive tabs retain normal `foreground`; terminal
   selection rendering retains `selection_foreground`.
8. Preserve exact active-tab background RGB, configured alpha, and opaque accent
   underline behavior. This patch does not alter compositing or attempt to infer
   the compositor background behind translucent tabs.
9. Live theme reload remains atomic: a malformed new role retains the last valid
   complete `ResolvedTheme` through the existing watcher boundary.
10. The optional Omarchy exporter emits `active_tab_foreground`. It preserves an
    explicit source role and applies the same contrast algorithm to the
    `background` and `foreground` roles in the JSON palette it emits. Native
    discovery instead compares its effective Foot background and foreground.
    Exact cross-path color equality is required only when those resolved
    endpoints and the active-tab background are equal; this patch does not
    redefine the exporter's existing color-source ownership.

## Implementation milestones

### Milestone 1 — theme role and deterministic fallback

Expected files:

- `crates/splinterm/src/config.rs`
- `config/splinterm/theme.json`

Work:

- add the optional JSON field and complete resolved field;
- add a small pure sRGB luminance/contrast chooser at theme resolution;
- resolve the role in both JSON and native Omarchy paths;
- keep explicit values exact and malformed values fail-closed; and
- update the bundled JSON theme so its schema example is complete.

Focused tests must prove:

- explicit native and JSON roles survive exactly;
- missing roles choose dark background against a bright selected-tab body and
  light foreground against a dark selected-tab body;
- an exact tie chooses foreground;
- fallback uses the final selection-derived tab background when
  `active_tab_background` is absent;
- malformed explicit roles fail rather than falling back;
- Dispatch resolves active tab foreground to `0x141d23` while preserving its
  independent Foot selection foreground; and
- default/live-reload paths still carry one complete atomic theme.

### Milestone 2 — renderer separation and exporter parity

Expected files:

- `crates/splinterm/src/wayland/tabs.rs`
- `tools/generate-omarchy-theme.py`
- `tools/benchmark/test_benchmark.py`

Work:

- route active tab text and close-glyph painting through the new resolved role;
- leave inactive tabs and terminal selection untouched;
- teach the optional Omarchy exporter to preserve or derive the new role; and
- keep Rust and Python fallback semantics aligned with shared fixed fixtures.

Focused tests must prove:

- active tabs use `active_tab_foreground` even when
  `selection_foreground` is deliberately different;
- inactive tabs still use normal foreground;
- active-tab background alpha and accent underline bytes remain unchanged;
- exporter output preserves an explicit role;
- exporter output derives both dark-on-light and light-on-dark fallbacks from
  the background and foreground roles emitted into that JSON;
- native discovery derives from deliberately different effective Foot endpoints
  when sibling `colors.toml` values differ; and
- native and generated JSON results match when their resolved endpoint fixtures
  and active-tab background match.

### Milestone 3 — documentation, review, and release boundary

Expected files:

- `docs/configuration.md`
- `TODO.md`
- release-state files only in a later dedicated Beta 1 release commit

Work:

- update the existing optional-role and fallback paragraph in
  `docs/configuration.md` to document native and JSON endpoint ownership,
  fail-closed explicit-role validation, fallback order, and the separation from
  Foot selection colors;
- record focused and serial non-graphical validation;
- inspect the actual diff and obtain one fresh read-only correctness review;
- prepare the `0.1.0-beta.1` version/package/provenance changes only after the
  implementation branch is accepted; and
- keep candidate construction, publication, and AUR distribution behind their
  existing separate approval boundaries.

## Validation

Run after each coherent implementation milestone as applicable:

```bash
cargo test -p splinterm config::tests
cargo test -p splinterm wayland::tabs::tests
python -m pytest tools/benchmark/test_benchmark.py -k omarchy_theme_generator
cargo fmt --all --check
cargo clippy -p splinterm --all-targets -- -D warnings
git diff --check
```

Before release integration, run the repository's complete serialized workspace,
release-tooling, package, portable Foot-provenance, and documentation boundaries.
Record any known unrelated flaky failure plus its exact isolated rerun rather
than weakening or silently skipping the boundary.

## Packaged graphical acceptance

After separate approval under the repository graphical-testing rules and after
an approved adjacent packaged client installation:

1. select the Dispatch Omarchy theme in one isolated Splinterm test Window;
2. prove the active tab label and close affordance use dark `#141d23` while the
   active tab body remains exact `#e6c93a` and the underline remains the UI
   accent;
3. create a terminal selection and prove its foreground remains Dispatch's Foot
   selection foreground rather than the tab foreground;
4. switch to a theme without `active_tab_foreground` and prove the active tab
   remains readable through the deterministic fallback;
5. switch back and prove live native discovery updates the complete palette
   without restart; and
6. restore the original theme, focus, workspace, monitor, Window geometry,
   package state, and test topology.

Abort on wrong-window input, unrelated theme/config mutation, loss of exact
selected-tab background behavior, selection-color regression, or incomplete
cleanup.

## Beta 1 acceptance

The patch is complete only when:

- native Omarchy and strict JSON themes accept optional
  `active_tab_foreground`;
- absent roles use the documented deterministic higher-contrast endpoint;
- active tab chrome no longer consumes `selection_foreground`;
- Dispatch's exact tab pair resolves to `#141d23` on `#e6c93a` without changing
  terminal selection colors;
- focused and serial validation plus fresh read-only review are recorded;
- separately approved packaged graphical acceptance is recorded; and
- the Beta 1 candidate, promotion, publication, and AUR states are recorded
  only after their separate authorizations.

This plan does not authorize implementation, installation, graphical testing,
pushing, candidate dispatch, promotion approval, AUR publication, or release
publication.
