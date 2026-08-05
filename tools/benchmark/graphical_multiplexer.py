"""Shared measured-case primitives for the graphical multiplexer matrix."""

from __future__ import annotations

import json
import math
import pathlib
import time
from collections.abc import Mapping, Sequence
from typing import Any

from metrics import snapshot_process_memory, snapshot_processes
from multiplexer_matrix import STACK_IDENTITIES, case_applicability
from multiplexing import Topology


def controlled_pane_commands(
    topology: Topology,
    output: pathlib.Path,
    child: pathlib.Path,
    lifetime_seconds: float,
    columns: int,
) -> dict[str, tuple[str, ...]]:
    return {
        name: (
            str(child),
            "multiplexer",
            "--ready-file",
            str(output / f"{name}-ready.json"),
            "--control-dir",
            str(output / f"{name}-control"),
            "--idle-seconds",
            str(lifetime_seconds),
            "--columns",
            str(columns),
        )
        for name in topology.pane_names
    }


def write_child_command(
    output: pathlib.Path,
    pane_name: str,
    sequence: int,
    action: str,
    **parameters: int | str,
) -> pathlib.Path:
    control = output / f"{pane_name}-control"
    control.mkdir(parents=True, exist_ok=True)
    path = control / f"command-{sequence:03d}.json"
    temporary = path.with_name(f".{path.name}.tmp")
    value = {
        "schema": "splinterm.benchmark.child-command.v1",
        "sequence": sequence,
        "action": action,
        **parameters,
    }
    temporary.write_text(json.dumps(value, sort_keys=True) + "\n", encoding="utf-8")
    temporary.replace(path)
    return path


def wait_child_results(
    output: pathlib.Path,
    sequences: Mapping[str, int],
    timeout: float,
) -> dict[str, dict[str, Any]]:
    deadline = time.monotonic() + timeout
    pending = set(sequences)
    records: dict[str, dict[str, Any]] = {}
    while pending:
        for pane_name in tuple(pending):
            sequence = sequences[pane_name]
            path = output / f"{pane_name}-control" / f"result-{sequence:03d}.json"
            try:
                value = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                continue
            if value.get("schema") != "splinterm.benchmark.child-result.v1":
                raise RuntimeError(f"{pane_name} returned an unsupported child result")
            if value.get("sequence") != sequence:
                raise RuntimeError(f"{pane_name} returned the wrong child sequence")
            records[pane_name] = value
            pending.remove(pane_name)
        if pending and time.monotonic() >= deadline:
            raise TimeoutError(
                f"timed out waiting for child results: {sorted(pending)}"
            )
        if pending:
            time.sleep(0.002)
    return records


def advance_sequences(
    current: dict[str, int], pane_names: Sequence[str]
) -> dict[str, int]:
    used = {name: current[name] for name in pane_names}
    for name in pane_names:
        current[name] += 1
    return used


def exact_resource_snapshot(
    infrastructure_pids: Sequence[int], workload_pids: Sequence[int]
) -> dict[str, Any]:
    infrastructure = sorted(set(infrastructure_pids))
    workloads = sorted(set(workload_pids))
    if set(infrastructure) & set(workloads):
        raise RuntimeError("infrastructure and workload PID roles overlap")
    total = [*infrastructure, *workloads]
    return {
        "infrastructure": _snapshot(infrastructure),
        "total": _snapshot(total),
    }


def resource_delta(
    before: Mapping[str, Any], after: Mapping[str, Any]
) -> dict[str, Any]:
    membership_changed = before["membership"] != after["membership"]
    return {
        "before": before["resources"],
        "after": after["resources"],
        "before_membership": before["membership"],
        "after_membership": after["membership"],
        "membership_changed": membership_changed,
        "delta": (
            None
            if membership_changed
            else {
                key: _resource_delta(before["resources"][key], after["resources"][key])
                for key in ("infrastructure", "total")
            }
        ),
    }


def topology_rectangles(
    topology_name: str,
    origin_x: int,
    origin_y: int,
    width: int,
    height: int,
) -> dict[str, tuple[int, int, int, int]]:
    if min(width, height) <= 0:
        raise ValueError("aggregate geometry must be positive")
    if topology_name == "single":
        return {"pane-0": (origin_x, origin_y, width, height)}
    if topology_name == "two-columns":
        first = width // 2
        return {
            "pane-0": (origin_x, origin_y, first, height),
            "pane-1": (origin_x + first, origin_y, width - first, height),
        }
    if topology_name == "four-grid":
        first_width = width // 2
        first_height = height // 2
        return {
            "pane-0": (origin_x, origin_y, first_width, first_height),
            "pane-1": (
                origin_x,
                origin_y + first_height,
                first_width,
                height - first_height,
            ),
            "pane-2": (
                origin_x + first_width,
                origin_y,
                width - first_width,
                first_height,
            ),
            "pane-3": (
                origin_x + first_width,
                origin_y + first_height,
                width - first_width,
                height - first_height,
            ),
        }
    raise ValueError(f"unsupported topology: {topology_name}")


