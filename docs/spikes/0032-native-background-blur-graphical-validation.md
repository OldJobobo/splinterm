# Spike 0032: native background blur graphical validation

- **Status:** Complete
- **Date:** 2026-07-29
- **Plan:** [Native Wayland background blur](../plans/0013-native-background-blur.md)
- **Scope:** Plan 0013 Slices 5–6
- **Evidence:** [`artifacts/0032-native-background-blur/`](artifacts/0032-native-background-blur/)

## Guarded environment

The user approved one guarded smoke and its conditional matrix. Testing used the
repository's established Hyprland isolation boundary:

- inactive workspace `8` on unfocused monitor `DP-2`;
- pre-map `workspace = '8 silent'`, floating, size, opacity, and permanent
  `no_initial_focus`/`no_focus` spawn properties through `hl.exec_cmd`;
- a private `splinterd` socket, configuration tree, state tree, and process
  group under `/tmp`;
- the exact release binary built from `d2affafcacd4df820e41df20971c673e99f6e46b`;
- active workspace and focused-window address checks before, during, and after
  every case; and
- `grim -o DP-2`, which captures the compositor output without activating the
  reserved workspace.

The release client SHA-256 was
`4e64dfea4bc1712356ad722198cc4bfdeeb581c22df4f3d8dcab4ab1520d16c7`.
Hyprland was 0.56.1 commit
`5c9377c15f85c50648f35ca5a213754f95b93ca0`. DP-2 began and ended at scale
1.0, transform 0, displaying empty workspace 8 while unfocused. No rotated lane
was run.

The exact one-shot harnesses are retained under `harness/`. They are evidence,
not reusable test tools: paths and dates are intentionally frozen, and the
matrix harness contains the recorded Foot-directory collision described below.

## Slice 5 smoke

The smoke used `alpha=0.75` and `blur=true`. It passed all abort conditions:

- the window mapped to workspace 8 on DP-2 and did not take focus;
- manager version 1 bound and advertised `capabilities=0x1 blur=true`;
- Splinterm created one effect object;
- the initial `664x504` logical region and configured `960x600` logical region
  were finite;
- enable and resize surface commits were observed; and
- window, private daemon, workspace, monitor, and focus cleanup passed.

`smoke-dp2.png` is a compositor-output capture, not a client SHM capture. It
visibly contains the blurred wallpaper behind translucent default-background
pixels. `smoke-summary.json` retains binary, compositor, placement, trace, and
capture identities.

Executed command:

```bash
python /tmp/run-native-blur-smoke.py
```

The exact script is retained as
`harness/smoke-runner.executed.py`.

## Slice 6 matrix

The eight Splinterm cases matched the runtime table:

| Case | Result |
| --- | --- |
| translucent, blur off | manager/capability observed; no effect object |
| translucent, blur on | one create, finite regions, enable/resize commits |
| opaque, blur requested | manager/capability observed; no effect object |
| live blur `no -> yes -> no` | one create/enable, then one destroy/disable |
| live alpha `1.0 -> 0.75 -> 1.0` | one create/enable, then one destroy/disable |
| resize while active | one object; region advanced to finite `1100x700` |
| fractional scale 1.25 | one object; logical region remained finite; DP-2 restored to 1.0 |
| two-pane window | one window effect object, not one per pane |

Each case passed guarded placement, focus preservation, and cleanup before the
next case. Exact per-case traces and capture hashes are retained in
`matrix-summary.json` and `matrix-protocol-traces.log`.

The one-shot matrix harness then aborted before launching Foot because it
created `foot-reference/` for the Foot config and the generic launcher correctly
refused to reuse that directory. This was a harness path collision, not a
product, protocol, placement, focus, compositor, or cleanup failure. The
harness `finally` path left workspace 8 empty, stopped the private daemon,
restored DP-2 to scale 1.0, and preserved the guarded focus baseline. The user
then approved one bounded Foot-only completion attempt; no Splinterm case was
repeated.

Executed commands:

```bash
python /tmp/run-native-blur-matrix.py
python /tmp/run-native-blur-foot-only.py
```

Their exact scripts are retained as `harness/matrix-runner.executed.py` and
`harness/foot-reference-runner.executed.py`.

## Foot differential

The reference used installed Foot 1.27.0 with blur support and source authority
commit `3c5b584b0eafa772eb4376fb6eaf6643399e190e`. Its binary SHA-256 was
`79ce907b8c00ab1dec254d5b954365c7212f00d67ce54e876799c8706da1d4e5`.
With `alpha=0.75` and `blur=yes`, `WAYLAND_DEBUG=client` recorded:

1. manager version 1 bind;
2. `capabilities(1)`;
3. one `get_background_effect` request;
4. one blur region assignment; and
5. a following main-surface commit.

Foot uses an `INT32_MAX` region in this reference. Splinterm intentionally does
not copy that geometry: its reviewed contract uses the finite logical surface
size. `foot-reference-dp2.png` is a valid compositor-output capture and is
visually consistent with the Splinterm smoke within the documented renderer and
alpha-mode differences. The Foot window remained isolated and cleanup passed.

## Capture limitation

