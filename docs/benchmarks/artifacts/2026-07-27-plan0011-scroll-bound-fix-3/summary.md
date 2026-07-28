# Plan 0011 final no-go

**Decision: do not close Plan 0011 as release-ready and do not tag `beta1`.**

## Correctness

Oversized coalesced scroll batches now fall back to bounded final-state viewport patches. Oversized append history uses the existing bounded `HistoryTransition::Replace`; no protocol limits, DTOs, or wire types were widened. Focused daemon/client/protocol tests and the full serial workspace suite pass.

## Non-graphical evidence

The corrected five-cycle/120-second daemon workload retained 13.68 MiB RSS and 9.42 MiB private-anonymous memory with zero overflow. Slice 4 allocator diagnostics still justify no allocator-specific product reclamation or manual trim.

## Graphical evidence

The final smoke passed workspace 8 / DP-2 placement, no-focus, marker, identity, and cleanup guards. The randomized clean-HEAD comparison completed with two warmups and ten measured samples per variant.

| Metric | Control median | Candidate median | Decision |
|---|---:|---:|---|
| Aggregate retained RSS | 70.45 MiB | 78.05 MiB | candidate 10.78% worse |
| Marker latency | 396.13 ms | 615.86 ms | regression |
| CPU ticks | 19.0 | 76.5 | regression |
| Daemon retained RSS | 34.11 MiB | 21.17 MiB | improvement |
| Client retained RSS | 36.31 MiB | 56.84 MiB | regression |

The daemon optimization works, but it moves high-water pressure into the client through a large coalesced update. The required 40% aggregate improvement is not established. Foot/Kitty/Ghostty comparisons were therefore correctly skipped.

## Next architecture

A future plan may evaluate bounded intermediate compact checkpoints/publication batches so fast clients receive protocol-sized updates while delayed subscribers still retain at most one compact snapshot. That is an architectural continuation, not a closure or Slice 4 reclamation tweak.

Exact source provenance, candidate/control binary hashes, raw records, process attribution, and serial validation are retained here.

## Final review

A fresh read-only final review found no acceptance blockers. It accepted Slices 1–3, the bounded correctness fallbacks, the Slice 4 no-change conclusion, and this no-go record. It confirmed that `beta1` remains forbidden. See [`review/final-review.md`](review/final-review.md).
