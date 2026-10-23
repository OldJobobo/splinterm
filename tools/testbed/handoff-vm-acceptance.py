#!/usr/bin/env python3
"""Live-upgrade VM acceptance sequence (Plan 0061 M3.17).

Prints the bounded sequence in docs/handoff-vm-acceptance.md, step by step,
as the exact `tools/testbed/omarchy-vm.sh` invocations it makes. It runs
nothing by itself:

  plan                          print every step (the default)
  show STEP                     print one step
  run STEP --operator-approved  run exactly one step's commands, in order,
                                stopping at the first failure

Running needs the operator's approval of the whole sequence (AGENTS.md
section 3), the testbed configuration (`SPLINTERM_TESTBED_CONFIG`), and a
private clone of the candidate under ~/.cache/splinterm-builds/vm-acceptance/.
Every package this sequence builds is a TEST-ONLY VM acceptance build
(`--vm-acceptance-live-handoff`), which must never be published or shipped.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import time
from typing import Callable

ROOT = Path(__file__).resolve().parents[2]
RUNNER = Path(os.environ.get("SPLINTERM_HANDOFF_VM_RUNNER", ROOT / "tools/testbed/omarchy-vm.sh"))
PRIVATE_CLONES = Path.home() / ".cache/splinterm-builds/vm-acceptance"
EVIDENCE = ".testbed-package/evidence/handoff"

# Every guest script starts here. `exec` and `desktop-exec` both run from the
# testbed checkout, so the package root and the private runtime that
# `package-launch` owns follow from it.
PRELUDE = r"""set -euo pipefail
leaf=$(basename "$PWD")
pkg=$PWD/.testbed-package
runtime=/run/user/$(id -u)/$leaf-package-test
socket=$runtime/splinterd.sock
ev=$PWD/.testbed-package/evidence/handoff
mkdir -p "$ev"
export SPLINTERM_SOCKET=$socket
# A live upgrade leaves packaged processes on a replaced inode or a sealed
# memfd snapshot, so match all three spellings, bound to the private socket.
socket_process() {
  local role=$1 process executable
  for process in /proc/[0-9]*; do
    tr '\0' '\n' 2>/dev/null <"$process/environ" | grep -Fqx "SPLINTERM_SOCKET=$socket" || continue
    executable=$(readlink "$process/exe" 2>/dev/null || true)
    case $role:$executable in
      daemon:/usr/bin/splinterd|'daemon:/usr/bin/splinterd (deleted)'|'daemon:/memfd:splinterd-daemon (deleted)') echo "${process##*/}" ;;
      client:/usr/bin/splinterm|'client:/usr/bin/splinterm (deleted)'|'client:/memfd:splinterd-client (deleted)') echo "${process##*/}" ;;
    esac
  done
}
daemon_pid() { socket_process daemon | head -n 1; }
address_of_pid() { hyprctl -j clients | jq -er --argjson pid "$1" 'map(select(.pid == $pid)) | if length == 1 then .[0].address else error("expected one window for pid") end'; }
focus() { hyprctl dispatch "hl.dsp.focus({ window = \"address:$(cat "$ev/$1")\" })" >/dev/null; }
guard() {
  local active
  active=$(hyprctl -j activewindow | jq -r .address)
  [[ $active == "$(cat "$ev/$1")" ]] || { printf 'input guard: active window %s is not %s\n' "$active" "$1" >&2; exit 1; }
}
closed() {
  local address
  address=$(cat "$ev/$1")
  for _ in $(seq 1 50); do
    hyprctl -j clients | jq -e --arg a "$address" 'any(.[]; .address == $a)' >/dev/null || return 0
    sleep 0.1
  done
  printf 'the window in %s is still mapped: the answer did not reach it\n' "$1" >&2
  exit 1
}
capture() { grim -o Virtual-1 "$ev/$1.png"; }
# Moves the pointer onto the text of the window an evidence file names,
# above a trusted dialog's action areas (its bottom 22%).
point_into() {
  local x y
  read -r x y < <(hyprctl -j clients | jq -er --arg a "$(cat "$ev/$1")" 'map(select(.address == $a)) | if length == 1 then .[0] | "\(.at[0] + (.size[0] / 2 | floor)) \(.at[1] + (.size[1] * 3 / 10 | floor))" else error("expected one window") end')
  hyprctl dispatch "hl.dsp.cursor.move({ x = $x, y = $y })" >/dev/null
}
expect() { grep -Fq -- "$1" "$2" || { printf 'missing %q in %s\n' "$1" "$2" >&2; exit 1; }; }
# The private daemon logs with ANSI colour, which splits key=value fields, so
# match daemon.log with the colour codes removed. grep reads the whole stream
# (no -q): stopping at an early match would make sed fail with SIGPIPE, and
# pipefail would then report a line that is there as missing.
expect_log() { sed 's/\x1b\[[0-9;]*m//g' "$2" | grep -F -- "$1" >/dev/null || { printf 'missing %q in %s\n' "$1" "$2" >&2; exit 1; }; }
"""

# Linux input key codes for `omarchy-vm.sh input key` (ydotool).
KEYS = {
    "ctrl": 29,
    "shift": 42,
    "d": 32,
    "g": 34,
    "tab": 15,
    "enter": 28,
    "escape": 1,
    "up": 103,
    "down": 108,
}


@dataclass(frozen=True)
class Action:
    """One thing a step does. `vm` is an omarchy-vm.sh invocation."""

    kind: str
    vm: tuple[str, ...] = ()
    seconds: float = 0
    note: str = ""


@dataclass(frozen=True)
class Step:
    id: str
    phase: str
    title: str
    expect: str
    actions: tuple[Action, ...]
    optional: bool = False
    graphical: bool = False


def vm(*arguments: str) -> Action:
    return Action("vm", tuple(arguments))


def guest(script: str) -> Action:
    return Action("vm", ("exec", "bash", "-c", PRELUDE + script), note=script.strip())


def desktop(script: str) -> Action:
    return Action("vm", ("desktop-exec", "bash", "-c", PRELUDE + script), note=script.strip())


def pause(seconds: float) -> Action:
    return Action("sleep", seconds=seconds)


def chord(*names: str) -> Action:
    codes = [KEYS[name] for name in names]
    sequence = [f"{code}:1" for code in codes] + [f"{code}:0" for code in reversed(codes)]
    return vm("input", "key", *sequence)


def line(text: str) -> tuple[Action, ...]:
    return (vm("input", "type", text), chord("enter"))


def click_inside(target: str) -> tuple[Action, ...]:
    """Engages a trusted dialog: the pointer onto its text, then a left
    click. A trusted dialog takes no keyboard focus when it appears, and its
    keys count only after a click inside it and the 600 ms arming delay."""
    return (desktop(f"point_into {target}"), vm("input", "click", "0xC0"), pause(1))


def guard(target: str) -> Action:
    """Requires the active window to be `target` (an evidence file naming an
    address); the step aborts otherwise."""
    return desktop(f"guard {target}")


def is_input(action: Action) -> bool:
    return action.kind == "vm" and action.vm[:1] == ("input",)


def guarded(
    target: str, *actions: Action, settle: float = 0.3, then: str = "guard"
) -> tuple[Action, ...]:
    """Focuses `target`, waits `settle` seconds, then runs `actions` with a
    guard directly before and directly after every input, never a pause in
    between, so input that reached another window is caught at once. With
    `then="closed"`, the check after the last input is instead that the
    target window closed (a dialog the last key answered)."""
    batch: list[Action] = [desktop(f"focus {target}"), pause(settle)]
    check = guard(target)
    for index, action in enumerate(actions):
        if not is_input(action):
            batch.append(action)
            continue
        if batch[-1] != check:
            batch.append(check)
        batch.append(action)
        last = index == len(actions) - 1
        batch.append(desktop(f"closed {target}") if last and then == "closed" else check)
    if batch[-1] != check and not batch[-1].note.startswith("closed "):
        batch.append(check)
    return tuple(batch)


def start_upgrade(case: str) -> Action:
    # In the guest Wayland session, so the upgrade dialog can appear; in the
    # background, so the next steps can answer it.
    return desktop(
        f"""rm -f "$ev/{case}-upgrade.exit"
