#!/usr/bin/env python3
"""Run one guarded real-Cava synchronized-frame responsiveness gate."""

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
IDLE_RUNNER = ROOT / "tools/benchmark/run-graphical-idle.py"


def load_idle_runner():
    spec = importlib.util.spec_from_file_location("splinterm_cava_common", IDLE_RUNNER)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


COMMON = load_idle_runner()
V1 = COMMON.V1


def trace_revision_sets(
    trace_dir: pathlib.Path, run_id: str
) -> tuple[int, dict[str, int], dict[str, set[int]]]:
    stage_counts: dict[str, int] = {}
    revisions: dict[str, set[int]] = {}
    records = 0
    for path in sorted(trace_dir.glob(f"{run_id}-*.jsonl")):
        with path.open(encoding="utf-8") as stream:
            for line in stream:
                record = json.loads(line)
                if record.get("run_id") != run_id:
                    continue
                records += 1
                stage = str(record["stage"])
                stage_counts[stage] = stage_counts.get(stage, 0) + 1
                revision = record.get("revision")
                if isinstance(revision, int):
                    revisions.setdefault(stage, set()).add(revision)
    return records, stage_counts, revisions


def trace_stage_records(
    trace_dir: pathlib.Path, run_id: str, stage: str
) -> list[dict[str, Any]]:
    records = []
    for path in sorted(trace_dir.glob(f"{run_id}-*.jsonl")):
        with path.open(encoding="utf-8") as stream:
            for line in stream:
                record = json.loads(line)
                if record.get("run_id") == run_id and record.get("stage") == stage:
                    records.append(record)
    return sorted(records, key=lambda record: int(record["monotonic_raw_ns"]))


def advanced_revision_counts(
    baseline: dict[str, set[int]], current: dict[str, set[int]]
) -> dict[str, int]:
    return {
        stage: len(values - baseline.get(stage, set()))
        for stage, values in current.items()
    }


def trace_progress(trace_dir: pathlib.Path, run_id: str) -> dict[str, Any]:
    records, stage_counts, revisions = trace_revision_sets(trace_dir, run_id)
    return {
        "records": records,
        "stage_counts": stage_counts,
        "distinct_revisions": {
            stage: len(values) for stage, values in sorted(revisions.items())
        },
    }


def topology_splint(socket: pathlib.Path) -> dict[str, Any]:
    result = COMMON.splinterm_client(socket, "topology", "--output", "json")
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or "topology request failed")
    document = json.loads(result.stdout)
    splints = document.get("data", {}).get("splints", [])
    if len(splints) != 1:
        raise RuntimeError("Cava gate expected exactly one Splint")
    return splints[0]


def write_cava_fixture(state: pathlib.Path) -> pathlib.Path:
    fifo = state / "audio.fifo"
    os.mkfifo(fifo, mode=0o600)
    config = state / "cava.conf"
    config.write_text(
        "[general]\n"
        "framerate = 60\n"
        "bars = 32\n"
        "[input]\n"
        "method = fifo\n"
        f"source = {fifo}\n"
        "sample_rate = 44100\n"
        "sample_bits = 16\n"
        "[output]\n"
        "method = noncurses\n"
        "channels = stereo\n"
        "synchronized_sync = 1\n",
        encoding="utf-8",
    )
    producer = state / "produce-audio.py"
    producer.write_text(
        "import math, pathlib, struct, sys, time\n"
        "path = pathlib.Path(sys.argv[1])\n"
        "phase = 0.0\n"
        "chunk = 735\n"
        "with path.open('wb', buffering=0) as output:\n"
        "    frame = 0\n"
        "    while True:\n"
        "        amplitude = 2500 + (frame % 45) * 500\n"
        "        samples = []\n"
        "        for _ in range(chunk):\n"
        "            value = int(amplitude * math.sin(phase))\n"
        "            samples.extend((value, value))\n"
        "            phase += 2.0 * math.pi * 220.0 / 44100.0\n"
        "        output.write(struct.pack('<' + 'h' * len(samples), *samples))\n"
        "        frame += 1\n"
        "        time.sleep(chunk / 44100.0)\n",
        encoding="utf-8",
    )
    wrapper = state / "run-cava-fixture.sh"
    wrapper.write_text(
        "#!/usr/bin/env bash\n"
        "set -u\n"
        "stty cols 120 rows 40\n"
        f"python {shlex.quote(str(producer))} {shlex.quote(str(fifo))} &\n"
        "producer=$!\n"
        "trap 'kill $producer 2>/dev/null || true' EXIT\n"
        f"cava -p {shlex.quote(str(config))}\n",
        encoding="utf-8",
    )
    wrapper.chmod(0o700)
    return wrapper


