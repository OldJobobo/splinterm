#!/usr/bin/env python3
"""Run guarded real-client terminal image compatibility and benchmark cases."""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import re
import shlex
import shutil
import signal
import statistics
import subprocess
import sys
import time
from collections.abc import Callable
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
IMAGE_SPIKE = Path(__file__).resolve().parent
FIXTURES = IMAGE_SPIKE / "fixtures"
FIXTURE_IMAGE = FIXTURES / "real-client-static.png"
PRESENTATION = FIXTURES / "real-client-presenterm.md"
SHARED_RUNNER_PATH = IMAGE_SPIKE / "run_phase5_graphical.py"
APP_ID = "com.oldjobobo.splinterm"
WORKSPACE = 8
SCHEMA = "splinterm.real-client-images.v1"


def load_shared_runner():
    spec = importlib.util.spec_from_file_location(
        "splinterm_phase5_graphical", SHARED_RUNNER_PATH
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


SHARED = load_shared_runner()
V1 = SHARED.V1


def case_specs() -> dict[str, dict[str, Any]]:
    """Return the fixed, deliberately small real-client case catalogue."""

    geometry = "40x20"
    return {
        "icat-kitty-smoke": {
            "app": "kitten",
            "protocol": "kitty",
            "kind": "smoke",
            "command": [
                "kitten",
                "icat",
                "--transfer-mode=stream",
                "--use-window-size=80,24,800,480",
                "--stdin=no",
                "--no-trailing-newline",
                "--align=left",
                "--scale-up=no",
                str(FIXTURE_IMAGE),
            ],
        },
        "chafa-sixel": {
            "app": "chafa",
            "protocol": "sixel",
            "kind": "compatibility",
            "command": [
                "chafa",
                "--probe",
                "off",
                "-f",
                "sixels",
                "--animate",
                "off",
                "--view-size",
                "80x24",
                "--size",
                geometry,
                str(FIXTURE_IMAGE),
            ],
        },
        "timg-iterm2": {
            "app": "timg",
            "protocol": "iterm2",
            "kind": "compatibility",
            "command": [
                "timg",
                "-p",
                "i",
                "-g",
                geometry,
                "--frames=1",
                str(FIXTURE_IMAGE),
            ],
        },
        "presenterm-sixel": {
            "app": "presenterm",
            "protocol": "sixel",
            "kind": "compatibility",
            "command": [
                "presenterm",
                "--image-protocol",
                "sixel",
                str(PRESENTATION),
            ],
        },
        "yazi-auto": {
            "app": "yazi",
            "protocol": "auto",
            "kind": "compatibility",
            "command": ["yazi", str(FIXTURE_IMAGE)],
        },
        "timg-kitty": {
            "app": "timg",
            "protocol": "kitty",
            "kind": "benchmark",
            "command": [
                "timg",
                "-p",
                "k",
                "-g",
                geometry,
                "--frames=1",
                str(FIXTURE_IMAGE),
            ],
        },
        "timg-sixel": {
            "app": "timg",
            "protocol": "sixel",
            "kind": "benchmark",
            "command": [
                "timg",
                "-p",
                "s",
                "-g",
                geometry,
                "--frames=1",
                str(FIXTURE_IMAGE),
            ],
        },
        "timg-iterm2-benchmark": {
            "app": "timg",
            "protocol": "iterm2",
            "kind": "benchmark",
            "command": [
                "timg",
                "-p",
                "i",
                "-g",
                geometry,
                "--frames=1",
                str(FIXTURE_IMAGE),
            ],
        },
    }


COMPATIBILITY_CASES = (
    "chafa-sixel",
    "timg-iterm2",
    "presenterm-sixel",
    "yazi-auto",
)
BENCHMARK_CASES = ("timg-kitty", "timg-sixel", "timg-iterm2-benchmark")
VERSION_ARGUMENTS = {
    "kitten": ["--version"],
    "chafa": ["--version"],
    "timg": ["--version"],
    "presenterm": ["--version"],
    "yazi": ["--version"],
}


def run(command: list[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
    check = kwargs.pop("check", False)
    return subprocess.run(command, text=True, check=check, **kwargs)


def wait_until(predicate: Callable[[], Any], seconds: float, message: str) -> Any:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        V1.assert_user_workspace_untouched()
        time.sleep(0.02)
    raise RuntimeError(message)


def resolved_command(spec: dict[str, Any]) -> list[str]:
    command = list(spec["command"])
    executable = shutil.which(command[0])
    if executable is None:
        raise RuntimeError(f"required application is not installed: {command[0]}")
    command[0] = executable
    return command


def application_versions(specs: dict[str, dict[str, Any]]) -> dict[str, dict[str, str]]:
    versions: dict[str, dict[str, str]] = {}
    for app in sorted({str(spec["app"]) for spec in specs.values()}):
        executable = shutil.which(app)
        if executable is None:
            versions[app] = {"path": "", "version": "missing"}
            continue
        result = run(
            [executable, *VERSION_ARGUMENTS[app]],
            capture_output=True,
            timeout=5,
        )
        output = (result.stdout or result.stderr).strip().splitlines()
        versions[app] = {
            "path": str(Path(executable).resolve()),
            "version": output[0] if output else f"exit {result.returncode}",
        }
    return versions


def build_plan(mode: str, samples: int = 3) -> list[dict[str, Any]]:
    specs = case_specs()
    if mode == "smoke":
        return [{"case": "icat-kitty-smoke", "sample": 1, "warmup": False}]
    if mode == "compatibility":
        return [
            {"case": name, "sample": 1, "warmup": False} for name in COMPATIBILITY_CASES
        ]
    if mode == "benchmark":
        plan: list[dict[str, Any]] = []
        for name in BENCHMARK_CASES:
            plan.append({"case": name, "sample": 0, "warmup": True})
            plan.extend(
                {"case": name, "sample": sample, "warmup": False}
                for sample in range(1, samples + 1)
            )
        return plan
    if mode == "all":
        return (
            build_plan("smoke", samples)
            + build_plan("compatibility", samples)
            + build_plan("benchmark", samples)
        )
    if mode in specs:
        return [{"case": mode, "sample": 1, "warmup": False}]
    raise ValueError(f"unknown mode or case: {mode}")


def child_command(
    trigger: Path, status: Path, command: list[str], working_directory: Path
) -> list[str]:
    """Build a shell-free child that waits for geometry before launching the app."""

    code = (
        "import json,os,pathlib,subprocess,time\n"
        f"trigger=pathlib.Path({str(trigger)!r})\n"
        f"status=pathlib.Path({str(status)!r})\n"
        f"command={command!r}\n"
        f"cwd={str(working_directory)!r}\n"
        "deadline=time.monotonic()+30\n"
        "while not trigger.exists():\n"
        "    if time.monotonic()>=deadline: raise SystemExit('trigger timeout')\n"
        "    time.sleep(0.01)\n"
        "os.write(1,b'\\x1b[2J\\x1b[H\\x1b[?25l')\n"
        "started=time.monotonic_ns()\n"
        "process=subprocess.Popen(command,cwd=cwd)\n"
        "returncode=process.wait()\n"
        "status.write_text(json.dumps({'returncode':returncode,'runtime_ns':time.monotonic_ns()-started})+'\\n')\n"
        "time.sleep(30)\n"
    )
    return [sys.executable, "-c", code]


def fixture_dimensions(path: Path = FIXTURE_IMAGE) -> tuple[int, int]:
    data = path.read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n" or data[12:16] != b"IHDR":
        raise ValueError("fixture is not a PNG with an IHDR header")
    return int.from_bytes(data[16:20], "big"), int.from_bytes(data[20:24], "big")


def color_summary(pixels: bytes) -> dict[str, int]:
    counts = {"red": 0, "green": 0, "blue": 0, "light": 0}
    for index in range(0, len(pixels), 3):
        red, green, blue = pixels[index : index + 3]
        if red > 170 and red > green * 3 // 2 and red > blue * 3 // 2:
            counts["red"] += 1
        if green > 150 and green > red * 3 // 2 and green > blue * 3 // 2:
            counts["green"] += 1
        if blue > 150 and blue > red * 3 // 2 and blue > green * 3 // 2:
            counts["blue"] += 1
        if min(red, green, blue) > 190:
            counts["light"] += 1
    return counts


def parse_trace(log: str, client_log: str) -> dict[str, Any]:
    decode = [int(value) for value in re.findall(r"decode_ns=(\d+)", log)]
    composition = [
        int(value) for value in re.findall(r"composition_ns=(\d+)", client_log)
    ]
    content = [
        {
            "bytes": int(size),
            "contents": int(contents),
            "placements": int(placements),
        }
        for size, contents, placements in re.findall(
            r"content_bytes=(\d+) content_count=(\d+) placement_count=(\d+)", log
        )
    ]
    return {
        "decode_ns": decode,
        "composition_ns": composition,
        "content": content,
        "image_rejected": "ImageRejected" in log or "image rejected" in log.lower(),
    }


def ensure_release_build(reuse_build: bool) -> list[Path]:
    if not reuse_build:
        run(
            [
                "cargo",
                "build",
                "--release",
                "-q",
                "-p",
                "splinterd",
                "-p",
                "splinterm",
                "-p",
                "splinterm-pty",
            ],
            cwd=ROOT,
            check=True,
        )
    releases = [
        ROOT / "target/release/splinterd",
        ROOT / "target/release/splinterm",
        ROOT / "target/release/splinterm-pty-child",
    ]
    if not all(path.is_file() for path in releases):
        raise RuntimeError("release Splinterm suite is incomplete")
    return releases


def run_case(
    output_dir: Path,
    case_name: str,
    run_name: str,
    releases: list[Path],
    idle_seconds: float,
) -> dict[str, Any]:
    specs = case_specs()
    spec = specs[case_name]
    command = resolved_command(spec)
    case_dir = output_dir.resolve() / run_name
    if case_dir.exists():
        raise RuntimeError(f"refusing to overwrite existing evidence: {case_dir}")

    monitor_id = V1.assert_test_workspace_isolated()
    V1.assert_user_workspace_untouched()
    active_before = V1.hyprland_json("activeworkspace")
    active_window_before = V1.hyprland_json("activewindow")
    pointer_before = V1.hyprland_json("cursorpos")

    case_dir.mkdir(parents=True)
    private = Path("/tmp") / f"splinterm-real-images-{run_name}-{os.getpid()}"
    shutil.rmtree(private, ignore_errors=True)
    private.mkdir(mode=0o700)
    binaries = [private / path.name for path in releases]
    for source, target in zip(releases, binaries, strict=True):
        shutil.copy2(source, target)
        if SHARED.sha256(source) != SHARED.sha256(target):
            raise RuntimeError("private binary copy hash mismatch")
    daemon_binary, client_binary, _ = binaries

    runtime = private / "runtime"
    state = private / "state"
    config = private / "config/splinterm"
    runtime.mkdir(mode=0o700)
    state.mkdir(mode=0o700)
    config.mkdir(parents=True)
    (config / "config.ini").write_text(
        "[main]\nfont=JetBrains Mono Nerd Font:style=Regular\nfont-pixelsize=12\n"
        "padding-left=12\npadding-right=12\npadding-top=12\npadding-bottom=12\n",
        encoding="utf-8",
    )

    capture = case_dir / "capture.ppm"
    trigger = private / "trigger"
    app_status = case_dir / "app-status.json"
    socket = runtime / "splinterd.sock"
    environment = os.environ.copy()
    environment.update(
        SPLINTERM_SOCKET=str(socket),
        SPLINTERM_ENABLE_DEV_ATTACH="1",
        XDG_STATE_HOME=str(state),
        XDG_CONFIG_HOME=str(private / "config"),
        SPLINTERM_PANE_CHROME_CAPTURE=str(capture),
        SPLINTERM_CAPTURE_MIN_IMAGES="1",
        SPLINTERM_IMAGE_TRACE="1",
    )
    (case_dir / "app-command.txt").write_text(
        shlex.join(command) + "\n", encoding="utf-8"
    )

    daemon_log = (case_dir / "daemon.log").open("w", encoding="utf-8")
    daemon = subprocess.Popen(
        [str(daemon_binary)],
        env=environment,
        stdin=subprocess.DEVNULL,
        stdout=daemon_log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
        text=True,
    )
    addresses: set[str] = set()
    splint_ids: list[str] = []
    dojo_id: str | None = None
    lair_id: str | None = None
    workspace_never_active = True
    window_never_active = True
    placement_preserved = True
    report: dict[str, Any] = {
        "schema": SCHEMA,
        "case": case_name,
        "run": run_name,
        "app": spec["app"],
        "expected_protocol": spec["protocol"],
        "kind": spec["kind"],
        "command": command,
        "valid": False,
        "error": None,
    }

    def client(*arguments: str) -> subprocess.CompletedProcess[str]:
        return run(
            [str(client_binary), *arguments],
            env=environment,
            capture_output=True,
            timeout=10,
        )

    def checked_client(*arguments: str) -> str:
        completed = client(*arguments)
        if completed.returncode:
            raise RuntimeError(
                completed.stderr.strip() or f"client {' '.join(arguments)} failed"
            )
        return completed.stdout

    try:
        wait_until(
            lambda: socket.exists() and client("ping").returncode == 0,
            5,
            "daemon not ready",
        )
        checked_client(
            "new",
            f"real-images-{run_name}",
            "--",
            *child_command(trigger, app_status, command, FIXTURES),
        )
        listing = checked_client("list", "--all")
        lair_match = re.search(
            rf"^([0-9a-f-]{{36}})  real-images-{re.escape(run_name)} ",
            listing,
            re.MULTILINE,
        )
        splint_ids = re.findall(r"^  ([0-9a-f-]{36})  ", listing, re.MULTILINE)
        dojos = re.findall(r"^  Dojo ([0-9a-f-]{36})  ", listing, re.MULTILINE)
        if lair_match is None or len(splint_ids) != 1 or len(dojos) != 1:
            raise RuntimeError(f"unexpected topology:\n{listing}")
        lair_id = lair_match.group(1)
        dojo_id = dojos[0]

        launcher = case_dir / "launch.sh"
        selected_keys = (
            "SPLINTERM_SOCKET",
            "SPLINTERM_ENABLE_DEV_ATTACH",
            "XDG_STATE_HOME",
            "XDG_CONFIG_HOME",
            "SPLINTERM_PANE_CHROME_CAPTURE",
            "SPLINTERM_CAPTURE_MIN_IMAGES",
            "SPLINTERM_IMAGE_TRACE",
        )
        selected = {key: environment[key] for key in selected_keys}
        window_command = [
            "env",
            *[f"{key}={value}" for key, value in selected.items()],
            str(client_binary),
            "window",
            "--lair-id",
            lair_id,
            "--dojo-id",
            dojo_id,
        ]
        launcher.write_text(
            "#!/bin/sh\nexec "
            + shlex.join(window_command)
            + f" >{shlex.quote(str(case_dir / 'client.stdout'))}"
            + f" 2>{shlex.quote(str(case_dir / 'client.stderr'))}\n",
            encoding="utf-8",
        )
        launcher.chmod(0o700)

        existing = {item["address"] for item in V1.all_clients()}
        expression = (
            f"hl.exec_cmd({json.dumps(str(launcher))}, "
            "{ workspace = '8 silent', float = true, size = '960 600', "
            "opacity = '1 1', no_initial_focus = true, no_focus = true })"
        )
        dispatched = run(
            ["hyprctl", "eval", expression], capture_output=True, timeout=5
        )
        if dispatched.returncode:
            raise RuntimeError(dispatched.stderr.strip() or dispatched.stdout.strip())
        window = wait_until(
            lambda: next(
                (
                    item
                    for item in V1.all_clients()
                    if item.get("class") == APP_ID
                    and item.get("address") not in existing
                ),
                None,
            ),
            8,
            "real-client image window did not map",
        )
        addresses.add(window["address"])
        if window["workspace"]["id"] != WORKSPACE or window["monitor"] != monitor_id:
            raise RuntimeError("real-client image window escaped workspace 8 / DP-2")

        def guarded_safe() -> bool:
            nonlocal workspace_never_active, window_never_active, placement_preserved
            current = next(
                (
                    item
                    for item in V1.all_clients()
                    if item.get("address") == window["address"]
                ),
                None,
            )
            if current is None:
                raise RuntimeError("real-client image window closed early")
            if (
                current["workspace"]["id"] != WORKSPACE
                or current["monitor"] != monitor_id
            ):
                placement_preserved = False
                raise RuntimeError("real-client image window moved")
            if V1.hyprland_json("activeworkspace").get("id") == WORKSPACE:
                workspace_never_active = False
                raise RuntimeError("reserved workspace became active")
            if V1.hyprland_json("activewindow").get("address") == window["address"]:
                window_never_active = False
                raise RuntimeError("real-client image window received focus")
            V1.assert_user_workspace_untouched()
            return True

        settle_deadline = time.monotonic() + 0.5
        while time.monotonic() < settle_deadline:
            guarded_safe()
            time.sleep(0.02)

        def geometry_ready() -> bool:
            guarded_safe()
            completed = client("snapshot", splint_ids[0], "--output", "json")
            return completed.returncode == 0 and SHARED.terminal_geometry_ready(
                json.loads(completed.stdout)
            )

        wait_until(geometry_ready, 8, "terminal geometry did not become ready")
        triggered_ns = time.monotonic_ns()
        trigger.touch()
        wait_until(
            lambda: (
                guarded_safe()
                and capture.exists()
                and SHARED._complete_capture(capture)
            ),
            15,
            "complete image capture was not written",
        )
        capture_ready_ns = time.monotonic_ns()
        width, height, pixels = SHARED.read_ppm(capture)
        colors = color_summary(pixels)
        if min(colors["red"], colors["green"], colors["blue"]) < 50:
            raise RuntimeError(
                f"capture lacks expected fixture color regions: {colors}"
            )

        daemon_metrics = SHARED.process_metrics(daemon.pid)
        client_metrics = SHARED.process_metrics(int(window["pid"]))
        daemon_before = SHARED.process_metrics(daemon.pid)
        client_before = SHARED.process_metrics(int(window["pid"]))
        idle_started = time.monotonic()
        while time.monotonic() - idle_started < idle_seconds:
            guarded_safe()
            time.sleep(0.05)
        daemon_after = SHARED.process_metrics(daemon.pid)
        client_after = SHARED.process_metrics(int(window["pid"]))

        daemon_log.flush()
        daemon_text = (case_dir / "daemon.log").read_text(encoding="utf-8")
        client_text = (case_dir / "client.stderr").read_text(encoding="utf-8")
        trace = parse_trace(daemon_text, client_text)
        if not trace["decode_ns"] or not trace["composition_ns"]:
            raise RuntimeError("image timing trace is incomplete")
        if trace["image_rejected"]:
            raise RuntimeError("application image was rejected")

        status = None
        if app_status.exists():
            status = json.loads(app_status.read_text(encoding="utf-8"))
            if status["returncode"] != 0:
                raise RuntimeError(
                    f"application exited with status {status['returncode']}"
                )

        report.update(
            valid=True,
            surface={"width": width, "height": height},
            colors=colors,
            latency_ns={
                "trigger_to_composed_capture": capture_ready_ns - triggered_ns,
                "decode_samples": trace["decode_ns"],
                "composition_samples": trace["composition_ns"],
            },
            image_trace={
                "content": trace["content"],
                "image_rejected": trace["image_rejected"],
            },
            application_status=status or {"state": "running_at_capture"},
            resources={"daemon": daemon_metrics, "client": client_metrics},
            idle={
                "seconds": idle_seconds,
                "daemon": SHARED.metrics_delta(daemon_before, daemon_after),
                "client": SHARED.metrics_delta(client_before, client_after),
            },
            binaries={
                "splinterd_sha256": SHARED.sha256(daemon_binary),
                "splinterm_sha256": SHARED.sha256(client_binary),
                "splinterm_pty_child_sha256": SHARED.sha256(binaries[2]),
            },
        )
    except Exception as caught:  # noqa: BLE001 - every failure must reach cleanup and the report
        report["error"] = str(caught)
    finally:

        def record_cleanup_error(caught: BaseException | str) -> None:
            report["valid"] = False
            report["error"] = report["error"] or str(caught)

        for address in addresses:
            try:
                V1.kill_oracle_window(address)
            except Exception as caught:  # noqa: BLE001 - continue remaining cleanup
                record_cleanup_error(caught)
        try:
            wait_until(
                lambda: not V1.workspace_clients(WORKSPACE),
                5,
                "real-client image window remained mapped",
            )
        except Exception as caught:  # noqa: BLE001 - cleanup failure belongs in evidence
            record_cleanup_error(caught)
        for splint_id in splint_ids:
            try:
                client("kill", splint_id, "--yes")
            except Exception as caught:  # noqa: BLE001 - continue remaining cleanup
                record_cleanup_error(caught)
        if dojo_id is not None:
            try:
                client("close-dojo", dojo_id)
            except Exception as caught:  # noqa: BLE001 - continue remaining cleanup
                record_cleanup_error(caught)
        try:
            if daemon.poll() is None:
                daemon.send_signal(signal.SIGINT)
            daemon.wait(timeout=8)
        except subprocess.TimeoutExpired:
            record_cleanup_error("daemon required forced cleanup")
            try:
                daemon.kill()
                daemon.wait(timeout=3)
            except Exception as caught:  # noqa: BLE001 - retain evidence after failure
                record_cleanup_error(caught)
        except Exception as caught:  # noqa: BLE001 - retain evidence after process race
            record_cleanup_error(caught)
        try:
            daemon_log.close()
        except OSError as caught:
            record_cleanup_error(caught)

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
        report["capture_sha256"] = SHARED.sha256(capture)
    (case_dir / "report.json").write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    shutil.rmtree(private, ignore_errors=True)
    return report


def summarize_values(values: list[int]) -> dict[str, Any]:
    if not values:
        return {
            "samples": 0,
            "median": None,
            "minimum": None,
            "maximum": None,
            "spread": None,
        }
    median = statistics.median(values)
    return {
        "samples": len(values),
        "median": int(median),
        "minimum": min(values),
        "maximum": max(values),
        "spread": (max(values) - min(values)) / median if median else 0.0,
    }


def measurement_summary(reports: list[dict[str, Any]]) -> dict[str, Any]:
    valid = [report for report in reports if report.get("valid")]
    return {
        "samples": len(valid),
        "trigger_to_composed_capture_ns": summarize_values(
            [
                int(report["latency_ns"]["trigger_to_composed_capture"])
                for report in valid
            ]
        ),
        "decode_ns": summarize_values(
            [
                int(statistics.median(report["latency_ns"]["decode_samples"]))
                for report in valid
            ]
        ),
        "composition_ns": summarize_values(
            [
                int(statistics.median(report["latency_ns"]["composition_samples"]))
                for report in valid
            ]
        ),
        "daemon_rss_bytes": summarize_values(
            [int(report["resources"]["daemon"]["rss_bytes"]) for report in valid]
        ),
        "client_rss_bytes": summarize_values(
            [int(report["resources"]["client"]["rss_bytes"]) for report in valid]
        ),
    }


def write_manifest(output_dir: Path, specs: dict[str, dict[str, Any]]) -> None:
    output_dir.mkdir(parents=True, exist_ok=True)
    manifest = {
        "schema": SCHEMA,
        "fixture": {
            "path": str(FIXTURE_IMAGE),
            "sha256": SHARED.sha256(FIXTURE_IMAGE),
            "dimensions": list(fixture_dimensions()),
        },
        "surface": {"width": 960, "height": 600},
        "terminal_geometry": {"columns": 80, "rows": 24},
        "applications": application_versions(specs),
        "cases": {
            name: {
                "app": spec["app"],
                "protocol": spec["protocol"],
                "kind": spec["kind"],
                "command": resolved_command(spec),
            }
            for name, spec in specs.items()
        },
    }
    (output_dir / "manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def print_catalogue(specs: dict[str, dict[str, Any]]) -> None:
    for name, spec in specs.items():
        print(f"{name:24} {spec['kind']:13} {spec['protocol']:7} {spec['app']}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output_dir", nargs="?", type=Path)
    selector = parser.add_mutually_exclusive_group()
    selector.add_argument(
        "--list", action="store_true", help="list cases without running them"
    )
    selector.add_argument(
        "--smoke", action="store_true", help="run only the icat smoke"
    )
    selector.add_argument(
        "--compatibility", action="store_true", help="run four real-client checks"
    )
    selector.add_argument(
        "--benchmark", action="store_true", help="run the timg protocol baseline"
    )
    selector.add_argument(
        "--all", action="store_true", help="run smoke, compatibility, and benchmark"
    )
    selector.add_argument(
        "--case", choices=tuple(case_specs()), help="run one named case"
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="print the selected plan without graphical work",
    )
    parser.add_argument("--samples", type=int, default=3, choices=range(1, 6))
    parser.add_argument("--idle-seconds", type=float, default=2.0)
    parser.add_argument("--reuse-build", action="store_true")
    args = parser.parse_args()

    specs = case_specs()
    if args.list:
        print_catalogue(specs)
        return 0
    mode = args.case or (
        "smoke"
        if args.smoke
        else "compatibility"
        if args.compatibility
        else "benchmark"
        if args.benchmark
        else "all"
        if args.all
        else ""
    )
    if not mode:
        parser.error("select --smoke, --compatibility, --benchmark, --all, or --case")
    plan = build_plan(mode, args.samples)
    if args.dry_run:
        print(json.dumps({"schema": SCHEMA, "mode": mode, "plan": plan}, indent=2))
        return 0
    if args.output_dir is None:
        parser.error("output_dir is required for graphical execution")
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        parser.error("a running Hyprland session is required")
    if args.idle_seconds <= 0:
        parser.error("idle duration must be positive")
    if not FIXTURE_IMAGE.is_file() or not PRESENTATION.is_file():
        parser.error("real-client image fixtures are incomplete")

    output_dir = args.output_dir.resolve()
    if output_dir.exists():
        parser.error(f"refusing to overwrite existing output directory: {output_dir}")
    for spec in specs.values():
        resolved_command(spec)
    releases = ensure_release_build(args.reuse_build)
    write_manifest(output_dir, specs)
    reports: list[dict[str, Any]] = []

    for item in plan:
        case_name = item["case"]
        if item["warmup"]:
            run_name = f"{case_name}-warmup"
        elif specs[case_name]["kind"] == "benchmark":
            run_name = f"{case_name}-sample-{item['sample']:02d}"
        else:
            run_name = case_name
        report = run_case(
            output_dir,
            case_name,
            run_name,
            releases,
            args.idle_seconds,
        )
        reports.append(report)
        print(json.dumps(report, indent=2, sort_keys=True))
        if not report["valid"]:
            break

    if (
        mode in ("benchmark", "all")
        and args.samples >= 3
        and args.samples < 5
        and all(report["valid"] for report in reports)
    ):
        extension_failed = False
        for case_name in BENCHMARK_CASES:
            measured = [
                report
                for report in reports
                if report["case"] == case_name and "-sample-" in report["run"]
            ]
            spread = measurement_summary(measured)["trigger_to_composed_capture_ns"][
                "spread"
            ]
            if spread is None or spread <= 0.20:
                continue
            for sample in range(args.samples + 1, min(5, args.samples + 2) + 1):
                item = {"case": case_name, "sample": sample, "warmup": False}
                plan.append(item)
                report = run_case(
                    output_dir,
                    case_name,
                    f"{case_name}-sample-{sample:02d}",
                    releases,
                    args.idle_seconds,
                )
                reports.append(report)
                print(json.dumps(report, indent=2, sort_keys=True))
                if not report["valid"]:
                    extension_failed = True
                    break
            if extension_failed:
                break

    summaries: dict[str, Any] = {}
    for case_name in BENCHMARK_CASES:
        measured = [
            report
            for report in reports
            if report["case"] == case_name and "-sample-" in report["run"]
        ]
        if measured:
            summaries[case_name] = measurement_summary(measured)
    summary = {
        "schema": SCHEMA,
        "mode": mode,
        "valid": len(reports) == len(plan)
        and all(report["valid"] for report in reports),
        "completed_runs": len(reports),
        "planned_runs": len(plan),
        "benchmark": summaries,
    }
    (output_dir / "summary.json").write_text(
        json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps(summary, indent=2, sort_keys=True))
    return 0 if summary["valid"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
