import importlib.util
import json
from pathlib import Path

import pytest

MODULE_PATH = Path(__file__).with_name("compare-final-buffers.py")
SPEC = importlib.util.spec_from_file_location("compare_final_buffers", MODULE_PATH)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def metadata(stride=16):
    return {
        "schema": "splinterm.final-buffer.v1",
        "width": 4,
        "height": 4,
        "stride": stride,
        "format": "argb8888",
        "byte_order": "bgra",
        "endianness": "little",
        "scale_120": 120,
        "grid": {"columns": 2, "rows": 2},
        "cell": {"width": 1, "height": 1, "baseline": 1},
        "padding": {"left": 1, "right": 1, "top": 1, "bottom": 1},
        "origin": {"x": 1, "y": 1},
        "cursor": None,
        "fixture": "test",
        "frame_id": "frame",
        "background_bgra": [0, 0, 0, 255],
        "composition": ["terminal-backgrounds", "glyphs", "decorations", "cursor"],
        "provenance": {},
    }


def write_capture(root, name, meta, visible, row_padding=b""):
    metadata_path = root / f"{name}.json"
    raw_path = root / f"{name}.argb"
    metadata_path.write_text(json.dumps(meta), encoding="utf-8")
    rows = []
    tight = meta["width"] * 4
    for row in range(meta["height"]):
        rows.append(visible[row * tight : (row + 1) * tight] + row_padding)
    raw_path.write_bytes(b"".join(rows))
    return MODULE.load_capture(metadata_path, raw_path)


def background_pixels():
    return bytes([0, 0, 0, 255] * 16)


def test_exact_comparison_ignores_declared_stride_padding(tmp_path):
    pixels = background_pixels()
    reference = write_capture(tmp_path, "reference", metadata(), pixels)
    actual_meta = metadata(20)
    actual_meta["frame_id"] = "actual"
    actual = write_capture(tmp_path, "actual", actual_meta, pixels, b"pad!")
    report = MODULE.compare(reference, actual, tmp_path / "diff")
    assert report["exact"]
    assert report["mismatch_pixels"] == 0
    assert (tmp_path / "diff" / "heatmap.ppm").exists()


def test_mismatch_reports_bounds_delta_and_cell(tmp_path):
    reference_pixels = bytearray(background_pixels())
    actual_pixels = bytearray(reference_pixels)
    offset = (1 * 4 + 2) * 4
    actual_pixels[offset : offset + 4] = bytes([10, 20, 30, 255])
    reference = write_capture(tmp_path, "reference", metadata(), reference_pixels)
    actual_meta = metadata()
    actual_meta["frame_id"] = "actual"
    actual = write_capture(tmp_path, "actual", actual_meta, actual_pixels)
    report = MODULE.compare(reference, actual, tmp_path / "diff")
    assert not report["exact"]
    assert report["mismatch_pixels"] == 1
    assert report["maximum_channel_delta"] == 30
    assert report["mismatch_bounds"] == [2, 1, 2, 1]
    assert report["first_divergent_cell"] == {"row": 0, "column": 1}
    assert report["per_cell_mismatch_pixels"] == {"0,1": 1}
    assert (tmp_path / "diff" / "actual-mismatch-crop.ppm").exists()


def test_parser_rejects_geometry_raw_length_and_malformed_json(tmp_path):
    pixels = background_pixels()
    invalid = metadata()
    invalid["origin"]["x"] = 0
    with pytest.raises(MODULE.CaptureError):
        write_capture(tmp_path, "origin", invalid, pixels)

    meta = metadata()
    metadata_path = tmp_path / "short.json"
    raw_path = tmp_path / "short.argb"
    metadata_path.write_text(json.dumps(meta), encoding="utf-8")
    raw_path.write_bytes(b"short")
    with pytest.raises(MODULE.CaptureError):
        MODULE.load_capture(metadata_path, raw_path)

    metadata_path.write_text("{", encoding="utf-8")
    with pytest.raises(MODULE.CaptureError):
        MODULE.load_capture(metadata_path, raw_path)


def test_comparator_rejects_declared_origin_difference(tmp_path):
    pixels = background_pixels()
    reference = write_capture(tmp_path, "reference", metadata(), pixels)
    actual_meta = metadata()
    actual_meta["padding"] = {"left": 0, "right": 2, "top": 1, "bottom": 1}
    actual_meta["origin"] = {"x": 0, "y": 1}
    actual = write_capture(tmp_path, "actual", actual_meta, pixels)
    with pytest.raises(MODULE.CaptureError, match="capture contracts differ"):
        MODULE.compare(reference, actual, tmp_path / "diff")
