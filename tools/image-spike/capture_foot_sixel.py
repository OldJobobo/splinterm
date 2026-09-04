#!/usr/bin/env python3
"""Capture one pinned-Foot Sixel seed under the repository graphical guard."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import pathlib
import shutil
import subprocess
import sys
from typing import Any

ROOT = pathlib.Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "fixtures/terminal-images/v1/protocol-fixtures/sixel-v1.json"
ORACLE_PATH = ROOT / "tools/foot-oracle/run-final-buffer-comparison.py"
PROVENANCE_CHECK = ROOT / "tools/foot-oracle/check-provenance.py"
BUILD = pathlib.Path("/tmp/splinterm-foot-oracle-build")


def load_oracle():
    spec = importlib.util.spec_from_file_location("splinterm_sixel_oracle", ORACLE_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


V1 = load_oracle()


def expand_rows(case: dict[str, Any]) -> list[bytes]:
    rows = []
    for row in case["expected"]["rows"]:
        value = bytearray()
        for count, pixel in row:
            value.extend(bytes.fromhex(pixel) * count)
        rows.append(bytes(value))
    return rows


def file_sha256(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def require_pinned_host() -> None:
    result = subprocess.run(
        [sys.executable, str(PROVENANCE_CHECK)],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "no diagnostic"
        raise RuntimeError(f"pinned Foot oracle prerequisite check failed: {detail}")


def sixel_provenance() -> dict[str, Any]:
    """Validate output-relevant inputs for the optional Foot differential."""

    require_pinned_host()
    path = ROOT / "tools/foot-oracle/provenance.json"
    provenance = json.loads(path.read_text(encoding="utf-8"))
    if provenance.get("schema") != 4:
        raise RuntimeError("unsupported oracle provenance schema")
    source = pathlib.Path(os.environ.get("FOOT_SOURCE", pathlib.Path.home() / "Playground/foot"))
    revision = subprocess.run(
        ["git", "-C", str(source), "rev-parse", "HEAD"],
        text=True,
        capture_output=True,
        check=True,
    ).stdout.strip()
    if revision != provenance["reference"]["commit"]:
        raise RuntimeError("Foot source revision drifted")
    if subprocess.run(["git", "-C", str(source), "diff", "--exit-code"], check=False).returncode:
        raise RuntimeError("Foot source checkout is dirty")
    for patch in provenance["oracle"]["patches"]:
        if file_sha256(ROOT / patch["path"]) != patch["sha256"]:
            raise RuntimeError(f"oracle patch hash drift: {patch['path']}")
    profile = provenance["default_final_buffer_profile"]
    match = subprocess.run(
        ["fc-match", "-f", "%{file}\\n%{index}\\n", profile["font_pattern"]],
        text=True,
        capture_output=True,
        check=True,
    ).stdout.splitlines()
    if (
        len(match) < 2
        or match[0] != profile["font_file"]
        or int(match[1]) != profile["font_index"]
        or file_sha256(pathlib.Path(match[0])) != profile["font_sha256"]
    ):
        raise RuntimeError("Foot oracle font provenance drifted")
    for package, key in (
        ("fcft", "fcft_version"),
        ("freetype2", "freetype_version"),
        ("fontconfig", "fontconfig_version"),
        ("pixman-1", "pixman_version"),
    ):
        version = subprocess.run(
            ["pkg-config", "--modversion", package],
            text=True,
            capture_output=True,
            check=True,
        ).stdout.strip()
        if version != provenance["build"][key]:
            raise RuntimeError(f"{package} provenance drifted")
    return provenance


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("output_dir", type=pathlib.Path)
    parser.add_argument("--case", required=True)
    parser.add_argument("--reuse-build", action="store_true")
    args = parser.parse_args()
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        parser.error("a running Hyprland session is required")

    args.output_dir = args.output_dir.resolve()
    fixture_document = json.loads(FIXTURES.read_text(encoding="utf-8"))
    case = next((item for item in fixture_document["cases"] if item["id"] == args.case), None)
    if case is None:
        parser.error("unknown Sixel fixture")

    manifest = V1.load_manifest(V1.DEFAULT_MANIFEST)
    provenance = sixel_provenance()
    monitor_id = V1.assert_test_workspace_isolated()
    V1.assert_user_workspace_untouched()
    if V1.workspace_clients(V1.TEST_WORKSPACE):
        parser.error("reserved workspace is occupied")

    foot_worktree = pathlib.Path("/tmp/splinterm-foot-oracle-worktree")
    state_patch = pathlib.Path(__file__).with_name("foot-sixel-state-dump.patch")
    if not args.reuse_build:
        subprocess.run([str(ROOT / "tools/foot-oracle/build-oracle.sh")], cwd=ROOT, check=True)
        subprocess.run(
            ["git", "apply", "--check", str(state_patch)],
            cwd=foot_worktree,
            check=True,
        )
        subprocess.run(["git", "apply", str(state_patch)], cwd=foot_worktree, check=True)
        subprocess.run(["ninja", "-C", str(BUILD), "foot"], check=True)
    foot = BUILD / "foot"
    if not foot.is_file():
        parser.error("patched Foot oracle binary is missing")

    case_dir = args.output_dir / case["id"]
    shutil.rmtree(case_dir, ignore_errors=True)
    case_dir.mkdir(parents=True)
    prefix = case_dir / "foot"
    state_path = case_dir / "foot-sixel-state.json"
    oracle_case = {
        "id": case["id"],
        "columns": 80,
        "rows": 24,
        "text": "",
        "style": "normal",
        "cursor": {"visible": False},
    }
    payload = b"\x1b[?25l" + bytes.fromhex(case["input_hex"])
    metadata = V1.capture_foot(
        foot,
        prefix,
        case_dir,
        manifest["profile"],
        provenance,
        oracle_case,
        V1.TEST_WORKSPACE,
        monitor_id,
        960,
        600,
        payload_override=payload,
        extra_environment=[f"SPLINTERM_FOOT_STATE_DUMP={state_path}"],
        extra_overrides=["--override=tweak.sixel=yes"],
        color_section="colors-dark",
    )
    V1.assert_test_workspace_isolated()
    V1.assert_user_workspace_untouched()

    state = json.loads(state_path.read_text(encoding="utf-8"))
    full = prefix.with_suffix(".argb").read_bytes()
    stride = int(metadata["stride"])
    origin_x = int(metadata["origin"]["x"])
    origin_y = int(metadata["origin"]["y"])
    expected_rows = expand_rows(case)
    expected_argb = "".join(
        f"{int.from_bytes(row[index : index + 4], 'little'):08x}"
        for row in expected_rows
        for index in range(0, len(row), 4)
    )
    sixels = state.get("sixels", [])
    semantic_exact = (
        len(sixels) == 1
        and sixels[0]["width"] == case["expected"]["width"]
        and sixels[0]["height"] == case["expected"]["height"]
        and sixels[0]["opaque"] == case["expected"]["opaque"]
        and sixels[0]["argb"] == expected_argb
    )
    observed_rows = []
    viewport_origin_matches = True
    for row_index, expected in enumerate(expected_rows):
        start = (origin_y + row_index) * stride + origin_x * 4
        observed = full[start : start + len(expected)]
        observed_rows.append(observed.hex())
        viewport_origin_matches = viewport_origin_matches and observed == expected

    if case["id"] == "transparent-green-trailing-trim":
        next_start = (origin_y + 1) * stride + origin_x * 4
        background = bytes(metadata["background_bgra"])
        viewport_origin_matches = (
            viewport_origin_matches and full[next_start : next_start + 4] == background
        )

    expected_pixels = {pixel for row in expected_rows for pixel in [row[index : index + 4] for index in range(0, len(row), 4)]}
    locations: dict[str, list[list[int]]] = {pixel.hex(): [] for pixel in expected_pixels}
    for y in range(int(metadata["height"])):
        for x in range(int(metadata["width"])):
            start = y * stride + x * 4
            pixel = full[start : start + 4]
            if pixel in expected_pixels and len(locations[pixel.hex()]) < 32:
                locations[pixel.hex()].append([x, y])

    exact = semantic_exact and viewport_origin_matches
    report = {
        "schema": "splinterm.phase5.foot-sixel-capture.v1",
        "case": case["id"],
        "exact": exact,
        "semantic_exact": semantic_exact,
        "viewport_origin_matches": viewport_origin_matches,
        "foot_commit": metadata["provenance"]["commit"],
        "foot_binary_sha256": file_sha256(foot),
        "capture_script_sha256": file_sha256(pathlib.Path(__file__)),
        "state_patch_sha256": file_sha256(state_patch),
        "oracle_patch_sha256": {
            patch.name: file_sha256(patch)
            for patch in sorted((ROOT / "tools/foot-oracle/patches").glob("*.patch"))
        },
        "source_argb_sha256": file_sha256(prefix.with_suffix(".argb")),
        "source_metadata_sha256": file_sha256(prefix.with_suffix(".json")),
        "state_sha256": file_sha256(state_path),
        "state_sixels": sixels,
        "expected_rows": [row.hex() for row in expected_rows],
        "observed_rows": observed_rows,
        "expected_pixel_locations_first_32": locations,
        "isolation": {
            "workspace": 8,
            "monitor": "DP-2",
            "no_initial_focus": True,
            "cleanup_verified": True,
        },
    }
    (case_dir / "report.json").write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    prefix.with_suffix(".capture").unlink(missing_ok=True)
    print(
        f"{'PASS' if exact else 'FAIL'} {case['id']}: "
        f"{case_dir / 'report.json'}"
    )
    return 0 if exact else 1


if __name__ == "__main__":
    raise SystemExit(main())
