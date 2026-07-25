#!/usr/bin/env python3
"""Run one guarded Phase 5 image scale/pane closure case."""

from __future__ import annotations

import argparse
import base64
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import subprocess
import sys
import time
from typing import Any, Callable

ROOT = Path(__file__).resolve().parents[2]
GUARD_PATH = ROOT / "tools/foot-oracle/run-final-buffer-comparison.py"
APP_ID = "com.oldjobobo.splinterm"
WORKSPACE = 8

CASES = {
    "kitty-single-scaled": {
        "axis": None,
        "payloads": ["kitty-red"],
        "minimum_images": 1,
    },
    "sixel-single-scaled": {
        "axis": None,
        "payloads": ["sixel-red"],
        "minimum_images": 1,
    },
    "kitty-horizontal-panes": {
        "axis": "horizontal",
        "payloads": ["kitty-red", "kitty-green"],
        "minimum_images": 2,
    },
    "kitty-vertical-panes": {
        "axis": "vertical",
        "payloads": ["kitty-red", "kitty-green"],
        "minimum_images": 2,
    },
}


def load_guard():
    spec = importlib.util.spec_from_file_location("phase5_graphical_guard", GUARD_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
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
        time.sleep(0.02)
    raise RuntimeError(message)


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def read_ppm(path: Path) -> tuple[int, int, bytes]:
    header, dimensions, maximum, pixels = path.read_bytes().split(b"\n", 3)
    if header != b"P6" or maximum != b"255":
        raise RuntimeError("unexpected PPM header")
    width, height = (int(value) for value in dimensions.split())
    if len(pixels) != width * height * 3:
        raise RuntimeError("incomplete PPM payload")
    return width, height, pixels


def protocol_payload(name: str) -> bytes:
    if name.startswith("kitty-"):
        color = {
            "kitty-red": bytes((255, 0, 0, 255)),
            "kitty-green": bytes((0, 255, 0, 255)),
        }[name]
        encoded = base64.b64encode(color)
        return b"\x1b_Ga=T,f=32,s=1,v=1,c=16,r=8;" + encoded + b"\x1b\\"
    palette = b"#1;2;100;0;0#1" if name == "sixel-red" else b"#2;2;0;100;0#2"
    return (
        b'\x1bP7;0;0q"1;1;10;12' + palette + b"!10~-!10~\x1b\\"
    )


def child_command(trigger: Path, payload_name: str) -> list[str]:
    payload = protocol_payload(payload_name)
    code = (
        "import os,time\n"
        f"trigger={str(trigger)!r}\n"
        "deadline=time.monotonic()+30\n"
        "while not os.path.exists(trigger):\n"
        "    if time.monotonic()>=deadline: raise SystemExit('trigger timeout')\n"
        "    time.sleep(0.01)\n"
        "os.write(1,b'\\x1b[2J\\x1b[H\\x1b[?25l')\n"
        f"os.write(1,bytes.fromhex({payload.hex()!r}))\n"
        "time.sleep(30)\n"
    )
    return [sys.executable, "-c", code]


def process_metrics(pid: int) -> dict[str, int]:
    status = (Path("/proc") / str(pid) / "status").read_text(encoding="utf-8")
    stat = (Path("/proc") / str(pid) / "stat").read_text(encoding="utf-8")
    fields = stat[stat.rfind(")") + 2 :].split()
    values = {"rss_bytes": 0, "pss_bytes": 0, "cpu_ticks": int(fields[11]) + int(fields[12]), "context_switches": 0}
    for line in status.splitlines():
        if line.startswith("VmRSS:"):
            values["rss_bytes"] = int(line.split()[1]) * 1024
        elif line.startswith(("voluntary_ctxt_switches:", "nonvoluntary_ctxt_switches:")):
            values["context_switches"] += int(line.split()[1])
    rollup = Path("/proc") / str(pid) / "smaps_rollup"
    try:
        for line in rollup.read_text(encoding="utf-8").splitlines():
            if line.startswith("Pss:"):
                values["pss_bytes"] = int(line.split()[1]) * 1024
                break
    except OSError:
        pass
    shm_bytes = 0
    try:
        for line in (Path("/proc") / str(pid) / "maps").read_text(encoding="utf-8").splitlines():
            parts = line.split(maxsplit=5)
            name = parts[5] if len(parts) == 6 else ""
            if "/dev/shm" in name or "memfd:" in name:
                start, end = (int(value, 16) for value in parts[0].split("-"))
                shm_bytes += end - start
    except OSError:
        pass
    values["shm_mapping_bytes"] = shm_bytes
    return values


def metrics_delta(before: dict[str, int], after: dict[str, int]) -> dict[str, int]:
    return {
        "cpu_ticks": max(0, after["cpu_ticks"] - before["cpu_ticks"]),
        "context_switches": max(0, after["context_switches"] - before["context_switches"]),
    }


def terminal_geometry_ready(value: Any) -> bool:
    if isinstance(value, dict):
        columns = value.get("columns")
        rows = value.get("rows")
        if (
            isinstance(columns, int)
            and columns > 0
            and isinstance(rows, list)
            and len(rows) > 0
        ):
            return True
        return any(terminal_geometry_ready(child) for child in value.values())
    if isinstance(value, list):
        return any(terminal_geometry_ready(child) for child in value)
    return False


def color_bounds(pixels: bytes, width: int, color: str) -> tuple[int, tuple[int, int, int, int] | None]:
    points: list[tuple[int, int]] = []
    for index in range(0, len(pixels), 3):
        red, green, blue = pixels[index : index + 3]
        matches = red > 180 and green < 80 and blue < 80 if color == "red" else green > 180 and red < 80 and blue < 80
        if matches:
            pixel = index // 3
            points.append((pixel % width, pixel // width))
    if not points:
        return 0, None
    xs = [point[0] for point in points]
    ys = [point[1] for point in points]
    return len(points), (min(xs), min(ys), max(xs) + 1, max(ys) + 1)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output_dir", type=Path)
    parser.add_argument("--case", choices=tuple(CASES), required=True)
    parser.add_argument("--reuse-build", action="store_true")
    parser.add_argument("--idle-seconds", type=float, default=2.0)
    args = parser.parse_args()
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        parser.error("a running Hyprland session is required")
    if args.idle_seconds <= 0:
        parser.error("idle duration must be positive")

    case = CASES[args.case]
    case_dir = args.output_dir.resolve() / args.case
    if case_dir.exists():
        parser.error(f"refusing to overwrite existing evidence: {case_dir}")
    monitor_id = V1.assert_test_workspace_isolated()
    V1.assert_user_workspace_untouched()
    active_before = V1.hyprland_json("activeworkspace")
    active_window_before = V1.hyprland_json("activewindow")
    pointer_before = V1.hyprland_json("cursorpos")

    if not args.reuse_build:
        run(["cargo", "build", "--release", "-q", "-p", "splinterd", "-p", "splinterm", "-p", "splinterm-pty"], cwd=ROOT, check=True)
    releases = [ROOT / "target/release/splinterd", ROOT / "target/release/splinterm", ROOT / "target/release/splinterm-pty-child"]
    if not all(path.is_file() for path in releases):
        parser.error("release Splinterm suite is incomplete")

    case_dir.mkdir(parents=True)
    private = Path("/tmp") / f"splinterm-phase5-{args.case}-{os.getpid()}"
    shutil.rmtree(private, ignore_errors=True)
    private.mkdir(mode=0o700)
    binaries = [private / path.name for path in releases]
    for source, target in zip(releases, binaries, strict=True):
        shutil.copy2(source, target)
        if sha256(source) != sha256(target):
            parser.error("private binary copy hash mismatch")
    daemon_binary, client_binary, _ = binaries
    runtime = private / "runtime"
    state = private / "state"
    config = private / "config/splinterm"
    runtime.mkdir(mode=0o700)
    state.mkdir(mode=0o700)
    config.mkdir(parents=True)
    (config / "config.ini").write_text(
        "[main]\nfont=JetBrains Mono Nerd Font:style=Regular\nfont-pixelsize=12\n"
        "padding-left=12\npadding-right=12\npadding-top=12\npadding-bottom=12\n"
        "[multiplexer]\ndivider-style=line\n",
        encoding="utf-8",
    )
    capture = case_dir / "capture.ppm"
    socket = runtime / "splinterd.sock"
    environment = os.environ.copy()
    environment.update(
        SPLINTERM_SOCKET=str(socket),
        SPLINTERM_ENABLE_DEV_ATTACH="1",
        XDG_STATE_HOME=str(state),
        XDG_CONFIG_HOME=str(private / "config"),
        SPLINTERM_PANE_CHROME_CAPTURE=str(capture),
        SPLINTERM_CAPTURE_MIN_IMAGES=str(case["minimum_images"]),
        SPLINTERM_IMAGE_TRACE="1",
    )
    daemon_log = (case_dir / "daemon.log").open("w", encoding="utf-8")
    daemon = subprocess.Popen([str(daemon_binary)], env=environment, stdin=subprocess.DEVNULL, stdout=daemon_log, stderr=subprocess.STDOUT, start_new_session=True, text=True)
    addresses: set[str] = set()
    splint_ids: list[str] = []
    window_id: str | None = None
    report: dict[str, Any] = {"schema": "splinterm.phase5.graphical-closure.v1", "case": args.case, "valid": False, "error": None}
    workspace_never_active = True
    window_never_active = True
    placement_preserved = True

    def client(*arguments: str) -> subprocess.CompletedProcess[str]:
        return run([str(client_binary), *arguments], env=environment, capture_output=True, timeout=10)

    def checked_client(*arguments: str) -> str:
        completed = client(*arguments)
        if completed.returncode:
            raise RuntimeError(completed.stderr.strip() or f"client {' '.join(arguments)} failed")
        return completed.stdout

    try:
        wait_until(lambda: socket.exists() and client("ping").returncode == 0, 5, "daemon not ready")
        triggers = [private / f"trigger-{index}" for index in range(len(case["payloads"]))]
        checked_client("new", f"phase5-{args.case}", "--", *child_command(triggers[0], case["payloads"][0]))
        listing = checked_client("list")
        dojo_match = re.search(rf"^([0-9a-f-]{{36}})  phase5-{re.escape(args.case)} ", listing, re.MULTILINE)
        initial_splints = re.findall(r"^  ([0-9a-f-]{36})  ", listing, re.MULTILINE)
        windows = re.findall(r"^  window ([0-9a-f-]{36})  ", listing, re.MULTILINE)
        if dojo_match is None or len(initial_splints) != 1 or len(windows) != 1:
            raise RuntimeError(f"unexpected initial topology:\n{listing}")
        window_id = windows[0]
        if case["axis"] is not None:
            checked_client("split", initial_splints[0], "--axis", case["axis"], "--side", "second", "--", *child_command(triggers[1], case["payloads"][1]))
        listing = checked_client("list")
        splint_ids = re.findall(r"^  ([0-9a-f-]{36})  ", listing, re.MULTILINE)
        if len(splint_ids) != len(case["payloads"]):
            raise RuntimeError(f"unexpected final topology:\n{listing}")

        launcher = case_dir / "launch.sh"
        selected = {key: environment[key] for key in ("SPLINTERM_SOCKET", "SPLINTERM_ENABLE_DEV_ATTACH", "XDG_STATE_HOME", "XDG_CONFIG_HOME", "SPLINTERM_PANE_CHROME_CAPTURE", "SPLINTERM_CAPTURE_MIN_IMAGES", "SPLINTERM_IMAGE_TRACE")}
        command = ["env", *[f"{key}={value}" for key, value in selected.items()], str(client_binary), "window", "--dojo-id", dojo_match.group(1), "--window-id", window_id]
        launcher.write_text("#!/bin/sh\nexec " + shlex.join(command) + f" >{shlex.quote(str(case_dir / 'client.stdout'))} 2>{shlex.quote(str(case_dir / 'client.stderr'))}\n", encoding="utf-8")
        launcher.chmod(0o700)
        existing = {item["address"] for item in V1.all_clients()}
        expression = f"hl.exec_cmd({json.dumps(str(launcher))}, {{ workspace = '8 silent', float = true, size = '960 600', no_initial_focus = true }})"
        dispatched = run(["hyprctl", "eval", expression], capture_output=True, timeout=5)
        if dispatched.returncode:
            raise RuntimeError(dispatched.stderr.strip() or dispatched.stdout.strip())
        window = wait_until(lambda: next((item for item in V1.all_clients() if item.get("class") == APP_ID and item.get("address") not in existing), None), 8, "closure window did not map")
        addresses.add(window["address"])
        if window["workspace"]["id"] != WORKSPACE or window["monitor"] != monitor_id:
            raise RuntimeError("closure window escaped workspace 8 / DP-2")

        def guarded_safe() -> bool:
            nonlocal workspace_never_active, window_never_active, placement_preserved
            current = next((item for item in V1.all_clients() if item.get("address") == window["address"]), None)
            if current is None:
                raise RuntimeError("closure window closed early")
            if current["workspace"]["id"] != WORKSPACE or current["monitor"] != monitor_id:
                placement_preserved = False
                raise RuntimeError("closure window moved")
            if V1.hyprland_json("activeworkspace").get("id") == WORKSPACE:
                workspace_never_active = False
                raise RuntimeError("reserved workspace became active")
            if V1.hyprland_json("activewindow").get("address") == window["address"]:
                window_never_active = False
                raise RuntimeError("closure window received focus")
            V1.assert_user_workspace_untouched()
            return True

        settle_deadline = time.monotonic() + 0.5
        while time.monotonic() < settle_deadline:
            guarded_safe()
            time.sleep(0.02)

        def every_pane_has_geometry() -> bool:
            guarded_safe()
            for splint_id in splint_ids:
                completed = client("snapshot", splint_id, "--output", "json")
                if completed.returncode or not terminal_geometry_ready(json.loads(completed.stdout)):
                    return False
            return True

        wait_until(every_pane_has_geometry, 8, "pane terminal geometry did not become ready")
        triggered_ns = time.monotonic_ns()
        for trigger in triggers:
            trigger.touch()

        wait_until(lambda: guarded_safe() and capture.exists() and _complete_capture(capture), 8, "complete capture was not written")
        capture_ready_ns = time.monotonic_ns()
        width, height, pixels = read_ppm(capture)
        red_count, red_bounds = color_bounds(pixels, width, "red")
        green_count, green_bounds = color_bounds(pixels, width, "green")
        if "red" in " ".join(case["payloads"]) and red_count < 100:
            raise RuntimeError("capture lacks expected red image pixels")
        if "green" in " ".join(case["payloads"]) and green_count < 100:
            raise RuntimeError("capture lacks expected green image pixels")
        if red_bounds is not None and green_bounds is not None:
            overlap_x = max(red_bounds[0], green_bounds[0]) < min(red_bounds[2], green_bounds[2])
            overlap_y = max(red_bounds[1], green_bounds[1]) < min(red_bounds[3], green_bounds[3])
            if overlap_x and overlap_y:
                raise RuntimeError("pane image color bounds overlap")

        daemon_metrics = process_metrics(daemon.pid)
        client_metrics = process_metrics(int(window["pid"]))
        daemon_before = process_metrics(daemon.pid)
        client_before = process_metrics(int(window["pid"]))
        idle_started = time.monotonic()
        while time.monotonic() - idle_started < args.idle_seconds:
            guarded_safe()
            time.sleep(0.05)
        daemon_after = process_metrics(daemon.pid)
        client_after = process_metrics(int(window["pid"]))
        daemon_log.flush()
        decode_samples = [
            int(value)
            for value in re.findall(
                r"phase5-image-trace decode_ns=(\d+)",
                (case_dir / "daemon.log").read_text(encoding="utf-8"),
            )
        ]
        composition_samples = [
            int(value)
            for value in re.findall(
                r"phase5-image-trace composition_ns=(\d+)",
                (case_dir / "client.stderr").read_text(encoding="utf-8"),
            )
        ]
        if len(decode_samples) < len(case["payloads"]):
            raise RuntimeError("decoder timing trace is incomplete")
        if not composition_samples:
            raise RuntimeError("composition timing trace is missing")
        report.update(
            valid=True,
            protocols=sorted({name.split("-", 1)[0] for name in case["payloads"]}),
            pane_count=len(splint_ids),
            pane_axis=case["axis"],
            destination_scale={"kitty_cells": [16, 8], "sixel_source_pixels": [10, 12]},
            surface={"width": width, "height": height},
            pixels={"red_count": red_count, "red_bounds": red_bounds, "green_count": green_count, "green_bounds": green_bounds},
            latency_ns={"decode_samples": decode_samples, "composition_samples": composition_samples, "trigger_to_composed_capture": capture_ready_ns - triggered_ns},
            resources={"daemon": daemon_metrics, "client": client_metrics, "authoritative_content_bytes": sum(10 * 12 * 4 if name.startswith("sixel") else 4 for name in case["payloads"]), "client_resident_source_bytes": sum(10 * 12 * 4 if name.startswith("sixel") else 4 for name in case["payloads"]), "frame_pacing": {"applicable": False, "reason": "static-image milestone; animation deferred with Slice 7"}},
            idle={"seconds": args.idle_seconds, "daemon": metrics_delta(daemon_before, daemon_after), "client": metrics_delta(client_before, client_after)},
            binaries={"splinterd_sha256": sha256(daemon_binary), "splinterm_sha256": sha256(client_binary), "splinterm_pty_child_sha256": sha256(binaries[2])},
        )
    except Exception as caught:
        report["error"] = str(caught)
    finally:
        for address in addresses:
            V1.kill_oracle_window(address)
        try:
            wait_until(lambda: not V1.workspace_clients(WORKSPACE), 5, "closure window remained mapped")
        except Exception as caught:
            report["valid"] = False
            report["error"] = report["error"] or str(caught)
        for splint_id in splint_ids:
            client("kill", splint_id, "--yes")
        if window_id is not None:
            client("close-window", window_id)
        daemon.send_signal(signal.SIGINT)
        try:
            daemon.wait(timeout=8)
        except subprocess.TimeoutExpired:
            daemon.kill()
            daemon.wait(timeout=3)
            report["valid"] = False
            report["error"] = report["error"] or "daemon required forced cleanup"
        daemon_log.close()

    active_after = V1.hyprland_json("activeworkspace")
    active_window_after = V1.hyprland_json("activewindow")
    pointer_after = V1.hyprland_json("cursorpos")
    cleanup_verified = not V1.workspace_clients(WORKSPACE) and not socket.exists()
    report["isolation"] = {
        "workspace": WORKSPACE,
        "monitor": "DP-2",
        "no_initial_focus": True,
        "workspace_never_active": workspace_never_active,
        "window_never_active": window_never_active,
        "window_placement_preserved": placement_preserved,
        "active_workspace_unchanged": active_after == active_before,
        "active_window_unchanged": active_window_after == active_window_before,
        "pointer_unchanged": pointer_after == pointer_before,
        "user_state_changes_are_informational": True,
        "cleanup_verified": cleanup_verified,
    }
    if not cleanup_verified:
        report["valid"] = False
        report["error"] = report["error"] or "cleanup incomplete"
    if capture.exists():
        report["capture_sha256"] = sha256(capture)
    (case_dir / "report.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    shutil.rmtree(private, ignore_errors=True)
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0 if report["valid"] else 1


def _complete_capture(path: Path) -> bool:
    try:
        read_ppm(path)
        return True
    except (OSError, RuntimeError, ValueError):
        return False


if __name__ == "__main__":
    raise SystemExit(main())
