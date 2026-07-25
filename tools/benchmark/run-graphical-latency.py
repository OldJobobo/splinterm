#!/usr/bin/env python3
"""Measure targeted input receipt and screenshot-visible latency without focusing."""

from __future__ import annotations

import argparse
import hashlib
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
IDLE_RUNNER = TOOLS / "run-graphical-idle.py"
OUTPUT_RUNNER = TOOLS / "run-graphical-output.py"


def load(path: pathlib.Path, name: str):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


COMMON = load(IDLE_RUNNER, "splinterbench_latency_idle")
OUTPUT = load(OUTPUT_RUNNER, "splinterbench_latency_output")
V1 = COMMON.V1


def focused_address() -> str | None:
    return V1.hyprland_json("activewindow").get("address")


def assert_isolated(app_id: str, address: str, original_focus: str | None) -> None:
    V1.assert_user_workspace_untouched()
    COMMON.assert_owned_window(app_id, address)
    if focused_address() != original_focus:
        raise RuntimeError("targeted input benchmark changed host focus")


def send_shortcut(address: str, key: str) -> subprocess.CompletedProcess[str]:
    selector = json.dumps(f"address:{address}")
    expression = (
        "hl.dispatch(hl.dsp.send_shortcut({ "
        f"mods = '', key = {json.dumps(key)}, window = {selector} "
        "}))"
    )
    return V1.run(["hyprctl", "eval", expression], capture_output=True, timeout=5)


def wait_result(
    app_id: str,
    address: str,
    window: dict[str, Any],
    original_focus: str | None,
    state: pathlib.Path,
    trigger_ns: int,
) -> tuple[dict[str, Any], dict[str, Any], int, int]:
    received = None
    done = None
    visible_ns = None
    visible_pixels = 0
    screenshot = state / "visible-marker.png"
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        assert_isolated(app_id, address, original_focus)
        if received is None and (state / "input-received.json").exists():
            received = json.loads((state / "input-received.json").read_text(encoding="utf-8"))
        if done is None and (state / "done.json").exists():
            done = json.loads((state / "done.json").read_text(encoding="utf-8"))
        if visible_ns is None:
            pixels = OUTPUT.screenshot_marker(window, screenshot)
            if pixels >= 100:
                visible_pixels = pixels
                visible_ns = time.monotonic_ns()
        if received is not None and done is not None and visible_ns is not None:
            return received, done, visible_ns, visible_pixels
        time.sleep(0.01)
    raise RuntimeError("input latency timed out before child receipt and visible marker")


def file_sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def version_line(executable: pathlib.Path) -> str:
    result = subprocess.run(
        [str(executable), "--version"], text=True, capture_output=True,
        check=False, timeout=5,
    )
    lines = (result.stdout + result.stderr).splitlines()
    return lines[0] if lines else "version unavailable"


