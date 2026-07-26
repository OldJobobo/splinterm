# Phase 5 Slice 8 no-image idle gate

Ten guarded current-build samples use the retained pre-image methodology:
one isolated Splinterm daemon, client, and idle child; one-second warmup;
two-second sample; child-inclusive process-tree RSS/CPU; workspace 8 on DP-2;
and verified cleanup.

Result:

- median RSS: 51,107,840 bytes;
- median growth over the 49,262,592-byte baseline: 1,845,248 bytes;
- allowed growth: 2,463,129 bytes — **pass**;
- median idle CPU: 0.5 ticks;
- nearest-rank p95 idle CPU: 2 ticks versus the retained 1-tick budget —
  **fail**.

This directory is honest blocking evidence. It does not close Slice 8 or Phase
5. Earlier LTO matrices and the pre-LTO failure remain separate diagnostic
records and are not acceptance evidence.
