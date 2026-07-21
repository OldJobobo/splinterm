# Phase 3 closure evidence

Recorded on 2026-07-20 for the Phase 3 multiplexing closure.

## Functional gates

- Protocol v17 carries bounded subscriber-specific control status, explicit
  transfer request/accept/deny/result events, separately confirmed forced
  takeover, and bounded search requests/results.
- Controller state tests cover per-Splint leases, atomic accepted transfer,
  denial, stale-owner rejection, timeout expiry, and disconnect cancellation.
- Real-daemon tests cover denial, acceptance, transferred input, stale owner
  rejection, disconnect release, literal Unicode search, opaque cursor paging,
  and revision/generation invalidation.
- All seven serialized `splinterd` lifecycle scenarios pass together. The old
  moving SIGINT failure was resolved by retaining one pinned prioritized signal,
  owning connection tasks, and aborting nested writer/subscription tasks on
  connection cancellation before runtime drain and final persistence.

## Bounds and performance continuity

Search does not copy configured history: it walks the actor-owned ring newest
first and stops at a 10 ms deadline or 64 results. Wire bounds are 256 query
bytes, 256 preview bytes, and 32 cursor bytes. Control fanout uses a bounded
32-event broadcast queue and pending transfers expire after 15 seconds.

Phase 3 adds no persisted terminal body, peer detail, unbounded index, or
long-lived search corpus. The enforced Phase 2 CPU/PSS, PTY queue, renderer,
scrollback-page, and package baselines remain recorded in
`../0017/slice9-performance/` and `../0018-packaging/`.

## Graphical isolation

The single guarded line/frame smoke ran on workspace 8 assigned to DP-2 while
workspace 1 on DP-1 remained active. It retained the active workspace, active
window, and pointer, and left workspace 8 and the isolated socket clean. See
`../phase3-pane-dividers/summary.json`.

No canonical Foot source, oracle image, tolerance, or comparison reference was
changed.