nohup bash -c '/usr/bin/splinterm upgrade --now >"$0/{case}-upgrade.log" 2>&1; echo $? >"$0/{case}-upgrade.exit"' "$ev" >/dev/null 2>&1 &"""
    )


def record_dialog(case: str) -> Action:
    return desktop(
        f"""hyprctl -j clients | jq -er 'map(select(.title == "Splinterm — Upgrade")) | if length == 1 then .[0].address else error("expected one upgrade dialog") end' >"$ev/dialog-address"
capture {case}-prompt"""
    )


def wait_for_exit(case: str, expected: int) -> Action:
    return guest(
        f"""for _ in $(seq 1 600); do [[ -s $ev/{case}-upgrade.exit ]] && break; sleep 0.1; done
cat "$ev/{case}-upgrade.log"
[[ $(cat "$ev/{case}-upgrade.exit") == {expected} ]]"""
    )


def same_daemon(generation: int, rollback: str) -> Action:
    return guest(
        f"""[[ $(daemon_pid) == $(cat "$ev/daemon-pid") ]]
/usr/bin/splinterm upgrade >"$ev/report-g{generation}.txt"
cat "$ev/report-g{generation}.txt"
expect 'Daemon: generation {generation} ' "$ev/report-g{generation}.txt"
expect 'Rollback copy: {rollback}' "$ev/report-g{generation}.txt\""""
    )


def record_target(prefix: str) -> Action:
    """Records the session's Window W1 and its daemon (after package-launch)."""
    return desktop(
        f"""jq -er .target_address "$runtime/guest-window.json" >"$ev/target-address"
hyprctl -j clients | jq --arg a "$(cat "$ev/target-address")" 'map(select(.address == $a))[0]' >"$ev/{prefix}-w1.json"
jq -er .pid "$ev/{prefix}-w1.json" >"$ev/w1-pid"
hyprctl -j activewindow >"$ev/{prefix}-activewindow.json"
hyprctl -j cursorpos >"$ev/{prefix}-cursorpos.json"
hyprctl -j monitors >"$ev/{prefix}-monitors.json"
daemon_pid >"$ev/daemon-pid"
readlink "/proc/$(cat "$ev/daemon-pid")/exe" | tee "$ev/{prefix}-daemon-exe"
cat "$ev/{prefix}-w1.json\""""
    )


def record_extra() -> Action:
    return desktop(
        """cp "$runtime/extra-1/client-pid" "$ev/w2-pid"
address_of_pid "$(cat "$ev/w2-pid")" >"$ev/w2-address"
focus target-address"""
    )


