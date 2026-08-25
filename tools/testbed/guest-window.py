#!/usr/bin/env python3
"""Guard one disposable Splinterm window inside the Omarchy test VM."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
from typing import Any, NoReturn


APP_ID = "com.oldjobobo.splinterm"
TARGET_MONITOR = "Virtual-1"
TARGET_WORKSPACE = 8
ADDRESS = re.compile(r"^0x[0-9A-Fa-f]+$")


def fail(message: str) -> NoReturn:
    raise RuntimeError(message)


def command(*arguments: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        arguments,
        check=check,
        text=True,
        capture_output=True,
    )


def hypr_json(name: str, *arguments: str) -> Any:
    result = command("hyprctl", "-j", name, *arguments)
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError as error:
        fail(f"hyprctl {name} returned invalid JSON: {error}")


def clients() -> list[dict[str, Any]]:
    value = hypr_json("clients")
    if not isinstance(value, list):
        fail("hyprctl clients did not return a list")
    return value


def monitor_record() -> dict[str, Any]:
    monitors = hypr_json("monitors", "all")
    matches = [monitor for monitor in monitors if monitor.get("name") == TARGET_MONITOR]
    if len(matches) != 1:
        fail(f"expected exactly one active {TARGET_MONITOR} monitor, found {len(matches)}")
    monitor = matches[0]
    if monitor.get("disabled") is True:
        fail(f"target monitor {TARGET_MONITOR} is disabled")
    return {
        "id": monitor.get("id"),
        "name": monitor.get("name"),
        "scale": monitor.get("scale"),
        "transform": monitor.get("transform"),
    }


def workspace_id(client: dict[str, Any]) -> int | None:
    workspace = client.get("workspace")
    return workspace.get("id") if isinstance(workspace, dict) else None


def require_empty_target_workspace(current_clients: list[dict[str, Any]]) -> None:
    occupants = [client for client in current_clients if workspace_id(client) == TARGET_WORKSPACE]
    if occupants:
        fail(f"guest workspace {TARGET_WORKSPACE} is not empty")
    workspaces = hypr_json("workspaces")
    matches = [workspace for workspace in workspaces if workspace.get("id") == TARGET_WORKSPACE]
    if len(matches) > 1:
        fail(f"guest workspace {TARGET_WORKSPACE} is ambiguous")
    if matches and matches[0].get("monitor") != TARGET_MONITOR:
        fail(
            f"guest workspace {TARGET_WORKSPACE} belongs to "
            f"{matches[0].get('monitor')}, not {TARGET_MONITOR}"
        )


def write_state(path: Path, state: dict[str, Any]) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".next")
    temporary.write_text(json.dumps(state, sort_keys=True) + "\n", encoding="utf-8")
    temporary.chmod(0o600)
    temporary.replace(path)


def read_state(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as error:
        fail(f"guest window state is missing: {path}")
    except json.JSONDecodeError as error:
        fail(f"guest window state is invalid: {error}")
    if not isinstance(value, dict):
        fail("guest window state is not an object")
    return value


def prepare(path: Path) -> None:
    if path.exists():
        fail(f"guest window state already exists; run stop first: {path}")
    current_clients = clients()
    require_empty_target_workspace(current_clients)
    active_workspace = hypr_json("activeworkspace")
    active_window = hypr_json("activewindow")
    cursor = hypr_json("cursorpos")
    monitor = monitor_record()
    address = active_window.get("address", "") if isinstance(active_window, dict) else ""
    if address and not ADDRESS.fullmatch(address):
        fail(f"active window returned an invalid address: {address}")
    write_state(
        path,
        {
            "active_window": address,
            "active_workspace": active_workspace.get("id"),
            "before_addresses": sorted(
                client["address"]
                for client in current_clients
                if isinstance(client.get("address"), str)
            ),
            "cursor": {"x": cursor.get("x"), "y": cursor.get("y")},
            "monitor": monitor,
            "target_address": None,
            "target_initial": None,
            "target_final": None,
        },
    )
    command(
        "hyprctl",
        "dispatch",
        f'hl.dsp.focus({{ monitor = "{TARGET_MONITOR}" }})',
    )
    command(
        "hyprctl",
        "dispatch",
        f'hl.dsp.focus({{ workspace = "{TARGET_WORKSPACE}" }})',
    )
    active_target = hypr_json("activeworkspace")
    if (
        active_target.get("id") != TARGET_WORKSPACE
        or active_target.get("monitor") != TARGET_MONITOR
    ):
        fail(f"could not activate workspace {TARGET_WORKSPACE} on {TARGET_MONITOR}")


def fresh_candidates(state: dict[str, Any]) -> list[dict[str, Any]]:
    before = set(state.get("before_addresses", []))
    return [
        client
        for client in clients()
        if client.get("address") not in before
        and client.get("initialClass") == APP_ID
        and ADDRESS.fullmatch(str(client.get("address", "")))
    ]


def place(path: Path) -> None:
    state = read_state(path)
    candidates: list[dict[str, Any]] = []
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        candidates = fresh_candidates(state)
        if len(candidates) == 1:
            break
        if len(candidates) > 1:
            fail(f"expected one fresh Splinterm window, found {len(candidates)}")
        time.sleep(0.05)
    if len(candidates) != 1:
        fail("fresh Splinterm window did not map within 10 seconds")

    candidate = candidates[0]
    address = candidate["address"]
    state["target_address"] = address
    state["target_initial"] = {
        "address": address,
        "pid": candidate.get("pid"),
        "workspace": workspace_id(candidate),
        "monitor": candidate.get("monitor"),
        "at": candidate.get("at"),
        "size": candidate.get("size"),
    }
    write_state(path, state)
    selector = f"address:{address}"
    command(
        "hyprctl",
        "dispatch",
        f'hl.dsp.window.move({{ monitor = "{TARGET_MONITOR}", follow = false, window = "{selector}" }})',
    )
    command(
        "hyprctl",
        "dispatch",
        f'hl.dsp.window.move({{ workspace = "{TARGET_WORKSPACE}", follow = false, window = "{selector}" }})',
    )
    matches = [client for client in clients() if client.get("address") == address]
    if len(matches) != 1:
        fail("fresh Splinterm window disappeared during placement")
    monitor = monitor_record()
    if workspace_id(matches[0]) != TARGET_WORKSPACE or matches[0].get("monitor") != monitor["id"]:
        fail(f"fresh Splinterm window was not isolated on workspace {TARGET_WORKSPACE} / {TARGET_MONITOR}")
    placed = matches[0]
    state["target_final"] = {
        "address": address,
        "pid": placed.get("pid"),
        "workspace": workspace_id(placed),
        "monitor": placed.get("monitor"),
        "at": placed.get("at"),
        "size": placed.get("size"),
    }
    write_state(path, state)
    print(
        f"guest window address={address} pid={placed.get('pid')} "
        f"workspace={TARGET_WORKSPACE} monitor={TARGET_MONITOR} "
        f"at={placed.get('at')} size={placed.get('size')}"
    )


def restore(path: Path) -> None:
    state = read_state(path)
    target_address = state.get("target_address")
    deadline = time.monotonic() + 10
    while target_address and time.monotonic() < deadline:
        if not any(client.get("address") == target_address for client in clients()):
            break
        time.sleep(0.05)
    current_clients = clients()
    if target_address and any(client.get("address") == target_address for client in current_clients):
        fail(f"guest test window is still mapped: {target_address}")
    require_empty_target_workspace(current_clients)

    recorded_monitor = state.get("monitor")
    if monitor_record() != recorded_monitor:
        fail(f"target monitor state changed during the test: {recorded_monitor!r}")
    workspace = state.get("active_workspace")
    if not isinstance(workspace, int) or workspace < 1:
        fail(f"recorded workspace is invalid: {workspace!r}")
    command("hyprctl", "dispatch", f'hl.dsp.focus({{ workspace = "{workspace}" }})')
    active_window = state.get("active_window")
    if active_window:
        if not ADDRESS.fullmatch(active_window):
            fail(f"recorded active window address is invalid: {active_window}")
        if any(client.get("address") == active_window for client in current_clients):
            command(
                "hyprctl",
                "dispatch",
                f'hl.dsp.focus({{ window = "address:{active_window}" }})',
            )
    cursor = state.get("cursor", {})
    x, y = cursor.get("x"), cursor.get("y")
    if not isinstance(x, (int, float)) or not isinstance(y, (int, float)):
        fail(f"recorded cursor is invalid: {cursor!r}")
    ydotool_socket = Path(os.environ.get("YDOTOOL_SOCKET", ""))
    if not ydotool_socket.is_socket():
        fail(f"ydotool socket is unavailable: {ydotool_socket}")
    command("ydotool", "mousemove", "--absolute", "-x", str(round(x)), "-y", str(round(y)))
    path.unlink()
    print(f"guest workspace {TARGET_WORKSPACE} is empty; prior focus and cursor restored")


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("action", choices=("prepare", "place", "restore"))
    result.add_argument("--state", required=True, type=Path)
    return result


def main() -> int:
    arguments = parser().parse_args()
    try:
        {"prepare": prepare, "place": place, "restore": restore}[arguments.action](arguments.state)
    except (RuntimeError, subprocess.CalledProcessError) as error:
        if isinstance(error, subprocess.CalledProcessError) and error.stderr:
            print(error.stderr.rstrip(), file=sys.stderr)
        print(f"guest-window: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
