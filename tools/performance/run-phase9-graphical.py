#!/usr/bin/env python3
"""Run guarded Splinterm graphical performance cases on inactive DP-2/workspace 8."""

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
APP_ID = "com.oldjobobo.splinterm"
STATE = pathlib.Path("/tmp/splinterm-phase9-graphical-runtime")
THRESHOLDS = ROOT / "tools/performance/phase9-thresholds.json"


def load_oracle():
    spec = importlib.util.spec_from_file_location("phase9_oracle_guard", ORACLE)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


V1 = load_oracle()


def client(socket: pathlib.Path, *arguments: str) -> subprocess.CompletedProcess[str]:
    environment = os.environ.copy()
    environment.update(SPLINTERM_SOCKET=str(socket), SPLINTERM_ENABLE_DEV_ATTACH="1")
    return subprocess.run(
        [str(ROOT / "target/release/splinterm"), *arguments],
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
            raise RuntimeError("isolated daemon exited before creating its socket")
        if socket.exists() and client(socket, "ping").returncode == 0:
            return
        time.sleep(0.02)
    raise RuntimeError("isolated daemon did not become ready")


def launch_window(socket: pathlib.Path, existing: set[str]) -> dict[str, Any]:
    launcher = STATE / "launch-window.sh"
    launcher.write_text(
        "#!/usr/bin/env bash\nexec env "
        f"SPLINTERM_SOCKET={shlex.quote(str(socket))} SPLINTERM_ENABLE_DEV_ATTACH=1 "
        f"{shlex.quote(str(ROOT / 'target/release/splinterm'))} window "
        f">{shlex.quote(str(STATE / 'window.stdout'))} "
        f"2>{shlex.quote(str(STATE / 'window.stderr'))}\n",
        encoding="utf-8",
    )
    launcher.chmod(0o700)
    expression = (
        f"hl.exec_cmd({json.dumps(str(launcher))}, "
        "{ workspace = '8 silent', float = true, size = '960 600', no_initial_focus = true })"
    )
    result = V1.run(["hyprctl", "eval", expression], capture_output=True, timeout=5)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or result.stdout.strip())
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        candidate = next(
            (
                item
                for item in V1.all_clients()
                if item.get("class") == APP_ID and item.get("address") not in existing
            ),
            None,
        )
        if candidate is not None:
            if (
                candidate.get("workspace", {}).get("id") != V1.TEST_WORKSPACE
                or candidate.get("monitor") != V1.test_monitor_id()
            ):
                raise RuntimeError("Splinterm performance window escaped workspace 8 / DP-2")
            V1.assert_user_workspace_untouched()
            return candidate
        time.sleep(0.01)
    raise RuntimeError("Splinterm performance window did not map")


def assert_owned_window_isolated() -> None:
    clients = V1.workspace_clients(V1.TEST_WORKSPACE)
    if len(clients) != 1 or clients[0].get("class") != APP_ID:
        raise RuntimeError("reserved workspace contains an unexpected window")
    if clients[0].get("monitor") != V1.test_monitor_id():
        raise RuntimeError("performance window left DP-2")
    V1.assert_user_workspace_untouched()


def process_metrics(pid: int) -> dict[str, int]:
    stat = pathlib.Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
    fields = stat[stat.rfind(")") + 2 :].split()
    status = pathlib.Path(f"/proc/{pid}/status").read_text(encoding="utf-8")
    values: dict[str, int] = {
        "cpu_ticks": int(fields[11]) + int(fields[12]),
        "rss_bytes": 0,
        "context_switches": 0,
    }
    for line in status.splitlines():
        if line.startswith("VmRSS:"):
            values["rss_bytes"] = int(line.split()[1]) * 1024
        elif line.startswith(("voluntary_ctxt_switches:", "nonvoluntary_ctxt_switches:")):
            values["context_switches"] += int(line.split()[1])
    return values


def shm_bytes(pid: int) -> int:
    total = 0
    for fd in pathlib.Path(f"/proc/{pid}/fd").iterdir():
        try:
            target = os.readlink(fd)
            if "memfd" in target or "/dev/shm" in target:
                total += fd.stat().st_size
        except OSError:
            continue
    return total


def metric_delta(before: dict[str, int], after: dict[str, int], key: str) -> int:
    return max(0, after[key] - before[key])


def wait_marker(socket: pathlib.Path, marker: str, timeout_seconds: float) -> None:
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        snapshot = client(socket, "snapshot")
        if snapshot.returncode == 0 and marker in snapshot.stdout:
            return
        assert_owned_window_isolated()
        time.sleep(0.02)
    raise RuntimeError("graphical workload marker timed out")


