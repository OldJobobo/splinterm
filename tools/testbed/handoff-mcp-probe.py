#!/usr/bin/env python3
"""MCP controller expiry probe for live-upgrade VM acceptance (Plan 0061 M3.17).

Runs inside the guest, next to a packaged `splinterd` started by
`omarchy-vm.sh package-launch`. It holds one `splinterm-mcp` stdio session
across a live upgrade and checks the operator decision of 2026-10-19 (M3.12):
controller handles expire with their daemon connection, and the client
acquires control again under the same policy.

  ids   Print the Lair, Splint, and incarnation of the named Lair's first
        Splint from the daemon's durable topology file (JSON).
  run   Acquire control and send one line, write `acquired.json`, wait for
        the `proceed` file (created after the upgrade), then use the old
        handle (expected: `controller_unavailable`), acquire again, send one
        line with the new handle, release it, and write `result.json`.

It changes nothing outside the given state directory and the probe Splint.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import time
from typing import Any, Iterator

MCP_PROTOCOL = "2025-11-25"


def splints(node: Any) -> Iterator[dict[str, Any]]:
    """Every Splint object in a Lair's Dojo layouts, in document order."""
    if isinstance(node, dict):
        if isinstance(node.get("id"), str) and "last_incarnation" in node and "state" in node:
            yield node
        for value in node.values():
            yield from splints(value)
    elif isinstance(node, list):
        for value in node:
            yield from splints(value)


def probe_ids(topology: Path, lair_name: str) -> dict[str, object]:
    document = json.loads(topology.read_text(encoding="utf-8"))
    lairs = [lair for lair in document.get("lairs", []) if lair.get("name") == lair_name]
    if len(lairs) != 1:
        raise ValueError(f"expected exactly one Lair named {lair_name!r}, found {len(lairs)}")
    lair = lairs[0]
    for splint in splints(lair.get("dojos", [])):
        incarnation = splint.get("last_incarnation")
        if isinstance(incarnation, int) and incarnation > 0:
            return {
                "lair_id": lair["id"],
                "splint_id": splint["id"],
                "incarnation": incarnation,
            }
    raise ValueError(f"Lair {lair_name!r} has no launched Splint")


