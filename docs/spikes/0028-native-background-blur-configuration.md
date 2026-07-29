# Spike 0028: native background-blur configuration

- **Status:** Complete
- **Date:** 2026-07-28
- **Plan:** [Native Wayland background blur](../plans/0013-native-background-blur.md)
- **Scope:** Plan 0013 Slice 1 only

## Configuration and schema

Splinterm now accepts strict `[colors] blur` booleans using the existing
`yes`/`no`, `true`/`false`, `on`/`off`, and `1`/`0` vocabulary. Invalid values
retain the parser's line-numbered startup failure. Repeated assignments remain
explicitly last-assignment-wins.

Generated `theme.json` includes a strict JSON boolean `blur`. Older generated
themes default it to `false`; a present non-boolean value fails validation.
`ResolvedTheme` carries alpha and requested blur together, and explicit user
values override their generated-theme counterparts independently.

This slice propagates requested blur as inert presentation state only. It does
not bind the Wayland protocol, create an effect object, or claim working native
blur; those behaviors remain gated by later plan slices.

## Omarchy selection

`tools/generate-omarchy-theme.py` reads alpha and blur from one deterministic
Foot section:

1. `[colors-dark]` when that section exists;
2. otherwise legacy `[colors]`; and
3. never `[colors-light]`.

Within the selected section, later alpha and blur assignments replace earlier
ones. Missing alpha and blur values default to `1.0` and `false` without being
filled from another section. Both values are validated before the generator's
existing temporary-file plus `os.replace` publication, so a theme switch
publishes one complete JSON replacement or leaves the previous file untouched.

## Startup and live reload

Single- and multi-pane launches now share the same startup resolver. A malformed
existing theme produces one diagnostic and one safe fallback theme; explicit
user alpha and blur overrides remain applied to that fallback.

Both launch modes also share one watcher reducer. A rejected live file leaves
the previous `ResolvedTheme` unchanged. Repeated invalid replacements emit one
diagnostic for the rejection episode; a valid accepted file resets that bound.
A multi-pane window fans one accepted theme value to its existing pane update
channels, preserving one alpha/blur decision for the whole window.

The watcher still communicates only `ResolvedTheme` values to the Wayland owner.
It does not manipulate Wayland proxies from its asynchronous task.

## Validation

Passed:

```bash
python -m pytest -q tools/benchmark/test_benchmark.py -k omarchy_theme_generator
python -m py_compile tools/generate-omarchy-theme.py tools/benchmark/test_benchmark.py
cargo test -p splinterm --lib -- --test-threads=1
cargo test -p splinterm --bin splinterm -- --test-threads=1
cargo fmt --all --check
git diff --check
```

Results:

- focused generator: 1 passed;
- Splinterm library: 173 passed;
- Splinterm binary: 25 passed; and
- formatting, Python compilation, and whitespace checks: passed.

Strict Clippy with Rust 1.97 remains blocked by the pre-existing warnings
recorded in Spike 0027. The command was not repeated after that diagnosed
failure; this slice does not edit the previously reported warning sites or
weaken lint policy.

## Review

A read-only acceptance review found one medium test-coverage gap in the bounded
diagnostic and complete-theme preservation evidence. The parent added an
injectable startup reporter, a pure live-reload rejection diagnostic reducer,
and focused tests for both startup paths, rejection suppression, acceptance
reset, and preservation of the entire prior non-default `ResolvedTheme`.

After the focused and complete Splinterm suites passed again, the bounded final
review approved Slice 1 with no blockers or residual risks.
