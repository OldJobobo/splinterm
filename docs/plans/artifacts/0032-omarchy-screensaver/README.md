# Plan 0032 Omarchy patch preparation

This artifact contains the bounded upstream Omarchy launcher patch prepared for
Splinterm screensaver support. It has not been submitted, published, installed,
or applied to the live Omarchy package.

## Provenance

- Repository: `https://github.com/basecamp/omarchy.git`
- Base commit: `4727bad5ebd37cf2344416ae937b02a931113ec3`
- Changed upstream path: `bin/omarchy-launch-screensaver`
- Patch: `omarchy-launch-screensaver.patch`

The retained patch uses zero context so repository whitespace checks do not
misinterpret unified-diff blank context markers. Apply it to the pinned base
with:

```bash
git apply --unidiff-zero omarchy-launch-screensaver.patch
```

## Behavior

The patch:

- adds `com.oldjobobo.splinterm.desktop` to the existing terminal allowlist;
- updates the bounded unsupported-terminal notification;
- retains Omarchy's existing per-monitor event-socket wait; and
- uses the current argv-safe `hypr_exec` helper to invoke:

```bash
env SPLINTERM_CONFIG=/usr/share/splinterm/omarchy/screensaver.ini \
  xdg-terminal-exec \
  --app-id=org.omarchy.screensaver \
  -- omarchy-screensaver
```

It adds no package dependency and does not alter Omarchy-owned profiles.

## Validation

Run in a disposable shallow clone at the base commit:

```bash
bash -n bin/omarchy-launch-screensaver
./test/cli

git diff --check
```

All checks passed. Graphical acceptance remains separately approval-gated, and
upstream submission remains separately publication-gated.
