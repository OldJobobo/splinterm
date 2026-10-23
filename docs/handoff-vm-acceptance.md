# Live-upgrade VM acceptance (Plan 0061 M3.17)

This is the bounded graphical sequence that proves live daemon upgrade in a
real guest. It was written to pass before live upgrade was enabled, and it is
the acceptance run for a release that ships it. The coordinator shows it to the
operator,
and **nothing in it runs without the operator's explicit approval of the
whole sequence** (AGENTS.md §2.6 and §3), including the guest package
replacement it performs (§4.4).

`tools/testbed/handoff-vm-acceptance.py` holds the same sequence as data. It
prints every step as the exact `tools/testbed/omarchy-vm.sh` invocations it
makes, and runs nothing by itself:

```bash
tools/testbed/handoff-vm-acceptance.py plan                       # every step
tools/testbed/handoff-vm-acceptance.py show c3-proceed-adopt      # one step
tools/testbed/handoff-vm-acceptance.py run c3-proceed-adopt --operator-approved
```

`run` executes one step, stopping at its first failing command. The tester
runs the steps in the order below and judges each step's pass condition from
its output and captures. Every guest script starts with the helper's prelude,
which locates the private package session that `package-launch` owns
(`$runtime/splinterd.sock`) and the evidence directory
`.testbed-package/evidence/handoff`.

## What is tested, and with what

Live upgrade is enabled in every shipped build since the enablement change
(below): `WINDOW_CONTINUITY_READY` is `true` (operator decision 2026-10-22), so
a plain local package (`package-build`, then `package-install
--confirm-guest-install`) performs a real live upgrade, and the sequence runs
on plain packages A and B.

**Acceptance status.** Accepted. The operator enabled live upgrade on
2026-10-22, and on 2026-10-23 the full sequence passed on plain packages of
4b7210a7, every step through this helper: c1 3, c2 3, c3 0, c5 4, r3 7 with the
daemon rolled back, r4 4, and x1 4. An earlier run on TEST-ONLY packages of
023f7769 had passed every product step except r3's exit code, fixed by #202.

Before enablement, the operator approved (2026-10-19) a **test-only build that
forces readiness**, and it stays available:

- `splinterd`'s `vm-acceptance-live-handoff` feature makes live handoff treat
  window continuity as ready for a local package build
  (`local-committed-head`) only. Since enablement every build already does, so
  the feature changes no decision; Nix-store builds stay refused in it as in
  every build. The daemon logs a warning at every serve start,
  "TEST-ONLY VM acceptance build", at the `warn` level; `package-launch` runs
  the private daemon with `RUST_LOG=info`, so its `daemon.log` carries this
  and the handoff phase lines the steps check (a plain package logs the
  handoff phase lines without the warning).
- Such a daemon carries a marker. Package provenance generation and package
  validation refuse it in every release-candidate and Nix package, and in a
  local package unless explicitly requested. No PKGBUILD, Nix recipe,
  installer, or workflow enables the feature, and `upgrade-local-package.sh
  --allow-vm-acceptance-live-handoff` refuses to run outside the VM testbed
  (`SPLINTERM_TESTBED_PACKAGE_ROOT` unset). `package-status` warns while a
  TEST-ONLY build is installed.
- `omarchy-vm.sh package-build --vm-acceptance-live-handoff` and
  `package-install --confirm-guest-install --vm-acceptance-live-handoff`
  build and install such a package in the guest. The package description
  reads "TEST-ONLY VM acceptance build: never ship".

**Plain or TEST-ONLY packages.** `tools/testbed/handoff-vm-acceptance.py`
builds and installs **plain** packages A and B by default, so the sequence
proves the package users install. With
`SPLINTERM_HANDOFF_VM_PACKAGES=test-only` it builds TEST-ONLY packages instead
(`--vm-acceptance-live-handoff` on every `package-build` and
`package-install`), and `s1-dry-run` then expects the TEST-ONLY warning in the
daemon log; for plain packages it checks that `/usr/bin/splinterd` carries no
marker. Every other pass condition is the same. The first build step records
the kind in the private clone, and every later build or install step refuses a
different one, so one run never mixes them. Which kind a recorded session uses
is the operator's choice when approving it. `z2-plain-package` is mandatory
either way.

Two packages are needed, because a live upgrade needs an installed build whose
pair identity differs from the running one:

- **Package A** is the candidate commit.
- **Package B** is the same tree plus one empty commit, made only in the
  private clone and never pushed. The commit is part of the pair identity, so
  B is a different build with the same code.

