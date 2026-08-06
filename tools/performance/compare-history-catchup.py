#!/usr/bin/env python3
"""Bootstrap-compare fixed Plan 0022 terminal-state timing reports."""

from __future__ import annotations

import argparse
import importlib.util
import json
import math
import pathlib
import random
import sys
from typing import Any

import jsonschema

ROOT = pathlib.Path(__file__).resolve().parents[2]
SUMMARY_SCRIPT = ROOT / "tools/performance/summarize-history-catchup.py"
SCHEMA = ROOT / "tools/performance/history-catchup-schema.json"
BOOTSTRAP_SEED = 220022
BOOTSTRAP_RESAMPLES = 20_000


def load_summary_module():
    spec = importlib.util.spec_from_file_location(
        "history_catchup_contract", SUMMARY_SCRIPT
    )
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load history catch-up contract")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


CONTRACT = load_summary_module()


def load_report(path: pathlib.Path) -> dict[str, Any]:
    report = CONTRACT.load_object(path)
    jsonschema.Draft202012Validator(CONTRACT.load_object(SCHEMA)).validate(report)
    CONTRACT.validate_semantics(report)
    if report["smoke"] or report["build_profile"] != "release":
        raise ValueError("comparison requires full release reports")
    return report


def nearest_p95(values: list[int]) -> int:
    return sorted(values)[max(0, math.ceil(len(values) * 0.95) - 1)]


def upper_ratio_bound(control: list[int], candidate: list[int], seed: int) -> float:
    rng = random.Random(seed)
    ratios = []
    for _ in range(BOOTSTRAP_RESAMPLES):
        control_sample = [control[rng.randrange(len(control))] for _ in control]
        candidate_sample = [candidate[rng.randrange(len(candidate))] for _ in candidate]
        ratios.append(nearest_p95(candidate_sample) / nearest_p95(control_sample))
    return sorted(ratios)[math.ceil(0.95 * len(ratios)) - 1]


def compare(control: dict[str, Any], candidate: dict[str, Any]) -> dict[str, Any]:
    control_cases = {case["name"]: case for case in control["cases"]}
    candidate_cases = {case["name"]: case for case in candidate["cases"]}
    if set(control_cases) != set(candidate_cases):
        raise ValueError("control and candidate case sets differ")
    cases = []
    for index, name in enumerate(control_cases):
        before = control_cases[name]
        after = candidate_cases[name]
        for field in (
            "history_rows",
            "viewport",
            "pane_count",
            "activity",
            "update_shape",
            "operation_updates",
        ):
            if before[field] != after[field]:
                raise ValueError(
                    f"control and candidate metadata differ: {name}/{field}"
                )
        control_p95 = nearest_p95(before["duration_ns"])
        candidate_p95 = nearest_p95(after["duration_ns"])
        ratio = candidate_p95 / control_p95
        upper_ratio = upper_ratio_bound(
            before["duration_ns"], after["duration_ns"], BOOTSTRAP_SEED + index
        )
        cases.append(
            {
                "name": name,
                "control_p95_ns": control_p95,
                "candidate_p95_ns": candidate_p95,
                "candidate_control_ratio": ratio,
                "candidate_control_ratio_one_sided_95_upper": upper_ratio,
                "improvement": 1.0 - ratio,
                "improvement_one_sided_95_lower": 1.0 - upper_ratio,
            }
        )
    return {
        "schema": "splinterm.performance.history-catchup-comparison.v1",
        "bootstrap_seed": BOOTSTRAP_SEED,
        "bootstrap_resamples": BOOTSTRAP_RESAMPLES,
        "confidence": "one-sided 95% bootstrap upper bound on candidate/control p95 ratio",
        "cases": cases,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("control", type=pathlib.Path)
    parser.add_argument("candidate", type=pathlib.Path)
    parser.add_argument("output", type=pathlib.Path)
    args = parser.parse_args()
    try:
        value = compare(load_report(args.control), load_report(args.candidate))
        temporary = args.output.with_name(f".{args.output.name}.tmp")
        temporary.write_text(
            json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        temporary.replace(args.output)
        return 0
    except (
        OSError,
        RuntimeError,
        TypeError,
        ValueError,
        json.JSONDecodeError,
        jsonschema.ValidationError,
    ) as error:
        print(f"history catch-up comparison error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
