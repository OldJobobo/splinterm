#!/usr/bin/env python3
"""Run one guarded mixed-output retention case with procfs attribution."""

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
from metrics import (  # noqa: E402
    ProcessMetrics,
    snapshot_process_forest,
    snapshot_process_memory_forest,
)


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
) -> tuple[int, int, ProcessMetrics, dict[str, Any]]:
    done = False
    visible_ns = None
    marker_memory = None
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
                marker_memory = snapshot_process_memory_forest(roots)
        if done and visible_ns is not None:
            assert marker_memory is not None
            return visible_ns - trigger_ns, pixels, peak, marker_memory
        time.sleep(0.02)
    raise RuntimeError("retention output or visible marker timed out")


def parse_settle_points(value: str) -> list[float]:
    points = sorted({float(item) for item in value.split(",")})
    if not points or points[0] < 0 or points[-1] > 120:
        raise ValueError("settle points must be between zero and 120 seconds")
    return points


def failure_record(phase: str, error: Exception) -> dict[str, Any]:
    failure = {
        "phase": phase,
        "type": type(error).__name__,
        "message": str(error),
    }
    details = getattr(error, "details", None)
    if details is not None:
        failure["isolation"] = details
    return failure


def sample_settles(
    app_id: str,
    address: str,
    roots: list[int],
    visible_at: float,
    points: list[float],
) -> list[dict[str, Any]]:
    samples = []
    for point in points:
        remaining = visible_at + point - time.monotonic()
        if remaining > 0:
            time.sleep(remaining)
        COMMON.assert_owned_window(app_id, address)
        samples.append({"seconds": point, "memory": snapshot_process_memory_forest(roots)})
    return samples


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run one guarded memory-retention case"
    )
    parser.add_argument("output_dir", type=pathlib.Path)
    parser.add_argument("--terminal", choices=tuple(COMMON.APP_IDS), required=True)
    parser.add_argument("--lines", type=int, default=5000)
    parser.add_argument("--settle-seconds", type=float, default=1.0)
    parser.add_argument("--settle-points", default="2,10,30,120")
    parser.add_argument("--variant", default="candidate")
    args = parser.parse_args()
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        parser.error("a running Hyprland session is required")
    try:
        settle_points = parse_settle_points(args.settle_points)
    except ValueError as error:
        parser.error(str(error))
    if args.lines < 500 or args.settle_seconds < 0:
        parser.error("invalid retention dimensions or settle duration")

    args.output_dir.mkdir(parents=True, exist_ok=True)
    state = pathlib.Path(f"/tmp/splinterbench-retention-{args.terminal}-{os.getpid()}")
    shutil.rmtree(state, ignore_errors=True)
    state.mkdir(mode=0o700)
    socket = state / "splinterd.sock"
    daemon = None
    daemon_log = None
    address = None
    window_pid = None
    child_pid = None
    screenshot = state / "marker.png"
    phase = "preflight"
    report: dict[str, Any] = {
        "schema": "splinterm.benchmark.graphical-retention.v2",
        "terminal": args.terminal,
        "variant": args.variant,
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
        phase = "daemon_start"
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
                [str(COMMON.splinterd_executable())],
                env=environment,
                stdin=subprocess.DEVNULL,
                stdout=daemon_log,
                stderr=subprocess.STDOUT,
                start_new_session=True,
                text=True,
            )
            COMMON.wait_socket(socket, daemon)
        phase = "launcher_prepare"
        command, environment = COMMON.launch_command(
            args.terminal,
            state,
            socket,
            30,
            case="retention",
            lines=args.lines,
            columns=80,
            hold_seconds=max(settle_points) + 10,
        )
        launcher = state / "launch.sh"
        COMMON.write_launcher(launcher, command, environment)
        existing = {item["address"] for item in V1.all_clients()}
        launched = time.monotonic_ns()
        phase = "launcher_dispatch"
        COMMON.dispatch_launcher(launcher)
        phase = "launch_wait"
        window, ready, _, _ = COMMON.wait_launch(
            COMMON.APP_IDS[args.terminal], existing, state / "ready.json", launched
        )
        address = str(window["address"])
        window_pid = int(window["pid"])
        phase = "post_launch_ownership"
        COMMON.assert_owned_window(COMMON.APP_IDS[args.terminal], address)
        roots = [window_pid]
        if daemon is not None:
            roots.insert(0, daemon.pid)
        child_pid = int(ready["pid"])
        measured = [*roots, child_pid]
        phase = "baseline_settle"
        time.sleep(args.settle_seconds)
        phase = "pre_baseline_ownership"
        COMMON.assert_owned_window(COMMON.APP_IDS[args.terminal], address)
        phase = "baseline"
        baseline = snapshot_process_forest(measured)
        baseline_memory = snapshot_process_memory_forest(measured)
        phase = "pre_trigger_ownership"
        COMMON.assert_owned_window(COMMON.APP_IDS[args.terminal], address)
        phase = "trigger"
        trigger_ns = time.monotonic_ns()
        (state / "start").touch()
        phase = "output_wait"
        visible_ns, _, peak, marker_memory = wait_retention(
            COMMON.APP_IDS[args.terminal],
            address,
            window,
            state / "done.json",
            screenshot,
            measured,
            trigger_ns,
        )
        visible_at = time.monotonic()
        phase = "settle_sampling"
        settle_samples = sample_settles(
            COMMON.APP_IDS[args.terminal], address, measured, visible_at, settle_points
        )
        phase = "post_sample"
        post = snapshot_process_forest(measured)
        post_memory = settle_samples[-1]["memory"]
        report.update(
            retention={
                "lines": args.lines,
                "clear_cycles": max(1, (args.lines - 1) // 500),
                "trigger_to_visible_marker_ns": visible_ns,
                "rss_baseline_bytes": baseline.rss_bytes,
                "rss_peak_observed_bytes": max(peak.rss_bytes, post.rss_bytes),
                "rss_post_settle_bytes": post_memory["aggregate"]["rss_bytes"],
                "retained_growth_bytes": max(
                    0,
                    post_memory["aggregate"]["rss_bytes"]
                    - baseline_memory["aggregate"]["rss_bytes"],
                ),
                "cpu_ticks": max(0, post.cpu_ticks - baseline.cpu_ticks),
                "context_switches": max(
                    0, post.context_switches - baseline.context_switches
                ),
            },
            memory_timeline={
                "classification": {
                    "private_anon": "min(total private, Anonymous) from smaps_rollup",
                    "private_file": "total private minus classified private anonymous",
                    "shared": "Shared_Clean plus Shared_Dirty",
                    "shmem": "ShmemPmdMapped",
                },
                "baseline": baseline_memory,
                "peak_observed_rss_bytes": peak.rss_bytes,
                "marker_visible": marker_memory,
                "settles": settle_samples,
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
        phase = "complete"
    except Exception as error:
        report["failure"] = failure_record(phase, error)
        report["process_state_at_failure"] = {
            "daemon_returncode": daemon.poll() if daemon is not None else None,
            "window_pid": window_pid,
            "window_process_exists": (
                pathlib.Path(f"/proc/{window_pid}").exists()
                if window_pid is not None
                else None
            ),
            "child_pid": child_pid,
            "child_process_exists": (
                pathlib.Path(f"/proc/{child_pid}").exists()
                if child_pid is not None
                else None
            ),
            "ready_file_exists": (state / "ready.json").exists(),
            "done_file_exists": (state / "done.json").exists(),
            "trigger_file_exists": (state / "start").exists(),
        }
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
            report["cleanup_failure"] = failure_record("cleanup", error)
            report["notes"].append(f"cleanup: {error}")
        if not report.get("valid"):
            debug = args.output_dir / "failure-state"
            debug.mkdir(exist_ok=True)
            for name in (
                "daemon.log",
                "ready.json",
                "done.json",
                "launch.sh",
                "launch.stdout",
                "launch.stderr",
                "launch.status.json",
                "marker.png",
            ):
                source = state / name
                if source.is_file():
                    shutil.copy2(source, debug / name)
        output = args.output_dir / f"{args.terminal}-retention.json"
        output.write_text(
            json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        print(f"Guarded retention result: {output}")
        shutil.rmtree(state, ignore_errors=True)
    return 0 if report.get("valid") else 1


if __name__ == "__main__":
    raise SystemExit(main())