## Where, who, and limits

- **Guest:** wintermute's persistent Omarchy VM (user `omarchy`, Virtual-1,
  SSH 127.0.0.1:2222). The operator boots it; nothing here starts or stops
  the VM.
- **Runner:** a private clone of the candidate on wintermute under
  `~/.cache/splinterm-builds/vm-acceptance/<commit>`, never an operator
  checkout. Run as `SPLINTERM_TESTBED_CONFIG=<testbed env>
  tools/testbed/handoff-vm-acceptance.py …`. The helper refuses to make
  package B's commit anywhere else.
- **Windows:** the packaged test Window W1 from `package-launch`, and one
  extra managed Window W2 from `package-launch-extra launch`, both on guest
  workspace 8 / Virtual-1. The upgrade dialog ("Splinterm — Upgrade") appears
  as a short-lived extra Window there too.
- **Identity:** before any input, `s1-record` records W1's address, PID,
  workspace, monitor, geometry, the current focus, cursor, scale, and
  transform. After a live upgrade a Window keeps its PID but maps a new
  surface, so the steps re-record its address by PID.
- **Permitted input, only on recorded addresses:**
  - focus by `address:` (`hyprctl dispatch`);
  - keyboard through `omarchy-vm.sh input key|type` (ydotool);
  - captures with `grim -o Virtual-1`.
  - There is no pointer input, no host input, and no QEMU-viewer input.
- **Input guard:** every input batch first focuses the recorded address and
  waits. Then a guard runs directly before **and** directly after every
  single `input` action, with no pause in between: it requires
  `hyprctl -j activewindow` to report the recorded address, and the step
  aborts otherwise. That catches input that reached another window at once.
  After the key that answers the upgrade dialog, the check instead requires
  the dialog to have closed.
- **Guest-wide state changed (recorded first, restored after):**

  | Step | Change | Restore |
  |---|---|---|
  | `p0-install-a`, `u3-package-b`, `r2-setup` | guest package replaced (plain builds, or TEST-ONLY ones with `SPLINTERM_HANDOFF_VM_PACKAGES=test-only`); rollback copies saved under `$PKG/rollback/<ts>` | `z2-plain-package` (mandatory), which checks that no TEST-ONLY package or marker remains |
  | `p0-install-mcp` (optional) | `splinterm-mcp` installed | `sudo pacman -R splinterm-mcp` if `p0-package-status` showed it absent |
  | `r3-rollback` | `/usr/bin/splinterm` replaced by a byte-identical copy (`cp -p`, then rename): a new inode only, so `pacman -Qk` stays clean | the temporary `/usr/bin/.splinterm.vm-acceptance` is removed by `z1-cleanup` if a step aborted between copy and rename; `z2` reinstalls the file |
  | `x1-memfd-noexec` (optional; guest kernel 6.6 or later only) | `vm.memfd_noexec` set to 2 | the recorded value, checked by x1 and again by `z1-cleanup` (which restores it if x1 aborted). On kernels 6.3–6.5 the sysctl can only be raised, so x1 refuses to run there: only a reboot would restore it |
  | `package-install` (any) | the guest's own `splinterd.service`, if active, is stopped and restarted by `upgrade-local-package.sh` | automatic |

- **Duration:** each `package-build` runs the full package check suite (about
  30–60 min, unattended, three builds). The graphical steps take about
  45–60 min, and `x1` adds about 5 min.

## The sequence

The step IDs are the helper's. Each step's full commands are in `plan`.

### Phase 0: preparation (touches the VM; no graphical input)

| Step | Action | Pass |
|---|---|---|
| `p0-status` | `status` | Guest up, Hyprland running, ydotool active |
| `p0-package-status` | `package-status` | Recorded, including whether `splinterm-mcp` is installed and the guest's `splinterd.service` state |
| `p0-sysctl` | `exec`: record `vm.memfd_noexec`, kernel | 0 or 1 |
| `p0-build-a` | record the package kind; `package-build` (with `--vm-acceptance-live-handoff` for TEST-ONLY packages) | Package A builds; for a TEST-ONLY package, validation prints the TEST-ONLY banner |
| `p0-install-a` | check the recorded package kind; `package-install --confirm-guest-install` (with `--vm-acceptance-live-handoff` for TEST-ONLY packages) | Installed, with rollback copies saved; the private install check passes |
| `p0-save-a` | `exec`: copy package A's files outside the guest source checkout | Copied, for the rollback round |
| `p0-install-mcp` (optional, for M1) | `exec`: `sudo pacman -U` package A's `splinterm-mcp` | Installed |

