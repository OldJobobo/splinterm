#!/usr/bin/env python3
"""Run one guarded startup/idle case on inactive DP-2 workspace 8."""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import pathlib
import shlex
import shutil
import subprocess
import sys
import time
from typing import Any

ROOT = pathlib.Path(__file__).resolve().parents[2]
ORACLE = ROOT / "tools/foot-oracle/run-final-buffer-comparison.py"
BENCH_CHILD = ROOT / "tools/benchmark/workloads/bench-child.py"
PROFILES = ROOT / "tools/benchmark/profiles"
APP_IDS = {
    "splinterm": "com.oldjobobo.splinterm",
    "foot": "com.oldjobobo.splinterbench.Foot",
    "kitty": "com.oldjobobo.splinterbench.Kitty",
    "ghostty": "com.oldjobobo.splinterbench.Ghostty",
    "alacritty": "com.oldjobobo.splinterbench.Alacritty",
}

sys.path.insert(0, str(ROOT / "tools/benchmark"))
from metrics import ProcessMetrics, snapshot_process_forest  # noqa: E402


def load_oracle():
    spec = importlib.util.spec_from_file_location(
        "splinterbench_graphical_guard", ORACLE
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


V1 = load_oracle()


def splinterm_executable() -> pathlib.Path:
    return pathlib.Path(
        os.environ.get("SPLINTERBENCH_SPLINTERM_CLIENT", ROOT / "target/release/splinterm")
    ).expanduser()


def splinterd_executable() -> pathlib.Path:
    return pathlib.Path(
        os.environ.get("SPLINTERBENCH_SPLINTERM_DAEMON", ROOT / "target/release/splinterd")
    ).expanduser()


def splinterm_client(
    socket: pathlib.Path, *arguments: str
) -> subprocess.CompletedProcess[str]:
    environment = os.environ.copy()
    environment.update(SPLINTERM_SOCKET=str(socket), SPLINTERM_ENABLE_DEV_ATTACH="1")
    return subprocess.run(
        [str(splinterm_executable()), *arguments],
        cwd=ROOT,
        env=environment,
        text=True,
        capture_output=True,
        check=False,
        timeout=10,
    )


def wait_socket(socket: pathlib.Path, daemon: subprocess.Popen[Any]) -> None:
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if daemon.poll() is not None:
            raise RuntimeError("isolated Splinterm daemon exited during startup")
        if socket.exists() and splinterm_client(socket, "ping").returncode == 0:
            return
        time.sleep(0.02)
    raise RuntimeError("isolated Splinterm daemon did not become ready")


def write_launcher(
    path: pathlib.Path, command: list[str], environment: dict[str, str]
) -> None:
    assignments = " ".join(
        f"{key}={shlex.quote(value)}" for key, value in sorted(environment.items())
    )
    quoted = " ".join(shlex.quote(argument) for argument in command)
    path.write_text(
        "#!/usr/bin/env bash\nset -eu\n"
        f"exec env {assignments} {quoted} >{shlex.quote(str(path.with_suffix('.stdout')))} "
        f"2>{shlex.quote(str(path.with_suffix('.stderr')))}\n",
        encoding="utf-8",
    )
    path.chmod(0o700)


def launch_command(
    terminal: str,
    state: pathlib.Path,
    socket: pathlib.Path,
    idle_seconds: float,
    *,
    case: str = "idle",
    lines: int = 1000,
    columns: int = 80,
) -> tuple[list[str], dict[str, str]]:
    ready = state / "ready.json"
    child = [
        sys.executable,
        str(BENCH_CHILD),
        case,
        "--lines",
        str(lines),
        "--columns",
        str(columns),
        "--ready-file",
        str(ready),
    ]
    if case == "idle":
        child.extend(("--idle-seconds", str(idle_seconds)))
    else:
        child.extend(
            (
                "--start-file",
                str(state / "start"),
                "--done-file",
                str(state / "done.json"),
                "--hold-seconds",
                "10",
            )
        )
        if case == "input":
            child.extend(("--received-file", str(state / "input-received.json")))
    if terminal == "foot":
        return (
            [
                shutil.which("foot") or "foot",
                "-c",
                str(PROFILES / "foot.ini"),
                "-a",
                APP_IDS[terminal],
                "-w",
                "960x600",
                "--hold",
                *child,
            ],
            {},
        )
    if terminal == "kitty":
        return (
            [
                shutil.which("kitty") or "kitty",
                "--config",
                str(PROFILES / "kitty.conf"),
                "--class",
                APP_IDS[terminal],
                "--hold",
                *child,
            ],
            {},
        )
    if terminal == "ghostty":
        return (
            [
                shutil.which("ghostty") or "ghostty",
                f"--config-file={PROFILES / 'ghostty.conf'}",
                "-e",
                *child,
            ],
            {},
        )
    if terminal == "alacritty":
        return (
            [
                shutil.which("alacritty") or "alacritty",
                "--config-file",
                str(PROFILES / "alacritty.toml"),
                "--class",
                APP_IDS[terminal],
                "--hold",
                "-e",
                *child,
            ],
            {},
        )
    return (
        [
            str(splinterm_executable()),
            "launch",
            "--new",
            "--name",
            "splinterbench",
            "--",
            *child,
        ],
        {
            "SPLINTERM_SOCKET": str(socket),
            "SPLINTERM_ENABLE_DEV_ATTACH": "1",
            "SPLINTERM_CONFIG": str(PROFILES / "splinterm.ini"),
            "XDG_STATE_HOME": str(state / "xdg-state"),
        },
    )


def dispatch_launcher(launcher: pathlib.Path) -> None:
    expression = (
        f"hl.exec_cmd({json.dumps(str(launcher))}, "
        "{ workspace = '8 silent', float = true, size = '960 600', "
        "no_initial_focus = true })"
    )
    result = V1.run(["hyprctl", "eval", expression], capture_output=True, timeout=5)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or result.stdout.strip())


