# Review disposition

The product/readability reviewer accepted the documentation boundary without a
finding.

The technical review's sole finding was corrected directly in `docs/cli.md` and
`docs/usage.md`. Both now state the current dispatcher contract exactly:

- human `kill` and `reset` prompt unless `--yes`;
- human `close` and `close-dojo` remove only already-exited topology without an
  additional interactive prompt; and
- machine `kill`, `close`, and `close-dojo` require `--yes`.

The correction matches `crates/splinterm/src/app/cli.rs`: `Close` and `CloseDojo`
dispatch directly, while `Kill` calls `confirm_kill` and reset owns its separate
confirmation workflow.

Focused post-review link, terminology, formatter, metadata, CLI-help, site, diff,
and index validation passed. No reviewer finding remains unresolved. Plan 0021
may be marked complete.