### Phase 1: smoke (must pass before anything else)

| Step | Action | Pass |
|---|---|---|
| `s1-launch` | `package-launch` | W1 maps on workspace 8 |
| `s1-record` | `desktop-exec`: record W1, the daemon, focus, cursor, monitors | Recorded |
| `s1-smoke` | guard, type `echo smoke-ok` | Echoed; no other Window moved. On failure: `package-stop` and abort |
| `s1-dry-run` | `exec`: `splinterm upgrade` | Generation 1, local package build, rollback copy kept, "nothing blocks it"; for plain packages, `/usr/bin/splinterd` carries no TEST-ONLY marker; for TEST-ONLY packages, the daemon log has the TEST-ONLY warning |

### Phase 2: setup

| Step | Action | Pass |
|---|---|---|
| `u1-tabs` | guard; `Ctrl+Shift+D` ×2, `Ctrl+Shift+Tab`, `Ctrl+Shift+Enter`, markers, `Ctrl+Shift+Up/Down` | Three tabs; tab 2 active with two panes; its lower pane focused; `t2-lower-mark` and `t2-upper-mark` visible |
| `u2-extra-window` | `package-launch-extra launch` | W2 maps; its PID and address are recorded; focus returns to W1 |
| `m1-prepare` (optional) | `exec`: `splinterm new mcp-probe -- cat`; `package-config splinterm/policy.json`; `SIGHUP` | A policy rule for `/usr/bin/splinterm-mcp` (its guest SHA-256) over the probe Lair is published |
| `m1-start` (optional) | `exec`: start `tools/testbed/handoff-mcp-probe.py run` | The probe holds an MCP session with a controller handle and waits |
| `u3-package-b` | private-clone empty commit; `package-build` and `package-install …` (with `--vm-acceptance-live-handoff` for TEST-ONLY packages) | B is installed; the same daemon process still serves generation 1 from its replaced inode, and nothing blocks |

### Phase 3: adoption round (daemon A, package B installed)

W2 is stopped (`SIGSTOP`) so that it cannot acknowledge the prepare frame.
That makes the daemon ask the D10 question, which the CLI routes to the
upgrade dialog because it runs outside a terminal.

| Step | Action | Pass |
|---|---|---|
| `c1-cancel-escape` | stop W2; `desktop-exec`: `splinterm upgrade --now` in the background; record the dialog; guard W1 and type `dropped-c1`; guard the dialog, click inside its text (it takes no focus and its keys count only after a click), wait 1 s (past the ~600 ms arming delay), press Escape | The dialog names W2. W1 shows "Upgrading — input paused" and drops `dropped-c1`. The CLI exits 3 ("Upgrade cancelled"). The same daemon serves generation 1 with its rollback copy, and W1 types again (`after-c1`) |
| `c2-cancel-enter` | the same, pressing Enter | Exit 3, nothing changed: Enter never proceeds |
| `c3-proceed-adopt` | the same, pressing G (proceed without W2) | Exit 0. The same daemon PID now runs `/memfd:splinterd-daemon` at generation 2 with no rollback copy, and the log shows `daemon_generation=2` and a restored controller. W1 keeps its PID, runs `/memfd:splinterd-client` (the pinned forward client snapshot), and maps a new surface showing three tabs, tab 2 active, the lower pane focused, both markers, and no `dropped-c3`. `after-c3` typed with no click lands in the lower pane (the controller came back through the resume ticket). The banner clears |
| `c4-stale-client` | `exec`: run W2's executable (`/proc/<pid>/exe`, the pre-upgrade inode) as `list` and `upgrade`; resume W2; `package-close-extra 1` | W2's executable reads `/usr/bin/splinterm (deleted)`. `list` is refused `[unauthorized]`, and `upgrade` exits 1 naming the installed client. The installed client still works. W2 resumes into its lost-connection state (or exits) and is closed |
| `c5-no-second-handoff` | `exec`: `splinterm upgrade --now` | D8: exit 4, "needs a daemon restart first", "Nothing changed"; the same daemon serves |
| `m1-check` (optional) | `exec`: let the probe continue | The old handle fails with `controller_unavailable`, and acquiring again succeeds and its input lands. If re-acquiring is refused because package B replaced the running adapter's binary, record that as an observation for the operator (whether long-running MCP servers must restart after a package upgrade) |

### Phase 4: rollback round (a fresh daemon from package B, package A reinstalled)

