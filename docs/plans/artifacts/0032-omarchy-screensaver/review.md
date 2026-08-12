# Plan 0032 source review

A fresh read-only review inspected Milestones 1–4 after the full non-graphical
validation boundary.

The review confirmed that:

- the app-ID override is confined to hidden local `xdg-launch`, validated by
  Clap before daemon access, and passed unchanged to Wayland;
- ordinary, picker, consent, native, and remote Window paths retain
  `com.oldjobobo.splinterm`;
- no app ID enters config, protocol, topology, persistence, automation, or daemon
  state;
- commandless XDG launches remain persistent, while command-bearing launches
  remain owner-bound transient Lairs and are not added to Recent Dojos;
- desktop metadata and the adapter preserve app ID, cwd, empty arguments, and
  shell metacharacters as separate argv entries;
- the owned profile has the required 18-point Nerd Font, zero padding, opaque
  background, and disabled blur;
- packaging claims only `/usr/share/splinterm`, and installer handling of a
  user-local Omarchy launcher is report-only; and
- the retained Omarchy patch uses the current argv-safe helper and per-monitor
  event wait without adding a package dependency.

The review initially blocked acceptance because extracted-package validation
asserted point size, padding, opacity, and blur but omitted the exact Nerd Font
name. The validation now also requires:

```text
JetBrains Mono Nerd Font:style=Regular
```

The reviewer also noted that `SHA256SUMS` contained an absolute workstation
path. It now records the portable relative patch filename. Both fixes passed
focused validation.

Clean package extraction, upstream submission, and guarded graphical acceptance
remain explicitly open.
