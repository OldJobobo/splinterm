# ADR 0013: Make Splinterm-owned contracts the renderer release authority

- **Status:** Accepted
- **Date:** 2026-09-04
- **Supersedes:** The release-authority portions of [ADR 0004](0004-font-and-cpu-renderer.md)

## Context

Foot 1.27.0 was an effective bootstrap reference while Splinterm established its
terminal semantics and CPU renderer. It exposed concrete compatibility defects
and produced the source fixtures and exact raster evidence from which the current
implementation was built.

The pinned-host harness later accumulated requirements that did not themselves
define observable renderer behavior. In particular, Cargo lockfile identity,
Rust compiler version, and a hash of every active Fontconfig file could reject a
run even when the selected font files, native raster stack, and compared output
were unchanged. Requiring that environment as a release prerequisite made
reference-host availability, rather than Splinterm behavior, a release gate.

Splinterm now has source-owned semantic vectors, renderer unit and integration
tests, exact comparators, package validation, and guarded graphical acceptance.
Those contracts can identify product regressions directly.

## Decision

Splinterm-owned tests and fixtures are the renderer release authority.
Mandatory CI and candidate construction require:

- workspace tests, including terminal semantics and renderer behavior;
- adopted source fixture vectors, contract validation, and graphical workspace safety guards;
- exact Splinterm pixel, geometry, cache, generation, and fallback regressions;
- package and release automation; and
- guarded packaged graphical acceptance when the changed behavior requires it.

Foot remains a pinned **optional historical differential** at commit
`3c5b584b0eafa772eb4376fb6eaf6643399e190e`. Its fixtures, adapter patches,
comparators, zero-tolerance policy, and accepted evidence remain intact. The
portable tooling job and standalone pinned-host workflow may report defects,
but their failure or unavailability does not fail the aggregate release check and does not
block candidate construction or promotion.

The optional pinned-host preflight checks inputs that can directly affect the
comparison: the Foot revision and adapter patches; resolved font paths, face
indexes and hashes; their resolved Fontconfig hinting, antialiasing, subpixel,
and LCD-filter options; Fontconfig, FreeType, fcft, and pixman versions; renderer
policy; and explicit environment variables. It does not require Cargo.lock or a
particular Rust compiler, because the comparison judges the produced output. It
does not hash the complete ambient Fontconfig inventory after all resolved face
identities have been checked.

Existing fixtures marked `oracle_verified` retain that provenance, but are
adopted as versioned Splinterm regression contracts. Their current contents do
not become mutable expectations. Changes still require explicit review; tools
must never translate images, widen tolerances, or silently regenerate accepted
references.

Foot should normally be rerun when a change intentionally modifies terminal
semantics, glyph rasterization, placement, cell geometry, fallback behavior,
decoration or cursor composition, or the accepted fixture corpus. Maintainers
may also use it diagnostically at any time.

## Consequences

- Ordinary source, dependency, and release-metadata changes no longer require a
  matching Foot host or Cargo-lock provenance refresh.
- A Foot differential failure is useful evidence but must be triaged against the
  Splinterm-owned contract before it can block a release.
- Deliberate renderer changes remain subject to exact output review and guarded
  graphical acceptance; making Foot advisory does not weaken those checks.
- Historical evidence stays reproducible without granting an external program
  permanent authority over Splinterm's release lifecycle.