def refuse_second_handoff(case: str) -> Action:
    return guest(
        f"""set +e; /usr/bin/splinterm upgrade --now >"$ev/{case}-upgrade.txt" 2>&1; status=$?; set -e
cat "$ev/{case}-upgrade.txt"; [[ $status == 4 ]]
expect 'needs a daemon restart first' "$ev/{case}-upgrade.txt"
expect 'Nothing changed' "$ev/{case}-upgrade.txt"
[[ $(daemon_pid) == $(cat "$ev/daemon-pid") ]]"""
    )


def d10_case(case: str, answer: str, expected_exit: int) -> tuple[Action, ...]:
    return (
        start_upgrade(case),
        pause(4),
        record_dialog(case),
        # Typed into the paused Window while the question is open: dropped.
        *guarded("target-address", *line(f"echo dropped-{case}")),
        # The dialog takes no focus and ignores keys until a click inside it
        # and its 600 ms arming delay; focus is taken only for the guards.
        *guarded(
            "dialog-address",
            *click_inside("dialog-address"),
            chord(answer),
            settle=1,
            then="closed",
        ),
        wait_for_exit(case, expected_exit),
    )


STEPS: tuple[Step, ...] = (
    Step(
        "p0-status",
        "0 preparation",
        "Guest, compositor, and input service are up",
        "Hyprland running, ydotool active, space free.",
        (vm("status"),),
    ),
    Step(
        "p0-package-status",
        "0 preparation",
        "Record the guest package and trusted identities before replacement",
        "Recorded; note whether splinterm-mcp is installed and whether the guest's own splinterd.service is active (package-install stops and restarts it).",
        (vm("package-status"),),
    ),
    Step(
        "p0-sysctl",
        "0 preparation",
        "Record vm.memfd_noexec and the kernel",
        "memfd_noexec is 0 or 1. At 2 the sealed snapshots cannot be created and live upgrade is refused before anything changes (case X1).",
        (guest('cat /proc/sys/vm/memfd_noexec | tee "$ev/memfd-noexec-before"; uname -r'),),
    ),
    Step(
        "p0-build-a",
        "0 preparation",
        "Build package A: the candidate commit as a TEST-ONLY VM acceptance build",
        "Builds and validates; validation prints the TEST-ONLY banner.",
        (vm("package-build", "--vm-acceptance-live-handoff"),),
    ),
    Step(
        "p0-install-a",
        "0 preparation",
        "Install package A in the guest (rollback copies saved)",
        "Installed; command -v splinterm is /usr/bin/splinterm; private install check passes.",
        (vm("package-install", "--confirm-guest-install", "--vm-acceptance-live-handoff"),),
    ),
    Step(
        "p0-save-a",
        "0 preparation",
        "Keep package A's files for the rollback round",
        "Package A's files are copied outside the guest source checkout, which package-build replaces.",
        (
            guest(
                """mkdir -p "$pkg/package-a"
cp "$pkg"/source/packaging/splinterm-*.pkg.tar.* "$pkg/package-a/"
ls -l "$pkg/package-a\""""
            ),
        ),
    ),
    Step(
        "p0-install-mcp",
        "0 preparation",
        "Install package A's MCP split package (for case M1 only)",
        "splinterm-mcp installed; package B's install then upgrades it too. Skip with M1 if not approved.",
        (
            guest(
                'sudo -n pacman -U --noconfirm "$pkg"/source/packaging/splinterm-mcp-[0-9]*.pkg.tar.*\nsha256sum /usr/bin/splinterm-mcp'
            ),
        ),
        optional=True,
    ),
    Step(
        "s1-launch",
        "1 smoke",
        "Launch the packaged daemon and Window W1 on guest workspace 8",
        "W1 maps on workspace 8 / Virtual-1; the guest-window record is written.",
        (vm("package-launch"),),
        graphical=True,
    ),
    Step(
        "s1-record",
        "1 smoke",
        "Record W1, the daemon, and the guest state to restore",
        "Address, PID, workspace, monitor, geometry, focus, cursor, scale, and transform recorded under the evidence directory.",
        (record_target("s1"),),
        graphical=True,
    ),
    Step(
        "s1-smoke",
        "1 smoke",
        "Type into W1",
        "smoke-ok is echoed and no other Window moved. On failure: package-stop and abort.",
        (*guarded("target-address", *line("echo smoke-ok")), pause(1), desktop("capture s1-smoke")),
        graphical=True,
    ),
    Step(
        "s1-dry-run",
        "1 smoke",
        "Dry run against package A: the feature build is live",
        "Generation 1, local package build, rollback copy kept, nothing blocks it; the daemon log carries the TEST-ONLY warning.",
        (
            guest(
                """/usr/bin/splinterm upgrade | tee "$ev/s1-dry-run.txt"
expect 'Daemon: generation 1 ' "$ev/s1-dry-run.txt"
expect 'local package build' "$ev/s1-dry-run.txt"
expect 'Rollback copy: kept' "$ev/s1-dry-run.txt"
expect 'Live upgrade: nothing blocks it.' "$ev/s1-dry-run.txt"
expect_log 'TEST-ONLY VM acceptance build' "$runtime/daemon.log\""""
            ),
        ),
    ),
    Step(
        "u1-tabs",
        "2 setup",
        "Give W1 three tabs, a split, markers, and a chosen focus",
        "Three tabs; tab 2 active with two panes; the lower pane focused; markers t2-lower-mark and t2-upper-mark visible.",
        (
            *guarded(
                "target-address",
                chord("ctrl", "shift", "d"),
                pause(1),
                chord("ctrl", "shift", "d"),
                pause(1),
                chord("ctrl", "shift", "tab"),
                pause(0.5),
                chord("ctrl", "shift", "enter"),
                pause(1),
                *line("echo t2-lower-mark"),
                chord("ctrl", "shift", "up"),
                pause(0.5),
                *line("echo t2-upper-mark"),
                chord("ctrl", "shift", "down"),
                pause(0.5),
            ),
            desktop("capture u1-tabs"),
            guest('/usr/bin/splinterm topology | tee "$ev/u1-topology.txt"'),
        ),
        graphical=True,
    ),
    Step(
        "u2-extra-window",
        "2 setup",
        "Open a second managed Window W2 from package A's client",
        "W2 maps on workspace 8; its PID and address are recorded; focus returns to W1.",
        (
            vm("package-launch-extra", "launch"),
            record_extra(),
        ),
        graphical=True,
    ),
    Step(
        "m1-prepare",
        "2 setup",
        "Create the MCP probe Lair and grant splinterm-mcp control of it",
        "A Lair named mcp-probe runs `cat`; its IDs are recorded; policy.json is written through package-config and the daemon reloads it (SIGHUP).",
        (
            guest(
                """/usr/bin/splinterm new mcp-probe -- /bin/sh -c 'printf mcp-ready; exec cat'
sleep 1
python3 "$pkg/source/tools/testbed/handoff-mcp-probe.py" ids --topology "$pkg/acceptance-state/splinterm/topology.json" | tee "$ev/mcp-ids.json\""""
            ),
            Action("policy"),
            guest('kill -HUP "$(daemon_pid)"; sleep 1; tail -n 5 "$runtime/daemon.log"'),
        ),
        optional=True,
    ),
    Step(
        "m1-start",
        "2 setup",
        "Start the MCP probe: acquire control of the probe Splint and hold the session",
        "acquired.json reports a handle; the probe waits for the upgrade.",
        (
            guest(
                """mkdir -p "$ev/mcp"
nohup python3 "$pkg/source/tools/testbed/handoff-mcp-probe.py" run --ids "$ev/mcp-ids.json" --state-dir "$ev/mcp" >"$ev/mcp-probe.log" 2>&1 &
for _ in $(seq 1 300); do [[ -s $ev/mcp/acquired.json ]] && break; sleep 0.1; done
cat "$ev/mcp/acquired.json"
jq -e .handle "$ev/mcp/acquired.json\""""
            ),
        ),
        optional=True,
    ),
    Step(
        "u3-package-b",
        "2 setup",
        "Build and install package B (a newer TEST-ONLY build) while A's daemon serves",
        "B is installed; the same daemon process still serves generation 1 from the replaced inode, and the dry run still finds nothing blocking.",
        (
            Action("commit-b"),
            vm("package-build", "--vm-acceptance-live-handoff"),
            vm("package-install", "--confirm-guest-install", "--vm-acceptance-live-handoff"),
            guest(
                """readlink "/proc/$(cat "$ev/daemon-pid")/exe" | tee "$ev/daemon-exe-after-b"
expect '(deleted)' "$ev/daemon-exe-after-b\""""
            ),
            same_daemon(1, "kept"),
        ),
    ),
    Step(
        "c1-cancel-escape",
        "3 adoption round",
        "D10 prompt, cancelled with Escape (W2 stopped so it cannot acknowledge)",
        "The dialog titled 'Splinterm — Upgrade' names W2; W1 shows 'Upgrading — input paused' and drops dropped-c1; Escape cancels: exit 3, 'Upgrade cancelled', the same daemon at generation 1, and W1 types again.",
        (
            guest('kill -STOP "$(cat "$ev/w2-pid")"'),
            *d10_case("c1", "escape", 3),
            same_daemon(1, "kept"),
            *guarded("target-address", *line("echo after-c1")),
            pause(1),
            desktop("capture c1-after"),
        ),
        graphical=True,
    ),
    Step(
        "c2-cancel-enter",
        "3 adoption round",
        "D10 prompt, Enter also cancels (W2 still stopped)",
        "Exit 3 and nothing changed: Enter never proceeds.",
        (*d10_case("c2", "enter", 3), same_daemon(1, "kept")),
        graphical=True,
    ),
    Step(
        "c3-proceed-adopt",
        "3 adoption round",
        "Proceed without the stuck W2 (G): the live upgrade adopts",
        "Exit 0. Same daemon PID, now executing the sealed memfd snapshot at generation 2. W1 keeps its PID, relaunches into the pinned client snapshot (a new surface), and restores three tabs, tab 2 active, the lower pane focused, the markers, and its controllers: after-c3 typed with no click lands in the lower pane, dropped-c3 is absent. The banner clears.",
        (
            *d10_case("c3", "g", 0),
            pause(5),
            guest(
                """pid=$(cat "$ev/daemon-pid")
[[ $(daemon_pid) == "$pid" ]]
readlink "/proc/$pid/exe" | tee "$ev/daemon-exe-adopted"
expect '/memfd:splinterd-daemon' "$ev/daemon-exe-adopted"
readlink "/proc/$(cat "$ev/w1-pid")/exe" | tee "$ev/w1-exe-adopted"
expect '/memfd:splinterd-client' "$ev/w1-exe-adopted"
expect_log 'daemon_generation=2' "$runtime/daemon.log"
expect_log "daemon handoff: restored a relaunched Window's controller" "$runtime/daemon.log"
/usr/bin/splinterm topology | tee "$ev/c3-topology.txt\""""
            ),
            same_daemon(2, "not kept"),
            desktop(
                """address_of_pid "$(cat "$ev/w1-pid")" >"$ev/target-address"
capture c3-restored"""
            ),
            *guarded("target-address", *line("echo after-c3")),
            pause(1),
            desktop("capture c3-typed"),
        ),
        graphical=True,
    ),
    Step(
        "c4-stale-client",
        "3 adoption round",
        "The stale client: W2's pre-upgrade inode gets no trusted UI",
        "W2's exe reads '/usr/bin/splinterm (deleted)'; executing it as `list` is refused [unauthorized] and as `upgrade` exits 1 naming the installed client; the installed client still works. W2 then resumes, shows the lost-connection state or exits, and is closed.",
        (
            guest(
                """p=$(cat "$ev/w2-pid")
readlink "/proc/$p/exe" | tee "$ev/w2-exe"
expect '/usr/bin/splinterm (deleted)' "$ev/w2-exe"
if "/proc/$p/exe" list >"$ev/c4-list.txt" 2>&1; then cat "$ev/c4-list.txt"; exit 1; fi
cat "$ev/c4-list.txt"; expect '[unauthorized]' "$ev/c4-list.txt"
set +e; "/proc/$p/exe" upgrade >"$ev/c4-upgrade.txt" 2>&1; status=$?; set -e
cat "$ev/c4-upgrade.txt"; [[ $status == 1 ]]
expect 'accepts upgrade requests only from the installed splinterm' "$ev/c4-upgrade.txt"
expect 'started before a package upgrade replaced it' "$ev/c4-upgrade.txt"
/usr/bin/splinterm list
kill -CONT "$p\""""
            ),
            pause(3),
            desktop("capture c4-w2-resumed"),
            vm("package-close-extra", "1"),
        ),
        graphical=True,
    ),
    Step(
        "c5-no-second-handoff",
        "3 adoption round",
        "D8: the adopted daemon refuses another live upgrade until it restarts",
        "Exit 4 with 'needs a daemon restart first' and 'Nothing changed'; the same daemon still serves.",
        (refuse_second_handoff("c5"),),
    ),
    Step(
        "m1-check",
        "3 adoption round",
        "MCP controller expiry across the upgrade",
        "The old handle fails with controller_unavailable; acquiring again succeeds and input lands. If acquiring again is refused because the running adapter's binary was replaced by package B, record it as an observation for the operator.",
        (
            guest(
                """touch "$ev/mcp/proceed"
for _ in $(seq 1 600); do [[ -s $ev/mcp/result.json ]] && break; sleep 0.1; done
cat "$ev/mcp-probe.log"
jq '{acquired_before, old_handle_expired, reacquired}' "$ev/mcp/result.json\""""
            ),
        ),
        optional=True,
    ),
    Step(
        "r1-restart",
        "4 rollback round",
        "Restart the session: a fresh daemon from package B, a new Window W1",
        "package-stop ends the adopted daemon and W1; package-launch serves generation 1 from /usr/bin (package B) with its rollback copy kept and nothing blocking.",
        (
            vm("package-stop"),
            vm("package-config", "--clear"),
            vm("package-launch"),
            record_target("r1"),
            same_daemon(1, "kept"),
        ),
        graphical=True,
    ),
    Step(
        "r2-setup",
        "4 rollback round",
        "A second tab in W1, a stuck-to-be W2, and package A reinstalled as the forward pair",
        "W1 has two tabs (r2-mark in tab 2); W2 maps; package A installs over B (a downgrade is still a different pair); the same daemon serves and nothing blocks.",
        (
            *guarded("target-address", chord("ctrl", "shift", "d"), pause(1), *line("echo r2-mark")),
            vm("package-launch-extra", "launch"),
            record_extra(),
            guest('cp "$pkg"/package-a/splinterm-*.pkg.tar.* "$pkg/source/packaging/"'),
            vm("package-install", "--confirm-guest-install", "--vm-acceptance-live-handoff"),
            same_daemon(1, "kept"),
        ),
        graphical=True,
    ),
    Step(
        "r3-rollback",
        "4 rollback round",
        "A real rollback: the installed client changes inode while the D10 prompt is open",
        "While the prompt is open, /usr/bin/splinterm is replaced by a byte-identical copy (cp -p, then rename: a new inode, pacman -Qk still clean). Proceeding (G) makes the forward candidate refuse the changed install pair at adoption and roll back: exit 7, saying the daemon no longer accepts this splinterm, which may have been replaced (the CLI runs the old inode); a fresh `splinterm upgrade` shows the same daemon PID now executing the sealed rollback snapshot at generation 1 with no rollback copy, W1 relaunched as the installed /usr/bin/splinterm, reconnected (its 3 s 'Rolled back' notice may be gone by the capture), and typing again.",
        (
            guest('kill -STOP "$(cat "$ev/w2-pid")"'),
            start_upgrade("r3"),
            pause(4),
            record_dialog("r3"),
            guest(
                """stat -c '%d:%i' /usr/bin/splinterm | tee "$ev/r3-client-before"
sudo -n cp -p /usr/bin/splinterm /usr/bin/.splinterm.vm-acceptance
sudo -n mv -f /usr/bin/.splinterm.vm-acceptance /usr/bin/splinterm
stat -c '%d:%i' /usr/bin/splinterm | tee "$ev/r3-client-after"
! cmp -s "$ev/r3-client-before" "$ev/r3-client-after"
pacman -Qk splinterm"""
            ),
            *guarded(
                "dialog-address",
                *click_inside("dialog-address"),
                chord("g"),
                settle=1,
                then="closed",
            ),
            # The CLI still runs the replaced inode, which the rolled-back
            # daemon refuses, so it cannot confirm (operator decision
            # 2026-10-22); same_daemon's fresh `splinterm upgrade` confirms.
            wait_for_exit("r3", 7),
            guest(
                """expect 'no longer accepts this splinterm' "$ev/r3-upgrade.log"
expect 'may have been replaced while the upgrade ran' "$ev/r3-upgrade.log\""""
            ),
            pause(5),
            guest(
                """pid=$(cat "$ev/daemon-pid")
readlink "/proc/$pid/exe" | tee "$ev/r3-daemon-exe"
expect '/memfd:splinterd-daemon' "$ev/r3-daemon-exe"
expect_log 'phase="adoption"' "$runtime/daemon.log"
expect_log 'the install prefix does not hold the forward pair the manifest recorded' "$runtime/daemon.log"
expect_log 'rolling back to the previous generation with the unchanged descriptors' "$runtime/daemon.log"
[[ $(readlink "/proc/$(cat "$ev/w1-pid")/exe") == /usr/bin/splinterm ]]"""
            ),
            same_daemon(1, "not kept"),
            desktop(
                """address_of_pid "$(cat "$ev/w1-pid")" >"$ev/target-address"
capture r3-rolled-back"""
            ),
            *guarded("target-address", *line("echo after-rollback")),
            pause(1),
            desktop("capture r3-typed"),
        ),
        graphical=True,
    ),
    Step(
        "r4-no-second-handoff",
        "4 rollback round",
        "D8: the rollback daemon refuses another live upgrade until it restarts",
        "Exit 4 with 'needs a daemon restart first'; the same daemon still serves.",
        (refuse_second_handoff("r4"),),
    ),
    Step(
        "r5-close-extra",
        "4 rollback round",
        "Resume and close W2",
        "W2 resumes and is closed by its launch token.",
        (guest('kill -CONT "$(cat "$ev/w2-pid")"'), pause(2), vm("package-close-extra", "1")),
        graphical=True,
    ),
    Step(
        "f1-exit-codes",
        "5 final checks",
        "Collect the CLI exit codes",
        "c1 and c2 exited 3 (cancelled), c3 0 (upgraded), r3 7 (unconfirmed: its replaced client is refused); c5 and r4 exited 4 (unavailable) and the stale client 1.",
        (guest('for case in c1 c2 c3 r3; do printf "%s=%s\\n" "$case" "$(cat "$ev/$case-upgrade.exit")"; done'),),
    ),
    Step(
        "x1-memfd-noexec",
        "5 final checks",
        "vm.memfd_noexec=2 refuses live upgrade before anything changes (guest-wide sysctl, restored after)",
        "Refused on guest kernels before 6.6, where the sysctl can only be raised (only a reboot restores it). With a fresh daemon under memfd_noexec=2: the dry run says 'Rollback copy: not kept'; upgrade --now exits 4 and nothing changes. The sysctl returns to its recorded value (z1 restores it too if x1 aborted).",
        (
            guest(
                """IFS=. read -r major minor _ <<<"$(uname -r)"
if (( major < 6 || (major == 6 && minor < 6) )); then
  printf 'x1 needs guest kernel 6.6 or later: on %s vm.memfd_noexec can only be raised, so only a reboot would restore it\n' "$(uname -r)" >&2
  exit 1
fi
[[ -s $ev/memfd-noexec-before ]]"""
            ),
            vm("package-stop"),
            guest('sudo -n sysctl -w vm.memfd_noexec=2'),
            vm("package-launch"),
            guest(
                """/usr/bin/splinterm upgrade | tee "$ev/x1-dry-run.txt"
expect 'Rollback copy: not kept' "$ev/x1-dry-run.txt"
set +e; /usr/bin/splinterm upgrade --now >"$ev/x1-upgrade.txt" 2>&1; status=$?; set -e
cat "$ev/x1-upgrade.txt"; [[ $status == 4 ]]"""
            ),
            vm("package-stop"),
            guest(
                """sudo -n sysctl -w vm.memfd_noexec="$(cat "$ev/memfd-noexec-before")"
[[ $(cat /proc/sys/vm/memfd_noexec) == $(cat "$ev/memfd-noexec-before") ]]"""
            ),
        ),
        optional=True,
        graphical=True,
    ),
    Step(
        "z1-cleanup",
        "6 cleanup",
        "Restore and clean up (AGENTS.md section 3.3)",
        "No stopped test process remains; the probe (checked by its command line) and W2 are gone; the r3 temporary copy is removed; vm.memfd_noexec is back at its recorded value; package-stop closes W1 and stops the adopted daemon, and its guest-window restore returns workspace, focus, and cursor; workspace 8 is empty and the monitors match the s1 record; evidence is copied to the host; the policy seed is cleared.",
        (
            guest(
                """[[ ! -s $ev/w2-pid ]] || kill -CONT "$(cat "$ev/w2-pid")" 2>/dev/null || true
if [[ -s $ev/mcp/probe.pid ]]; then
  probe=$(cat "$ev/mcp/probe.pid")
  if tr '\\0' ' ' <"/proc/$probe/cmdline" 2>/dev/null | grep -Fq handoff-mcp-probe.py; then
    kill "$probe"
  fi
fi
sudo -n rm -f /usr/bin/.splinterm.vm-acceptance
if [[ -s $ev/memfd-noexec-before && $(cat /proc/sys/vm/memfd_noexec) != "$(cat "$ev/memfd-noexec-before")" ]]; then
  sudo -n sysctl -w vm.memfd_noexec="$(cat "$ev/memfd-noexec-before")"
fi
[[ ! -s $ev/memfd-noexec-before || $(cat /proc/sys/vm/memfd_noexec) == "$(cat "$ev/memfd-noexec-before")" ]]
[[ ! -d $runtime/extra-1 ]] || echo 'extra Window slot 1 is still open: run package-close-extra 1'"""
            ),
            Action("stop-if-running"),
            desktop(
                """[[ $(hyprctl -j clients | jq '[.[] | select(.workspace.id == 8)] | length') == 0 ]]
hyprctl -j monitors >"$ev/z1-monitors.json"
diff <(jq -S 'map({name, scale, transform, width, height, x, y})' "$ev/s1-monitors.json") <(jq -S 'map({name, scale, transform, width, height, x, y})' "$ev/z1-monitors.json")"""
            ),
            Action("evidence"),
            vm("package-config", "--clear"),
        ),
        graphical=True,
    ),
)