def wait_launch(
    app_id: str,
    existing: set[str],
    ready_path: pathlib.Path,
    started_ns: int,
) -> tuple[dict[str, Any], dict[str, Any], int, int]:
    window = None
    ready = None
    mapped_ns = None
    ready_seen_ns = None
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if window is None:
            window = next(
                (
                    item
                    for item in V1.all_clients()
                    if item.get("class") == app_id
                    and item.get("address") not in existing
                ),
                None,
            )
            if window is not None:
                mapped_ns = time.monotonic_ns()
                if (
                    window.get("workspace", {}).get("id") != V1.TEST_WORKSPACE
                    or window.get("monitor") != V1.test_monitor_id()
                ):
                    raise RuntimeError("benchmark window escaped workspace 8 / DP-2")
        if ready is None and ready_path.exists():
            ready = json.loads(ready_path.read_text(encoding="utf-8"))
            ready_seen_ns = time.monotonic_ns()
        V1.assert_user_workspace_untouched()
        if window is not None and ready is not None:
            assert mapped_ns is not None and ready_seen_ns is not None
            return window, ready, ready_seen_ns - started_ns, mapped_ns - started_ns
        time.sleep(0.01)
    raise RuntimeError("benchmark child or window did not become ready")


def assert_owned_window(app_id: str, address: str) -> None:
    clients = V1.workspace_clients(V1.TEST_WORKSPACE)
    if len(clients) != 1:
        raise RuntimeError(
            "reserved workspace does not contain exactly one benchmark window"
        )
    window = clients[0]
    if window.get("address") != address or window.get("class") != app_id:
        raise RuntimeError("reserved workspace contains an unexpected window")
    if window.get("monitor") != V1.test_monitor_id():
        raise RuntimeError("benchmark window left DP-2")
    V1.assert_user_workspace_untouched()


def metrics_delta(before: ProcessMetrics, after: ProcessMetrics) -> dict[str, int]:
    return {
        "cpu_ticks": max(0, after.cpu_ticks - before.cpu_ticks),
        "context_switches": max(0, after.context_switches - before.context_switches),
        "rss_bytes": after.rss_bytes,
    }


