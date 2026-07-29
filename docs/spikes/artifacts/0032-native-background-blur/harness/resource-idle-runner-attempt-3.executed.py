#!/usr/bin/env python3
"""One-shot approved Plan 0013 opaque/blur-disabled RSS and idle comparison."""
from __future__ import annotations

import hashlib
import json
import os
import pathlib
import shlex
import shutil
import signal
import statistics
import subprocess
import sys
import time
from typing import Any

ROOT = pathlib.Path("/home/oldjobobo/Projects/splinterm")
PREFEATURE = pathlib.Path("/tmp/splinterm-plan0013-prefeature")
sys.path.insert(0, str(ROOT / "tools"))
from phase10_support import V1, descendants, process_metrics, run, wait_until

APP_ID = "com.oldjobobo.splinterm"
EVIDENCE = ROOT / "docs/spikes/artifacts/0032-native-background-blur/resource-idle"
RUNTIME = pathlib.Path("/tmp/sbr3")
PAIR_COUNT = 5
IDLE_SECONDS = 2.0
SETTLE_SECONDS = 1.0
IDLE_TICK_LIMIT = 5
RSS_NOISE_FLOOR = 1024 * 1024

VERSIONS = {
    "prefeature": {
        "commit": "1e233a1c37039d147f99505a92e83169bb81f19f",
        "client": PREFEATURE / "target/release/splinterm",
        "daemon": PREFEATURE / "target/release/splinterd",
        "helper": PREFEATURE / "target/release/splinterm-pty-child",
    },
    "rc": {
        "commit": "d2affafcacd4df820e41df20971c673e99f6e46b+reviewed-worktree-config-fix",
        "client": ROOT / "target/release/splinterm",
        "daemon": ROOT / "target/release/splinterd",
        "helper": ROOT / "target/release/splinterm-pty-child",
    },
}


