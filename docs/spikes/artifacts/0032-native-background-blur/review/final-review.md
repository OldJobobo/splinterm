# Not approved

## Review

### Product defects

- **Blocker (medium) — non-finite user alpha bypasses the strict configuration contract.** `crates/splinterm/src/config.rs:296-299` sends `[colors] alpha` through the generic range parser, whose check at `crates/splinterm/src/config.rs:393-399` accepts `NaN` because both ordered comparisons are false. `foot_alpha` then casts it at `crates/splinterm/src/config.rs:546-548` (to zero on the retained Rust baseline), so `alpha=NaN` can become fully transparent and make `blur=yes` eligible instead of failing startup. This contradicts the documented `0.0–1.0` range at `docs/configuration.md:26` and Plan 0013's strict-config completion criterion at `docs/plans/0013-native-background-blur.md:488`. **Smallest safe fix:** explicitly reject non-finite INI alpha before conversion and add focused `NaN`/infinity startup-parser tests; retain the existing range and lint policy.

### Evidence and policy blockers

- **Blocker (high) — the exact required strict-Clippy gate fails.** Plan 0013 names `cargo clippy --workspace --all-targets -- -D warnings` at `docs/plans/0013-native-background-blur.md:414-420` and requires strict Clippy to pass at `docs/plans/0013-native-background-blur.md:484-499`. The retained result is explicitly failed at `docs/spikes/artifacts/0032-native-background-blur/validation/summary.json:20-27`; the raw workspace log stops on Rust 1.97 `replace_box` and `collapsible_match` errors in `splinterm-core` and `splinterm-terminal`. Recording a known baseline is accurate evidence, but it does not satisfy the written pass criterion, so Plan 0013 cannot accurately close as written. **Smallest policy-preserving resolution:** authorize and complete the broader repository lint-baseline fixes, then rerun the exact command. Otherwise the release must remain blocked; no lint weakening is proposed.

- **Blocker (medium) — retained evidence does not establish the resource/idle completion gate.** The plan requires no added SHM/backing allocation, no idle wakeup, startup RSS within measurement noise, and no graphical idle/resize regression at `docs/plans/0013-native-background-blur.md:430-441`, then requires the resource/idle gates to pass at `docs/plans/0013-native-background-blur.md:495`. Static implementation and tests support the first parts: blur-only reconciliation is protocol-only (`crates/splinterm/src/wayland.rs:3298-3347`, `3393-3478`), and the serial log records the event-driven timeout test. The graphical artifacts prove resize, placement, focus, and cleanup, but contain no RSS or settled-idle measurement; `docs/spikes/0032-native-background-blur-graphical-validation.md:134-149` claims only generator/tests/format/workspace/diff closure. **Smallest safe fix:** retain one approved guarded RC-versus-pre-feature opaque/blur-disabled startup-RSS measurement and a settled-idle measurement (with the existing resize evidence), without repeating the visual matrix. That graphical measurement needs separate user approval and was not run in this review.

- **Blocker (low, bounded documentation fix) — release-facing docs omit the staging-protocol limitation.** Slice 7 explicitly requires staging status to be documented at `docs/plans/0013-native-background-blur.md:400-405`. The updated configuration guide describes capability fallback at `docs/configuration.md:39-44`, and ADR 0004 does so at `docs/adr/0004-font-and-cpu-renderer.md:133-137`, but neither says `ext-background-effect-v1` is a staging protocol or names the initially validated Hyprland 0.56.1+ boundary. **Smallest safe fix:** add one concise staging/availability sentence to the user-facing configuration guide and ADR.

### Correct

- **Reducer and Wayland lifecycle:** eligibility is conjunctive and lazy (`crates/splinterm/src/background_effect.rs:198-205`); enable orders create/finite-region/commit, disable orders destroy/commit, and `DestroyPending` blocks same-transition recreation (`crates/splinterm/src/background_effect.rs:227-291`). Geometry rejects non-positive/out-of-range values (`crates/splinterm/src/background_effect.rs:18-34,163-170`). The Wayland executor translates those actions in order, drops the temporary region before commit, coalesces draw-bound commits, and deterministically releases effect then manager (`crates/splinterm/src/wayland.rs:3393-3504`). Capability events preserve unknown bits and reconcile every event (`crates/splinterm/src/wayland.rs:7052-7082`). No lifecycle product defect was found.

- **Scope containment:** the committed range changes only the listed Splinterm client/config/generator/spike files plus the disposable protocol example. No added first-party `unsafe` line was found. `crates/splinterm/src/renderer.rs` is byte-identical between `1e233a1` and the reviewed tree (SHA-256 `e7b53bf5ac5bbcd2a62b7f39e9a117b04c0ca4d613652db53e238195c9906969`), and the daemon, PTY, private protocol, terminal semantics, Foot-oracle tooling, and retained oracle artifacts are unchanged. The direct staging dependency is correctly declared at `Cargo.toml:52` and `crates/splinterm/Cargo.toml:29`.

- **Precedence and fallback (apart from non-finite INI alpha):** the generator selects one section and uses last assignment at `tools/generate-omarchy-theme.py:45-79`; generated blur defaults false and malformed present JSON blur is rejected at `crates/splinterm/src/config.rs:412-429`; explicit overrides are applied after theme resolution at `crates/splinterm/src/main.rs:4440-4465`. Focused generator and 185-library-test evidence is retained at `validation/summary.json:5-17`.

