#!/usr/bin/env python3
"""Run the guarded Phase 8.1 Slice 10 Omarchy/Hyprland sign-off."""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import platform
import shlex
import shutil
import signal
import subprocess
import time

from phase10_support import (
    APP_ID,
    MONITOR,
    ROOT,
    S3,
    V1,
    WORKSPACE,
    apply_monitor_scale_owned,
    client,
    descendants,
    output_screenshot,
    process_metrics,
    run,
    screenshot,
    snapshot_has,
    wait_until,
    window_by_address,
    window_title,
)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output_dir", type=pathlib.Path)
    args = parser.parse_args()
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        parser.error("a running Hyprland session is required")
    output = args.output_dir.resolve()
    runtime = output / "runtime"
    shutil.rmtree(output, ignore_errors=True)
    (output / "config/splinterm").mkdir(parents=True)
    (output / "data/applications").mkdir(parents=True)
    runtime.mkdir(mode=0o700)
    shutil.copy(ROOT / "dist/applications/com.oldjobobo.splinterm.desktop", output / "data/applications")
    (output / "config/xdg-terminals.list").write_text(f"{APP_ID}.desktop\n")
    (output / "config/splinterm/config.ini").write_text("[scrollback]\nlines=8192\n")
    V1.assert_test_workspace_isolated()
    V1.assert_user_workspace_untouched()
    original_monitor = S3.monitor_state()
    active_before = V1.hyprland_json("activeworkspace")
    build = ["cargo", "build", "--release", "-q", "-p", "splinterd", "-p", "splinterm", "-p", "splinterm-pty"]
    run(build, cwd=ROOT, check=True)
    socket = runtime / "splinterd.sock"
    report_path = output / "interactions.json"
    environment = os.environ.copy()
    environment.update(
        SPLINTERM_SOCKET=str(socket),
        SPLINTERM_ENABLE_DEV_ATTACH="1",
        SPLINTERM_SIGNOFF_REPORT=str(report_path),
        SPLINTERM_SCROLL_TRACE="1",
        XDG_CONFIG_HOME=str(output / "config"),
        XDG_DATA_HOME=str(output / "data"),
        XDG_CACHE_HOME=str(output / "cache"),
        PATH=f"{ROOT / 'dist/bin'}:{ROOT / 'target/release'}:{environment['PATH']}",
    )
    daemon_log = (output / "daemon.log").open("w", encoding="utf-8")
    daemon = subprocess.Popen(
        [str(ROOT / "target/release/splinterd")],
        env=environment,
        stdin=subprocess.DEVNULL,
        stdout=daemon_log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
        text=True,
    )
    address: str | None = None
    stopped_pid: int | None = None
    launcher_count = 0
    summary: dict[str, Any] = {"schema": "splinterm.signoff.slice10.v1", "exact": False}
    try:
        wait_until(lambda: socket.exists() and client(socket, "ping").returncode == 0, 5, "daemon not ready")
        child_script = (
            "printf '\\033]0;Slice 10 Interaction\\007'; yes history | head -n 7000; "
            f"while [ ! -e {shlex.quote(str(output / 'start-stream'))} ]; do sleep 0.02; done; "
            "i=0; while [ $i -lt 25 ]; do printf 'stream-%04d\\n' $i; i=$((i+1)); sleep 1; done; "
            f"while [ ! -e {shlex.quote(str(output / 'enable-mouse'))} ]; do sleep 0.02; done; "
            "printf '\\033[?1000h\\033[?1006h'; "
            f"while [ ! -e {shlex.quote(str(output / 'clear'))} ]; do sleep 0.02; done; "
            "printf '\\033[?1000l\\033[?1006l\\033[3J\\033[H\\033[2J\\033]0;Slice 10 Clear\\007CLEAR_OK\\n'; "
            f"while [ ! -e {shlex.quote(str(output / 'alt'))} ]; do sleep 0.02; done; "
            "printf '\\033[?1049h\\033]0;Slice 10 Alternate\\007ALTERNATE_OK\\n'; "
            f"while [ ! -e {shlex.quote(str(output / 'normal'))} ]; do sleep 0.02; done; "
            "printf '\\033[?1049l\\033]0;Slice 10 Normal\\007NORMAL_OK\\n'; "
            f"while [ ! -e {shlex.quote(str(output / 'burst'))} ]; do sleep 0.02; done; "
            "yes forced-resync | head -n 2000; printf '\\033]0;Slice 10 Resync Recovered\\007FORCED_RESYNC_RECOVERED\\n'; "
            f"while [ ! -e {shlex.quote(str(output / 'detached'))} ]; do sleep 0.02; done; "
            "yes detached | head -n 500; printf '\\033]0;Slice 10 Reattached\\007DETACHED_OUTPUT_READY\\n'; sleep 60"
        )
        xdg_environment = [f"{key}={environment[key]}" for key in (
            "SPLINTERM_SOCKET", "SPLINTERM_ENABLE_DEV_ATTACH", "SPLINTERM_SIGNOFF_REPORT",
            "SPLINTERM_SCROLL_TRACE", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "PATH")]
        selected = run(
            ["env", *xdg_environment, "XTE_CACHE_ENABLED=0", "/usr/bin/xdg-terminal-exec", "--print-id", "--print-cmd", "--dir=/tmp", "--", "/bin/sh", "-c", child_script],
            capture_output=True,
            timeout=10,
        )
        if selected.returncode or not selected.stdout.startswith(f"{APP_ID}.desktop\n"):
            raise RuntimeError("xdg-terminal-exec did not select Splinterm")
        if "--working-directory=/tmp" not in selected.stdout:
            raise RuntimeError("xdg-terminal-exec did not translate the working directory")

        def launch_window(probe: bool) -> dict[str, Any]:
            nonlocal launcher_count
            launcher_count += 1
            launcher = output / f"launch-{launcher_count}.sh"
            launch_environment = list(xdg_environment)
            if not probe:
                launch_environment = [item for item in launch_environment if not item.startswith("SPLINTERM_SIGNOFF_REPORT=")]
            command = (
                ["env", *launch_environment, "XTE_CACHE_ENABLED=0", "/usr/bin/xdg-terminal-exec", "--dir=/tmp", "--", "/bin/sh", "-c", child_script]
                if launcher_count == 1
                else ["env", *launch_environment, str(ROOT / "target/release/splinterm"), "window"]
            )
            launcher.write_text("#!/bin/sh\nexec " + shlex.join(command) + f" >{shlex.quote(str(output / f'window-{launcher_count}.stdout'))} 2>{shlex.quote(str(output / f'window-{launcher_count}.stderr'))}\n")
            launcher.chmod(0o700)
            existing = {item["address"] for item in V1.all_clients()}
            expression = f"hl.exec_cmd({json.dumps(str(launcher))}, {{ workspace = '8 silent', float = true, size = '720 480', no_initial_focus = true }})"
            dispatched = run(["hyprctl", "eval", expression], capture_output=True, timeout=5)
            if dispatched.returncode:
                raise RuntimeError(dispatched.stderr or dispatched.stdout)
            window = wait_until(lambda: next((item for item in V1.all_clients() if item.get("class") == APP_ID and item.get("address") not in existing), None), 8, "window did not map")
            if window["workspace"]["id"] != WORKSPACE or window["monitor"] != V1.test_monitor_id():
                raise RuntimeError("window escaped workspace 8 / DP-2")
            return window

        window = launch_window(True)
        address = window["address"]
        window_pid = int(window["pid"])
        metrics_before = process_metrics(window_pid)
        interaction = None
        deadline = time.monotonic() + 75
        while time.monotonic() < deadline:
            window = window_by_address(address)
            if window is None:
                raise RuntimeError("interaction window exited")
            if report_path.exists():
                interaction = json.loads(report_path.read_text())
                if interaction.get("step") == "WaitSelectedOutput":
                    (output / "start-stream").touch()
                if interaction.get("step") == "WaitMouseTracking":
                    (output / "enable-mouse").touch()
                if interaction.get("error"):
                    raise RuntimeError(interaction["error"])
                if interaction.get("exact"):
                    break
            V1.assert_user_workspace_untouched()
            time.sleep(0.05)
        else:
            raise RuntimeError(f"interaction probe did not complete: {interaction}")
        metrics_after = process_metrics(window_pid)
        captures = {"interaction": screenshot(window_by_address(address), output / "interaction.png")}
        scales = []
        for scale_120 in (120, 150, 180, 240):
            apply_monitor_scale_owned(original_monitor, scale_120, address)
            current = window_by_address(address)
            if current is None:
                raise RuntimeError("window exited during scale matrix")
            selector = json.dumps(f"address:{address}")
            resize = f"hl.dispatch(hl.dsp.window.resize({{ x = 700, y = 460, window = {selector} }}))"
            if run(["hyprctl", "eval", resize], capture_output=True, timeout=5).returncode:
                raise RuntimeError("targeted scale resize failed")
            time.sleep(0.15)
            scales.append({"scale_120": scale_120, **output_screenshot(output / f"scale-{scale_120}.png")})
            V1.assert_user_workspace_untouched()
        S3.restore_monitor(original_monitor)
        V1.assert_user_workspace_untouched()
        (output / "clear").touch()
        window_title(address, "Slice 10 Clear — local controller — DEVELOPMENT BYPASS")
        captures["clear"] = screenshot(window_by_address(address), output / "clear.png")
        (output / "alt").touch()
        window_title(address, "Slice 10 Alternate — local controller — DEVELOPMENT BYPASS")
        captures["alternate"] = screenshot(window_by_address(address), output / "alternate.png")
        (output / "normal").touch()
        window_title(address, "Slice 10 Normal — local controller — DEVELOPMENT BYPASS")
        child_pids_before = descendants(daemon.pid)
        stopped_pid = window_pid
        os.kill(stopped_pid, signal.SIGSTOP)
        (output / "burst").touch()
        wait_until(lambda: snapshot_has(socket, "FORCED_RESYNC_RECOVERED"), 15, "forced resync marker missing")
        os.kill(stopped_pid, signal.SIGCONT)
        stopped_pid = None
        window_title(address, "Slice 10 Resync Recovered — local controller — DEVELOPMENT BYPASS")
        captures["resync"] = screenshot(window_by_address(address), output / "resync.png")
        V1.kill_oracle_window(address)
        address = None
        wait_until(lambda: not V1.workspace_clients(WORKSPACE), 5, "window did not detach")
        (output / "detached").touch()
        wait_until(lambda: snapshot_has(socket, "DETACHED_OUTPUT_READY"), 10, "detached output marker missing")
        window = launch_window(False)
        address = window["address"]
        window_title(address, "Slice 10 Reattached — local controller — DEVELOPMENT BYPASS")
        captures["reattached"] = screenshot(window_by_address(address), output / "reattached.png")
        child_pids_after = descendants(daemon.pid)
        trace = "\n".join(path.read_text(errors="replace") for path in sorted(output.glob("window-*.stderr")))
        summary = {
            "schema": "splinterm.signoff.slice10.v1",
            "exact": True,
            "host": {
                "os": platform.platform(), "architecture": platform.machine(),
                "hyprland": run(["hyprctl", "version"], capture_output=True, timeout=5).stdout.strip(),
                "rustc": run(["rustc", "--version"], capture_output=True).stdout.strip(),
                "git_commit": run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True).stdout.strip(),
            },
            "xdg": {"desktop": f"{APP_ID}.desktop", "print_cmd": selected.stdout.splitlines()[1:], "working_directory": "/tmp"},
            "window": {"app_id": APP_ID, "workspace": WORKSPACE, "monitor": MONITOR},
            "interactions": interaction,
            "performance": {"before": metrics_before, "after": metrics_after},
            "scales": scales,
            "captures": captures,
            "lifecycle": {
                "daemon_pid": daemon.pid, "initial_window_pid": window_pid,
                "child_pids_before": child_pids_before, "child_pids_after": child_pids_after,
                "process_continuity": bool(set(child_pids_before) & set(child_pids_after)),
                "forced_resync_trace_count": trace.count("resync="),
            },
            "active_workspace_before": active_before,
            "active_workspace_after": V1.hyprland_json("activeworkspace"),
        }
        if not summary["lifecycle"]["process_continuity"] or summary["active_workspace_after"] != active_before:
            raise RuntimeError("lifecycle or focus continuity failed")
    except Exception as error:
        summary.update(exact=False, error=str(error))
    finally:
        if stopped_pid is not None:
            os.kill(stopped_pid, signal.SIGCONT)
        if address:
            V1.kill_oracle_window(address)
        client(socket, "terminate")
        daemon.terminate()
        try:
            daemon.wait(timeout=3)
        except subprocess.TimeoutExpired:
            daemon.kill()
            daemon.wait(timeout=2)
        daemon_log.close()
        try:
            S3.restore_monitor(original_monitor)
            time.sleep(0.2)
            V1.assert_test_workspace_isolated()
            V1.assert_user_workspace_untouched()
            summary["cleanup"] = {"workspace_empty": True, "focus_untouched": True, "monitor_restored": True}
        except Exception as error:
            summary["exact"] = False
            summary["cleanup"] = {"workspace_empty": False, "error": str(error)}
    (output / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
    print(f"Slice 10 sign-off result: {output / 'summary.json'}")
    return 0 if summary.get("exact") else 1


if __name__ == "__main__":
    raise SystemExit(main())
