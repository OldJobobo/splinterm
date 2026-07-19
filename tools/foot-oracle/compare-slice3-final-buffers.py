#!/usr/bin/env python3
"""Strict bounded comparator for truthful Slice 3 cursor/decorations captures."""

from __future__ import annotations

import argparse
import copy
import importlib.util
import json
from pathlib import Path
from typing import Any

ROOT = Path(__file__).parent
SPEC = importlib.util.spec_from_file_location("v1_final_buffer", ROOT / "compare-final-buffers.py")
assert SPEC and SPEC.loader
V1 = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(V1)

SCHEMA = "splinterm.final-buffer.slice3.v2"
SHAPES = {"block", "beam", "underline"}
EFFECTIVE = SHAPES | {"hollow", "none"}


def validate_metadata(value: Any) -> dict[str, Any]:
    value = V1._object(value, "metadata")
    if value.get("schema") != SCHEMA:
        raise ValueError("unsupported Slice 3 capture schema")
    if (value.get("format"), value.get("byte_order"), value.get("endianness")) != (
        "argb8888", "bgra", "little"
    ):
        raise ValueError("unsupported pixel representation")
    width = V1._integer(value.get("width"), "width", 1, V1.MAX_DIMENSION)
    height = V1._integer(value.get("height"), "height", 1, V1.MAX_DIMENSION)
    stride = V1._integer(value.get("stride"), "stride", width * 4, V1.MAX_RAW_BYTES)
    if stride * height > V1.MAX_RAW_BYTES:
        raise ValueError("raw buffer exceeds the capture limit")
    grid = V1._object(value.get("grid"), "grid")
    columns = V1._integer(grid.get("columns"), "grid.columns", 1, 4096)
    rows = V1._integer(grid.get("rows"), "grid.rows", 1, 4096)
    cell = V1._object(value.get("cell"), "cell")
    cell_width = V1._integer(cell.get("width"), "cell.width", 1, V1.MAX_DIMENSION)
    cell_height = V1._integer(cell.get("height"), "cell.height", 1, V1.MAX_DIMENSION)
    V1._integer(cell.get("baseline"), "cell.baseline", -V1.MAX_DIMENSION, V1.MAX_DIMENSION)
    padding = V1._object(value.get("padding"), "padding")
    edges = {
        name: V1._integer(padding.get(name), f"padding.{name}", 0, V1.MAX_DIMENSION)
        for name in ("left", "right", "top", "bottom")
    }
    origin = V1._object(value.get("origin"), "origin")
    origin_x = V1._integer(origin.get("x"), "origin.x", 0, V1.MAX_DIMENSION)
    origin_y = V1._integer(origin.get("y"), "origin.y", 0, V1.MAX_DIMENSION)
    if (origin_x, origin_y) != (edges["left"], edges["top"]):
        raise ValueError("origin must equal the declared left/top padding")
    if edges["left"] + columns * cell_width + edges["right"] != width:
        raise ValueError("horizontal geometry does not cover the buffer")
    if edges["top"] + rows * cell_height + edges["bottom"] != height:
        raise ValueError("vertical geometry does not cover the buffer")
    V1._integer(value.get("scale_120"), "scale_120", 120, 960)
    for name in ("fixture", "frame_id"):
        if not isinstance(value.get(name), str) or not value[name] or len(value[name]) > 256:
            raise ValueError(f"{name} must be a non-empty bounded string")
    background = value.get("background_bgra")
    if not isinstance(background, list) or len(background) != 4:
        raise ValueError("background_bgra must contain four channels")
    for index, channel in enumerate(background):
        V1._integer(channel, f"background_bgra[{index}]", 0, 255)
    if value.get("composition") != "foot-cell-rtl-v1":
        raise ValueError("invalid composition model")
    V1._object(value.get("provenance"), "provenance")

    cursor = V1._object(value.get("cursor"), "cursor")
    context = V1._object(value.get("capture_context"), "capture_context")
    configured = cursor.get("configured_shape")
    effective = cursor.get("effective_shape")
    target = cursor.get("target_focus_semantics")
    style = context.get("unfocused_style")
    if configured not in SHAPES or effective not in EFFECTIVE:
        raise ValueError("invalid configured/effective cursor shape")
    if target not in {"focused-steady", "unfocused"} or style not in {"unchanged", "hollow", "none"}:
        raise ValueError("invalid focus semantics")
    if not isinstance(context.get("actual_keyboard_focus"), bool):
        raise ValueError("actual_keyboard_focus must be boolean provenance")
    position = cursor.get("position")
    if position is not None:
        position = V1._object(position, "cursor.position")
        if set(position) != {"column", "row"}:
            raise ValueError("invalid cursor position")
        V1._integer(position.get("column"), "cursor.position.column", 0, columns - 1)
        V1._integer(position.get("row"), "cursor.position.row", 0, rows - 1)
    if position is None and effective != "none":
        raise ValueError("cursor without position must be effectively none")
    if target == "focused-steady" and (
        style != "unchanged" or (position is not None and effective != configured)
    ):
        raise ValueError("focused-steady cursor declaration is inconsistent")
    if target == "unfocused" and context["actual_keyboard_focus"]:
        raise ValueError("unfocused capture cannot attest keyboard focus")
    if target == "unfocused" and style == "hollow" and position is not None and effective != "hollow":
        raise ValueError("default unfocused cursor must be effectively hollow")
    return value


