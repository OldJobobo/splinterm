# Splinterm Agent Guardrails

## 0. Branch-First Repository Workflow

### 0.1 Main Worktree Is Coordination-Only

The repository-root worktree on `main` is reserved for fetch, status inspection,
review, approved integration, and release operations. Do not begin task-file
mutations or task commits there; approved integration and release-boundary
commits remain permitted.

Before the first edit for an authorized task:

* Fetch the current remote state.
* Create a short-lived branch from the reviewed `origin/main` base for 0.2 and
  general work, or from `origin/maint/0.1` for an explicitly authorized 0.1
  maintenance patch.
* Create or use a dedicated worktree for that branch.
* Confirm the branch and worktree path before editing.

Use milestone-oriented names such as `feat/binding-help-search`,
`fix/font-reload-race`, or `docs/configuration-font-sync`. Do not use one long-lived branch for all of a
release program.

If pre-existing changes are found on `main`, partition and preserve them before
creating the task worktree. Never sweep unrelated tracked or untracked files
into the new branch, stash, commit, move, or delete them without establishing
ownership. If ownership cannot be established, do not alter the changes; stop
and obtain an ownership decision.

### 0.2 One Writer Per Branch and Worktree

One writer owns each task branch and worktree. Read-only scouting, review, and
validation may inspect that worktree. Review fixes return to the same writer and
branch by default.

Concurrent writers still require the approval in Section 1.2 and separate
branches and worktrees. Branch isolation does not authorize uncontrolled
parallel edits. Serialize dependent milestones and work that overlaps known
convergence points such as `wayland.rs`, `keymap.rs`, `action_menu.rs`, or
shared release and package files.

### 0.3 Pull Request and Merge Boundary

Each coherent branch must pass its task's focused checks, the appropriate
non-graphical boundary, actual-diff inspection, `git diff --check`, and required
independent review before merge. Record exact validation and residual risks in
the pull request.

Prefer squash merge so the owning integration branch receives one coherent
milestone commit. Merge 0.2 and general work into `main`; merge 0.1 maintenance
patches into `maint/0.1` and forward-port applicable fixes to `main` through a
separate reviewed branch. Delete the merged task branch and remove its worktree
after verifying the merge. Release candidates, promotions, tags, and publication
may originate only from `main` or `maint/0.1`; the candidate and promotion must
use the same authority branch and retain their separate approval requirements.

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

Distinguish execution failure from review outcome:

* An execution failure means the subagent timed out, crashed, could not access its
  required inputs, exhausted its tools before producing the requested result, or
  returned incomplete work.
* A reviewer finding defects or rejecting a milestone is a successful review, not
  a failed subagent.

If a reviewer successfully returns actionable findings:

* Apply in-scope fixes directly in the single-writer parent without asking for
  another approval.
* Run the already-authorized non-graphical validation.
* Request user input only when a finding triggers Section 1.2 or Section 2.4.
* Do not describe the review itself as an agent failure.

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
* One review rounds.

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

### 1.7 Subagent Launch-Readiness Gate

Do not use a subagent as a substitute for the parent’s first-pass debugging,
implementation, or validation. Before every reviewer or verifier launch, the
parent must confirm all of the following:

* The coherent milestone is implemented rather than partially sketched.
* Relevant focused tests, formatters, linters, and `git diff --check` pass, or the
  prompt explicitly identifies and bounds a known failure under review.
* The parent has inspected the actual diff and handled obvious missing cases,
  error paths, cleanup paths, and requirement mismatches.
* Every referenced file and artifact exists and is readable.
* The task prompt names exact files, exact evidence, constraints, expected
  decision, and commands the agent may run.
* The agent has enough turn and tool budget to inspect the stated scope and
  produce a complete result. Do not impose a soft budget below the realistic
  read/validation workload.
* Read-only agents receive the evidence directly or an explicit bounded read
  list; do not make them rediscover broad historical context.

