#!/usr/bin/env python3
"""Run one guarded twelve-step resize sequence on reserved workspace 8."""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import pathlib
import shutil
import subprocess
import sys
import time
from typing import Any

ROOT = pathlib.Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools/benchmark"
COMMON_PATH = TOOLS / "run-graphical-idle.py"
SIZES = ((800, 500), (1200, 700))

sys.path.insert(0, str(TOOLS))
from metrics import ProcessMetrics, snapshot_process_forest  # noqa: E402


def load_common():
    spec = importlib.util.spec_from_file_location(
        "splinterbench_resize_common", COMMON_PATH
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


COMMON = load_common()
V1 = COMMON.V1


def window_by_address(address: str) -> dict[str, Any] | None:
    return next(
        (item for item in V1.all_clients() if item.get("address") == address), None
    )


def wait_size(address: str, size: tuple[int, int]) -> None:
    deadline = time.monotonic() + 2
    while time.monotonic() < deadline:
        window = window_by_address(address)
        if window is None:
            raise RuntimeError("benchmark window exited during resize")
        if tuple(window.get("size", ())) == size:
            return
        V1.assert_user_workspace_untouched()
        time.sleep(0.01)
    observed = window_by_address(address)
    raise RuntimeError(
        f"resize did not settle at {size}; observed {observed.get('size') if observed else None}"
    )


def resize_sequence(app_id: str, address: str) -> tuple[int, int, list[int]]:
    dispatch_ns = 0
    started_ns = time.monotonic_ns()
    for width, height in SIZES * 6:
        selector = json.dumps(f"address:{address}")
        expression = (
            "hl.dispatch(hl.dsp.window.resize("
            f"{{ x = {width}, y = {height}, window = {selector} }}))"
        )
        dispatch_started = time.monotonic_ns()
        result = V1.run(["hyprctl", "eval", expression], capture_output=True, timeout=5)
        dispatch_ns += time.monotonic_ns() - dispatch_started
        if result.returncode:
            raise RuntimeError(result.stderr.strip() or result.stdout.strip())
        wait_size(address, (width, height))
        COMMON.assert_owned_window(app_id, address)
    settled_ns = time.monotonic_ns() - started_ns
    window = window_by_address(address)
    if window is None:
        raise RuntimeError("benchmark window exited after resize")
    return dispatch_ns, settled_ns, list(window["size"])


def resource_delta(before: ProcessMetrics, after: ProcessMetrics) -> dict[str, int]:
    return {
        "cpu_ticks": max(0, after.cpu_ticks - before.cpu_ticks),
        "context_switches": max(0, after.context_switches - before.context_switches),
        "rss_before_bytes": before.rss_bytes,
        "rss_after_bytes": after.rss_bytes,
        "rss_growth_bytes": max(0, after.rss_bytes - before.rss_bytes),
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run one guarded twelve-step resize case"
    )
    parser.add_argument("output_dir", type=pathlib.Path)
    parser.add_argument("--terminal", choices=tuple(COMMON.APP_IDS), required=True)
    parser.add_argument("--settle-seconds", type=float, default=1.0)
    args = parser.parse_args()
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        parser.error("a running Hyprland session is required")
    if args.settle_seconds < 0:
        parser.error("settle duration must be nonnegative")

    args.output_dir.mkdir(parents=True, exist_ok=True)
    state = pathlib.Path(f"/tmp/splinterbench-resize-{args.terminal}-{os.getpid()}")
    shutil.rmtree(state, ignore_errors=True)
    state.mkdir(mode=0o700)
    socket = state / "splinterd.sock"
    daemon = None
    daemon_log = None
    address = None
    report: dict[str, Any] = {
        "schema": "splinterm.benchmark.graphical-resize.v1",
        "terminal": args.terminal,
        "launch_mode": (
            "prestarted_daemon_client_launch"
            if args.terminal == "splinterm"
            else "standalone_process_launch"
        ),
        "valid": False,
        "notes": [],
    }
    try:
        V1.assert_test_workspace_isolated()
        V1.assert_user_workspace_untouched()
        if args.terminal == "splinterm":
            environment = os.environ.copy()
            environment.update(
                SPLINTERM_SOCKET=str(socket),
                SPLINTERM_ENABLE_DEV_ATTACH="1",
                SPLINTERM_CONFIG=str(COMMON.PROFILES / "splinterm.ini"),
                XDG_STATE_HOME=str(state / "xdg-state"),
            )
            daemon_log = (state / "daemon.log").open("w", encoding="utf-8")
            daemon = subprocess.Popen(
                [str(ROOT / "target/release/splinterd")],
                env=environment,
                stdin=subprocess.DEVNULL,
                stdout=daemon_log,
                stderr=subprocess.STDOUT,
                start_new_session=True,
                text=True,
            )
            COMMON.wait_socket(socket, daemon)
        command, environment = COMMON.launch_command(args.terminal, state, socket, 30)
        launcher = state / "launch.sh"
        COMMON.write_launcher(launcher, command, environment)
        existing = {item["address"] for item in V1.all_clients()}
        launched_ns = time.monotonic_ns()
        COMMON.dispatch_launcher(launcher)
        window, ready, _, _ = COMMON.wait_launch(
            COMMON.APP_IDS[args.terminal], existing, state / "ready.json", launched_ns
        )
        address = str(window["address"])
        COMMON.assert_owned_window(COMMON.APP_IDS[args.terminal], address)
        roots = [int(window["pid"])]
        if daemon is not None:
            roots.insert(0, daemon.pid)
        child_pid = int(ready["pid"])
        measured = [*roots, child_pid]
        time.sleep(args.settle_seconds)
        before = snapshot_process_forest(measured)
        dispatch_ns, settled_ns, final_size = resize_sequence(
            COMMON.APP_IDS[args.terminal], address
        )
        COMMON.assert_owned_window(COMMON.APP_IDS[args.terminal], address)
        after = snapshot_process_forest(measured)
        report.update(
            resize={
                "count": 12,
                "sizes": [list(size) for size in SIZES],
                "dispatch_duration_ns": dispatch_ns,
                "settled_duration_ns": settled_ns,
                "final_size": final_size,
                "window_survived": True,
            },
            resources=resource_delta(before, after),
            processes={
                "root_pids": roots,
                "child_pid": child_pid,
                "child_included": True,
                "count_before": before.process_count,
                "count_after": after.process_count,
            },
            isolation={
                "workspace": 8,
                "monitor": "DP-2",
                "no_initial_focus": True,
                "cleanup_verified": False,
            },
            valid=True,
        )
    except Exception as error:
        report["notes"].append(str(error))
    finally:
        if address is not None:
            V1.kill_oracle_window(address)
        if daemon is not None:
            try:
                COMMON.splinterm_client(socket, "terminate")
            except (OSError, subprocess.TimeoutExpired):
                pass
            daemon.terminate()
            try:
                daemon.wait(timeout=3)
            except subprocess.TimeoutExpired:
                daemon.kill()
                daemon.wait(timeout=2)
        if daemon_log is not None:
            daemon_log.close()
        try:
            COMMON.wait_cleanup()
            report.setdefault(
                "isolation",
                {
                    "workspace": 8,
                    "monitor": "DP-2",
                    "no_initial_focus": True,
                    "cleanup_verified": False,
                },
            )["cleanup_verified"] = True
        except Exception as error:
            report["valid"] = False
            report["notes"].append(f"cleanup: {error}")
        output = args.output_dir / f"{args.terminal}-resize.json"
        output.write_text(
            json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        print(f"Guarded resize result: {output}")
        shutil.rmtree(state, ignore_errors=True)
    return 0 if report.get("valid") else 1


if __name__ == "__main__":
    raise SystemExit(main())
