# Five-terminal startup and idle benchmark evidence

This directory preserves the development matrix run recorded on 2026-07-23.
It is durable raw evidence for the benchmark harness, not a publishable claim
that one terminal is universally faster or lighter than another.

## Run shape

- Terminals: Splinterm, Foot, Kitty, Ghostty, and Alacritty
- Guarded location: inactive workspace 8 on DP-2
- Warmup blocks: 3
- Measured blocks: 10
- Measured cases: 50
- Randomization seed: `20260723`
- Per-case settle interval: 1 second
- Per-case idle sample: 2 seconds
- Result: all 65 warmup and measured cases valid; every case verified cleanup

The matrix measures launch-to-child-ready, launch-to-window-map, and
child-inclusive process-tree idle resources. It does not measure
input-to-photon latency. Splinterm uses a prestarted daemon/client launch while
the other terminals use standalone process launches, so startup boundaries
must be interpreted with that architectural distinction.

## Contents

- `manifest.json`: host, repository state, executable versions, paths, and
  SHA-256 identities.
- `matrix.json`: random execution order, sample counts, validity, and complete
  summary statistics.
- `summary.md`: concise median table and interpretation warning.
- `raw/`: every warmup and measured per-terminal JSON record.
- `SHA256SUMS`: integrity hashes for the evidence files, excluding itself.

## Provenance warning

The manifest records repository commit
`e44e6b2f0ce83999b7a1c7de70c17b67a2a7008a` with `dirty: true`. The exact
Splinterm release binary is therefore identified by its manifest SHA-256 rather
than claimed to be reproducible from that commit alone. Keep this run labeled
as development evidence.

See [`../../terminal-benchmark-plan.md`](../../terminal-benchmark-plan.md) for
the methodology, fairness contract, and remaining output/resize lanes.
