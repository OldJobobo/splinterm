# Not approved for the bounded-fix package

## Review

- **Correct — alpha product fix:** `colors.alpha` now calls `parse_alpha` before `foot_alpha` (`crates/splinterm/src/config.rs:296-298`). `parse_alpha` parses as `f32`, rejects every non-finite value with `is_finite`, rejects finite values outside the inclusive `0.0..=1.0` interval, and only then returns the value for conversion (`crates/splinterm/src/config.rs:401-408`; conversion at `crates/splinterm/src/config.rs:551-558`). Thus `NaN`, positive infinity (including `inf`, `+inf`, and `+infinity`), negative infinity, `-0.1`, and `1.1` cannot reach `foot_alpha`. The test asserts the exact line-numbered range diagnostic for `NaN`, positive/negative infinity, and both finite range directions (`crates/splinterm/src/config.rs:673-678`). The focused test passed in this review, and the refreshed retained library log records that test plus all 185 library tests passing (`docs/spikes/artifacts/0032-native-background-blur/validation/cargo-test-splinterm-lib.log:23,191`). No config regression was found.

- **Correct — release-facing documentation:** the configuration guide identifies `ext-background-effect-v1`, finite logical-region behavior, graceful missing-protocol/capability fallback, no effect object for opaque/disabled states, staging status, and the initially validated Hyprland 0.56.1+ target (`docs/configuration.md:34-50`). ADR 0004 now gives the same staging/Hyprland/fallback boundary (`docs/adr/0004-font-and-cpu-renderer.md:133-138`) and keeps native blur in disposable presentation state without changing CPU pixels or oracle semantics (`docs/adr/0004-font-and-cpu-renderer.md:152-157`). These bounded documentation defects from the prior review are resolved accurately.

- **Correct — plan status:** Plan 0013 remains explicitly in progress (`docs/plans/0013-native-background-blur.md:3-7,396-408`) and Spike 0032 does not advertise release (`docs/spikes/0032-native-background-blur-graphical-validation.md:165-177`). This correctly preserves the outstanding completion gates at `docs/plans/0013-native-background-blur.md:482-499`.

- **Blocker (low, retained-evidence integrity) — the refreshed 185-test log invalidated its recorded checksum.** `docs/spikes/artifacts/0032-native-background-blur/SHA256SUMS:13` records `b600f693…` for `validation/cargo-test-splinterm-lib.log`, but the current file hashes to `81819f65…`; `sha256sum --check` fails exactly that entry while the other 14 pass. The log timestamp is later than the manifest and contains the post-fix test result. This contradicts the prior review's retained-integrity statement (`docs/spikes/artifacts/0032-native-background-blur/review/final-review.md:29`) and means the bounded-fix evidence package is not yet approvable as a whole. **Smallest safe fix:** refresh only the manifest entry for `validation/cargo-test-splinterm-lib.log`, then rerun `sha256sum --check`; do not alter or regenerate graphical evidence.

- **Note — no remaining product/code/docs blocker:** apart from the checksum bookkeeping defect, the non-finite-alpha and staging/availability documentation fixes are approved. No new source or documentation defect was found.

- **Note — user-owned release gates remain:** the exact strict workspace Clippy command is still required by `docs/plans/0013-native-background-blur.md:414-420,497-498` and its retained failure is accurately recorded at `docs/spikes/artifacts/0032-native-background-blur/validation/summary.json:20-27`. The resource/idle requirement remains at `docs/plans/0013-native-background-blur.md:430-441,495` and still needs a separately approved guarded RC-versus-pre-feature measurement. Those two gates require user authorization/decision as identified by the prior review (`docs/spikes/artifacts/0032-native-background-blur/review/final-review.md:11-13,31-33`).

## Plan 0013 release decision

**Bounded product/code/docs fixes: Approved. Bounded-fix evidence package: Not approved until the one stale checksum is refreshed and verified. Whole Plan 0013 release: Blocked.**

After the checksum-only correction, the only residual Plan 0013 release blockers are the two known user-owned gates:

1. exact strict workspace Clippy requires broad repository baseline cleanup (or an explicit closure-policy decision); and
2. resource/idle completion requires separately approved guarded graphical RC-versus-pre-feature startup-RSS and settled-idle measurement.

