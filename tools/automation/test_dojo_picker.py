#!/usr/bin/env python3
"""Contract tests for the public-CLI-only reference Dojo picker."""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
PICKER = HERE / "splinterm-dojo-picker.py"
SPEC = importlib.util.spec_from_file_location("dojo_picker", PICKER)
assert SPEC and SPEC.loader
picker = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = picker
SPEC.loader.exec_module(picker)

LAIR = "018f4d8c-2a18-4b31-8c2f-9e7c5de77101"
DOJO = "018f4d8c-2a18-4b31-8c2f-9e7c5de77102"
SPLINT = "018f4d8c-2a18-4b31-8c2f-9e7c5de77103"
INCARNATION = 2


def envelope(operation: str, data: dict, resource: dict | None = None) -> dict:
    document = {
        "schema": "splinterm.cli.v2",
        "request_id": "1",
        "operation": operation,
        "ok": True,
        "truncated": False,
        "data": data,
    }
    if resource is not None:
        document["resource"] = resource
    return document


TOPOLOGY = envelope(
    "inspect_topology",
    {
        "lairs": [{"lair_id": LAIR, "name": "main", "dojo_count": 1}],
        "dojos": [{"lair_id": LAIR, "dojo_id": DOJO, "name": "editor"}],
        "splints": [
            {
                "lair_id": LAIR,
                "dojo_id": DOJO,
                "splint_id": SPLINT,
                "title": "shell",
                "lifecycle": "running",
                "current_incarnation": INCARNATION,
                "last_incarnation": INCARNATION,
            }
        ],
    },
    {"topology_revision": 7},
)


FAKE_CLI = r'''#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
with open(os.environ["FAKE_LOG"], "a", encoding="utf-8") as stream:
    stream.write(json.dumps(args) + "\n")
topology = json.loads(os.environ["FAKE_TOPOLOGY"])
mode = os.environ.get("FAKE_MODE")
if mode == "hang":
    import time
    time.sleep(30)
if mode == "orphan_pipe":
    import time
    child = os.fork()
    if child == 0:
        time.sleep(30)
        os._exit(0)
    with open(os.environ["FAKE_CHILD_PID"], "w", encoding="utf-8") as stream:
        stream.write(str(child))
    os._exit(0)
if mode == "oversized_stdout":
    sys.stdout.write("x" * (8 * 1024 * 1024 + 1))
    raise SystemExit(0)
if mode == "stderr_flood" and "subscribe" in args:
    sys.stderr.write("x" * (64 * 1024 + 1))
    sys.stderr.flush()
if mode == "subscription_failure" and "subscribe" in args:
    raise SystemExit(6)
if mode == "partial_event" and "subscribe" in args:
    sys.stdout.write('{"schema":"splinterm.cli.event.v2"')
    raise SystemExit(0)
if mode == "access_revoked" and "subscribe" in args:
    print(json.dumps({
        "schema":"splinterm.cli.event.v2", "subscription_id":"1", "sequence":1,
        "event_type":"access_revoked", "stream":"terminal",
        "resource":{"splint_id":os.environ["FAKE_SPLINT"],"incarnation":2},
        "data":{"grant_id":"9"}, "truncated":False
    }))
    raise SystemExit(0)
if mode == "malformed_topology":
    topology["data"]["dojos"] = [{}]
if "window" in args and "--output" not in args:
    raise SystemExit(0)
if "subscribe" in args:
    snapshot_data = ({
        "content_encoding":"unicode_scalars", "columns":80, "rows":24,
        "title":"shell", "visible_rows":[]
    } if mode != "malformed_snapshot" else {})
    print(json.dumps({
        "schema":"splinterm.cli.event.v2", "subscription_id":"1", "sequence":1,
        "event_type":"snapshot", "stream":"terminal",
        "resource":{"splint_id":os.environ["FAKE_SPLINT"],"incarnation":2,
                    "terminal_revision":1,"history_generation":1},
        "data":snapshot_data, "truncated":False
    }))
    if mode in {"snapshot_eof", "malformed_snapshot"}:
        raise SystemExit(0)
    print(json.dumps({
        "schema":"splinterm.cli.event.v2", "subscription_id":"1", "sequence":2,
        "event_type":"resync_required", "stream":"terminal",
        "resource":{"splint_id":os.environ["FAKE_SPLINT"],"incarnation":2,
                    "terminal_revision":2,"history_generation":1},
        "data":{}, "truncated":False, "resync":{"reason":"subscriber_stalled"}
    }))
    raise SystemExit(0)
if args[-1:] == ["topology"]:
    print(json.dumps(topology))
    raise SystemExit(0)
if "snapshot" in args:
    print(json.dumps({
        "schema":"splinterm.cli.v2", "request_id":"1", "operation":"terminal_snapshot",
        "ok":True, "truncated":False,
        "resource":{"lair_id":os.environ["FAKE_LAIR"],"dojo_id":os.environ["FAKE_DOJO"],
                    "splint_id":os.environ["FAKE_SPLINT"],"incarnation":2,
                    "terminal_revision":2,"history_generation":1},
        "data":{"content_encoding":"unicode_scalars","columns":80,"rows":[],
                "cursor":{"column":0,"row":0,"visible":True},"continuation_cursor":None}
    }))
    raise SystemExit(0)
operation = ("new_dojo" if "new-dojo" in args else
             "input" if "send" in args else "split_splint")
if os.environ.get("FAKE_DENY") == operation:
    code = os.environ.get("FAKE_ERROR_CODE", "unauthorized")
    status = 3 if code == "unauthorized" else 5
    print(json.dumps({
        "schema":"splinterm.cli.v2", "request_id":"1", "operation":operation,
        "ok":False, "truncated":False,
        "error":{"code":code,"message":"policy denied","retryable":False}
    }))
    raise SystemExit(status)
print(json.dumps({
    "schema":"splinterm.cli.v2", "request_id":"1", "operation":operation,
    "ok":True, "truncated":False,
    "resource":{"lair_id":os.environ["FAKE_LAIR"],"dojo_id":os.environ["FAKE_DOJO"],
                "splint_id":os.environ["FAKE_SPLINT"],"incarnation":3,"topology_revision":8},
    "data":{"created":True}
}))
'''


class DojoPickerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="splinterm-picker-")
        self.root = Path(self.temporary.name)
        self.fake = self.root / "splinterm"
        self.fake.write_text(FAKE_CLI, encoding="utf-8")
        self.fake.chmod(0o755)
        self.log = self.root / "calls.jsonl"
        self.environment = os.environ.copy()
        self.environment.update(
            {
                "FAKE_LOG": str(self.log),
                "FAKE_TOPOLOGY": json.dumps(TOPOLOGY),
                "FAKE_LAIR": LAIR,
                "FAKE_DOJO": DOJO,
                "FAKE_SPLINT": SPLINT,
                "FAKE_CHILD_PID": str(self.root / "child.pid"),
                "SPLINTERM_LAIR_ID": LAIR,
                "SPLINTERM_DOJO_ID": DOJO,
                "SPLINTERM_SPLINT_ID": SPLINT,
                "SPLINTERM_SPLINT_INCARNATION": str(INCARNATION),
            }
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def run_picker(self, *arguments: str, environment: dict[str, str] | None = None):
        return subprocess.run(
            [sys.executable, str(PICKER), "--splinterm", str(self.fake), *arguments],
            env=environment or self.environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            check=False,
        )

    def calls(self) -> list[list[str]]:
        return [json.loads(line) for line in self.log.read_text(encoding="utf-8").splitlines()]

    def test_checked_in_topology_fixture_uses_supported_schema(self) -> None:
        fixture = json.loads(
            (REPO / "tests/automation/fixtures/valid/cli-inspect-topology.json").read_text()
        )["document"]
        parsed = picker.parse_json_document(json.dumps(fixture).encode(), "inspect_topology")
        self.assertEqual(parsed["schema"], picker.SCHEMA)
        with self.assertRaises(picker.ClientError):
            picker.parse_json_document(json.dumps({**fixture, "schema": "future.v2"}).encode(), "inspect_topology")

    def test_context_is_only_accepted_after_authoritative_reconciliation(self) -> None:
        completed = self.run_picker("context")
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(json.loads(completed.stdout)["splint_id"], SPLINT)

        stale = self.environment.copy()
        stale["SPLINTERM_SPLINT_INCARNATION"] = "99"
        completed = self.run_picker("context", environment=stale)
        self.assertEqual(completed.returncode, 5)
        self.assertIn("stale", completed.stderr)
        self.assertEqual(completed.stdout, "")

    def test_direct_argv_is_never_joined_or_interpreted(self) -> None:
        dangerous = "$(touch should-not-exist); spaced value"
        completed = self.run_picker(
            "start", LAIR, "--name", "task", "--cwd", str(self.root), "--",
            "/usr/bin/editor", "--flag", dangerous,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        call = self.calls()[-1]
        separator = call.index("--")
        self.assertEqual(call[separator + 1 :], ["/usr/bin/editor", "--flag", dangerous])
        self.assertFalse((self.root / "should-not-exist").exists())

    def test_split_context_carries_exact_incarnation_and_structured_child(self) -> None:
        completed = self.run_picker(
            "split-context",
            "--axis",
            "vertical",
            "--side",
            "second",
            "--",
            "/bin/echo",
            "child output is data",
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        call = self.calls()[-1]
        expected = call.index("--expected-incarnation")
        self.assertEqual(call[expected + 1], str(INCARNATION))
        separator = call.index("--")
        self.assertEqual(call[separator + 1 :], ["/bin/echo", "child output is data"])

    def test_controller_denial_is_bounded_and_preserves_exit_category(self) -> None:
        denied = self.environment.copy()
        denied["FAKE_DENY"] = "input"
        completed = self.run_picker(
            "send-context", "literal input",
            environment=denied,
        )
        self.assertEqual(completed.returncode, 3)
        self.assertEqual(completed.stdout, "")
        self.assertIn("unauthorized: policy denied", completed.stderr)
        call = self.calls()[-1]
        expected = call.index("--expected-incarnation")
        self.assertEqual(call[expected + 1], str(INCARNATION))

    def test_open_uses_validated_dojo_ids_without_machine_output(self) -> None:
        completed = self.run_picker("open", DOJO)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(
            self.calls()[-1],
            ["window", "--lair-id", LAIR, "--dojo-id", DOJO],
        )

    def test_terminal_resync_rebuilds_topology_and_snapshot(self) -> None:
        completed = self.run_picker("watch-context")
        self.assertEqual(completed.returncode, 0, completed.stderr)
        result = json.loads(completed.stdout)
        self.assertEqual(result["status"], "reconciled_after_resync")
        calls = self.calls()
        self.assertEqual(sum(call[-1:] == ["topology"] for call in calls), 2)
        self.assertTrue(any("subscribe" in call for call in calls))
        self.assertTrue(any("snapshot" in call for call in calls))
        for call in calls:
            if "subscribe" in call or "snapshot" in call:
                expected = call.index("--expected-incarnation")
                self.assertEqual(call[expected + 1], str(INCARNATION))

    def test_subscription_failure_is_not_reported_as_success(self) -> None:
        failed = self.environment.copy()
        failed["FAKE_MODE"] = "subscription_failure"
        completed = self.run_picker("--timeout-ms", "100", "watch-context", environment=failed)
        self.assertEqual(completed.returncode, 6)
        self.assertEqual(completed.stdout, "")
        self.assertIn("subscription process failed", completed.stderr)

    def test_access_revocation_and_stale_incarnation_are_explicit(self) -> None:
        revoked = self.environment.copy()
        revoked["FAKE_MODE"] = "access_revoked"
        completed = self.run_picker("watch-context", environment=revoked)
        self.assertEqual(completed.returncode, 3)
        self.assertIn("access_revoked", completed.stderr)

        stale = self.environment.copy()
        stale["FAKE_DENY"] = "input"
        stale["FAKE_ERROR_CODE"] = "stale_incarnation"
        completed = self.run_picker("send-context", "text", environment=stale)
        self.assertEqual(completed.returncode, 5)
        self.assertIn("stale_incarnation", completed.stderr)

    def test_malformed_topology_is_rejected_without_traceback(self) -> None:
        malformed = self.environment.copy()
        malformed["FAKE_MODE"] = "malformed_topology"
        completed = self.run_picker("list", environment=malformed)
        self.assertEqual(completed.returncode, 4)
        self.assertIn("invalid Dojo summary", completed.stderr)
        self.assertNotIn("Traceback", completed.stderr)

    def test_malformed_snapshot_and_premature_clean_eof_fail(self) -> None:
        for mode, diagnostic in (
            ("malformed_snapshot", "snapshot event"),
            ("snapshot_eof", "ended before a terminal event"),
        ):
            with self.subTest(mode=mode):
                failed = self.environment.copy()
                failed["FAKE_MODE"] = mode
                completed = self.run_picker("watch-context", environment=failed)
                self.assertEqual(completed.returncode, 4)
                self.assertIn(diagnostic, completed.stderr)

    def test_partial_event_and_stderr_flood_fail_bounded(self) -> None:
        for mode, diagnostic in (
            ("partial_event", "partial NDJSON"),
            ("stderr_flood", "stderr exceeds"),
        ):
            with self.subTest(mode=mode):
                failed = self.environment.copy()
                failed["FAKE_MODE"] = mode
                completed = self.run_picker(
                    "--timeout-ms", "100", "watch-context", environment=failed
                )
                self.assertEqual(completed.returncode, 4)
                self.assertIn(diagnostic, completed.stderr)

    def test_leader_exit_still_reaps_pipe_holding_process_group(self) -> None:
        failed = self.environment.copy()
        failed["FAKE_MODE"] = "orphan_pipe"
        completed = self.run_picker("--timeout-ms", "20", "list", environment=failed)
        self.assertEqual(completed.returncode, 6)
        child_pid = int((self.root / "child.pid").read_text())
        deadline = time.monotonic() + 2
        while Path(f"/proc/{child_pid}").exists() and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertFalse(Path(f"/proc/{child_pid}").exists())

    def test_one_shot_deadline_and_output_bound_reap_child(self) -> None:
        for mode, expected_code, diagnostic in (
            ("hang", 6, "deadline elapsed"),
            ("oversized_stdout", 4, "stdout exceeds"),
        ):
            with self.subTest(mode=mode):
                failed = self.environment.copy()
                failed["FAKE_MODE"] = mode
                completed = self.run_picker(
                    "--timeout-ms", "20", "list", environment=failed
                )
                self.assertEqual(completed.returncode, expected_code)
                self.assertIn(diagnostic, completed.stderr)


if __name__ == "__main__":
    unittest.main()