STEPS = (
    *STEPS,
    Step(
        "z2-plain-package",
        "6 cleanup",
        "Return the guest to an ordinary (non-test) package of the candidate (mandatory)",
        "A plain build installs; package-status reports no TEST-ONLY package or marker. Remove splinterm-mcp too if p0-package-status showed it absent before.",
        (
            Action("checkout-candidate"),
            vm("package-build"),
            vm("package-install", "--confirm-guest-install"),
            vm("package-status"),
            guest(
                """! pacman -Qi splinterm | grep -Fq TEST-ONLY
! grep -aqF splinterm-vm-acceptance-live-handoff: /usr/bin/splinterd"""
            ),
        ),
    ),
)

ACTIONS = {
    "status",
    "package-build",
    "package-status",
    "package-install",
    "package-launch",
    "package-config",
    "package-launch-extra",
    "package-close-extra",
    "package-stop",
    "exec",
    "desktop-exec",
    "input",
}


def step_named(name: str) -> Step:
    for step in STEPS:
        if step.id == name:
            return step
    raise SystemExit(f"unknown step: {name} (see `plan`)")


def runner_command(action: Action) -> list[str]:
    return [str(RUNNER), *action.vm]


def describe(action: Action) -> str:
    if action.kind == "sleep":
        return f"sleep {action.seconds:g}"
    if action.kind == "commit-b":
        return (
            "git commit --allow-empty -m 'vm acceptance: package B (local only, never pushed)'"
            f"   # in this private clone ({PRIVATE_CLONES}/...) only"
        )
    if action.kind == "checkout-candidate":
        return "git checkout --detach <candidate recorded before package B>   # private clone only"
    if action.kind == "policy":
        return (
            "tools/testbed/omarchy-vm.sh package-config splinterm/policy.json"
            "   # stdin: an mcp-control rule for /usr/bin/splinterm-mcp (its guest sha256) over the mcp-probe Lair"
        )
    if action.kind == "stop-if-running":
        return "tools/testbed/omarchy-vm.sh package-stop   # when the private session still exists"
    if action.kind == "evidence":
        return (
            f"tools/testbed/omarchy-vm.sh exec tar czf - {EVIDENCE} > target/vm-acceptance/handoff-evidence.tar.gz"
        )
    if action.note:
        verb = action.vm[0]
        script = "\n      ".join(action.note.splitlines())
        return f"tools/testbed/omarchy-vm.sh {verb} bash -c '[prelude]\n      {script}'"
    return shlex.join(["tools/testbed/omarchy-vm.sh", *action.vm])


