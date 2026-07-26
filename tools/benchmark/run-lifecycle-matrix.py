#!/usr/bin/env python3
"""Run randomized guarded process-exit lifecycle blocks."""

from __future__ import annotations
import argparse
import json
import pathlib
import random
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools/benchmark"
RUNNER = TOOLS / "run-graphical-lifecycle.py"
TERMINALS = ("splinterm", "foot", "kitty", "ghostty", "alacritty")
sys.path.insert(0, str(TOOLS))
from manifest import collect  # noqa: E402
from summary import summarize_values  # noqa: E402


def summaries(records):
    result = {}
    for terminal in TERMINALS:
        selected = [item["result"] for item in records if item["terminal"] == terminal]
        if selected:
            unmapped = [
                item["window_unmap_ns"]
                for item in selected
                if item["window_unmap_ns"] is not None
            ]
            result[terminal] = {
                "child_exit_ns": summarize_values(
                    item["child_exit_ns"] for item in selected
                ),
                "window_unmap_ns": summarize_values(unmapped) if unmapped else None,
                "persisted_count": sum(item["window_persisted"] for item in selected),
                "residual_process_count": summarize_values(
                    item["residual_process_count"] for item in selected
                ),
            }
    return result


def markdown(summary, args):
    lines = [
        "# Splinterbench process-exit lifecycle matrix",
        "",
        f"Measured samples per terminal: {args.samples}  ",
        f"Randomization seed: {args.seed}",
        "",
        "No terminal hold option is enabled. Persisted windows are recorded as lifecycle semantics, not failures.",
        "",
        "| Terminal | Child exit | Window unmap | Persisted | Residual processes |",
        "|---|---:|---:|---:|---:|",
    ]
    for terminal in TERMINALS:
        item = summary.get(terminal)
        if not item:
            continue
        unmap = (
            "n/a"
            if item["window_unmap_ns"] is None
            else f"{item['window_unmap_ns']['median'] / 1e6:.1f} ms"
        )
        lines.append(
            f"| {terminal.title()} | {item['child_exit_ns']['median'] / 1e6:.1f} ms | {unmap} | {item['persisted_count']}/{args.samples} | {item['residual_process_count']['median']:g} |"
        )
    lines.append("")
    return "\n".join(lines)


def save(output, args, orders, records, error):
    expected = args.samples * len(TERMINALS)
    summary = summaries(records)
    matrix = {
        "schema": "splinterm.benchmark.lifecycle-matrix.v1",
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
        json.dumps(matrix, indent=2, sort_keys=True) + "\n"
    )
    (output / "summary.md").write_text(markdown(summary, args))


def main():
    parser = argparse.ArgumentParser(
        description="Run randomized five-terminal lifecycle blocks"
    )
    parser.add_argument("output_dir", type=pathlib.Path)
    parser.add_argument("--warmup-runs", type=int, default=3)
    parser.add_argument("--samples", type=int, default=10)
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--resume", action="store_true")
    args = parser.parse_args()
    args.output_dir.mkdir(parents=True, exist_ok=True)
    manifest = args.output_dir / "manifest.json"
    if not args.resume or not manifest.exists():
        manifest.write_text(json.dumps(collect(ROOT), indent=2, sort_keys=True) + "\n")
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
                    directory = (
                        args.output_dir / "raw" / phase / f"{iteration:02d}" / terminal
                    )
                    directory.mkdir(parents=True, exist_ok=True)
                    path = directory / f"{terminal}-lifecycle.json"
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
                            ],
                            cwd=ROOT,
                            text=True,
                            capture_output=True,
                            check=False,
                            timeout=20,
                        )
                    else:
                        completed = subprocess.CompletedProcess([], 0)
                    if not path.exists():
                        raise RuntimeError(f"{terminal} produced no lifecycle result")
                    result = json.loads(path.read_text())
                    if completed.returncode or not result.get("valid"):
                        raise RuntimeError(
                            f"{terminal} lifecycle failed: {result.get('notes', [])}"
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
        print(f"Lifecycle matrix stopped: {error}", file=sys.stderr)
    finally:
        save(args.output_dir, args, orders, measured, error)
    return 0 if error is None else 1


if __name__ == "__main__":
    raise SystemExit(main())
