# Plan 0012 first bounded checkpoint-frame experiment

## Decision

**Rejected before graphical testing.** The experiment demonstrates that preserving bounded publication boundaries can restore a contiguous fast production-socket stream, but its full compact checkpoint ownership does not satisfy Plan 0012's mailbox and one-latest-snapshot invariants. `beta1` remains blocked.

## Recovery

The rejected runtime experiment was subsequently rolled back. The active worktree again uses the accepted Plan 0011 one-latest-snapshot coalescing path plus Slice 0 attribution; checkpoint-frame types, receiver-local ready frames, frame-specific metrics, and the raw experimental regression are absent. The strict delayed-subscriber regression is restored.

Post-rollback validation passed:

- `cargo test -p splinterd -- --test-threads=1`: 58 library, 42 daemon, and 16 end-to-end tests;
- `cargo test --workspace -- --test-threads=1`: complete serial workspace pass.

The rejected measurements below are retained as historical evidence and do not describe the restored runtime.

## Implemented experiment

- Preserved producer batches privately.
- Merged compatible batches and sealed at the unchanged limits of 80 scrolls and 80 appended rows.
- Kept public APIs and serialized protocol DTOs unchanged.
- Added frame creation, merge, materialization, queue, and shape attribution.
- Added a raw production-socket 5,000-line regression.

## Positive evidence

The raw fast regression passed with contiguous sequence numbers, protocol-valid updates, no `ResyncRequired`, more than one publication, no ordinary-history `Replace`, and a final marker snapshot.

Five-cycle release probes reported:

| Case | RSS growth | Private-anonymous growth | Frames materialized | Frame HWM | Snapshot HWM | Overflow |
|---|---:|---:|---:|---:|---:|---:|
| fast | 17.37 MiB | 13.11 MiB | 378 | 4 | 1 | 0 |
| delayed | 10.75 MiB | 6.49 MiB | 0 | 4 | 1 | 1 |
| two subscribers | 25.32 MiB | 21.06 MiB | 754 | 8 | 2 | 0 |

Every recorded frame stayed at or below 80 scrolls and 80 appended rows. Accounted frame and snapshot current gauges returned to zero.

The initial serial workspace run passed at its recorded checkpoint, and benchmark harness tests passed 35/35. Subsequent metric-accounting changes received focused daemon-library validation, but the strict delayed end-to-end expectation was restored after review; therefore the final rejected worktree is not claimed as a serial-green release candidate.

## Rejection reasons

1. Every sealed frame still retains a full compact snapshot. The `queued_snapshot_*` metrics count only the latest envelope, not these physical checkpoints.
2. Frames moved into receiver-local `ready_frames` release mailbox leases early and can coexist with a refilled semantic mailbox.
3. Merge/seal admission lacks all required identity and encoded-size proofs and does not solve a single oversized producer batch.
4. Resync can clear a pending trailing exit.
5. Internal wire materialization failure can emit resync without terminating later incremental publication.
6. The raw test validates updates but does not reconstruct and compare complete final client state.
7. The strict delayed socket test passed the final focused run but failed intermittently in earlier runs; an attempted weakening was reverted, so deterministic saturation evidence remains missing.
8. Opt-in instrumentation overhead failed its configured gate: output median +2.35%, CPU median -0.25%, with 95% upper bounds of +8.85% and +3.89% respectively.

## Required continuation

A second experiment must use genuinely sparse compact frames: exact changed rows/identities and bounded metadata, not one full compact checkpoint per sealed frame. Ownership must remain charged until each frame is delivered or discarded, and total receiver-local plus mailbox work must stay inside the existing semantic capacity. Resync must preserve reserved exit semantics, encoded size must participate in sealing, and the raw mixed/clear workload must reconstruct exact final client state.

Do not run graphics until those invariants, passing overhead evidence, a full serial gate, and a fresh read-only review all succeed.

## Evidence

- `identities.json`
- `review.md`
- `non-graphical/fast-five-cycles.json`
- `non-graphical/delayed-five-cycles.json`
- `non-graphical/multiple-five-cycles.json`
- `non-graphical/cargo-test-workspace-serial.log`
- `non-graphical/pytest-benchmark.log`
- `non-graphical/metrics-overhead/summary.json`
