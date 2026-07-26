#!/usr/bin/env python3
"""Run randomized disabled/large scrollback comparisons."""

from __future__ import annotations

import argparse
import json
import pathlib
import random
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools/benchmark"
RUNNER = TOOLS / "run-graphical-scrollback.py"
TERMINALS = ("splinterm", "foot", "kitty", "ghostty", "alacritty")
PROFILES = ("disabled", "large")
sys.path.insert(0, str(TOOLS))
from manifest import collect  # noqa: E402
from summary import summarize_values  # noqa: E402


def summaries(records):
    result = {}
    for terminal in TERMINALS:
        result[terminal] = {}
        for profile in PROFILES:
            selected = [
                item["result"]["result"]
                for item in records
                if item["terminal"] == terminal and item["profile"] == profile
            ]
            if selected:
                result[terminal][profile] = {
                    "visible_ns": summarize_values(
                        value["workload"]["trigger_to_visible_marker_ns"]
                        for value in selected
                    ),
                    "rss_growth_bytes": summarize_values(
                        value["resources"]["rss_growth_bytes"] for value in selected
                    ),
                }
    return result


def markdown(summary, args):
    lines = [
        "# Splinterbench scrollback policy matrix",
        "",
        f"Measured samples per terminal/profile: {args.samples}  ",
        f"Output lines: {args.lines}  ",
        f"Randomization seed: {args.seed}",
        "",
        "| Terminal | Disabled visible | Large visible | Disabled RSS growth | Large RSS growth |",
        "|---|---:|---:|---:|---:|",
    ]
    for terminal in TERMINALS:
        disabled = summary[terminal].get("disabled")
        large = summary[terminal].get("large")
        if not disabled or not large:
            continue
        lines.append(
            f"| {terminal.title()} | {disabled['visible_ns']['median'] / 1e6:.1f} ms | "
            f"{large['visible_ns']['median'] / 1e6:.1f} ms | "
            f"{disabled['rss_growth_bytes']['median'] / (1024 * 1024):.1f} MiB | "
            f"{large['rss_growth_bytes']['median'] / (1024 * 1024):.1f} MiB |"
        )
    lines.append("")
    return "\n".join(lines)


def save(output, args, orders, measured, error):
    summary = summaries(measured)
    expected = args.samples * len(TERMINALS) * len(PROFILES)
    matrix = {
        "schema": "splinterm.benchmark.scrollback-matrix.v1",
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
        description="Run randomized scrollback profile blocks"
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
    orders, measured, error = [], [], None
    try:
        for phase, count in (("warmup", args.warmup_runs), ("measured", args.samples)):
            for iteration in range(count):
                order = [
                    (terminal, profile)
                    for terminal in TERMINALS
                    for profile in PROFILES
                ]
                generator.shuffle(order)
                orders.append(
                    {
                        "phase": phase,
                        "iteration": iteration,
                        "cases": [f"{a}:{b}" for a, b in order],
                    }
                )
                print(
                    f"{phase.title()} {iteration + 1}/{count}: "
                    + " → ".join(f"{a}:{b}" for a, b in order)
                )
                for terminal, profile in order:
                    directory = (
                        args.output_dir
                        / "raw"
                        / phase
                        / f"{iteration:02d}"
                        / terminal
                        / profile
                    )
                    directory.mkdir(parents=True, exist_ok=True)
                    path = directory / f"{terminal}-{profile}.json"
                    result = None
                    if args.resume and path.exists():
                        candidate = json.loads(path.read_text())
                        if candidate.get("valid"):
                            result = candidate
                    if result is None:
                        completed = subprocess.run(
                            [
                                sys.executable,
                                str(RUNNER),
                                str(directory),
                                "--terminal",
                                terminal,
                                "--profile",
                                profile,
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
                        raise RuntimeError(f"{terminal}:{profile} produced no result")
                    result = json.loads(path.read_text())
                    if completed.returncode or not result.get("valid"):
                        raise RuntimeError(f"{terminal}:{profile} failed")
                    if phase == "measured":
                        measured.append(
                            {
                                "terminal": terminal,
                                "profile": profile,
                                "iteration": iteration,
                                "result": result,
                            }
                        )
    except (OSError, ValueError, subprocess.TimeoutExpired, RuntimeError) as caught:
        error = str(caught)
        print(f"Scrollback matrix stopped: {error}", file=sys.stderr)
    finally:
        save(args.output_dir, args, orders, measured, error)
    return 0 if error is None else 1


if __name__ == "__main__":
    raise SystemExit(main())