def print_step(step: Step) -> None:
    flags = [flag for flag, on in (("optional", step.optional), ("graphical", step.graphical)) if on]
    suffix = f" [{', '.join(flags)}]" if flags else ""
    print(f"\n{step.id}  ({step.phase}){suffix}\n  {step.title}")
    for action in step.actions:
        print(f"    $ {describe(action)}")
    print(f"  Pass: {step.expect}")


def print_plan() -> None:
    print(__doc__.strip())
    print("\nEvery guest script starts with this prelude:\n")
    print("\n".join(f"    {text}" for text in PRELUDE.strip().splitlines()))
    for step in STEPS:
        print_step(step)


def run_capture(arguments: list[str]) -> str:
    return subprocess.run(arguments, check=True, capture_output=True, text=True).stdout


def write_policy() -> None:
    ids = json.loads(
        run_capture(
            runner_command(guest('cat "$ev/mcp-ids.json"'))
        ).strip().splitlines()[-1]
    )
    digest = run_capture(runner_command(guest("sha256sum /usr/bin/splinterm-mcp"))).split()[0]
    policy = {
        "schema": "splinterm.policy.v2",
        "rules": [
            {
                "id": "vm-acceptance-mcp-probe",
                "executable": {"path": "/usr/bin/splinterm-mcp", "sha256": digest},
                "scopes": [
                    "topology_metadata_read",
                    "terminal_visible_read",
                    "terminal_subscribe",
                    "controller_acquire",
                    "controller_transfer",
                    "input",
                    "resize",
                ],
                "resources": [{"kind": "lair", "lair_id": ids["lair_id"]}],
                "limits": {
                    "max_returned_rows": 64,
                    "max_returned_bytes": 1048576,
                    "max_live_subscriptions": 2,
                    "deadline_ms": 5000,
                },
            }
        ],
    }
    subprocess.run(
        [str(RUNNER), "package-config", "splinterm/policy.json"],
        input=json.dumps(policy, indent=2) + "\n",
        text=True,
        check=True,
    )


