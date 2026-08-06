"""Finite, portable planning primitives for Plan 0022 catch-up diagnostics."""

from __future__ import annotations

import hashlib
import json
import random
from collections.abc import Mapping, Sequence
from dataclasses import asdict, dataclass
from itertools import pairwise
from typing import Any

PLAN_SCHEMA = "splinterm.benchmark.graphical-catchup-plan.v1"
REPORT_SCHEMA = "splinterm.benchmark.graphical-catchup-report.v1"
HISTORY_CAPACITY_ROWS = 4096
ANSI_OPERATION_LINES = 2000
DEFAULT_WARMUPS = 3
DEFAULT_SAMPLES = 10


@dataclass(frozen=True)
class CatchupCell:
    name: str
    pane_count: int
    cached_rows_per_pane: int
    viewport: str
    activity: str
    operation: str


CELLS = (
    CatchupCell("zero-history", 1, 0, "live", "focused-only", "small-marker"),
    CatchupCell("history-1000", 1, 1000, "live", "focused-only", "small-marker"),
    CatchupCell("history-4096", 1, 4096, "live", "focused-only", "small-marker"),
    CatchupCell(
        "detached-history",
        1,
        4096,
        "detached",
        "focused-only",
        "continuing-small-output",
    ),
    CatchupCell("inactive-scaling-2", 2, 4096, "live", "focused-only", "small-marker"),
    CatchupCell("inactive-scaling-4", 4, 4096, "live", "focused-only", "small-marker"),
    CatchupCell("all-pane-scaling", 4, 4096, "live", "all-panes", "small-marker"),
    CatchupCell(
        "static-movement", 4, 4096, "live", "idle", "same-output-position-movement"
    ),
    CatchupCell(
        "outer-resize", 4, 4096, "live", "idle-then-focused", "twelve-step-outer-resize"
    ),
    CatchupCell("ansi-stress", 4, 4096, "live", "all-panes", "ansi-2000-lines"),
)
CELL_BY_NAME = {cell.name: cell for cell in CELLS}


def canonical_sha256(value: Mapping[str, Any]) -> str:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def activity_panes(cell: CatchupCell) -> tuple[int, ...]:
    if cell.activity == "all-panes":
        return tuple(range(cell.pane_count))
    if cell.activity in {"focused-only", "idle-then-focused"}:
        return (0,)
    if cell.activity == "idle":
        return ()
    raise ValueError(f"unsupported activity mode: {cell.activity}")


def preload_spec(cell: CatchupCell, visible_rows: int) -> dict[str, Any]:
    if visible_rows < 1:
        raise ValueError("visible_rows must be positive")
    target = cell.cached_rows_per_pane
    if not 0 <= target <= HISTORY_CAPACITY_ROWS:
        raise ValueError("cached-row target exceeds the retained history bound")
    # After clear/home, the first fixed-width record occupies visible row zero;
    # the remaining visible rows precede exact history accumulation.
    emitted = 0 if target == 0 else target + visible_rows - 1
    return {
        "target_cached_rows": target,
        "visible_rows": visible_rows,
        "fixed_width_records": True,
        "emitted_rows_per_pane": emitted,
        "verification_required": True,
    }


def operation_spec(cell: CatchupCell) -> tuple[str, int]:
    return {
        "small-marker": ("marker", 1),
        "continuing-small-output": ("output-lines", 1),
        "same-output-position-movement": ("position-moves", 1),
        "twelve-step-outer-resize": ("resize-steps", 12),
        "ansi-2000-lines": ("output-lines", ANSI_OPERATION_LINES),
    }[cell.operation]


def viewport_steps(cell: CatchupCell) -> tuple[str, ...]:
    if cell.viewport == "live":
        return ("assert-live",)
    if cell.viewport == "detached":
        return ("assert-live", "scroll-back-one-page", "assert-detached")
    raise ValueError(f"unsupported viewport: {cell.viewport}")


