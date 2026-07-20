# Slice 10 Omarchy/Hyprland sign-off

Guarded release evidence captured on 2026-07-20 on inactive workspace 8 / DP-2.
The lifecycle run used commit `0bd981e`; the exact oracle summaries include that
implementation plus no subsequent renderer change.

## Accepted evidence

- `lifecycle.json`: exact end-to-end result from
  `tools/run-phase10-signoff.py`.
  - isolated `xdg-terminal-exec` selected `com.oldjobobo.splinterm.desktop`,
    preserved direct argv, and translated `--dir=/tmp` correctly;
  - app ID, title, workspace, monitor, and no-focus placement passed;
  - selection spanned 1,052 stable row IDs and copied 4,215 bytes without
    recording terminal content;
  - selection survived detached output, local wheel moved three history rows,
    and SGR application wheel emitted one 10-byte report;
  - the client cache reached its 4,096-row bound at 14,053,376 bytes;
  - 1×/1.25×/1.5×/2× output-scale captures and targeted resize/reflow passed;
  - clear, alternate screen, forced resync recovery, detach/output/reattach,
    daemon-owned child continuity, and leak-free shutdown passed;
  - screenshot hashes are recorded; the synthetic-content contact sheet was
    reviewed. DP-2 was restored to scale 1 and workspace 8 was empty.
- `final-buffer.json`: 16/16 pinned-Foot final-buffer cases byte-exact.
- `source-first.json`: representative source-first cases are exact at
  1×/1.25×/1.5×/2× with zero mismatch pixels and zero channel delta.

Slice 9 remains the accepted numeric CPU/memory/pacing budget. The Slice 10
lifecycle run additionally records client RSS/CPU before and after the full
interaction scenario and content-free resync counts.

## Reproduce

```bash
python tools/run-phase10-signoff.py /tmp/splinterm-slice10
python tools/foot-oracle/run-final-buffer-comparison.py /tmp/splinterm-slice10-final-buffer
python tools/foot-oracle/run-slice3-final-buffer-comparison.py /tmp/splinterm-slice10-source-first \
  --case underline-single-default \
  --case cursor-beam-reverse \
  --case underline-curly-rgb \
  --case underline-dotted
```

The graphical commands refuse to launch unless workspace 8 is assigned to
DP-2, inactive, and empty. They use silent pre-map placement and no initial
focus. The lifecycle runner restores monitor scale and terminates its isolated
window, child, and daemon in `finally`.

## Exploratory anomaly

`source-first-exploratory.json` records an additional non-closure run:
`underline-double-indexed` at 1.25× was not exact, while the accepted 1×, 1.5×,
and 2× cases passed. That fixture was not in the reviewed Slice 3 closure set;
Slice 10 uses the reviewed exact `cursor-beam-reverse` case for its 1.25× lane.
No reference, tolerance, or image was modified to hide this result.
