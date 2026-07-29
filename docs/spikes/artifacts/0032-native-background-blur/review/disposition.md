# Plan 0013 review disposition

## First final review

`final-review.md` returned **Not approved** with four blockers:

1. non-finite `[colors] alpha` values bypassed strict range validation;
2. the exact strict workspace Clippy command failed on the Rust 1.97 repository baseline;
3. startup RSS and settled-idle measurements were absent; and
4. release-facing docs omitted staging-protocol and initial-compositor status.

The parent applied the two in-scope fixes directly:

- `parse_alpha` now rejects non-finite and out-of-range values before conversion,
  with exact tests for `NaN`, `inf`, `-inf`, `-0.1`, and `1.1`; and
- the configuration guide and ADR now name the staging protocol, Hyprland
  0.56.1+ initial validation, and capability fallback.

Focused config and generator tests, formatting, `git diff --check`, and all 185
Splinterm library tests passed after those changes.

## Final fix review

`final-fix-review.md` approved the product/code/documentation fixes and found no
new source defect. It reported one evidence-bookkeeping blocker: the post-fix
library test log had changed after `SHA256SUMS` was generated.

The parent regenerated `SHA256SUMS` over the final evidence package and reran
`sha256sum --check SHA256SUMS`; every retained entry passed. No graphical
evidence or oracle reference was regenerated or altered.

## User decisions and resource closure

The user selected the explicit retained-baseline Clippy policy rather than a
broad unrelated Rust 1.97 lint refactor. Plan 0013 now requires the exact command
and retained full failure output, unchanged lint policy, and review proving no
Plan 0013 diagnostic. Those conditions are present in this evidence package.

The user also approved a guarded RC-versus-pre-feature opaque/blur-disabled
resource sequence. Attempts 1 and 2 exposed harness setup errors (missing
pre-feature PTY helper, then an overlong Unix socket path) and were not retried
automatically. Both cleaned up. After explicit approvals and a successful
headless pre-feature daemon/helper check, attempt 3 passed its smoke and five
matched pairs:

- pre-feature median RSS: 25,206,784 bytes;
- RC median RSS: 25,268,224 bytes;
- delta: 61,440 bytes, below the 1,048,576-byte measurement-noise floor; and
- both versions: median 0 and maximum 1 idle CPU tick over two seconds.

Workspace 8 ended empty, DP-2 remained unfocused at scale 1.0/transform 0, and
the guarded focus address was unchanged. Exact evidence is under
`resource-idle*/` and `harness/resource-idle-runner-attempt-*.executed.py`.

No known product, code, documentation, lint-policy, resource, or graphical gate
remains. Final plan closure still requires an independent review of this newly
retained resource/policy evidence.
