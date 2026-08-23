# Plan 0017 graphical closure evidence — 2026-08-09

## Build and non-graphical validation

The guarded sequence used the Pacman-verified installed sibling binaries against a fully isolated daemon, socket, state directory, and configuration:

- repository HEAD: `47d2e450ec21a8e07f4f9d316b2e37acfc58f028`;
- `/usr/bin/splinterm` SHA-256: `1c579a1ac16ccb91c47c6b996afb4c660037b7de1bbe79e7db80531878233cbf`;
- `/usr/bin/splinterd` SHA-256: `41bb5c7b0bcaeea73a900d7a7f9cefc8392ee884bab5d6c3bb3a74b397bf4`;
- Pacman integrity: `splinterm: 42 total files, 0 altered files`.

Current non-graphical validation passed:

- `cargo test -p splinterm --lib -- --test-threads=1`: 293 passed, 1 ignored manual benchmark;
- `cargo clippy --workspace --all-targets -- -D warnings`;
- `cargo fmt --all -- --check`;
- `git diff --check`.

The library suite includes picker state/catalog paging, responsive scale-deterministic layout, minimal pointer target size, Unicode/combining truncation, bounded caches and hit rectangles, theme contrast correction, painter sentinels, no terminal-snapshot identity for the inline host, exact application-owned shortcut handling, modal key/pointer ownership, stale clipboard/IME rejection, activation matching, visual-surface translation, and selection-only transient-row invalidation.

## Guarded environment and authority

The user approved one complete smoke and conditional matrix. Every native test Window was launched on workspace 8 / DP-2 with silent pre-map placement, floating geometry, full compositor opacity, and `no_initial_focus`. Before input, the harness required one freshly mapped exact `address` and PID on workspace 8 / monitor 1, then focused only that Window. The production daemon and historical topology were not used.

DP-2 began and ended at 1920×1080, scale 1.0, transform 0, workspace 8, unfocused. The Foot Window active at the start of each harness was restored after each sequence. Retained pre/post state pairs prove exact cursor equality for the smoke, first matrix attempt, pointer retry, and final keyboard run. Workspace 8 ended empty.

## Passed smoke and matrix

### Guarded smoke

`smoke-dark-opaque-normal-scale120.png` records the 960×600 scale-1.2 inline picker over one running session. It demonstrates the centered native panel, trusted scrim/frame, non-color selection rail/marker, complete normal-mode chrome, dark opaque theme, and one-session catalog. `Ctrl+Shift+S` opened the picker; Escape preserved the exact Window address/PID and restored the same frontend. Initial mapping did not change focus. `smoke-summary.json` records the exact identity and binary hash.

### Empty catalog

`empty-dark-opaque-normal-scale120.png` records the standalone Recent Sessions host with zero daemon sessions at dark/opaque scale 1.2. Escape closed only that picker and topology remained empty. The standalone host intentionally uses its documented synthetic presentation; the inline host's absence of terminal snapshot identity is proven by focused source tests and the native captures in every other case.

### Paged and long-Unicode catalog

`paged-unicode-light-translucent-compact-scale150.png` records eleven available sessions at light/translucent scale 1.5 in compact layout. The visible catalog includes `Plan0017–Unicode–測試–é`, safely clipped with an ellipsis, and paged entries. `keyboard-picker-light-translucent-compact-scale150.png` records the final corrected three-entry keyboard case at the same theme, scale, and layout.

While that picker was open, synthetic literal `vvvv`—which has no picker action—was sent and then Escape was pressed. A machine snapshot of the exact target Splint contained no `vvvv`, proving the modal keyboard input did not reach the PTY. Reopening and sending documented `j, j, Enter` selected an existing catalog session. `keyboard-existing-session-same-window.png` records the activated existing session. The exact Window address/PID survived and the isolated daemon Dojo count remained 3 before and after, proving this was a switch rather than New activation. See `keyboard-summary.json`.

### Minimal pointer behavior