def capture_phase_is_ordered(capture: Mapping[str, Any]) -> bool:
    try:
        trigger = int(capture["marker_trigger_ns"])
        child_receipt = int(capture["child_receipt_ns"])
        found_sequence = int(capture["found_sequence"])
        attempts = capture["attempts"]
        if not isinstance(attempts, list) or not attempts:
            return False
        if trigger > child_receipt:
            return False
        if int(attempts[0]["capture_start_ns"]) < child_receipt:
            return False
        marker_sequences = []
        previous_decode = trigger
        for sequence, attempt in enumerate(attempts):
            if not isinstance(attempt, Mapping) or attempt["sequence"] != sequence:
                return False
            times = [
                trigger,
                previous_decode,
                int(attempt["capture_start_ns"]),
                int(attempt["capture_end_ns"]),
                int(attempt["decode_scan_complete_ns"]),
            ]
            if not all(left <= right for left, right in pairwise(times)):
                return False
            previous_decode = times[-1]
            if attempt["marker_found"] is True:
                marker_sequences.append(sequence)
        return marker_sequences == [found_sequence]
    except (KeyError, TypeError, ValueError):
        return False


def plan_document(
    seed: int,
    warmup_runs: int = DEFAULT_WARMUPS,
    sample_runs: int = DEFAULT_SAMPLES,
) -> dict[str, Any]:
    if warmup_runs < 0 or sample_runs < 1:
        raise ValueError(
            "run counts must include zero or more warmups and measured runs"
        )
    rng = random.Random(seed)
    schedule: list[dict[str, Any]] = []
    execution_index = 0
    for phase, runs in (("warmup", warmup_runs), ("measured", sample_runs)):
        for iteration in range(runs):
            names = [cell.name for cell in CELLS]
            rng.shuffle(names)
            for name in names:
                schedule.append(
                    {
                        "phase": phase,
                        "iteration": iteration,
                        "execution_index": execution_index,
                        "case_id": f"{phase}-{iteration:02d}-{execution_index:04d}-{name}",
                        "cell": name,
                    }
                )
                execution_index += 1
    document: dict[str, Any] = {
        "schema": PLAN_SCHEMA,
        "seed": seed,
        "warmup_runs": warmup_runs,
        "sample_runs": sample_runs,
        "history_capacity_rows": HISTORY_CAPACITY_ROWS,
        "ansi_operation_lines": ANSI_OPERATION_LINES,
        "workspace": 8,
        "monitor": "DP-2",
        "cells": [asdict(cell) for cell in CELLS],
        "schedule": schedule,
    }
    document["plan_sha256"] = canonical_sha256(document)
    return document


def validate_plan_semantics(document: Mapping[str, Any]) -> None:
    if document.get("schema") != PLAN_SCHEMA:
        raise ValueError("unsupported graphical catch-up plan schema")
    expected_cells = [asdict(cell) for cell in CELLS]
    if document.get("cells") != expected_cells:
        raise ValueError("plan cells differ from the finite Plan 0022 matrix")
    if document.get("history_capacity_rows") != HISTORY_CAPACITY_ROWS:
        raise ValueError("history capacity changed")
    if document.get("ansi_operation_lines") != ANSI_OPERATION_LINES:
        raise ValueError("ANSI operation size changed")
    unsigned = dict(document)
    claimed = unsigned.pop("plan_sha256", None)
    if claimed != canonical_sha256(unsigned):
        raise ValueError("plan digest mismatch")
    schedule = document.get("schedule")
    if not isinstance(schedule, list):
        raise TypeError("schedule is not a list")
    expected_index = 0
    for phase, runs in (
        ("warmup", int(document["warmup_runs"])),
        ("measured", int(document["sample_runs"])),
    ):
        for iteration in range(runs):
            batch = [
                item
                for item in schedule
                if item.get("phase") == phase and item.get("iteration") == iteration
            ]
            if {item.get("cell") for item in batch} != set(CELL_BY_NAME):
                raise ValueError(
                    f"{phase} iteration {iteration} is not a complete matrix"
                )
            if len(batch) != len(CELLS):
                raise ValueError(f"{phase} iteration {iteration} contains duplicates")
    for item in schedule:
        if item.get("execution_index") != expected_index:
            raise ValueError("execution indexes are not contiguous")
        expected_case_id = (
            f"{item.get('phase')}-{int(item.get('iteration')):02d}-"
            f"{expected_index:04d}-{item.get('cell')}"
        )
        if item.get("case_id") != expected_case_id:
            raise ValueError("case identity differs from its schedule position")
        expected_index += 1
    if expected_index != (
        int(document["warmup_runs"]) + int(document["sample_runs"])
    ) * len(CELLS):
        raise ValueError("schedule length does not match run counts")