def main() -> int:
    parser = argparse.ArgumentParser(description="Run one guarded targeted-input latency case")
    parser.add_argument("output_dir", type=pathlib.Path)
    parser.add_argument("--terminal", choices=tuple(COMMON.APP_IDS), required=True)
    parser.add_argument("--settle-seconds", type=float, default=1.0)
    parser.add_argument("--retain-screenshot", action="store_true")
    args = parser.parse_args()
    if args.settle_seconds < 0:
        parser.error("settle duration must be non-negative")
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        parser.error("a running Hyprland session is required")
    if shutil.which("grim") is None or OUTPUT.Image is None:
        parser.error("grim and Pillow are required")

    args.output_dir.mkdir(parents=True, exist_ok=True)
    state = pathlib.Path(f"/tmp/splinterbench-latency-{args.terminal}-{os.getpid()}")
    shutil.rmtree(state, ignore_errors=True)
    state.mkdir(mode=0o700)
    socket = state / "splinterd.sock"
    daemon = None
    daemon_log = None
    address = None
    original_focus = None
    report: dict[str, Any] = {
        "schema": "splinterm.benchmark.input-latency.v1",
        "terminal": args.terminal,
        "valid": False,
        "notes": [],
    }
    try:
        V1.assert_test_workspace_isolated()
        V1.assert_user_workspace_untouched()
        original_focus = focused_address()
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
                [str(COMMON.splinterd_executable())], env=environment,
                stdin=subprocess.DEVNULL, stdout=daemon_log,
                stderr=subprocess.STDOUT, start_new_session=True, text=True,
            )
            COMMON.wait_socket(socket, daemon)

        command, environment = COMMON.launch_command(
            args.terminal, state, socket, 30, case="input", columns=80
        )
        launcher = state / "launch.sh"
        COMMON.write_launcher(launcher, command, environment)
        existing = {item["address"] for item in V1.all_clients()}
        launched_ns = time.monotonic_ns()
        COMMON.dispatch_launcher(launcher)
        window, _, _, _ = COMMON.wait_launch(
            COMMON.APP_IDS[args.terminal], existing, state / "ready.json", launched_ns
        )
        address = str(window["address"])
        assert_isolated(COMMON.APP_IDS[args.terminal], address, original_focus)
        time.sleep(args.settle_seconds)
        assert_isolated(COMMON.APP_IDS[args.terminal], address, original_focus)

        (state / "start").touch()
        trigger_ns = time.monotonic_ns()
        for key in ("x", "Return"):
            injected = send_shortcut(address, key)
            if injected.returncode:
                raise RuntimeError(injected.stderr.strip() or injected.stdout.strip() or f"targeted {key} injection failed")
        received, _, visible_ns, marker_pixels = wait_result(
            COMMON.APP_IDS[args.terminal], address, window, original_focus, state, trigger_ns
        )
        received_ns = int(received["monotonic_ns"])
        if received_ns < trigger_ns or visible_ns < received_ns:
            raise RuntimeError("monotonic input latency ordering is invalid")
        hyprland = pathlib.Path(shutil.which("Hyprland") or "/usr/bin/Hyprland")
        report.update(
            boundary={
                "backend": "host-hyprland-targeted-shortcut",
                "components": [{
                    "name": "Hyprland",
                    "version": version_line(hyprland),
                    "sha256": file_sha256(hyprland),
                }],
                "width": 960, "height": 600, "refresh_hz": 60, "scale": 1,
                "input_protocol": "Hyprland hl.dsp.send_shortcut targeted window",
                "capture_protocol": "zwlr_screencopy_manager_v1 via grim",
                "targeted_window_verified": True,
            },
            input={
                "token": "x",
                "clock": "CLOCK_MONOTONIC shared host namespace",
                "injected_monotonic_ns": trigger_ns,
                "child_received_monotonic_ns": received_ns,
                "input_to_child_ns": received_ns - trigger_ns,
                "injector_returncode": 0,
            },
            visible={
                "boundary": "host_window_screenshot_polling_approximation",
                "detected_monotonic_ns": visible_ns,
                "input_to_visible_marker_ns": visible_ns - trigger_ns,
                "marker_pixels": marker_pixels,
                "poll_interval_ms": 10,
            },
            presentation={
                "status": "not-measured",
                "input_to_compositor_presentation_ns": None,
            },
            isolation={
                "workspace": 8, "monitor": "DP-2",
                "no_initial_focus": True,
                "targeted_input_without_focus": True,
                "host_focus_unchanged": True,
                "host_workspace_unchanged": True,
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
            V1.assert_user_workspace_untouched()
            if focused_address() != original_focus:
                raise RuntimeError("cleanup did not preserve original focus")
            if "isolation" in report:
                report["isolation"]["cleanup_verified"] = True
        except Exception as error:
            report["valid"] = False
            report["notes"].append(f"cleanup: {error}")
        screenshot = state / "visible-marker.png"
        if args.retain_screenshot and screenshot.exists():
            shutil.copy2(screenshot, args.output_dir / f"{args.terminal}-input.png")
        if not report.get("valid"):
            for diagnostic in ("daemon.log", "launch.stdout", "launch.stderr"):
                source = state / diagnostic
                if source.exists():
                    shutil.copy2(
                        source,
                        args.output_dir / f"{args.terminal}-{diagnostic}",
                    )
        output = args.output_dir / f"{args.terminal}-input.json"
        output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        print(f"Targeted input-latency result: {output}")
        shutil.rmtree(state, ignore_errors=True)
    return 0 if report.get("valid") else 1


if __name__ == "__main__":
    raise SystemExit(main())
