#!/usr/bin/env python3
"""Run one trigger-gated graphical output workload on reserved workspace 8."""

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

try:
    from PIL import Image
except ImportError:
    Image = None

ROOT = pathlib.Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools/benchmark"
IDLE_RUNNER = TOOLS / "run-graphical-idle.py"

sys.path.insert(0, str(TOOLS))
from metrics import ProcessMetrics, snapshot_process_forest  # noqa: E402


def load_idle_runner():
    spec = importlib.util.spec_from_file_location(
        "splinterbench_output_common", IDLE_RUNNER
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


COMMON = load_idle_runner()
V1 = COMMON.V1


def is_visible_marker_pixel(red: int, green: int, blue: int) -> bool:
    """Match the marker before or after bounded inactive-window alpha composition."""

    return (
        red <= 35
        and 175 <= green <= 245
        and 70 <= blue <= 125
        and green - red >= 140
        and green - blue >= 80
    )


def screenshot_marker(window: dict[str, Any], path: pathlib.Path) -> int:
    x, y = window["at"]
    width, height = window["size"]
    result = subprocess.run(
        ["grim", "-g", f"{x},{y} {width}x{height}", str(path)],
        text=True,
        capture_output=True,
        check=False,
        timeout=5,
    )
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or "grim screenshot failed")
    if Image is None:
        raise RuntimeError("Pillow is required for visible-marker detection")
    with Image.open(path) as image:
        rgb = image.convert("RGB")
        pixels = (
            rgb.get_flattened_data()
            if hasattr(rgb, "get_flattened_data")
            else rgb.getdata()
        )
        return sum(
            1
            for red, green, blue in pixels
            if is_visible_marker_pixel(red, green, blue)
        )


def wait_output(
    app_id: str,
    address: str,
    window: dict[str, Any],
    done_path: pathlib.Path,
    screenshot: pathlib.Path,
    trigger_ns: int,
) -> tuple[dict[str, Any], int, int]:
    done = None
    visible_ns = None
    visible_pixels = 0
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        COMMON.assert_owned_window(app_id, address)
        if done is None and done_path.exists():
            done = json.loads(done_path.read_text(encoding="utf-8"))
        if visible_ns is None:
            pixels = screenshot_marker(window, screenshot)
            if pixels >= 100:
                visible_pixels = pixels
                visible_ns = time.monotonic_ns()
        if done is not None and visible_ns is not None:
            return done, visible_ns - trigger_ns, visible_pixels
        time.sleep(0.015)
    missing = []
    if done is None:
        missing.append("child write completion")
    if visible_ns is None:
        missing.append("visible marker")
    raise RuntimeError(f"output timed out waiting for {' and '.join(missing)}")


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
        description="Run one guarded trigger-gated terminal output case"
    )
    parser.add_argument("output_dir", type=pathlib.Path)
    parser.add_argument("--terminal", choices=tuple(COMMON.APP_IDS), required=True)
    parser.add_argument("--case", choices=("plain", "ansi", "unicode"), required=True)
    parser.add_argument("--lines", type=int, default=2000)
    parser.add_argument("--columns", type=int, default=80)
    parser.add_argument("--settle-seconds", type=float, default=1.0)
    parser.add_argument("--retain-screenshot", action="store_true")
    parser.add_argument("--scrollback-lines", type=int)
    args = parser.parse_args()
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        parser.error("a running Hyprland session is required")
    if args.lines <= 0 or args.columns < 20 or args.settle_seconds < 0:
        parser.error("invalid workload dimensions or settle duration")
    if (
        args.scrollback_lines is not None
        and not 0 <= args.scrollback_lines <= 1_000_000
    ):
        parser.error("scrollback lines must be between 0 and 1,000,000")
    if shutil.which("grim") is None:
        parser.error("grim is required for visible-marker approximation")
    if Image is None:
        parser.error("Pillow is required for visible-marker approximation")

    args.output_dir.mkdir(parents=True, exist_ok=True)
    state = pathlib.Path(f"/tmp/splinterbench-output-{args.terminal}-{os.getpid()}")
    shutil.rmtree(state, ignore_errors=True)
    state.mkdir(mode=0o700)
    socket = state / "splinterd.sock"
    daemon = None
    daemon_log = None
    address = None
    screenshot = state / "visible-marker.png"
    report: dict[str, Any] = {
        "schema": "splinterm.benchmark.graphical-output.v1",
        "terminal": args.terminal,
        "launch_mode": (
            "prestarted_daemon_client_launch"
            if args.terminal == "splinterm"
            else "standalone_process_launch"
        ),
        "case": args.case,
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
                [str(COMMON.splinterd_executable())],
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
            case=args.case,
            lines=args.lines,
            columns=args.columns,
            scrollback_lines=args.scrollback_lines,
        )
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
        root_pids = [int(window["pid"])]
        if daemon is not None:
            root_pids.insert(0, daemon.pid)
        child_pid = int(ready["pid"])
        measured_roots = [*root_pids, child_pid]
        time.sleep(args.settle_seconds)
        before = snapshot_process_forest(measured_roots)
        trigger_ns = time.monotonic_ns()
        (state / "start").touch()
        done, visible_ns, marker_pixels = wait_output(
            COMMON.APP_IDS[args.terminal],
            address,
            window,
            state / "done.json",
            screenshot,
            trigger_ns,
        )
        after = snapshot_process_forest(measured_roots)
        report.update(
            workload={
                "lines": args.lines,
                "columns": args.columns,
                "payload_bytes": int(done["payload_bytes"]),
                "total_bytes": int(done["total_bytes"]),
                "child_write_duration_ns": int(done["duration_ns"]),
                "trigger_to_write_complete_ns": int(done["monotonic_ns"]) - trigger_ns,
                "trigger_to_visible_marker_ns": visible_ns,
                "visible_marker_pixels": marker_pixels,
                "visible_boundary": "screenshot_polling_approximation",
            },
            resources=resource_delta(before, after),
            processes={
                "root_pids": root_pids,
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
        if args.retain_screenshot and screenshot.exists():
            shutil.copy2(
                screenshot, args.output_dir / f"{args.terminal}-{args.case}.png"
            )
        output = args.output_dir / f"{args.terminal}-{args.case}.json"
        output.write_text(
            json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        print(f"Guarded output result: {output}")
        shutil.rmtree(state, ignore_errors=True)
    return 0 if report.get("valid") else 1


if __name__ == "__main__":
    raise SystemExit(main())
