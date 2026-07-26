#!/usr/bin/env python3
"""Validate and summarize bounded body-free Splinterm stage traces."""

from __future__ import annotations

import argparse
import json
import math
import pathlib
import statistics
import sys
from collections import defaultdict
from typing import Any

SCHEMA = "splinterm.performance.stage.v1"
CLOCK = "CLOCK_MONOTONIC_RAW shared host namespace"
ALLOWED = {
    "schema",
    "run_id",
    "process",
    "pid",
    "sequence",
    "clock",
    "monotonic_raw_ns",
    "stage",
    "splint_id",
    "incarnation",
    "base_revision",
    "revision",
    "subscription_id",
    "transaction_sequence",
    "duration_ns",
    "queue_wait_ns",
    "bytes",
    "rows",
    "cells",
    "count",
    "queue_depth",
    "full_reload",
    "resync",
}
STAGE_ORDER = [
    "terminal_mutation",
    "owned_snapshot",
    "daemon_publication",
    "wire_materialize",
    "frame_encode",
    "socket_write",
    "client_receive",
    "client_enqueue",
    "client_apply",
    "frame_prepare",
    "draw_commit",
    "graphical_input",
]


def percentile(values: list[int], numerator: int) -> int:
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * numerator / 100) - 1)]


def duration_summary(values: list[int]) -> dict[str, int]:
    return {
        "count": len(values),
        "total_ns": sum(values),
        "median_ns": int(statistics.median(values)),
        "p95_ns": percentile(values, 95),
        "max_ns": max(values),
    }


def transaction_key(record: dict[str, Any]) -> tuple[Any, ...] | None:
    revision = record.get("revision")
    if revision is None:
        return None
    splint_id = record.get("splint_id")
    incarnation = record.get("incarnation")
    if splint_id is None or incarnation is None:
        return None
    return (splint_id, incarnation, revision)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("trace_dir", type=pathlib.Path)
    parser.add_argument("output", type=pathlib.Path)
    parser.add_argument("--run-id", required=True)
    args = parser.parse_args()

    records = []
    files = sorted(args.trace_dir.glob(f"{args.run_id}-*.jsonl"))
    if not files:
        parser.error("no trace files matched the run ID")
    for path in files:
        for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            record = json.loads(line)
            unknown = set(record) - ALLOWED
            if unknown:
                raise ValueError(f"{path}:{line_number}: unknown/body field(s): {sorted(unknown)}")
            if record.get("schema") != SCHEMA:
                raise ValueError(f"{path}:{line_number}: wrong schema")
            if record.get("run_id") != args.run_id:
                raise ValueError(f"{path}:{line_number}: wrong run ID")
            if record.get("clock") != CLOCK:
                raise ValueError(f"{path}:{line_number}: wrong clock domain")
            required_types = {
                "process": str,
                "pid": int,
                "sequence": int,
                "monotonic_raw_ns": int,
                "stage": str,
            }
            for field, expected in required_types.items():
                if not isinstance(record.get(field), expected):
                    raise ValueError(f"{path}:{line_number}: invalid {field}")
            if record["stage"] == "trace_saturated":
                raise ValueError(f"{path}:{line_number}: trace event bound was exhausted")
            if record["stage"] not in STAGE_ORDER:
                raise ValueError(f"{path}:{line_number}: unknown stage")
            for field in (
                "monotonic_raw_ns",
                "sequence",
                "duration_ns",
                "queue_wait_ns",
                "bytes",
                "rows",
                "cells",
                "count",
                "queue_depth",
            ):
                if field in record and (
                    isinstance(record[field], bool)
                    or not isinstance(record[field], int)
                    or record[field] < 0
                ):
                    raise ValueError(f"{path}:{line_number}: invalid {field}")
            for field in ("full_reload", "resync"):
                if field in record and not isinstance(record[field], bool):
                    raise ValueError(f"{path}:{line_number}: invalid {field}")
            records.append(record)

    sequences: dict[tuple[str, int], list[int]] = defaultdict(list)
    for record in records:
        sequences[(record["process"], record["pid"])].append(record["sequence"])
    for process, values in sequences.items():
        if values != list(range(len(values))):
            raise ValueError(f"non-contiguous or reused trace sequence for {process}")

    by_stage: dict[str, list[dict[str, Any]]] = defaultdict(list)
    by_transaction: dict[tuple[Any, ...], list[dict[str, Any]]] = defaultdict(list)
    for record in records:
        by_stage[record["stage"]].append(record)
        key = transaction_key(record)
        if key is not None:
            by_transaction[key].append(record)

    stages = {}
    for stage in STAGE_ORDER:
        items = by_stage.get(stage, [])
        if not items:
            continue
        durations = [item["duration_ns"] for item in items if "duration_ns" in item]
        stages[stage] = {
            "records": len(items),
            "duration": duration_summary(durations) if durations else None,
            "bytes": sum(item.get("bytes", 0) for item in items),
            "rows": sum(item.get("rows", 0) for item in items),
            "cells": sum(item.get("cells", 0) for item in items),
            "max_queue_depth": max((item.get("queue_depth", 0) for item in items), default=0),
            "full_reloads": sum(item.get("full_reload") is True for item in items),
            "resyncs": sum(item.get("resync") is True for item in items),
        }

    correlated = []
    for key, items in by_transaction.items():
        names = {item["stage"] for item in items}
        if "wire_materialize" not in names or "draw_commit" not in names:
            continue
        starts = [
            item["monotonic_raw_ns"]
            for item in items
            if item["stage"] == "wire_materialize"
        ]
        ends = [
            item["monotonic_raw_ns"]
            for item in items
            if item["stage"] == "draw_commit"
        ]
        if len(starts) != 1 or len(ends) != 1:
            continue
        start = starts[0]
        end = ends[0]
        if end >= start:
            correlated.append({"key": key, "wire_to_commit_ns": end - start})

    report = {
        "schema": "splinterm.performance.stage-summary.v1",
        "valid": True,
        "run_id": args.run_id,
        "clock": CLOCK,
        "files": [str(path) for path in files],
        "record_count": len(records),
        "processes": sorted({(item["process"], item["pid"]) for item in records}),
        "stage_order": STAGE_ORDER,
        "interval_ownership": {
            "terminal_mutation": "terminal parser/state active work only",
            "owned_snapshot": "borrowed terminal state to owned LiveSnapshot",
            "daemon_publication": "subscriber history lookup, snapshot, and queue admission",
            "wire_materialize": "LiveSnapshot/update batch to one bounded protocol event",
            "frame_encode": "protocol event JSON/frame encoding",
            "socket_write": "awaited local-socket write including backpressure",
            "client_receive": "timestamp after complete frame decode/classification",
            "client_enqueue": "awaited bounded Wayland update-queue admission",
            "client_apply": "protocol update validation and snapshot mutation",
            "frame_prepare": "display snapshot plus full/incremental SnapshotFrame preparation",
            "draw_commit": "SHM acquisition, composition/copy, damage, attach, and wl_surface.commit",
            "graphical_input": "development-only graphical client input queued after committed revisions",
        },
        "stages": stages,
        "correlated_wire_to_commit": {
            "count": len(correlated),
            "duration": duration_summary(
                [item["wire_to_commit_ns"] for item in correlated]
            )
            if correlated
            else None,
        },
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"records": len(records), "stages": stages}, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"stage trace error: {error}", file=sys.stderr)
        raise SystemExit(1)
