#!/usr/bin/env python3
"""Run the single guarded Phase 3 Slice 4 two-window smoke case."""

from __future__ import annotations

import importlib.util
import json
import os
import pathlib
import re
import shlex
import shutil
import signal
import subprocess
import sys
import time
from typing import Any, Callable

ROOT = pathlib.Path(__file__).resolve().parents[1]
GUARD_PATH = ROOT / "tools/foot-oracle/run-final-buffer-comparison.py"
APP_ID = "com.oldjobobo.splinterm"
WORKSPACE = 8


def load_guard():
    spec = importlib.util.spec_from_file_location("slice4_guard", GUARD_PATH)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


V1 = load_guard()


def run(command: list[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
    kwargs.setdefault("check", False)
    return subprocess.run(command, text=True, **kwargs)


def wait_until(predicate: Callable[[], Any], seconds: float, message: str) -> Any:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        V1.assert_user_workspace_untouched()
        time.sleep(0.05)
    raise RuntimeError(message)


def main() -> int:
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        print("A running Hyprland session is required.", file=sys.stderr)
        return 2
    output = pathlib.Path("/tmp/splinterm-phase3-slice4-window-smoke")
    shutil.rmtree(output, ignore_errors=True)
    runtime = output / "runtime"
    state = output / "state"
    config = output / "config/splinterm"
    runtime.mkdir(parents=True, mode=0o700)
    state.mkdir(parents=True, mode=0o700)
    config.mkdir(parents=True)
    (config / "config.ini").write_text("[scrollback]\nlines=2048\n", encoding="utf-8")

    V1.assert_test_workspace_isolated()
    V1.assert_user_workspace_untouched()
    active_before = V1.hyprland_json("activeworkspace")
    active_window_before = V1.hyprland_json("activewindow")
    cursor_before = V1.hyprland_json("cursorpos")
    run(
        ["cargo", "build", "--release", "-q", "-p", "splinterd", "-p", "splinterm"],
        cwd=ROOT,
        check=True,
    )

    socket = runtime / "splinterd.sock"
    environment = os.environ.copy()
    environment.update(
        SPLINTERM_SOCKET=str(socket),
        SPLINTERM_ENABLE_DEV_ATTACH="1",
        XDG_STATE_HOME=str(state),
        XDG_CONFIG_HOME=str(output / "config"),
    )
    daemon_log = (output / "daemon.log").open("w", encoding="utf-8")
    daemon = subprocess.Popen(
        [str(ROOT / "target/release/splinterd")],
        env=environment,
        stdin=subprocess.DEVNULL,
        stdout=daemon_log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
        text=True,
    )
    addresses: set[str] = set()
    splints: list[str] = []
    windows: list[str] = []
    exact = False
    error: str | None = None

    def client(*arguments: str) -> subprocess.CompletedProcess[str]:
        return run(
            [str(ROOT / "target/release/splinterm"), *arguments],
            env=environment,
            capture_output=True,
            timeout=10,
        )

    def checked_client(*arguments: str) -> str:
        result = client(*arguments)
        if result.returncode:
            raise RuntimeError(result.stderr.strip() or f"client {' '.join(arguments)} failed")
        return result.stdout

    def topology_ids() -> tuple[str, list[str], list[str]]:
        listing = checked_client("list")
        dojo = re.search(r"^([0-9a-f-]{36})  slice4-windows ", listing, re.MULTILINE)
        found_windows = re.findall(r"^  window ([0-9a-f-]{36})  ", listing, re.MULTILINE)
        found_splints = re.findall(r"^  ([0-9a-f-]{36})  ", listing, re.MULTILINE)
        if dojo is None or len(found_windows) != 2 or len(found_splints) != 2:
            raise RuntimeError(f"unexpected two-window topology:\n{listing}")
        return dojo.group(1), found_windows, found_splints

    def launch(dojo_id: str, window_id: str) -> dict[str, Any]:
        launcher = output / f"launch-{window_id}.sh"
        command = [
            "env",
            *[f"{key}={value}" for key, value in environment.items() if key in {
                "SPLINTERM_SOCKET", "SPLINTERM_ENABLE_DEV_ATTACH", "XDG_STATE_HOME",
                "XDG_CONFIG_HOME", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR",
            }],
            str(ROOT / "target/release/splinterm"),
            "window",
            "--dojo-id",
            dojo_id,
            "--window-id",
            window_id,
        ]
        stdout = output / f"window-{window_id}.stdout"
        stderr = output / f"window-{window_id}.stderr"
        launcher.write_text(
            "#!/bin/sh\nexec "
            + shlex.join(command)
            + f" >{shlex.quote(str(stdout))} 2>{shlex.quote(str(stderr))}\n",
            encoding="utf-8",
        )
        launcher.chmod(0o700)
        existing = {item["address"] for item in V1.all_clients()}
        expression = (
            f"hl.exec_cmd({json.dumps(str(launcher))}, "
            "{ workspace = '8 silent', float = true, size = '700 420', "
            "no_initial_focus = true })"
        )
        dispatched = run(["hyprctl", "eval", expression], capture_output=True, timeout=5)
        if dispatched.returncode:
            raise RuntimeError(dispatched.stderr.strip() or dispatched.stdout.strip())
        window = wait_until(
            lambda: next(
                (
                    item for item in V1.all_clients()
                    if item.get("class") == APP_ID and item.get("address") not in existing
                ),
                None,
            ),
            8,
            "selected window did not map",
        )
        if window["workspace"]["id"] != WORKSPACE or window["monitor"] != V1.test_monitor_id():
            raise RuntimeError("window escaped workspace 8 / DP-2")
        addresses.add(window["address"])
        return window

    def snapshot_size(splint_id: str) -> tuple[int, int]:
        text = checked_client("snapshot", splint_id)
        match = re.search(r"· (\d+)x(\d+)$", text.splitlines()[0])
        if match is None:
            raise RuntimeError(f"snapshot omitted dimensions: {text}")
        return int(match.group(1)), int(match.group(2))

    def resize(address: str, width: int, height: int) -> None:
        selector = json.dumps(f"address:{address}")
        expression = (
            "hl.dispatch(hl.dsp.window.resize({ "
            f"x = {width}, y = {height}, window = {selector} }}))"
        )
        result = run(["hyprctl", "eval", expression], capture_output=True, timeout=5)
        if result.returncode:
            raise RuntimeError("targeted no-focus resize failed")

    try:
        wait_until(lambda: socket.exists() and client("ping").returncode == 0, 5, "daemon not ready")
        checked_client("new", "slice4-windows", "--", "/bin/sh")
        first_listing = checked_client("list")
        dojo_match = re.search(r"^([0-9a-f-]{36})  slice4-windows ", first_listing, re.MULTILINE)
        if dojo_match is None:
            raise RuntimeError("new Dojo was not listed")
        checked_client(
            "new-window", dojo_match.group(1), "--title", "second", "--", "/bin/sh",
        )
        dojo_id, windows, splints = topology_ids()
        checked_client("send", splints[0], "printf '\\033]0;Slice4 One\\007ONE_INPUT\\n'\n")
        checked_client("send", splints[1], "printf '\\033]0;Slice4 Two\\007TWO_INPUT\\n'\n")
        wait_until(lambda: "ONE_INPUT" in checked_client("snapshot", splints[0]), 5, "first input missing")
        wait_until(lambda: "TWO_INPUT" in checked_client("snapshot", splints[1]), 5, "second input missing")
        if "TWO_INPUT" in checked_client("snapshot", splints[0]) or "ONE_INPUT" in checked_client("snapshot", splints[1]):
            raise RuntimeError("input crossed daemon windows")

        first = launch(dojo_id, windows[0])
        second = launch(dojo_id, windows[1])
        if len(V1.workspace_clients(WORKSPACE)) != 2:
            raise RuntimeError("launch did not create exactly two selected toplevels")
        wait_until(
            lambda: "Slice4 One" in next(
                (item for item in V1.all_clients() if item["address"] == first["address"]),
                {},
            ).get("title", ""),
            5,
            "first toplevel did not attach to its selected input",
        )
        wait_until(
            lambda: "Slice4 Two" in next(
                (item for item in V1.all_clients() if item["address"] == second["address"]),
                {},
            ).get("title", ""),
            5,
            "second toplevel did not attach to its selected input",
        )

        before = [snapshot_size(splint) for splint in splints]
        resize(first["address"], 620, 360)
        resize(second["address"], 900, 520)

        def resized_sizes() -> list[tuple[int, int]] | None:
            sizes = [snapshot_size(splint) for splint in splints]
            if sizes[0] != before[0] and sizes[1] != before[1] and sizes[0] != sizes[1]:
                return sizes
            return None

        after = wait_until(
            resized_sizes,
            5,
            "independent graphical resizes did not reach both PTYs",
        )

        V1.kill_oracle_window(first["address"])
        wait_until(lambda: len(V1.workspace_clients(WORKSPACE)) == 1, 5, "first client did not close")
        wait_until(
            lambda: client(
                "send", splints[0], "printf 'DETACHED_CONTINUITY\\n'\n"
            ).returncode == 0,
            5,
            "closed client did not release its controller",
        )
        wait_until(
            lambda: "DETACHED_CONTINUITY" in checked_client("snapshot", splints[0]),
            5,
            "detached process did not continue",
        )
        reopened = launch(dojo_id, windows[0])
        if len(V1.workspace_clients(WORKSPACE)) != 2 or second["address"] not in {
            item["address"] for item in V1.workspace_clients(WORKSPACE)
        }:
            raise RuntimeError("reopen altered the other daemon window/client")
        if snapshot_size(splints[0]) == snapshot_size(splints[1]):
            raise RuntimeError("window-local resize continuity was lost")
        if reopened["address"] == second["address"]:
            raise RuntimeError("reopen selected the wrong toplevel")
        exact = True
    except Exception as caught:
        error = str(caught)
    finally:
        for address in list(addresses):
            V1.kill_oracle_window(address)
        try:
            wait_until(lambda: not V1.workspace_clients(WORKSPACE), 5, "test windows remained mapped")
        except Exception as caught:
            error = error or str(caught)
            exact = False
        for splint in splints:
            client("kill", splint, "--yes")
        for window in windows:
            client("close-window", window)
        daemon.send_signal(signal.SIGINT)
        try:
            daemon.wait(timeout=8)
        except subprocess.TimeoutExpired:
            daemon.kill()
            daemon.wait(timeout=3)
            exact = False
            error = error or "daemon required forced cleanup"
        daemon_log.close()

    active_after = V1.hyprland_json("activeworkspace")
    active_window_after = V1.hyprland_json("activewindow")
    cursor_after = V1.hyprland_json("cursorpos")
    cleanup_clean = not V1.workspace_clients(WORKSPACE) and not socket.exists()
    if active_after != active_before or active_window_after != active_window_before or cursor_after != cursor_before:
        exact = False
        error = error or "focus, active workspace, or pointer changed"
    if not cleanup_clean:
        exact = False
        error = error or "workspace or socket cleanup was incomplete"
    summary = {
        "schema": "splinterm.phase3.slice4.window-smoke.v1",
        "exact": exact,
        "error": error,
        "workspace": WORKSPACE,
        "monitor": "DP-2",
        "dojo_id": dojo_id if "dojo_id" in locals() else None,
        "window_ids": windows,
        "splint_ids": splints,
        "input_independent": exact,
        "resize_sizes": after if "after" in locals() else None,
        "close_reopen_continuity": exact,
        "active_workspace_unchanged": active_after == active_before,
        "active_window_unchanged": active_window_after == active_window_before,
        "pointer_unchanged": cursor_after == cursor_before,
        "cleanup_clean": cleanup_clean,
    }
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary, indent=2))
    return 0 if exact else 1


if __name__ == "__main__":
    raise SystemExit(main())
