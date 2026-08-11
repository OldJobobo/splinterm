#!/usr/bin/env python3
"""Exercise the extracted splinterm-mcp package against an isolated real daemon."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import select
import signal
import subprocess
import sys
import tempfile
import time


class McpHost:
    def __init__(self, executable: Path, environment: dict[str, str]):
        self.process = subprocess.Popen(
            [str(executable)],
            env=environment,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        assert self.process.stdin is not None and self.process.stdout is not None
        self.next_id = 1
        self.notifications: list[dict[str, object]] = []

    def send(self, document: dict[str, object]) -> None:
        assert self.process.stdin is not None
        self.process.stdin.write(json.dumps(document, separators=(",", ":")) + "\n")
        self.process.stdin.flush()

    def receive(self, timeout: float = 15) -> dict[str, object]:
        assert self.process.stdout is not None
        ready, _, _ = select.select([self.process.stdout], [], [], timeout)
        assert ready, "timed out reading MCP response"
        line = self.process.stdout.readline()
        assert line, "MCP server closed stdout"
        return json.loads(line)

    def request(self, method: str, params: dict[str, object] | None = None) -> dict[str, object]:
        request_id = self.next_id
        self.next_id += 1
        document: dict[str, object] = {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
        }
        if params is not None:
            document["params"] = params
        self.send(document)
        while True:
            response = self.receive()
            if response.get("id") == request_id:
                return response
            self.notifications.append(response)

    def initialize(self) -> dict[str, object]:
        response = self.request("initialize", {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "splinterm-package-validator", "version": "1"},
        })
        assert "error" not in response, response
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        return response["result"]

    def tool(self, name: str, arguments: dict[str, object]) -> dict[str, object]:
        response = self.request("tools/call", {"name": name, "arguments": arguments})
        assert "error" not in response, response
        return response["result"]

    def close(self) -> None:
        if self.process.stdin is not None and not self.process.stdin.closed:
            self.process.stdin.close()
        self.process.wait(timeout=15)
        assert self.process.returncode == 0
        stderr = self.process.stderr.read() if self.process.stderr else ""
        assert not stderr, stderr


def write_policy(path: Path, executable: Path, scopes: list[str], resources: list[dict[str, object]]) -> None:
    path.write_text(json.dumps({
        "schema": "splinterm.policy.v2",
        "rules": [{
            "id": "package-mcp",
            "executable": {
                "path": str(executable.resolve()),
                "sha256": hashlib.sha256(executable.read_bytes()).hexdigest(),
            },
            "scopes": scopes,
            "resources": resources,
            "limits": {
                "max_returned_rows": 64,
                "max_results": 64,
                "max_returned_bytes": 1048576,
                "max_live_subscriptions": 4,
                "max_spawn_count": 4,
                "deadline_ms": 10000,
            },
        }],
    }), encoding="utf-8")
    path.chmod(0o600)


def main() -> int:
    if len(sys.argv) != 2:
        raise SystemExit("usage: validate-mcp-package.py EXTRACTED_ROOT")
    root = Path(sys.argv[1]).resolve()
    daemon = root / "usr/bin/splinterd"
    client = root / "usr/bin/splinterm"
    mcp = root / "usr/bin/splinterm-mcp"
    with tempfile.TemporaryDirectory(prefix="splinterm-mcp-package-") as directory:
        runtime = Path(directory)
        socket = runtime / "splinterd.sock"
        policy = runtime / "policy.json"
        environment = os.environ.copy()
        for name in ("DISPLAY", "WAYLAND_DISPLAY", "SPLINTERM_ENABLE_DEV_ATTACH"):
            environment.pop(name, None)
        environment.update(
            HOME=str(runtime),
            XDG_CONFIG_HOME=str(runtime / "config"),
            XDG_RUNTIME_DIR=str(runtime),
            XDG_STATE_HOME=str(runtime / "state"),
            SPLINTERM_SOCKET=str(socket),
            SPLINTERM_POLICY=str(policy),
            SPLINTERM_MCP_TIMEOUT_MS="10000",
        )

        # Start deny-all. Transport/discovery remains available, daemon-backed
        # operations fail closed until the exact packaged identity is published.
        policy.write_text('{"schema":"splinterm.policy.v2","rules":[]}', encoding="utf-8")
        policy.chmod(0o600)
        daemon_process = subprocess.Popen(
            [str(daemon)], env=environment, stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True,
        )
        try:
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline and not socket.exists():
                assert daemon_process.poll() is None, "MCP package daemon exited"
                time.sleep(0.02)
            assert socket.exists(), "MCP package daemon did not create socket"

            host = McpHost(mcp, environment)
            info = host.initialize()
            assert info["protocolVersion"] == "2025-11-25"
            assert info["capabilities"] == {"resources": {"subscribe": True}, "tools": {}}
            tools = host.request("tools/list")["result"]["tools"]
            assert len(tools) == 33 and len({item["name"] for item in tools}) == 33
            resources = host.request("resources/list")["result"]["resources"]
            templates = host.request("resources/templates/list")["result"]["resourceTemplates"]
            assert [item["uri"] for item in resources] == ["splinterm://topology"]
            assert len(templates) == 2
            denied = host.tool("splinterm.inspect_topology", {})
            assert denied["isError"] is True
            assert denied["structuredContent"]["error"]["code"] == "unauthorized"
            host.close()

            lifecycle_scopes = ["topology_metadata_read", "process_spawn", "topology_layout_mutate"]
            write_policy(policy, client, lifecycle_scopes, [{"kind": "daemon"}])
            daemon_process.send_signal(signal.SIGHUP)
            time.sleep(0.2)
            created = subprocess.run(
                [
                    str(client), "--output", "json", "--schema-major", "2",
                    "--timeout-ms", "10000", "new", "package-mcp", "--",
                    "/bin/sh", "-c", "printf 'package-mcp-ready\\n'; exec sleep 30",
                ],
                cwd=runtime,
                env=environment,
                capture_output=True,
                text=True,
                timeout=15,
            )
            assert created.returncode == 0, (created.stdout, created.stderr)
            body = json.loads(created.stdout)
            lair_id = body["resource"]["lair_id"]
            dojo_id = body["resource"]["dojo_id"]
            splint_id = body["resource"]["splint_id"]
            incarnation = body["resource"]["incarnation"]

            full_scopes = [
                "topology_metadata_read", "topology_subscribe",
                "terminal_visible_read", "terminal_subscribe", "scrollback_read",
                "controller_acquire", "controller_transfer", "input", "resize",
                "process_spawn", "process_restore", "process_terminate",
                "topology_layout_mutate", "topology_name_mutate",
            ]
            resources_exact = [
                {"kind": "daemon"},
                {"kind": "lair", "lair_id": lair_id},
                {"kind": "dojo", "dojo_id": dojo_id},
                {"kind": "splint", "splint_id": splint_id, "incarnation": incarnation},
            ]
            write_policy(policy, mcp, full_scopes, resources_exact)
            daemon_process.send_signal(signal.SIGHUP)
            time.sleep(0.2)
            host = McpHost(mcp, environment)
            host.initialize()
            topology = host.tool("splinterm.inspect_topology", {})
            assert topology["isError"] is False
            terminal = host.tool("splinterm.read_terminal", {"splint_id": splint_id})
            assert terminal["isError"] is False
            assert terminal["structuredContent"]["content_trust"] == "untrusted_terminal_data"

            control = host.tool("splinterm.acquire_control", {
                "splint_id": splint_id, "incarnation": incarnation,
                "modes": ["input", "resize"],
            })
            assert control["isError"] is False, control
            handle = control["structuredContent"]["data"]["controller_handle"]
            sent = host.tool("splinterm.input", {
                "splint_id": splint_id, "incarnation": incarnation,
                "text": "printf package-mcp-input\\n", "controller_handle": handle,
            })
            assert sent["isError"] is False
            released = host.tool("splinterm.release_control", {"controller_handle": handle})
            assert released["isError"] is False

            uri = "splinterm://topology"
            subscribed = host.request("resources/subscribe", {"uri": uri})
            assert "error" not in subscribed
            renamed = host.tool("splinterm.rename_splint", {
                "splint_id": splint_id, "title": "package-mcp-renamed",
            })
            assert renamed["isError"] is False
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                note = host.receive(deadline - time.monotonic())
                if note.get("method") == "notifications/resources/updated":
                    assert note["params"]["uri"] == uri
                    break
            else:
                raise AssertionError("missing MCP resource update")
            unsubscribed = host.request("resources/unsubscribe", {"uri": uri})
            assert "error" not in unsubscribed

            # Narrow policy and reload: the same explicit target remains readable,
            # while controller acquisition is denied without controller scopes.
            write_policy(policy, mcp, ["terminal_visible_read", "terminal_subscribe"], [
                {"kind": "splint", "splint_id": splint_id, "incarnation": incarnation},
            ])
            daemon_process.send_signal(signal.SIGHUP)
            time.sleep(0.2)
            denied_control = host.tool("splinterm.acquire_control", {
                "splint_id": splint_id, "incarnation": incarnation, "modes": ["input"],
            })
            assert denied_control["isError"] is True
            assert denied_control["structuredContent"]["error"]["code"] == "unauthorized"
            host.close()
        finally:
            if daemon_process.poll() is None:
                daemon_process.send_signal(signal.SIGINT)
                try:
                    daemon_process.wait(timeout=90)
                except subprocess.TimeoutExpired:
                    daemon_process.kill()
                    daemon_process.wait(timeout=5)
        assert daemon_process.returncode == 0
        assert not socket.exists()
        stderr = daemon_process.stderr.read() if daemon_process.stderr else ""
        assert "package-mcp-input" not in stderr
    print("Extracted MCP package runtime validation passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