def sha256(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def assert_guard(focus_address: str) -> None:
    V1.assert_user_workspace_untouched()
    if V1.hyprland_json("activewindow").get("address") != focus_address:
        raise RuntimeError("focused window changed from guarded baseline")


def guarded_delay(seconds: float, focus_address: str) -> None:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        assert_guard(focus_address)
        time.sleep(min(0.05, max(0.0, deadline - time.monotonic())))


def write_config(root: pathlib.Path) -> None:
    config_dir = root / "config/splinterm"
    config_dir.mkdir(parents=True)
    theme = json.loads(pathlib.Path.home().joinpath(".config/splinterm/theme.json").read_text())
    theme.pop("blur", None)
    theme["alpha"] = 1.0
    theme_path = config_dir / "theme.json"
    theme_path.write_text(json.dumps(theme, indent=2) + "\n")
    (config_dir / "config.ini").write_text(
        "[main]\n"
        "font=JetBrains Mono Nerd Font:style=Regular\n"
        "font-pixelsize=14\n"
        "font-sizing-policy=output-scale\n"
        "padding-left=12\n"
        "padding-right=12\n"
        "padding-top=12\n"
        "padding-bottom=12\n"
        "initial-columns=80\n"
        "initial-rows=24\n"
        "login-shell=no\n"
        "resize-delay-ms=0\n"
        f"theme={theme_path}\n\n"
        "[scrollback]\nlines=1000\n\n"
        "[multiplexer]\ndivider-style=line\nframe-title=splint\n\n"
        "[cursor]\nstyle=block\nblink=no\n"
    )


def one_run(version: str, label: str, focus_address: str, *, trace: bool) -> dict[str, Any]:
    if V1.workspace_clients(8):
        raise RuntimeError("workspace 8 occupied before resource run")
    assert_guard(focus_address)
    info = VERSIONS[version]
    root = RUNTIME / label
    (root / "runtime").mkdir(parents=True, mode=0o700)
    (root / "state").mkdir()
    write_config(root)
    socket = root / "runtime/splinterd.sock"
    if len(os.fsencode(socket)) >= 108:
        raise RuntimeError(f"{label}: socket path exceeds Linux SUN_LEN: {socket}")
    env = os.environ.copy()
    env.update(
        SPLINTERM_SOCKET=str(socket),
        SPLINTERM_ENABLE_DEV_ATTACH="1",
        XDG_CONFIG_HOME=str(root / "config"),
        XDG_STATE_HOME=str(root / "state"),
    )
    if trace:
        env["SPLINTERM_BACKGROUND_EFFECT_TRACE"] = "1"
    daemon_log = (root / "daemon.log").open("w")
    daemon = subprocess.Popen(
        [str(info["daemon"])],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=daemon_log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
        text=True,
    )
    address: str | None = None
    client_pid: int | None = None
    try:
        def ping() -> bool:
            result = subprocess.run(
                [str(info["client"]), "ping"],
                env=env,
                text=True,
                capture_output=True,
                timeout=3,
            )
            return socket.exists() and result.returncode == 0

        wait_until(ping, 5, f"{label}: private daemon not ready")
        launcher = root / "launch.sh"
        child = "import sys,time; print('RESOURCE IDLE'); sys.stdout.flush(); time.sleep(120)"
        command = [
            "env",
            *[
                f"{key}={env[key]}"
                for key in (
                    "SPLINTERM_SOCKET",
                    "SPLINTERM_ENABLE_DEV_ATTACH",
                    "XDG_CONFIG_HOME",
                    "XDG_STATE_HOME",
                )
            ],
        ]
        if trace:
            command.append("SPLINTERM_BACKGROUND_EFFECT_TRACE=1")
        command.extend(
            [
                str(info["client"]),
                "launch",
                "--new",
                "--name",
                label,
                "--",
                "/usr/bin/python",
                "-c",
                child,
            ]
        )
        launcher.write_text(
            "#!/bin/sh\nexec "
            + shlex.join(command)
            + " >"
            + shlex.quote(str(root / "client.stdout"))
            + " 2>"
            + shlex.quote(str(root / "client.stderr"))
            + "\n"
        )
        launcher.chmod(0o700)
        existing = {client["address"] for client in V1.all_clients()}
        expression = (
            f"hl.exec_cmd({json.dumps(str(launcher))}, "
            "{ workspace = '8 silent', float = true, size = '960 600', "
            "opacity = '1 1', no_initial_focus = true, no_focus = true })"
        )
        dispatched = run(["hyprctl", "eval", expression], capture_output=True, timeout=5)
        if dispatched.returncode:
            raise RuntimeError(dispatched.stderr.strip() or dispatched.stdout.strip())
        window = wait_until(
            lambda: next(
                (
                    client
                    for client in V1.all_clients()
                    if client.get("class") == APP_ID and client.get("address") not in existing
                ),
                None,
            ),
            10,
            f"{label}: client did not map",
        )
        address = window["address"]
        client_pid = int(window["pid"])
        if window["workspace"]["id"] != 8 or window["monitor"] != V1.test_monitor_id():
            raise RuntimeError(f"{label}: placement violation")
        assert_guard(focus_address)
        if trace:
            def trace_ready() -> bool:
                path = root / "client.stderr"
                if not path.exists():
                    return False
                text = path.read_text(errors="replace")
                return "manager version=1 bound" in text and "capabilities=0x1 blur=true" in text

            wait_until(trace_ready, 10, f"{label}: capability trace missing")
            if "background-effect create" in (root / "client.stderr").read_text(errors="replace"):
                raise RuntimeError(f"{label}: opaque blur-disabled smoke created an effect")
        guarded_delay(SETTLE_SECONDS, focus_address)
        before = process_metrics(client_pid)
        rss_samples = [before["rss_bytes"]]
        idle_deadline = time.monotonic() + IDLE_SECONDS
        while time.monotonic() < idle_deadline:
            assert_guard(focus_address)
            sample = process_metrics(client_pid)
            rss_samples.append(sample["rss_bytes"])
            time.sleep(min(0.1, max(0.0, idle_deadline - time.monotonic())))
        after = process_metrics(client_pid)
        rss_samples.append(after["rss_bytes"])
        cpu_ticks = after["cpu_ticks"] - before["cpu_ticks"]
        if cpu_ticks > IDLE_TICK_LIMIT:
            raise RuntimeError(f"{label}: idle CPU delta {cpu_ticks} exceeds {IDLE_TICK_LIMIT}")
        return {
            "label": label,
            "version": version,
            "commit": info["commit"],
            "client_sha256": sha256(info["client"]),
            "daemon_sha256": sha256(info["daemon"]),
            "helper_sha256": sha256(info["helper"]),
            "client_size_bytes": info["client"].stat().st_size,
            "window": {
                "workspace": window["workspace"],
                "monitor": window["monitor"],
                "at": window["at"],
                "size": window["size"],
            },
            "settle_seconds": SETTLE_SECONDS,
            "idle_seconds": IDLE_SECONDS,
            "rss_bytes": max(rss_samples),
            "rss_samples_bytes": rss_samples,
            "idle_cpu_ticks": cpu_ticks,
            "focus_unchanged": True,
            "effect_created": False,
        }
    finally:
        if address:
            V1.kill_oracle_window(address)
            try:
                wait_until(lambda: not V1.workspace_clients(8), 5, f"{label}: window cleanup failed")
            except Exception:
                pass
        if client_pid and pathlib.Path(f"/proc/{client_pid}").exists():
            deadline = time.monotonic() + 5
            while pathlib.Path(f"/proc/{client_pid}").exists() and time.monotonic() < deadline:
                time.sleep(0.05)
            if pathlib.Path(f"/proc/{client_pid}").exists():
                try:
                    os.kill(client_pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
        try:
            os.killpg(daemon.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            daemon.wait(timeout=5)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(daemon.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            daemon.wait(timeout=5)
        daemon_log.close()
        assert_guard(focus_address)
        if V1.workspace_clients(8):
            raise RuntimeError(f"{label}: workspace residue")
        if pathlib.Path(f"/proc/{daemon.pid}").exists() or descendants(daemon.pid):
            raise RuntimeError(f"{label}: process residue")


def distribution(records: list[dict[str, Any]]) -> dict[str, Any]:
    rss = [int(record["rss_bytes"]) for record in records]
    idle = [int(record["idle_cpu_ticks"]) for record in records]
    return {
        "count": len(records),
        "rss_bytes": {
            "min": min(rss),
            "median": int(statistics.median(rss)),
            "max": max(rss),
            "range": max(rss) - min(rss),
        },
        "idle_cpu_ticks": {"min": min(idle), "median": statistics.median(idle), "max": max(idle)},
    }


def main() -> None:
    if EVIDENCE.exists():
        raise RuntimeError(f"refusing to reuse evidence directory {EVIDENCE}")
    shutil.rmtree(RUNTIME, ignore_errors=True)
    RUNTIME.mkdir()
    EVIDENCE.mkdir()
    for name, info in VERSIONS.items():
        for binary_name in ("client", "daemon", "helper"):
            binary = info[binary_name]
            if not binary.is_file() or not os.access(binary, os.X_OK):
                raise RuntimeError(f"{name}: required {binary_name} binary unavailable: {binary}")
    V1.assert_test_workspace_isolated()
    V1.assert_user_workspace_untouched()
    active_before = V1.hyprland_json("activeworkspace")
    focus_before = V1.hyprland_json("activewindow")
    focus_address = str(focus_before.get("address", ""))
    monitor = next(
        item for item in V1.hyprland_json("monitors all") if item.get("name") == "DP-2"
    )
    smoke: dict[str, Any] | None = None
    records: list[dict[str, Any]] = []
    failure: str | None = None
    try:
        smoke = one_run("prefeature", "prefeature-guarded-smoke", focus_address, trace=False)
        for index in range(1, PAIR_COUNT + 1):
            records.append(one_run("prefeature", f"pair-{index:02d}-prefeature", focus_address, trace=False))
            records.append(one_run("rc", f"pair-{index:02d}-rc", focus_address, trace=False))
    except Exception as error:
        failure = f"{type(error).__name__}: {error}"
        raise
    finally:
        pre = [record for record in records if record["version"] == "prefeature"]
        rc = [record for record in records if record["version"] == "rc"]
        comparison: dict[str, Any] | None = None
        if len(pre) == PAIR_COUNT and len(rc) == PAIR_COUNT:
            pre_stats = distribution(pre)
            rc_stats = distribution(rc)
            rss_delta = rc_stats["rss_bytes"]["median"] - pre_stats["rss_bytes"]["median"]
            noise = max(
                RSS_NOISE_FLOOR,
                pre_stats["rss_bytes"]["range"],
                rc_stats["rss_bytes"]["range"],
            )
            comparison = {
                "prefeature": pre_stats,
                "rc": rc_stats,
                "rc_minus_prefeature_median_rss_bytes": rss_delta,
                "measurement_noise_bytes": noise,
                "rss_within_measurement_noise": rss_delta <= noise,
                "all_idle_runs_within_tick_limit": all(
                    record["idle_cpu_ticks"] <= IDLE_TICK_LIMIT for record in records
                ),
            }
            if not comparison["rss_within_measurement_noise"]:
                failure = f"RC median RSS delta {rss_delta} exceeds measurement noise {noise}"
            if not comparison["all_idle_runs_within_tick_limit"]:
                failure = "one or more idle runs exceeded the CPU tick limit"
        summary = {
            "schema": "splinterm.native-blur.resource-idle.v1",
            "exact": True,
            "date": "2026-07-29",
            "hyprland": subprocess.check_output(["hyprctl", "version"], text=True).splitlines()[0],
            "guard": {
                "workspace": 8,
                "monitor": "DP-2",
                "monitor_geometry": {
                    key: monitor[key]
                    for key in ("width", "height", "scale", "transform", "focused", "activeWorkspace")
                },
                "active_workspace_unchanged": V1.hyprland_json("activeworkspace") == active_before,
                "focus_address_unchanged": V1.hyprland_json("activewindow").get("address") == focus_address,
                "workspace_8_clients_after": V1.workspace_clients(8),
                "screenshots_taken": False,
                "dp3_used": False,
                "transform_changed": False,
            },
            "policy": {
                "alpha": 1.0,
                "blur": False,
                "pair_count": PAIR_COUNT,
                "settle_seconds": SETTLE_SECONDS,
                "idle_seconds": IDLE_SECONDS,
                "idle_tick_limit": IDLE_TICK_LIMIT,
                "rss_noise_floor_bytes": RSS_NOISE_FLOOR,
            },
            "binaries": {
                name: {
                    "commit": info["commit"],
                    "client_sha256": sha256(info["client"]),
                    "client_size_bytes": info["client"].stat().st_size,
                    "daemon_sha256": sha256(info["daemon"]),
                    "helper_sha256": sha256(info["helper"]),
                }
                for name, info in VERSIONS.items()
            },
            "smoke": smoke,
            "runs": records,
            "comparison": comparison,
            "passed": failure is None and comparison is not None,
            "failure": failure,
        }
        (EVIDENCE / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
        if V1.workspace_clients(8):
            raise RuntimeError("resource sequence left workspace 8 occupied")
        assert_guard(focus_address)
    if failure:
        raise RuntimeError(failure)
    print(EVIDENCE)


if __name__ == "__main__":
    main()
