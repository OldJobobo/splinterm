#!/usr/bin/env python3
"""Leave one tiled Splinterm/Foot PipeWire Cava comparison on guarded workspace 8."""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import pathlib
import shlex
import shutil
import signal
import subprocess
import time
from typing import Any

ROOT = pathlib.Path(__file__).resolve().parents[2]
IDLE_RUNNER = ROOT / "tools/benchmark/run-graphical-idle.py"


def load_idle_runner():
    spec = importlib.util.spec_from_file_location("splinterm_cava_showcase_common", IDLE_RUNNER)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


COMMON = load_idle_runner()
V1 = COMMON.V1
FOOT_APP_ID = "splinterbench-foot-cava"


def write_pipewire_fixture(state: pathlib.Path) -> pathlib.Path:
    config = state / "cava.conf"
    config.write_text(
        "[general]\n"
        "framerate = 60\n"
        "bars = 32\n"
        "[input]\n"
        "method = pipewire\n"
        "source = auto\n"
        "[output]\n"
        "method = noncurses\n"
        "channels = stereo\n"
        "synchronized_sync = 1\n",
        encoding="utf-8",
    )
    wrapper = state / "run-cava-pipewire.sh"
    wrapper.write_text(
        "#!/usr/bin/env bash\nset -eu\n"
        "stty cols 120 rows 40\n"
        f"exec cava -p {shlex.quote(str(config))}\n",
        encoding="utf-8",
    )
    wrapper.chmod(0o700)
    return wrapper


def dispatch_tiled_launcher(launcher: pathlib.Path) -> None:
    expression = (
        f"hl.exec_cmd({json.dumps(str(launcher))}, "
        "{ workspace = '8 silent', float = false, opacity = '1 1', "
        "no_initial_focus = true, no_focus = true })"
    )
    result = V1.run(["hyprctl", "eval", expression], capture_output=True, timeout=5)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or result.stdout.strip())


def wait_window(existing: set[str], app_id: str) -> dict[str, Any]:
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        V1.assert_user_workspace_untouched()
        window = next(
            (
                item
                for item in V1.all_clients()
                if item.get("address") not in existing and item.get("class") == app_id
            ),
            None,
        )
        if window is not None:
            if (
                window.get("workspace", {}).get("id") != V1.TEST_WORKSPACE
                or window.get("monitor") != V1.test_monitor_id()
            ):
                raise RuntimeError(f"{app_id} escaped workspace 8 / DP-2")
            if window.get("floating") is not False:
                raise RuntimeError(f"{app_id} was not tiled by the workspace layout")
            return window
        time.sleep(0.02)
    raise RuntimeError(f"{app_id} did not map")


def rectangles_overlap(left: dict[str, Any], right: dict[str, Any]) -> bool:
    lx, ly = left["at"]
    lw, lh = left["size"]
    rx, ry = right["at"]
    rw, rh = right["size"]
    return lx < rx + rw and rx < lx + lw and ly < ry + rh and ry < ly + lh