`minimal-hover-dark-translucent-scale240.png` records the minimal layout at dark/translucent scale 2.4 after the exact Window was moved wholly inside DP-2's reduced logical bounds. A dedicated temporary `/dev/uinput` pointer moved over the selected action.

For cancellation, the harness pressed inside the selected picker action, moved onto empty workspace outside the Window, and released. The compositor's implicit pointer grab delivered the paired release to the picker; `minimal-press-release-cancel.png` records that the picker remained open with the same selected action. Escape then cancelled it without changing Window identity.

The picker was reopened and pointer activation selected New terminal. The same exact Window survived while the isolated daemon Dojo count changed exactly from 4 to 5. `pointer-new-same-window.png` records the new tab during shell startup; the colored blocks are the configured shell's startup presentation, not picker or backing-buffer evidence. See `pointer-summary.json`.

## Harness diagnostics retained honestly

The exact executed harnesses are under `harness/`. Aborted setup/measurement attempts were diagnosed before retry:

1. the first smoke preflight lacked Hyprland instance environment and performed no graphical action;
2. the first scale attempt used legacy `keyword monitor` under the Lua config provider, mapped no Window, and restored state;
3. a generic no-focus oracle rejected the explicitly approved exact focus before input;
4. permanent `no_focus` prevented the explicitly approved exact focus before input, so the final harness retained only `no_initial_focus`;
5. the first matrix pointer phase found the scale-2.4 Window partly outside reduced logical monitor bounds and aborted before pointer input; the retry moved the exact Window inside the monitor;
6. the first keyboard sentinel contained `N`, the documented New-terminal action. It correctly created a Dojo, after which remaining synthetic text reached the new terminal. The invalid sentinel claim was discarded. Final evidence uses ignored literal `vvvv` and documented `j` navigation with topology-count assertions.

Every aborted harness cleaned its exact Window, isolated daemon process, scale change, cursor position, and focus before the next attempt. The harness roots initially retained their private configuration/runtime/state files for artifact export. Final reviewer `c341bfbb` correctly rejected the broader cleanup claim because the executed harnesses did not themselves remove those roots or assert cursor equality.

The bounded closure fix copied all four pre-state records into this artifact, compared each against its corresponding post-state cursor, and found exact equality in every pair. It then removed only `/tmp/splinterm-plan0017-{smoke,matrix,retry,keyboard-final}` and their four top-level harness scripts. `final-cleanup.json` proves all eight temporary paths absent, workspace 8 empty, DP-2 scale 1.0/transform 0/unfocused, and records the final active Window. No graphical rerun was required because the finding concerned retained cleanup attestation rather than product behavior.

Reviewer `c341bfbb` found no product, source, input-isolation, same-Window, matrix-coverage, or visual blocker; its sole cleanup-evidence blocker is resolved by the retained pre/post pairs and `final-cleanup.json`.

## Acceptance mapping

1. Native centered panel: graphical smoke and all inline captures.
2. No inline terminal snapshot presentation: focused test plus native chrome captures.
3. Escape/newest frontend restoration: smoke identity assertion, corrected keyboard cancellation, and current modal reconciliation tests.
4. New and running-session same-Window switching: pointer Dojo increment and keyboard existing-session unchanged-count case.
5. No input leakage: complete modal-focused library tests plus the `vvvv` PTY snapshot probe and pointer paired-release case.
6. Keyboard/pointer agreement: keyboard existing-session activation and pointer New activation.
7. Normal/compact/minimal usability: smoke, keyboard, and pointer captures.
8. Non-color selection identity: selection rail and `›` marker in captures.
9. Theme-derived appearance: dark/light and opaque/translucent cases with no new theme keys.
10. Selection-only updates avoid terminal rebuilds: deterministic painter/cache/invalidation tests in the passing library suite.
11. Tests/format/lint/diff hygiene: current commands above all pass.
12. Required graphical evidence: this retained artifact.

Fresh independent closure review `c341bfbb` inspected all eight screenshots, source/tests, summaries, and harnesses. Its sole cleanup-evidence blocker was corrected and objectively revalidated as recorded above; no unresolved blocker remains.
