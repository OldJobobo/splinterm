# Splinterm 0.1.0 RC2 — Font Reload Closure

This is the second release candidate for Splinterm 0.1.0. It retains the complete
RC1 feature line and closes two live-font correctness and resource findings found
during RC1 review.

RC2 is intended for normal daily use and soak testing on the documented target:
x86_64 Omarchy/Arch Linux with native Wayland under Hyprland. Any later code
change requires another release candidate before the final `v0.1.0` release.

## Live-font closure fixes

- A staged generation that differs from its preceding probe now becomes the
  watcher authority, so a later return to the probed generation is not skipped.
- Cached FreeType faces now retain shared immutable font mappings instead of
  copying the complete font file for every generation, face, and raster size.
- Font-generation identity and lifetime tests resolve the host's generic
  monospace family instead of requiring JetBrains Mono to be installed.

## Live Omarchy font following

When `main.font` is unset, a valid change to Fontconfig's effective `monospace`
family now replaces one complete immutable renderer generation without
restarting the Window, daemon, shell, or applications. Explicit font patterns
remain authoritative, and an invalid live generation retains the last valid
family.

Font changes preserve configured size and sizing policy, padding, DPI, runtime
zoom, topology, focus, history, modal and IME state, and controller authority.
Observer panes never acquire control solely to resize after a font change, and
deferred font-driven resizes retry after transient command-queue backpressure.
Fontconfig named instances in variable fonts are preserved through shaping,
metrics, and rasterization. Ambient Fontconfig checks run at a bounded ten-second
cadence.

## Yazi uses bounded Sixel previews

When Sixel is enabled, Splinterm now advertises primary device attribute `4`.
Yazi therefore selects its Sixel image path instead of the incompatible legacy
per-cell Kitty placement path. The capability remains conditional, and the
existing image-content and 256-placement bounds are unchanged.

## Legacy Dojo names normalize on restore

Loading schema-v2, schema-v3, or schema-v4 metadata now replaces only the exact
historical generated forms `terminal`, `terminal-<timestamp>`, and
`terminal-<timestamp>-<pid>` with collision-free `Dojo N` names. Numeric fields
may be zero-padded, matching names emitted by older builds. Explicit names in
current schema metadata are preserved.

## New Splints follow the current Gum palette

`splinterd` is persistent, so environment variables inherited when the daemon
started can outlive an Omarchy theme change. Before RC1, a newly created shell
could therefore receive stale Gum colors even though Splinterm's graphical
palette already reflected the active theme.

For every new Splint, the daemon now reads the active rendered palette from:

```text
${XDG_STATE_HOME:-$HOME/.local/state}/omarchy/current/theme/gum_env.lua
```

It refreshes only Omarchy's bounded Gum environment namespace:

- `GUM_*`;
- `FOREGROUND` and `BACKGROUND`; and
- `BORDER_FOREGROUND` and `BORDER_BACKGROUND`.

A complete valid palette replaces current managed values and removes obsolete
managed `GUM_*` entries. Unrelated environment variables remain untouched.
Existing PTYs keep the environment with which they were created; only later
Splints see a later valid theme.

Missing, malformed, oversized, symlinked, or non-regular palette state fails
closed. In those cases the new PTY preserves the daemon's inherited environment
rather than receiving a partial palette.

## Stabilized Beta 3 baseline

RC1 retains the Beta 3 workload and interface corrections:

- terminal workloads inherit the systemd user manager's task policy while
  `splinterd.service` keeps its independent `TasksMax=2048` guard;
- the New Dojo control sits after the final visible tab;
- inactive tabs use semantic dividers; and
- the strict historical unnamed initial Dojo form is presented as `Dojo 1`
  without mutating daemon-owned topology.

Persistent topology, explicit restore, scrollback and search, terminal images,
remote graphical access, JSON/NDJSON automation, MCP, configurable terminal
lifetime, presets, and the documented native Wayland path remain part of the
0.1 baseline.

## Upgrade boundary

Splinterm 0.1 does not support live daemon upgrade handoff. Upgrading RC1 to
RC2 therefore ends active Dojos. Run the upgrade from Foot or another terminal
that is not owned by `splinterd`:

```bash
systemctl --user stop splinterd.service
# upgrade the splinterm package here
systemctl --user daemon-reload
systemctl --user start splinterd.service
```

Then reopen Splinterm Windows. Package installation does not silently reload or
restart the user service.

## RC2 soak focus

During the release-candidate soak, please pay particular attention to:

- repeated valid, invalid, and rapidly superseded Omarchy font changes;
- stable file-descriptor and memory use across repeated font and scale changes;
- repeated Omarchy theme changes followed by newly created Splints;
- existing PTYs retaining their original environment;
- clean installation and Beta 3 upgrade/rollback;
- saved-Lair restore and ordinary long-running terminal workloads;
- trusted graphical-client identity and desktop launching; and
- optional MCP package behavior when installed.

RC2 remains a prerelease. If stabilization requires any code change, the next
public build will be RC3 rather than the final `v0.1.0` release.