def write_cleanup(
    state: pathlib.Path, daemon_pid: int, addresses: list[str], duration: int
) -> pathlib.Path:
    cleanup = state / "cleanup.sh"
    commands = ["#!/usr/bin/env bash", "set +e", f"sleep {duration}"]
    for address in addresses:
        selector = json.dumps(f"address:{address}")
        expression = f"hl.dispatch(hl.dsp.window.kill({{ window = {selector} }}))"
        commands.append(
            f"hyprctl eval {shlex.quote(expression)} >/dev/null 2>&1 || true"
        )
    commands.extend(
        (
            f"kill -TERM -- -{daemon_pid} 2>/dev/null || true",
            f"rm -rf {shlex.quote(str(state))}",
        )
    )
    cleanup.write_text("\n".join(commands) + "\n", encoding="utf-8")
    cleanup.chmod(0o700)
    return cleanup


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--duration-seconds", type=int, default=300)
    parser.add_argument("--trace", action="store_true")
    args = parser.parse_args()
    if not 30 <= args.duration_seconds <= 1800:
        parser.error("duration must be between 30 and 1800 seconds")
    if shutil.which("cava") is None or shutil.which("foot") is None:
        parser.error("cava and foot are required")
    ambient_cava = subprocess.run(
        ["pgrep", "-x", "cava"],
        check=False,
        capture_output=True,
        text=True,
    ).stdout.split()
    if ambient_cava:
        parser.error(
            "ambient Cava processes would pollute the comparison: "
            + ", ".join(ambient_cava)
        )

    state = pathlib.Path(f"/tmp/splinterm-cava-side-by-side-{os.getpid()}")
    state.mkdir(mode=0o700)
    socket = state / "splinterd.sock"
    addresses: list[str] = []
    daemon: subprocess.Popen[str] | None = None
    daemon_log = None
    try:
        V1.assert_test_workspace_isolated()
        V1.assert_user_workspace_untouched()
        daemon_environment = os.environ.copy()
        daemon_environment.update(
            SPLINTERM_SOCKET=str(socket),
            SPLINTERM_ENABLE_DEV_ATTACH="1",
            SPLINTERM_CONFIG=str(COMMON.PROFILES / "splinterm.ini"),
            XDG_STATE_HOME=str(state / "xdg-state"),
        )
        daemon_log = (state / "daemon.log").open("w", encoding="utf-8")
        daemon = subprocess.Popen(
            [str(COMMON.splinterd_executable())],
            env=daemon_environment,
            stdin=subprocess.DEVNULL,
            stdout=daemon_log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
            text=True,
        )
        COMMON.wait_socket(socket, daemon)

        splinter_state = state / "splinterm"
        foot_state = state / "foot"
        splinter_state.mkdir()
        foot_state.mkdir()
        splinter_fixture = write_pipewire_fixture(splinter_state)
        foot_fixture = write_pipewire_fixture(foot_state)

        splinter_command, splinter_environment = COMMON.launch_command(
            "splinterm", splinter_state, socket, args.duration_seconds
        )
        trace_dir = state / "trace"
        if args.trace:
            trace_dir.mkdir()
            splinter_environment.update(
                SPLINTERM_PERF_TRACE_DIR=str(trace_dir),
                SPLINTERM_PERF_RUN_ID="pipewire-cava",
                SPLINTERM_PERF_TRACE_MAX_EVENTS="32768",
            )
        separator = splinter_command.index("--")
        name = splinter_command.index("--name")
        splinter_command[name + 1] = "Splinterm candidate — PipeWire Cava"
        splinter_command = [
            *splinter_command[: separator + 1],
            str(splinter_fixture),
        ]
        splinter_launcher = state / "launch-splinterm.sh"
        COMMON.write_launcher(
            splinter_launcher, splinter_command, splinter_environment
        )

        foot_command = [
            shutil.which("foot") or "foot",
            "-c",
            str(COMMON.PROFILES / "foot.ini"),
            "-a",
            FOOT_APP_ID,
            "-T",
            "Foot 1.27.0 — PipeWire Cava",
            str(foot_fixture),
        ]
        foot_launcher = state / "launch-foot.sh"
        COMMON.write_launcher(foot_launcher, foot_command, {})

        existing = {item["address"] for item in V1.all_clients()}
        dispatch_tiled_launcher(splinter_launcher)
        splinter = wait_window(existing, COMMON.APP_IDS["splinterm"])
        addresses.append(str(splinter["address"]))

        existing = {item["address"] for item in V1.all_clients()}
        dispatch_tiled_launcher(foot_launcher)
        foot = wait_window(existing, FOOT_APP_ID)
        addresses.append(str(foot["address"]))
        time.sleep(0.15)
        V1.assert_user_workspace_untouched()

        clients = [
            item
            for item in V1.workspace_clients(V1.TEST_WORKSPACE)
            if item.get("address") in addresses
        ]
        if len(clients) != 2 or any(item.get("floating") is not False for item in clients):
            raise RuntimeError("comparison did not retain exactly two tiled clients")
        if rectangles_overlap(clients[0], clients[1]):
            raise RuntimeError("workspace layout overlapped the tiled comparison clients")

        cleanup = write_cleanup(state, daemon.pid, addresses, args.duration_seconds)
        subprocess.Popen(
            [str(cleanup)],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        daemon_log.close()
        daemon_log = None
        print(
            json.dumps(
                {
                    "schema": "splinterm.performance.cava-side-by-side.v1",
                    "valid": True,
                    "workspace": 8,
                    "monitor": "DP-2",
                    "layout": "native_tiled",
                    "audio": {"method": "pipewire", "source": "auto"},
                    "duration_seconds": args.duration_seconds,
                    "trace_dir": str(trace_dir) if args.trace else None,
                    "windows": [
                        {
                            "terminal": "splinterm-candidate",
                            "address": addresses[0],
                        },
                        {"terminal": "foot-1.27.0", "address": addresses[1]},
                    ],
                    "state": str(state),
                },
                indent=2,
            )
        )
        return 0
    except Exception:
        for address in addresses:
            try:
                V1.kill_oracle_window(address)
            except Exception:
                pass
        if daemon is not None:
            try:
                os.killpg(daemon.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        if daemon_log is not None:
            daemon_log.close()
        shutil.rmtree(state, ignore_errors=True)
        raise


if __name__ == "__main__":
    raise SystemExit(main())
