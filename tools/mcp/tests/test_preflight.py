#!/usr/bin/env python3
"""Identity, isolation, classification, and cleanup tests for the preflight.

Full runs use fake `splinterd`/`splinterm-mcp` stand-ins so every partial-start
and cleanup boundary is exercised without real binaries. The real staged
preflight (see README.md) remains the M1 acceptance gate.
"""

from __future__ import annotations

import copy
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

import mcp_preflight  # noqa: E402
from mcp_host import ToolOutcome  # noqa: E402
from mcp_preflight import (  # noqa: E402
    COMMON_PREFIX,
    DEFAULT_SCHEMA_ROOT,
    REQUIRED_TOOLS,
    Hooks,
    Options,
    Preflight,
    PreflightAbort,
    PrivateRoot,
    classify_deny,
    classify_ping,
    evaluate_catalog,
    inspect_binary,
    identity_still_matches,
    isolated_environment,
    path_shadowing,
    process_alive,
    processes_referencing,
    select_binaries,
    wire_input_schema,
    wire_output_schema,
)
sys.path.insert(0, str(HERE))
from fake_adapter import catalog as _fake_catalog  # noqa: E402


def fake_catalog() -> list[dict]:
    return _fake_catalog(None)


FAKE_ADAPTER = HERE / "fake_adapter.py"
FAKE_DAEMON = HERE / "fake_daemon.py"
ROLES = {"daemon": "splinterd", "client": "splinterm", "mcp": "splinterm-mcp"}


def fake_report(version: tuple[int, int, int] = (0, 2, 0), identity: bytes = b"\x11" * 32) -> bytes:
    frame = bytearray(98)
    frame[:8] = b"SPLTPF01"
    frame[44:76] = identity
    for index, part in enumerate(version):
        frame[76 + index * 2:78 + index * 2] = part.to_bytes(2, "big")
    return bytes(frame)


def short_temporary() -> tempfile.TemporaryDirectory:
    # Keep Unix socket paths short regardless of TMPDIR.
    return tempfile.TemporaryDirectory(prefix="mcp-t-", dir="/tmp" if os.path.isdir("/tmp") else None)


