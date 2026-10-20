#!/usr/bin/env python3
"""Tests for the typed, allowlisted MCP demo evidence model."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import stat
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

from mcp_report import (  # noqa: E402
    BinaryIdentity,
    Check,
    CleanupStep,
    Evidence,
    Report,
    Transition,
    bounded_text,
    new_nonce,
    sanitize_build,
    write_private_json,
)


def identity(role: str = "splinterd") -> BinaryIdentity:
    return BinaryIdentity(role, "/opt/x/" + role, "/opt/x/" + role, 1, 2, 3, "0755", "a" * 64)


class ModelValidationTests(unittest.TestCase):
    def test_nonce_is_random_hex(self) -> None:
        first, second = new_nonce(), new_nonce()
        self.assertNotEqual(first, second)
        self.assertRegex(first, r"^[0-9a-f]{16}$")

    def test_check_rejects_free_form_names_and_codes(self) -> None:
        for name, code in (("Bad Name", "ok"), ("ok", "two words"), ("ok", ""), ("x" * 70, "ok")):
            with self.subTest(name=name, code=code):
                with self.assertRaises(ValueError):
                    Check(name, "passed", code)
        with self.assertRaises(ValueError):
            Check("ok", "maybe", "ok")

    def test_detail_is_bounded_printable_ascii(self) -> None:
        text = bounded_text("line\nwith\x1b[31mescape" + "y" * 500)
        self.assertNotIn("\n", text)
        self.assertNotIn("\x1b", text)
        self.assertEqual(len(text), 240)

    def test_binary_identity_validation(self) -> None:
        identity()
        with self.assertRaises(ValueError):
            BinaryIdentity("sh", "/bin/sh", "/bin/sh", 1, 2, 3, "0755", "a" * 64)
        with self.assertRaises(ValueError):
            BinaryIdentity("splinterd", "relative", "/x", 1, 2, 3, "0755", "a" * 64)
        with self.assertRaises(ValueError):
            BinaryIdentity("splinterd", "/x", "/x", 1, 2, 3, "0755", "nothex")

    def test_transition_validation(self) -> None:
        Transition("daemon_ready", "now", "ok", (("daemon", 12),))
        with self.assertRaises(ValueError):
            Transition("flying", "now", "ok")
        with self.assertRaises(ValueError):
            Transition("clean", "now", "ok", (("daemon", -1),))

    def test_result_must_be_known(self) -> None:
        report = Report(nonce=new_nonce(), mode="staged")
        report.result = "mostly_fine"
        with self.assertRaises(ValueError):
            report.to_json()

    def test_build_facts_are_allowlisted(self) -> None:
        build = sanitize_build({
            "pair_build_identity": "ab" * 32,
            "pair_build_version": "0.2.0",
            "environment": {"HOME": "/home/secret"},
            "argv": ["--token", "secret"],
            "source_dirty": True,
        })
        self.assertEqual(set(build), {"pair_build_identity", "pair_build_version", "source_dirty"})
        with self.assertRaises(ValueError):
            sanitize_build({"source_commit": "not hex"})
        with self.assertRaises(ValueError):
            sanitize_build({"mcp_server_version": "1.0 $(rm -rf)"})

    def test_report_serializes_no_arbitrary_mappings(self) -> None:
        report = Report(nonce=new_nonce(), mode="installed")
        report.build["policy_body"] = {"rules": []}
        report.add_check("daemon_ping", True, "pong")
        report.result = "passed"
        serialized = json.dumps(report.to_json())
        self.assertNotIn("policy_body", serialized)
        self.assertNotIn("rules", serialized)

    def test_result_precedence_inputs(self) -> None:
        report = Report(nonce=new_nonce(), mode="staged")
        self.assertFalse(report.cleanup_complete())
        report.cleanup = [CleanupStep("socket_absent", "passed"), CleanupStep("revoke_grant", "skipped")]
        self.assertTrue(report.cleanup_complete())
        report.cleanup.append(CleanupStep("remove_private_root", "failed"))
        self.assertFalse(report.cleanup_complete())


class EvidenceTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.base = Path(temporary.name)

    def test_in_memory_evidence_writes_nothing(self) -> None:
        report = Report(nonce=new_nonce(), mode="staged")
        evidence = Evidence(None, report)
        evidence.event({"kind": "spawn"})
        evidence.finalize()
        self.assertIsNone(evidence.directory)
        self.assertEqual(list(self.base.iterdir()), [])

    def test_layout_permissions_and_checksums(self) -> None:
        report = Report(nonce=new_nonce(), mode="staged")
        evidence = Evidence(self.base, report)
        assert evidence.directory is not None
        self.assertTrue(evidence.directory.name.startswith("mcp-demo-"))
        self.assertTrue(evidence.directory.name.endswith(report.nonce))
        evidence.event({"kind": "request", "method": "tools/call", "body": "SECRET", "params": {"x": 1}})
        report.identities.append(identity())
        report.add_check("daemon_ping", True, "pong")
        report.transition("daemon_ready", "ok", {"daemon": 42})
        report.cleanup = [CleanupStep("socket_absent", "passed")]
        report.result = "passed"
        evidence.finalize()
        directory = evidence.directory
        self.assertEqual(stat.S_IMODE(directory.stat().st_mode), 0o700)
        names = {path.name for path in directory.iterdir()}
        self.assertEqual(names, set(Evidence.FILES) | {"SHA256SUMS"})
        for path in directory.iterdir():
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600, path.name)
        for line in (directory / "SHA256SUMS").read_text().splitlines():
            digest, name = line.split("  ")
            self.assertEqual(hashlib.sha256((directory / name).read_bytes()).hexdigest(), digest)
        events = (directory / "mcp-events.jsonl").read_text()
        self.assertNotIn("SECRET", events)
        self.assertNotIn("params", events)
        manifest = json.loads((directory / "manifest.json").read_text())
        self.assertTrue(manifest["complete"])
        self.assertIn("hyprland-before.json", manifest["not_applicable"])
        state = json.loads((directory / "state.json").read_text())
        self.assertEqual(state["transitions"][0]["pids"], {"daemon": 42})

    def test_state_is_persisted_before_finalize(self) -> None:
        report = Report(nonce=new_nonce(), mode="staged")
        evidence = Evidence(self.base, report)
        self.addCleanup(evidence.close)
        assert evidence.directory is not None
        report.transition("daemon_ready")
        evidence.persist_state()
        state = json.loads((evidence.directory / "state.json").read_text())
        self.assertEqual(state["state"], "daemon_ready")
        self.assertFalse(json.loads((evidence.directory / "manifest.json").read_text())["complete"])

    def test_evidence_base_must_be_a_real_directory(self) -> None:
        target = self.base / "real"
        target.mkdir()
        link = self.base / "link"
        link.symlink_to(target)
        with self.assertRaises(ValueError):
            Evidence(link, Report(nonce=new_nonce(), mode="staged"))
        with self.assertRaises(ValueError):
            Evidence(self.base / "missing", Report(nonce=new_nonce(), mode="staged"))

    def test_atomic_private_write_leaves_no_temporaries(self) -> None:
        path = self.base / "value.json"
        write_private_json(path, {"a": 1})
        write_private_json(path, {"a": 2})
        self.assertEqual(json.loads(path.read_text()), {"a": 2})
        self.assertEqual([entry.name for entry in self.base.iterdir()], ["value.json"])
        self.assertEqual(stat.S_IMODE(os.stat(path).st_mode), 0o600)


if __name__ == "__main__":
    unittest.main()
