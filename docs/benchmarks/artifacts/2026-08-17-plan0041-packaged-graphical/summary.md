# Plan 0041 packaged graphical acceptance

## Candidate

- Maintenance commit: `d3cbbc6b013ff4cab28a8ec6ef6aaa267603ff35`
- Package: `splinterm-0.1.0alpha3.3-1-x86_64.pkg.tar.zst`
- Package SHA-256: `22f21317c17bbcfc510d99f6fdb9f3a593e7ab08736bcb8180527c8e2cac3c0b`
- Staged client SHA-256: `afb65d3c0598d13939933789075ed24f7f44b912e6b052e926113738b14d93c8`
- Staged daemon SHA-256: `62e68414de51d5ce0e4f75412d93d581557eb430fe850607c3bb9966c52ea416`
- Staged PTY child SHA-256: `e43de0a260dc37f2ba4680d525c3cdd3bf29ce72d694eb025ff4f42a2de3d7ca`

The package was extracted to a temporary root. The staged adjacent client and
daemon used an explicit private `SPLINTERM_SOCKET`, HOME, config, state, and
theme tree. The real `XDG_RUNTIME_DIR` was retained only for Wayland compositor
access. No packaged executable or live Omarchy theme was modified.

## Bounded graphical sequence

The client mapped as a uniquely identified staged executable on workspace 8 /
DP-2 with no initial focus. Its address and PID remained stable through both
native theme switches. Launch-time opaque/no-blur window rules made configured
source RGB values directly measurable without changing Splinterm compositing.

Theme A defined standard `lighter_bg = "#f4c95d"`, Foot background
`#101820`, foreground `#f2f4f8`, bright0 `#5a6570`, selection foreground
`#00ff88`, selection background `#335577`, and accent `#ff4fd8`.

Theme B omitted `lighter_bg` and defined Foot background `#111111`, foreground
`#fafafa`, bright0 `#e8e8e8`, selection foreground `#ff8a00`, selection
background `#375a7f`, and accent `#00c8ff`.

## Results

- **Theme A standard roles:** active-tab body rendered exact `#f4c95d` from
  `lighter_bg`; label and close affordance used the higher-contrast dark
  endpoint; underline rendered exact `#ff4fd8`.
- **Theme B fallback:** with no `lighter_bg`, the body rendered exact `#e8e8e8`
  from Foot `bright0`; label and close used the dark `#111111` endpoint;
  underline rendered exact `#00c8ff`.
- **Live reload:** switching back to Theme A restored exact `#f4c95d` and
  `#ff4fd8` on the same client PID and window address without restart.
- **Selection independence:** the selected terminal row rendered exact
  `#335577` background and `#00ff88` foreground while active-tab chrome
  remained exact `#f4c95d` / `#ff4fd8`.

The first automated focus request was guarded before input because the smoke
window had been launched permanently non-focusable; focus and pointer were
restored before relaunching a focusable no-initial-focus target. Two bounded
automated drags then crossed blank terminal space because the row position was
miscalibrated. Both reached only the exact staged target and were followed by
state restoration. The user selected the visible second row on that same staged
window; the agent performed no further input and captured the conclusive exact
selection evidence in `04-theme-a-selection.png`.

## Cleanup

- Exact staged window closed; staged client, daemon, and owned mouse helper
  stopped.
- Private socket, extracted package root, config, state, theme, and logs removed.
- Workspace 8 contained zero windows.
- Original Foot address `0x55d2cdf21a40`, PID, size, workspace 6, and monitor
  2 were restored. Hyprland reported its y position as `522` after cleanup
  versus `518` before, a disclosed 4 px residual. The test did not intentionally
  move this unrelated tiled/scrolling window, and it was not repositioned after
  the bounded authorization ended.
- Pointer changed only from recorded `(3280, 1119)` to `(3280, 1118)`, within
  one pixel, as recorded in the before/after cursor files.
- `pacman -Qkk splinterm`: `56 total files, 0 altered files` before and after.

## Evidence

- `01-theme-a-live-reload.png`
- `02-theme-b-bright0.png`
- `04-theme-a-selection.png`
- `candidate.sha256`
- window identity JSON for the focusable target, both reload states, and
  selection capture
- pre-test and post-cleanup focus and cursor records
- post-cleanup clients and before/after Pacman integrity records
