#!/usr/bin/env python3
"""Run randomized targeted-input latency blocks across five terminals."""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import random
import shutil
import subprocess
import sys
from typing import Any

ROOT = pathlib.Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools/benchmark"
CASE_RUNNER = TOOLS / "run-graphical-latency.py"
TERMINALS = ("splinterm", "foot", "kitty", "ghostty", "alacritty")
IMPLEMENTATION_FILES = (
    "crates/splinterm/src/wayland.rs",
    "tools/benchmark/latency.py",
    "tools/benchmark/latency-schema.json",
    "tools/benchmark/run-graphical-latency.py",
    "tools/benchmark/run-latency-matrix.py",
    "tools/benchmark/run-graphical-idle.py",
    "tools/benchmark/run-graphical-output.py",
    "tools/benchmark/manifest.py",
    "tools/benchmark/adapters/splinterm.py",
    "tools/benchmark/workloads/bench-child.py",
    "tools/benchmark/profiles/foot.ini",
    "tools/benchmark/profiles/kitty.conf",
    "tools/benchmark/profiles/ghostty.conf",
    "tools/benchmark/profiles/alacritty.toml",
    "tools/benchmark/profiles/splinterm.ini",
)

sys.path.insert(0, str(TOOLS))
from manifest import collect  # noqa: E402
from summary import summarize_values  # noqa: E402


