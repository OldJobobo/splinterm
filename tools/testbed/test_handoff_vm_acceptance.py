#!/usr/bin/env python3
"""Contract tests for the live-upgrade VM acceptance helper and MCP probe."""

from __future__ import annotations

import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
import unittest


ROOT = Path(__file__).resolve().parents[2]
HELPER = ROOT / "tools/testbed/handoff-vm-acceptance.py"
PROBE = ROOT / "tools/testbed/handoff-mcp-probe.py"
RUNNER = ROOT / "tools/testbed/omarchy-vm.sh"
DOCUMENT = ROOT / "docs/handoff-vm-acceptance.md"


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


HELPER_MODULE = load("handoff_vm_acceptance", HELPER)
PROBE_MODULE = load("handoff_mcp_probe", PROBE)

# A stand-in `splinterm-mcp`: the first input with handle h1 succeeds, the
# second (after the upgrade) fails with controller_unavailable, and a second
# acquisition returns h2.
FAKE_MCP = r"""
import json, sys
inputs = {}
acquired = 0
for line in sys.stdin:
    message = json.loads(line)
    if "id" not in message:
        continue
    method = message["method"]
    result = {}
    if method == "tools/call":
        name = message["params"]["name"]
        arguments = message["params"]["arguments"]
        if name == "splinterm.acquire_control":
            acquired += 1
            result = {"structuredContent": {"data": {"controller_handle": f"h{acquired}"}}}
        elif name == "splinterm.input":
            handle = arguments["controller_handle"]
            inputs[handle] = inputs.get(handle, 0) + 1
            if handle == "h1" and inputs[handle] > 1:
                result = {"isError": True, "structuredContent": {"code": "controller_unavailable"}}
            else:
                result = {"structuredContent": {"data": {"accepted": True}}}
        else:
            result = {"structuredContent": {"data": {}}}
    print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": result}), flush=True)
"""


