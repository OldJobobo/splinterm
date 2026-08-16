# Plan 0043 paced daemon retention diagnosis

**Leading allocation source isolated; allocator causality remains to be proven.**
The graphical regression is daemon-side. A matched renderer-free production-socket
profile compares the identical diagnostic test, workload, real 33 ms daemon pacing,
final-marker condition, resync rejection, build mode, and heaptrack preload on both
commits.

| Boundary | Plan 0042 | Plan 0043 | Delta |
|---|---:|---:|---:|
| Graphical daemon private-anon growth | 6.75 MiB | 14.57 MiB | +7.82 MiB |
| Matched profiled heap peak | 3.64 MiB | 7.86 MiB | +4.22 MiB |
| Matched profiled private-anon growth | 6.53 MiB | 13.85 MiB | +7.32 MiB |
| Heaptrack leak total | 62.22 KiB | 62.22 KiB | 0 |

Heaptrack attributes 6.70 MiB (85.2%) of the candidate's matched
7.86 MiB heap peak to allocation stacks rooted at
`SparsePublicationFrame::capture`, principally cloned compact visible/history row
bodies. Materialization and compact-to-live conversion account for only
0.10 MiB and
0.12 MiB at peak.
The existing production-socket reconstruction regression separately proves exact
final state and zero resync; the matched profiling harness intentionally measures
final-marker delivery and rejects any resync.

This evidence supports a narrower conclusion than the first draft: sparse capture
causes the additional live heap peak and is the leading source to remove before the
graphical rerun. Equal leak totals exclude a differential retained-allocation leak.
Because heaptrack was not sampled at the exact post-drain RSS timestamp, glibc page
retention is a plausible correlation, not yet proven as the cause of every byte in
the graphical plateau. A successful ownership-shape A/B must establish that link.

Rejected experiments:

- A direct 33 ms embedded consumer was invalid because it overflowed and resynced.
- Moving rows out of the temporary compact snapshot improved the debug median by
less than 1 MiB; queued sparse ownership remained.
- Raising sealing from 8 to 64 frames improved the median by about 0.9 MiB.
- Raising the per-chunk span to the existing 16 MiB subscriber limit improved it
by about 1.3 MiB. Neither cap change removed the peak.

The bounded fix direction is to avoid allocating a complete successor sparse frame
before merge. After admission and complete prevalidation, compose successor damage
directly into one reusable mailbox-tail representation with row/history buffer
reuse. Preserve ordered updates, one count lease per producer frame, exact semantic
bytes, continuity, exit precedence, and fail-closed resync. This changes ownership
shape, not the allocator, renderer, protocol, or admission ceilings.

The two harness patches, plain machine-readable peak attribution, plain peak excerpt,
compressed full reports/traces, profile logs, and checksums are retained here.
Production sources were restored after every diagnostic experiment.

Fresh read-only review `dbd49ebd` initially blocked on harness parity and overclaiming. After matched reprofiling and narrowing the conclusion, follow-up `2c5f9be7` returned **CLEAN**.
