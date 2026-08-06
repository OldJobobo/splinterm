#!/usr/bin/env python3
"""Validate and summarize the Plan 0022 non-graphical timing harness."""

from __future__ import annotations

import argparse
import json
import math
import pathlib
import statistics
import sys
from typing import Any

import jsonschema

ROOT = pathlib.Path(__file__).resolve().parents[2]
SCHEMA = ROOT / "tools/performance/history-catchup-schema.json"

CASE_FIELDS = {
    "small-h0-live-p1-focused": (0, "live", 1, "focused-only", "small", 1),
    "small-h1000-live-p1-focused": (1000, "live", 1, "focused-only", "small", 1),
    "small-h4096-live-p1-focused": (4096, "live", 1, "focused-only", "small", 1),
    "small-h4096-detached-p1-focused": (
        4096,
        "detached",
        1,
        "focused-only",
        "small",
        1,
    ),
    "small-h4096-live-p2-focused": (4096, "live", 2, "focused-only", "small", 1),
    "small-h4096-live-p4-focused": (4096, "live", 4, "focused-only", "small", 1),
    "small-h4096-live-p4-all": (4096, "live", 4, "all-panes", "small", 1),
    "small-h4096-live-p4-inactive": (4096, "live", 4, "inactive-only", "small", 1),
    "ansi-h0-live-p1-focused": (0, "live", 1, "focused-only", "ansi-2000-lines", 2000),
    "ansi-h1000-live-p1-focused": (
        1000,
        "live",
        1,
        "focused-only",
        "ansi-2000-lines",
        2000,
    ),
    "ansi-h4096-live-p1-focused": (
        4096,
        "live",
        1,
        "focused-only",
        "ansi-2000-lines",
        2000,
    ),
    "ansi-h4096-detached-p1-focused": (
        4096,
        "detached",
        1,
        "focused-only",
        "ansi-2000-lines",
        2000,
    ),
    "ansi-h4096-live-p4-all": (4096, "live", 4, "all-panes", "ansi-2000-lines", 2000),
    "ansi-h4096-live-p4-inactive": (
        4096,
        "live",
        4,
        "inactive-only",
        "ansi-2000-lines",
        2000,
    ),
}
SMOKE_CASES = {name for name in CASE_FIELDS if name.startswith("small-")}


def load_object(path: pathlib.Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise TypeError(f"{path} does not contain a JSON object")
    return value


def validate_semantics(report: dict[str, Any]) -> None:
    smoke = report["smoke"]
    expected_names = SMOKE_CASES if smoke else set(CASE_FIELDS)
    expected_runs = (0, 1) if smoke else (5, 30)
    if (report["warmup_runs"], report["sample_runs"]) != expected_runs:
        raise ValueError("run counts differ from the fixed smoke/full contract")
    cases = report["cases"]
    names = [case["name"] for case in cases]
    if len(names) != len(set(names)) or set(names) != expected_names:
        raise ValueError("case set differs from the fixed timing matrix")
    for case in cases:
        expected = CASE_FIELDS[case["name"]]
        actual = (
            case["history_rows"],
            case["viewport"],
            case["pane_count"],
            case["activity"],
            case["update_shape"],
            case["operation_updates"],
        )
        if actual != expected:
            raise ValueError(f"case metadata mismatch: {case['name']}")
        if len(case["duration_ns"]) != report["sample_runs"]:
            raise ValueError(f"sample count mismatch: {case['name']}")


def duration_summary(values: list[int]) -> dict[str, int | float]:
    ordered = sorted(values)
    p95_index = max(0, math.ceil(len(ordered) * 0.95) - 1)
    return {
        "count": len(ordered),
        "min_ns": ordered[0],
        "median_ns": statistics.median(ordered),
        "p95_ns": ordered[p95_index],
        "max_ns": ordered[-1],
    }


def summarize(report: dict[str, Any]) -> dict[str, Any]:
    return {
        "schema": "splinterm.performance.history-catchup-summary.v1",
        "source_schema": report["schema"],
        "build_profile": report["build_profile"],
        "smoke": report["smoke"],
        "warmup_runs": report["warmup_runs"],
        "sample_runs": report["sample_runs"],
        "cases": [
            {
                **{key: value for key, value in case.items() if key != "duration_ns"},
                "duration": duration_summary(case["duration_ns"]),
            }
            for case in report["cases"]
        ],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("report", type=pathlib.Path)
    parser.add_argument("summary", type=pathlib.Path)
    args = parser.parse_args()
    try:
        report = load_object(args.report)
        schema = load_object(SCHEMA)
        jsonschema.Draft202012Validator(schema).validate(report)
        validate_semantics(report)
        value = summarize(report)
        temporary = args.summary.with_name(f".{args.summary.name}.tmp")
        temporary.write_text(
            json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        temporary.replace(args.summary)
        return 0
    except (
        OSError,
        TypeError,
        ValueError,
        json.JSONDecodeError,
        jsonschema.ValidationError,
    ) as error:
        print(f"history catch-up report error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
