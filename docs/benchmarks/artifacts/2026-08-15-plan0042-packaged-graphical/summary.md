# Plan 0042 packaged graphical acceptance

## Decision

**The post-fix packaged graphical acceptance is accepted; integration remains
pending.** The exact packaged
candidate passed the guarded 1440p wide-grid, right-edge, split-layout,
legacy-ceiling crossing, real remote endpoint, capped endpoint, residual
hit-testing, and actual Mozc IME cases. No safe 4K output was available, so the
plan-authorized non-graphical 4K proof remains the recorded 4K boundary.

This evidence does not authorize or claim merge, installation, promotion,
distribution, or Beta1 release.

## Provenance

### Initial candidate

- Source commit: `dc8e1165968f66c17dd872bf6153b8eb1681650a`
- Package SHA-256:
  `edde0c446aaf842034970aa312a1306a0a6d0d2f8626d2c6d973b834ca1ca2a2`
- Packaged `splinterm` SHA-256:
  `9e114026843acb4cb6cb6b818e95a6f8bb4c5d0d55efa41814a1a9e273ee9304`
- Packaged `splinterd` SHA-256:
  `da212f39741691b59228c19fcced291faabfde5a3780215bf0b0dfb00b3b4764`

### Post-fix candidate

- Source branch: `feat/0042-beta1-wide-splint-grid`
- Source commit: `6c03fb7f3365adef7b24b8afd5ffb460a0a2402a`
- Package: `splinterm-0.1.0alpha3.3-1-x86_64.pkg.tar.zst`
- Package SHA-256:
  `bd4199ff6bcb25386ab6de1b1f90fce16c5681bd450a5738eddf49823e529886`
- Packaged `splinterm` SHA-256:
  `7deb0d2c7cdeaf1c4acaf5e28ce91d5be977a520926b97f349effe83959649a0`
- Packaged `splinterd` SHA-256:
  `da212f39741691b59228c19fcced291faabfde5a3780215bf0b0dfb00b3b4764`
- `tools/package/build-local-package.sh --no-check` and
  `tools/package/validate-package.py` completed successfully. Package checks
  were skipped only because the complete focused Splinterm boundary had just
  passed; package structure and runtime validation still ran.
- Both candidates were extracted under `/tmp` and run through their adjacent
  packaged binaries. `/usr/bin/splinterm` and `/usr/bin/splinterd` were never
  replaced.

## Environment

- Hyprland workspace 3 on landscape DP-1
- DP-1: 2560x1440, scale 1, transform 0
- Shipped profile: JetBrains Mono Nerd Font, output-scale sizing, 14 configured
  pixels, and 12 px four-sided padding
- No safe 3840x2160 graphical output was available
- Fcitx5 with installed and configured Mozc supplied the actual IME case

## Initial candidate results

### Packaged smoke and exact 1440p fullscreen

The initial staged daemon accepted `ping` and reported an empty isolated runtime.
The staged client mapped exactly once. Its smoke snapshot reported `309x65`.
At an exact 2560x1440 fullscreen surface, the authoritative snapshot reported
`317x69`; `RIGHT_EDGE_OK` and the final-column marker rendered without stale
pixels, and the cursor returned visibly to a known row.

### Right-edge selection

An isolated `ydotoold` device selected through the right-edge marker after one
bounded device-calibration retry. Output, cursor, pointer targeting, and
selection reached the final natural column.

### Pane-local geometry

One vertical split produced two `317x33` PTYs. Splitting the second pane
horizontally produced `317x33`, `156x33`, and `156x33`. Every Splint supplied an
authoritative snapshot. Test siblings were terminated and closed afterward.

### Legacy-ceiling crossing

The exact Window crossed between authoritative 228- and 259-column grids eight
times. `PLAN0042_RESIZE_HISTORY_KEEP` remained in the final `259x47` snapshot.
The client stayed alive; no resync loop or stale right-edge region appeared.

