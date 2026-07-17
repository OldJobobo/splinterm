# Contributing

Splinterm is pre-alpha. Keep changes small, preserve crate boundaries, and add
tests for domain or protocol behavior.

Before submitting a change, run:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

When adapting code from Foot or another project, verify license compatibility,
preserve required notices, and update `THIRD_PARTY.md` with exact provenance.