def file_sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def snapshot_implementation(output_dir: pathlib.Path) -> None:
    records = []
    for relative in IMPLEMENTATION_FILES:
        source = ROOT / relative
        destination = output_dir / "implementation" / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
        records.append({
            "path": relative,
            "sha256": file_sha256(destination),
        })
    (output_dir / "implementation.json").write_text(
        json.dumps({
            "schema": "splinterm.benchmark.implementation-snapshot.v1",
            "files": records,
        }, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def validate_case(result: dict[str, Any]) -> None:
    failures = []
    expected_values = (
        ("schema", result.get("schema"), "splinterm.benchmark.input-latency.v1"),
        ("valid", result.get("valid"), True),
        ("terminal", result.get("terminal") in TERMINALS, True),
    )
    for label, actual, expected in expected_values:
        if actual != expected:
            failures.append(f"{label} is {actual!r}, expected {expected!r}")
    if result.get("notes") != []:
        failures.append("notes are not empty")

    boundary = result.get("boundary", {})
    boundary_expected = {
        "backend": "host-hyprland-targeted-shortcut",
        "width": 960,
        "height": 600,
        "refresh_hz": 60,
        "scale": 1,
        "input_protocol": "Hyprland hl.dsp.send_shortcut targeted window",
        "capture_protocol": "zwlr_screencopy_manager_v1 via grim",
        "targeted_window_verified": True,
    }
    for field, expected in boundary_expected.items():
        if boundary.get(field) != expected:
            failures.append(f"boundary.{field} is not {expected!r}")

    isolation = result.get("isolation", {})
    isolation_expected = {
        "workspace": 8,
        "monitor": "DP-2",
        "no_initial_focus": True,
        "targeted_input_without_focus": True,
        "host_focus_unchanged": True,
        "host_workspace_unchanged": True,
        "cleanup_verified": True,
    }
    for field, expected in isolation_expected.items():
        if isolation.get(field) != expected:
            failures.append(f"isolation.{field} is not {expected!r}")

    input_record = result.get("input", {})
    if input_record.get("token") != "x" or input_record.get("injector_returncode") != 0:
        failures.append("input token or injector result is invalid")
    if input_record.get("clock") != "CLOCK_MONOTONIC shared host namespace":
        failures.append("input clock boundary is invalid")
    presentation = result.get("presentation")
    if presentation != {
        "status": "not-measured",
        "input_to_compositor_presentation_ns": None,
    }:
        failures.append("presentation boundary is not explicitly unmeasured")
    visible = result.get("visible", {})
    if visible.get("boundary") != "host_window_screenshot_polling_approximation":
        failures.append("visible boundary is not screenshot polling")
    if visible.get("poll_interval_ms") != 10:
        failures.append("visible poll interval is not pinned")
    try:
        if int(visible["input_to_visible_marker_ns"]) < int(input_record["input_to_child_ns"]):
            failures.append("visible detection precedes child receipt")
    except (KeyError, TypeError, ValueError):
        failures.append("latency values are missing or invalid")
    if failures:
        raise RuntimeError("unsafe latency result: " + "; ".join(failures))


def summaries(records: list[dict[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for terminal in TERMINALS:
        samples = [item["result"] for item in records if item["terminal"] == terminal]
        if samples:
            result[terminal] = {
                "input_to_child_ns": summarize_values(
                    int(item["input"]["input_to_child_ns"]) for item in samples
                ),
                "input_to_visible_marker_ns": summarize_values(
                    int(item["visible"]["input_to_visible_marker_ns"]) for item in samples
                ),
            }
    return result


def markdown(summary: dict[str, Any], samples: int, seed: int) -> str:
    lines = [
        "# Splinterbench targeted-input latency matrix",
        "",
        f"Measured samples per terminal: {samples}  ",
        f"Randomization seed: {seed}",
        "",
        "Input is delivered without focus through Hyprland's targeted shortcut dispatcher.",
        "Input-to-child ends at the child's monotonic receipt record. Input-to-visible ends",
        "at screenshot polling detection and is not compositor presentation or input-to-photon.",
        "",
        "| Terminal | Input → child median | Input → visible median |",
        "|---|---:|---:|",
    ]
    for terminal in TERMINALS:
        item = summary.get(terminal)
        if item:
            child = item["input_to_child_ns"]["median"] / 1_000_000
            visible = item["input_to_visible_marker_ns"]["median"] / 1_000_000
            lines.append(f"| {terminal.title()} | {child:.2f} ms | {visible:.2f} ms |")
    lines.extend(["", "Raw randomized samples are retained beside this report.", ""])
    return "\n".join(lines)


def save(
    output_dir: pathlib.Path,
    seed: int,
    warmups: int,
    samples: int,
    orders: list[dict[str, Any]],
    measured: list[dict[str, Any]],
    error: str | None,
) -> None:
    summary = summaries(measured)
    document = {
        "schema": "splinterm.benchmark.input-latency-matrix.v1",
        "boundary": "host-hyprland-targeted-shortcut",
        "visible_boundary": "host_window_screenshot_polling_approximation",
        "presentation_status": "not-measured",
        "implementation_snapshot": "implementation.json",
        "seed": seed,
        "warmup_runs": warmups,
        "sample_runs": samples,
        "orders": orders,
        "completed_measured_cases": len(measured),
        "expected_measured_cases": samples * len(TERMINALS),
        "summary": summary,
        "error": error,
        "valid": error is None and len(measured) == samples * len(TERMINALS),
    }
    (output_dir / "matrix.json").write_text(
        json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    (output_dir / "summary.md").write_text(
        markdown(summary, samples, seed), encoding="utf-8"
    )


def main() -> int:
    parser = argparse.ArgumentParser(description="Run a guarded five-terminal input-latency matrix")
    parser.add_argument("output_dir", type=pathlib.Path)
    parser.add_argument("--warmup-runs", type=int, default=3)
    parser.add_argument("--samples", type=int, default=10)
    parser.add_argument("--seed", type=int, default=20260729)
    parser.add_argument("--settle-seconds", type=float, default=1.0)
    args = parser.parse_args()
    if args.warmup_runs < 0 or args.samples <= 0 or args.settle_seconds < 0:
        parser.error("invalid matrix run counts or settle duration")

    args.output_dir.mkdir(parents=True, exist_ok=True)
    (args.output_dir / "manifest.json").write_text(
        json.dumps(collect(ROOT), indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    snapshot_implementation(args.output_dir)
    generator = random.Random(args.seed)
    orders: list[dict[str, Any]] = []
    measured: list[dict[str, Any]] = []
    error = None
    try:
        for phase, count in (("warmup", args.warmup_runs), ("measured", args.samples)):
            for iteration in range(count):
                order = list(TERMINALS)
                generator.shuffle(order)
                orders.append({"phase": phase, "iteration": iteration, "terminals": order})
                print(f"{phase.title()} {iteration + 1}/{count}: {' → '.join(order)}", flush=True)
                for terminal in order:
                    case_dir = args.output_dir / "raw" / phase / f"{iteration:02d}" / terminal
                    case_dir.mkdir(parents=True, exist_ok=True)
                    completed = subprocess.run(
                        [
                            sys.executable, str(CASE_RUNNER), str(case_dir),
                            "--terminal", terminal,
                            "--settle-seconds", str(args.settle_seconds),
                        ],
                        cwd=ROOT, check=False, timeout=45,
                    )
                    result_path = case_dir / f"{terminal}-input.json"
                    if not result_path.exists():
                        raise RuntimeError(f"{terminal} produced no latency result")
                    result = json.loads(result_path.read_text(encoding="utf-8"))
                    if completed.returncode:
                        raise RuntimeError(f"{terminal} runner exited {completed.returncode}: {result.get('notes', [])}")
                    validate_case(result)
                    if phase == "measured":
                        measured.append({
                            "phase": phase,
                            "iteration": iteration,
                            "terminal": terminal,
                            "result": result,
                        })
                    save(args.output_dir, args.seed, args.warmup_runs, args.samples, orders, measured, None)
    except (OSError, subprocess.TimeoutExpired, RuntimeError, json.JSONDecodeError) as failure:
        error = str(failure)
    save(args.output_dir, args.seed, args.warmup_runs, args.samples, orders, measured, error)
    return 0 if error is None and len(measured) == args.samples * len(TERMINALS) else 1


if __name__ == "__main__":
    raise SystemExit(main())