class FakeSet:
    """A staged bin dir of fake executables plus matching hooks."""

    def __init__(self, base: Path, daemon_mode: str = "normal", adapter_mode: str = "server") -> None:
        self.bin = base / "bin"
        self.bin.mkdir()
        self.write("splinterd", FAKE_DAEMON, daemon_mode, "daemon")
        self.write("splinterm-mcp", FAKE_ADAPTER, adapter_mode, "mcp")
        client = self.bin / "splinterm"
        client.write_text("#!/bin/sh\nexit 0\n")
        client.chmod(0o755)

    def write(self, name: str, script: Path, mode: str, role: str) -> None:
        path = self.bin / name
        path.write_text(f'#!/bin/sh\nexec "{sys.executable}" "{script}" --mode {mode} --role {role} "$@"\n')
        path.chmod(0o755)

    def executable(self, pid: int) -> tuple[int, int] | None:
        """Map a fake's /proc cmdline back to the staged file it stands for."""
        for _ in range(100):
            try:
                argv = [part.decode() for part in Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0") if part]
            except OSError:
                return None
            name = None
            if "--role" in argv:
                name = ROLES[argv[argv.index("--role") + 1]]
            else:
                for part in argv:
                    if Path(part).parent == self.bin:
                        name = Path(part).name
            if name is not None:
                current = os.stat(self.bin / name)
                return (current.st_dev, current.st_ino)
            time.sleep(0.01)
        return None


class RunTestCase(unittest.TestCase):
    def setUp(self) -> None:
        temporary = short_temporary()
        self.addCleanup(temporary.cleanup)
        self.base = Path(temporary.name)
        self.tmp_base = self.base / "tmp"
        self.tmp_base.mkdir()
        self.production = self.base / "production"
        (self.production / "state" / "splinterm").mkdir(parents=True)
        (self.production / "state" / "splinterm" / "topology.json").write_text('{"lairs": []}')
        (self.production / "run").mkdir()
        self.production_env = {
            "HOME": str(self.production),
            "XDG_STATE_HOME": str(self.production / "state"),
            "XDG_RUNTIME_DIR": str(self.production / "run"),
            "XDG_CONFIG_HOME": str(self.production / "config"),
        }

    def run_preflight(
        self,
        daemon_mode: str = "normal",
        adapter_mode: str = "server",
        evidence: bool = True,
        sealed=None,
        after_daemon_ready=None,
        remove: tuple[str, ...] = (),
        stop_timeout: float = 10,
    ) -> tuple[Preflight, mcp_preflight.Report]:
        fakes = FakeSet(self.base, daemon_mode, adapter_mode)
        for name in remove:
            (fakes.bin / name).unlink()
        evidence_dir = self.base / "evidence"
        evidence_dir.mkdir(exist_ok=True)
        options = Options(
            evidence_base=evidence_dir if evidence else None,
            temporary_base=self.tmp_base,
            request_timeout=10,
            daemon_ready_timeout=10,
            daemon_stop_timeout=stop_timeout,
            exit_timeout=5,
            production_environment=self.production_env,
            hooks=Hooks(
                sealed_report=sealed or (lambda path, role: fake_report()),
                process_executable=fakes.executable,
                after_daemon_ready=after_daemon_ready,
            ),
        )
        runner = Preflight(select_binaries("staged", fakes.bin), options)
        report = runner.run()
        return runner, report

    def assert_no_residue(self, runner: Preflight) -> None:
        self.assertEqual(list(self.tmp_base.iterdir()), [], "private root left behind")
        if runner.root is not None:
            self.assertEqual(processes_referencing(runner.root.path), [])
        if runner.daemon is not None:
            self.assertIsNotNone(runner.daemon.returncode)
        for host in runner.hosts:
            self.assertEqual(host.state, "closed")
            with self.assertRaises(ProcessLookupError):
                os.kill(host.pid, 0)

    def checks(self, report: mcp_preflight.Report) -> dict[str, str]:
        return {check.name: check.status for check in report.checks}


class FullRunTests(RunTestCase):
    def test_passing_run_records_honest_private_evidence(self) -> None:
        runner, report = self.run_preflight()
        failed = [(check.name, check.code) for check in report.failed_checks()]
        self.assertEqual(report.result, "passed", failed)
        self.assertEqual(report.state, "clean")
        self.assertEqual([t.state for t in report.transitions], ["daemon_ready", "preflight_passed", "cleaning", "clean"])
        self.assertEqual(len(report.negative_cases), 8)
        self.assertTrue(all(case.status == "passed" for case in report.negative_cases))
        self.assertEqual(report.catalog.count, 12)
        self.assertIn("splinterm.report_agent_status", report.catalog.additional)
        self.assert_no_residue(runner)
        directory = runner.evidence.directory
        assert directory is not None and runner.root is not None
        blob = "".join(path.read_text() for path in directory.iterdir())
        for forbidden in (str(runner.root.path), "SPLINTERM_SOCKET", "FAKE-TERMINAL-SECRET", "controller_handle", str(self.production)):
            self.assertNotIn(forbidden, blob)
        report_json = json.loads((directory / "report.json").read_text())
        self.assertEqual(report_json["result"], "passed")
        self.assertTrue(json.loads((directory / "cleanup.json").read_text())["complete"])

    def test_repeated_runs_leave_no_residue(self) -> None:
        for _ in range(2):
            runner, report = self.run_preflight(evidence=False)
            self.assertEqual(report.result, "passed")
            self.assert_no_residue(runner)
            shutil.rmtree(self.base / "bin")

    def test_missing_binary_fails_before_any_process(self) -> None:
        runner, report = self.run_preflight(remove=("splinterm-mcp",))
        self.assertEqual(report.result, "preflight_failed")
        self.assertEqual(self.checks(report)["mcp_binary"], "failed")
        self.assertIsNone(runner.daemon)
        self.assertIsNone(runner.root)
        self.assertTrue(report.cleanup_complete())
        self.assert_no_residue(runner)

    def test_build_pair_mismatch_stops_before_daemon(self) -> None:
        reports = {1: fake_report(identity=b"\x01" * 32), 2: fake_report(identity=b"\x02" * 32)}
        runner, report = self.run_preflight(sealed=lambda path, role: reports[role])
        self.assertEqual(self.checks(report)["build_pair_coherent"], "failed")
        self.assertIsNone(runner.daemon)
        self.assertEqual(report.result, "preflight_failed")

    def test_sealed_report_failure_is_a_coherence_failure(self) -> None:
        def broken(path: Path, role: int) -> bytes:
            raise AssertionError("child failed")

        runner, report = self.run_preflight(sealed=broken)
        self.assertEqual(self.checks(report)["build_pair_coherent"], "failed")
        self.assertIsNone(runner.daemon)

    def test_version_skew_between_pair_and_adapter_fails(self) -> None:
        runner, report = self.run_preflight(sealed=lambda path, role: fake_report(version=(9, 9, 9)))
        self.assertEqual(self.checks(report)["mcp_version_coherent"], "failed")
        self.assertEqual(report.result, "preflight_failed")
        self.assert_no_residue(runner)

    def test_daemon_exiting_early_cleans_up(self) -> None:
        runner, report = self.run_preflight(daemon_mode="exit-early")
        self.assertEqual(report.result, "preflight_failed")
        self.assertEqual(self.checks(report)["daemon_ready"], "failed")
        self.assertTrue(report.cleanup_complete())
        self.assert_no_residue(runner)

    def test_sigterm_resistant_daemon_is_killed_exactly(self) -> None:
        runner, report = self.run_preflight(daemon_mode="ignore-term", stop_timeout=1)
        self.assertEqual(self.checks(report)["daemon_clean_exit"], "failed")
        self.assertEqual(report.result, "preflight_failed")
        self.assertTrue(report.cleanup_complete())
        self.assertEqual(runner.daemon.returncode, -9)
        self.assert_no_residue(runner)

    def test_surviving_descendant_is_reaped_by_exact_identity(self) -> None:
        runner, report = self.run_preflight(daemon_mode="spawn-child")
        self.assertEqual(len(runner.descendants), 1)
        (pid, start), = runner.descendants
        self.assertFalse(process_alive(pid, start))
        steps = {step.name: step for step in report.cleanup}
        self.assertEqual(steps["descendants_exited"].status, "passed")
        self.assert_no_residue(runner)

    def test_socket_left_by_daemon_is_reported_and_removed(self) -> None:
        runner, report = self.run_preflight(daemon_mode="leave-socket")
        steps = {step.name: step for step in report.cleanup}
        self.assertEqual(steps["socket_absent"].status, "passed")
        self.assertEqual(steps["socket_absent"].detail, "stale socket removed by harness")
        self.assertEqual(self.checks(report)["daemon_removed_socket"], "failed")
        self.assertEqual(report.result, "preflight_failed")
        self.assert_no_residue(runner)

    def test_generic_error_is_not_deny_proof(self) -> None:
        runner, report = self.run_preflight(adapter_mode="server-deny-internal")
        self.assertEqual(self.checks(report)["deny_all_topology"], "failed")
        self.assertEqual(report.result, "preflight_failed")
        self.assert_no_residue(runner)

    def test_topology_leak_fails_and_is_not_recorded(self) -> None:
        runner, report = self.run_preflight(adapter_mode="server-leak")
        self.assertEqual(self.checks(report)["deny_all_topology"], "failed")
        blob = "".join(path.read_text() for path in runner.evidence.directory.iterdir())
        self.assertNotIn("FAKE-TERMINAL-SECRET", blob)

    def test_missing_required_tool_fails(self) -> None:
        runner, report = self.run_preflight(adapter_mode="server-missing-tool")
        self.assertEqual(self.checks(report)["required_tools_present"], "failed")
        self.assertEqual(report.catalog.required_missing, ["splinterm.revoke_access"])
        self.assert_no_residue(runner)

    def test_interrupt_after_daemon_start_is_aborted_and_clean(self) -> None:
        def interrupt() -> None:
            raise KeyboardInterrupt

        runner, report = self.run_preflight(after_daemon_ready=interrupt)
        self.assertEqual(report.result, "aborted")
        self.assertTrue(report.cleanup_complete())
        self.assertEqual(report.transitions[-2].result, "interrupted")
        self.assert_no_residue(runner)

    def test_production_change_is_detected(self) -> None:
        topology = self.production / "state" / "splinterm" / "topology.json"

        def touch_production() -> None:
            topology.write_text('{"lairs": ["changed"]}')

        runner, report = self.run_preflight(after_daemon_ready=touch_production)
        self.assertEqual(self.checks(report)["production_state_unchanged"], "failed")
        self.assertEqual(report.result, "preflight_failed")

    def test_cli_reports_and_exits_by_result(self) -> None:
        fakes = FakeSet(self.base)
        (fakes.bin / "splinterd").unlink()
        completed = subprocess.run(
            [sys.executable, str(HERE.parent / "run-demo.py"), "preflight", "--staged-bin-dir", str(fakes.bin)],
            capture_output=True, text=True, timeout=60,
        )
        self.assertEqual(completed.returncode, 1, completed.stderr)
        summary = json.loads(completed.stdout)
        self.assertEqual(summary["result"], "preflight_failed")
        self.assertIn("daemon_binary", summary["checks_failed"])
        self.assertTrue(summary["cleanup_complete"])

    def test_cli_usage_errors_and_no_graphical_lane(self) -> None:
        for argv in ([], ["preflight"], ["graphical"]):
            completed = subprocess.run(
                [sys.executable, str(HERE.parent / "run-demo.py"), *argv],
                capture_output=True, text=True, timeout=60,
            )
            self.assertEqual(completed.returncode, 64, argv)


class IdentityTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = short_temporary()
        self.addCleanup(temporary.cleanup)
        self.base = Path(temporary.name)

    def executable(self, name: str, mode: int = 0o755) -> Path:
        path = self.base / name
        path.write_bytes(b"#!/bin/sh\nexit 0\n")
        path.chmod(mode)
        return path

    def test_identity_fields(self) -> None:
        path = self.executable("splinterd")
        identity = inspect_binary("splinterd", path)
        self.assertEqual(identity.canonical_path, str(path))
        self.assertEqual(identity.inode, path.stat().st_ino)
        self.assertEqual(identity.mode, "0755")
        self.assertTrue(identity_still_matches(identity))

    def test_rejections(self) -> None:
        cases = {
            "binary_missing": self.base / "absent",
            "binary_not_executable": self.executable("plain", 0o644),
            "binary_world_writable": self.executable("loose", 0o757),
        }
        directory = self.base / "dir"
        directory.mkdir()
        cases["binary_not_regular"] = directory
        for code, path in cases.items():
            with self.subTest(code=code):
                with self.assertRaises(PreflightAbort) as caught:
                    inspect_binary("splinterd", path)
                self.assertEqual(caught.exception.code, code)

    def test_symlink_records_canonical_target(self) -> None:
        target = self.executable("real-splinterd")
        link = self.base / "splinterd"
        link.symlink_to(target)
        identity = inspect_binary("splinterd", link)
        self.assertEqual(identity.path, str(link))
        self.assertEqual(identity.canonical_path, str(target))

    def test_replacement_is_detected(self) -> None:
        path = self.executable("splinterd")
        identity = inspect_binary("splinterd", path)
        path.unlink()
        path.write_bytes(b"#!/bin/sh\nexit 1\n")  # same size, maybe same inode
        path.chmod(0o755)
        self.assertFalse(identity_still_matches(identity))

    def test_staged_mode_refuses_installed_prefix(self) -> None:
        with self.assertRaises(PreflightAbort) as caught:
            select_binaries("staged", Path("/usr/bin"))
        self.assertEqual(caught.exception.code, "staged_dir_is_installed_prefix")
        selection = select_binaries("installed", Path("/usr/bin"))
        self.assertEqual(selection.paths["splinterm-mcp"], Path("/usr/bin/splinterm-mcp"))

    def test_path_shadowing(self) -> None:
        selected = self.base / "selected"
        other = self.base / "other"
        selected.mkdir()
        other.mkdir()
        (selected / "splinterm").write_bytes(b"#!/bin/sh\n")
        (selected / "splinterm").chmod(0o755)
        identity = inspect_binary("splinterm", selected / "splinterm")
        self.assertEqual(path_shadowing("splinterm", identity, str(selected)), "ok")
        self.assertEqual(path_shadowing("splinterm", identity, str(other)), "absent")
        (other / "splinterm").write_bytes(b"#!/bin/sh\n")
        (other / "splinterm").chmod(0o755)
        self.assertEqual(path_shadowing("splinterm", identity, f"{other}:{selected}"), "shadowed")


class IsolationTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = short_temporary()
        self.addCleanup(temporary.cleanup)
        self.base = Path(temporary.name)

    def test_environment_ignores_hostile_ambient_values(self) -> None:
        hostile = {
            "HOME": "/home/victim",
            "WAYLAND_DISPLAY": "wayland-1",
            "DISPLAY": ":0",
            "DBUS_SESSION_BUS_ADDRESS": "unix:path=/run/user/1000/bus",
            "SPLINTERM_POLICY": "/home/victim/.config/splinterm/policy.json",
            "SPLINTERM_SOCKET": "/run/user/1000/splinterm/splinterd.sock",
            "SPLINTERM_SPLINT_ID": "11111111-2222-4333-8444-555555555555",
            "SPLINTERM_ENABLE_DEV_ATTACH": "1",
            "HYPRLAND_INSTANCE_SIGNATURE": "abc",
        }
        root = PrivateRoot.create(self.base, "0123456789abcdef")
        with mock.patch.dict(os.environ, hostile):
            environment = isolated_environment(root)
        for name in ("WAYLAND_DISPLAY", "DISPLAY", "DBUS_SESSION_BUS_ADDRESS", "SPLINTERM_POLICY",
                     "SPLINTERM_SPLINT_ID", "SPLINTERM_ENABLE_DEV_ATTACH", "HYPRLAND_INSTANCE_SIGNATURE"):
            self.assertNotIn(name, environment)
        for name in ("HOME", "XDG_CONFIG_HOME", "XDG_STATE_HOME", "XDG_RUNTIME_DIR", "XDG_CACHE_HOME",
                     "XDG_DATA_HOME", "TMPDIR", "SPLINTERM_SOCKET"):
            self.assertTrue(environment[name].startswith(str(root.path) + "/"), name)
        self.assertNotIn("/home/victim", json.dumps(environment))

    def test_private_root_is_exclusive_and_private(self) -> None:
        root = PrivateRoot.create(self.base, "0123456789abcdef")
        self.assertEqual(stat.S_IMODE(root.path.stat().st_mode), 0o700)
        with self.assertRaises(FileExistsError):
            PrivateRoot.create(self.base, "0123456789abcdef")
        self.assertEqual(root.remove(), "removed")
        self.assertEqual(root.remove(), "absent")

    def test_replaced_root_is_never_removed(self) -> None:
        root = PrivateRoot.create(self.base, "0123456789abcdef")
        moved = self.base / "moved"
        root.path.rename(moved)
        victim = self.base / "victim"
        victim.mkdir()
        (victim / "keep.txt").write_text("keep")
        root.path.symlink_to(victim)
        self.assertEqual(root.remove(), "refused_not_owned")
        self.assertEqual((victim / "keep.txt").read_text(), "keep")
        root.path.unlink()
        root.path.mkdir()  # same name, different inode
        self.assertEqual(root.remove(), "refused_not_owned")
        self.assertTrue(root.path.is_dir())

    def test_removal_does_not_follow_inner_symlinks(self) -> None:
        root = PrivateRoot.create(self.base, "0123456789abcdef")
        outside = self.base / "outside"
        outside.mkdir()
        (outside / "keep.txt").write_text("keep")
        (root.path / "home" / "escape").symlink_to(outside)
        self.assertEqual(root.remove(), "removed")
        self.assertEqual((outside / "keep.txt").read_text(), "keep")


class ClassificationTests(unittest.TestCase):
    def envelope(self, code: str, **extra) -> dict:
        value = {
            "schema": "splinterm.mcp.v2", "tool": "splinterm.inspect_topology", "ok": False,
            "error": {"code": code, "message": "m", "retryable": False},
            "truncated": False, "content_trust": "trusted_metadata",
        }
        value.update(extra)
        return value

    def test_deny_requires_structured_unauthorized(self) -> None:
        tool = "splinterm.inspect_topology"
        self.assertEqual(classify_deny(ToolOutcome(True, self.envelope("unauthorized"), "unauthorized"), tool), "unauthorized")
        self.assertEqual(classify_deny(ToolOutcome(True, self.envelope("internal"), "internal"), tool), "wrong_error_internal")
        self.assertEqual(classify_deny(ToolOutcome(False, {"data": {}}, None), tool), "not_denied")
        self.assertEqual(classify_deny(ToolOutcome(True, None, None), tool), "unstructured_error")
        leaked = self.envelope("unauthorized", data={"lairs": []})
        self.assertEqual(classify_deny(ToolOutcome(True, leaked, "unauthorized"), tool), "unstructured_error")
        other = self.envelope("unauthorized", tool="splinterm.ping")
        self.assertEqual(classify_deny(ToolOutcome(True, other, "unauthorized"), tool), "unstructured_error")

    def test_ping_classification(self) -> None:
        pong = {"tool": "splinterm.ping", "content_trust": "trusted_metadata", "data": {"protocol_version": "2025-11-25"}}
        self.assertEqual(classify_ping(ToolOutcome(False, pong, None)), "pong")
        self.assertEqual(classify_ping(ToolOutcome(True, {}, "internal")), "error_internal")
        self.assertEqual(classify_ping(ToolOutcome(False, {}, None)), "unexpected_success_shape")


class CatalogTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tools = fake_catalog()

    def test_reviewed_catalog_passes_and_additions_are_accepted(self) -> None:
        tools = self.tools + [{"name": "splinterm.future_tool", "inputSchema": {}, "outputSchema": {}}]
        evaluation = evaluate_catalog(tools, DEFAULT_SCHEMA_ROOT)
        self.assertTrue(evaluation.schemas_compared)
        self.assertEqual(evaluation.missing, [])
        self.assertEqual(evaluation.schema_mismatches, [])
        self.assertEqual(evaluation.contract_mismatches, [])
        self.assertIn("splinterm.future_tool", evaluation.additional)

    def test_missing_duplicate_and_incompatible_tools_fail(self) -> None:
        removed = [tool for tool in self.tools if tool["name"] != "splinterm.resize"]
        self.assertEqual(evaluate_catalog(removed, DEFAULT_SCHEMA_ROOT).missing, ["splinterm.resize"])
        duplicated = self.tools + [copy.deepcopy(self.tools[0])]
        self.assertEqual(evaluate_catalog(duplicated, DEFAULT_SCHEMA_ROOT).duplicates, [self.tools[0]["name"]])
        changed = copy.deepcopy(self.tools)
        target = next(tool for tool in changed if tool["name"] == "splinterm.input")
        target["inputSchema"]["required"] = ["text"]
        self.assertEqual(evaluate_catalog(changed, DEFAULT_SCHEMA_ROOT).schema_mismatches, ["splinterm.input"])
        annotated = copy.deepcopy(self.tools)
        target = next(tool for tool in annotated if tool["name"] == "splinterm.read_terminal")
        target["annotations"]["readOnlyHint"] = False
        self.assertEqual(evaluate_catalog(annotated, DEFAULT_SCHEMA_ROOT).contract_mismatches, ["splinterm.read_terminal"])

    def test_schema_root_absent_still_checks_structure(self) -> None:
        evaluation = evaluate_catalog(self.tools, None)
        self.assertFalse(evaluation.schemas_compared)
        self.assertEqual(evaluation.missing, [])

    def test_wire_schemas_are_localized_and_wrapped(self) -> None:
        for name in REQUIRED_TOOLS:
            stem = name.split(".", 1)[1]
            input_schema = wire_input_schema(DEFAULT_SCHEMA_ROOT, stem)
            output_schema = wire_output_schema(DEFAULT_SCHEMA_ROOT, stem)
            self.assertNotIn(COMMON_PREFIX, json.dumps(input_schema))
            self.assertNotIn(COMMON_PREFIX, json.dumps(output_schema))
            self.assertEqual(input_schema["type"], "object")
            self.assertEqual(len(output_schema["oneOf"]), 2)
            self.assertIn("$defs", output_schema)


if __name__ == "__main__":
    unittest.main()
