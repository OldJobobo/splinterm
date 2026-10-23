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
  agent Act as one agent for the easy-agent-access VM sequence (Plan 0053
        PR 5): hold one `splinterm-mcp` session and run the numbered command
        files `cmd-1.json`, `cmd-2.json`, ... that appear in the state
        directory, in order, writing `out-N.json` for each. Publish each
        command by writing a temporary file and renaming it. Operations:
        `request` (a level or scopes for a Lair; `until` is `once` or
        `answered`), `cancel`, `read` (read_terminal, to show whether access
        still holds), `acquire` and `release` (control), and `exit`.

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


def outcome(response: dict[str, Any]) -> dict[str, object]:
    """A short summary of one tool response: its error code or data status."""
    content = structured(response)
    error = content.get("error")
    if failed(response) or content.get("ok") is False:
        code = error.get("code") if isinstance(error, dict) else None
        return {"ok": False, "code": code if isinstance(code, str) else "unknown"}
    data = content.get("data")
    summary: dict[str, object] = {"ok": True}
    if isinstance(data, dict):
        for key in ("status", "pending_request_id", "source", "grant_id", "expires_at"):
            if key in data:
                summary[key] = data[key]
    return summary


class Agent:
    """One agent: an MCP session plus the controller handle it holds."""

    def __init__(self, session: Session, poll_seconds: float) -> None:
        self.session = session
        self.poll_seconds = poll_seconds
        self.handle: str | None = None

    def run(self, command: dict[str, Any]) -> dict[str, object]:
        operation = command.get("op")
        if operation == "request":
            return self.request(command)
        if operation == "cancel":
            response = self.session.call(
                "splinterm.request_lair_access",
                {
                    "lair_id": command["lair_id"],
                    "cancel_request_id": command["cancel_request_id"],
                },
            )
            return {"op": "cancel", **outcome(response)}
        if operation == "read":
            response = self.session.call(
                "splinterm.read_terminal", {"splint_id": command["splint_id"]}
            )
            return {"op": "read", **outcome(response)}
        if operation == "acquire":
            arguments: dict[str, object] = {
                "splint_id": command["splint_id"],
                "incarnation": command["incarnation"],
                "modes": command.get("modes", ["input", "resize"]),
            }
            if command.get("takeover"):
                arguments["takeover"] = True
            response = self.session.call("splinterm.acquire_control", arguments)
            self.handle = handle_of(response)
            return {"op": "acquire", "handle": self.handle is not None, **outcome(response)}
        if operation == "release":
            if self.handle is None:
                return {"op": "release", "ok": False, "code": "no_handle"}
            response = self.session.call(
                "splinterm.release_control", {"controller_handle": self.handle}
            )
            self.handle = None
            return {"op": "release", **outcome(response)}
        raise ValueError(f"unknown operation {operation!r}")

    def request(self, command: dict[str, Any]) -> dict[str, object]:
        arguments: dict[str, object] = {"lair_id": command["lair_id"]}
        if "level" in command:
            arguments["level"] = command["level"]
        else:
            arguments["scopes"] = command["scopes"]
        until = command.get("until", "answered")
        if until not in ("once", "answered"):
            raise ValueError(f"unknown until {until!r}")
        deadline = time.monotonic() + float(command.get("deadline_seconds", 150))
        calls = 0
        while True:
            calls += 1
            summary = outcome(self.session.call("splinterm.request_lair_access", arguments))
            # Calling again with the same Lair and access joins the pending
            # request and collects the answer; it never opens a second prompt.
            if until == "once" or summary.get("status") != "pending":
                break
            if time.monotonic() > deadline:
                summary["deadline_passed"] = True
                break
            time.sleep(self.poll_seconds)
        return {"op": "request", "calls": calls, **summary}


COMMAND_SETTLE_SECONDS = 5.0


def read_command(path: Path, settle_seconds: float) -> dict[str, Any] | None:
    """The command object in `path`, or `None` if it never becomes one.

    Publish a command by writing a temporary sibling and renaming it over
    `cmd-N.json`. A file written in place may be seen half written, so an
    empty or incomplete file is read again for up to `settle_seconds`
    before it counts as bad; the session is never ended by it.
    """
    deadline = time.monotonic() + settle_seconds
    while True:
        try:
            value = json.loads(path.read_text(encoding="utf-8"))
        except ValueError:
            value = None
        if isinstance(value, dict):
            return value
        if time.monotonic() > deadline:
            return None
        time.sleep(0.05)


def run_agent(
    mcp: list[str],
    state: Path,
    client_name: str,
    idle_seconds: float,
    poll_seconds: float,
) -> int:
    state.mkdir(parents=True, exist_ok=True)
    (state / "agent.pid").write_text(f"{os.getpid()}\n", encoding="utf-8")
    session = Session(mcp)
    try:
        session.request(
            "initialize",
            {
                "protocolVersion": MCP_PROTOCOL,
                "capabilities": {},
                "clientInfo": {"name": client_name, "version": "1"},
            },
        )
        session.notify("notifications/initialized")
        agent = Agent(session, poll_seconds)
        number = 1
        idle_until = time.monotonic() + idle_seconds
        while True:
            path = state / f"cmd-{number}.json"
            if not path.exists():
                if time.monotonic() > idle_until:
                    raise RuntimeError(f"no {path.name} within {idle_seconds} seconds")
                time.sleep(0.1)
                continue
            command = read_command(path, COMMAND_SETTLE_SECONDS)
            if command is None:
                write_json(
                    state / f"out-{number}.json",
                    {"op": None, "ok": False, "code": "bad_command", "detail": "not a JSON object"},
                )
                number += 1
                idle_until = time.monotonic() + idle_seconds
                continue
            if command.get("op") == "exit":
                write_json(state / f"out-{number}.json", {"op": "exit", "ok": True})
                return 0
            try:
                result = agent.run(command)
            except (KeyError, ValueError) as error:
                result = {"op": command.get("op"), "ok": False, "code": "bad_command", "detail": str(error)}
            write_json(state / f"out-{number}.json", result)
            number += 1
            idle_until = time.monotonic() + idle_seconds
    finally:
        session.close()


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
    agent = commands.add_parser("agent")
    agent.add_argument("--state-dir", required=True, type=Path)
    agent.add_argument("--mcp", default="/usr/bin/splinterm-mcp")
    agent.add_argument(
        "--client-name",
        default="splinterm-vm-agent",
        help="the MCP client name the prompt and list show as a label (may hold control characters)",
    )
    agent.add_argument("--idle-seconds", type=float, default=1800)
    agent.add_argument("--poll-seconds", type=float, default=1.0)
    return result


def main() -> int:
    arguments = parser().parse_args()
    if arguments.command == "ids":
        print(json.dumps(probe_ids(arguments.topology, arguments.lair), sort_keys=True))
        return 0
    if arguments.command == "agent":
        return run_agent(
            [arguments.mcp],
            arguments.state_dir,
            arguments.client_name,
            arguments.idle_seconds,
            arguments.poll_seconds,
        )
    ids = json.loads(arguments.ids.read_text(encoding="utf-8"))
    result = run_probe([arguments.mcp], ids, arguments.state_dir, arguments.wait_seconds)
    passed = all(result[key] for key in ("acquired_before", "old_handle_expired", "reacquired"))
    print(json.dumps({key: value for key, value in result.items() if key != "responses"}))
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
