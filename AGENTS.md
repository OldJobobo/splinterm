# Splinterm Agent Guardrails

## Cost and delegation stop-loss

- A user request authorizing implementation, review, investigation, or delegated work also authorizes routine subagent launches needed to complete that request. Announce each launch with its role and bounded task, but do not wait for separate confirmation.
- Ask before launching only when it introduces a new concurrent writer, materially expands scope or expected cost, performs graphical testing, or requires a destructive or irreversible action.
- Never automatically retry, resume, or replace a failed, timed-out, or incomplete subagent. Stop and report to the user.
- Keep the active worktree single-writer by default. Parallelize read-only scouting, review, and validation; isolate intentionally concurrent writers in separate worktrees only with user approval.
- For ordinary implementation, default to at most one scout or planner, one writer, two fresh read-only reviewers, one fix writer when justified, and two review rounds. Ask before exceeding this envelope.
- Before launch, state the agent role, bounded task, expected files or area, and validation command. After launch, report its scope and outcome without adding a confirmation gate.

## Implementation stop-loss

- Split work into small, dependency-ordered milestones with validation after each coherent milestone rather than after every individual edit.
- Before editing, state the immediate coherent change and expected validation once; do not repeat the ceremony for each small edit within that change.
- Routine reads, edits, non-graphical tests, and validation commands within the authorized scope do not require separate approval.
- Ask the user only for product or scope decisions, destructive or irreversible actions, publishing or pushing, graphical tests covered below, or unexpected material expansion of cost or fanout.
- Do not repeat a failed expensive command without first diagnosing the failure and explaining the next bounded attempt.
- For graphical matrices, run one guarded case first. Run the full matrix only after the one-case smoke test succeeds.

## Graphical test isolation

- Run graphical tests only on inactive workspace 8 on DP-2.
- Never switch the user to workspace 8, focus a test window, or map a test window on another workspace or monitor.
- Use pre-map placement and no-focus rules. Abort and clean up immediately on any placement or focus violation.
- Non-graphical build, lint, and unit-test commands do not launch windows and may run normally from the repository.

## Repository safety

- Preserve pinned Foot 1.27.0 commit `3c5b584b0eafa772eb4376fb6eaf6643399e190e` as oracle authority.
- Do not modify the canonical Foot checkout, translate comparison images, widen tolerances broadly, or regenerate references silently.
- Do not claim a slice complete without recorded validation evidence and review.