def resize_sequence(address: str) -> int:
    selector = json.dumps(f"address:{address}")
    started = time.monotonic_ns()
    for width, height in [(800, 500), (1200, 700)] * 6:
        expression = (
            "hl.dispatch(hl.dsp.window.resize("
            f"{{ x = {width}, y = {height}, window = {selector} }}))"
        )
        result = V1.run(["hyprctl", "eval", expression], capture_output=True, timeout=5)
        if result.returncode:
            raise RuntimeError(result.stderr.strip() or result.stdout.strip())
        assert_owned_window_isolated()
    return time.monotonic_ns() - started


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("output_dir", type=pathlib.Path)
    parser.add_argument("--case", choices=("idle", "workload", "all"), default="idle")
    args = parser.parse_args()
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        parser.error("a running Hyprland session is required")
    thresholds = json.loads(THRESHOLDS.read_text(encoding="utf-8"))["graphical"]
    idle_seconds = float(thresholds["idle_seconds"])
    args.output_dir.mkdir(parents=True, exist_ok=True)
    V1.assert_test_workspace_isolated()
    V1.assert_user_workspace_untouched()
    build = ["cargo", "build", "--release", "-q", "-p", "splinterd", "-p", "splinterm", "-p", "splinterm-pty"]
    subprocess.run(build, cwd=ROOT, check=True)
    shutil.rmtree(STATE, ignore_errors=True)
    STATE.mkdir(mode=0o700)
    socket = STATE / "splinterd.sock"
    environment = os.environ.copy()
    environment.update(SPLINTERM_SOCKET=str(socket), SPLINTERM_ENABLE_DEV_ATTACH="1")
    log = (STATE / "daemon.log").open("w", encoding="utf-8")
    daemon = subprocess.Popen(
        [str(ROOT / "target/release/splinterd")],
        env=environment,
        stdin=subprocess.DEVNULL,
        stdout=log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
        text=True,
    )
    address = None
    failures: list[str] = []
    report: dict[str, Any] = {"schema": "splinterm.performance.graphical.v1", "case": args.case}
    try:
        wait_socket(socket, daemon)
        trigger = STATE / "start-output"
        child_script = (
            f"while [ ! -e {shlex.quote(str(trigger))} ]; do sleep 0.01; done; yes phase9 | head -n 2000; "
            "i=0; while [ $i -lt 500 ]; do printf '\\033[3%dmcolor-%04d\\033[0m\\n' "
            "$((i % 8)) $i; i=$((i+1)); done; printf 'PHASE9_GRAPHICAL_DONE\\n'; sleep 60"
        )
        created = client(socket, "new", "phase9-graphical", "--", "/bin/sh", "-c", child_script)
        if created.returncode:
            raise RuntimeError("failed to create isolated graphical benchmark child")
        existing = {item["address"] for item in V1.all_clients()}
        window = launch_window(socket, existing)
        address = window["address"]
        pid = int(window["pid"])
        time.sleep(float(thresholds["warmup_seconds"]))
        idle_before = process_metrics(pid)
        time.sleep(idle_seconds)
        idle_after = process_metrics(pid)
        report["idle"] = {
            "warmup_seconds": thresholds["warmup_seconds"],
            "seconds": idle_seconds,
            "cpu_ticks": metric_delta(idle_before, idle_after, "cpu_ticks"),
            "context_switches": metric_delta(idle_before, idle_after, "context_switches"),
            "rss_bytes": idle_after["rss_bytes"],
            "shm_bytes": shm_bytes(pid),
        }
        if report["idle"]["cpu_ticks"] > thresholds["idle_cpu_ticks_max"]:
            failures.append("idle CPU ticks exceeded budget")
        if report["idle"]["context_switches"] > thresholds["idle_context_switches_max"]:
            failures.append("idle context switches exceeded budget")
        if report["idle"]["rss_bytes"] > thresholds["rss_bytes_max"]:
            failures.append("graphical RSS exceeded budget")
        if report["idle"]["shm_bytes"] > thresholds["shm_bytes_max"]:
            failures.append("graphical SHM bytes exceeded budget")
        if args.case in ("workload", "all"):
            before = process_metrics(pid)
            marker = "PHASE9_GRAPHICAL_DONE"
            started = time.monotonic_ns()
            trigger.touch()
            wait_marker(socket, marker, thresholds["output_ns_max"] / 1_000_000_000)
            output_ns = time.monotonic_ns() - started
            after = process_metrics(pid)
            resize_ns = resize_sequence(address)
            V1.kill_oracle_window(address)
            address = None
            deadline = time.monotonic() + 3
            while V1.workspace_clients(V1.TEST_WORKSPACE) and time.monotonic() < deadline:
                time.sleep(0.02)
            reattach_started = time.monotonic_ns()
            window = launch_window(socket, {item["address"] for item in V1.all_clients()})
            address = window["address"]
            reattach_ns = time.monotonic_ns() - reattach_started
            report["workload"] = {
                "output_ns": output_ns,
                "rss_growth_bytes": metric_delta(before, after, "rss_bytes"),
                "cpu_ticks": metric_delta(before, after, "cpu_ticks"),
                "resize_sequence_ns": resize_ns,
                "reattach_ns": reattach_ns,
                "process_continuity": client(socket, "snapshot").returncode == 0,
            }
            if output_ns > thresholds["output_ns_max"]:
                failures.append("graphical output exceeded budget")
            if report["workload"]["rss_growth_bytes"] > thresholds["output_rss_growth_bytes_max"]:
                failures.append("graphical output RSS growth exceeded budget")
            if resize_ns > thresholds["resize_sequence_ns_max"]:
                failures.append("graphical resize sequence exceeded budget")
            if reattach_ns > thresholds["reattach_ns_max"]:
                failures.append("graphical reattach exceeded budget")
            if not report["workload"]["process_continuity"]:
                failures.append("daemon-owned process did not survive reattach")
        report["exact"] = not failures
        report["failures"] = failures
    except Exception as error:
        report.update(exact=False, error=str(error), failures=failures)
    finally:
        if address:
            V1.kill_oracle_window(address)
        client(socket, "terminate")
        daemon.terminate()
        try:
            daemon.wait(timeout=3)
        except subprocess.TimeoutExpired:
            daemon.kill()
            daemon.wait(timeout=2)
        log.close()
        try:
            V1.assert_test_workspace_isolated()
            V1.assert_user_workspace_untouched()
            report["cleanup"] = {"workspace_empty": True, "focus_untouched": True}
        except Exception as error:
            report["exact"] = False
            report["cleanup"] = {"workspace_empty": False, "error": str(error)}
    output = args.output_dir / "summary.json"
    output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"Phase 9 graphical result: {output}")
    return 0 if report.get("exact") else 1

if __name__ == "__main__":
    raise SystemExit(main())
