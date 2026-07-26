#!/usr/bin/env python3
"""Interleaved high-resolution overhead gate for opt-in stage instrumentation."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import pathlib
import random
import resource
import statistics
import subprocess
import sys
from typing import Any

ROOT = pathlib.Path(__file__).resolve().parents[2]


def identity(path: pathlib.Path) -> dict[str, Any]:
    return {
        "path": str(path),
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "size_bytes": path.stat().st_size,
    }


def percentile(values: list[float], numerator: int) -> float:
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * numerator / 100) - 1)]


def summary(values: list[float]) -> dict[str, float | int]:
    median = statistics.median(values)
    return {
        "count": len(values),
        "min": min(values),
        "median": median,
        "p95": percentile(values, 95),
        "max": max(values),
        "mad": statistics.median(abs(value - median) for value in values),
    }


def bootstrap_regression(
    control: list[float], candidate: list[float], seed: int, resamples: int
) -> dict[str, float | int]:
    rng = random.Random(seed)
    point = (statistics.median(candidate) / statistics.median(control) - 1.0) * 100.0
    deltas = []
    for _ in range(resamples):
        control_sample = [rng.choice(control) for _ in control]
        candidate_sample = [rng.choice(candidate) for _ in candidate]
        deltas.append(
            (
                statistics.median(candidate_sample)
                / statistics.median(control_sample)
                - 1.0
            )
            * 100.0
        )
    return {
        "point_percent": point,
        "one_sided_95_upper_percent": percentile(deltas, 95),
        "seed": seed,
        "resamples": resamples,
    }


def run_once(binary: pathlib.Path, helper: pathlib.Path) -> dict[str, Any]:
    environment = os.environ.copy()
    environment.pop("SPLINTERM_PERF_TRACE_DIR", None)
    environment.pop("SPLINTERM_PERF_RUN_ID", None)
    environment.pop("SPLINTERM_PERF_TRACE_MAX_EVENTS", None)
    environment["SPLINTERM_PTY_HELPER"] = str(helper)
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    result = subprocess.run(
        [str(binary)],
        cwd=ROOT,
        env=environment,
        text=True,
        capture_output=True,
        check=False,
        timeout=30,
    )
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    if result.returncode:
        raise RuntimeError(
            f"benchmark failed ({result.returncode}): {result.stderr.strip()}"
        )
    report = json.loads(result.stdout)
    report["process_cpu_ns"] = round(
        (
            after.ru_utime
            + after.ru_stime
            - before.ru_utime
            - before.ru_stime
        )
        * 1_000_000_000
    )
    return report


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=pathlib.Path)
    parser.add_argument("--control", type=pathlib.Path, required=True)
    parser.add_argument("--control-helper", type=pathlib.Path, required=True)
    parser.add_argument("--candidate", type=pathlib.Path, required=True)
    parser.add_argument("--candidate-helper", type=pathlib.Path, required=True)
    parser.add_argument("--warmups", type=int, default=3)
    parser.add_argument("--samples", type=int, default=10)
    parser.add_argument("--seed", type=int, default=10_013)
    parser.add_argument("--bootstrap-resamples", type=int, default=10_000)
    args = parser.parse_args()
    if min(args.warmups, args.samples, args.bootstrap_resamples) <= 0:
        parser.error("warmups, samples, and bootstrap resamples must be positive")
    paths = [args.control, args.control_helper, args.candidate, args.candidate_helper]
    if any(not path.is_file() for path in paths):
        parser.error("every binary/helper path must be an existing file")

    args.output.mkdir(parents=True, exist_ok=True)
    rng = random.Random(args.seed)
    records: dict[str, list[dict[str, Any]]] = {"control": [], "candidate": []}
    orders = []
    for phase, count in (("warmup", args.warmups), ("measured", args.samples)):
        for iteration in range(count):
            order = ["control", "candidate"]
            rng.shuffle(order)
            orders.append({"phase": phase, "iteration": iteration, "order": order})
            for variant in order:
                binary = args.control if variant == "control" else args.candidate
                helper = (
                    args.control_helper
                    if variant == "control"
                    else args.candidate_helper
                )
                report = run_once(binary, helper)
                if phase == "measured":
                    records[variant].append(report)

    metrics = {
        "output_ns": lambda item: item["output_ns"],
        "process_cpu_ns": lambda item: item["process_cpu_ns"],
        "small_write_ns": lambda item: item["post_output_input_response_ns"],
    }
    summaries = {
        variant: {
            name: summary([float(metric(item)) for item in items])
            for name, metric in metrics.items()
        }
        for variant, items in records.items()
    }
    regressions = {
        name: bootstrap_regression(
            [float(metric(item)) for item in records["control"]],
            [float(metric(item)) for item in records["candidate"]],
            args.seed + index,
            args.bootstrap_resamples,
        )
        for index, (name, metric) in enumerate(metrics.items())
    }
    limits = {"output_ns": 2.0, "process_cpu_ns": 2.0, "small_write_ns": 5.0}
    failures = [
        f"{name} upper regression exceeds {limit:.1f}%"
        for name, limit in limits.items()
        if regressions[name]["one_sided_95_upper_percent"] > limit
    ]
    report = {
        "schema": "splinterm.performance.instrumentation-overhead.v1",
        "valid": not failures,
        "clock": "getrusage RUSAGE_CHILDREN high-resolution process CPU",
        "trace_environment": "disabled for both builds",
        "warmups": args.warmups,
        "samples": args.samples,
        "order_seed": args.seed,
        "orders": orders,
        "binaries": {
            "control": identity(args.control),
            "control_helper": identity(args.control_helper),
            "candidate": identity(args.candidate),
            "candidate_helper": identity(args.candidate_helper),
        },
        "summaries": summaries,
        "regressions": regressions,
        "limits_percent": limits,
        "failures": failures,
    }
    path = args.output / "summary.json"
    path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"valid": not failures, "regressions": regressions}, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(f"stage overhead error: {error}", file=sys.stderr)
        raise SystemExit(1)