def validate_topology_geometry(
    topology: Topology, panes: Sequence[Mapping[str, Any]]
) -> None:
    if [str(item["name"]) for item in panes] != list(topology.pane_names):
        raise RuntimeError("pane geometry identities do not match the topology")
    if any(int(item["columns"]) <= 0 or int(item["rows"]) <= 0 for item in panes):
        raise RuntimeError("pane geometry contains an empty pane")
    positioned = all("x" in item and "y" in item for item in panes)
    if topology.name == "single":
        return
    if not positioned:
        return
    by_name = {str(item["name"]): item for item in panes}
    if topology.name == "two-columns":
        left, right = by_name["pane-0"], by_name["pane-1"]
        if not (int(left["x"]) < int(right["x"]) and int(left["y"]) == int(right["y"])):
            raise RuntimeError("two-column panes do not have left-right orientation")
        return
    top_left = by_name["pane-0"]
    bottom_left = by_name["pane-1"]
    top_right = by_name["pane-2"]
    bottom_right = by_name["pane-3"]
    if not (
        int(top_left["x"])
        == int(bottom_left["x"])
        < int(top_right["x"])
        == int(bottom_right["x"])
        and int(top_left["y"]) == int(top_right["y"])
        and int(top_left["y"]) < int(bottom_left["y"])
        and int(top_right["y"]) < int(bottom_right["y"])
    ):
        raise RuntimeError("four-grid panes do not have quadrant orientation")


def case_stub(stack: str, topology: str, case: str) -> dict[str, Any]:
    applicability = case_applicability(stack, topology, case)
    return {
        "schema": "splinterm.benchmark.multiplexer-operation.v1",
        "case": case,
        "applicability": applicability,
        "boundary": None,
        "metrics": {},
        "pane_metrics": {},
        "resources": None,
        "valid": applicability["status"] == "not-applicable",
        "notes": [],
    }


def stack_identity(stack: str) -> dict[str, str | None]:
    try:
        return dict(STACK_IDENTITIES[stack])
    except KeyError as error:
        raise ValueError(f"unsupported stack: {stack}") from error


def _snapshot(pids: list[int]) -> dict[str, Any]:
    metrics = snapshot_processes(pids)
    memory = snapshot_process_memory(pids)["aggregate"]
    return {
        **metrics.as_dict(),
        "rss_bytes": int(memory["rss_bytes"]),
        "pss_bytes": int(memory["pss_bytes"]),
    }


def _resource_delta(
    before: Mapping[str, Any], after: Mapping[str, Any]
) -> dict[str, int]:
    return {
        "process_count": int(after["process_count"]),
        "cpu_ticks": max(0, int(after["cpu_ticks"]) - int(before["cpu_ticks"])),
        "context_switches": max(
            0,
            int(after["context_switches"]) - int(before["context_switches"]),
        ),
        "rss_bytes": int(after["rss_bytes"]),
        "pss_bytes": int(after["pss_bytes"]),
    }


def equal_ratio_with_one_cell_rounding(first: int, second: int) -> tuple[bool, int]:
    total = first + second
    ratio = round(first * 1000 / total)
    tolerance = math.ceil(1000 / total)
    return abs(ratio - 500) <= tolerance, ratio


def splinterm_lifecycle_window_state(
    selected_state: str,
    *,
    final_leaf: bool,
    server_alive: bool,
    window_alive: bool,
) -> str | None:
    if not server_alive:
        return None
    if selected_state == "exited-auto-closed-by-graphical-client" and final_leaf:
        return (
            "final-close-committed-unmap-pending"
            if window_alive
            else "final-close-committed-unmap-complete"
        )
    if (
        selected_state == "exited-retained-restorable"
        and final_leaf
        and not window_alive
    ):
        return "final-window-exited-with-retained-restorable-leaf"
    if window_alive:
        return (
            "running-with-unaffected-panes"
            if selected_state == "exited-auto-closed-by-graphical-client"
            else "running-with-retained-exited-leaf"
        )
    return None


def validate_equal_topology_geometry(
    topology: Topology, panes: Sequence[Mapping[str, Any]]
) -> None:
    if topology.name == "single":
        return
    topology_name = topology.name
    by_name = {str(item["name"]): item for item in panes}
    if topology_name == "two-columns":
        left, right = by_name["pane-0"], by_name["pane-1"]
        equal, ratio = equal_ratio_with_one_cell_rounding(
            int(left["columns"]), int(right["columns"])
        )
        if not equal or int(left["rows"]) != int(right["rows"]):
            raise RuntimeError(
                f"two-column panes are not at the equal-ratio baseline: {ratio}"
            )
        return
    top_left = by_name["pane-0"]
    bottom_left = by_name["pane-1"]
    top_right = by_name["pane-2"]
    bottom_right = by_name["pane-3"]
    left_widths = (int(top_left["columns"]), int(bottom_left["columns"]))
    right_widths = (int(top_right["columns"]), int(bottom_right["columns"]))
    top_rows = (int(top_left["rows"]), int(top_right["rows"]))
    bottom_rows = (int(bottom_left["rows"]), int(bottom_right["rows"]))
    equal_ratios = (
        equal_ratio_with_one_cell_rounding(left_widths[0], right_widths[0]),
        equal_ratio_with_one_cell_rounding(top_rows[0], bottom_rows[0]),
        equal_ratio_with_one_cell_rounding(top_rows[1], bottom_rows[1]),
    )
    ratios = tuple(item[1] for item in equal_ratios)
    if (
        abs(left_widths[0] - left_widths[1]) > 1
        or abs(right_widths[0] - right_widths[1]) > 1
        or abs(top_rows[0] - top_rows[1]) > 1
        or abs(bottom_rows[0] - bottom_rows[1]) > 1
        or any(not item[0] for item in equal_ratios)
    ):
        raise RuntimeError(
            f"four-grid panes are not at the equal-ratio baseline: {ratios}"
        )