class HandoffVmAcceptanceHelperTests(unittest.TestCase):
    def run_helper(self, *arguments: str) -> tuple[subprocess.CompletedProcess[str], Path]:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        log = Path(temporary.name) / "calls.log"
        fake = Path(temporary.name) / "omarchy-vm.sh"
        fake.write_text(
            "#!/bin/sh\nprintf '%s\\n' \"--- $1\" >>\"$CALL_LOG\"\n", encoding="utf-8"
        )
        fake.chmod(0o755)
        environment = os.environ.copy()
        environment.update(
            {"SPLINTERM_HANDOFF_VM_RUNNER": str(fake), "CALL_LOG": str(log)}
        )
        result = subprocess.run(
            [sys.executable, str(HELPER), *arguments],
            cwd=ROOT,
            env=environment,
            text=True,
            capture_output=True,
            check=False,
        )
        return result, log

    def test_plan_prints_every_step_and_runs_nothing(self) -> None:
        for arguments in ((), ("plan",), ("show", "c3-proceed-adopt")):
            with self.subTest(arguments=arguments):
                result, log = self.run_helper(*arguments)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertFalse(log.exists(), "printing the plan must run nothing")
        result, _ = self.run_helper("plan")
        for step in HELPER_MODULE.STEPS:
            self.assertIn(f"\n{step.id}  (", result.stdout)
        self.assertIn("TEST-ONLY", result.stdout)

    def test_running_needs_the_operators_approval_and_a_known_step(self) -> None:
        result, log = self.run_helper("run", "p0-status")
        self.assertEqual(result.returncode, 2)
        self.assertIn("--operator-approved", result.stderr)
        self.assertFalse(log.exists())
        result, log = self.run_helper("run", "no-such-step", "--operator-approved")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unknown step", result.stderr)
        self.assertFalse(log.exists())
        result, log = self.run_helper("run", "p0-status", "--operator-approved")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(log.read_text(encoding="utf-8"), "--- status\n")

    def test_package_b_is_committed_only_in_a_private_clone(self) -> None:
        # This checkout is not under ~/.cache/splinterm-builds/vm-acceptance.
        result, log = self.run_helper("run", "u3-package-b", "--operator-approved")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("private clone", result.stderr)
        self.assertFalse(log.exists())

    def test_candidate_record_round_trip_and_stale_record_refused(self) -> None:
        from unittest import mock

        def git(*arguments: str) -> str:
            return subprocess.run(
                ["git", "-C", str(clone), *arguments], check=True, capture_output=True, text=True
            ).stdout.strip()

        with tempfile.TemporaryDirectory() as directory:
            clones = Path(directory) / "vm-acceptance"
            clone = clones / "candidate"
            clone.mkdir(parents=True)
            git("init", "--quiet")
            git("-c", "user.name=t", "-c", "user.email=t@t", "commit", "--allow-empty", "-qm", "c")
            candidate = git("rev-parse", "HEAD")
            environment = {
                "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t",
                "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t",
            }
            with mock.patch.object(HELPER_MODULE, "ROOT", clone), mock.patch.object(
                HELPER_MODULE, "PRIVATE_CLONES", clones
            ), mock.patch.dict(os.environ, environment):
                # No record yet: z2 leaves the clone alone.
                HELPER_MODULE.checkout_candidate()
                self.assertEqual(git("rev-parse", "HEAD"), candidate)
                # u3 records the candidate and commits B once, even if retried.
                HELPER_MODULE.commit_package_b()
                package_b = git("rev-parse", "HEAD")
                self.assertNotEqual(package_b, candidate)
                HELPER_MODULE.commit_package_b()
                self.assertEqual(git("rev-parse", "HEAD"), package_b)
                # z2 returns to the candidate.
                HELPER_MODULE.checkout_candidate()
                self.assertEqual(git("rev-parse", "HEAD"), candidate)
                # A record from an earlier run (not HEAD or HEAD^) is refused.
                git("-c", "user.name=t", "-c", "user.email=t@t", "commit", "--allow-empty", "-qm", "n1")
                git("-c", "user.name=t", "-c", "user.email=t@t", "commit", "--allow-empty", "-qm", "n2")
                with self.assertRaises(SystemExit) as refused:
                    HELPER_MODULE.checkout_candidate()
                self.assertIn("stale candidate record", str(refused.exception))
                with self.assertRaises(SystemExit):
                    HELPER_MODULE.commit_package_b()

    def test_plain_package_is_built_from_the_candidate_not_package_b(self) -> None:
        z2 = HELPER_MODULE.step_named("z2-plain-package")
        kinds = [action.kind for action in z2.actions]
        self.assertEqual(kinds[0], "checkout-candidate")
        self.assertLess(
            kinds.index("checkout-candidate"),
            next(i for i, action in enumerate(z2.actions) if action.vm[:1] == ("package-build",)),
        )

    def test_every_step_uses_runner_actions_and_valid_guest_scripts(self) -> None:
        runner = RUNNER.read_text(encoding="utf-8")
        cases = set(re.findall(r"^  ([a-z-]+)\)$", runner, re.MULTILINE))
        self.assertLessEqual(HELPER_MODULE.ACTIONS, cases)
        ids = [step.id for step in HELPER_MODULE.STEPS]
        self.assertEqual(len(ids), len(set(ids)))
        for step in HELPER_MODULE.STEPS:
            for action in step.actions:
                with self.subTest(step=step.id, action=action.kind):
                    if action.kind == "sleep":
                        continue
                    if action.kind != "vm":
                        self.assertIn(action.kind, HELPER_MODULE.SPECIAL)
                        continue
                    self.assertIn(action.vm[0], HELPER_MODULE.ACTIONS)
                    if action.vm[0] in ("exec", "desktop-exec") and action.vm[1:3] == (
                        "bash",
                        "-c",
                    ):
                        syntax = subprocess.run(
                            ["bash", "-n", "-c", action.vm[3]],
                            capture_output=True,
                            text=True,
                            check=False,
                        )
                        self.assertEqual(syntax.returncode, 0, syntax.stderr)
                    if action.vm[0] == "input":
                        self.assertIn(action.vm[1], ("key", "type", "click"))
                        if action.vm[1] == "click":
                            # A left click only; trusted dialogs need one
                            # before their keys count.
                            self.assertEqual(action.vm[2:], ("0xC0",))

    def test_packages_are_test_only_builds_and_installs(self) -> None:
        builds = [
            action.vm
            for step in HELPER_MODULE.STEPS
            if step.id != "z2-plain-package"
            for action in step.actions
            if action.kind == "vm" and action.vm[:1] in (("package-build",), ("package-install",))
        ]
        self.assertTrue(builds)
        for arguments in builds:
            self.assertIn("--vm-acceptance-live-handoff", arguments)

    def test_every_input_is_guarded_directly_before_and_after(self) -> None:
        def check(action, *verbs: str) -> bool:
            return (
                action.kind == "vm"
                and action.vm[0] == "desktop-exec"
                and any(re.fullmatch(rf"{verb} [a-z0-9-]+", action.note) for verb in verbs)
            )

        inputs = 0
        for step in HELPER_MODULE.STEPS:
            actions = step.actions
            for index, action in enumerate(actions):
                if not HELPER_MODULE.is_input(action):
                    continue
                inputs += 1
                with self.subTest(step=step.id, index=index):
                    self.assertTrue(step.graphical)
                    # No sleep, focus, or anything else between guard and input.
                    self.assertGreater(index, 0)
                    self.assertTrue(check(actions[index - 1], "guard"), actions[index - 1])
                    self.assertLess(index + 1, len(actions))
                    self.assertTrue(
                        check(actions[index + 1], "guard", "closed"), actions[index + 1]
                    )
                    if check(actions[index + 1], "closed"):
                        self.assertEqual(
                            actions[index - 1].note.split()[1],
                            actions[index + 1].note.split()[1],
                        )
        self.assertGreater(inputs, 10)

    def test_document_covers_every_step(self) -> None:
        document = DOCUMENT.read_text(encoding="utf-8")
        for step in HELPER_MODULE.STEPS:
            with self.subTest(step=step.id):
                self.assertIn(f"`{step.id}`", document)


