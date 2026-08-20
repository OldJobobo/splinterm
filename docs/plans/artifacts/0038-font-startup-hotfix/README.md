# Plan 0038 startup font hotfix

## Problem

The graphical client resolved its configured regular face but hardcoded
JetBrains Mono patterns for bold, italic, and bold-italic. When Omarchy selected
another terminal family through Fontconfig, those style requests could resolve
to the Omarchy family and then fail the hardcoded JetBrains identity check. The
client created a persistent Splint and exited before mapping a Window, making
the otherwise-correct `SUPER+RETURN` binding appear broken.

## Hotfix boundary

New clients now:

1. resolve the configured regular Fontconfig pattern;
2. use the effective regular family as authority for all three style requests;
3. accept a style face only when it belongs to that family, represents the
   requested weight/slant, and has the same terminal-cell advance;
4. deliberately reuse the regular face with a warning when the style is absent,
   substituted, unreadable, or metric-incompatible; and
5. default `main.font` to `monospace:style=Regular`, following Omarchy's current
   Fontconfig terminal-family selection.

Failure to resolve or load any usable regular primary face remains fatal. CJK
and emoji fallback policy is unchanged. Existing clients retain immutable
renderer resources; live family changes remain in the larger Plan 0038 scope.

## Regression evidence

Committed tests cover:

- style patterns derived from an arbitrary selected family with Fontconfig
  metacharacter escaping;
- rejection of foreign-family, duplicate, wrong-weight/slant, and
  metric-incompatible style candidates;
- exact regular-face identity reuse for unavailable styles;
- coherent real-system `monospace` resolution; and
- the new application default.

Local isolated Fontconfig fixtures additionally exercised:

- CaskaydiaMono Nerd Font with four real same-family style faces; and
- regular-only Audiowide, where all three styles warned and reused regular while
  the test remained successful.

No Omarchy/Fontconfig user configuration, package, installed executable, running
daemon, or graphical window was changed during implementation.