A real rollback needs no test seams: if the installed client changes inode
between preparation and adoption, the forward candidate refuses the install
pair (`InstallPrefixMismatch`) and rolls back. The D10 question holds the
attempt open long enough to make that change deterministically.

| Step | Action | Pass |
|---|---|---|
| `r1-restart` | `package-stop`; clear the policy seed; `package-launch`; record | A fresh daemon from package B serves generation 1, keeps its rollback copy, and nothing blocks |
| `r2-setup` | guard; `Ctrl+Shift+D` and `r2-mark`; `package-launch-extra launch`; reinstall package A through `package-install …` | W1 has two tabs; W2 maps; A installs over B (a different pair); the same daemon still serves and nothing blocks |
| `r3-rollback` | stop W2; `splinterm upgrade --now`; record the dialog; replace `/usr/bin/splinterm` by a byte-identical copy (new inode); guard the dialog, click inside its text, wait 1 s, press G | Exit 7: the CLI still runs the replaced inode, so the rolled-back daemon refuses it, and it says it is no longer accepted and the installed splinterm may have been replaced. A fresh `splinterm upgrade` then confirms the rollback: the same daemon PID runs `/memfd:splinterd-daemon` (the sealed rollback snapshot) at generation 1 with no rollback copy. The log shows `phase="adoption"`, the install-prefix refusal, and the rollback. W1 relaunched as the installed `/usr/bin/splinterm`, reconnected (its "Rolled back" notice lasts 3 s, so the capture may miss it), and types again (`after-rollback`). `pacman -Qk splinterm` stays clean |
| `r4-no-second-handoff` | `exec`: `splinterm upgrade --now` | D8 for a rollback daemon: exit 4, "needs a daemon restart first" |
| `r5-close-extra` | resume W2; `package-close-extra 1` | Closed |

### Phase 5: final checks

| Step | Action | Pass |
|---|---|---|
| `f1-exit-codes` | `exec`: collect the recorded exit statuses | c1 = c2 = 3, c3 = 0, r3 = 7; c5 and r4 exited 4 and the stale client 1. Code 6 (rolled back) needs a rollback whose CLI binary was not replaced, which the VM cannot drive; its classification is unit-tested |
| `x1-memfd-noexec` (optional, guest-wide sysctl) | refuse unless the guest kernel is 6.6 or later; `package-stop`; `sysctl vm.memfd_noexec=2`; `package-launch`; dry run and `upgrade --now`; `package-stop`; restore the sysctl | Under `memfd_noexec=2` the kernel refuses the executable memfds (`MFD_EXEC`), so the fresh daemon keeps no rollback copy ("Rollback copy: not kept") and `upgrade --now` exits 4 with nothing changed. The sysctl returns to its recorded value |

### Phase 6: restore and cleanup (AGENTS.md §3.3)

