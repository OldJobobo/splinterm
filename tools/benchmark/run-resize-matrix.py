#!/usr/bin/env python3
"""Run randomized guarded resize blocks across all benchmark terminals."""

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
RUNNER = TOOLS / "run-graphical-resize.py"
TERMINALS = ("splinterm", "foot", "kitty", "ghostty", "alacritty")

sys.path.insert(0, str(TOOLS))
from manifest import collect  # noqa: E402
from summary import summarize_values  # noqa: E402


def summaries(records: list[dict[str, Any]]) -> dict[str, Any]:
    result = {}
    for terminal in TERMINALS:
        selected = [
            record["result"] for record in records if record["terminal"] == terminal
        ]
        if selected:
            result[terminal] = {
                "settled_duration_ns": summarize_values(
                    item["resize"]["settled_duration_ns"] for item in selected
                ),
                "dispatch_duration_ns": summarize_values(
                    item["resize"]["dispatch_duration_ns"] for item in selected
                ),
                "rss_growth_bytes": summarize_values(
                    item["resources"]["rss_growth_bytes"] for item in selected
                ),
                "cpu_ticks": summarize_values(
                    item["resources"]["cpu_ticks"] for item in selected
                ),
            }
    return result


def markdown(summary: dict[str, Any], samples: int, seed: int) -> str:
    lines = [
        "# Splinterbench graphical resize matrix",
        "",
        f"Measured samples per terminal: {samples}  ",
        f"Randomization seed: {seed}",
        "",
        "Each sample alternates 800×500 and 1200×700 six times and verifies every",
        "settled geometry before continuing.",
        "",
        "| Terminal | 12 resizes settled | Dispatch time | RSS growth | CPU ticks |",
        "|---|---:|---:|---:|---:|",
    ]
    for terminal in TERMINALS:
        item = summary.get(terminal)
        if item is None:
            continue
        settled = item["settled_duration_ns"]["median"] / 1_000_000
        dispatch = item["dispatch_duration_ns"]["median"] / 1_000_000
        rss = item["rss_growth_bytes"]["median"] / (1024 * 1024)
        cpu = item["cpu_ticks"]["median"]
        lines.append(
            f"| {terminal.title()} | {settled:.1f} ms | {dispatch:.1f} ms | {rss:.1f} MiB | {cpu:g} |"
        )
    lines.extend(
        ["", "Raw records and execution order are retained beside this report.", ""]
    )
    return "\n".join(lines)


def save(
    output: pathlib.Path, args: argparse.Namespace, orders, records, error
) -> None:
    summary = summaries(records)
    expected = args.samples * len(TERMINALS)
    matrix = {
        "schema": "splinterm.benchmark.resize-matrix.v1",
        "seed": args.seed,
        "warmup_runs": args.warmup_runs,
        "sample_runs": args.samples,
        "orders": orders,
        "completed_measured_cases": len(records),
        "expected_measured_cases": expected,
        "summary": summary,
        "error": error,
        "valid": error is None and len(records) == expected,
    }
    (output / "matrix.json").write_text(
        json.dumps(matrix, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    (output / "summary.md").write_text(
        markdown(summary, args.samples, args.seed), encoding="utf-8"
    )


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run randomized five-terminal resize blocks"
    )
    parser.add_argument("output_dir", type=pathlib.Path)
    parser.add_argument("--warmup-runs", type=int, default=3)
    parser.add_argument("--samples", type=int, default=10)
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--settle-seconds", type=float, default=1.0)
    parser.add_argument("--resume", action="store_true")
    args = parser.parse_args()
    if args.warmup_runs < 0 or args.samples <= 0:
        parser.error("warmups must be nonnegative and samples positive")
    args.output_dir.mkdir(parents=True, exist_ok=True)
    manifest = args.output_dir / "manifest.json"
    if not args.resume or not manifest.exists():
        manifest.write_text(
            json.dumps(collect(ROOT), indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
    generator = random.Random(args.seed)
    orders = []
    measured = []
    error = None
    try:
        for phase, count in (("warmup", args.warmup_runs), ("measured", args.samples)):
            for iteration in range(count):
                order = list(TERMINALS)
                generator.shuffle(order)
                orders.append(
                    {"phase": phase, "iteration": iteration, "terminals": order}
                )
                print(f"{phase.title()} {iteration + 1}/{count}: {' → '.join(order)}")
                for terminal in order:
                    case_dir = (
                        args.output_dir / "raw" / phase / f"{iteration:02d}" / terminal
                    )
                    case_dir.mkdir(parents=True, exist_ok=True)
                    result_path = case_dir / f"{terminal}-resize.json"
                    result = None
                    if args.resume and result_path.exists():
                        candidate = json.loads(result_path.read_text(encoding="utf-8"))
                        if candidate.get("valid") and candidate.get(
                            "isolation", {}
                        ).get("cleanup_verified"):
                            result = candidate
                    if result is None:
                        completed = subprocess.run(
                            [
                                sys.executable,
                                str(RUNNER),
                                str(case_dir),
                                "--terminal",
                                terminal,
                                "--settle-seconds",
                                str(args.settle_seconds),
                            ],
                            cwd=ROOT,
                            text=True,
                            capture_output=True,
                            check=False,
                            timeout=30,
                        )
                    else:
                        completed = subprocess.CompletedProcess([], 0)
                    if not result_path.exists():
                        raise RuntimeError(f"{terminal} produced no resize result")
                    result = json.loads(result_path.read_text(encoding="utf-8"))
                    if completed.returncode or not result.get("valid"):
                        raise RuntimeError(
                            f"{terminal} resize failed: {result.get('notes', [])}"
                        )
                    if phase == "measured":
                        measured.append(
                            {
                                "terminal": terminal,
                                "iteration": iteration,
                                "result": result,
                            }
                        )
    except (OSError, ValueError, subprocess.TimeoutExpired, RuntimeError) as caught:
        error = str(caught)
        print(f"Resize matrix stopped: {error}", file=sys.stderr)
    finally:
        save(args.output_dir, args, orders, measured, error)
    if error is None:
        print(f"Resize matrix complete: {args.output_dir / 'summary.md'}")
        return 0
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
