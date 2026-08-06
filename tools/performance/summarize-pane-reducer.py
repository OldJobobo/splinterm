#!/usr/bin/env python3
"""Validate and summarize the bounded Plan 0022 PaneView reducer harness."""

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
SCHEMA = ROOT / "tools/performance/pane-reducer-schema.json"
FOCUSED_SCOPE = (
    "PaneView semantic reducer only; not the full App::apply_updates active path"
)


def expected_cases(smoke: bool) -> dict[str, tuple[str, int, str, int]]:
    histories = [(0, "live"), (4096, "live")]
    batches = [1, 16]
    if not smoke:
        histories = [(0, "live"), (1000, "live"), (4096, "live"), (4096, "detached")]
        batches = [1, 16, 64]
    return {
        f"{mode}-h{history}-{viewport}-b{batch}": (mode, history, viewport, batch)
        for history, viewport in histories
        for batch in batches
        for mode in ("focused-role", "inactive-batch")
    }


def load_object(path: pathlib.Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise TypeError(f"{path} does not contain a JSON object")
    return value


def validate_semantics(report: dict[str, Any]) -> None:
    smoke = report["smoke"]
    if (report["warmup_runs"], report["sample_runs"]) != ((0, 1) if smoke else (5, 30)):
        raise ValueError("run counts differ from the fixed smoke/full contract")
    expected = expected_cases(smoke)
    names = [case["name"] for case in report["cases"]]
    if len(names) != len(set(names)) or set(names) != set(expected):
        raise ValueError("case set differs from the fixed reducer matrix")
    if report["focused_role_scope"] != FOCUSED_SCOPE:
        raise ValueError("focused-role scope disclaimer changed")
    for case in report["cases"]:
        actual = (
            case["mode"],
            case["history_rows"],
            case["viewport"],
            case["batch_size"],
        )
        if actual != expected[case["name"]]:
            raise ValueError(f"case metadata mismatch: {case['name']}")
        if len(case["duration_ns"]) != report["sample_runs"]:
            raise ValueError(f"sample count mismatch: {case['name']}")


def duration_summary(values: list[int]) -> dict[str, int | float]:
    ordered = sorted(values)
    return {
        "count": len(ordered),
        "min_ns": ordered[0],
        "median_ns": statistics.median(ordered),
        "p95_ns": ordered[max(0, math.ceil(len(ordered) * 0.95) - 1)],
        "max_ns": ordered[-1],
    }


def summarize(report: dict[str, Any]) -> dict[str, Any]:
    return {
        "schema": "splinterm.performance.pane-reducer-summary.v1",
        "source_schema": report["schema"],
        "build_profile": report["build_profile"],
        "smoke": report["smoke"],
        "warmup_runs": report["warmup_runs"],
        "sample_runs": report["sample_runs"],
        "focused_role_scope": report["focused_role_scope"],
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
        jsonschema.Draft202012Validator(load_object(SCHEMA)).validate(report)
        validate_semantics(report)
        temporary = args.summary.with_name(f".{args.summary.name}.tmp")
        temporary.write_text(
            json.dumps(summarize(report), indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
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
        print(f"pane reducer report error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