def wait_cleanup() -> None:
    deadline = time.monotonic() + 3
    while V1.workspace_clients(V1.TEST_WORKSPACE) and time.monotonic() < deadline:
        V1.assert_user_workspace_untouched()
        time.sleep(0.02)
    V1.assert_test_workspace_isolated()
    V1.assert_user_workspace_untouched()


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run exactly one guarded Splinterbench startup/idle case"
    )
    parser.add_argument("output_dir", type=pathlib.Path)
    parser.add_argument("--terminal", choices=tuple(APP_IDS), required=True)
    parser.add_argument("--warmup-seconds", type=float, default=1.0)
    parser.add_argument("--sample-seconds", type=float, default=2.0)
    args = parser.parse_args()
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        parser.error("a running Hyprland session is required")
    if args.warmup_seconds < 0 or args.sample_seconds <= 0:
        parser.error("warmup must be nonnegative and sample duration must be positive")

    args.output_dir.mkdir(parents=True, exist_ok=True)
    state = pathlib.Path(f"/tmp/splinterbench-{args.terminal}-{os.getpid()}")
    shutil.rmtree(state, ignore_errors=True)
    state.mkdir(mode=0o700)
    socket = state / "splinterd.sock"
    daemon = None
    daemon_log = None
    address = None
    report: dict[str, Any] = {
        "schema": "splinterm.benchmark.graphical-idle.v1",
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
                SPLINTERM_CONFIG=str(PROFILES / "splinterm.ini"),
                XDG_STATE_HOME=str(state / "xdg-state"),
            )
            daemon_log = (state / "daemon.log").open("w", encoding="utf-8")
            daemon = subprocess.Popen(
                [str(splinterd_executable())],
                env=environment,
                stdin=subprocess.DEVNULL,
                stdout=daemon_log,
                stderr=subprocess.STDOUT,
                start_new_session=True,
                text=True,
            )
            wait_socket(socket, daemon)

        command, environment = launch_command(
            args.terminal,
            state,
            socket,
            max(30.0, args.warmup_seconds + args.sample_seconds + 10),
        )
        launcher = state / "launch.sh"
        write_launcher(launcher, command, environment)
        existing = {item["address"] for item in V1.all_clients()}
        started_ns = time.monotonic_ns()
        dispatch_launcher(launcher)
        window, ready, child_ns, map_ns = wait_launch(
            APP_IDS[args.terminal], existing, state / "ready.json", started_ns
        )
        address = str(window["address"])
        assert_owned_window(APP_IDS[args.terminal], address)
        root_pids = [int(window["pid"])]
        if daemon is not None:
            root_pids.insert(0, daemon.pid)
        child_pid = int(ready["pid"])
        measured_roots = [*root_pids, child_pid]
        time.sleep(args.warmup_seconds)
        assert_owned_window(APP_IDS[args.terminal], address)
        before = snapshot_process_forest(measured_roots)
        time.sleep(args.sample_seconds)
        assert_owned_window(APP_IDS[args.terminal], address)
        after = snapshot_process_forest(measured_roots)
        report.update(
            boundaries={
                "launch_to_child_ready_ns": child_ns,
                "launch_to_window_map_ns": map_ns,
            },
            idle={
                "warmup_seconds": args.warmup_seconds,
                "sample_seconds": args.sample_seconds,
                **metrics_delta(before, after),
            },
            processes={
                "root_pids": root_pids,
                "child_pid": child_pid,
                "child_included": True,
                "count": after.process_count,
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
                splinterm_client(socket, "terminate")
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
            wait_cleanup()
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
        output = args.output_dir / f"{args.terminal}.json"
        output.write_text(
            json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        print(f"Guarded graphical result: {output}")
        shutil.rmtree(state, ignore_errors=True)
    return 0 if report.get("valid") else 1


if __name__ == "__main__":
    raise SystemExit(main())
