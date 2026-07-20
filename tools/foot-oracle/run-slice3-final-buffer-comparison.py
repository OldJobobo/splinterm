#!/usr/bin/env python3
"""Run the bounded source-first Slice 3 matrix on inactive workspace 8 / DP-2."""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools/foot-oracle"
DEFAULT_OUTPUT = Path("/tmp/splinterm-slice3-final")
DEFAULT_MANIFEST = TOOLS / "slice3-final-buffer-fixtures.json"
SELECTED_CASES = {
    "underline-single-default",
    "underline-curly-rgb",
    "underline-dotted",
    "italic-overhang-adjacent",
    "cursor-beam-reverse",
    "unfocused-hollow-from-block",
}


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


V1 = load_module("final_buffer_v1_runner", TOOLS / "run-final-buffer-comparison.py")
FIXTURES = load_module("slice3_fixtures", TOOLS / "slice3-fixtures.py")
COMPARATOR = load_module("slice3_comparator", TOOLS / "compare-slice3-final-buffers.py")


def monitor_state() -> dict[str, Any]:
    monitor = next(
        (item for item in V1.hyprland_json("monitors all") if item.get("name") == V1.TEST_MONITOR),
        None,
    )
    if monitor is None or monitor.get("disabled"):
        raise RuntimeError(f"required monitor {V1.TEST_MONITOR} is unavailable")
    return monitor


def monitor_expression(monitor: dict[str, Any], scale: float) -> str:
    mode = f"{monitor['width']}x{monitor['height']}@{float(monitor['refreshRate']):.5f}"
    position = f"{monitor['x']}x{monitor['y']}"
    return (
        "hl.monitor({ "
        f"output = {json.dumps(V1.TEST_MONITOR)}, mode = {json.dumps(mode)}, "
        f"position = {json.dumps(position)}, scale = {scale:g}, "
        f"transform = {int(monitor.get('transform', 0))} "
        "})"
    )


def apply_monitor_scale(original: dict[str, Any], scale_120: int) -> None:
    expression = monitor_expression(original, scale_120 / 120)
    result = V1.run(["hyprctl", "eval", expression], capture_output=True, timeout=5)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or result.stdout.strip())
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        current = monitor_state()
        if abs(float(current["scale"]) - scale_120 / 120) < 0.001:
            V1.assert_test_workspace_isolated()
            V1.assert_user_workspace_untouched()
            return
        time.sleep(0.05)
    raise RuntimeError(f"DP-2 did not reach scale {scale_120}/120")


def restore_monitor(original: dict[str, Any]) -> None:
    result = V1.run(
        ["hyprctl", "eval", monitor_expression(original, float(original["scale"]))],
        capture_output=True,
        timeout=5,
    )
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or result.stdout.strip())


def v1_preflight() -> dict[str, Any]:
    provenance = json.loads((TOOLS / "provenance.json").read_text())
    profile = provenance["default_final_buffer_profile"]
    manifest = {
        "profile": {
            "font": profile["font_pattern"].removesuffix(":pixelsize=12"),
            "font_size": profile["logical_size_px"],
            "scale_120": profile["scale_120"],
            "padding": profile["padding"]["left"],
            "foreground": profile["foreground"],
            "background": profile["background"],
        }
    }
    return V1.preflight_provenance(manifest)


def capture_splinterm(
    binary: Path,
    case_dir: Path,
    profile: dict[str, Any],
    case: dict[str, Any],
    logical_size: list[int] | None = None,
) -> dict[str, Any]:
    prefix = case_dir / "splinterm"
    cells = case_dir / "cells.json"
    cells.write_text(json.dumps(FIXTURES.splinterm_cells(case)), encoding="utf-8")
    columns, rows = len(case["cells"][0]), len(case["cells"])
    command = [
        str(binary), "--output-prefix", str(prefix), "--fixture", case["id"],
        "--frame-id", case["id"], "--cells-json", str(cells),
        "--font", profile["font"], "--font-size", str(profile["font_size"]),
        "--font-size-unit", profile["font_size_unit"], "--scale-120", str(case["scale_120"]),
        "--columns", str(columns), "--rows", str(rows),
        "--cursor-shape", case["configured_cursor"],
        "--cursor-column", str(case["cursor"][0]), "--cursor-row", str(case["cursor"][1]),
        "--capture-schema", "slice3-v2", "--target-focus-semantics", case["lane"],
        "--unfocused-style", case["unfocused_style"],
    ]
    if not case["cursor_visible"]:
        command.append("--hide-cursor")
    if logical_size is not None:
        command.extend(["--logical-width", str(logical_size[0]), "--logical-height", str(logical_size[1])])
    result = V1.run(command, cwd=ROOT, capture_output=True)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or "Splinterm Slice 3 capture failed")
    return json.loads(prefix.with_suffix(".json").read_text())