def validate_report_against_plan(
    report: Mapping[str, Any], plan: Mapping[str, Any]
) -> None:
    validate_plan_semantics(plan)
    if report.get("plan_sha256") != plan.get("plan_sha256"):
        raise ValueError("report plan digest differs from selected plan")
    execution_index = report.get("execution_index")
    matches = [
        item
        for item in plan["schedule"]
        if item.get("execution_index") == execution_index
    ]
    if len(matches) != 1:
        raise ValueError("report execution index does not select one plan entry")
    entry = matches[0]
    for field in ("case_id", "phase", "iteration", "execution_index", "cell"):
        expected = entry["cell"] if field == "cell" else entry[field]
        actual = (
            report.get("cell", {}).get("name") if field == "cell" else report.get(field)
        )
        if actual != expected:
            raise ValueError(f"report {field} differs from selected plan entry")


def validate_report_semantics(report: Mapping[str, Any]) -> None:
    if report.get("schema") != REPORT_SCHEMA:
        raise ValueError("unsupported graphical catch-up report schema")
    cell_name = report.get("cell", {}).get("name")
    cell = CELL_BY_NAME.get(str(cell_name))
    if cell is None or report.get("cell") != asdict(cell):
        raise ValueError("report cell differs from the finite matrix")
    precondition = report["precondition"]
    panes: Sequence[Mapping[str, Any]] = precondition["panes"]
    if len(panes) != cell.pane_count:
        raise ValueError("precondition pane count differs from the cell")
    for index, pane in enumerate(panes):
        if pane.get("pane_index") != index:
            raise ValueError("precondition pane indexes are not contiguous")
        if pane.get("target_cached_rows") != cell.cached_rows_per_pane:
            raise ValueError("precondition target differs from the cell")
        if pane.get("actual_cached_rows") != cell.cached_rows_per_pane:
            raise ValueError("actual cached rows do not prove the target")
        if pane.get("verified") is not True:
            raise ValueError("cached row count was not verified")
    viewport = precondition["viewport"]
    if (
        viewport.get("steps") != list(viewport_steps(cell))
        or viewport.get("final") != cell.viewport
        or viewport.get("proven") is not True
    ):
        raise ValueError("viewport transition was not proven")
    activity = precondition["activity"]
    expected_activity = list(activity_panes(cell))
    if (
        activity.get("mode") != cell.activity
        or activity.get("target_panes") != expected_activity
        or activity.get("observed_panes") != expected_activity
        or activity.get("proven") is not True
    ):
        raise ValueError("activity mode was not proven")
    if not capture_phase_is_ordered(report["capture"]):
        raise ValueError("capture phases are not monotonic")
    unit, requested = operation_spec(cell)
    operation = report["operation_evidence"]
    if (
        operation.get("unit") != unit
        or operation.get("requested_units") != requested
        or operation.get("completed_units") != requested
        or operation.get("completed") is not True
    ):
        raise ValueError("operation evidence does not prove the finite workload")
    operation_panes = operation.get("panes")
    expected_panes = expected_activity or [0]
    if (
        not isinstance(operation_panes, list)
        or [item.get("pane_index") for item in operation_panes] != expected_panes
    ):
        raise ValueError("operation pane evidence differs from target panes")
    expected_event, expected_workload = {
        "small-marker": ("marker_complete", "marker"),
        "continuing-small-output": ("output_complete", "plain"),
        "same-output-position-movement": ("movement_complete", "movement"),
        "twelve-step-outer-resize": ("resize_complete", "resize"),
        "ansi-2000-lines": ("output_complete", "ansi"),
    }[cell.operation]
    if any(
        item.get("event") != expected_event
        or item.get("workload") != expected_workload
        or item.get("completed_units") != requested
        or not all(
            isinstance(item.get(field), int) and item[field] >= 0
            for field in ("payload_bytes", "marker_bytes", "control_bytes")
        )
        for item in operation_panes
    ):
        raise ValueError("operation pane evidence does not prove exact completion")
    for item in operation_panes:
        raw = item.get("raw_result")
        if cell.operation == "same-output-position-movement":
            if raw is not None:
                raise ValueError(
                    "movement operation unexpectedly carries child evidence"
                )
            continue
        if not isinstance(raw, Mapping):
            raise TypeError("operation pane omits raw child evidence")
        numeric_fields = (
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
        if raw.get("schema") != "splinterm.benchmark.child-result.v1" or any(
            isinstance(raw.get(field), bool)
            or not isinstance(raw.get(field), int)
            or raw[field] < 0
            for field in numeric_fields
        ):
            raise ValueError("raw child evidence metadata is invalid")
        if (
            raw["pid"] == 0
            or raw["received_monotonic_ns"] > raw["completed_monotonic_ns"]
        ):
            raise ValueError("raw child evidence identity or ordering is invalid")
        if raw["total_bytes"] != (
            raw["payload_bytes"] + raw["marker_bytes"] + raw["control_bytes"]
        ):
            raise ValueError("raw child evidence byte partition is inconsistent")
        if any(
            item[field] != raw[field]
            for field in ("payload_bytes", "marker_bytes", "control_bytes")
        ):
            raise ValueError(
                "normalized operation bytes differ from raw child evidence"
            )
        if cell.operation in {"small-marker", "twelve-step-outer-resize"}:
            if not (
                raw.get("event") == "marker_complete"
                and raw.get("workload") is None
                and raw["lines"] == 0
                and raw["completed_units"] == 1
                and raw["payload_bytes"] == 0
                and raw["marker_bytes"] > 0
                and raw["control_bytes"] == 0
            ):
                raise ValueError("raw marker evidence has an invalid byte partition")
        elif cell.operation == "continuing-small-output":
            if not (
                raw.get("event") == "output_complete"
                and raw.get("workload") == "plain"
                and raw["lines"] == raw["completed_units"] == 1
                and raw["payload_bytes"] > 0
                and raw["marker_bytes"] == raw["control_bytes"] == 0
            ):
                raise ValueError("raw continuing output evidence is invalid")
        elif cell.operation == "ansi-2000-lines" and not (
            raw.get("event") == "write_complete"
            and raw.get("workload") == "ansi"
            and raw["lines"] == raw["completed_units"] == ANSI_OPERATION_LINES
            and raw["payload_bytes"] > 0
            and raw["marker_bytes"] > 0
            and raw["control_bytes"] > 0
        ):
            raise ValueError("raw ANSI output evidence is invalid")
    capture_targets = set(expected_panes)
    for attempt in report["capture"]["attempts"]:
        pixels = attempt.get("pane_marker_pixels")
        if not isinstance(pixels, list) or [
            item.get("pane_index") for item in pixels
        ] != list(range(cell.pane_count)):
            raise ValueError("capture pane evidence is incomplete")
        expected_found = all(
            item.get("pixels", 0) >= 100
            for item in pixels
            if item.get("pane_index") in capture_targets
        )
        if attempt.get("marker_found") is not expected_found:
            raise ValueError("capture marker result differs from pane evidence")
    if report.get("valid") is True:
        if report.get("failure") is not None:
            raise ValueError("valid report contains a failure")
        if report.get("cleanup", {}).get("verified") is not True:
            raise ValueError("valid report lacks verified cleanup")
        trace = report["trace"]
        if trace.get("saturated") or trace.get("ambiguous"):
            raise ValueError("valid report contains an invalid trace")
        if (
            cell.operation != "same-output-position-movement"
            and trace.get("client_receive_to_pane_commit_ns") is None
        ):
            raise ValueError("valid update report lacks a correlated primary interval")
        if cell.operation == "same-output-position-movement":
            movement = report["movement"]
            if movement.get("position_changes", 0) < 1 or any(
                movement.get(field) != 0
                for field in (
                    "configure_events",
                    "output_enter_events",
                    "output_leave_events",
                    "semantic_applies",
                    "history_clones",
                    "pane_frame_rebuilds",
                )
            ):
                raise ValueError("static movement control includes non-position work")
