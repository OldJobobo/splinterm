# Plan 0023 ANSI history throughput

This artifact compares the retained Plan 0022 live-history candidate against a
single client-side history-cache optimization. Both reports use the same release
harness contract: five warmups, 30 measured samples, deterministic interleaving,
and update construction outside measured intervals.

## Results

| Case | Control p95 | Candidate p95 | Improvement | 95% lower improvement |
| --- | ---: | ---: | ---: | ---: |
| `ansi-h0-live-p1-focused` | 117.028 ms | 5.000 ms | 95.7% | 93.6% |
| `ansi-h1000-live-p1-focused` | 253.246 ms | 8.626 ms | 96.6% | 96.0% |
| `ansi-h4096-live-p1-focused` | 430.084 ms | 28.515 ms | 93.4% | 93.2% |
| `ansi-h4096-detached-p1-focused` | 1,023.232 ms | 604.791 ms | 40.9% | 36.0% |
| `ansi-h4096-live-p4-all` | 2,048.908 ms | 118.723 ms | 94.2% | 93.6% |
| `ansi-h4096-live-p4-inactive` | 1,424.074 ms | 102.086 ms | 92.8% | 92.5% |

The optimization retains cached history rows in place, scans cache bytes once,
and trims prefixes in batches. It does not change the 4,096-row / 16 MiB bounds.
The exact equivalence test includes empty, exact-row-bound, row-overflow,
byte-overflow, newest-retained, and oldest-retained cases.

All small-update candidate p95 values remain below 0.344 ms. Relative changes at
that microsecond boundary are not treated as performance claims.

## Interpretation

This proves removal of reducer-level scrollback amplification. It does not claim
end-to-end presentation latency or replace Plan 0022's graphical evidence.
Detached operation intentionally retains previous rows for anchor accounting and
therefore improves less than live operation.

## Contents

- `control-report.json` / `control-summary.json`: immutable Plan 0022 control.
- `candidate-report.json` / `candidate-summary.json`: optimized 5/30 release run.
- `comparison.json`: 20,000-resample deterministic bootstrap comparison.
- `implementation/terminal_state.rs`: exact measured reducer source.
- `PROVENANCE.json`: commands, hashes, host/toolchain, and source identities.
- `SHA256SUMS`: checksum coverage for every publication file except itself.