def candidate_record() -> Path:
    """Where u3 records the candidate commit, inside the clone's Git directory
    so the checkout stays clean for `package-build`."""
    git_path = run_capture(
        ["git", "-C", str(ROOT), "rev-parse", "--git-path", "vm-acceptance-candidate"]
    ).strip()
    return (ROOT / git_path).resolve()


def require_private_clone(what: str) -> None:
    if PRIVATE_CLONES not in ROOT.parents:
        raise SystemExit(f"{what} only in a private clone under {PRIVATE_CLONES}, not {ROOT}")


def revision(name: str) -> str | None:
    result = subprocess.run(
        ["git", "-C", str(ROOT), "rev-parse", "--verify", "--quiet", name],
        capture_output=True, text=True,
    )
    return result.stdout.strip() if result.returncode == 0 else None


def recorded_candidate() -> str | None:
    """The candidate u3 recorded, checked against this clone's history: HEAD
    is the candidate (B not yet committed) or B on top of it. Anything else is
    a record from an earlier run, refused rather than guessed at."""
    record = candidate_record()
    if not record.exists():
        return None
    candidate = record.read_text(encoding="utf-8").strip()
    if candidate not in (revision("HEAD"), revision("HEAD^")):
        raise SystemExit(
            f"stale candidate record {candidate} in {record}: this clone was reused; "
            "start again from a fresh private clone"
        )
    return candidate