class Session:
    """One newline-delimited JSON-RPC session with a `splinterm-mcp` child."""

    def __init__(self, command: list[str]) -> None:
        self.child = subprocess.Popen(
            command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        self.next_id = 1

    def request(self, method: str, params: dict[str, object]) -> dict[str, Any]:
        assert self.child.stdin is not None and self.child.stdout is not None
        request_id = self.next_id
        self.next_id += 1
        message = {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}
        self.child.stdin.write(json.dumps(message) + "\n")
        self.child.stdin.flush()
        while True:
            line = self.child.stdout.readline()
            if not line:
                raise RuntimeError(f"splinterm-mcp closed its output during {method}")
            response = json.loads(line)
            if response.get("id") == request_id:
                return response

    def notify(self, method: str) -> None:
        assert self.child.stdin is not None
        self.child.stdin.write(json.dumps({"jsonrpc": "2.0", "method": method}) + "\n")
        self.child.stdin.flush()

    def call(self, name: str, arguments: dict[str, object]) -> dict[str, Any]:
        return self.request("tools/call", {"name": name, "arguments": arguments})

    def close(self) -> None:
        if self.child.stdin is not None:
            self.child.stdin.close()
        try:
            self.child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.child.kill()
            self.child.wait()
        if self.child.stdout is not None:
            self.child.stdout.close()


def structured(response: dict[str, Any]) -> dict[str, Any]:
    """The tool's structured result, from `structuredContent` or its text."""
    result = response.get("result")
    if not isinstance(result, dict):
        return {}
    content = result.get("structuredContent")
    if isinstance(content, dict):
        return content
    for item in result.get("content", []):
        if isinstance(item, dict) and isinstance(item.get("text"), str):
            try:
                value = json.loads(item["text"])
            except ValueError:
                continue
            if isinstance(value, dict):
                return value
    return {}


def handle_of(response: dict[str, Any]) -> str | None:
    data = structured(response).get("data")
    handle = data.get("controller_handle") if isinstance(data, dict) else None
    return handle if isinstance(handle, str) else None


def failed(response: dict[str, Any]) -> bool:
    result = response.get("result")
    return "error" in response or (isinstance(result, dict) and result.get("isError") is True)


def write_json(path: Path, value: object) -> None:
    partial = path.with_name(path.name + ".partial")
    partial.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    os.replace(partial, path)


def run_probe(
    mcp: list[str], ids: dict[str, Any], state: Path, wait_seconds: float
) -> dict[str, object]:
    state.mkdir(parents=True, exist_ok=True)
    (state / "probe.pid").write_text(f"{os.getpid()}\n", encoding="utf-8")
    target = {"splint_id": ids["splint_id"], "incarnation": ids["incarnation"]}
    responses: list[dict[str, Any]] = []
    session = Session(mcp)
    try:
        responses.append(
            session.request(
                "initialize",
                {
                    "protocolVersion": MCP_PROTOCOL,
                    "capabilities": {},
                    "clientInfo": {"name": "splinterm-handoff-vm-probe", "version": "1"},
                },
            )
        )
        session.notify("notifications/initialized")
        acquired = session.call(
            "splinterm.acquire_control", {**target, "modes": ["input", "resize"]}
        )
        responses.append(acquired)
        old_handle = handle_of(acquired)
        before = None
        if old_handle is not None:
            before = session.call(
                "splinterm.input",
                {**target, "text": "mcp-before-upgrade\n", "controller_handle": old_handle},
            )
            responses.append(before)
        write_json(state / "acquired.json", {"handle": old_handle is not None})
        deadline = time.monotonic() + wait_seconds
        while not (state / "proceed").exists():
            if time.monotonic() > deadline:
                raise RuntimeError("the proceed file never appeared")
            time.sleep(0.2)
        stale = None
        if old_handle is not None:
            stale = session.call(
                "splinterm.input",
                {**target, "text": "mcp-stale-handle\n", "controller_handle": old_handle},
            )
            responses.append(stale)
        again = session.call("splinterm.acquire_control", {**target, "modes": ["input", "resize"]})
        responses.append(again)
        new_handle = handle_of(again)
        after = None
        if new_handle is not None:
            after = session.call(
                "splinterm.input",
                {**target, "text": "mcp-after-upgrade\n", "controller_handle": new_handle},
            )
            responses.append(after)
            responses.append(
                session.call("splinterm.release_control", {"controller_handle": new_handle})
            )
    finally:
        session.close()
    result = {
        "acquired_before": old_handle is not None and before is not None and not failed(before),
        "old_handle_expired": stale is not None
        and failed(stale)
        and "controller_unavailable" in json.dumps(stale),
        "reacquired": new_handle is not None
        and new_handle != old_handle
        and after is not None
        and not failed(after),
        "responses": responses,
    }
    write_json(state / "result.json", result)
    return result


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = result.add_subparsers(dest="command", required=True)
    ids = commands.add_parser("ids")
    ids.add_argument("--topology", required=True, type=Path)
    ids.add_argument("--lair", default="mcp-probe")
    run = commands.add_parser("run")
    run.add_argument("--ids", required=True, type=Path)
    run.add_argument("--state-dir", required=True, type=Path)
    run.add_argument("--mcp", default="/usr/bin/splinterm-mcp")
    run.add_argument("--wait-seconds", type=float, default=1800)
    return result


def main() -> int:
    arguments = parser().parse_args()
    if arguments.command == "ids":
        print(json.dumps(probe_ids(arguments.topology, arguments.lair), sort_keys=True))
        return 0
    ids = json.loads(arguments.ids.read_text(encoding="utf-8"))
    result = run_probe([arguments.mcp], ids, arguments.state_dir, arguments.wait_seconds)
    passed = all(result[key] for key in ("acquired_before", "old_handle_expired", "reacquired"))
    print(json.dumps({key: value for key, value in result.items() if key != "responses"}))
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
