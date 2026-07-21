# Splinterm Agent Guardrails

## Cost and delegation stop-loss

- Never automatically retry, resume, or replace a failed, timed-out, or incomplete subagent. Stop and report to the user.
- Before an approved launch, state its exact task, expected files, and validation command.
- After a launch, immediately report the agent name, scope, and outcome.

## Implementation stop-loss

- Split work into small, dependency-ordered changes with validation after each change.
- Before editing, state the immediate change and expected validation.
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
