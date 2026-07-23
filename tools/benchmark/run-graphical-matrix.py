#!/usr/bin/env python3
"""Run randomized guarded startup/idle blocks across all benchmark terminals."""

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
CASE_RUNNER = TOOLS / "run-graphical-idle.py"
TERMINALS = ("splinterm", "foot", "kitty", "ghostty", "alacritty")

sys.path.insert(0, str(TOOLS))
from manifest import collect  # noqa: E402
from summary import summarize_values  # noqa: E402


def metric(result: dict[str, Any], name: str) -> int:
    if name == "launch_to_child_ready_ns":
        return int(result["boundaries"][name])
    if name == "launch_to_window_map_ns":
        return int(result["boundaries"][name])
    return int(result["idle"][name])


def summaries(records: list[dict[str, Any]]) -> dict[str, Any]:
    names = (
        "launch_to_child_ready_ns",
        "launch_to_window_map_ns",
        "rss_bytes",
        "cpu_ticks",
        "context_switches",
    )
    result: dict[str, Any] = {}
    for terminal in TERMINALS:
        terminal_records = [
            record["result"] for record in records if record["terminal"] == terminal
        ]
        if not terminal_records:
            continue
        result[terminal] = {
            name: summarize_values(metric(record, name) for record in terminal_records)
            for name in names
        }
    return result


def markdown(summary: dict[str, Any], samples: int, seed: int) -> str:
    lines = [
        "# Splinterbench graphical idle matrix",
        "",
        f"Measured samples per terminal: {samples}  ",
        f"Randomization seed: {seed}",
        "",
        "Startup boundaries are observed independently. Splinterm uses a prestarted daemon;",
        "the other terminals use standalone process launches. Values are medians of",
        "child-inclusive process-forest measurements and are not input-to-photon latency.",
        "",
        "| Terminal | Child ready | Window mapped | Idle RSS | CPU ticks | Context switches |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    for terminal in TERMINALS:
        item = summary.get(terminal)
        if item is None:
            continue
        child_ms = item["launch_to_child_ready_ns"]["median"] / 1_000_000
        map_ms = item["launch_to_window_map_ns"]["median"] / 1_000_000
        rss_mib = item["rss_bytes"]["median"] / (1024 * 1024)
        cpu = item["cpu_ticks"]["median"]
        switches = item["context_switches"]["median"]
        lines.append(
            f"| {terminal.title()} | {child_ms:.1f} ms | {map_ms:.1f} ms | "
            f"{rss_mib:.1f} MiB | {cpu:g} | {switches:g} |"
        )
    lines.extend(
        [
            "",
            "Raw samples and execution order are retained beside this report. Do not treat",
            "this development matrix as a publishable cross-host conclusion.",
            "",
        ]
    )
    return "\n".join(lines)


def save(
    output_dir: pathlib.Path,
    seed: int,
    warmup_runs: int,
    sample_runs: int,
    orders: list[dict[str, Any]],
    measured: list[dict[str, Any]],
    error: str | None,
) -> None:
    summary = summaries(measured)
    document = {
        "schema": "splinterm.benchmark.graphical-matrix.v1",
        "seed": seed,
        "warmup_runs": warmup_runs,
        "sample_runs": sample_runs,
        "orders": orders,
        "completed_measured_cases": len(measured),
        "expected_measured_cases": sample_runs * len(TERMINALS),
        "summary": summary,
        "error": error,
        "valid": error is None and len(measured) == sample_runs * len(TERMINALS),
    }
    (output_dir / "matrix.json").write_text(
        json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    (output_dir / "summary.md").write_text(
        markdown(summary, sample_runs, seed), encoding="utf-8"
    )


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run a randomized five-terminal guarded graphical matrix"
    )
    parser.add_argument("output_dir", type=pathlib.Path)
    parser.add_argument("--warmup-runs", type=int, default=3)
    parser.add_argument("--samples", type=int, default=10)
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--settle-seconds", type=float, default=1.0)
    parser.add_argument("--sample-seconds", type=float, default=2.0)
    args = parser.parse_args()
    if args.warmup_runs < 0 or args.samples <= 0:
        parser.error("warmup runs must be nonnegative and samples must be positive")

    args.output_dir.mkdir(parents=True, exist_ok=True)
    (args.output_dir / "manifest.json").write_text(
        json.dumps(collect(ROOT), indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    generator = random.Random(args.seed)
    orders: list[dict[str, Any]] = []
    measured: list[dict[str, Any]] = []
    error = None
    phases = (("warmup", args.warmup_runs), ("measured", args.samples))
    try:
        for phase, count in phases:
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
                    command = [
                        sys.executable,
                        str(CASE_RUNNER),
                        str(case_dir),
                        "--terminal",
                        terminal,
                        "--warmup-seconds",
                        str(args.settle_seconds),
                        "--sample-seconds",
                        str(args.sample_seconds),
                    ]
                    completed = subprocess.run(
                        command, cwd=ROOT, check=False, timeout=30
                    )
                    result_path = case_dir / f"{terminal}.json"
                    if not result_path.exists():
                        raise RuntimeError(f"{terminal} produced no result record")
                    result = json.loads(result_path.read_text(encoding="utf-8"))
                    if completed.returncode or not result.get("valid"):
                        raise RuntimeError(
                            f"{terminal} failed guarded case: {result.get('notes', [])}"
                        )
                    if not result.get("isolation", {}).get("cleanup_verified"):
                        raise RuntimeError(f"{terminal} did not verify cleanup")
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
        print(f"Matrix stopped: {error}", file=sys.stderr)
    finally:
        save(
            args.output_dir,
            args.seed,
            args.warmup_runs,
            args.samples,
            orders,
            measured,
            error,
        )
    if error is None:
        print(f"Matrix complete: {args.output_dir / 'summary.md'}")
        return 0
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
