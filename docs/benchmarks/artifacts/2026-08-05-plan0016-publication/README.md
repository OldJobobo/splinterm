# Plan 0016 multiplexer benchmark publication

This is the compact, tracked publication artifact for
[Plan 0016](../../../plans/0016-multiplexer-benchmark-suite.md). The final
independent review approved publication on 2026-08-05.

## Contents

- `summary.md` and `summary.json`: generated aggregate results for the four-stack
  multiplexer matrix and the current five-terminal idle control.
- `review.md`: final independent publication decision and residual limitations.
- `PROVENANCE.json`: historical source artifact hashes and generation command.
- `CURATION.json`: the command's working directory plus durable identities for
  all three retained local source directories.
- `generate-publication.py`: publication generator snapshot.
- `implementation/`: checksum-bound aggregation and regression-test snapshots.
- `source-manifests/`: complete checksum manifests for the multiplexer and
  five-terminal raw source directories.
- `SOURCE-SHA256SUMS`: checksums copied verbatim from the reviewed publication
  bundle.
- `SHA256SUMS`: checksums for this complete tracked directory.

`summary.md` records its generation-time status as “candidate pending independent
review.” The later `review.md` is the authoritative publication decision.

## Source evidence

The aggregate was generated from these immutable local evidence directories:

- `benchmark-results/20260805T210644Z-milestone3-3w10m-seed-13372075-approved-retry/`
- `benchmark-results/20260805T215737Z-milestone4-five-terminal-idle-3w10m-seed-20260723-restart/`
- `benchmark-results/20260805T220456Z-plan0016-publication-review/`

The large raw matrix remains outside Git. Its identities are retained in
`PROVENANCE.json`, `CURATION.json`, and `source-manifests/`; the multiplexer
manifest contains 183 entries and the five-terminal manifest contains 68. The
historical generation command in `PROVENANCE.json` resolves relative to the
working directory recorded in `CURATION.json` and requires those local sources.

The generator recomputes multiplexer timing boundaries from raw cell reports. It
retains the five-terminal aggregate from that run's canonical `matrix.json`;
independent review reproduced it from all measured baseline reports. Do not
regenerate or edit this tracked artifact silently. Future benchmark work should
produce a new dated artifact.

## Interpretation limits

- Native and nested values measure complete stacks; Foot overhead is not
  subtracted.
- Screenshot marker polling is a coarse graphical observation, not compositor
  presentation latency or input-to-photon latency.
- Warmups are excluded from aggregates.
- Bare Foot divider and detach/reattach operations remain explicit N/A results.
- The five-terminal startup values use independently observed, non-identical
  launch models: Splinterm uses a prestarted daemon and peers use standalone
  launches.
- Results apply only to the recorded host and build.

## Verification

Verify the tracked curation from this directory:

```bash
sha256sum --check SHA256SUMS
sha256sum --check SOURCE-SHA256SUMS
```

Verify the local raw sources from the repository root:

```bash
artifact=$PWD/docs/benchmarks/artifacts/2026-08-05-plan0016-publication
(
  cd benchmark-results/20260805T210644Z-milestone3-3w10m-seed-13372075-approved-retry
  sha256sum --check "$artifact/source-manifests/multiplexer-SHA256SUMS"
)
(
  cd benchmark-results/20260805T215737Z-milestone4-five-terminal-idle-3w10m-seed-20260723-restart
  sha256sum --check "$artifact/source-manifests/five-terminal-SHA256SUMS"
)
```
