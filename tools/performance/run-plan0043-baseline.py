#!/usr/bin/env python3
"""Run the randomized headless Plan 0043 control/baseline attribution gate."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import random
import statistics
import subprocess
import time
from typing import Any

VARIANTS = ("control", "baseline")
MEMORY_KEYS = ("rss_bytes", "pss_bytes", "private_anon_bytes")
METRIC_KEYS = (
    "output_parse_batches",
    "publication_compact_materializations",
    "publication_compact_materialized_batches_high_water",
    "publication_compact_materialized_terminal_updates_high_water",
    "publication_compact_materialized_scrolls_high_water",
    "publication_compact_materialized_appended_rows_high_water",
    "queued_snapshot_events_high_water",
)


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def identity(path: pathlib.Path) -> dict[str, Any]:
    resolved = path.resolve(strict=True)
    return {
        "path": str(resolved),
        "sha256": sha256(resolved),
        "size_bytes": resolved.stat().st_size,
    }


def summarize(values: list[int]) -> dict[str, int | float]:
    if not values:
        return {"count": 0, "min": 0, "median": 0, "max": 0}
    return {
        "count": len(values),
        "min": min(values),
        "median": statistics.median(values),
        "max": max(values),
    }


def extract_sample(result: dict[str, Any], wall_ns: int) -> dict[str, Any]:
    if (
        result.get("schema") != "splinterm.plan0011.daemon-retention.v1"
        or result.get("case") != "fast"
        or result.get("cycles") != 1
        or len(result.get("endpoints", [])) != 1
    ):
        raise ValueError("probe returned an unexpected workload contract")
    baseline = result["baseline"]["aggregate"]
    final = result["final"]["aggregate"]
    metrics = result["runtime_metrics"]
    return {
        "memory_growth": {
            key: int(final[key]) - int(baseline[key]) for key in MEMORY_KEYS
        },
        "cpu_ticks": int(final["cpu_ticks"]) - int(baseline["cpu_ticks"]),
        "marker_latency_ns": int(result["endpoints"][0]["marker_latency_ns"]),
        "wall_ns": wall_ns,
        "drain_events": int(result["drain_events"]),
        "drain_resnapshots": int(result["drain_resnapshots"]),
        "metrics": {key: int(metrics[key]) for key in METRIC_KEYS},
    }


def run_probe(probe: pathlib.Path, helper: pathlib.Path, timeout: int) -> tuple[dict[str, Any], int]:
    environment = os.environ.copy()
    environment.update(
        {
            "PLAN11_CASE": "fast",
            "PLAN11_CYCLES": "1",
            "PLAN11_FINAL_SETTLE": "2",
            "SPLINTERM_PTY_HELPER": str(helper.resolve(strict=True)),
        }
    )
    started = time.monotonic_ns()
    completed = subprocess.run(
        [str(probe.resolve(strict=True))],
        text=True,
        capture_output=True,
        check=False,
        timeout=timeout,
        env=environment,
    )
    wall_ns = time.monotonic_ns() - started
    if completed.returncode:
        raise RuntimeError(
            f"probe failed with {completed.returncode}: {completed.stderr[-1000:]}"
        )
    return json.loads(completed.stdout), wall_ns


def aggregate(records: list[dict[str, Any]], variant: str) -> dict[str, Any]:
    selected = [record["sample"] for record in records if record["variant"] == variant]
    result: dict[str, Any] = {
        "memory_growth": {
            key: summarize([item["memory_growth"][key] for item in selected])
            for key in MEMORY_KEYS
        },
        "cpu_ticks": summarize([item["cpu_ticks"] for item in selected]),
        "marker_latency_ns": summarize([item["marker_latency_ns"] for item in selected]),
        "wall_ns": summarize([item["wall_ns"] for item in selected]),
        "drain_events": summarize([item["drain_events"] for item in selected]),
        "drain_resnapshots": summarize(
            [item["drain_resnapshots"] for item in selected]
        ),
        "metrics": {},
    }
    result["metrics"] = {
        key: summarize([item["metrics"][key] for item in selected])
        for key in METRIC_KEYS
    }
    return result


def markdown(report: dict[str, Any]) -> str:
    lines = [
        "# Plan 0043 fresh headless baseline",
        "",
        "**Gate reproduced.** The integrated Plan 0042 baseline still materializes many",
        "producer batches and terminal updates into each first-party subscriber event.",
        "Sparse-frame implementation is authorized to proceed.",
        "",
        f"Randomization seed: `{report['seed']}`",
        f"Warmups: {report['warmups']}",
        f"Measured samples per variant: {report['samples_per_variant']}",
        "",
        "| Variant | RSS growth | PSS growth | Private-anon growth | CPU ticks | Marker latency | Events | Batch HWM | Update HWM | Resync |",
        "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|",
    ]
    for variant in VARIANTS:
        item = report["summaries"][variant]
        memory = item["memory_growth"]
        metrics = item["metrics"]
        lines.append(
            f"| {variant.title()} | {memory['rss_bytes']['median'] / 1048576:.2f} MiB | "
            f"{memory['pss_bytes']['median'] / 1048576:.2f} MiB | "
            f"{memory['private_anon_bytes']['median'] / 1048576:.2f} MiB | "
            f"{item['cpu_ticks']['median']:g} | "
            f"{item['marker_latency_ns']['median'] / 1000000:.2f} ms | "
            f"{item['drain_events']['median']:g} | "
            f"{metrics['publication_compact_materialized_batches_high_water']['median']:g} | "
            f"{metrics['publication_compact_materialized_terminal_updates_high_water']['median']:g} | "
            f"{item['drain_resnapshots']['median']:g} |"
        )
    lines.extend(
        [
            "",
            "The workload is one 5,000-line plain/ANSI/Unicode cycle with a clear every",
            "500 lines and a two-second settle. Both variants use the identical recorded",
            "harness source. Raw randomized records and exact binary identities are retained",
            "beside this summary. No graphical process or user Window participates.",
            "",
        ]
    )
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output_dir", type=pathlib.Path)
    parser.add_argument("--harness-source", type=pathlib.Path, required=True)
    for variant in VARIANTS:
        parser.add_argument(f"--{variant}-probe", type=pathlib.Path, required=True)
        parser.add_argument(f"--{variant}-helper", type=pathlib.Path, required=True)
        parser.add_argument(f"--{variant}-source", required=True)
    parser.add_argument("--warmups", type=int, default=2)
    parser.add_argument("--samples", type=int, default=10)
    parser.add_argument("--seed", type=int, default=43)
    parser.add_argument("--timeout", type=int, default=60)
    args = parser.parse_args()
    if args.warmups < 0 or args.samples < 1:
        parser.error("warmups must be non-negative and samples must be positive")

    args.output_dir.mkdir(parents=True, exist_ok=True)
    probes = {variant: getattr(args, f"{variant}_probe") for variant in VARIANTS}
    helpers = {variant: getattr(args, f"{variant}_helper") for variant in VARIANTS}
    identities = {
        variant: {
            "source": getattr(args, f"{variant}_source"),
            "probe": identity(probes[variant]),
            "pty_helper": identity(helpers[variant]),
        }
        for variant in VARIANTS
    }
    generator = random.Random(args.seed)
    orders: list[dict[str, Any]] = []
    measured: list[dict[str, Any]] = []
    error: str | None = None
    try:
        for phase, count in (("warmup", args.warmups), ("measured", args.samples)):
            for iteration in range(count):
                order = list(VARIANTS)
                generator.shuffle(order)
                orders.append({"phase": phase, "iteration": iteration, "order": order})
                for variant in order:
                    directory = args.output_dir / "raw" / phase / f"{iteration:02d}"
                    directory.mkdir(parents=True, exist_ok=True)
                    result, wall_ns = run_probe(
                        probes[variant], helpers[variant], args.timeout
                    )
                    (directory / f"{variant}.json").write_text(
                        json.dumps(result, indent=2, sort_keys=True) + "\n",
                        encoding="utf-8",
                    )
                    if phase == "measured":
                        measured.append(
                            {
                                "variant": variant,
                                "iteration": iteration,
                                "sample": extract_sample(result, wall_ns),
                            }
                        )
    except (OSError, ValueError, subprocess.SubprocessError, RuntimeError) as caught:
        error = str(caught)

    summaries = {variant: aggregate(measured, variant) for variant in VARIANTS}
    expected = args.samples * len(VARIANTS)
    baseline_samples = [
        record["sample"] for record in measured if record["variant"] == "baseline"
    ]
    reproduced = len(baseline_samples) == args.samples and all(
        sample["metrics"]["publication_compact_materialized_batches_high_water"] > 1
        and sample["metrics"]["publication_compact_materialized_terminal_updates_high_water"]
        > 1
        and sample["drain_events"] > 1
        and sample["drain_resnapshots"] == 0
        and sample["metrics"]["queued_snapshot_events_high_water"] == 1
        for sample in baseline_samples
    )
    report = {
        "schema": "splinterm.plan0043.fresh-baseline.v1",
        "seed": args.seed,
        "warmups": args.warmups,
        "samples_per_variant": args.samples,
        "workload": {"cycles": 1, "lines": 5000, "clear_interval_lines": 500},
        "identities": identities,
        "harness_source": identity(args.harness_source),
        "orders": orders,
        "measured": measured,
        "summaries": summaries,
        "completed_measured_cases": len(measured),
        "expected_measured_cases": expected,
        "attribution_gate_reproduced": reproduced,
        "error": error,
        "valid": error is None and len(measured) == expected and reproduced,
    }
    (args.output_dir / "summary.json").write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    (args.output_dir / "summary.md").write_text(markdown(report), encoding="utf-8")
    if report["valid"]:
        print(args.output_dir / "summary.json")
        return 0
    print(error or "Plan 0043 attribution gate did not reproduce", file=os.sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
