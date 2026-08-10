# Technical-accuracy review

- Reviewer run: `fb9f72d9`
- Role: fresh read-only technical-accuracy reviewer
- Initial decision: **must remain in progress** pending one documentation fix

## Finding

`docs/cli.md` and `docs/usage.md` stated that destructive human commands prompt.
That was false for human `close` and `close-dojo`: their parsed `--yes` values are
ignored by the human dispatcher, and both requests proceed without confirmation.
Only human `kill` calls `confirm_kill`; `reset` owns its separate confirmation.

The blanket claim overstated the interactive safety boundary for topology
removal. The reviewer requested exact wording: human `kill` and `reset` prompt
unless `--yes`; `close`/`close-dojo` remove already-exited topology without an
interactive prompt; machine variants require `--yes`.

No other actionable blocker or fix worth doing now was reported.
