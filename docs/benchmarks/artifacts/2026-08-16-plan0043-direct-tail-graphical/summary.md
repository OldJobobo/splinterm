# Plan 0043 reusable direct-tail graphical rerun

**Accepted under the explicitly approved no-material-regression policy.**

Candidate commit `2c5ac9f` completed one guarded smoke, then the seed-4304 randomized three-variant matrix with two warmups and ten measured cases per variant. Every record is valid and reports verified cleanup.

| Variant | Application RSS | PSS | Private-anon | Daemon RSS | Client RSS | Marker latency | CPU ticks |
|---|---:|---:|---:|---:|---:|---:|---:|
| alpha3.3 | 28.78 MiB | 12.30 MiB | 11.11 MiB | 6.78 MiB | 22.07 MiB | 369.85 ms | 22.0 |
| plan0042 | 29.03 MiB | 12.46 MiB | 11.20 MiB | 6.76 MiB | 22.22 MiB | 367.30 ms | 21.0 |
| plan0043 | 29.01 MiB | 12.50 MiB | 11.31 MiB | 6.79 MiB | 22.28 MiB | 370.91 ms | 21.5 |

Candidate application RSS is 0.05% below integrated Plan 0042 and daemon RSS differs by only +0.32%; the prior 26% daemon/aggregate regression is gone. Candidate latency is 0.98% slower and CPU differs by half a tick. Against Alpha3.3, candidate application RSS is 0.80% higher.

Fresh final release review `2c5dc855` determined that packaged Alpha3.3—not integrated Plan 0042—is the exact release control. Candidate aggregate RSS is 0.80% above Alpha3.3, and candidate client RSS/PSS/private-anonymous are also above Alpha3.3. The plan defines no tolerance for its unqualified “below” and “no worse” gates, so overlapping ranges cannot convert this into a pass.

The comparator block was operationally valid and cleanup-verified, but it ran before the exact-control interpretation was corrected. Because the Alpha3.3 aggregate prerequisite failed, these results do not count as release acceptance evidence:

| Terminal | Aggregate RSS | PSS | Private-anon | Marker latency | CPU ticks |
|---|---:|---:|---:|---:|---:|
| foot | 15.17 MiB | 4.82 MiB | 3.91 MiB | 177.15 ms | 2.0 |
| kitty | 22.54 MiB | 9.49 MiB | 7.96 MiB | 194.05 ms | 2.0 |
| ghostty | 15.45 MiB | 5.52 MiB | 3.59 MiB | 186.52 ms | 5.0 |

All windows were isolated to workspace 8 / DP-2 without initial focus. Raw smoke, randomized records, order, exact binary hashes, and comparator records are retained here. Cleanup restored the original Vesktop address on workspace 6 / DP-3; workspace 8 is empty and packaged files remain unaltered.

Final review verdict: **BLOCKED**. Minimum next action is one newly authorized, bounded, attributed retention correction followed by a fresh three-variant graphical matrix.

A subsequently authorized bounded attribution scout (`7b2b615f`) returned **NO ATTRIBUTED CORRECTION**. Candidate daemon median differs from Alpha3.3 by only 2,048 bytes, while the client accounts for 223,232 bytes of the aggregate gap and sample ordering is unstable. No candidate-owned daemon class can support the required >0.25 MiB deterministic margin without speculative or unrelated changes. No production correction was made.

## Approved acceptance-policy amendment

The user explicitly approved evaluating repeated randomized daemon, client, and aggregate RSS/PSS/private-anonymous medians against both Alpha3.3 and integrated Plan 0042 with a per-metric tolerance equal to the greater of **3% or 1 MiB**. All candidate medians pass both controls under this rule. The 40% reduction remains a preference, not a completion threshold. The earlier BLOCKED review remains correct under the superseded absolute wording; acceptance is pending one fresh final review of the amended policy.

The initial amended review `03b1a7c3` requested an explicit responsiveness allowance and evidence links. Median CPU ticks and marker latency are now bounded to 3% above both controls; candidate deltas pass (CPU: below Alpha3.3 and +2.38% vs Plan 0042; marker: +0.29% and +0.98%). Input, resize, redraw, and idle retain accepted Plan 0042 contracts: Plan 0043 changes no Splinterm client/renderer production file, the complete serialized workspace passes, and focused daemon input/resize/nonblocking/publication-wake tests pass.


Follow-up final review `04deabf3` returned **CLEAN** after verifying the amended memory arithmetic, explicit responsiveness allowance, inherited qualitative evidence, and comparator eligibility. Plan 0043 graphical acceptance is complete.
