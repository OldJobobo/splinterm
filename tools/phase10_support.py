"""Guarded helpers for the Slice 10 Omarchy/Hyprland sign-off."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import pathlib
import subprocess
import time
from typing import Any

ROOT = pathlib.Path(__file__).resolve().parents[1]
ORACLE = ROOT / "tools/foot-oracle/run-final-buffer-comparison.py"
SLICE3 = ROOT / "tools/foot-oracle/run-slice3-final-buffer-comparison.py"
APP_ID = "com.oldjobobo.splinterm"
WORKSPACE = 8
MONITOR = "DP-2"


def load(name: str, path: pathlib.Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


V1 = load("slice10_guard", ORACLE)
S3 = load("slice10_scale", SLICE3)


def run(command: list[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
    kwargs.setdefault("check", False)
    return subprocess.run(command, text=True, **kwargs)


def client(socket: pathlib.Path, *arguments: str) -> subprocess.CompletedProcess[str]:
    environment = os.environ.copy()
    environment.update(SPLINTERM_SOCKET=str(socket), SPLINTERM_ENABLE_DEV_ATTACH="1")
    return run(
        [str(ROOT / "target/release/splinterm"), *arguments],
        env=environment,
        capture_output=True,
        timeout=10,
    )


def process_metrics(pid: int) -> dict[str, int]:
    status = pathlib.Path(f"/proc/{pid}/status").read_text(encoding="utf-8")
    stat = pathlib.Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
    fields = stat[stat.rfind(")") + 2 :].split()
    rss = next(
        (int(line.split()[1]) * 1024 for line in status.splitlines() if line.startswith("VmRSS:")),
        0,
    )
    return {"cpu_ticks": int(fields[11]) + int(fields[12]), "rss_bytes": rss}


def descendants(pid: int) -> list[int]:
    found: set[int] = set()
    pending = [pid]
    while pending:
        parent = pending.pop()
        children = pathlib.Path(f"/proc/{parent}/task/{parent}/children")
        try:
            values = [int(value) for value in children.read_text().split()]
        except OSError:
            values = []
        for child in values:
            if child not in found:
                found.add(child)
                pending.append(child)
    return sorted(found)


def wait_until(predicate, seconds: float, message: str):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        V1.assert_user_workspace_untouched()
        time.sleep(0.05)
    raise RuntimeError(message)


def window_by_address(address: str) -> dict[str, Any] | None:
    return next((item for item in V1.all_clients() if item.get("address") == address), None)


def screenshot(window: dict[str, Any], path: pathlib.Path) -> dict[str, Any]:
    x, y = window["at"]
    width, height = window["size"]
    result = run(
        ["grim", "-g", f"{x},{y} {width}x{height}", str(path)],
        capture_output=True,
        timeout=10,
    )
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or "grim capture failed")
    return {
        "file": path.name,
        "width": width,
        "height": height,
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
    }


def output_screenshot(path: pathlib.Path) -> dict[str, Any]:
    result = run(["grim", "-o", MONITOR, str(path)], capture_output=True, timeout=10)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or "grim output capture failed")
    return {"file": path.name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}


def apply_monitor_scale_owned(original: dict[str, Any], scale_120: int, address: str) -> None:
    expression = S3.monitor_expression(original, scale_120 / 120)
    result = run(["hyprctl", "eval", expression], capture_output=True, timeout=5)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or result.stdout.strip())
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        current = S3.monitor_state()
        clients = V1.workspace_clients(WORKSPACE)
        owned = len(clients) == 1 and clients[0].get("address") == address
        if abs(float(current["scale"]) - scale_120 / 120) < 0.001 and owned:
            if clients[0].get("monitor") != V1.test_monitor_id():
                raise RuntimeError("owned sign-off window left DP-2")
            V1.assert_user_workspace_untouched()
            return
        time.sleep(0.05)
    raise RuntimeError(f"DP-2 did not reach scale {scale_120}/120 with its owned window")


def window_title(address: str, title: str):
    return wait_until(
        lambda: (item := window_by_address(address)) and item.get("title") == title and item,
        10,
        f"window title did not reach {title!r}",
    )


def snapshot_has(socket: pathlib.Path, marker: str) -> bool:
    response = client(socket, "snapshot")
    return response.returncode == 0 and marker in response.stdout
