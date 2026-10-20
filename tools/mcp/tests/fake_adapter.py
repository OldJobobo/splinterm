#!/usr/bin/env python3
"""Deterministic stand-in for `splinterm-mcp`, selected by `--mode`.

Host-driver modes exercise framing, ordering, deadlines, EOF, and cleanup.
The `server*` modes emulate enough of the real adapter's public stdio
contract (lifecycle, discovery, ping, deny-all) for preflight subprocess tests.
Nothing here is a product contract; the real adapter remains the authority.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import signal
import socket
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

MAX_INBOUND = 256 * 1024
SECRET_TEXT = "FAKE-TERMINAL-SECRET-0031"


def write(document: object) -> None:
    os.write(1, (json.dumps(document, separators=(",", ":")) + "\n").encode())


def lines():
    """Yield inbound frames like the real transport, exiting 1 on violations."""
    buffer = bytearray()
    while True:
        chunk = os.read(0, 65536)
        if not chunk:
            if buffer:
                sys.exit(1)  # unterminated trailing frame
            return
        buffer.extend(chunk)
        while True:
            index = buffer.find(b"\n")
            if index < 0:
                if len(buffer) >= MAX_INBOUND:
                    sys.exit(1)
                break
            line = bytes(buffer[:index])
            del buffer[: index + 1]
            if len(line) + 1 > MAX_INBOUND:
                sys.exit(1)
            yield line


def parse(line: bytes) -> dict:
    try:
        message = json.loads(line)
    except ValueError:
        sys.exit(1)
    if not isinstance(message, dict) or message.get("jsonrpc") != "2.0":
        sys.exit(1)
    return message


def respond(request: dict, result: object) -> None:
    write({"jsonrpc": "2.0", "id": request["id"], "result": result})


def respond_error(request: dict, code: int, message: str) -> None:
    write({"jsonrpc": "2.0", "id": request["id"], "error": {"code": code, "message": message}})


# ------------------------------------------------------------- server modes


def catalog(missing: str | None) -> list[dict]:
    from mcp_preflight import READ_ONLY_REQUIRED, REQUIRED_TOOLS, DEFAULT_SCHEMA_ROOT, wire_input_schema, wire_output_schema

    tools = []
    for name in REQUIRED_TOOLS + ("splinterm.report_agent_status",):
        if name == missing:
            continue
        stem = name.split(".", 1)[1]
        required = name in REQUIRED_TOOLS
        tools.append({
            "name": name,
            "inputSchema": wire_input_schema(DEFAULT_SCHEMA_ROOT, stem) if required else {"type": "object"},
            "outputSchema": wire_output_schema(DEFAULT_SCHEMA_ROOT, stem) if required else {"type": "object"},
            "execution": {"taskSupport": "forbidden"},
            "annotations": {
                "openWorldHint": False,
                "readOnlyHint": name in READ_ONLY_REQUIRED,
                "destructiveHint": name == "splinterm.revoke_access",
            },
        })
    return tools


def tool_text(structured: dict, is_error: bool) -> dict:
    return {
        "content": [{"type": "text", "text": json.dumps(structured)}],
        "structuredContent": structured,
        "isError": is_error,
    }


def error_envelope(tool: str, code: str) -> dict:
    return {
        "schema": "splinterm.mcp.v2",
        "tool": tool,
        "ok": False,
        "error": {"code": code, "message": "fixed fake message", "retryable": False},
        "truncated": False,
        "content_trust": "trusted_metadata",
    }


def daemon_ping() -> bool:
    path = os.environ.get("SPLINTERM_SOCKET")
    if not path:
        return False
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
            stream.settimeout(5)
            stream.connect(path)
            stream.sendall(b"ping\n")
            return stream.recv(16) == b"pong\n"
    except OSError:
        return False


def server(mode: str) -> None:
    accepted = False
    initialized = False
    for line in lines():
        message = parse(line)
        method = message.get("method")
        if "id" not in message:
            if method == "notifications/initialized" and accepted:
                initialized = True
            continue
        if method == "initialize":
            if accepted:
                respond_error(message, -32600, "initialize has already been accepted")
                continue
            params = message.get("params") or {}
            if not isinstance(params.get("capabilities"), dict):
                sys.exit(1)
            version = params.get("protocolVersion")
            if version not in ("2025-06-18", "2025-11-25"):
                respond_error(message, -32600, "unsupported version")
                sys.exit(1)
            accepted = True
            respond(message, {
                "protocolVersion": version,
                "capabilities": {"resources": {"subscribe": True}, "tools": {}},
                "serverInfo": {
                    "name": "splinterm-mcp",
                    "version": "0.2.0 (dev)" if mode == "server-badversion" else "0.2.0-dev",
                },
            })
            continue
        if not initialized:
            respond_error(message, -32600, "notifications/initialized is required")
            continue
        if method == "tools/list":
            respond(message, {"tools": catalog("splinterm.revoke_access" if mode == "server-missing-tool" else None)})
        elif method == "resources/list":
            respond(message, {"resources": [{"uri": "splinterm://topology", "name": "topology"}]})
        elif method == "ping":
            respond(message, {})
        elif method == "tools/call":
            name = (message.get("params") or {}).get("name")
            if name == "splinterm.ping":
                if mode == "server-slow":
                    time.sleep(3)
                if daemon_ping():
                    respond(message, tool_text({
                        "schema": "splinterm.mcp.v2", "tool": name, "ok": True,
                        "resource": {"kind": "daemon"}, "data": {"protocol_version": "2025-11-25"},
                        "truncated": False, "content_trust": "trusted_metadata",
                    }, False))
                else:
                    respond(message, tool_text(error_envelope(name, "internal"), True))
            elif name == "splinterm.inspect_topology":
                if mode == "server-leak":
                    respond(message, tool_text({"ok": True, "tool": name, "data": {"text": SECRET_TEXT}}, False))
                else:
                    code = {
                        "server-deny-internal": "internal",
                        "server-deny-oddcode": "Daemon.Unavailable",
                    }.get(mode, "unauthorized")
                    respond(message, tool_text(error_envelope(name, code), True))
            else:
                respond_error(message, -32602, "unknown tool")
        else:
            respond_error(message, -32601, "method not found")
    sys.exit(0)


# ------------------------------------------------------- host-driver modes


def host_mode(mode: str) -> None:
    if mode == "exit-now":
        sys.exit(3)
    if mode == "no-read":
        time.sleep(60)
        return
    if mode == "stderr-flood":
        os.write(2, b"e" * (1024 * 1024))
    if mode == "ignore-term":
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
    if mode == "trailing":
        os.write(1, b'{"jsonrpc":')
        sys.exit(0)
    if mode == "chatty-silent":
        for line in lines():
            for _ in range(200):
                write({"jsonrpc": "2.0", "method": "notifications/message", "params": {}})
                time.sleep(0.01)
        return
    held: list[dict] = []
    for line in lines():
        message = json.loads(line)
        if "id" not in message:
            continue
        if mode in ("echo", "stderr-flood", "ignore-eof", "ignore-term"):
            respond(message, {"echo": message.get("params"), "secret": SECRET_TEXT})
        elif mode == "initialize":
            respond(message, {"protocolVersion": message["params"]["protocolVersion"]})
        elif mode == "wrong-version":
            respond(message, {"protocolVersion": "2025-06-18"})
        elif mode == "reorder":
            held.append(message)
            if len(held) == 2:
                respond(held[1], {"order": 2})
                respond(held[0], {"order": 1})
                held.clear()
        elif mode == "notify":
            write({"jsonrpc": "2.0", "method": "notifications/resources/updated", "params": {"uri": "splinterm://topology"}})
            respond(message, {})
        elif mode == "notify-many":
            for index in range(5):
                write({"jsonrpc": "2.0", "method": "notifications/message", "params": {"n": index}})
            respond(message, {})
        elif mode == "silent":
            continue
        elif mode == "late":
            time.sleep(0.5)
            respond(message, {"late": True})
        elif mode == "garbage":
            os.write(1, b"this is not json\n")
        elif mode == "badutf8":
            os.write(1, b'{"jsonrpc":"2.0","id":1,"result":"\xff"}\n')
        elif mode == "envelope":
            write({"jsonrpc": "1.0", "id": message["id"], "result": {}})
        elif mode == "both":
            write({"jsonrpc": "2.0", "id": message["id"], "result": {}, "error": {"code": 1, "message": "x"}})
        elif mode == "unknown-id":
            write({"jsonrpc": "2.0", "id": 999, "result": {}})
        elif mode == "duplicate":
            respond(message, {})
            respond(message, {})
        elif mode == "peer-request":
            write({"jsonrpc": "2.0", "id": "srv-1", "method": "sampling/createMessage", "params": {}})
        elif mode == "big":
            write({"jsonrpc": "2.0", "id": message["id"], "result": {"pad": "x" * 4096}})
        elif mode == "big-partial":
            os.write(1, b"x" * 8192)
        elif mode == "exit-on-request":
            sys.exit(4)
        elif mode == "dupkey":
            os.write(1, ('{"jsonrpc":"2.0","id":%d,"result":{},"result":{}}\n' % message["id"]).encode())
    if mode in ("ignore-eof", "ignore-term"):
        while True:
            time.sleep(1)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", required=True)
    parser.add_argument("--role", default="mcp")
    arguments = parser.parse_args()
    if arguments.mode.startswith("server"):
        server(arguments.mode)
    else:
        host_mode(arguments.mode)


if __name__ == "__main__":
    main()