| Step | Action | Pass |
|---|---|---|
| `z1-cleanup` | resume any stopped test process; stop the probe (only if its recorded PID's command line is `handoff-mcp-probe.py`); remove the r3 temporary copy; restore `vm.memfd_noexec` if it differs from the recorded value, and verify; `package-stop` (when the session still runs); check workspace 8 and the monitors; copy the evidence to `target/vm-acceptance/` on the host; `package-config --clear` | `package-stop` stops the adopted or rollback daemon and the relaunched Window too: both run sealed memfd snapshots or replaced inodes, which it now recognises. Its guest-window restore returns workspace, focus, and cursor. Workspace 8 is empty and the monitors match `s1` |
| `z2-plain-package` (mandatory) | return the private clone to the candidate commit (u3 recorded it before package B); `package-build`; `package-install --confirm-guest-install`; `package-status` | The guest leaves the TEST-ONLY build: no TEST-ONLY description and no marker in `/usr/bin/splinterd`. Remove `splinterm-mcp` too if `p0-package-status` showed it absent |

Report anything that could not be restored. The private clone, with package
B's commit, is discarded; nothing is pushed.

### Abort rules (any step)

Abort at once, run `z1-cleanup` and then `z2-plain-package`, and report if any of these happens:

- input reaches any address other than the guarded one;
- an unrelated guest Window moves;
- workspace 8 holds an unexpected Window;
- a recorded Window disappears unexpectedly (other than a relaunch keeping
  its PID);
- the host or the QEMU viewer receives input;
- a step would need an action this plan does not list.

## Not covered here, and where it is proven instead

- **Rollback through the integration-test fault points** (each pre-commit
  step, a pending SIGTERM, a stray descriptor, the guard timeout, rollback of
  a rollback). A package can never contain the integration-test seams:
  provenance generation and package validation refuse their marker, and an
  unpackaged daemon has no rollback authority at all. These rows are proven by
  real-exec tests in `handoff_exec_adoption`, including the M3.17 cases
  `rollback-refuses-claims` (no ticket: `NoSnapshot`), `rollback-guard-timeout`,
  `hold-expiry`, and `old-side-signal-carry`, and the stale-client check in
  `adopt` (the stale inode's trusted-UI `Hello` is refused).
  `r3-rollback` above is the one rollback the VM can drive naturally.
- **The resume hold expiring** (a Window that does not redeem its ticket
  within 30 s) is not driven in the VM. The `hold-expiry` real-exec case
  proves it: a real ticket, redeemed after the hold ended, is invalid, and the
  hold ended with `windows=1 placeholders=1`.
- **The old side's signal carry** (`systemctl stop` during the runtime stop)
  is not driven in the VM. The `old-side-signal-carry` real-exec case proves
  it in a real daemon process.
- **Nix** refuses live upgrade for 0.2.0. The headless NixOS test proves the
  refusal and the restart path (`nix/headless-test.py`).

## The enablement change (done, operator-approved)

**Done** (operator decision 2026-10-22, "enable"): live upgrade is enabled for
0.2.0 by one small change, approved by the operator on its own:

- `crates/splinterd/src/handoff_requests.rs`: `WINDOW_CONTINUITY_READY` is
  `true`; its doc comment and the module's "Enablement" section (formerly
  "Staging") describe live upgrade as enabled.
  `LiveHandoffUnsupported::WindowContinuityPending` stays, naming only a build
  without window continuity, which no build of this source is, so that the
  decision's readiness input is still a real, unit-tested refusal.
- The tests that asserted the shipped decision now assert the enabled one, and
  still cover every refusal that remains (Nix, not packaged, D8, transient
  Lairs, no rollback pair). An unreadable build provenance is refused at the
  exec boundary, and a dry run then fails as an internal error, not as
  `Unsupported`. They no longer need ignoring in
  `cargo test --features splinterd/vm-acceptance-live-handoff`, since that
  build now makes the same decision:
  - in `handoff_requests.rs`:
    `a_packaged_build_supports_live_handoff_now_that_window_continuity_is_ready`
    (formerly `a_packaged_build_is_unsupported_only_until_window_continuity`),
    `an_unpackaged_build_always_reports_not_packaged`,
    `idle_development_daemon_reports_only_not_packaged` (formerly
    `idle_development_daemon_reports_not_packaged_and_unsupported`),
    `transient_lairs_block_with_an_exact_count_and_a_bounded_named_subset`,
    `compiled_build_identity_reports_a_valid_dry_run`, and
    `window_continuity_is_ready_in_every_build_and_only_nix_is_refused`
    (formerly `only_the_vm_acceptance_build_forces_window_continuity`);
  - in `main.rs`: `execute_is_refused_before_the_gate_while_live_handoff_is_unsupported`
    (now for a Nix-store build),
    `the_shipped_execute_reaches_the_gate_for_a_supported_build` (new),
    `dry_run_reports_current_state_and_refused_execute_changes_nothing`,
    `a_second_execute_is_refused_while_a_handoff_runs_and_a_dry_run_still_reports`,
    `an_installed_running_build_cancels_as_blocked_and_says_so`, and
    `execute_runs_the_gate_and_reaches_topology_subscribers_in_sequence`.
  - in the real-exec target `handoff_exec_adoption`, `old-side-signal-carry`
    no longer expects the "live handoff is unsupported" log line: its exec
    stays unarmed only because the drill holds no forward daemon.
- The comments that described the gate as closed, in `handoff_exec.rs`,
  `handoff_rollback.rs`, `main.rs` (`run_daemon`, `arm_forward_exec`), and the
  real-exec test module documentation.
- User documentation: `docs/cli.md` (`splinterm upgrade` no longer says live
  upgrade is unavailable in this build), `docs/usage.md`, `docs/headless.md`,
  and `docs/packaging.md`. A running daemon from before enablement still
  reports `Unsupported`; the client now says that the running daemon's build
  does not support live upgrade, and gives the restart path.
- What it does **not** change:
  - Nix stays refused.
  - D8 stays: one live upgrade per daemon until it restarts.
  - The `vm-acceptance-live-handoff` feature and its packaging guard stay; a
    follow-up may remove the feature once it has no use.
  - The restart path stays available.
