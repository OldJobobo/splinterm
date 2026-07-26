#!/usr/bin/env python3
"""Measure child exit and window lifecycle with no terminal hold option."""

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

ROOT = pathlib.Path(__file__).resolve().parents[2]
COMMON_PATH = ROOT / "tools/benchmark/run-graphical-idle.py"
sys.path.insert(0, str(ROOT / "tools/benchmark"))
from metrics import snapshot_process_forest  # noqa: E402


def load_common():
    spec = importlib.util.spec_from_file_location("lifecycle_common", COMMON_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


C = load_common()
V1 = C.V1


def window(address):
    return next(
        (item for item in V1.all_clients() if item.get("address") == address), None
    )


def main():
    parser = argparse.ArgumentParser(
        description="Run one guarded no-hold lifecycle case"
    )
    parser.add_argument("output_dir", type=pathlib.Path)
    parser.add_argument("--terminal", choices=tuple(C.APP_IDS), required=True)
    args = parser.parse_args()
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        parser.error("Hyprland required")
    args.output_dir.mkdir(parents=True, exist_ok=True)
    state = pathlib.Path(f"/tmp/splinterbench-lifecycle-{args.terminal}-{os.getpid()}")
    shutil.rmtree(state, ignore_errors=True)
    state.mkdir(mode=0o700)
    socket = state / "splinterd.sock"
    daemon = None
    log = None
    address = None
    report = {
        "schema": "splinterm.benchmark.lifecycle.v1",
        "terminal": args.terminal,
        "valid": False,
        "notes": [],
    }
    try:
        V1.assert_test_workspace_isolated()
        V1.assert_user_workspace_untouched()
        if args.terminal == "splinterm":
            env = os.environ.copy()
            env.update(
                SPLINTERM_SOCKET=str(socket),
                SPLINTERM_ENABLE_DEV_ATTACH="1",
                SPLINTERM_CONFIG=str(C.PROFILES / "splinterm.ini"),
                XDG_STATE_HOME=str(state / "state"),
            )
            log = (state / "daemon.log").open("w")
            daemon = subprocess.Popen(
                [str(ROOT / "target/release/splinterd")],
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=log,
                stderr=subprocess.STDOUT,
                start_new_session=True,
                text=True,
            )
            C.wait_socket(socket, daemon)
        command, env = C.launch_command(
            args.terminal, state, socket, 0.25, hold_window=False
        )
        if "--hold" in command:
            raise RuntimeError("lifecycle command unexpectedly contains --hold")
        launcher = state / "launch.sh"
        C.write_launcher(launcher, command, env)
        existing = {item["address"] for item in V1.all_clients()}
        launched = time.monotonic_ns()
        C.dispatch_launcher(launcher)
        mapped, ready, _, _ = C.wait_launch(
            C.APP_IDS[args.terminal], existing, state / "ready.json", launched
        )
        address = str(mapped["address"])
        child = int(ready["pid"])
        roots = [int(mapped["pid"])]
        if daemon:
            roots.insert(0, daemon.pid)
        observed = time.monotonic_ns()
        child_exit = None
        unmapped = None
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            V1.assert_user_workspace_untouched()
            now = time.monotonic_ns()
            if child_exit is None and not pathlib.Path(f"/proc/{child}").exists():
                child_exit = now - observed
            if unmapped is None and window(address) is None:
                unmapped = now - observed
            if child_exit is not None and (
                unmapped is not None or time.monotonic() + 0.1 >= deadline
            ):
                break
            time.sleep(0.01)
        if child_exit is None:
            raise RuntimeError("child did not exit")
        persisted = unmapped is None
        residual = snapshot_process_forest(roots).process_count
        report.update(
            child_exit_ns=child_exit,
            window_unmap_ns=unmapped,
            window_persisted=persisted,
            residual_process_count=residual,
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
        if address and window(address):
            V1.kill_oracle_window(address)
        if daemon:
            daemon.terminate()
            try:
                daemon.wait(timeout=3)
            except subprocess.TimeoutExpired:
                daemon.kill()
                daemon.wait(timeout=2)
        if log:
            log.close()
        try:
            C.wait_cleanup()
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
        output = args.output_dir / f"{args.terminal}-lifecycle.json"
        output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
        shutil.rmtree(state, ignore_errors=True)
        print(f"Guarded lifecycle result: {output}")
    return 0 if report.get("valid") else 1


if __name__ == "__main__":
    raise SystemExit(main())