def capture_case(
    foot: Path,
    splinterm: Path,
    output: Path,
    profile: dict[str, Any],
    provenance: dict[str, Any],
    case: dict[str, Any],
    monitor_id: int,
) -> dict[str, Any]:
    case_dir = output / case["id"]
    shutil.rmtree(case_dir, ignore_errors=True)
    case_dir.mkdir(parents=True)
    actual = capture_splinterm(splinterm, case_dir, profile, case)
    logical = actual["provenance"]["window_geometry"]["surface_logical_size"]
    columns, rows = len(case["cells"][0]), len(case["cells"])
    adapter = {"id": case["id"], "columns": columns, "rows": rows}
    foot_profile = {
        "font": profile["font"], "font_size": profile["font_size"],
        "padding": profile["padding"], "foreground": profile["foreground"],
        "background": profile["background"],
    }
    reference = V1.capture_foot(
        foot, case_dir / "foot", case_dir, foot_profile, provenance, adapter,
        V1.TEST_WORKSPACE, monitor_id, logical["width"], logical["height"],
        payload_override=FIXTURES.foot_payload(case),
        extra_environment=["FOOT_ORACLE_SCHEMA=slice3-v2", f"FOOT_ORACLE_TARGET_FOCUS={case['lane']}"],
        extra_overrides=[
            f"--override=cursor.unfocused-style={case['unfocused_style']}",
            "--override=dpi-aware=no",
        ],
    )
    observed_logical = reference.pop("_oracle_logical_size")
    if observed_logical != [logical["width"], logical["height"]]:
        actual = capture_splinterm(splinterm, case_dir, profile, case, observed_logical)
    report = COMPARATOR.compare(case_dir / "foot.json", case_dir / "splinterm.json", case_dir / "diff")
    return {
        "id": case["id"], "scale_120": case["scale_120"], "lane": case["lane"],
        "exact": bool(report["exact"]), "mismatch_pixels": report["mismatch_pixels"],
        "maximum_channel_delta": report["maximum_channel_delta"],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output_dir", type=Path, nargs="?", default=DEFAULT_OUTPUT)
    parser.add_argument("--workspace", type=int, default=V1.TEST_WORKSPACE)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--case", action="append", dest="case_ids")
    args = parser.parse_args()
    if args.workspace != V1.TEST_WORKSPACE or not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        parser.error("Slice 3 graphical tests are restricted to workspace 8 on DP-2")
    manifest = FIXTURES.load_manifest(args.manifest)
    default_selected = (
        SELECTED_CASES
        if args.manifest.resolve() == DEFAULT_MANIFEST.resolve()
        else {case["id"] for case in manifest["cases"]}
    )
    selected = set(args.case_ids or default_selected)
    cases = sorted(
        (case for case in manifest["cases"] if case["id"] in selected),
        key=lambda case: (case["scale_120"], case["id"]),
    )
    if {case["id"] for case in cases} != selected:
        parser.error("unknown or missing selected Slice 3 case")
    original = monitor_state()
    monitor_id = V1.assert_test_workspace_isolated()
    provenance = v1_preflight()
    build = V1.run([str(TOOLS / "build-oracle.sh")], cwd=ROOT)
    if build.returncode:
        return build.returncode
    build = V1.run(["cargo", "build", "-q", "-p", "splinterm", "--bin", "final-buffer-capture"], cwd=ROOT)
    if build.returncode:
        return build.returncode
    foot = Path(os.environ.get("FOOT_ORACLE_BUILD", V1.DEFAULT_BUILD)) / "foot"
    splinterm = ROOT / "target/debug/final-buffer-capture"
    args.output_dir.mkdir(parents=True, exist_ok=True)
    records: list[dict[str, Any]] = []
    try:
        for case in cases:
            apply_monitor_scale(original, case["scale_120"])
            try:
                record = capture_case(foot, splinterm, args.output_dir, manifest["profile"], provenance, case, monitor_id)
            except (OSError, RuntimeError, ValueError, json.JSONDecodeError) as error:
                record = {"id": case["id"], "scale_120": case["scale_120"], "lane": case["lane"], "exact": False, "error": str(error)}
            records.append(record)
            print(f"{'PASS' if record['exact'] else 'FAIL'} {case['id']}")
    finally:
        restore_monitor(original)
    schema = (
        "splinterm.final-buffer.slice3-matrix.v2"
        if args.manifest.resolve() == DEFAULT_MANIFEST.resolve()
        else "splinterm.final-buffer.slice4-matrix.v1"
    )
    summary = {"schema": schema, "source_first": True, "cases": records, "exact": all(item["exact"] for item in records)}
    (args.output_dir / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
    print(f"Slice 3 matrix: {sum(item['exact'] for item in records)}/{len(records)} exact")
    return 0 if summary["exact"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
