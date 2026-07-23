#!/usr/bin/env python3
"""Run and enforce the Phase 8.1 release performance baseline."""

from __future__ import annotations

import argparse
import datetime
import json
import os
import pathlib
import platform
import subprocess
import sys
from typing import Any

ROOT = pathlib.Path(__file__).resolve().parents[2]
THRESHOLDS = ROOT / "tools/performance/phase9-thresholds.json"


def run(command: list[str], *, stdout: pathlib.Path | None = None) -> None:
    target = stdout.open("w", encoding="utf-8") if stdout else None
    try:
        result = subprocess.run(command, cwd=ROOT, text=True, stdout=target, check=False)
    finally:
        if target:
            target.close()
    if result.returncode:
        raise RuntimeError(f"command failed ({result.returncode}): {' '.join(command)}")


def output(command: list[str]) -> str:
    result = subprocess.run(command, cwd=ROOT, text=True, capture_output=True, check=False)
    if result.returncode:
        raise RuntimeError(f"command failed ({result.returncode}): {' '.join(command)}")
    return result.stdout.strip()


def host_context() -> dict[str, Any]:
    os_release = {}
    for line in pathlib.Path("/etc/os-release").read_text(encoding="utf-8").splitlines():
        if "=" in line:
            key, value = line.split("=", 1)
            os_release[key] = value.strip().strip('"')
    cpu = next(
        (
            line.split(":", 1)[1].strip()
            for line in pathlib.Path("/proc/cpuinfo").read_text(encoding="utf-8").splitlines()
            if line.startswith("model name")
        ),
        platform.processor() or "unknown",
    )
    return {
        "recorded_at": datetime.datetime.now(datetime.UTC).isoformat(),
        "os": os_release.get("PRETTY_NAME"),
        "kernel": platform.release(),
        "architecture": platform.machine(),
        "cpu": cpu,
        "rustc": output(["rustc", "--version"]),
        "cargo": output(["cargo", "--version"]),
        "git_commit": output(["git", "rev-parse", "HEAD"]),
        "git_dirty": bool(output(["git", "status", "--porcelain"])),
    }


def check_max(failures: list[str], label: str, value: int | None, maximum: int) -> None:
    if value is None or value > maximum:
        failures.append(f"{label}: {value!r} exceeds {maximum}")


def validate_renderer(
    report: dict[str, Any], thresholds: dict[str, Any], failures: list[str]
) -> None:
    if report.get("profile") != "release":
        failures.append("renderer benchmark was not a release build")
    grids = {(grid["columns"], grid["rows"]): grid for grid in report.get("grids", [])}
    for dimensions, key in (((80, 24), "80x24"), ((240, 80), "240x80")):
        grid = grids.get(dimensions)
        if grid is None:
            failures.append(f"renderer grid missing: {key}")
            continue
        budget = thresholds[key]
        check_max(failures, f"{key}.cold", grid["cold_frame_ns"], budget["cold_frame_ns_max"])
        check_max(
            failures,
            f"{key}.warm_prepare_p95",
            grid["warm_full_prepare_ns"]["p95"],
            budget["warm_prepare_p95_ns_max"],
        )
        check_max(
            failures,
            f"{key}.full_paint_p95",
            grid["full_paint_ns"]["p95"],
            budget["full_paint_p95_ns_max"],
        )
        check_max(
            failures,
            f"{key}.one_row_prepare_p95",
            grid["one_row_prepare_ns"]["p95"],
            budget["one_row_prepare_p95_ns_max"],
        )
        check_max(
            failures,
            f"{key}.one_row_paint_p95",
            grid["one_row_paint_ns"]["p95"],
            budget["one_row_paint_p95_ns_max"],
        )
        check_max(
            failures,
            f"{key}.repopulate",
            grid["forced_eviction"]["repopulate_ns"],
            budget["repopulate_ns_max"],
        )
        for name, value in grid["scale_invalidation_ns"].items():
            check_max(failures, f"{key}.scale_{name}", value, budget["invalidation_ns_max"])
        for name, value in grid["theme_invalidation_ns"].items():
            check_max(failures, f"{key}.theme_{name}", value, budget["invalidation_ns_max"])
        check_max(
            failures,
            f"{key}.rss",
            grid["rss_bytes_after_grid"],
            budget["rss_bytes_max"],
        )
        cache = grid["glyph_cache"]
        if cache["entries"] > cache["glyph_budget"]:
            failures.append(f"{key}.glyph entries exceed budget")
        if cache["approximate_bytes"] > cache["glyph_byte_budget"]:
            failures.append(f"{key}.glyph bytes exceed budget")
        if cache["raster_faces"] > cache["raster_face_budget"]:
            failures.append(f"{key}.raster faces exceed budget")


