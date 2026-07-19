import importlib.util
import json
from pathlib import Path

import pytest

PATH = Path(__file__).with_name("compare-slice3-final-buffers.py")
SPEC = importlib.util.spec_from_file_location("slice3_compare", PATH)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def metadata():
    return {
        "schema": "splinterm.final-buffer.slice3.v2",
        "width": 2,
        "height": 1,
        "stride": 8,
        "format": "argb8888",
        "byte_order": "bgra",
        "endianness": "little",
        "scale_120": 120,
        "grid": {"columns": 1, "rows": 1},
        "cell": {"width": 2, "height": 1, "baseline": 1},
        "padding": {"left": 0, "right": 0, "top": 0, "bottom": 0},
        "origin": {"x": 0, "y": 0},
        "fixture": "cursor",
        "frame_id": "cursor",
        "background_bgra": [0, 0, 0, 255],
        "composition": "foot-cell-rtl-v1",
        "cursor": {
            "position": {"column": 0, "row": 0},
            "configured_shape": "block",
            "effective_shape": "hollow",
            "target_focus_semantics": "unfocused",
        },
        "capture_context": {"actual_keyboard_focus": False, "unfocused_style": "hollow"},
        "provenance": {"implementation": "test"},
    }


def write_capture(root, name, value, pixels=b"\0" * 8):
    path = root / f"{name}.json"
    path.write_text(json.dumps(value))
    path.with_suffix(".argb").write_bytes(pixels)
    return path


def test_exact_v2_comparison_and_focus_provenance_must_match(tmp_path):
    reference = metadata()
    actual = metadata()
    result = MODULE.compare(
        write_capture(tmp_path, "reference", reference),
        write_capture(tmp_path, "actual", actual),
        tmp_path / "diff",
    )
    assert result["exact"]
    actual["capture_context"]["actual_keyboard_focus"] = True
    with pytest.raises(ValueError, match="unfocused capture"):
        MODULE.compare(
            write_capture(tmp_path, "reference2", reference),
            write_capture(tmp_path, "actual2", actual),
            tmp_path / "diff2",
        )


def test_inconsistent_effective_shapes_and_unknown_modes_are_rejected():
    value = metadata()
    value["cursor"]["effective_shape"] = "beam"
    with pytest.raises(ValueError, match="hollow"):
        MODULE.validate_metadata(value)

    value = metadata()
    value["cursor"]["target_focus_semantics"] = "auto"
    with pytest.raises(ValueError, match="focus"):
        MODULE.validate_metadata(value)


def test_focused_steady_requires_unchanged_focus_and_configured_shape():
    value = metadata()
    value["cursor"].update(target_focus_semantics="focused-steady", effective_shape="block")
    value["capture_context"]["unfocused_style"] = "hollow"
    with pytest.raises(ValueError, match="focused-steady"):
        MODULE.validate_metadata(value)


def test_v1_bounds_geometry_and_boolean_dimensions_are_reused():
    value = metadata()
    value["width"] = True
    with pytest.raises(ValueError, match="width"):
        MODULE.validate_metadata(value)
    value = metadata()
    value["padding"]["right"] = 1
    with pytest.raises(ValueError, match="horizontal geometry"):
        MODULE.validate_metadata(value)


def test_pixel_mismatch_has_v1_diagnostics_and_artifacts(tmp_path):
    value = metadata()
    result = MODULE.compare(
        write_capture(tmp_path, "reference", value),
        write_capture(tmp_path, "actual", value, b"\1" + b"\0" * 7),
        tmp_path / "diff",
    )
    assert not result["exact"]
    assert result["mismatch_pixels"] == 1
    assert result["maximum_channel_delta"] == 1
    assert result["mismatch_bounds"] == [0, 0, 0, 0]
    assert (tmp_path / "diff" / "heatmap.ppm").is_file()