def commit_package_b() -> None:
    require_private_clone("package B is committed")
    candidate = recorded_candidate()
    if candidate is None:
        candidate_record().write_text(revision("HEAD") + "\n", encoding="utf-8")
    elif candidate != revision("HEAD"):
        print(f"package B is already committed on the candidate {candidate}")
        return
    subprocess.run(
        [
            "git", "-C", str(ROOT), "commit", "--allow-empty", "--quiet",
            "-m", "vm acceptance: package B (local only, never pushed)",
        ],
        check=True,
    )


def checkout_candidate() -> None:
    """Return the private clone to the candidate commit after package B, so
    the plain package z2 builds records the candidate, not B's commit."""
    candidate = recorded_candidate()
    if candidate is None:
        print("package B was never committed; the clone is at the candidate")
        return
    require_private_clone("the candidate is checked out")
    subprocess.run(
        ["git", "-C", str(ROOT), "checkout", "--quiet", "--detach", candidate], check=True
    )
    print(f"clone back at the candidate {candidate}")


def stop_if_running() -> None:
    state = run_capture(
        runner_command(guest('[[ -d $runtime ]] && echo running || echo stopped'))
    ).split()
    if state and state[-1] == "running":
        subprocess.run([str(RUNNER), "package-stop"], check=True)
    else:
        print("no private packaged session is running")


