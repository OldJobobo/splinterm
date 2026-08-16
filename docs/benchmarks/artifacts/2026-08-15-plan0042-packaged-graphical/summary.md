# Plan 0042 packaged graphical partial acceptance

## Decision

**Partial acceptance only; Plan 0042 and Beta1 remain blocked.** The exact
committed packaged candidate passed the guarded 1440p smoke, wide-grid,
right-edge, split-layout, legacy-ceiling crossing, and real remote graphical
cases. The synthetic capped-endpoint case did not reach a mapped Window, no
non-keyboard IME was available for popup-placement proof, and no safe 4K output
was present. These gaps prevent Plan 0042 closure.

## Provenance

- Source branch: `feat/0042-beta1-wide-splint-grid`
- Source commit: `dc8e1165968f66c17dd872bf6153b8eb1681650a`
- Package: `splinterm-0.1.0alpha3.3-1-x86_64.pkg.tar.zst`
- Package SHA-256:
  `edde0c446aaf842034970aa312a1306a0a6d0d2f8626d2c6d973b834ca1ca2a2`
- Packaged `splinterm` SHA-256:
  `9e114026843acb4cb6cb6b818e95a6f8bb4c5d0d55efa41814a1a9e273ee9304`
- Packaged `splinterd` SHA-256:
  `da212f39741691b59228c19fcced291faabfde5a3780215bf0b0dfb00b3b4764`
- Package builder and `tools/package/validate-package.py` completed
  successfully. Package checks were skipped during compilation because the
  branch's complete non-graphical validation and review were already recorded;
  package runtime validation still ran.
- The package was extracted under `/tmp` and its adjacent packaged client and
  daemon were used directly. `/usr/bin/splinterm` and `/usr/bin/splinterd` were
  not replaced.

## Environment

- Hyprland workspace `3`, statically assigned to landscape `DP-1`
- `DP-1`: 2560x1440, scale 1, transform 0
- `DP-2`: 1920x1080; unsuitable for the mandatory 1440p case
- `DP-3`: 2560x1440 with portrait transform; not used
- No 4K monitor or safe 4K compositor path was available
- Shipped profile: JetBrains Mono Nerd Font, output-scale sizing, 14 configured
  pixels, and 12 px four-sided padding
- Original focus: Foot PID `3298740`, address `0x55d2ce9c2ea0`, workspace 1,
  DP-1

## Passed cases

### Packaged smoke

The staged adjacent daemon accepted `ping` and reported an empty isolated
runtime before launch. The staged client mapped exactly once on workspace 3,
DP-1, and accepted the bounded marker command `PLAN0042_SMOKE_OK`. The first
authoritative snapshot reported `309x65`, already beyond the legacy 240-column
ceiling.

### Exact 1440p fullscreen and right edge

The exact test Window was set to a 2560x1440 fullscreen surface. Its
authoritative snapshot reported `317x69`. `RIGHT_EDGE_OK` rendered at the final
right-edge cells and `Z` rendered in the last column without stale right-edge
pixels. The cursor returned to a known row and remained visible.

A temporary isolated `ydotoold` socket sent the selection press, relative
pointer motion, and release while the exact test Window was freshly verified as
focused. Selection visibly reached the right-edge marker. The first attempt,
which mixed Hyprland cursor movement with virtual-device button events, produced
no selection and was discarded; the bounded same-device retry succeeded.

### Pane-local geometry

One vertical split produced two independent `317x33` PTYs. Splitting the second
pane horizontally produced grids of `317x33`, `156x33`, and `156x33`.
Authoritative snapshots were taken for every Splint. Test siblings were then
terminated and their exited leaves closed, returning to one running Splint.

### Legacy 240-column crossing

The exact Window was floated and resized eight times between 1850 and 2100
logical pixels, crossing from an authoritative 228-column grid to 259 columns
and back. It ended at `259x47`. `PLAN0042_RESIZE_HISTORY_KEEP` remained in the
final authoritative snapshot. The staged client remained alive, no resync loop
was observed, and the final screenshot showed no stale right-edge region.

### Real remote graphical endpoint

A second staged adjacent daemon and real graphical relay were run under isolated
state. Splinterm's generated SSH argv was recorded; a bounded wrapper replaced
network transport with the staged `splinterm relay --graphical-stdio` connected
to that daemon. The remote packaged client mapped exactly once on workspace 3
and negotiated an authoritative `309x66` grid. The local endpoint remained at
`259x47`, demonstrating endpoint-local geometry with no dimension leakage.

## Open and rejected cases

### Synthetic capped endpoint

A temporary copy of the repository's `fake_ssh.py` fixture advertised limits of
`120x64` and one synthetic running Splint. The first launch stopped after
`inspect_splint` because the fixture lacked that response. One bounded fixture
correction added the inspection response. The retry progressed through
`list_lairs`, `inspect_splint`, and `request_access`, then stopped at the next
unsupported fixture request, `authorization_status`, before mapping a Window.

No production binary failed, no test Window mapped, and no existing Window
received input. The stop-loss prohibited continuing to grow an ad hoc endpoint
during acceptance. The capped diagnostic and residual pointer hit-testing
therefore remain unproven graphically.

### IME

Fcitx5 was running, but its only active input method was `keyboard-us`. The
right-edge terminal cursor and pointer selection were proven, but no actual IME
candidate popup or preedit placement was available. IME placement remains open.

### 4K

No safe 3840x2160 graphical output was available. Plan 0042's pinned
non-graphical 4K fixture remains the only 4K evidence and must be reported as a
graphical limitation.

## Cleanup

- Stopped staged local client PID `948303` and daemon PID `902063`.
- Stopped staged remote client PID `997783` and daemon PID `997761`.
- Confirmed the failed capped client was stopped.
- Confirmed workspace 3 contained zero remaining clients.
- Restored workspace 1 on DP-1, exact Foot focus, and cursor position.
- Preserved unrelated packaged daemon PID `2433077` and Plan 0041 development
  daemon PID `3968722`.
- Restored monitor workspace assignments exactly: DP-1/workspace 1,
  DP-2/workspace 8, and DP-3/workspace 6.
- Pacman reported `56 total files, 0 altered files` for `splinterm`.

Image hashes are recorded in `SHA256SUMS`. This record does not authorize or
claim merge, installation, candidate promotion, package publication, or Beta1
release.
