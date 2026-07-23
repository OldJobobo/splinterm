#!/usr/bin/env python3
"""Run one guarded mixed-output memory-retention case."""

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
OUTPUT_PATH = TOOLS / "run-graphical-output.py"

sys.path.insert(0, str(TOOLS))
from metrics import ProcessMetrics, snapshot_process_forest  # noqa: E402


def load_output():
    spec = importlib.util.spec_from_file_location(
        "splinterbench_retention_common", OUTPUT_PATH
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


OUTPUT = load_output()
COMMON = OUTPUT.COMMON
V1 = COMMON.V1


def wait_retention(
    app_id: str,
    address: str,
    window: dict[str, Any],
    done_path: pathlib.Path,
    screenshot: pathlib.Path,
    roots: list[int],
    trigger_ns: int,
) -> tuple[int, int, ProcessMetrics]:
    done = False
    visible_ns = None
    pixels = 0
    peak = snapshot_process_forest(roots)
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        COMMON.assert_owned_window(app_id, address)
        current = snapshot_process_forest(roots)
        if current.rss_bytes > peak.rss_bytes:
            peak = current
        done = done or done_path.exists()
        if visible_ns is None:
            observed = OUTPUT.screenshot_marker(window, screenshot)
            if observed >= 100:
                pixels = observed
                visible_ns = time.monotonic_ns()
        if done and visible_ns is not None:
            return visible_ns - trigger_ns, pixels, peak
        time.sleep(0.02)
    raise RuntimeError("retention output or visible marker timed out")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run one guarded memory-retention case"
    )
    parser.add_argument("output_dir", type=pathlib.Path)
    parser.add_argument("--terminal", choices=tuple(COMMON.APP_IDS), required=True)
    parser.add_argument("--lines", type=int, default=5000)
    parser.add_argument("--settle-seconds", type=float, default=1.0)
    parser.add_argument("--post-settle-seconds", type=float, default=2.0)
    args = parser.parse_args()
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        parser.error("a running Hyprland session is required")
    if args.lines < 500 or args.settle_seconds < 0 or args.post_settle_seconds < 0:
        parser.error("invalid retention dimensions or settle duration")

    args.output_dir.mkdir(parents=True, exist_ok=True)
    state = pathlib.Path(f"/tmp/splinterbench-retention-{args.terminal}-{os.getpid()}")
    shutil.rmtree(state, ignore_errors=True)
    state.mkdir(mode=0o700)
    socket = state / "splinterd.sock"
    daemon = None
    daemon_log = None
    address = None
    screenshot = state / "marker.png"
    report: dict[str, Any] = {
        "schema": "splinterm.benchmark.graphical-retention.v1",
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
        command, environment = COMMON.launch_command(
            args.terminal,
            state,
            socket,
            30,
            case="retention",
            lines=args.lines,
            columns=80,
        )
        launcher = state / "launch.sh"
        COMMON.write_launcher(launcher, command, environment)
        existing = {item["address"] for item in V1.all_clients()}
        launched = time.monotonic_ns()
        COMMON.dispatch_launcher(launcher)
        window, ready, _, _ = COMMON.wait_launch(
            COMMON.APP_IDS[args.terminal], existing, state / "ready.json", launched
        )
        address = str(window["address"])
        roots = [int(window["pid"])]
        if daemon is not None:
            roots.insert(0, daemon.pid)
        child_pid = int(ready["pid"])
        measured = [*roots, child_pid]
        time.sleep(args.settle_seconds)
        baseline = snapshot_process_forest(measured)
        trigger_ns = time.monotonic_ns()
        (state / "start").touch()
        visible_ns, _, peak = wait_retention(
            COMMON.APP_IDS[args.terminal],
            address,
            window,
            state / "done.json",
            screenshot,
            measured,
            trigger_ns,
        )
        time.sleep(args.post_settle_seconds)
        COMMON.assert_owned_window(COMMON.APP_IDS[args.terminal], address)
        post = snapshot_process_forest(measured)
        report.update(
            retention={
                "lines": args.lines,
                "clear_cycles": max(1, (args.lines - 1) // 500),
                "trigger_to_visible_marker_ns": visible_ns,
                "rss_baseline_bytes": baseline.rss_bytes,
                "rss_peak_observed_bytes": max(peak.rss_bytes, post.rss_bytes),
                "rss_post_settle_bytes": post.rss_bytes,
                "retained_growth_bytes": max(0, post.rss_bytes - baseline.rss_bytes),
                "cpu_ticks": max(0, post.cpu_ticks - baseline.cpu_ticks),
                "context_switches": max(
                    0, post.context_switches - baseline.context_switches
                ),
            },
            processes={
                "root_pids": roots,
                "child_pid": child_pid,
                "child_included": True,
                "count": post.process_count,
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
        output = args.output_dir / f"{args.terminal}-retention.json"
        output.write_text(
            json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        print(f"Guarded retention result: {output}")
        shutil.rmtree(state, ignore_errors=True)
    return 0 if report.get("valid") else 1


if __name__ == "__main__":
    raise SystemExit(main())