def wait_window(existing: set[str]) -> dict[str, Any]:
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        window = next(
            (
                item
                for item in V1.all_clients()
                if item.get("class") == COMMON.APP_IDS["splinterm"]
                and item.get("address") not in existing
            ),
            None,
        )
        V1.assert_user_workspace_untouched()
        if window is not None:
            if (
                window.get("workspace", {}).get("id") != V1.TEST_WORKSPACE
                or window.get("monitor") != V1.test_monitor_id()
            ):
                raise RuntimeError("Cava window escaped workspace 8 / DP-2")
            return window
        time.sleep(0.01)
    raise RuntimeError("Cava window did not map")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run one guarded real-Cava synchronized-frame gate"
    )
    parser.add_argument("output", type=pathlib.Path)
    parser.add_argument("--minimum-revisions", type=int, default=8)
    parser.add_argument("--advance-timeout-seconds", type=float, default=5.0)
    parser.add_argument("--input-timeout-seconds", type=float, default=2.0)
    parser.add_argument(
        "--frame-only",
        action="store_true",
        help="validate synchronized frame advancement without a second controller connection",
    )
    args = parser.parse_args()
    if args.minimum_revisions < 2 or args.advance_timeout_seconds <= 0:
        parser.error("invalid frame-advance bounds")
    if not 0 < args.input_timeout_seconds <= 5:
        parser.error("invalid input timeout")
    if shutil.which("cava") is None:
        parser.error("cava is required")

    state = pathlib.Path(f"/tmp/splinterm-cava-gate-{os.getpid()}")
    shutil.rmtree(state, ignore_errors=True)
    state.mkdir(mode=0o700)
    trace_dir = state / "trace"
    trace_dir.mkdir(mode=0o700)
    socket = state / "splinterd.sock"
    run_id = f"cava-{os.getpid()}"
    mode = "frame_only" if args.frame_only else "full"
    report: dict[str, Any] = {
        "schema": (
            "splinterm.performance.graphical-cava-frame.v1"
            if args.frame_only
            else "splinterm.performance.graphical-cava.v1"
        ),
        "mode": mode,
        "claims": ["frame_advancement"]
        if args.frame_only
        else ["frame_advancement", "graphical_client_input_exit"],
        "valid": False,
        "run_id": run_id,
        "notes": [],
    }
    daemon = None
    daemon_log = None
    address = None
    progress: dict[str, Any] = {}
    try:
        V1.assert_test_workspace_isolated()
        V1.assert_user_workspace_untouched()
        environment = os.environ.copy()
        environment.update(
            SPLINTERM_SOCKET=str(socket),
            SPLINTERM_ENABLE_DEV_ATTACH="1",
            SPLINTERM_CONFIG=str(COMMON.PROFILES / "splinterm.ini"),
            XDG_STATE_HOME=str(state / "xdg-state"),
            SPLINTERM_PERF_TRACE_DIR=str(trace_dir),
            SPLINTERM_PERF_RUN_ID=run_id,
            SPLINTERM_PERF_TRACE_MAX_EVENTS="32768",
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

        command, client_environment = COMMON.launch_command(
            "splinterm", state, socket, 30
        )
        separator = command.index("--")
        command = [*command[: separator + 1], str(write_cava_fixture(state))]
        client_environment.update(
            SPLINTERM_PERF_TRACE_DIR=str(trace_dir),
            SPLINTERM_PERF_RUN_ID=run_id,
            SPLINTERM_PERF_TRACE_MAX_EVENTS="32768",
        )
        if not args.frame_only:
            client_environment["SPLINTERM_GRAPHICAL_INPUT_AFTER_COMMITS"] = "12"
        launcher = state / "launch.sh"
        COMMON.write_launcher(launcher, command, client_environment)
        existing = {item["address"] for item in V1.all_clients()}
        _, _, baseline_revisions = trace_revision_sets(trace_dir, run_id)
        COMMON.dispatch_launcher(launcher)
        window = wait_window(existing)
        address = str(window["address"])
        COMMON.assert_owned_window(COMMON.APP_IDS["splinterm"], address)

        splint = topology_splint(socket)
        splint_id = str(splint["splint_id"])
        deadline = time.monotonic() + args.advance_timeout_seconds
        while time.monotonic() < deadline:
            COMMON.assert_owned_window(COMMON.APP_IDS["splinterm"], address)
            progress = trace_progress(trace_dir, run_id)
            _, _, current_revisions = trace_revision_sets(trace_dir, run_id)
            advanced = advanced_revision_counts(baseline_revisions, current_revisions)
            if (
                advanced.get("client_apply", 0) >= args.minimum_revisions
                and advanced.get("draw_commit", 0) >= args.minimum_revisions
            ):
                progress["post_readiness_distinct_revisions"] = advanced
                break
            time.sleep(0.05)
        else:
            raise RuntimeError("Cava synchronized frames did not keep advancing")

        report.update(
            splint_id=splint_id,
            frame_progress=progress,
            isolation={
                "workspace": 8,
                "monitor": "DP-2",
                "no_initial_focus": True,
                "cleanup_verified": False,
            },
        )
        if args.frame_only:
            report.update(
                valid=True,
                input={
                    "action": "skipped",
                    "reason": "frame-only renderer smoke",
                },
            )
        else:
            input_started = time.monotonic_ns()
            input_deadline = time.monotonic() + args.input_timeout_seconds
            lifecycle = "running"
            while time.monotonic() < input_deadline:
                COMMON.assert_owned_window(COMMON.APP_IDS["splinterm"], address)
                lifecycle = str(topology_splint(socket)["lifecycle"])
                if lifecycle != "running":
                    break
                time.sleep(0.02)
            if lifecycle == "running":
                raise RuntimeError("Cava did not respond to the q input within the bound")
            if lifecycle not in {"exited", "restorable"}:
                raise RuntimeError(f"Cava reached unexpected lifecycle {lifecycle!r}")
            input_events = trace_stage_records(trace_dir, run_id, "graphical_input")
            if len(input_events) != 1:
                raise RuntimeError("full Cava smoke requires exactly one graphical input event")
            input_event = input_events[0]
            input_time = int(input_event["monotonic_raw_ns"])
            committed_before_input = {
                (
                    record.get("splint_id"),
                    record.get("incarnation"),
                    record.get("revision"),
                )
                for record in trace_stage_records(trace_dir, run_id, "draw_commit")
                if int(record["monotonic_raw_ns"]) <= input_time
                and isinstance(record.get("revision"), int)
            }
            if len(committed_before_input) < 12:
                raise RuntimeError(
                    "graphical input preceded twelve distinct committed revisions"
                )
            input_identity = (
                input_event.get("splint_id"),
                input_event.get("incarnation"),
                input_event.get("revision"),
            )
            if input_identity not in committed_before_input:
                raise RuntimeError("graphical input identity was not committed first")
            observation_latency_ns = time.monotonic_ns() - input_started
            report.update(
                valid=True,
                input={
                    "action": "graphical_client_q_after_distinct_commits",
                    "commit_threshold": 12,
                    "committed_revisions_before_input": len(committed_before_input),
                    "trace_monotonic_raw_ns": input_time,
                    "resulting_lifecycle": lifecycle,
                    "observation_latency_ns": observation_latency_ns,
                    "timeout_ns": int(args.input_timeout_seconds * 1_000_000_000),
                },
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
                },
            )["cleanup_verified"] = True
        except Exception as error:
            report["valid"] = False
            report["notes"].append(str(error))
        report["frame_progress"] = trace_progress(trace_dir, run_id)
        retained_trace = args.output.with_name(f"{args.output.stem}-trace")
        shutil.rmtree(retained_trace, ignore_errors=True)
        if trace_dir.exists():
            shutil.copytree(trace_dir, retained_trace)
            report["trace_directory"] = str(retained_trace)
        shutil.rmtree(state, ignore_errors=True)

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))
    return 0 if report["valid"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
