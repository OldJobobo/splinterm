# Plan 0041: Beta 1 active-tab foreground contrast

- **Status:** Implementation, complete non-graphical validation, fresh review,
  package validation, and packaged graphical acceptance accepted for
  `0.1.0-beta.1`
- **Date:** 2026-08-14
- **Product authority:** Active Dojo-tab chrome derives from standard Omarchy
  and Foot roles without requiring app-specific additions to `colors.toml`; its
  body and text never borrow terminal selection colors implicitly
- **Depends on:** native Omarchy theme discovery, strict JSON theme resolution,
  and the accepted exact-color selected-tab rendering path

## Decision

Active Dojo-tab labels and close affordances use a resolved foreground that is
independent from terminal `selection_foreground`; changing tab text must not
change selected terminal glyphs.

Native Omarchy discovery derives the tab body only from standard theme roles:
`lighter_bg`, then effective Foot `bright0`. It derives tab text by choosing
whichever effective Foot `background` or `foreground` has the higher WCAG
contrast ratio against that resolved body, preferring `foreground` on an exact
tie. Native themes require no Splinterm-specific `colors.toml` keys.

Strict JSON themes remain a Splinterm-owned portable format. They may explicitly
set `active_tab_background` and `active_tab_foreground`; absent values use ANSI
color 8 and the same deterministic contrast chooser. Explicit JSON values remain
exact and malformed values reject the candidate palette atomically.

## Confirmed baseline defect

Before this patch, the native path accepted a Splinterm-specific
`active_tab_background` from `colors.toml` and otherwise borrowed Foot selection
background. `tab_foreground()` then used Foot `selection_foreground` for active
tabs. This coupled application chrome to terminal selection roles and encouraged
app-specific additions to Omarchy themes.

An explicit `~/.config/splinterm/theme.json` does not affect the running client
unless `main.theme` selects it. With `main.theme` unset, native Omarchy discovery
is authoritative, so changing only that generated JSON is not a runtime fix.
Changing Dispatch's Foot selection foreground would couple unrelated terminal
selection and application-chrome semantics and is outside this patch.

## Behavior contract

1. Extend `ResolvedTheme` with a complete `active_tab_foreground` value and give
   the bundled default theme an explicit readable value.
2. Extend strict JSON `ThemePalette` with optional `active_tab_foreground`.
   Resolve explicit valid JSON overrides exactly; otherwise compute the fallback
   only after `background` and `active_tab_background` are resolved.
3. In native Omarchy discovery, keep `selection-foreground` sourced from
   effective Foot `[colors-dark]` or legacy `[colors]` exactly as before. Do not
   read Splinterm-specific active-tab roles from `colors.toml`.
4. Resolve native `active_tab_background` from standard `lighter_bg`, then
   effective Foot `bright0`. Resolve strict JSON from an explicit override, then
   ANSI color 8. Never borrow terminal selection. Derive the native foreground,
   or a missing JSON foreground, from the higher-contrast background/foreground
   endpoint against that final active-tab body.
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
10. The optional Omarchy exporter derives both active-tab JSON roles from
    standard Omarchy roles and ignores app-specific source keys. It applies the
    same contrast algorithm to the `background` and `foreground` roles in the
    JSON palette it emits. Native discovery instead compares its effective Foot
    background and foreground. Exact cross-path color equality is required only
    when those resolved endpoints and the active-tab background are equal.

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

- explicit strict JSON roles survive exactly;
- native themes use `lighter_bg`, then Foot `bright0`, without app-specific keys;
- missing JSON roles choose dark background against a bright selected-tab body
  and light foreground against a dark selected-tab body;
- an exact tie chooses foreground;
- foreground fallback uses the final background-ramp-derived tab body;
- malformed explicit JSON roles fail rather than falling back;
- native active-tab text is derived independently from Foot selection
  foreground; and
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
- missing active-tab backgrounds use the normal background ramp even when the
  terminal selection background is deliberately different;
- inactive tabs still use normal foreground;
- active-tab background alpha and accent underline bytes remain unchanged;
- exporter output ignores app-specific Omarchy source keys;
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

The reviewed implementation boundary at `9316a5f` passed the focused config,
tab-renderer, and exporter tests; the complete serialized workspace; all-target
warnings-denied Clippy; all 63 benchmark tests; release/package/automation
unittests; portable Foot provenance; automation contract fixtures; formatting;
and `git diff --check`. Fresh read-only review `4c6bdf91` returned **CLEAN**.

## Packaged graphical acceptance

After separate approval under the repository graphical-testing rules, use an
adjacent staged packaged client and daemon on an explicit private
`SPLINTERM_SOCKET`; do not replace the Pacman-owned installation:

1. select an Omarchy theme in one isolated Splinterm test Window;
2. prove the active tab body uses that theme's standard `lighter_bg` and the
   label and close affordance use the derived higher-contrast endpoint while the
   underline remains the UI accent;
3. create a terminal selection and prove its foreground remains Foot's selection
   foreground rather than the tab foreground;
4. switch to a theme without `lighter_bg` and prove Foot `bright0` supplies the
   body fallback;
5. switch back and prove live native discovery updates the complete palette
   without restart; and
6. restore the original theme, focus, workspace, monitor, Window geometry,
   package state, and test topology.

Abort on wrong-window input, unrelated theme/config mutation, loss of exact
selected-tab background behavior, selection-color regression, or incomplete
cleanup.

Acceptance completed at maintenance commit `d3cbbc6` with package SHA-256
`22f21317c17bbcfc510d99f6fdb9f3a593e7ab08736bcb8180527c8e2cac3c0b`.
The accepted follow-up used the real Wayland runtime only for compositor access
and an explicit private Splinterm socket, config, state, theme, and process
hierarchy. Contemporaneous evidence ties the graphical client PID, adjacent
staged client and daemon executable paths, hashes, device/inode identities,
environments, and daemon-owned private sockets together while live. Exact
rendered pixels proved native `lighter_bg`, Foot `bright0` fallback,
higher-contrast text, accent underline, independent Foot selection colors, and
same-process live reload. The user selected the terminal row on the uniquely
identified staged window; the agent captured it without automated pointer or
keyboard input. Cleanup left workspace 8 empty, removed every owned process and
temporary file, restored the recorded focus, window geometry, and pointer
exactly, and ended with `pacman -Qkk splinterm` reporting 56 files and zero
alterations. Evidence is recorded under
`docs/benchmarks/artifacts/2026-08-17-plan0041-packaged-graphical/`.

## Beta 1 acceptance

The patch is complete only when:

- native Omarchy uses no Splinterm-specific `colors.toml` roles;
- strict JSON themes accept optional active-tab overrides;
- absent JSON roles and native inputs use the documented deterministic
  fallbacks;
- active tab chrome no longer consumes `selection_foreground`;
- missing active-tab backgrounds no longer consume terminal selection color;
- native active-tab chrome remains independent from terminal selection colors;
- focused and serial validation plus fresh read-only review are recorded;
- separately approved packaged graphical acceptance is recorded; and
- the Beta 1 candidate, promotion, publication, and AUR states are recorded
  only after their separate authorizations.

This plan does not authorize implementation, installation, graphical testing,
pushing, candidate dispatch, promotion approval, AUR publication, or release
publication.
