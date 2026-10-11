#!/usr/bin/env python3
"""Regression tests for the extracted MCP package validator host."""

from __future__ import annotations

import importlib.util
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock


MODULE_PATH = Path(__file__).with_name("validate-mcp-package.py")
SPEC = importlib.util.spec_from_file_location("validate_mcp_package", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
validate_mcp_package = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(validate_mcp_package)


class McpHostTests(unittest.TestCase):
    def fake_server(self, payload: bytes) -> Path:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        script = Path(temporary.name) / "fake-mcp-server"
        script.write_text(
            f"#!{sys.executable}\n"
            "import os\n"
            "import sys\n"
            "sys.stdin.buffer.readline()\n"
            f"os.write(1, {payload!r})\n"
            "for _ in sys.stdin.buffer:\n"
            "    pass\n",
            encoding="utf-8",
        )
        script.chmod(0o755)
        return script

    def test_receive_preserves_a_coalesced_second_frame(self) -> None:
        payload = (
            b'{"jsonrpc":"2.0","id":1,"result":{}}\n'
            b'{"jsonrpc":"2.0","method":"notifications/resources/updated",'
            b'"params":{"uri":"splinterm://topology"}}\n'
        )
        host = validate_mcp_package.McpHost(self.fake_server(payload), os.environ.copy())
        try:
            host.send({"jsonrpc": "2.0", "id": 1, "method": "initialize"})
            self.assertEqual(host.receive(1)["id"], 1)
            self.assertEqual(
                host.receive(1)["method"],
                "notifications/resources/updated",
            )
        finally:
            host.close()

    def test_resource_update_survives_both_response_orders(self) -> None:
        update = (
            b'{"jsonrpc":"2.0","method":"notifications/resources/updated",'
            b'"params":{"uri":"splinterm://topology"}}\n'
        )
        response = b'{"jsonrpc":"2.0","id":1,"result":{}}\n'
        for payload in (update + response, response + update):
            with self.subTest(payload=payload):
                host = validate_mcp_package.McpHost(
                    self.fake_server(payload), os.environ.copy()
                )
                try:
                    host.request("tools/call", {})
                    self.assertEqual(
                        host.wait_for_resource_update("splinterm://topology", 1)["params"],
                        {"uri": "splinterm://topology"},
                    )
                    self.assertEqual(host.notifications, [])
                finally:
                    host.close()

    def test_queued_matching_update_does_not_read_and_preserves_other_notifications(self) -> None:
        host = validate_mcp_package.McpHost(self.fake_server(b""), os.environ.copy())
        unrelated = {"method": "notifications/resources/updated", "params": {"uri": "other"}}
        matching = {"method": "notifications/resources/updated", "params": {"uri": "wanted"}}
        malformed = {"method": "notifications/resources/updated", "params": None}
        host.notifications = [unrelated, matching, malformed]
        try:
            with mock.patch.object(host, "receive", side_effect=AssertionError("unexpected read")):
                self.assertIs(host.wait_for_resource_update("wanted"), matching)
                self.assertEqual(host.notifications, [unrelated, malformed])
        finally:
            host.close()

    def test_unrelated_updates_do_not_extend_deadline(self) -> None:
        host = validate_mcp_package.McpHost(self.fake_server(b""), os.environ.copy())
        unrelated = {"method": "notifications/resources/updated", "params": {"uri": "other"}}
        try:
            with (
                mock.patch.object(validate_mcp_package.time, "monotonic", side_effect=[0, 1, 2, 3]),
                mock.patch.object(host, "receive", return_value=unrelated) as receive,
            ):
                with self.assertRaisesRegex(AssertionError, "missing MCP resource update"):
                    host.wait_for_resource_update("wanted", 3)
                self.assertEqual(receive.call_args_list, [mock.call(2), mock.call(1)])
                self.assertEqual(host.notifications, [unrelated, unrelated])
        finally:
            host.close()

    def test_missing_update_still_fails(self) -> None:
        host = validate_mcp_package.McpHost(self.fake_server(b""), os.environ.copy())
        try:
            with mock.patch.object(host, "receive", side_effect=AssertionError("timed out")):
                with self.assertRaisesRegex(AssertionError, "timed out"):
                    host.wait_for_resource_update("wanted", 1)
        finally:
            host.close()

    def test_receive_rejects_an_oversized_buffered_line(self) -> None:
        host = validate_mcp_package.McpHost(self.fake_server(b""), os.environ.copy())
        try:
            with mock.patch.object(
                validate_mcp_package,
                "MAXIMUM_MCP_RESPONSE_BYTES",
                32,
            ):
                host.buffer.extend(b"x" * 32 + b"\n")
                with self.assertRaisesRegex(AssertionError, "line limit"):
                    host.receive(1)
        finally:
            host.close()


if __name__ == "__main__":
    unittest.main()
