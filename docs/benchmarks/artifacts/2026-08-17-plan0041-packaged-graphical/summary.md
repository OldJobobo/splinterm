# Plan 0041 packaged graphical acceptance

## Candidate and isolation

- Maintenance commit: `d3cbbc6b013ff4cab28a8ec6ef6aaa267603ff35`
- Package: `splinterm-0.1.0alpha3.3-1-x86_64.pkg.tar.zst`
- Package SHA-256: `22f21317c17bbcfc510d99f6fdb9f3a593e7ab08736bcb8180527c8e2cac3c0b`
- Staged client SHA-256: `afb65d3c0598d13939933789075ed24f7f44b912e6b052e926113738b14d93c8`
- Staged daemon SHA-256: `62e68414de51d5ce0e4f75412d93d581557eb430fe850607c3bb9966c52ea416`
- Staged PTY child SHA-256: `e43de0a260dc37f2ba4680d525c3cdd3bf29ce72d694eb025ff4f42a2de3d7ca`

The package was extracted under `/tmp/splinterm-plan0041-provenance-v3/pkgroot`.
The adjacent staged client and daemon used private HOME, config, state, theme,
and `SPLINTERM_SOCKET` paths. The real `XDG_RUNTIME_DIR` was retained only for
Wayland compositor access. `/usr/bin` and the live Omarchy theme were not
modified.

`source-package-provenance.txt` records the exact source archive hash and its
embedded Git archive commit ID `450f6cbb`; that package-build source commit and
its squash-merged maintenance commit `d3cbbc6b` have the identical tree
`48d6418c`. The retained `.BUILDINFO` PKGBUILD hash matches the PKGBUILD from
that tree. `package-member-sha256.txt` hashes the three tested executables
directly from the package archive and matches both the staged-file and
live-process hashes. Retained package metadata, archive manifest, PKGBUILD, and
build script complete the upstream package/source link.

## Live packaged-runtime provenance

`runtime-identity.json` was captured while both processes and the socket were
live. It records:

- daemon PID `3939236`, exact staged executable path, device/inode, SHA-256, and
  private socket environment;
- graphical client PID `3939254`, window `0x55d2ce8b9ff0`, exact staged
  executable path, device/inode, SHA-256, and the same private socket;
- private socket device/inode and mode; and
- the shared staged config and state roots.

The raw `daemon.*`, `client.*`, `candidate.*`, and `process-sockets.json` files
retain `/proc` executable, environment, FD, inode, and hash evidence.
`sockets-selection-live.txt` ties both private listeners directly to
`splinterd` PID `3939236` and its FDs:

```text
/tmp/splinterm-plan0041-provenance-v3/splinterd.sock         users:(("splinterd",pid=3939236,fd=9))
/tmp/splinterm-plan0041-provenance-v3/splinterd.sock.content users:(("splinterd",pid=3939236,fd=10))
```

The window JSON ties graphical PID `3939254` and address `0x55d2ce8b9ff0` to
all theme and selection captures. These records close the provenance gap in the
first graphical attempt, whose runtime paths had been deleted before they were
committed.

## Bounded graphical sequence

The client mapped on workspace 8 / DP-2 with no initial focus. Launch-time
opaque/no-blur window properties made configured source RGB values directly
measurable. The exact staged address was focused once to activate rendering and
the recorded original focus was immediately restored. No automated pointer or
keyboard input was used during the follow-up acceptance sequence.

Theme A defined standard `lighter_bg = "#f4c95d"`, Foot background
`#101820`, foreground `#f2f4f8`, selection foreground `#00ff88`, selection
background `#335577`, and accent `#ff4fd8`.

Theme B omitted `lighter_bg` and defined Foot background `#111111`, foreground
`#fafafa`, bright0 `#e8e8e8`, and accent `#00c8ff`. The exact staged config,
Theme A, and Theme B inputs are retained under `inputs/` and covered by
`SHA256SUMS`.

## Results

- **Theme A standard roles:** active-tab body rendered exact `#f4c95d` from
  `lighter_bg`; label and close used the higher-contrast dark endpoint;
  underline rendered exact `#ff4fd8`.
- **Theme B fallback:** with no `lighter_bg`, active-tab body rendered exact
  `#e8e8e8` from Foot `bright0`; label and close used the dark `#111111`
  endpoint; underline rendered exact `#00c8ff`.
- **Live reload:** switching back to Theme A restored exact `#f4c95d` and
  `#ff4fd8` on client PID `3939254` and the same window without restart.
- **Selection independence:** the user selected a visible row on the uniquely
  identified staged window. The captured row rendered exact `#335577`
  background and `#00ff88` foreground while active-tab chrome remained exact
  `#f4c95d` / `#ff4fd8`.

## Cleanup

- Exact staged window closed; staged client and daemon stopped.
- Private socket, extracted package root, config, state, theme, and logs removed.
- Workspace 8 contained zero windows.
- Original Foot address, PID, workspace, monitor, position, size, and focus were
  restored exactly.
- After the user's manual selection, a separately approved pointer-only cleanup
  was run. A relative ydotool attempt was accelerated across the multi-monitor
  layout and overshot without clicking; it was stopped. One diagnosed native
  absolute cursor move then restored the exact recorded `(908, 621)` position.
  No click or keyboard input was sent during cleanup.
- `pacman -Qkk splinterm`: `56 total files, 0 altered files` before and after.

`cleanup-comparison.json` records the exact before/after focus, geometry, and
pointer assertions.

## Evidence

- exact-pixel screenshots: `01-theme-a.png`, `02-theme-b-bright0.png`,
  `03-theme-a-live-reload.png`, `04-theme-a-selection.png`
- exact staged config and native Theme A/B inputs under `inputs/`
- package/source provenance, package metadata and archive manifest, retained
  PKGBUILD/build script, and direct package-member hashes
- live runtime identity: `runtime-identity.json`, raw client/daemon `/proc`
  records, candidate hashes/stats, per-process socket maps, and `ss` captures
- stable theme/reload/selection window identity JSON
- pre-test and post-cleanup focus, cursor, client inventory, and comparison JSON
- before/after Pacman integrity records
- `SHA256SUMS` covering the complete retained evidence bundle
