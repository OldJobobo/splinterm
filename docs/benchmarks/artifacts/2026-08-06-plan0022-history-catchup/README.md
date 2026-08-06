# Plan 0022 graphical catch-up evidence

- Plan SHA-256: `091cc4d798e0ed460f421902713d1bd17efc2f1e68379f1859da372739d87869`
- Schedule: 30 warmups + 100 measured cases; all 130 execution indexes valid
- Bootstrap: 20,000 deterministic resamples, seed `220022`, one-sided 95% upper bounds
- Screenshot values are coarse observation latency and are not presentation timing
- Full raw traces remain retained locally and are checksum-bound by `RAW-SHA256SUMS`

| Cell | Receive→commit median | p95 | p95 UCB | Callback p95 | Screenshot p95 |
| --- | ---: | ---: | ---: | ---: | ---: |
| `all-pane-scaling` | 49.078 ms | 65.172 ms | 65.172 ms | 36.333 ms | 584.483 ms |
| `ansi-stress` | 293.804 ms | 379.673 ms | 379.673 ms | 82.747 ms | 5848.461 ms |
| `detached-history` | 19.404 ms | 21.696 ms | 21.696 ms | 17.399 ms | 160.123 ms |
| `history-1000` | 2.301 ms | 3.100 ms | 3.100 ms | 32.581 ms | 206.599 ms |
| `history-4096` | 8.029 ms | 10.025 ms | 10.025 ms | 34.561 ms | 183.204 ms |
| `inactive-scaling-2` | 8.783 ms | 10.113 ms | 10.113 ms | 32.038 ms | 305.731 ms |
| `inactive-scaling-4` | 8.860 ms | 10.423 ms | 10.423 ms | 28.855 ms | 299.325 ms |
| `outer-resize` | 1.206 ms | 3.056 ms | 3.056 ms | 34.665 ms | 5390.073 ms |
| `static-movement` | N/A | N/A | N/A | N/A | 389.263 ms |
| `zero-history` | 1.232 ms | 1.563 ms | 1.563 ms | 31.009 ms | 185.166 ms |

## Decisions

- Focused 0/1,000/4,096-row small updates pass the 50 ms p95 gate: **True**.
- Four-pane focused-only scaling ratio is `1.040` with UCB `1.165`: **passes ≤1.25**.
- Static movement caused zero semantic applies, history clones, frame rebuilds, or configure events: **passes**.
- Measured trace resyncs: `0`.
- Four-pane all-active p95 is 65.172 ms; ANSI-stress p95 is 379.673 ms. These remain bounded slow paths, not ordinary focused-update passes.
- Candidate graphical history/zero amplification is `6.414` (UCB `7.257`). Reduction and zero-history regression versus a graphical control remain **unresolved** because no graphical control binary was run.
- Existing matched non-graphical evidence still proves the history-heavy candidate improvement, while its microsecond-scale zero-history confidence bound remains unresolved.
- Outer-resize receive→commit is the post-resize marker boundary, not total resize settlement; its screenshot observation includes all twelve paced steps. ANSI screenshot observation likewise includes the deliberately paced 2,000-line child workload.

The single retained invalid report is the first execution-index 29 attempt rejected by the old summarizer for legitimate repeated `frame_prepare` records. Its raw evidence was preserved; the retry is the sole valid report for index 29.
