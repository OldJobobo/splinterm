# Phase 5 Slice 8 final no-image idle gate

Ten guarded current-build samples use the retained pre-image methodology:
one isolated Splinterm daemon, client, and idle child; one-second warmup;
two-second sample; child-inclusive process-tree RSS/CPU; workspace 8 on DP-2;
and verified cleanup.

Result:

- median RSS: 51,036,160 bytes;
- median growth over the 49,262,592-byte baseline: 1,773,568 bytes;
- allowed growth: 2,463,129 bytes — **pass**;
- median idle CPU: 1 tick;
- nearest-rank p95 idle CPU: 1 tick — **pass**;
- maximum idle CPU: 1 tick.

Release Thin LTO keeps RSS within the strict 5% allowance. Image-token expiry
sleeps when no token exists, and unchanged theme files are fingerprinted before
both legacy and managed-window reload paths parse JSON. All samples preserve
the guarded workspace/focus/cleanup invariants.