If this gate is not satisfied, keep working in the parent. Do not launch the
reviewer yet.

### 1.8 Review and Watchdog Reliability

* Launch fresh review only at a coherent acceptance boundary, not after the first
  draft of a fix.
* Ask reviewers for blockers and fixes worth doing now; do not ask them to
  speculate beyond the bounded milestone.
* A review rejection authorizes the parent to make bounded fixes and validate
  them under the original request. It does not create a new user confirmation
  gate.
* Before treating a watchdog warning as a missed authorization, reconstruct the
  approval chain from the full conversation, including user messages immediately
  before and after the narrow event delta. Do not blindly accept a warning that
  omits a relevant approval.
* Report a false-positive watchdog warning as such and continue only within the
  authorization actually granted.

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

   Request approval once for the complete bounded graphical sequence, identifying
   the target window, permitted focus/input actions, smoke test, gated matrix, and
   cleanup plan.

   Run one guarded smoke case first. If it succeeds, continue with the approved


## 3. Graphical Testing

### 3.1 Allowed Test Targets

Graphical tests may use either:

* An isolated test window on workspace 8 / DP-2.
* An existing active user window when the user explicitly approves a bounded
  sequence and identifies the intended window.

### 3.2 Active-Window Authorization

After explicit approval, the agent may focus, raise, resize, and send bounded
keyboard or pointer input to the identified active window. Authorization applies
only to the named test sequence and target window.

Before manipulating an active window:

* Record its address, process ID, workspace, monitor, and current focus.
* State the exact actions and expected cleanup.
* Target actions by window address whenever possible.
* Preserve unrelated windows and user processes.

Do not close the window, terminate its shell, restart its daemon, move unrelated
windows, or enter terminal commands unless those actions were explicitly
approved.

### 3.3 Abort and Cleanup

Abort immediately if input reaches the wrong window, an unrelated window moves,
or the approved target cannot be identified reliably.

After testing, restore the original focus, workspace, monitor, size, and position
when practical, and report anything that could not be restored.

### 3.4 Isolated Testing

When active-window manipulation was not explicitly approved, retain the existing
workspace 8 / DP-2 placement and no-focus isolation requirements.

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

### 4.4 Local Client Installation and Trusted-UI Identity

The daemon grants the graphical trusted-UI bypass only when the client process
matches the device and inode of the `splinterm` binary adjacent to the running
`splinterd` executable. On the packaged system, that authority is
`/usr/bin/splinterm` next to `/usr/bin/splinterd`.

Do not install a development client to `~/.local/bin/splinterm`,
`~/.cargo/bin/splinterm`, or another earlier `PATH` entry and then claim the
desktop launcher is installed. The packaged desktop wrapper resolves
`splinterm` through `PATH`; a shadowing user-local copy does not match the
system daemon's trusted-UI inode and exits as unauthorized before mapping a
window.

For local installation work:

* First inspect `command -v splinterm`, the desktop-launcher process `PATH`, the
  running daemon executable, and both executable device/inode identities.
* Remember that `./install.sh` deliberately packages only a clean committed
  `HEAD`; it cannot install an uncommitted worktree implementation.
* Ask before replacing the Pacman-owned `/usr/bin/splinterm`. When approved,
  save a rollback copy, use `pkexec` for the privileged replacement, remove any
  shadowing user-local client, and disclose that Pacman integrity checks will
  report the file as altered until reinstall or upgrade.
* Reopen existing Splinterm windows after replacement. Their running executable
  retains the old inode and no longer matches the newly installed trusted UI.
* Validate non-graphically with normal human-mode `/usr/bin/splinterm list`,
  checksum and ownership checks, `command -v splinterm`, and
  `desktop-file-validate`. Do not use `--output json list` as the trusted-UI
  check: machine mode intentionally uses the automation role and may be denied
  without a persistent policy even when installation is correct.
* Do not launch the desktop entry as validation unless the user has separately
  approved the guarded graphical test sequence in Section 3.
