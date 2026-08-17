# PR #7 active-divider graphical acceptance

This artifact records guarded graphical acceptance for commit
`7813220e757535d8a3bad18717d8122a4c1fd5e6` on workspace 8 / DP-2.

## Accepted matrix

A private development `splinterd` used an isolated socket, configuration, and
state directory. Six line-divider layouts produced sixteen deterministic
`888×642` captures:

- two left/right panes with each pane focused;
- two top/bottom panes with each pane focused;
- three-pane `├`, `┤`, `┬`, and `┴` layouts with each leaf focused.

Every capture contained both the configured inactive `#445566` and active
`#ff00aa` divider colors. Visual inspection of the contact sheets confirmed:

- left and right focus select the upper and lower halves of a fully shared
  vertical divider;
- top and bottom focus select the left and right halves of a fully shared
  horizontal divider;
- nested panes activate only the adjoining separator span;
- all four tee orientations retain inactive connectivity while independently
  overlaying the active arms; and
- no visible gap or whole-junction promotion occurs at the intersections.

`summary.json` retains per-capture exact-color counts. The source PPM files were
kept out of the repository; the two lossless contact sheets retain every tested
focus/layout state.

## Isolation and cleanup

Each mapped window was required to match the exact release-profile client next
to the private daemon before capture. Windows mapped with `no_initial_focus` and
`no_focus` on workspace 8 / DP-2. The matrix reported unchanged active
workspace, active window, and pointer state. Cleanup removed every owned window,
shell, daemon, and socket. Workspace 8 was empty afterward, and
`pacman -Qkk splinterm` reported `56 total files, 0 altered files`.

## Diagnostic attempts excluded from acceptance

The repository's older mixed line/frame smoke was attempted first and cleaned
up after every run. Its first attempt omitted the adjacent release-profile
`splinterm-pty-child`; its second used the now-obsolete assumption that a private
`theme.json` is selected without an explicit `theme=` setting. After those
fixture issues were corrected, its line capture contained 444 inactive and
1,230 active pixels, but its unrelated frame-edge assertion rejected three
configured-color edge pixels. PR #7 does not change frame rendering, so that
legacy mixed-style assertion is not acceptance evidence. The focused line-only
matrix above is the acceptance authority.

## Files

- `two-pane-contact.png` — four two-pane focus states.
- `junction-contact.png` — twelve focus states across all tee orientations.
- `summary.json` — bounded matrix result and exact-color counts.
- `SHA256SUMS` — hashes for the retained evidence files.
