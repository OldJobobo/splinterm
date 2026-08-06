#!/usr/bin/env python3
"""Validate and summarize bounded body-free Splinterm stage traces."""

from __future__ import annotations

import argparse
import json
import math
import pathlib
import statistics
import sys
import uuid
from collections import defaultdict
from typing import Any

SCHEMA_V1 = "splinterm.performance.stage.v1"
SCHEMA_V2 = "splinterm.performance.stage.v2"
CLOCK = "CLOCK_MONOTONIC_RAW shared host namespace"
COMMON_FIELDS = {
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
V2_FIELDS = {
    "commit_sequence",
    "pane_role",
    "pane_count",
    "active_pane_count",
    "columns",
    "cached_history_rows",
    "cached_history_bytes",
    "copied_history_rows",
    "copied_history_bytes",
    "history_scan_rows",
    "history_trim_rows",
    "receiver_batch_size",
    "contiguous_updates",
    "superseded_revisions",
    "dirty_rows",
    "prepared_rows",
    "prepared_cells",
    "inactive_panes_dirty",
    "inactive_panes_prepared",
    "inactive_panes_skipped",
    "inactive_panes_superseded",
    "configure_count",
    "old_width",
    "old_height",
    "final_width",
    "final_height",
    "scale_120",
    "glyph_cache_hits",
    "glyph_cache_misses",
    "image_generation",
    "backing_clear_bytes",
    "backing_copy_bytes",
    "damage_regions",
    "damage_area",
    "shm_acquire_ns",
    "buffers_available",
    "buffers_total",
    "callbacks_coalesced",
    "event_loop_active_ns",
    "output_enter_events",
    "output_leave_events",
    "scale_changed",
}
NUMERIC_FIELDS = {
    "monotonic_raw_ns",
    "sequence",
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
    "commit_sequence",
    "pane_count",
    "active_pane_count",
    "columns",
    "cached_history_rows",
    "cached_history_bytes",
    "copied_history_rows",
    "copied_history_bytes",
    "history_scan_rows",
    "history_trim_rows",
    "receiver_batch_size",
    "contiguous_updates",
    "superseded_revisions",
    "dirty_rows",
    "prepared_rows",
    "prepared_cells",
    "inactive_panes_dirty",
    "inactive_panes_prepared",
    "inactive_panes_skipped",
    "inactive_panes_superseded",
    "configure_count",
    "old_width",
    "old_height",
    "final_width",
    "final_height",
    "scale_120",
    "glyph_cache_hits",
    "glyph_cache_misses",
    "image_generation",
    "backing_clear_bytes",
    "backing_copy_bytes",
    "damage_regions",
    "damage_area",
    "shm_acquire_ns",
    "buffers_available",
    "buffers_total",
    "callbacks_coalesced",
    "event_loop_active_ns",
    "output_enter_events",
    "output_leave_events",
}
BOOLEAN_FIELDS = {"full_reload", "resync", "scale_changed"}
PANE_ROLES = {"focused", "visible-inactive", "hidden", "detached-viewport"}
PROCESSES = {"splinterm", "splinterd"}
MAX_COMPONENT_BYTES = 64
UINT32_MAX = (1 << 32) - 1
STAGE_ORDER_V1 = [
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
STAGE_ORDER_V2 = [
    *STAGE_ORDER_V1[:-2],
    "pane_commit",
    "draw_commit",
    "frame_callback",
    "window_event",
    "graphical_input",
    "tab_switch",
]
UINT64_MAX = (1 << 64) - 1


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


def transaction_key(record: dict[str, Any], schema: str) -> tuple[Any, ...] | None:
    identity = (
        record.get("splint_id"),
        record.get("incarnation"),
        record.get("revision"),
    )
    if any(value is None for value in identity):
        return None
    if schema == SCHEMA_V1:
        return identity
    correlation = (
        record.get("subscription_id"),
        record.get("transaction_sequence"),
    )
    if any(value is None for value in correlation):
        return None
    return (*identity[:2], *correlation, identity[2])


def valid_component(value: Any) -> bool:
    return (
        isinstance(value, str)
        and 0 < len(value.encode("utf-8")) <= MAX_COMPONENT_BYTES
        and all(
            character.isascii() and (character.isalnum() or character in "-_.")
            for character in value
        )
    )


def valid_splint_id(value: Any) -> bool:
    if not isinstance(value, str):
        return False
    try:
        return str(uuid.UUID(value)) == value
    except ValueError:
        return False


def validate_record(
    record: dict[str, Any], path: pathlib.Path, line_number: int, schema: str
) -> None:
    allowed = COMMON_FIELDS | (V2_FIELDS if schema == SCHEMA_V2 else set())
    unknown = set(record) - allowed
    if unknown:
        raise ValueError(
            f"{path}:{line_number}: unknown/body field(s): {sorted(unknown)}"
        )
    if record.get("schema") != schema:
        raise ValueError(f"{path}:{line_number}: mixed or wrong schema")
    if record.get("clock") != CLOCK:
        raise ValueError(f"{path}:{line_number}: wrong clock domain")
    if not valid_component(record.get("run_id")):
        raise ValueError(f"{path}:{line_number}: invalid run_id")
    if record.get("process") not in PROCESSES:
        raise ValueError(f"{path}:{line_number}: invalid process")
    pid = record.get("pid")
    if isinstance(pid, bool) or not isinstance(pid, int) or not 0 < pid <= UINT32_MAX:
        raise ValueError(f"{path}:{line_number}: invalid pid")
    required_types = {
        "sequence": int,
        "monotonic_raw_ns": int,
        "stage": str,
    }
    for field, expected in required_types.items():
        if not isinstance(record.get(field), expected):
            raise TypeError(f"{path}:{line_number}: invalid {field}")
    if "splint_id" in record and not valid_splint_id(record["splint_id"]):
        raise ValueError(f"{path}:{line_number}: invalid splint_id")
    if record["stage"] == "trace_saturated":
        raise ValueError(f"{path}:{line_number}: trace event bound was exhausted")
    stage_order = STAGE_ORDER_V2 if schema == SCHEMA_V2 else STAGE_ORDER_V1
    if record["stage"] not in stage_order:
        raise ValueError(f"{path}:{line_number}: unknown stage")
    for field in NUMERIC_FIELDS:
        if field not in record:
            continue
        value = record[field]
        if (
            isinstance(value, bool)
            or not isinstance(value, int)
            or not 0 <= value <= UINT64_MAX
        ):
            raise ValueError(f"{path}:{line_number}: invalid {field}")
    for field in BOOLEAN_FIELDS:
        if field in record and not isinstance(record[field], bool):
            raise ValueError(f"{path}:{line_number}: invalid {field}")
    if "pane_role" in record and record["pane_role"] not in PANE_ROLES:
        raise ValueError(f"{path}:{line_number}: invalid pane_role")
    if schema == SCHEMA_V2:
        correlation_fields = (
            "splint_id",
            "incarnation",
            "revision",
            "subscription_id",
            "transaction_sequence",
        )
        present = [field in record for field in correlation_fields]
        required_correlation_stages = {
            "client_receive",
            "client_enqueue",
            "client_apply",
            "pane_commit",
        }
        if record["stage"] in {"draw_commit", "frame_callback", "window_event"} and (
            any(present) or "pane_role" in record
        ):
            raise ValueError(
                f"{path}:{line_number}: surface stage carries pane correlation identity"
            )
        if record["stage"] in required_correlation_stages and not all(present):
            raise ValueError(
                f"{path}:{line_number}: incomplete transaction correlation identity"
            )
        has_transaction = (
            "subscription_id" in record or "transaction_sequence" in record
        )
        if record["stage"] == "frame_prepare" and has_transaction and not all(present):
            raise ValueError(
                f"{path}:{line_number}: incomplete transaction correlation identity"
            )
        if (
            record["stage"] in {"pane_commit", "draw_commit", "frame_callback"}
            and "commit_sequence" not in record
        ):
            raise ValueError(f"{path}:{line_number}: missing commit_sequence")
        if record["stage"] == "frame_callback" and "duration_ns" not in record:
            raise ValueError(
                f"{path}:{line_number}: frame_callback missing duration_ns"
            )
        if record["stage"] == "window_event":
            event_fields = (
                "configure_count",
                "output_enter_events",
                "output_leave_events",
            )
            present_events = [field for field in event_fields if field in record]
            if len(present_events) != 1 or record[present_events[0]] != 1:
                raise ValueError(
                    f"{path}:{line_number}: window_event must carry exactly one unit event counter"
                )
            geometry_fields = ("old_width", "old_height", "final_width", "final_height")
            has_geometry = [field in record for field in geometry_fields]
            if present_events[0] == "configure_count" and not all(has_geometry):
                raise ValueError(
                    f"{path}:{line_number}: configure window_event lacks exact geometry"
                )
            if present_events[0] != "configure_count" and any(has_geometry):
                raise ValueError(
                    f"{path}:{line_number}: output window_event carries configure geometry"
                )


def one_interval(
    items: list[dict[str, Any]], start_stage: str, end_stage: str, field: str
) -> dict[str, Any] | None:
    starts = [item for item in items if item["stage"] == start_stage]
    ends = [item for item in items if item["stage"] == end_stage]
    if len(starts) != 1 or len(ends) != 1:
        return None
    start = starts[0]["monotonic_raw_ns"]
    end = ends[0]["monotonic_raw_ns"]
    if end < start:
        raise ValueError(f"{end_stage} precedes {start_stage} for one transaction")
    return {field: end - start}


def validate_transaction_order(items: list[dict[str, Any]]) -> None:
    timestamps: dict[str, list[int]] = defaultdict(list)
    for item in items:
        timestamps[item["stage"]].append(item["monotonic_raw_ns"])
    ordered = (
        "wire_materialize",
        "client_receive",
        "client_apply",
        "frame_prepare",
        "pane_commit",
    )
    previous: int | None = None
    for stage in ordered:
        values = sorted(timestamps.get(stage, []))
        if len(values) > 1 and stage != "frame_prepare":
            raise ValueError(f"ambiguous {stage} records for one transaction")
        if not values:
            continue
        if previous is not None and values[0] < previous:
            raise ValueError(f"impossible transaction stage order at {stage}")
        previous = values[-1]
    enqueue = timestamps.get("client_enqueue", [])
    receive = timestamps.get("client_receive", [])
    if len(enqueue) > 1:
        raise ValueError("ambiguous client_enqueue records for one transaction")
    if enqueue and receive and enqueue[0] < receive[0]:
        raise ValueError("client_enqueue precedes client_receive")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("trace_dir", type=pathlib.Path)
    parser.add_argument("output", type=pathlib.Path)
    parser.add_argument("--run-id", required=True)
    args = parser.parse_args()

    records: list[dict[str, Any]] = []
    schema: str | None = None
    files = sorted(args.trace_dir.glob(f"{args.run_id}-*.jsonl"))
    if not files:
        parser.error("no trace files matched the run ID")
    for path in files:
        for line_number, line in enumerate(
            path.read_text(encoding="utf-8").splitlines(), 1
        ):
            record = json.loads(line)
            record_schema = record.get("schema")
            if record_schema not in {SCHEMA_V1, SCHEMA_V2}:
                raise ValueError(f"{path}:{line_number}: wrong schema")
            if schema is None:
                schema = record_schema
            if record.get("run_id") != args.run_id:
                raise ValueError(f"{path}:{line_number}: wrong run ID")
            validate_record(record, path, line_number, schema)
            records.append(record)
    if schema is None:
        raise ValueError("trace files contained no records")

    sequences: dict[tuple[str, int], list[int]] = defaultdict(list)
    for record in records:
        sequences[(record["process"], record["pid"])].append(record["sequence"])
    for process, values in sequences.items():
        if sorted(values) != list(range(len(values))):
            raise ValueError(f"non-contiguous or reused trace sequence for {process}")

    stage_order = STAGE_ORDER_V2 if schema == SCHEMA_V2 else STAGE_ORDER_V1
    by_stage: dict[str, list[dict[str, Any]]] = defaultdict(list)
    by_transaction: dict[tuple[Any, ...], list[dict[str, Any]]] = defaultdict(list)
    for record in records:
        by_stage[record["stage"]].append(record)
        key = transaction_key(record, schema)
        if key is not None:
            by_transaction[key].append(record)

    stages = {}
    for stage in stage_order:
        items = by_stage.get(stage, [])
        if not items:
            continue
        durations = [item["duration_ns"] for item in items if "duration_ns" in item]
        totals = {
            field: sum(item.get(field, 0) for item in items)
            for field in sorted(NUMERIC_FIELDS - {"monotonic_raw_ns", "sequence"})
            if any(field in item for item in items)
        }
        stages[stage] = {
            "records": len(items),
            "duration": duration_summary(durations) if durations else None,
            "bytes": sum(item.get("bytes", 0) for item in items),
            "rows": sum(item.get("rows", 0) for item in items),
            "cells": sum(item.get("cells", 0) for item in items),
            "max_queue_depth": max(
                (item.get("queue_depth", 0) for item in items), default=0
            ),
            "full_reloads": sum(item.get("full_reload") is True for item in items),
            "resyncs": sum(item.get("resync") is True for item in items),
        }
        if schema == SCHEMA_V2:
            stages[stage]["totals"] = totals

    wire_to_commit = []
    client_receive_to_commit = []
    uncommitted_transactions = []
    committed_transactions = 0
    for key, items in by_transaction.items():
        if schema == SCHEMA_V2:
            validate_transaction_order(items)
            receives = [item for item in items if item["stage"] == "client_receive"]
            commits = [item for item in items if item["stage"] == "pane_commit"]
            if commits and not receives:
                raise ValueError("pane_commit has no matching client_receive")
            if receives:
                if len(receives) != 1:
                    raise ValueError(
                        "ambiguous client_receive records for one transaction"
                    )
                if commits:
                    committed_transactions += 1
                else:
                    last = max(items, key=lambda item: item["monotonic_raw_ns"])
                    uncommitted_transactions.append(
                        {"key": key, "last_stage": last["stage"]}
                    )
        interval = one_interval(
            items,
            "wire_materialize",
            "pane_commit" if schema == SCHEMA_V2 else "draw_commit",
            "wire_to_commit_ns",
        )
        if interval is not None:
            wire_to_commit.append({"key": key, **interval})
        interval = one_interval(
            items,
            "client_receive",
            "pane_commit",
            "client_receive_to_commit_ns",
        )
        if interval is not None:
            client_receive_to_commit.append({"key": key, **interval})

    commit_to_callback = []
    if schema == SCHEMA_V2:
        by_commit: dict[tuple[str, int, int], list[dict[str, Any]]] = defaultdict(list)
        for record in records:
            if "commit_sequence" in record:
                by_commit[
                    (record["process"], record["pid"], record["commit_sequence"])
                ].append(record)
        for key, items in by_commit.items():
            draws = [item for item in items if item["stage"] == "draw_commit"]
            pane_commits = [item for item in items if item["stage"] == "pane_commit"]
            callbacks = [item for item in items if item["stage"] == "frame_callback"]
            if len(draws) != 1:
                raise ValueError(f"commit {key} does not have exactly one draw_commit")
            if len(callbacks) > 1:
                raise ValueError(f"commit {key} has ambiguous frame callbacks")
            draw = draws[0]
            if any(
                pane["monotonic_raw_ns"] != draw["monotonic_raw_ns"]
                for pane in pane_commits
            ):
                raise ValueError(
                    f"commit {key} pane_commit timestamp differs from draw"
                )
            if not callbacks:
                continue
            callback = callbacks[0]
            elapsed = callback["monotonic_raw_ns"] - draw["monotonic_raw_ns"]
            if elapsed < 0:
                raise ValueError(f"commit {key} callback precedes draw")
            if callback["duration_ns"] != elapsed:
                raise ValueError(
                    f"commit {key} callback duration differs from timestamp delta"
                )
            commit_to_callback.append({"key": key, "commit_to_callback_ns": elapsed})

    def correlated_summary(items: list[dict[str, Any]], field: str) -> dict[str, Any]:
        return {
            "count": len(items),
            "duration": duration_summary([item[field] for item in items])
            if items
            else None,
        }

    report = {
        "schema": f"splinterm.performance.stage-summary.v{2 if schema == SCHEMA_V2 else 1}",
        "valid": True,
        "run_id": args.run_id,
        "clock": CLOCK,
        "files": [str(path) for path in files],
        "record_count": len(records),
        "processes": sorted({(item["process"], item["pid"]) for item in records}),
        "stage_order": stage_order,
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
        "correlated_wire_to_commit": correlated_summary(
            wire_to_commit, "wire_to_commit_ns"
        ),
    }
    if schema == SCHEMA_V2:
        report["trace_schema"] = schema
        report["interval_ownership"].update(
            {
                "pane_commit": "one body-free pane transaction associated with the surface commit boundary",
                "frame_callback": "commit-to-compositor callback wait; not presentation latency",
                "tab_switch": "client-local tab activation and bounded state replacement",
                "window_event": "transaction-free Wayland configure or surface output-enter/output-leave callback count",
            }
        )
        report["correlated_client_receive_to_commit"] = correlated_summary(
            client_receive_to_commit, "client_receive_to_commit_ns"
        )
        report["correlated_commit_to_callback"] = correlated_summary(
            commit_to_callback, "commit_to_callback_ns"
        )
        report["transactions"] = {
            "committed": committed_transactions,
            "uncommitted": len(uncommitted_transactions),
            "uncommitted_records": uncommitted_transactions,
        }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"records": len(records), "stages": stages}, indent=2))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, TypeError, ValueError, json.JSONDecodeError) as error:
        print(f"stage trace error: {error}", file=sys.stderr)
        raise SystemExit(1)
