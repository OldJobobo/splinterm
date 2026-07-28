# Plan 0012 first bounded-frame review

**Verdict:** not ready for graphical testing.

Fresh read-only review completed after the initial focused implementation and serial workspace evidence. It found these blockers:

1. Each sealed frame owns a full `CompactLiveSnapshot`; the reported one-snapshot HWM counts only the latest `SnapshotEnvelope` and omits frame checkpoints.
2. `ready_frames` outlives the mailbox queue leases, can coexist with a refilled mailbox, and is absent from current ownership gauges.
3. Merge admission proves scroll and append bounds but not every required revision, identity, row-patch, image/history, and encoded-size condition.
4. Resync precedence can clear a reserved pending exit.
5. A wire-materialization-generated `ResyncRequired` does not terminate later incremental publication.
6. The raw production test validates individual updates but does not apply them into an exact reconstructed client state for field-for-field final comparison.
7. The delayed end-to-end test had been weakened to accept ordinary updates; that weakening was removed after review.
8. The instrumentation-overhead artifact is invalid against its configured confidence-bound limits.

The review found no public protocol DTO/schema change, and the public `Subscription` API remains present. Those facts do not override the ownership and correctness blockers.

No graphical test, comparator-terminal run, release tag, commit, stage, or push followed this review.

## Rollback verification

A second fresh read-only review inspected the restored runtime after complete serial validation.

**Verdict: rollback accepted; no blockers.** It confirmed:

- `SnapshotEnvelope` again owns exactly one boxed compact snapshot;
- rejected checkpoint-frame types, receiver-local ready frames, pending-exit state, frame metrics, and the raw experimental regression are absent;
- accepted `recv_coalesced`, strict delayed-subscriber, and daemon `try_send` behavior are restored;
- Slice 0 batch attribution and Plan 0011 correctness fallbacks remain;
- public API and wire compatibility protections remain; and
- historical frame names and metrics exist only in retained rejection evidence.

The reviewer found no residual cleanup requirement.
