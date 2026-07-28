# Plan 0011 Slice 3 candidate evidence

Source HEAD: `0ce4fc62ed7ade2138fd35f492075942b415a571` (uncommitted candidate)

| Case | RSS growth | PSS growth | Overflows | Queue HWM | Snapshot HWM | Rows HWM |
|---|---:|---:|---:|---:|---:|---:|
| fast | 19.05 MiB | 19.05 MiB | 0 | 64 | 1 | 300 |
| delayed | 5.46 MiB | 5.46 MiB | 1 | 64 | 1 | 266 |
| overflow | 2.46 MiB | 2.46 MiB | 1 | 1 | 1 | 24 |
| multiple | 20.41 MiB | 20.41 MiB | 0 | 48 | 2 | 255 |

Fast retained growth fell 44.50% from the accepted Slice 2 evidence and is below both the 24 MiB minimum and 20 MiB preferred bounded thresholds.

Contiguous compact updates materialize no history or only a proven append tail. Clear, reflow, alternate-screen, generation change, and ambiguous scrollback retain full-history fallback. Attach, explicit snapshots, paging, and search are unchanged.

Validation: directly affected packages passed; the bounded serial workspace retry passed. The ordinary concurrent daemon suite had the known policy timeout and one phase-8 timing failure, both exact isolated cases passed. The first serial workspace attempt had one unrelated MCP controller flake; its exact isolated test and the bounded retry passed.

This is candidate evidence pending fresh independent review. No graphical testing was run.
