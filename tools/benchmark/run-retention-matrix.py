#!/usr/bin/env python3
"""Run randomized guarded mixed-output retention blocks."""

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
RUNNER = TOOLS / "run-graphical-retention.py"
TERMINALS = ("splinterm", "foot", "kitty", "ghostty", "alacritty")
sys.path.insert(0, str(TOOLS))
from manifest import collect  # noqa: E402
from summary import summarize_values  # noqa: E402


def summaries(records: list[dict[str, Any]]) -> dict[str, Any]:
    result = {}
    for terminal in TERMINALS:
        selected = [
            record["result"]["retention"]
            for record in records
            if record["terminal"] == terminal
        ]
        if selected:
            result[terminal] = {
                key: summarize_values(item[key] for item in selected)
                for key in (
                    "trigger_to_visible_marker_ns",
                    "rss_peak_observed_bytes",
                    "rss_post_settle_bytes",
                    "retained_growth_bytes",
                    "cpu_ticks",
                )
            }
    return result


def markdown(summary: dict[str, Any], args: argparse.Namespace) -> str:
    lines = [
        "# Splinterbench graphical memory-retention matrix",
        "",
        f"Measured samples per terminal: {args.samples}  ",
        f"Mixed lines per sample: {args.lines}  ",
        f"Randomization seed: {args.seed}",
        "",
        "Each workload mixes plain, ANSI, and Unicode rows, clears every 500 lines,",
        "then records observed peak and post-settle child-inclusive RSS.",
        "",
        "| Terminal | Trigger→visible | Peak RSS | Post-settle RSS | Retained growth | CPU ticks |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    for terminal in TERMINALS:
        item = summary.get(terminal)
        if item is None:
            continue
        visible = item["trigger_to_visible_marker_ns"]["median"] / 1_000_000
        peak = item["rss_peak_observed_bytes"]["median"] / (1024 * 1024)
        post = item["rss_post_settle_bytes"]["median"] / (1024 * 1024)
        growth = item["retained_growth_bytes"]["median"] / (1024 * 1024)
        cpu = item["cpu_ticks"]["median"]
        lines.append(
            f"| {terminal.title()} | {visible:.1f} ms | {peak:.1f} MiB | "
            f"{post:.1f} MiB | {growth:.1f} MiB | {cpu:g} |"
        )
    lines.extend(
        ["", "Raw records and execution order are retained beside this report.", ""]
    )
    return "\n".join(lines)


def save(output, args, orders, measured, error):
    summary = summaries(measured)
    expected = args.samples * len(TERMINALS)
    matrix = {
        "schema": "splinterm.benchmark.retention-matrix.v1",
        "seed": args.seed,
        "warmup_runs": args.warmup_runs,
        "sample_runs": args.samples,
        "lines": args.lines,
        "orders": orders,
        "completed_measured_cases": len(measured),
        "expected_measured_cases": expected,
        "summary": summary,
        "error": error,
        "valid": error is None and len(measured) == expected,
    }
    (output / "matrix.json").write_text(
        json.dumps(matrix, indent=2, sort_keys=True) + "\n"
    )
    (output / "summary.md").write_text(markdown(summary, args))


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run randomized five-terminal retention blocks"
    )
    parser.add_argument("output_dir", type=pathlib.Path)
    parser.add_argument("--warmup-runs", type=int, default=3)
    parser.add_argument("--samples", type=int, default=10)
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--lines", type=int, default=5000)
    parser.add_argument("--resume", action="store_true")
    args = parser.parse_args()
    args.output_dir.mkdir(parents=True, exist_ok=True)
    manifest = args.output_dir / "manifest.json"
    if not args.resume or not manifest.exists():
        manifest.write_text(json.dumps(collect(ROOT), indent=2, sort_keys=True) + "\n")
    generator = random.Random(args.seed)
    orders, measured = [], []
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
                    directory = (
                        args.output_dir / "raw" / phase / f"{iteration:02d}" / terminal
                    )
                    directory.mkdir(parents=True, exist_ok=True)
                    path = directory / f"{terminal}-retention.json"
                    result = None
                    if args.resume and path.exists():
                        candidate = json.loads(path.read_text())
                        if candidate.get("valid") and candidate.get(
                            "isolation", {}
                        ).get("cleanup_verified"):
                            result = candidate
                    if result is None:
                        completed = subprocess.run(
                            [
                                sys.executable,
                                str(RUNNER),
                                str(directory),
                                "--terminal",
                                terminal,
                                "--lines",
                                str(args.lines),
                            ],
                            cwd=ROOT,
                            text=True,
                            capture_output=True,
                            check=False,
                            timeout=45,
                        )
                    else:
                        completed = subprocess.CompletedProcess([], 0)
                    if not path.exists():
                        raise RuntimeError(f"{terminal} produced no retention result")
                    result = json.loads(path.read_text())
                    if completed.returncode or not result.get("valid"):
                        raise RuntimeError(
                            f"{terminal} retention failed: {result.get('notes', [])}"
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
        print(f"Retention matrix stopped: {error}", file=sys.stderr)
    finally:
        save(args.output_dir, args, orders, measured, error)
    if error is None:
        print(f"Retention matrix complete: {args.output_dir / 'summary.md'}")
        return 0
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
