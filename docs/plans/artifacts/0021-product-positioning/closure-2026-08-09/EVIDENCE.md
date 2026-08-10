# Plan 0021 closure evidence — 2026-08-09

## Scope and baseline

This closure begins at commit `a65a740` and implements documentation/product-
metadata work only. [`BASELINE.md`](BASELINE.md) records eight native-Wayland
website paths that were already uncommitted before Plan 0021 implementation.
They were preserved byte-for-byte and are excluded from the Plan 0021 patch and
future staged commit. The retained site validation therefore checks the combined
worktree and does not claim ownership of those edits.

No graphical testing, installation, publishing, protocol change, or product-
behavior change was required or performed.

## Repository authority delivered

- [`docs/status.md`](../../../../status.md) is the current maturity authority. It
  defines advanced private prerelease, validated environment, a capability truth
  table, limitations, release gates, classification vocabulary, and one authority
  per detailed subject.
- [`docs/usage.md`](../../../../usage.md) owns human operation: persistent concepts,
  new/detach/reopen flows, tabs, picker, panes, controls, pointer/selection,
  history, control ownership, restore/relaunch, reset, remote use, and
  configuration.
- [`docs/cli.md`](../../../../cli.md) owns the human command inventory and common
  cookbook while explicitly deferring stable JSON/NDJSON schemas, policy,
  cancellation, limits, and exit contracts to `automation.md`.
- [`CONTRIBUTING.md`](../../../../../CONTRIBUTING.md) now owns isolated-daemon work,
  standard/focused validation, benchmark calibration, the pinned Foot oracle,
  bounded fuzzing, graphical-test authorization/cleanup, package safety, and
  evidence/review expectations.

README status links now target repository authority, and its documentation map
links human usage, CLI, and contributing destinations. The PRD names those
sources and no longer calls the implemented website proposed. The roadmap is a
completion ledger plus forward plan; pre-planning research has an archival
banner without rewriting historical content.

## Truth and positioning audit

Current public surfaces consistently use:

> **A persistent, security-conscious terminal substrate for humans and bounded
> automation.**

and the maturity label:

> **advanced private prerelease**

The repository-authoritative truth table distinguishes implemented, validated,
supported documented subsets, planned, deferred, and unreleased behavior. It
specifically binds Window-local Dojo tabs to the accepted Plan 0019 closure and
retains the unresolved Plan 0011/0012 performance boundaries rather than
presenting them as completed.

Distribution wording is synchronized to the approved short description:

> Persistent Wayland terminal for humans and bounded automation

in `packaging/PKGBUILD`, the desktop comment, AppStream summary, and the primary
`splinterm` Cargo description. The daemon retains the package-specific
“Persistent terminal topology daemon for Splinterm” wording rather than
misrepresenting itself as the graphical product.

## Command and control accuracy

The CLI inventory was checked against current-source
`cargo run --locked --quiet -p splinterm --bin splinterm -- --help`: all 39
current top-level commands are represented. The effective built-in keymap output
was retained and matches the documented session, tab, pane, history, control,
clipboard, and view actions.

During parent inspection, the Plan/scout form
`makepkg --printsrcinfo -p packaging/PKGBUILD` was found invalid for the installed
`makepkg`; contributor documentation records the verified form
`(cd packaging && makepkg --printsrcinfo -p PKGBUILD >/dev/null)`.

The docs preserve these critical boundaries:

- Window/tab close detaches and does not terminate daemon topology;
- restore is explicit and saved argv never executes automatically;
- stable Splint IDs are distinct from positive process incarnations;
- topology commands do not control compositor Windows;
- machine clients do not inherit human graphical authority;
- graphical/local-administration commands are not falsely presented as public
  machine schemas;
- raw daemon protocol/Rust DTOs remain private;
- terminal content remains untrusted data; and
- native Wayland is not claimed as GPU rendering, universal compatibility,
  absolute security, or automatic performance superiority.

## Validation

Exact outputs are retained under `validation/`:

- current-source top-level help and 39-command inventory: passed;
- effective built-in keymap/action comparison: passed;
- 164 local links across ten changed authority/evidence documents: passed;
- `desktop-file-validate`: passed;
- `appstreamcli validate --no-net`: passed;
- `(cd packaging && makepkg --printsrcinfo -p PKGBUILD)`: passed;
- `cargo metadata --no-deps --format-version 1`: passed;
- final combined-worktree `site/npm run validate`: 0 Astro diagnostics, 14 pages
  built, 356 local page/asset links valid; concurrent site additions are bounded
  in `BASELINE.md` and remain outside this closure patch;
- `cargo fmt --all -- --check`: passed;
- current-surface stale terminology scan: passed;
- `git diff --check`: passed; and
- `git diff --cached --quiet`: passed before review.

## Completion mapping

1. README does not call the current product proposed: passed.
2. Product sentence leads current public positioning: passed.
3. Advanced private prerelease is consistent: passed.
4. Implementation maturity and public availability are distinct: passed.
5. Persistence and bounded shared human/automation access lead: passed.
6. `docs/status.md` owns current status: passed.
7. Detailed usage, CLI, and development have authoritative homes: passed.
8. Roadmap and historical research have current framing: passed.
9. Distribution metadata is aligned and syntactically valid: passed.
10. Links, commands, site, package, formatter, and diff checks pass: passed.
11. Product/readability review: reviewer `93d8c0d2` found no actionable issue and
    concluded **may mark complete**.
12. Technical-accuracy review: reviewer `fb9f72d9` found one inaccurate blanket
    prompt claim. `docs/cli.md` and `docs/usage.md` now distinguish human
    `kill`/`reset` prompts, non-prompting human `close`/`close-dojo` on exited
    topology, and machine `--yes`; focused validation passed.
13. No security, compatibility, platform, availability, or feature claim is
    intentionally overstated: independently confirmed after the retained review
    disposition.

Both required fresh read-only reviews are recorded under `review/`; no finding
remains unresolved. Plan 0021 may be marked complete.
