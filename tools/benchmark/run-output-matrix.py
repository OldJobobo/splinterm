#!/usr/bin/env python3
"""Run randomized trigger-gated output workloads across five terminals."""

from __future__ import annotations

import argparse
import json
import pathlib
import random
import subprocess
import sys
from typing import Any

ROOT = pathlib.Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools/benchmark"
RUNNER = TOOLS / "run-graphical-output.py"
TERMINALS = ("splinterm", "foot", "kitty", "ghostty", "alacritty")
CASES = ("plain", "ansi", "unicode")

sys.path.insert(0, str(TOOLS))
from manifest import collect  # noqa: E402
from summary import summarize_values  # noqa: E402


def value(result: dict[str, Any], metric: str) -> int:
    if metric in result["workload"]:
        return int(result["workload"][metric])
    return int(result["resources"][metric])


def summaries(records: list[dict[str, Any]]) -> dict[str, Any]:
    metrics = (
        "child_write_duration_ns",
        "trigger_to_write_complete_ns",
        "trigger_to_visible_marker_ns",
        "rss_growth_bytes",
        "cpu_ticks",
        "context_switches",
    )
    summary: dict[str, Any] = {}
    for case in CASES:
        summary[case] = {}
        for terminal in TERMINALS:
            selected = [
                record["result"]
                for record in records
                if record["case"] == case and record["terminal"] == terminal
            ]
            if selected:
                summary[case][terminal] = {
                    metric: summarize_values(
                        value(result, metric) for result in selected
                    )
                    for metric in metrics
                }
    return summary


def markdown(summary: dict[str, Any], samples: int, seed: int, lines_count: int) -> str:
    lines = [
        "# Splinterbench graphical output matrix",
        "",
        f"Measured samples per terminal/workload: {samples}  ",
        f"Lines per workload: {lines_count}  ",
        f"Randomization seed: {seed}",
        "",
        "Child-write timing and screenshot polling are distinct boundaries. The visible",
        "marker is an approximation based on detecting a final uncommon truecolor row in guarded",
        "window screenshots; it is not a compositor presentation timestamp.",
        "",
    ]
    for case in CASES:
        lines.extend(
            [
                f"## {case.title()}",
                "",
                "| Terminal | Child write | Trigger→write | Trigger→visible | RSS growth | CPU ticks |",
                "|---|---:|---:|---:|---:|---:|",
            ]
        )
        for terminal in TERMINALS:
            item = summary.get(case, {}).get(terminal)
            if item is None:
                continue
            child = item["child_write_duration_ns"]["median"] / 1_000_000
            write = item["trigger_to_write_complete_ns"]["median"] / 1_000_000
            visible = item["trigger_to_visible_marker_ns"]["median"] / 1_000_000
            rss = item["rss_growth_bytes"]["median"] / (1024 * 1024)
            cpu = item["cpu_ticks"]["median"]
            lines.append(
                f"| {terminal.title()} | {child:.1f} ms | {write:.1f} ms | "
                f"{visible:.1f} ms | {rss:.1f} MiB | {cpu:g} |"
            )
        lines.append("")
    lines.extend(
        [
            "Raw records and randomized execution order are retained beside this report.",
            "This is development evidence from one dirty-worktree host, not a universal ranking.",
            "",
        ]
    )
    return "\n".join(lines)


