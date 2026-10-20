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
                        self.assertIn(action.vm[1], ("key", "type"))

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