def validate_daemon(
    report: dict[str, Any], thresholds: dict[str, Any], failures: list[str]
) -> None:
    if report.get("profile") != "release":
        failures.append("daemon benchmark was not a release build")
    check_max(failures, "daemon.output", report.get("output_ns"), thresholds["output_ns_max"])
    check_max(
        failures,
        "daemon.snapshot_p95",
        report.get("snapshot_ns", {}).get("p95"),
        thresholds["snapshot_p95_ns_max"],
    )
    check_max(
        failures,
        "daemon.page_p95",
        report.get("paging", {}).get("fetch_ns", {}).get("p95"),
        thresholds["page_fetch_p95_ns_max"],
    )
    check_max(
        failures,
        "daemon.page_bytes",
        report.get("paging", {}).get("approximate_retained_bytes"),
        thresholds["page_bytes_max"],
    )
    check_max(failures, "daemon.resize", report.get("resize", {}).get("ns"), thresholds["resize_ns_max"])
    check_max(
        failures,
        "daemon.input_response",
        report.get("post_output_input_response_ns"),
        thresholds["post_output_input_response_ns_max"],
    )
    check_max(
        failures,
        "daemon.rss",
        report.get("rss_bytes", {}).get("after"),
        thresholds["rss_bytes_max"],
    )
    if not report.get("subscriber_resnapshot_required"):
        failures.append("stalled subscriber did not require bounded resnapshot")
    if not report.get("resize", {}).get("generation_advanced"):
        failures.append("resize did not advance the history generation")
    history = report.get("history", {})
    if history.get("available_rows", 0) > history.get("effective_row_bound", -1):
        failures.append("daemon history exceeded its effective row bound")
    metrics = report.get("runtime_metrics", {})
    bounds = report.get("bounds", {})
    if metrics.get("command_queue_high_water", 0) > bounds.get("command_capacity", -1):
        failures.append("daemon command queue exceeded capacity")
    if metrics.get("user_write_queue_high_water_bytes", 0) > bounds.get("input_byte_limit", -1):
        failures.append("daemon user write queue exceeded byte limit")
    if metrics.get("reply_write_queue_high_water_bytes", 0) > bounds.get("reply_byte_limit", -1):
        failures.append("daemon reply write queue exceeded byte limit")
    if metrics.get("pty_read_calls", 0) <= 0:
        failures.append("daemon did not record PTY read calls")
    if metrics.get("pty_read_bytes", 0) <= 0:
        failures.append("daemon did not consume PTY output")
    if metrics.get("output_parse_batches", 0) <= 0:
        failures.append("daemon did not record output parse batches")
    if metrics.get("output_terminal_updates", 0) <= 0:
        failures.append("daemon did not record terminal update amplification")
    if metrics.get("output_live_events", 0) <= 0:
        failures.append("daemon did not record live output events")
    if metrics.get("output_processing_ns", 0) <= 0:
        failures.append("daemon did not record output processing time")
    if metrics.get("snapshot_builds", 0) <= 0:
        failures.append("daemon did not record snapshot builds")
    if metrics.get("snapshot_build_ns", 0) <= 0:
        failures.append("daemon did not record snapshot build time")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("output_dir", type=pathlib.Path)
    parser.add_argument("--samples", type=int, default=10)
    parser.add_argument("--skip-build", action="store_true")
    args = parser.parse_args()
    if args.samples <= 0:
        parser.error("--samples must be positive")
    args.output_dir.mkdir(parents=True, exist_ok=True)
    renderer_path = args.output_dir / "renderer.json"
    daemon_path = args.output_dir / "daemon.json"
    summary_path = args.output_dir / "summary.json"
    try:
        if not args.skip_build:
            run(["cargo", "build", "--release", "-p", "splinterm", "--example", "phase4-renderer-benchmark"])
            run(["cargo", "build", "--release", "-p", "splinterm-pty", "--bin", "splinterm-pty-child"])
            run(["cargo", "build", "--release", "-p", "splinterd", "--example", "phase9-daemon-benchmark"])
        run([str(ROOT / "target/release/examples/phase4-renderer-benchmark"), str(args.samples)], stdout=renderer_path)
        run([str(ROOT / "target/release/examples/phase9-daemon-benchmark")], stdout=daemon_path)
        renderer = json.loads(renderer_path.read_text(encoding="utf-8"))
        daemon = json.loads(daemon_path.read_text(encoding="utf-8"))
        thresholds = json.loads(THRESHOLDS.read_text(encoding="utf-8"))
        failures: list[str] = []
        validate_renderer(renderer, thresholds["renderer"], failures)
        validate_daemon(daemon, thresholds["daemon"], failures)
        summary = {
            "schema": "splinterm.performance.phase9.v1",
            "exact": not failures,
            "host": host_context(),
            "thresholds": thresholds,
            "renderer": renderer,
            "daemon": daemon,
            "failures": failures,
        }
        summary_path.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        if failures:
            print("\n".join(failures), file=sys.stderr)
            return 1
        print(f"Phase 9 baseline passed: {summary_path}")
        return 0
    except (OSError, ValueError, KeyError, RuntimeError) as error:
        summary_path.write_text(json.dumps({"exact": False, "error": str(error)}, indent=2) + "\n", encoding="utf-8")
        print(f"phase9 baseline error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