def save(
    output: pathlib.Path,
    args: argparse.Namespace,
    orders: list[dict[str, Any]],
    records: list[dict[str, Any]],
    error: str | None,
) -> None:
    summary = summaries(records)
    expected = args.samples * len(TERMINALS) * len(CASES)
    document = {
        "schema": "splinterm.benchmark.output-matrix.v1",
        "seed": args.seed,
        "warmup_runs": args.warmup_runs,
        "sample_runs": args.samples,
        "lines": args.lines,
        "columns": args.columns,
        "orders": orders,
        "completed_measured_cases": len(records),
        "expected_measured_cases": expected,
        "summary": summary,
        "error": error,
        "valid": error is None and len(records) == expected,
    }
    (output / "matrix.json").write_text(
        json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    (output / "summary.md").write_text(
        markdown(summary, args.samples, args.seed, args.lines), encoding="utf-8"
    )


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run randomized five-terminal plain/ANSI/Unicode output blocks"
    )
    parser.add_argument("output_dir", type=pathlib.Path)
    parser.add_argument("--warmup-runs", type=int, default=3)
    parser.add_argument("--samples", type=int, default=10)
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--lines", type=int, default=2000)
    parser.add_argument("--columns", type=int, default=80)
    parser.add_argument("--settle-seconds", type=float, default=1.0)
    parser.add_argument(
        "--resume",
        action="store_true",
        help="reuse valid raw records and rerun only missing or invalid cases",
    )
    args = parser.parse_args()
    if args.warmup_runs < 0 or args.samples <= 0 or args.lines <= 0:
        parser.error("warmups must be nonnegative and samples/lines must be positive")

    args.output_dir.mkdir(parents=True, exist_ok=True)
    manifest_path = args.output_dir / "manifest.json"
    if not args.resume or not manifest_path.exists():
        manifest_path.write_text(
            json.dumps(collect(ROOT), indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
    generator = random.Random(args.seed)
    orders: list[dict[str, Any]] = []
    measured: list[dict[str, Any]] = []
    error = None
    try:
        for phase, count in (("warmup", args.warmup_runs), ("measured", args.samples)):
            for iteration in range(count):
                order = [(terminal, case) for terminal in TERMINALS for case in CASES]
                generator.shuffle(order)
                orders.append(
                    {
                        "phase": phase,
                        "iteration": iteration,
                        "cases": [f"{terminal}:{case}" for terminal, case in order],
                    }
                )
                print(
                    f"{phase.title()} {iteration + 1}/{count}: "
                    + " → ".join(f"{terminal}:{case}" for terminal, case in order)
                )
                for terminal, case in order:
                    case_dir = (
                        args.output_dir
                        / "raw"
                        / phase
                        / f"{iteration:02d}"
                        / terminal
                        / case
                    )
                    case_dir.mkdir(parents=True, exist_ok=True)
                    command = [
                        sys.executable,
                        str(RUNNER),
                        str(case_dir),
                        "--terminal",
                        terminal,
                        "--case",
                        case,
                        "--lines",
                        str(args.lines),
                        "--columns",
                        str(args.columns),
                        "--settle-seconds",
                        str(args.settle_seconds),
                    ]
                    result_path = case_dir / f"{terminal}-{case}.json"
                    result = None
                    if args.resume and result_path.exists():
                        candidate = json.loads(result_path.read_text(encoding="utf-8"))
                        if candidate.get("valid") and candidate.get(
                            "isolation", {}
                        ).get("cleanup_verified"):
                            result = candidate
                    if result is None:
                        completed = subprocess.run(
                            command,
                            cwd=ROOT,
                            text=True,
                            capture_output=True,
                            check=False,
                            timeout=30,
                        )
                    else:
                        completed = subprocess.CompletedProcess(command, 0)
                    if not result_path.exists():
                        raise RuntimeError(f"{terminal}:{case} produced no result")
                    result = json.loads(result_path.read_text(encoding="utf-8"))
                    if completed.returncode or not result.get("valid"):
                        raise RuntimeError(
                            f"{terminal}:{case} failed: {result.get('notes', [])}"
                        )
                    if not result.get("isolation", {}).get("cleanup_verified"):
                        raise RuntimeError(f"{terminal}:{case} did not verify cleanup")
                    if phase == "measured":
                        measured.append(
                            {
                                "terminal": terminal,
                                "case": case,
                                "iteration": iteration,
                                "result": result,
                            }
                        )
    except (OSError, ValueError, subprocess.TimeoutExpired, RuntimeError) as caught:
        error = str(caught)
        print(f"Output matrix stopped: {error}", file=sys.stderr)
    finally:
        save(args.output_dir, args, orders, measured, error)
    if error is None:
        print(f"Output matrix complete: {args.output_dir / 'summary.md'}")
        return 0
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
