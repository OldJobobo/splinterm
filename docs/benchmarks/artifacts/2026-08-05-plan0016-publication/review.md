# Plan 0016 final independent publication review

**Decision: APPROVED — 2026-08-05**

All three prior publication blockers are corrected:

1. Plan 0016 records complete status and exact evidence facts.
2. Checksum-covered provenance retains the generator, corrected aggregator and test, input hashes, outputs, and exact generation command.
3. The five-terminal baseline qualifies Splinterm's prestarted-daemon model versus standalone peers.

Plan 0016 is approved for publication subject to its recorded host/build, complete-stack, visible-marker, warmup, launch-model, and N/A limitations.

## Independently verified evidence

- 183/183 Milestone 3 source checksum entries passed.
- 6/6 pre-review publication checksum entries passed.
- Corrected aggregator and regression-test snapshots byte-match current source.
- Every provenance input hash matches its identified artifact.
- Focused Ruff validation passed.
- `tools/benchmark/test_multiplexer_matrix.py`: 13 passed.
- `git diff --check` passed.
- No publication blocker remains.

## Residual limitations

- Results apply only to the recorded host and build.
- Stack values are complete-stack measurements; Foot overhead is not subtracted.
- Visible-marker polling is an approximation, not compositor presentation latency.
- Warmups are excluded from aggregates.
- Independent Foot divider and detach/reattach cases remain explicitly N/A.
- Five-terminal startup values compare independently observed, non-identical launch models.

Reviewer session: `/home/oldjobobo/.pi/agent/sessions/--home-oldjobobo-Projects-splinterm--/2026-08-05T07-21-32-057Z_019fd0cc-8499-7ac8-bb98-baa4123a1a5b/0c40cb23/run-0/session.jsonl`