The inactive-output capture mechanism itself is valid, as proved by the smoke
and separately synchronized Foot run. Several rapid matrix captures were taken
after the protocol condition but before the compositor displayed the current
terminal frame; visual inspection showed the preceding case's marker. Those
files are **not** retained as visual acceptance evidence and are not described
as differential screenshots. Their source hashes remain in
`matrix-summary.json` only to make the limitation auditable.

The matrix acceptance authority is therefore the exact bounded protocol traces,
placement/focus assertions, and cleanup checks. Visual compositor evidence is
limited to the independently valid Splinterm smoke and Foot reference captures.
No client-only framebuffer is presented as proof of blur.

## Resource and idle closure

The user separately approved an opaque, blur-disabled RC-versus-pre-feature
resource sequence. The authority was commit `1e233a1` immediately before Plan
0013 implementation. Both versions used fresh private daemons, identical opaque
theme/configuration inputs, the same `960x600` placement, a one-second settle,
and two seconds of process CPU/RSS sampling. No screenshot, transform, or DP-3
lane was used.

Two one-shot harness setup failures were retained rather than hidden:

1. attempt 1's RC smoke passed, but the isolated pre-feature build omitted the
   sibling `splinterm-pty-child`, so the first comparison client could not spawn
   its shell; and
2. attempt 2 included the helper but used a Unix socket path longer than
   `SUN_LEN`, so its daemon rejected startup before any window launch.

Both attempts cleaned up without workspace, monitor, focus, or process residue.
Before attempt 3, a headless check proved the pre-feature daemon, short socket,
PTY helper, child spawn, and exit path. Attempt 3 then passed one guarded
pre-feature smoke and five matched pre-feature/RC pairs:

| Metric | Pre-feature | RC |
| --- | ---: | ---: |
| median RSS | 25,206,784 bytes | 25,268,224 bytes |
| RSS range | 196,608 bytes | 274,432 bytes |
| median idle CPU | 0 ticks | 0 ticks |
| maximum idle CPU | 1 tick | 1 tick |

The RC median RSS delta was 61,440 bytes, below the conservative 1,048,576-byte
measurement-noise floor. Every idle sample stayed below the existing five-tick
two-second limit. Workspace 8 ended empty, DP-2 remained scale 1.0/transform 0
and unfocused, and the focused window address was unchanged.

The raw summary's `active_workspace_unchanged=false` compares the complete
Hyprland workspace JSON, including mutable title/window metadata. Preflight and
postflight both identified workspace 1 on DP-1, the focus address remained
identical, and the continuous guard never observed workspace 8 or DP-2 active.
This is metadata drift, not a workspace/focus isolation failure.

Exact successful results are in `resource-idle/summary.json`; the two failed
setup attempts and diagnoses are retained under `resource-idle-attempt-1/` and
`resource-idle-attempt-2/`. The three exact one-shot harnesses are retained under
`harness/`.

## Non-graphical closure

After the graphical sequence, the following passed:

```bash
python -m pytest -q tools/benchmark/test_benchmark.py -k omarchy_theme_generator
cargo test -p splinterm --lib -- --test-threads=1
cargo fmt --all --check
cargo test --workspace -- --test-threads=1
git diff --check
```

The focused generator lane passed and all 185 Splinterm library tests passed,
including the final non-finite-alpha fix. The serial workspace unit,
integration, and doc-test suite passed before that isolated parser fix. A final
serial rerun reached the established unrelated
`phase8_detach_reattach_overflow_resync_and_cleanup` slow-subscriber timing
flake; Plan 0013 changes no `splinterd` source, and the exact failed test then
passed alone in 15.16 seconds. Both the failed rerun and isolated pass are
retained rather than hidden. Existing renderer tests continued to cover
unchanged opaque final-buffer and alpha pixel semantics.

The plan's exact strict-Clippy command did **not** pass on Rust 1.97:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

It stopped on pre-existing `replace_box` and `collapsible_match` diagnostics in
`splinterm-core` and `splinterm-terminal`. A narrower graphical-client scan also
reported the existing Rust 1.97 style baseline across renderer and Wayland code,
including `map_entry`, `collapsible_if`, `manual_is_multiple_of`, and
`struct_excessive_bools`. Review found no Plan 0013 diagnostic. The user
explicitly approved the plan's retained-baseline closure exception instead of a
broad unrelated lint refactor. No lint policy was weakened and no terminal/core
or broad renderer refactor was smuggled into the native-blur slice. Exact logs
and `validation/summary.json` are retained with the evidence.

## Outcome

Slices 5 and 6 pass. Native blur is active only for translucent default
backgrounds when requested and supported; inactive and opaque paths own no
empty effect object. Live removal, resize, fractional scale, multi-pane
ownership, Foot differential behavior, and graphical isolation matched Plan
0013. The staging protocol and compositor capability fallback remain documented
release limitations.

The resource/idle gate now passes, and the explicit retained-Clippy-baseline
policy decision is recorded without weakening lint configuration. Plan 0013
remains in progress only until the final independent closure review dispositions
this newly retained evidence. This note does not prematurely advertise the
feature as released or mark the plan complete.