- **Graphical claim integrity:** the stale matrix captures are explicitly non-retained/non-acceptance evidence (`matrix-summary.json:24-31` and each case equivalent; `docs/spikes/0032-native-background-blur-graphical-validation.md:119-132`). The guarded smoke records one manager/capability/create, finite regions, commits, unchanged focus/workspace, and exact identities (`smoke-summary.json:4-45`). All eight Splinterm protocol cases, including opaque/no-object, live destroy, resize, fractional scale, and one multi-pane object, are retained at `matrix-summary.json:10-240`; separate Foot completion and the harness collision are disclosed at `matrix-summary.json:242-256`. Cleanup is retained at `matrix-cleanup.json:1-17`. No rotated lane was claimed (`matrix-summary.json:7-9`) and no DP-3 claim appears. The graphical binary was built at later commit `d2affaf`, but `7bd8c49..d2affaf` changes only two unrelated Herdr documentation files, so binary inputs match the requested implementation range.

- **Opaque/final-buffer evidence is sufficient:** the renderer source hash is unchanged, the retained 185-test log includes the final-buffer and alpha semantics tests, and the opaque graphical lane records zero effect objects (`matrix-summary.json:63-83`). No Foot reference or tolerance was changed.

- **Artifact integrity and retained validation:** all entries in `SHA256SUMS` verify; all five JSON summaries parse; relevant local Markdown links resolve; retained results record generator, 185 library tests, formatting, serial workspace, link, hash-manifest, and diff checks as passing. Strict Clippy is the sole recorded failed validation command, but the additional resource/idle evidence is absent rather than failed.

## Release decision

**Not approved.** Native blur's reducer, Wayland lifecycle, scope boundary, precedence, graphical protocol evidence, cleanup, and opaque pixel/oracle boundary are otherwise release-sound. Release remains blocked by one bounded product fix (non-finite INI alpha), missing resource/idle measurements, the staging-status documentation fix, and the exact strict-Clippy gate. The only out-of-scope decision is whether the user authorizes the broader repository lint-baseline cleanup; without that authorization, Plan 0013 must remain in progress. A separate approval is also required before collecting the missing guarded graphical resource/idle evidence.

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Concrete product and evidence/policy blockers cite config.rs, Plan 0013, configuration/ADR docs, validation logs, and graphical artifact line ranges; residual risks and smallest fixes are stated."
    }
  ],
  "changedFiles": [
    "docs/spikes/artifacts/0032-native-background-blur/review/final-review.md"
  ],
  "testsAddedOrUpdated": [],
  "commandsRun": [
    {
      "command": "git status --short; git diff/stat/show inspections for 1e233a1..7bd8c49 and 7bd8c49..d2affaf",
      "result": "passed",
      "summary": "Reviewed requested range and current Slice 7 files; confirmed d2affaf is documentation-only and binary inputs are unchanged."
    },
    {
      "command": "cd docs/spikes/artifacts/0032-native-background-blur && sha256sum --check SHA256SUMS",
      "result": "passed",
      "summary": "All 15 retained artifact entries verified."
    },
    {
      "command": "git diff --check",
      "result": "passed",
      "summary": "Current tracked Slice 7 diff has no whitespace errors."
    },
    {
      "command": "local JSON parse and Markdown link checks",
      "result": "passed",
      "summary": "Five JSON summaries parsed and local links in the Plan/Slice 7 documentation resolved."
    },
    {
      "command": "cargo clippy --workspace --all-targets -- -D warnings",
      "result": "not-run",
      "summary": "Not rerun by instruction; retained Rust 1.97 log records failure on replace_box and collapsible_match diagnostics."
    },
    {
      "command": "full workspace tests and graphical/resource matrix",
      "result": "not-run",
      "summary": "Expensive suite and graphical commands were prohibited; retained logs/evidence were reviewed instead."
    }
  ],
  "validationOutput": [
    "Retained generator validation: 1 passed, 34 deselected.",
    "Retained Splinterm library validation: 185 passed.",
    "Retained formatting, serial workspace suite, links, hashes, and diff checks: passed.",
    "Retained strict workspace Clippy: failed on Rust 1.97 baseline diagnostics.",
    "Graphical artifacts: guarded smoke, eight Splinterm protocol cases, separate Foot reference, and cleanup retained; stale matrix frames excluded from acceptance."
  ],
  "residualRisks": [
    "Strict INI alpha parsing accepts NaN and can incorrectly make native blur eligible.",
    "Opaque/blur-disabled startup RSS and settled graphical idle behavior lack retained measurements.",
    "Strict workspace Clippy does not satisfy the written Plan 0013 completion criterion.",
    "User-facing configuration and ADR text do not explicitly disclose staging-protocol status."
  ],
  "noStagedFiles": true,
  "diffSummary": "Read-only review of Plan 0013 implementation/evidence; no project/source edits, one required review artifact created.",
  "reviewFindings": [
    "blocker (product): crates/splinterm/src/config.rs:296-299,393-399,546-548 - non-finite INI alpha bypasses strict validation and may activate blur.",
    "blocker (policy): docs/plans/0013-native-background-blur.md:414-420,484-499 - exact strict Clippy gate is required but retained evidence fails.",
    "blocker (evidence): docs/plans/0013-native-background-blur.md:430-441,495 - retained artifacts do not measure startup RSS or settled idle behavior.",
    "blocker (documentation): docs/configuration.md:39-44 and docs/adr/0004-font-and-cpu-renderer.md:133-137 - staging-protocol limitation is omitted."
  ],
  "manualNotes": "Release decision: Not approved. No graphical command, capture, compositor test, full suite, Clippy rerun, or subagent was launched."
}
```