def copy_evidence() -> None:
    target = ROOT / "target/vm-acceptance"
    target.mkdir(parents=True, exist_ok=True)
    archive = target / f"handoff-evidence-{time.strftime('%Y%m%dT%H%M%SZ', time.gmtime())}.tar.gz"
    with archive.open("wb") as output:
        subprocess.run(
            [str(RUNNER), "exec", "tar", "czf", "-", EVIDENCE], stdout=output, check=True
        )
    print(f"evidence: {archive}")


SPECIAL: dict[str, Callable[[], None]] = {
    "policy": write_policy,
    "commit-b": commit_package_b,
    "checkout-candidate": checkout_candidate,
    "evidence": copy_evidence,
    "stop-if-running": stop_if_running,
}


def run_step(step: Step) -> None:
    for action in step.actions:
        print(f"$ {describe(action)}", flush=True)
        if action.kind == "sleep":
            time.sleep(action.seconds)
        elif action.kind in SPECIAL:
            SPECIAL[action.kind]()
        else:
            subprocess.run(runner_command(action), check=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command")
    commands.add_parser("plan")
    show = commands.add_parser("show")
    show.add_argument("step")
    run = commands.add_parser("run")
    run.add_argument("step")
    run.add_argument("--operator-approved", action="store_true")
    arguments = parser.parse_args()
    if arguments.command in (None, "plan"):
        print_plan()
        return 0
    step = step_named(arguments.step)
    if arguments.command == "show":
        print_step(step)
        return 0
    if not arguments.operator_approved:
        print(
            "refusing to run: the operator approves the whole bounded sequence first "
            "(AGENTS.md section 3); pass --operator-approved once they have",
            file=sys.stderr,
        )
        return 2
    try:
        run_step(step)
    except subprocess.CalledProcessError as error:
        print(f"step {step.id} failed: {error}", file=sys.stderr)
        return 1
    print(f"step {step.id} done; check: {step.expect}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
