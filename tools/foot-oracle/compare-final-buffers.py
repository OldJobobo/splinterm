#!/usr/bin/env python3
"""Compare declared final ARGB buffers without image translation."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

SCHEMA = "splinterm.final-buffer.v1"
MAX_DIMENSION = 32768
MAX_RAW_BYTES = 256 * 1024 * 1024
MAX_METADATA_BYTES = 1024 * 1024


class CaptureError(ValueError):
    pass


def _integer(value: Any, name: str, minimum: int, maximum: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or not minimum <= value <= maximum:
        raise CaptureError(f"{name} must be an integer in {minimum}..{maximum}")
    return value


def _object(value: Any, name: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise CaptureError(f"{name} must be an object")
    return value


def load_capture(metadata_path: Path, raw_path: Path | None = None) -> tuple[dict[str, Any], bytes]:
    size = metadata_path.stat().st_size
    if size <= 0 or size > MAX_METADATA_BYTES:
        raise CaptureError("metadata size is outside the supported bounds")
    try:
        metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise CaptureError(f"invalid metadata JSON: {error}") from error
    metadata = _object(metadata, "metadata")
    if metadata.get("schema") != SCHEMA:
        raise CaptureError("unsupported capture schema")
    if (metadata.get("format"), metadata.get("byte_order"), metadata.get("endianness")) != (
        "argb8888",
        "bgra",
        "little",
    ):
        raise CaptureError("unsupported pixel representation")

    width = _integer(metadata.get("width"), "width", 1, MAX_DIMENSION)
    height = _integer(metadata.get("height"), "height", 1, MAX_DIMENSION)
    stride = _integer(metadata.get("stride"), "stride", width * 4, MAX_RAW_BYTES)
    raw_length = stride * height
    if raw_length > MAX_RAW_BYTES:
        raise CaptureError("raw buffer exceeds the capture limit")

    grid = _object(metadata.get("grid"), "grid")
    columns = _integer(grid.get("columns"), "grid.columns", 1, 4096)
    rows = _integer(grid.get("rows"), "grid.rows", 1, 4096)
    cell = _object(metadata.get("cell"), "cell")
    cell_width = _integer(cell.get("width"), "cell.width", 1, MAX_DIMENSION)
    cell_height = _integer(cell.get("height"), "cell.height", 1, MAX_DIMENSION)
    _integer(cell.get("baseline"), "cell.baseline", -MAX_DIMENSION, MAX_DIMENSION)
    padding = _object(metadata.get("padding"), "padding")
    edges = {
        name: _integer(padding.get(name), f"padding.{name}", 0, MAX_DIMENSION)
        for name in ("left", "right", "top", "bottom")
    }
    origin = _object(metadata.get("origin"), "origin")
    origin_x = _integer(origin.get("x"), "origin.x", 0, MAX_DIMENSION)
    origin_y = _integer(origin.get("y"), "origin.y", 0, MAX_DIMENSION)
    if origin_x != edges["left"] or origin_y != edges["top"]:
        raise CaptureError("origin must equal the declared left/top padding")
    if edges["left"] + columns * cell_width + edges["right"] != width:
        raise CaptureError("horizontal geometry does not cover the buffer")
    if edges["top"] + rows * cell_height + edges["bottom"] != height:
        raise CaptureError("vertical geometry does not cover the buffer")
    _integer(metadata.get("scale_120"), "scale_120", 120, 960)
    for name in ("fixture", "frame_id"):
        if not isinstance(metadata.get(name), str) or not metadata[name] or len(metadata[name]) > 256:
            raise CaptureError(f"{name} must be a non-empty bounded string")
    background = metadata.get("background_bgra")
    if not isinstance(background, list) or len(background) != 4:
        raise CaptureError("background_bgra must contain four channels")
    for index, channel in enumerate(background):
        _integer(channel, f"background_bgra[{index}]", 0, 255)
    cursor = metadata.get("cursor")
    if cursor is not None:
        cursor = _object(cursor, "cursor")
        _integer(cursor.get("column"), "cursor.column", 0, columns - 1)
        _integer(cursor.get("row"), "cursor.row", 0, rows - 1)
        if cursor.get("shape") not in ("block", "beam", "underline"):
            raise CaptureError("cursor.shape is unsupported")
    composition = metadata.get("composition")
    if composition != ["terminal-backgrounds", "glyphs", "decorations", "cursor"]:
        raise CaptureError("composition does not describe the production terminal layers")
    _object(metadata.get("provenance"), "provenance")

    raw_path = raw_path or metadata_path.with_suffix(".argb")
    raw = raw_path.read_bytes()
    if len(raw) != raw_length:
        raise CaptureError(f"raw length {len(raw)} does not equal stride*height {raw_length}")
    return metadata, raw


def _pixel(raw: bytes, stride: int, x: int, y: int) -> bytes:
    offset = y * stride + x * 4
    return raw[offset : offset + 4]


def _ink_clearances(metadata: dict[str, Any], raw: bytes) -> dict[str, int | None]:
    width, height, stride = metadata["width"], metadata["height"], metadata["stride"]
    background = bytes(metadata["background_bgra"])
    minimum_x = minimum_y = None
    maximum_x = maximum_y = None
    for y in range(height):
        for x in range(width):
            if _pixel(raw, stride, x, y) == background:
                continue
            minimum_x = x if minimum_x is None else min(minimum_x, x)
            maximum_x = x if maximum_x is None else max(maximum_x, x)
            minimum_y = y if minimum_y is None else min(minimum_y, y)
            maximum_y = y if maximum_y is None else max(maximum_y, y)
    if minimum_x is None:
        return {edge: None for edge in ("left", "right", "top", "bottom")}
    return {
        "left": minimum_x,
        "right": width - 1 - maximum_x,
        "top": minimum_y,
        "bottom": height - 1 - maximum_y,
    }


def _write_ppm(path: Path, pixels: list[tuple[int, int, int]], width: int, height: int) -> None:
    body = bytearray()
    for red, green, blue in pixels:
        body.extend((red, green, blue))
    path.write_bytes(f"P6\n{width} {height}\n255\n".encode() + body)


def compare(reference: tuple[dict[str, Any], bytes], actual: tuple[dict[str, Any], bytes], output: Path) -> dict[str, Any]:
    ref_meta, ref_raw = reference
    act_meta, act_raw = actual
    comparable = (
        "width",
        "height",
        "format",
        "byte_order",
        "endianness",
        "scale_120",
        "grid",
        "cell",
        "padding",
        "origin",
        "cursor",
        "fixture",
        "frame_id",
        "background_bgra",
        "composition",
    )
    differences = [name for name in comparable if ref_meta.get(name) != act_meta.get(name)]
    if differences:
        raise CaptureError("capture contracts differ: " + ", ".join(differences))

    width, height = ref_meta["width"], ref_meta["height"]
    mismatch_count = 0
    maximum_channel_delta = 0
    bounds: list[int] | None = None
    first_cell: dict[str, int] | None = None
    per_cell: dict[str, int] = {}
    heatmap: list[tuple[int, int, int]] = []
    for y in range(height):
        for x in range(width):
            expected = _pixel(ref_raw, ref_meta["stride"], x, y)
            observed = _pixel(act_raw, act_meta["stride"], x, y)
            delta = max(abs(left - right) for left, right in zip(expected, observed, strict=True))
            if delta:
                mismatch_count += 1
                maximum_channel_delta = max(maximum_channel_delta, delta)
                bounds = [x, y, x, y] if bounds is None else [min(bounds[0], x), min(bounds[1], y), max(bounds[2], x), max(bounds[3], y)]
                padding = ref_meta["padding"]
                cell = ref_meta["cell"]
                column = (x - padding["left"]) // cell["width"]
                row = (y - padding["top"]) // cell["height"]
                if 0 <= column < ref_meta["grid"]["columns"] and 0 <= row < ref_meta["grid"]["rows"]:
                    key = f"{row},{column}"
                    per_cell[key] = per_cell.get(key, 0) + 1
                    if first_cell is None:
                        first_cell = {"row": row, "column": column}
            heatmap.append((delta, 0, 0))

    ref_clearance = _ink_clearances(ref_meta, ref_raw)
    act_clearance = _ink_clearances(act_meta, act_raw)
    clearance_delta = {
        edge: None if ref_clearance[edge] is None or act_clearance[edge] is None else act_clearance[edge] - ref_clearance[edge]
        for edge in ref_clearance
    }
    report = {
        "schema": "splinterm.final-buffer-comparison.v1",
        "reference_frame_id": ref_meta["frame_id"],
        "actual_frame_id": act_meta["frame_id"],
        "width": width,
        "height": height,
        "mismatch_pixels": mismatch_count,
        "maximum_channel_delta": maximum_channel_delta,
        "mismatch_bounds": bounds,
        "first_divergent_cell": first_cell,
        "per_cell_mismatch_pixels": per_cell,
        "reference_edge_clearance": ref_clearance,
        "actual_edge_clearance": act_clearance,
        "edge_clearance_delta": clearance_delta,
        "exact": mismatch_count == 0,
    }
    output.mkdir(parents=True, exist_ok=True)
    (output / "comparison.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    _write_ppm(output / "heatmap.ppm", heatmap, width, height)
    if bounds is not None:
        left, top, right, bottom = bounds
        reference_crop = []
        actual_crop = []
        difference_crop = []
        for y in range(top, bottom + 1):
            for x in range(left, right + 1):
                expected = _pixel(ref_raw, ref_meta["stride"], x, y)
                observed = _pixel(act_raw, act_meta["stride"], x, y)
                ref_blue, ref_green, ref_red, _ref_alpha = expected
                act_blue, act_green, act_red, _act_alpha = observed
                reference_crop.append((ref_red, ref_green, ref_blue))
                actual_crop.append((act_red, act_green, act_blue))
                difference_crop.append(
                    (
                        abs(ref_red - act_red),
                        abs(ref_green - act_green),
                        abs(ref_blue - act_blue),
                    )
                )
        crop_width = right - left + 1
        crop_height = bottom - top + 1
        _write_ppm(output / "reference-mismatch-crop.ppm", reference_crop, crop_width, crop_height)
        _write_ppm(output / "actual-mismatch-crop.ppm", actual_crop, crop_width, crop_height)
        _write_ppm(output / "difference-mismatch-crop.ppm", difference_crop, crop_width, crop_height)
    return report


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--reference-metadata", type=Path, required=True)
    parser.add_argument("--reference-raw", type=Path)
    parser.add_argument("--actual-metadata", type=Path, required=True)
    parser.add_argument("--actual-raw", type=Path)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    try:
        report = compare(
            load_capture(args.reference_metadata, args.reference_raw),
            load_capture(args.actual_metadata, args.actual_raw),
            args.output_dir,
        )
    except (CaptureError, OSError) as error:
        parser.error(str(error))
    print(json.dumps(report, sort_keys=True))
    return 0 if report["exact"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