No graphical test, Clippy run, expensive full suite, or subagent was launched in this review.

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Concrete findings cite config.rs:296-298,401-408,551-558,673-678; configuration.md:34-50; ADR 0004:133-138,152-157; Plan 0013:414-420,430-441,482-499; and the stale checksum at SHA256SUMS:13. Residual release risks and the smallest evidence fix are explicit."
    }
  ],
  "changedFiles": [
    "docs/spikes/artifacts/0032-native-background-blur/review/final-fix-review.md"
  ],
  "testsAddedOrUpdated": [
    "crates/splinterm/src/config.rs::tests::foot_background_alpha_and_blur_are_strict_last_assignment_overrides"
  ],
  "commandsRun": [
    {
      "command": "git diff/status/stat inspections for the bounded source and documentation files",
      "result": "passed",
      "summary": "Inspected the actual worktree changes; no staged files and no additional product/code/docs defect found."
    },
    {
      "command": "cargo test -p splinterm --lib config::tests::foot_background_alpha_and_blur_are_strict_last_assignment_overrides -- --exact",
      "result": "passed",
      "summary": "1 passed; 184 filtered out."
    },
    {
      "command": "python -m pytest -q tools/benchmark/test_benchmark.py -k omarchy_theme_generator",
      "result": "passed",
      "summary": "1 passed, 34 deselected."
    },
    {
      "command": "cargo fmt --all --check",
      "result": "passed",
      "summary": "Formatting check completed without output."
    },
    {
      "command": "git diff --check",
      "result": "passed",
      "summary": "No whitespace errors."
    },
    {
      "command": "cd docs/spikes/artifacts/0032-native-background-blur && sha256sum --check SHA256SUMS",
      "result": "failed",
      "summary": "14 entries passed; validation/cargo-test-splinterm-lib.log failed because its post-fix content no longer matches SHA256SUMS:13."
    },
    {
      "command": "cargo clippy --workspace --all-targets -- -D warnings",
      "result": "not-run",
      "summary": "Prohibited by task; retained logs record the known Rust 1.97 repository-baseline failure."
    },
    {
      "command": "graphical resource/idle measurement and expensive full workspace suite",
      "result": "not-run",
      "summary": "Prohibited by task; retained 185-library-test and prior serial-workspace logs were inspected."
    }
  ],
  "validationOutput": [
    "Focused alpha/config test: 1 passed, 184 filtered out.",
    "Generator pytest: 1 passed, 34 deselected.",
    "Retained refreshed Splinterm library log: 185 passed, including the updated alpha test.",
    "cargo fmt --all --check and git diff --check: passed.",
    "Artifact checksum verification: 14 passed, 1 failed (refreshed library log)."
  ],
  "residualRisks": [
    "Low: SHA256SUMS:13 is stale for the refreshed post-fix library-test log; refresh that single entry and verify the manifest.",
    "Plan release: exact strict workspace Clippy remains blocked on broad repository baseline cleanup or a user-owned policy decision.",
    "Plan release: resource/idle completion still requires separately approved guarded graphical RC-versus-pre-feature measurement."
  ],
  "noStagedFiles": true,
  "diffSummary": "The bounded source fix adds finite/range-specific alpha validation and exact rejection tests; user-facing docs and ADR add staging, Hyprland 0.56.1+, and fallback disclosures; the plan remains in progress. The retained library-test log was refreshed without refreshing its checksum manifest entry.",
  "reviewFindings": [
    "correct: crates/splinterm/src/config.rs:296-298,401-408,673-678 - all non-finite and out-of-range alpha values are rejected before conversion with exact diagnostics and test coverage.",
    "correct: docs/configuration.md:34-50 and docs/adr/0004-font-and-cpu-renderer.md:133-138 - staging status, Hyprland 0.56.1+ initial target, and graceful capability fallback are accurately disclosed.",
    "blocker (low, evidence): docs/spikes/artifacts/0032-native-background-blur/SHA256SUMS:13 - refreshed 185-test log does not match its retained checksum.",
    "release blockers: strict workspace Clippy baseline cleanup and separately approved guarded resource/idle measurement remain user-owned."
  ],
  "manualNotes": "Product/code/docs fixes are approved. The evidence package needs one checksum-only correction; after it, only the two known user-owned Plan 0013 release gates remain."
}
```