class HandoffMcpProbeTests(unittest.TestCase):
    def test_ids_come_from_the_named_lairs_first_launched_splint(self) -> None:
        topology = {
            "schema_version": 4,
            "revision": 9,
            "lairs": [
                {"id": "lair-other", "name": "other", "dojos": []},
                {
                    "id": "lair-probe",
                    "name": "mcp-probe",
                    "dojos": [
                        {
                            "id": "dojo-1",
                            "root": {
                                "splint": {
                                    "id": "splint-1",
                                    "state": "running",
                                    "last_incarnation": 3,
                                }
                            },
                        }
                    ],
                },
            ],
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "topology.json"
            path.write_text(json.dumps(topology), encoding="utf-8")
            self.assertEqual(
                PROBE_MODULE.probe_ids(path, "mcp-probe"),
                {"lair_id": "lair-probe", "splint_id": "splint-1", "incarnation": 3},
            )
            with self.assertRaisesRegex(ValueError, "exactly one Lair"):
                PROBE_MODULE.probe_ids(path, "missing")

    def test_run_expects_the_old_handle_to_expire_and_a_new_one(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fake = root / "fake-mcp.py"
            fake.write_text(FAKE_MCP, encoding="utf-8")
            state = root / "state"
            state.mkdir()
            (state / "proceed").touch()
            result = PROBE_MODULE.run_probe(
                [sys.executable, str(fake)],
                {"splint_id": "splint-1", "incarnation": 3},
                state,
                5,
            )
            self.assertTrue(result["acquired_before"])
            self.assertTrue(result["old_handle_expired"])
            self.assertTrue(result["reacquired"])
            self.assertTrue((state / "acquired.json").is_file())
            recorded = json.loads((state / "result.json").read_text(encoding="utf-8"))
            self.assertEqual(recorded["reacquired"], True)


class RunnerUpgradeAwarenessTests(unittest.TestCase):
    def test_package_stop_and_launch_recognise_upgraded_processes(self) -> None:
        runner = RUNNER.read_text(encoding="utf-8")
        stop = runner.split("  package-stop)", 1)[1].split("  package-config)", 1)[0]
        launch = runner.split("  package-launch)", 1)[1].split("  package-stop)", 1)[0]
        for spelling in (
            "'/usr/bin/splinterm (deleted)'",
            "'/usr/bin/splinterd (deleted)'",
            "'/memfd:splinterd-client (deleted)'",
            "'/memfd:splinterd-daemon (deleted)'",
        ):
            with self.subTest(spelling=spelling):
                self.assertIn(spelling, stop)
                self.assertIn(spelling, launch)
        self.assertIn("stop_matching client", stop)
        self.assertIn("stop_matching daemon", stop)
        # Still bound to the private socket.
        self.assertIn('grep -Fqx "SPLINTERM_SOCKET=$socket"', stop)


if __name__ == "__main__":
    unittest.main()


class TrustedDialogEngagementTests(unittest.TestCase):
    """Trusted dialogs take no focus and ignore keys until a click inside."""

    def test_every_dialog_answer_follows_a_click_inside_the_dialog(self) -> None:
        answered = 0
        for step in HELPER_MODULE.STEPS:
            actions = step.actions
            for index, action in enumerate(actions):
                if not (
                    action.kind == "vm"
                    and action.vm[0] == "desktop-exec"
                    and action.note == "closed dialog-address"
                ):
                    continue
                answered += 1
                before = actions[:index]
                with self.subTest(step=step.id):
                    points = [
                        position
                        for position, earlier in enumerate(before)
                        if earlier.kind == "vm"
                        and earlier.vm[0] == "desktop-exec"
                        and earlier.note == "point_into dialog-address"
                    ]
                    self.assertTrue(points, "the dialog was never clicked")
                    clicked = [
                        earlier
                        for earlier in before[points[-1] :]
                        if HELPER_MODULE.is_input(earlier) and earlier.vm[1] == "click"
                    ]
                    self.assertEqual(len(clicked), 1)
        self.assertGreaterEqual(answered, 4)

    def test_log_checks_ignore_colour_and_survive_an_early_match(self) -> None:
        definition = next(
            line
            for line in HELPER_MODULE.PRELUDE.splitlines()
            if line.startswith("expect_log()")
        )
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "daemon.log"
            # A coloured key=value field first, then far more than a pipe
            # buffer, so a reader that stopped early would break the pipe.
            log.write_bytes(
                b"\x1b[3mdaemon_generation\x1b[0m\x1b[2m=\x1b[0m2 adopted\n"
                + b"filler line after the match\n" * 200_000
            )

            def check(needle: str) -> subprocess.CompletedProcess[str]:
                script = f'set -euo pipefail\n{definition}\nexpect_log "$1" "$2"'
                return subprocess.run(
                    ["bash", "-c", script, "_", needle, str(log)],
                    capture_output=True,
                    text=True,
                    check=False,
                )

            self.assertEqual(check("daemon_generation=2").returncode, 0)
            missing = check("daemon_generation=3")
            self.assertEqual(missing.returncode, 1)
            self.assertIn("missing", missing.stderr)

# A stand-in `splinterm-mcp` for the agent: the first Lair answers after two
# pending calls, the second stays pending, reads are refused, and the client
# name it was given is written next to it.
FAKE_ACCESS_MCP = r"""
import json, pathlib, sys
calls = {}
for line in sys.stdin:
    message = json.loads(line)
    if "id" not in message:
        continue
    method = message["method"]
    params = message.get("params", {})
    result = {"structuredContent": {"ok": True, "data": {}}}
    if method == "initialize":
        (pathlib.Path(sys.argv[1]) / "client-name").write_text(params["clientInfo"]["name"])
    elif method == "tools/call":
        name = params["name"]
        arguments = params["arguments"]
        if name == "splinterm.request_lair_access":
            lair = arguments["lair_id"]
            calls[lair] = calls.get(lair, 0) + 1
            if "cancel_request_id" in arguments:
                data = {"status": "cancelled"}
            elif lair == "lair-a" and calls[lair] >= 3:
                data = {"status": "granted", "source": "ephemeral", "grant_id": "17"}
            else:
                data = {"status": "pending", "pending_request_id": "4"}
            result = {"structuredContent": {"ok": True, "data": data}}
        elif name == "splinterm.read_terminal":
            result = {"isError": True, "structuredContent": {"ok": False, "error": {"code": "unauthorized"}}}
        elif name == "splinterm.acquire_control":
            result = {"structuredContent": {"ok": True, "data": {"controller_handle": "h1"}}}
    print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": result}), flush=True)
"""


class AccessAgentTests(unittest.TestCase):
    def test_agent_runs_numbered_commands_and_collects_pending_answers(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fake = root / "fake-mcp.py"
            fake.write_text(FAKE_ACCESS_MCP, encoding="utf-8")
            state = root / "state"
            state.mkdir()
            commands = [
                {"op": "request", "lair_id": "lair-a", "level": "look"},
                {"op": "request", "lair_id": "lair-b", "level": "manage", "until": "once"},
                {"op": "cancel", "lair_id": "lair-b", "cancel_request_id": "4"},
                {"op": "request", "lair_id": "lair-b", "level": "look", "deadline_seconds": 0},
                {"op": "read", "splint_id": "splint-1"},
                {"op": "acquire", "splint_id": "splint-1", "incarnation": 2, "takeover": True},
                {"op": "release"},
                {"op": "release"},
                {"op": "bogus"},
                {"op": "exit"},
            ]
            for number, command in enumerate(commands, start=1):
                (state / f"cmd-{number}.json").write_text(json.dumps(command), encoding="utf-8")
            name = "agent\x1b[31m‮evil"
            status = PROBE_MODULE.run_agent(
                [sys.executable, str(fake), str(root)], state, name, 5, 0.01
            )
            self.assertEqual(status, 0)
            out = [
                json.loads((state / f"out-{number}.json").read_text(encoding="utf-8"))
                for number in range(1, len(commands) + 1)
            ]
            self.assertEqual(out[0]["status"], "granted")
            self.assertEqual(out[0]["calls"], 3)
            self.assertEqual(out[0]["grant_id"], "17")
            self.assertEqual(out[1]["status"], "pending")
            self.assertEqual(out[1]["calls"], 1)
            self.assertEqual(out[1]["pending_request_id"], "4")
            self.assertEqual(out[2]["status"], "cancelled")
            self.assertTrue(out[3]["deadline_passed"])
            self.assertEqual(out[4], {"op": "read", "ok": False, "code": "unauthorized"})
            self.assertTrue(out[5]["handle"])
            self.assertTrue(out[6]["ok"])
            self.assertEqual(out[7]["code"], "no_handle")
            self.assertEqual(out[8]["code"], "bad_command")
            self.assertEqual(out[9], {"op": "exit", "ok": True})
            self.assertEqual((root / "client-name").read_text(), name)
            self.assertTrue((state / "agent.pid").is_file())

    def test_agent_gives_up_when_no_command_arrives(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fake = root / "fake-mcp.py"
            fake.write_text(FAKE_ACCESS_MCP, encoding="utf-8")
            with self.assertRaises(RuntimeError):
                PROBE_MODULE.run_agent(
                    [sys.executable, str(fake), str(root)], root / "state", "agent", 0.2, 0.01
                )

    def test_agent_waits_out_a_half_written_command_and_survives_a_bad_one(self) -> None:
        import threading
        from unittest import mock

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fake = root / "fake-mcp.py"
            fake.write_text(FAKE_ACCESS_MCP, encoding="utf-8")
            state = root / "state"
            state.mkdir()
            (state / "cmd-1.json").write_text("[1, 2]", encoding="utf-8")

            def publish_late() -> None:
                # Written in place while the agent already polls: first a
                # truncated object, completed a moment later.
                time.sleep(0.3)
                path = state / "cmd-2.json"
                path.write_text('{"op": "request", "lair_id": "lair-b", "le', encoding="utf-8")
                time.sleep(0.3)
                path.write_text(
                    json.dumps({"op": "request", "lair_id": "lair-b", "level": "look", "until": "once"}),
                    encoding="utf-8",
                )
                (state / "cmd-3.json").write_text('{"op": "exit"}', encoding="utf-8")

            writer = threading.Thread(target=publish_late)
            writer.start()
            with mock.patch.object(PROBE_MODULE, "COMMAND_SETTLE_SECONDS", 1.0):
                status = PROBE_MODULE.run_agent(
                    [sys.executable, str(fake), str(root)], state, "agent", 5, 0.01
                )
            writer.join()
            self.assertEqual(status, 0)
            first = json.loads((state / "out-1.json").read_text(encoding="utf-8"))
            self.assertEqual(first["code"], "bad_command")
            second = json.loads((state / "out-2.json").read_text(encoding="utf-8"))
            self.assertEqual(second["status"], "pending")
            third = json.loads((state / "out-3.json").read_text(encoding="utf-8"))
            self.assertEqual(third, {"op": "exit", "ok": True})
