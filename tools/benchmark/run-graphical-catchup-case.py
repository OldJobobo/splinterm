#!/usr/bin/env python3
"""Run one guarded native Plan 0022 graphical catch-up schedule entry."""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import asdict
from typing import Any

import jsonschema

ROOT = pathlib.Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools/benchmark"
MUX_PATH = TOOLS / "run-graphical-multiplexer.py"
TRACE_SUMMARIZER = ROOT / "tools/performance/summarize-stage-trace.py"
PLAN_SCHEMA = TOOLS / "graphical-catchup-plan-schema.json"
REPORT_SCHEMA = TOOLS / "graphical-catchup-report-schema.json"
BENCH_CHILD = TOOLS / "workloads/bench-child.py"
SPLINTERM_PROFILE = TOOLS / "profiles/splinterm.ini"
APP_ID = "com.oldjobobo.splinterm"
EXPECTED_ERRORS = (
    OSError,
    RuntimeError,
    TimeoutError,
    TypeError,
    ValueError,
    subprocess.SubprocessError,
    json.JSONDecodeError,
    jsonschema.ValidationError,
)

sys.path.insert(0, str(TOOLS))
from graphical_catchup import (
    CELL_BY_NAME,
    activity_panes,
    operation_spec,
    preload_spec,
    validate_plan_semantics,
    validate_report_against_plan,
    validate_report_semantics,
    viewport_steps,
)
from graphical_catchup import (
    REPORT_SCHEMA as REPORT_SCHEMA_NAME,
)
from graphical_multiplexer import (
    advance_sequences,
    controlled_pane_commands,
    wait_child_results,
    write_child_command,
)
from headless_multiplexer import (
    ProcessIdentity,
    SplintermController,
    process_identity,
    terminate_processes_exact,
    wait_for_ready,
    wait_processes_gone,
)
from multiplexing import topology_named


def load(path: pathlib.Path, name: str):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


MUX = load(MUX_PATH, "splinterbench_catchup_mux")
SMOKE = MUX.SMOKE
COMMON = MUX.COMMON
V1 = MUX.V1
OUTPUT = MUX.OUTPUT


def catchup_profile_text() -> str:
    source = SPLINTERM_PROFILE.read_text(encoding="utf-8")
    replacements = {
        "initial-columns=80": "initial-columns=56",
        "[scrollback]\nlines=1000": "[scrollback]\nlines=4096",
    }
    for authority in replacements:
        if source.count(authority) != 1:
            raise RuntimeError(
                f"controlled Splinterm profile lost authority: {authority}"
            )
    for authority, replacement in replacements.items():
        source = source.replace(authority, replacement)
    return source


