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
