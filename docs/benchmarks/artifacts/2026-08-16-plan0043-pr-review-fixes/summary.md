# Plan 0043 PR review fixes

GitHub PR #22 review found two valid admission gaps. Materialization bases now own exact semantic leases across subscriber, Splint, and daemon limits, resize admission occurs before retained base mutation, and subscription drop releases ownership. Reusable-tail vectors now grow with deterministic exact capacities so the admitted old aggregate plus successor capture bounds mutation before consolidation to exact final bytes.

| Variant | RSS/PSS | Private-anon | CPU ticks | Marker latency | Resync |
|---|---:|---:|---:|---:|---:|
| Integrated Plan 0042 | 8.86 MiB | 8.83 MiB | 14.5 | 132.85 ms | 0 |
| PR review fixes | 7.50 MiB | 7.47 MiB | 13.0 | 126.75 ms | 0 |

All 20 measured cases completed with `error: null`. The generic runner remains `valid: false` only because its historical predicate expects both variants to reproduce the superseded defect. Full serialized workspace, warnings-denied Clippy, 58 focused live tests, ten exact reconstruction runs, formatting, and diff checks pass. Source is `7c22ad9` plus working diff `ac3d18df528b703bb62851577c86ffa8a278ce501ea4fd7a4e51b8f6865571f1`. A new exact-candidate graphical rerun is still required before merge.