def load_json(path: pathlib.Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise TypeError(f"{path} does not contain an object")
    return value


def validate(schema_path: pathlib.Path, value: dict[str, Any]) -> None:
    jsonschema.Draft202012Validator(load_json(schema_path)).validate(value)


def selected_schedule_entry(
    plan: dict[str, Any], execution_index: int
) -> dict[str, Any]:
    validate(PLAN_SCHEMA, plan)
    validate_plan_semantics(plan)
    matches = [
        item for item in plan["schedule"] if item["execution_index"] == execution_index
    ]
    if len(matches) != 1:
        raise ValueError("execution index does not select exactly one schedule entry")
    return matches[0]


def snapshot(controller: SplintermController, pane_name: str) -> dict[str, Any]:
    value = controller._json_command(["snapshot", controller.runtime_ids[pane_name]])
    resource = value.get("resource")
    data = value.get("data")
    if not isinstance(resource, dict) or not isinstance(data, dict):
        raise TypeError("terminal snapshot response is malformed")
    return {
        "splint_id": controller.runtime_ids[pane_name],
        "incarnation": int(resource["incarnation"]),
        "revision": int(resource["terminal_revision"]),
        "columns": int(data["columns"]),
        "rows": len(data["rows"]),
    }


def settled_snapshots(
    controller: SplintermController,
    pane_names: tuple[str, ...],
    minimum_revisions: dict[str, int],
    timeout: float,
    *,
    stable_seconds: float = 0.5,
    poll_seconds: float = 0.02,
) -> list[dict[str, Any]]:
    deadline = time.monotonic() + timeout
    stable_since: float | None = None
    previous: tuple[tuple[int, int], ...] | None = None
    latest: list[dict[str, Any]] = []
    while time.monotonic() < deadline:
        latest = [snapshot(controller, name) for name in pane_names]
        signature = tuple((item["incarnation"], item["revision"]) for item in latest)
        advanced = all(
            item["revision"] > minimum_revisions[name]
            for name, item in zip(pane_names, latest, strict=True)
        )
        now = time.monotonic()
        if advanced and signature == previous:
            if stable_since is not None and now - stable_since >= stable_seconds:
                return latest
        else:
            stable_since = now if advanced else None
            previous = signature
        time.sleep(poll_seconds)
    raise TimeoutError("terminal revisions did not advance and settle")


def trace_records(trace_dir: pathlib.Path, run_id: str) -> list[dict[str, Any]]:
    records: list[dict[str, Any]] = []
    for path in sorted(trace_dir.glob("*.jsonl")):
        for line in path.read_text(encoding="utf-8").splitlines():
            if line.strip():
                value = json.loads(line)
                if value.get("run_id") == run_id:
                    records.append(value)
    return records


def wait_for_revision_commit(
    trace_dir: pathlib.Path,
    run_id: str,
    identities: list[dict[str, Any]],
    timeout: float,
) -> list[dict[str, Any]]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        records = trace_records(trace_dir, run_id)
        matches = []
        for identity in identities:
            commits = [
                item
                for item in records
                if item.get("stage") == "pane_commit"
                and item.get("splint_id") == identity["splint_id"]
                and item.get("incarnation") == identity["incarnation"]
                and item.get("revision") == identity["revision"]
            ]
            if len(commits) == 1:
                callbacks = [
                    item
                    for item in records
                    if item.get("stage") == "frame_callback"
                    and item.get("pid") == commits[0].get("pid")
                    and item.get("commit_sequence") == commits[0].get("commit_sequence")
                ]
                if len(callbacks) == 1:
                    matches.append(commits[0])
        if len(matches) == len(identities):
            return records
        time.sleep(0.01)
    raise TimeoutError(
        "exact operation revisions did not reach pane_commit and callback"
    )


def correlated_trace(
    records: list[dict[str, Any]], identities: list[dict[str, Any]]
) -> dict[str, Any]:
    receive_to_commit = []
    commit_to_callback = []
    represented_transactions: set[tuple[Any, ...]] = set()
    uncommitted: set[tuple[Any, ...]] = set()
    for identity in identities:
        matching = [
            item
            for item in records
            if item.get("splint_id") == identity["splint_id"]
            and item.get("incarnation") == identity["incarnation"]
            and item.get("revision") == identity["revision"]
        ]
        commits = [item for item in matching if item.get("stage") == "pane_commit"]
        receives = [item for item in matching if item.get("stage") == "client_receive"]
        if len(commits) != 1 or len(receives) != 1:
            raise RuntimeError("operation revision trace is incomplete or ambiguous")
        commit = commits[0]
        receive = receives[0]
        if (
            commit.get("subscription_id"),
            commit.get("transaction_sequence"),
        ) != (
            receive.get("subscription_id"),
            receive.get("transaction_sequence"),
        ):
            raise RuntimeError("operation trace transaction identity differs")
        receive_to_commit.append(
            commit["monotonic_raw_ns"] - receive["monotonic_raw_ns"]
        )
        callbacks = [
            item
            for item in records
            if item.get("stage") == "frame_callback"
            and item.get("commit_sequence") == commit.get("commit_sequence")
            and item.get("pid") == commit.get("pid")
        ]
        if len(callbacks) != 1:
            raise RuntimeError("operation commit lacks exactly one frame callback")
        commit_to_callback.append(
            callbacks[0]["monotonic_raw_ns"] - commit["monotonic_raw_ns"]
        )
    for item in records:
        if item.get("stage") == "client_receive":
            key = (
                item.get("splint_id"),
                item.get("incarnation"),
                item.get("subscription_id"),
                item.get("transaction_sequence"),
                item.get("revision"),
            )
            represented_transactions.add(key)
        if item.get("stage") == "pane_commit":
            key = (
                item.get("splint_id"),
                item.get("incarnation"),
                item.get("subscription_id"),
                item.get("transaction_sequence"),
                item.get("revision"),
            )
            represented_transactions.discard(key)
    uncommitted.update(represented_transactions)
    return {
        "records": len(records),
        "saturated": any(item.get("stage") == "trace_saturated" for item in records),
        "ambiguous": False,
        "client_receive_to_pane_commit_ns": max(receive_to_commit),
        "commit_to_callback_ns": max(commit_to_callback),
        "uncommitted_transactions": len(uncommitted),
    }


def pane_preconditions(
    records: list[dict[str, Any]], identities: list[dict[str, Any]], target: int
) -> list[dict[str, Any]]:
    panes = []
    for index, identity in enumerate(identities):
        applies = [
            item
            for item in records
            if item.get("stage") == "client_apply"
            and item.get("splint_id") == identity["splint_id"]
            and item.get("incarnation") == identity["incarnation"]
            and item.get("revision") == identity["revision"]
        ]
        if len(applies) != 1:
            raise RuntimeError("operation revision lacks exact client_apply evidence")
        apply = applies[0]
        rows = int(apply.get("cached_history_rows", -1))
        cached_bytes = int(apply.get("cached_history_bytes", -1))
        panes.append(
            {
                "pane_index": index,
                "target_cached_rows": target,
                "actual_cached_rows": rows,
                "cached_bytes": cached_bytes,
                "verified": rows == target and 0 <= cached_bytes <= 16 * 1024 * 1024,
            }
        )
    return panes


def capture_marker_attempts(
    topology: Any,
    pane_windows: dict[str, dict[str, Any]],
    target_panes: tuple[str, ...],
    state: pathlib.Path,
    timeout: float,
) -> tuple[list[dict[str, Any]], int, int]:
    attempts = []
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        sequence = len(attempts)
        capture_start = time.monotonic_ns()
        counts = MUX.pane_marker_counts(state, topology, pane_windows)
        capture_end = time.monotonic_ns()
        decode_complete = time.monotonic_ns()
        found = all(counts[name] >= 100 for name in target_panes)
        attempts.append(
            {
                "sequence": sequence,
                "capture_start_ns": capture_start,
                "capture_end_ns": capture_end,
                "decode_scan_complete_ns": decode_complete,
                "marker_found": found,
                "pane_marker_pixels": [
                    {
                        "pane_index": topology.pane_names.index(name),
                        "pixels": int(counts[name]),
                    }
                    for name in topology.pane_names
                ],
            }
        )
        if found:
            quantization = max(
                item["decode_scan_complete_ns"] - item["capture_start_ns"]
                for item in attempts
            )
            return attempts, sequence, quantization
        time.sleep(0.01)
    raise TimeoutError("visible marker was not observed")


def send_window_shortcut(address: str, key: str, mods: str = "") -> None:
    selector = json.dumps(f"address:{address}")
    for state in ("down", "up"):
        expression = (
            "hl.dsp.send_key_state({ "
            f"mods = {json.dumps(mods)}, key = {json.dumps(key)}, "
            f"state = {json.dumps(state)}, window = {selector} }})"
        )
        result = V1.run(
            ["hyprctl", "dispatch", expression], capture_output=True, timeout=5
        )
        if result.returncode:
            raise RuntimeError(result.stderr.strip() or result.stdout.strip())


def records_after(
    records: list[dict[str, Any]], baseline: set[tuple[int, int]]
) -> list[dict[str, Any]]:
    return [
        item
        for item in records
        if (int(item["pid"]), int(item["sequence"])) not in baseline
    ]


def operation_pane_evidence(
    pane_index: int,
    result: dict[str, Any],
    event: str,
    workload: str,
    units: int,
) -> dict[str, Any]:
    accepted_events = {
        "marker_complete": {"marker_complete"},
        "output_complete": {"output_complete", "write_complete"},
    }
    if result.get("event") not in accepted_events[event]:
        raise RuntimeError("child result event does not match measured operation")
    if workload in {"plain", "ansi"} and result.get("workload") != workload:
        raise RuntimeError("child result workload does not match measured operation")
    if int(result.get("completed_units", -1)) != units:
        raise RuntimeError("child result does not prove exact completed units")
    raw = {
        "schema": result.get("schema"),
        "event": result.get("event"),
        "sequence": result.get("sequence"),
        "pid": result.get("pid"),
        "received_monotonic_ns": result.get("received_monotonic_ns"),
        "completed_monotonic_ns": result.get("monotonic_ns"),
        "workload": result.get("workload"),
        "lines": result.get("lines"),
        "completed_units": result.get("completed_units"),
        "payload_bytes": result.get("payload_bytes"),
        "marker_bytes": result.get("marker_bytes"),
        "control_bytes": result.get("control_bytes"),
        "total_bytes": result.get("total_bytes"),
    }
    numeric = (
        "sequence",
        "pid",
        "received_monotonic_ns",
        "completed_monotonic_ns",
        "lines",
        "completed_units",
        "payload_bytes",
        "marker_bytes",
        "control_bytes",
        "total_bytes",
    )
    if raw["schema"] != "splinterm.benchmark.child-result.v1" or any(
        isinstance(raw[field], bool)
        or not isinstance(raw[field], int)
        or raw[field] < 0
        for field in numeric
    ):
        raise RuntimeError("child result metadata is incomplete or invalid")
    if raw["pid"] == 0 or raw["received_monotonic_ns"] > raw["completed_monotonic_ns"]:
        raise RuntimeError("child result identity or timestamp ordering is invalid")
    if raw["total_bytes"] != (
        raw["payload_bytes"] + raw["marker_bytes"] + raw["control_bytes"]
    ):
        raise RuntimeError("child result byte partition is inconsistent")
    if workload == "marker" and not (
        raw["lines"] == 0
        and raw["payload_bytes"] == 0
        and raw["marker_bytes"] > 0
        and raw["control_bytes"] == 0
    ):
        raise RuntimeError("marker child result byte partition is invalid")
    if workload == "plain" and not (
        raw["lines"] == units
        and raw["payload_bytes"] > 0
        and raw["marker_bytes"] == 0
        and raw["control_bytes"] == 0
    ):
        raise RuntimeError("continuing output byte partition is invalid")
    if workload == "ansi" and not (
        raw["lines"] == units
        and raw["payload_bytes"] > 0
        and raw["marker_bytes"] > 0
        and raw["control_bytes"] > 0
    ):
        raise RuntimeError("ANSI output byte partition is invalid")
    return {
        "pane_index": pane_index,
        "event": event,
        "workload": workload,
        "completed_units": units,
        "payload_bytes": raw["payload_bytes"],
        "marker_bytes": raw["marker_bytes"],
        "control_bytes": raw["control_bytes"],
        "raw_result": raw,
    }


def movement_counts(
    records: list[dict[str, Any]], position_changes: int
) -> dict[str, int]:
    return {
        "position_changes": position_changes,
        "configure_events": sum(
            int(item.get("configure_count", 0)) for item in records
        ),
        "output_enter_events": sum(
            int(item.get("output_enter_events", 0)) for item in records
        ),
        "output_leave_events": sum(
            int(item.get("output_leave_events", 0)) for item in records
        ),
        "semantic_applies": sum(
            item.get("stage") == "client_apply" for item in records
        ),
        "history_clones": sum(
            int(item.get("copied_history_rows", 0)) for item in records
        ),
        "pane_frame_rebuilds": sum(
            item.get("stage") == "frame_prepare" for item in records
        ),
    }


def topology_for_pane_count(pane_count: int):
    return topology_named({1: "single", 2: "two-columns", 4: "four-grid"}[pane_count])


def target_outer_size(pane_count: int) -> tuple[int, int]:
    return (480, 601) if pane_count == 1 else (960, 601)


def settle_geometry(
    topology: Any,
    controller: SplintermController,
    window: dict[str, Any],
    original_user_state: dict[str, Any],
) -> list[dict[str, Any]]:
    return MUX.stable_geometry(
        topology,
        controller,
        {"pane-0": MUX.capture_window(window)},
        original_user_state,
    )


def assert_case_is_smoke_supported(cell_name: str) -> None:
    if cell_name != "zero-history":
        raise ValueError("smoke must select the zero-history cell")


def run_case(args: argparse.Namespace) -> dict[str, Any]:
    plan = load_json(args.plan.resolve())
    entry = selected_schedule_entry(plan, args.execution_index)
    cell = CELL_BY_NAME[str(entry["cell"])]
    if args.smoke:
        assert_case_is_smoke_supported(cell.name)
    output = args.output.resolve()
    if output.exists() and any(output.iterdir()):
        raise RuntimeError(f"output directory is not empty: {output}")
    output.mkdir(parents=True, exist_ok=True)
    state = pathlib.Path(tempfile.mkdtemp(prefix="sb-catchup-cell-"))
    controller_output = state / "controller"
    controller_output.mkdir()
    trace_dir = output / "trace"
    trace_dir.mkdir()
    run_id = f"gc-{args.execution_index}-{os.getpid()}"
    topology = topology_for_pane_count(cell.pane_count)
    controller: SplintermController | None = None
    server: ProcessIdentity | None = None
    window_identity: ProcessIdentity | None = None
    workload_identities: list[ProcessIdentity] = []
    infrastructure_identities: list[ProcessIdentity] = []
    address: str | None = None
    original_user_state: dict[str, Any] | None = None
    cleanup_failure: str | None = None
    failure: str | None = None
    cleanup = {
        "windows_absent": False,
        "processes_absent": False,
        "namespace_absent": False,
        "trace_closed": False,
        "verified": False,
        "failure": None,
    }
    capture = None
    operation_identities: list[dict[str, Any]] = []
    trace_value: dict[str, Any] | None = None
    panes: list[dict[str, Any]] = []
    observed_activity: list[int] = []
    operation_panes: list[dict[str, Any]] = []
    movement = movement_counts([], 0)
    try:
        if args.smoke:
            assert_case_is_smoke_supported(cell.name)
        V1.assert_test_workspace_isolated()
        original_user_state = {}
        SMOKE.assert_user_state(original_user_state)
        controller = SplintermController(topology, controller_output, run_id)
        catchup_profile = controller_output / "plan0022-splinterm.ini"
        catchup_profile.write_text(catchup_profile_text(), encoding="utf-8")
        controller.environment.update(
            SPLINTERM_CONFIG=str(catchup_profile),
            SPLINTERM_PERF_TRACE_DIR=str(trace_dir),
            SPLINTERM_PERF_RUN_ID=run_id,
            SPLINTERM_PERF_TRACE_MAX_EVENTS="65536",
        )
        commands = controlled_pane_commands(
            topology, controller_output, BENCH_CHILD, args.lifetime_seconds, 80
        )
        controller.start(commands)
        readiness = wait_for_ready(topology, controller_output, args.ready_timeout)
        server = controller.server_identity
        workload_identities = [
            process_identity(int(readiness[name]["pid"]))
            for name in topology.pane_names
        ]
        command, environment = SMOKE.launch_spec("splinterm-native", controller)
        launcher = state / "launch.sh"
        COMMON.write_launcher(launcher, command, environment)
        existing = {str(item["address"]) for item in V1.all_clients()}
        COMMON.dispatch_launcher(launcher)
        window = SMOKE.wait_window(
            APP_ID,
            existing,
            set(),
            SMOKE.owned_window_token(controller),
            original_user_state,
            args.ready_timeout,
        )
        address = str(window["address"])
        window_identity = process_identity(int(window["pid"]))
        MUX.assert_window_group(APP_ID, {address}, original_user_state)
        target_width, target_height = target_outer_size(cell.pane_count)
        MUX.resize_window(address, target_width, target_height)
        settle_geometry(topology, controller, window, original_user_state)
        time.sleep(args.settle_seconds)

        pane_windows = {"pane-0": MUX.capture_window(window)}
        _, infrastructure_identities, exact_workloads = MUX.process_document(
            controller, server, readiness, pane_windows
        )
        if {(item.pid, item.start_ticks) for item in exact_workloads} != {
            (item.pid, item.start_ticks) for item in workload_identities
        }:
            raise RuntimeError("captured workload identities differ from readiness")
        sequences = {name: 0 for name in topology.pane_names}
        initial = {name: snapshot(controller, name) for name in topology.pane_names}
        precondition_records: list[dict[str, Any]] | None = None
        if cell.cached_rows_per_pane == 0:
            precondition_records = wait_for_revision_commit(
                trace_dir, run_id, list(initial.values()), args.operation_timeout
            )
            panes = pane_preconditions(precondition_records, list(initial.values()), 0)
            if not all(item["verified"] for item in panes):
                raise RuntimeError("settled initial state does not prove zero history")
        else:
            before_preload = {
                name: snapshot(controller, name) for name in topology.pane_names
            }
            used = advance_sequences(sequences, topology.pane_names)
            for name, sequence in used.items():
                write_child_command(
                    controller_output,
                    name,
                    sequence,
                    "preload",
                    lines=preload_spec(cell, initial[name]["rows"])[
                        "emitted_rows_per_pane"
                    ],
                    columns=80,
                    batch_lines=16,
                    pace_milliseconds=60,
                )
            wait_child_results(controller_output, used, args.operation_timeout)
            preload_identities = settled_snapshots(
                controller,
                topology.pane_names,
                {
                    name: before_preload[name]["revision"]
                    for name in topology.pane_names
                },
                args.operation_timeout,
            )
            wait_for_revision_commit(
                trace_dir, run_id, preload_identities, args.operation_timeout
            )
            precondition_records = trace_records(trace_dir, run_id)
            panes = pane_preconditions(
                precondition_records,
                preload_identities,
                cell.cached_rows_per_pane,
            )
            if not all(item["verified"] for item in panes):
                raise RuntimeError(
                    "preload did not reach every exact cached-row target"
                )

        if cell.viewport == "detached":
            marker_before = snapshot(controller, "pane-0")
            used = advance_sequences(sequences, ("pane-0",))
            write_child_command(
                controller_output,
                "pane-0",
                used["pane-0"],
                "history-marker",
                lines=initial["pane-0"]["rows"],
                columns=80,
            )
            wait_child_results(controller_output, used, args.operation_timeout)
            marker_identity = settled_snapshots(
                controller,
                ("pane-0",),
                {"pane-0": marker_before["revision"]},
                args.operation_timeout,
            )[0]
            wait_for_revision_commit(
                trace_dir, run_id, [marker_identity], args.operation_timeout
            )
            controller.focus_pane("pane-0")
            send_window_shortcut(address, "Page_Up", "SHIFT")
            time.sleep(0.1)
            capture_marker_attempts(
                topology,
                pane_windows,
                ("pane-0",),
                state,
                args.operation_timeout,
            )

        target_names = tuple(
            topology.pane_names[index] for index in activity_panes(cell)
        )
        before_operation = {
            name: snapshot(controller, name) for name in topology.pane_names
        }
        baseline_records = trace_records(trace_dir, run_id)
        baseline = {
            (int(item["pid"]), int(item["sequence"])) for item in baseline_records
        }

        if cell.operation == "same-output-position-movement":
            used = advance_sequences(sequences, ("pane-0",))
            write_child_command(
                controller_output,
                "pane-0",
                used["pane-0"],
                "marker",
                columns=80,
            )
            marker_result = wait_child_results(
                controller_output, used, args.operation_timeout
            )["pane-0"]
            operation_pane_evidence(0, marker_result, "marker_complete", "marker", 1)
            marker_identity = settled_snapshots(
                controller,
                ("pane-0",),
                {"pane-0": before_operation["pane-0"]["revision"]},
                args.operation_timeout,
            )[0]
            wait_for_revision_commit(
                trace_dir, run_id, [marker_identity], args.operation_timeout
            )
            capture_marker_attempts(
                topology,
                pane_windows,
                ("pane-0",),
                state,
                args.operation_timeout,
            )
            before_operation = {
                name: snapshot(controller, name) for name in topology.pane_names
            }
            baseline_records = trace_records(trace_dir, run_id)
            baseline = {
                (int(item["pid"]), int(item["sequence"])) for item in baseline_records
            }
            current_window = MUX.window_by_address(address)
            if current_window is None:
                raise RuntimeError("movement target disappeared")
            old_x, old_y = (int(value) for value in current_window["at"])
            trigger_ns = time.monotonic_ns()
            COMMON.move_window_absolute(address, old_x + 20, old_y)
            deadline = time.monotonic() + args.operation_timeout
            while time.monotonic() < deadline:
                moved = MUX.window_by_address(address)
                if moved is None:
                    raise RuntimeError("movement target disappeared")
                if tuple(int(value) for value in moved["at"]) == (old_x + 20, old_y):
                    break
                SMOKE.assert_user_state(original_user_state)
                time.sleep(0.01)
            else:
                raise TimeoutError("same-output position movement did not settle")
            attempts, found_sequence, quantization = capture_marker_attempts(
                topology,
                pane_windows,
                ("pane-0",),
                state,
                args.operation_timeout,
            )
            time.sleep(0.1)
            delta = records_after(trace_records(trace_dir, run_id), baseline)
            movement = movement_counts(delta, 1)
            if any(
                value for key, value in movement.items() if key != "position_changes"
            ):
                raise RuntimeError("static movement produced non-position client work")
            after_operation = {
                name: snapshot(controller, name) for name in topology.pane_names
            }
            if any(
                after_operation[name]["revision"] != before_operation[name]["revision"]
                for name in topology.pane_names
            ):
                raise RuntimeError("static movement changed terminal revision")
            receipt_ns = trigger_ns
            trace_value = {
                "run_id": run_id,
                "records": len(delta),
                "saturated": False,
                "ambiguous": False,
                "client_receive_to_pane_commit_ns": None,
                "commit_to_callback_ns": None,
                "uncommitted_transactions": 0,
            }
            observed_activity = []
            operation_panes = [
                {
                    "pane_index": 0,
                    "event": "movement_complete",
                    "workload": "movement",
                    "completed_units": 1,
                    "payload_bytes": 0,
                    "marker_bytes": 0,
                    "control_bytes": 0,
                    "raw_result": None,
                }
            ]
        else:
            resize_steps = 0
            trigger_ns = time.monotonic_ns()
            if cell.operation == "twelve-step-outer-resize":
                sizes = ((800, 501), (960, 601)) * 6
                for width, height in sizes:
                    MUX.resize_window(address, width, height)
                    settle_geometry(topology, controller, window, original_user_state)
                    resize_steps += 1
                target_names = ("pane-0",)

            used = advance_sequences(sequences, target_names)
            for name, sequence in used.items():
                if cell.operation == "continuing-small-output":
                    write_child_command(
                        controller_output,
                        name,
                        sequence,
                        "continue-output",
                        lines=1,
                        columns=80,
                    )
                elif cell.operation == "ansi-2000-lines":
                    write_child_command(
                        controller_output,
                        name,
                        sequence,
                        "output",
                        workload="ansi",
                        lines=2000,
                        columns=80,
                        batch_lines=16,
                        pace_milliseconds=40,
                    )
                else:
                    write_child_command(
                        controller_output,
                        name,
                        sequence,
                        "marker",
                        columns=80,
                    )
            results = wait_child_results(
                controller_output, used, args.operation_timeout
            )
            if cell.operation == "twelve-step-outer-resize":
                marker_result = results["pane-0"]
                marker_evidence = operation_pane_evidence(
                    0, marker_result, "marker_complete", "marker", 1
                )
                operation_panes = [
                    {
                        "pane_index": 0,
                        "event": "resize_complete",
                        "workload": "resize",
                        "completed_units": 12,
                        "payload_bytes": 0,
                        "marker_bytes": int(marker_result["marker_bytes"]),
                        "control_bytes": 0,
                        "raw_result": marker_evidence["raw_result"],
                    }
                ]
            else:
                expected_event, expected_workload, expected_units = {
                    "small-marker": ("marker_complete", "marker", 1),
                    "continuing-small-output": ("output_complete", "plain", 1),
                    "ansi-2000-lines": ("output_complete", "ansi", 2000),
                }[cell.operation]
                operation_panes = [
                    operation_pane_evidence(
                        topology.pane_names.index(name),
                        results[name],
                        expected_event,
                        expected_workload,
                        expected_units,
                    )
                    for name in target_names
                ]
            attempts, found_sequence, quantization = capture_marker_attempts(
                topology,
                pane_windows,
                target_names,
                state,
                args.operation_timeout,
            )
            operation_identities = settled_snapshots(
                controller,
                target_names,
                {name: before_operation[name]["revision"] for name in target_names},
                args.operation_timeout,
            )
            for identity in operation_identities:
                before = before_operation[
                    next(
                        name
                        for name in target_names
                        if controller.runtime_ids[name] == identity["splint_id"]
                    )
                ]
                if identity["revision"] <= before["revision"]:
                    raise RuntimeError(
                        "measured update did not advance terminal revision"
                    )
            records = wait_for_revision_commit(
                trace_dir, run_id, operation_identities, args.operation_timeout
            )
            trace_value = {
                "run_id": run_id,
                **correlated_trace(records, operation_identities),
            }
            delta = records_after(records, baseline)
            movement = movement_counts(delta, 0)
            if cell.viewport == "detached":
                roles = {
                    item.get("pane_role")
                    for item in records
                    if item.get("stage") == "pane_commit"
                    and item.get("splint_id") == operation_identities[0]["splint_id"]
                    and item.get("revision") == operation_identities[0]["revision"]
                }
                if roles != {"detached-viewport"}:
                    raise RuntimeError(
                        "detached viewport was not proven by pane commit"
                    )
            if cell.operation == "twelve-step-outer-resize" and resize_steps != 12:
                raise RuntimeError("outer resize did not complete twelve steps")
            receipt_ns = max(
                int(item.get("received_monotonic_ns", item["monotonic_ns"]))
                for item in results.values()
            )
            observed_activity = [
                topology.pane_names.index(name) for name in target_names
            ]

        if not panes:
            records = trace_records(trace_dir, run_id)
            panes = pane_preconditions(
                records, operation_identities, cell.cached_rows_per_pane
            )
        capture = {
            "marker_trigger_ns": trigger_ns,
            "child_receipt_ns": receipt_ns,
            "attempts": attempts,
            "found_sequence": found_sequence,
            "quantization_ns": quantization,
        }
        if trigger_ns > receipt_ns:
            raise RuntimeError("child receipt preceded marker trigger")
        MUX.assert_window_group(APP_ID, {address}, original_user_state)
    except EXPECTED_ERRORS as error:
        failure = f"{type(error).__name__}: {error}"
    finally:
        if address is not None:
            try:
                if MUX.window_by_address(address) is not None:
                    V1.kill_oracle_window(address)
                MUX.wait_addresses_absent({address}, 5.0)
                cleanup["windows_absent"] = True
            except EXPECTED_ERRORS as error:
                cleanup_failure = f"window cleanup: {type(error).__name__}: {error}"
        else:
            cleanup["windows_absent"] = True
        if controller is not None:
            try:
                controller.cleanup()
            except EXPECTED_ERRORS as error:
                detail = f"namespace cleanup: {type(error).__name__}: {error}"
                cleanup_failure = (
                    f"{cleanup_failure}; {detail}" if cleanup_failure else detail
                )
            cleanup["namespace_absent"] = controller.namespace_absent()
        else:
            cleanup["namespace_absent"] = True
        identities_by_incarnation = {
            (item.pid, item.start_ticks): item
            for item in [
                *infrastructure_identities,
                *workload_identities,
                *([server] if server is not None else []),
                *([window_identity] if window_identity is not None else []),
            ]
        }
        identities = list(identities_by_incarnation.values())
        if not wait_processes_gone(identities, 2.0):
            terminate_processes_exact(identities)
        cleanup["processes_absent"] = wait_processes_gone(identities, 5.0)
        cleanup["trace_closed"] = cleanup["processes_absent"]
        try:
            if original_user_state is not None:
                SMOKE.assert_user_state(original_user_state)
        except EXPECTED_ERRORS as error:
            detail = f"host cleanup: {type(error).__name__}: {error}"
            cleanup_failure = (
                f"{cleanup_failure}; {detail}" if cleanup_failure else detail
            )
        cleanup["failure"] = cleanup_failure
        cleanup["verified"] = (
            cleanup_failure is None
            and cleanup["windows_absent"]
            and cleanup["processes_absent"]
            and cleanup["namespace_absent"]
            and cleanup["trace_closed"]
        )

    if failure is None and cleanup["verified"]:
        summary_path = output / "trace-summary.json"
        result = subprocess.run(
            [
                sys.executable,
                str(TRACE_SUMMARIZER),
                str(trace_dir),
                str(summary_path),
                "--run-id",
                run_id,
            ],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
            timeout=30,
        )
        (output / "trace-summary.stdout").write_text(result.stdout, encoding="utf-8")
        (output / "trace-summary.stderr").write_text(result.stderr, encoding="utf-8")
        if result.returncode:
            failure = "strict stage-trace summary rejected the captured trace"
    valid = failure is None and cleanup["verified"]
    if not valid:
        SMOKE.copy_diagnostics(state, output)
    if capture is None:
        now = time.monotonic_ns()
        capture = {
            "marker_trigger_ns": now,
            "child_receipt_ns": now,
            "attempts": [
                {
                    "sequence": 0,
                    "capture_start_ns": now,
                    "capture_end_ns": now,
                    "decode_scan_complete_ns": now,
                    "marker_found": True,
                    "pane_marker_pixels": [
                        {"pane_index": index, "pixels": 0}
                        for index in range(cell.pane_count)
                    ],
                }
            ],
            "found_sequence": 0,
            "quantization_ns": 0,
        }
    if trace_value is None:
        trace_value = {
            "run_id": run_id,
            "records": 0,
            "saturated": False,
            "ambiguous": False,
            "client_receive_to_pane_commit_ns": None,
            "commit_to_callback_ns": None,
            "uncommitted_transactions": 0,
        }
    if not panes:
        panes = [
            {
                "pane_index": index,
                "target_cached_rows": cell.cached_rows_per_pane,
                "actual_cached_rows": 0,
                "cached_bytes": 0,
                "verified": False,
            }
            for index in range(cell.pane_count)
        ]
    report = {
        "schema": REPORT_SCHEMA_NAME,
        "case_id": entry["case_id"],
        "plan_sha256": plan["plan_sha256"],
        "phase": entry["phase"],
        "iteration": entry["iteration"],
        "execution_index": entry["execution_index"],
        "cell": asdict(cell),
        "precondition": {
            "panes": panes,
            "viewport": {
                "initial": "live",
                "steps": list(viewport_steps(cell)),
                "final": cell.viewport,
                "proven": valid,
            },
            "activity": {
                "mode": cell.activity,
                "target_panes": list(activity_panes(cell)),
                "observed_panes": observed_activity if valid else [],
                "proven": valid,
            },
        },
        "operation_evidence": {
            "unit": operation_spec(cell)[0],
            "requested_units": operation_spec(cell)[1],
            "completed_units": operation_spec(cell)[1] if valid else 0,
            "completed": valid,
            "panes": operation_panes if valid else [],
        },
        "trace": trace_value,
        "capture": capture,
        "movement": movement,
        "isolation": {
            "workspace": 8,
            "monitor": "DP-2",
            "target_owned": valid,
            "placement_preserved": valid,
            "focus_preserved": valid,
            "unrelated_activity_untouched": valid,
        },
        "cleanup": cleanup,
        "failure": failure or cleanup_failure,
        "valid": valid,
        "notes": [
            "native exact-plan collector; screenshot timing is coarse observation, not presentation"
        ],
    }
    validate(REPORT_SCHEMA, report)
    if valid:
        validate_report_against_plan(report, plan)
        validate_report_semantics(report)
    (output / "report.json").write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    shutil.rmtree(state, ignore_errors=True)
    return report


def parser() -> argparse.ArgumentParser:
    value = argparse.ArgumentParser()
    value.add_argument("plan", type=pathlib.Path)
    value.add_argument("output", type=pathlib.Path)
    value.add_argument("--execution-index", type=int, required=True)
    value.add_argument("--smoke", action="store_true")
    value.add_argument("--ready-timeout", type=float, default=10.0)
    value.add_argument("--operation-timeout", type=float, default=20.0)
    value.add_argument("--settle-seconds", type=float, default=0.5)
    value.add_argument("--lifetime-seconds", type=float, default=120.0)
    return value


def main() -> int:
    args = parser().parse_args()
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        print("a running Hyprland session is required", file=sys.stderr)
        return 2
    try:
        report = run_case(args)
        print(f"graphical catch-up result: {args.output / 'report.json'}")
        return 0 if report["valid"] else 1
    except EXPECTED_ERRORS as error:
        print(f"graphical catch-up error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
