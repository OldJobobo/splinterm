#!/usr/bin/env python3
"""Interleaved default-off versus opt-in publication-metrics overhead gate."""

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
from typing import Any

ROOT = pathlib.Path(__file__).resolve().parents[2]


def percentile(values: list[float], percent: int) -> float:
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * percent / 100) - 1)]


def summary(values: list[float]) -> dict[str, float | int]:
    median = statistics.median(values)
    return {"count": len(values), "min": min(values), "median": median,
            "p95": percentile(values, 95), "max": max(values),
            "mad": statistics.median(abs(value - median) for value in values)}


def bootstrap(control: list[float], candidate: list[float], seed: int, count: int) -> dict[str, float | int]:
    rng = random.Random(seed)
    values = []
    for _ in range(count):
        left = statistics.median(rng.choice(control) for _ in control)
        right = statistics.median(rng.choice(candidate) for _ in candidate)
        values.append((right / left - 1.0) * 100.0)
    return {"point_percent": (statistics.median(candidate) / statistics.median(control) - 1.0) * 100.0,
            "one_sided_95_upper_percent": percentile(values, 95), "seed": seed, "resamples": count}


def run(binary: pathlib.Path, helper: pathlib.Path, enabled: bool) -> dict[str, Any]:
    env = os.environ.copy()
    env["SPLINTERM_PTY_HELPER"] = str(helper)
    if enabled:
        env["SPLINTERM_PUBLICATION_MEMORY_METRICS"] = "1"
    else:
        env.pop("SPLINTERM_PUBLICATION_MEMORY_METRICS", None)
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    result = subprocess.run([str(binary)], cwd=ROOT, env=env, text=True, capture_output=True,
                            check=False, timeout=30)
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or f"benchmark exited {result.returncode}")
    value = json.loads(result.stdout)
    value["process_cpu_ns"] = round((after.ru_utime + after.ru_stime - before.ru_utime - before.ru_stime) * 1e9)
    return value


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=pathlib.Path)
    parser.add_argument("--binary", type=pathlib.Path, required=True)
    parser.add_argument("--helper", type=pathlib.Path, required=True)
    parser.add_argument("--warmups", type=int, default=3)
    parser.add_argument("--samples", type=int, default=15)
    parser.add_argument("--seed", type=int, default=11001)
    parser.add_argument("--bootstrap-resamples", type=int, default=10000)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    rng = random.Random(args.seed)
    records: dict[str, list[dict[str, Any]]] = {"default_off": [], "enabled": []}
    orders = []
    for phase, count in (("warmup", args.warmups), ("measured", args.samples)):
        for iteration in range(count):
            order = ["default_off", "enabled"]
            rng.shuffle(order)
            orders.append({"phase": phase, "iteration": iteration, "order": order})
            for variant in order:
                result = run(args.binary, args.helper, variant == "enabled")
                if phase == "measured":
                    records[variant].append(result)
    metrics = {"output_ns": lambda x: x["output_ns"], "process_cpu_ns": lambda x: x["process_cpu_ns"],
               "small_write_ns": lambda x: x["post_output_input_response_ns"]}
    summaries = {variant: {name: summary([float(fn(row)) for row in rows]) for name, fn in metrics.items()}
                 for variant, rows in records.items()}
    regressions = {name: bootstrap([float(fn(x)) for x in records["default_off"]],
                                   [float(fn(x)) for x in records["enabled"]], args.seed + i,
                                   args.bootstrap_resamples)
                   for i, (name, fn) in enumerate(metrics.items())}
    limits = {"output_ns": 2.0, "process_cpu_ns": 2.0, "small_write_ns": 5.0}
    failures = [f"{name} 95% upper {regressions[name]['one_sided_95_upper_percent']:.3f}% > {limit:.1f}%"
                for name, limit in limits.items() if regressions[name]["one_sided_95_upper_percent"] > limit]
    report = {"schema": "splinterm.performance.publication-metrics-overhead.v1", "valid": not failures,
              "warmups": args.warmups, "samples": args.samples, "seed": args.seed,
              "binary": {"path": str(args.binary), "sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest()},
              "helper": {"path": str(args.helper), "sha256": hashlib.sha256(args.helper.read_bytes()).hexdigest()},
              "orders": orders, "raw": records, "summaries": summaries, "regressions": regressions,
              "limits_percent": limits, "failures": failures}
    (args.output / "summary.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"valid": not failures, "regressions": regressions, "failures": failures}, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