### Real remote graphical endpoint

A second staged adjacent daemon and real packaged graphical relay ran under
isolated state. A bounded SSH wrapper replaced network transport with staged
`relay --graphical-stdio`. The remote endpoint negotiated `309x66` while the
local endpoint remained `259x47`, proving endpoint-local geometry without
cross-endpoint leakage at the default 480x128 limits.

## Capped endpoint defect and correction

The first temporary 120x64 fixture was incomplete and stopped before mapping.
A complete reusable `graphical-capped` fixture and a full authorization,
attachment, control, resize, and detach integration test were then added.

The complete fixture exposed a real product defect in the initial package: the
direct `launch --splint-id` path sent `309x66` despite the endpoint advertising
120x64. The multi-pane path already propagated negotiated limits, but
`run_live_window` fell back to compile-time 480x128 defaults. Commit `6c03fb7`
shares the endpoint-limit conversion and explicitly supplies the negotiated
limits to direct-window `WindowOptions`.

Post-fix validation records:

- 379 Splinterm library tests passed; one manual timing harness remained ignored;
- 101 Splinterm binary tests passed;
- all Splinterm integration tests passed, including 14 remote-session tests;
- all-target Splinterm Clippy passed with warnings denied;
- Python fixture syntax, formatting, and `git diff --check` passed; and
- the private Arch package built and validated successfully.

## Post-fix capped graphical acceptance

The exact packaged client from `6c03fb7` mapped on workspace 3 / DP-1. For a
2500x1362 logical pane it emitted exactly one bounded diagnostic:

```text
splinterm terminal grid capped at 120x64 for a 2500x1362 logical pane
```

The fixture recorded an endpoint-local resize of `120x64` and `960x1280` terminal
pixels. The terminal remained top-left anchored and the large trailing right
residual was visibly distinct from the real terminal rectangle.

### Residual hit-testing

One isolated virtual pointer remained alive for both controls. After compositor
admission, a gesture beginning on the first real `CAP_EDGE` cell and ending in
the residual selected exactly that one real cell. A second gesture ran wholly
inside the residual from global x=1120 to x=1274, beyond the grid ending near
x=1002. It neither created fictitious selection nor changed the retained real
cell selection. A 160x80 crop around the marker contained **0 changed pixels**
before and after the residual-only gesture. Neither gesture generated protocol
terminal input.

### Actual IME placement

Fcitx was switched from `keyboard-us` to installed Mozc only while the exact
packaged Window was freshly verified as focused. Typing bounded `nihongo` at
cursor column 119 displayed Mozc's candidate popup adjacent to the final real
cell and inside the grid boundary. Space committed UTF-8 `にほんご` through the
exact Splint input channel; no Enter was sent. Escape cancelled remaining IME
state, and Fcitx was restored to `keyboard-us`.

## 4K limitation

No safe 3840x2160 output existed. Plan 0042 explicitly permits retaining the
pinned non-graphical 4K proof while reporting this graphical limitation. The
absence of a safe 4K monitor is not represented as graphical 4K execution.

## Cleanup

Each graphical sequence used exact process and Window identities. Final cleanup:

- stopped every staged client, staged daemon, fixture process, relay, and
  isolated input daemon created by the matrix;
- left workspace 3 empty;
- restored the exact pre-test active Window, workspace, monitor, and cursor;
- restored Fcitx to `keyboard-us`;
- preserved unrelated user Windows and Splinterm daemons;
- restored DP-1/DP-2/DP-3 workspace assignments; and
- confirmed `splinterm: 56 total files, 0 altered files` through Pacman.

Fresh post-fix read-only review `ae030b0b` returned **CLEAN**, with no
blocker-level or fix-worth-doing-now issue in the code, fixture, tests, images,
or acceptance record.

Image hashes are recorded in `SHA256SUMS`. Images 01–08 record the initial
candidate matrix; images 09–13 record the post-fix capped and Mozc follow-up.