def read_capture(path: Path) -> tuple[dict[str, Any], bytes]:
    size = path.stat().st_size
    if size <= 0 or size > V1.MAX_METADATA_BYTES:
        raise ValueError("metadata size is outside the supported bounds")
    try:
        metadata = json.loads(path.read_text(encoding="utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError(f"invalid metadata JSON: {error}") from error
    metadata = validate_metadata(metadata)
    raw = path.with_suffix(".argb").read_bytes()
    expected = metadata["stride"] * metadata["height"]
    if len(raw) != expected:
        raise ValueError(f"raw length {len(raw)} does not equal stride*height {expected}")
    return metadata, raw


def _as_v1(capture: tuple[dict[str, Any], bytes]) -> tuple[dict[str, Any], bytes]:
    metadata, raw = capture
    normalized = copy.deepcopy(metadata)
    normalized["schema"] = V1.SCHEMA
    normalized["composition"] = ["terminal-backgrounds", "glyphs", "decorations", "cursor"]
    position = metadata["cursor"]["position"]
    normalized["cursor"] = None if position is None else {
        "column": position["column"],
        "row": position["row"],
        "shape": metadata["cursor"]["configured_shape"],
    }
    return normalized, raw


def compare(reference: Path, actual: Path, output_dir: Path | None = None) -> dict[str, Any]:
    reference_capture = read_capture(reference)
    actual_capture = read_capture(actual)
    ref_meta, _ = reference_capture
    got_meta, _ = actual_capture
    for key in ("configured_shape", "effective_shape", "target_focus_semantics", "position"):
        if ref_meta["cursor"].get(key) != got_meta["cursor"].get(key):
            raise ValueError(f"cursor metadata mismatch: {key}")
    if ref_meta["capture_context"] != got_meta["capture_context"]:
        raise ValueError("capture_context metadata mismatch")
    output_dir = output_dir or actual.parent / "diff"
    report = V1.compare(_as_v1(reference_capture), _as_v1(actual_capture), output_dir)
    report["schema"] = "splinterm.final-buffer.slice3-comparison.v2"
    report["cursor"] = copy.deepcopy(ref_meta["cursor"])
    (output_dir / "comparison.json").write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return report


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("reference", type=Path)
    parser.add_argument("actual", type=Path)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    try:
        result = compare(args.reference, args.actual, args.output_dir)
    except (OSError, ValueError) as error:
        parser.error(str(error))
    print(json.dumps(result, sort_keys=True))
    return 0 if result["exact"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
