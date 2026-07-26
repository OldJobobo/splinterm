# Splinterm Agent Guardrails

## 1. Cost and Delegation Stop-Loss

### 1.1 Routine Subagent Authorization

A user request that authorizes implementation, review, investigation, or delegated work also authorizes the routine subagent launches required to complete that request.

For every subagent launch:

* Announce the launch.
* State the subagent’s role.
* State its bounded task.
* Do not wait for separate user confirmation unless Section 1.2 applies.

### 1.2 Subagent Launches Requiring Approval

Ask the user before launching a subagent if the launch would:

* Introduce a new concurrent writer.
* Materially expand the scope.
* Materially increase the expected cost.
* Perform graphical testing.
* Require a destructive or irreversible action.

### 1.3 Failed or Incomplete Subagents

If a subagent fails, times out, or returns incomplete work:

* Do not automatically retry it.
* Do not automatically resume it.
* Do not automatically replace it.
* Stop and report the result to the user.

### 1.4 Worktree Writers

Keep the active worktree single-writer by default.

The following work may run in parallel without creating additional writers:

* Read-only scouting.
* Read-only review.
* Read-only validation.

Intentionally concurrent writers must:

* Use separate worktrees.
* Receive user approval before launch.

### 1.5 Default Agent and Review Limits

For ordinary implementation, use no more than:

* One scout or planner.
* One writer.
* Two fresh, read-only reviewers.
* One fix writer, when justified.
* Two review rounds.

Ask the user before exceeding any of these limits.

### 1.6 Launch Reporting

Before launching a subagent, state:

* The subagent’s role.
* Its bounded task.
* The expected files or area of responsibility.
* The validation command.

After launch, report:

* The subagent’s scope.
* The subagent’s outcome.

Do not introduce an additional confirmation gate after launch.

## 2. Implementation Stop-Loss

### 2.1 Milestones and Validation

Split implementation work into small, dependency-ordered milestones.

Validate after each coherent milestone. Do not require validation after every individual edit.

### 2.2 Pre-Edit Notice

Before editing, state once:

* The immediate coherent change.
* The expected validation for that change.

Do not repeat this notice for every small edit within the same coherent change.

### 2.3 Actions That Do Not Require Separate Approval

Separate user approval is not required for the following actions when they remain within the authorized scope:

* Routine file reads.
* Routine file edits.
* Non-graphical tests.
* Validation commands.

### 2.4 Actions That Require User Input or Approval

Ask the user when the work requires:

* A product decision.
* A scope decision.
* A destructive or irreversible action.
* Publishing or pushing changes.
* Graphical testing governed by Section 3.
* An unexpected, material increase in cost.
* An unexpected, material increase in agent fanout.

### 2.5 Failed Expensive Commands

Do not repeat a failed, expensive command until both of the following have occurred:

* Diagnose the failure.
* Explain the next bounded attempt.

### 2.6 Graphical Test Matrices

For a graphical test matrix:

1. Run one guarded test case.
2. Confirm that the guarded smoke test succeeds.
3. Run the full matrix only after that success.

## 3. Graphical Test Isolation

### 3.1 Required Test Location

Run graphical tests only on:

* Workspace: `8`
* Monitor: `DP-2`

Workspace 8 must be inactive when graphical testing begins.

### 3.2 Prohibited Effects

Never:

* Switch the user to workspace 8.
* Focus a test window.
* Map a test window to any other workspace.
* Map a test window to any other monitor.

### 3.3 Placement and Focus Controls

Use:

* Pre-map placement rules.
* No-focus rules.

If any placement or focus violation occurs:

1. Abort the graphical test immediately.
2. Clean up the test immediately.

### 3.4 Non-Graphical Commands

The following commands may run normally from the repository because they do not launch windows:

* Non-graphical build commands.
* Lint commands.
* Unit-test commands.

## 4. Repository Safety

### 4.1 Foot Oracle Authority

Preserve the pinned Foot 1.27.0 commit as the oracle authority:

`3c5b584b0eafa772eb4376fb6eaf6643399e190e`

### 4.2 Prohibited Repository Changes

Do not:

* Modify the canonical Foot checkout.
* Translate comparison images.
* Broadly widen tolerances.
* Silently regenerate references.

### 4.3 Completion Claims

Do not claim that a slice is complete unless both of the following exist:

* Recorded validation evidence.
* Recorded review.
