import copy
import importlib.util
import json
from pathlib import Path

import pytest

MODULE_PATH = Path(__file__).with_name("validate-slice3-fixtures.py")
SPEC = importlib.util.spec_from_file_location("slice3_validator", MODULE_PATH)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
MANIFEST = json.loads(Path(__file__).with_name("slice3-final-buffer-fixtures.json").read_text())


def test_checked_in_slice3_manifest_is_valid_and_covers_required_axes():
    value = MODULE.validate_manifest(copy.deepcopy(MANIFEST))
    cases = value["cases"]
    assert {case["scale_120"] for case in cases} == {120, 150, 180, 240}
    assert {case["lane"] for case in cases} == {"focused-steady", "unfocused"}
    underlines = {
        cell.get("underline")
        for case in cases
        for row in case["cells"]
        for cell in row
        if "underline" in cell
    }
    assert underlines == {"single", "double", "curly", "dotted", "dashed"}
    assert any(cell.get("wide") == 2 for case in cases for row in case["cells"] for cell in row)
    assert any(cell.get("italic") for case in cases for row in case["cells"] for cell in row)


def test_unknown_focus_mode_and_oversized_steps_are_rejected():
    value = copy.deepcopy(MANIFEST)
    value["cases"][0]["lane"] = "focused"
    with pytest.raises(ValueError, match="lane"):
        MODULE.validate_manifest(value)

    value = copy.deepcopy(MANIFEST)
    value["cases"][0]["vt_hex"] = "00" * 5000
    with pytest.raises(ValueError, match="vt_hex"):
        MODULE.validate_manifest(value)


def test_invalid_wide_cells_and_incompatible_scales_are_rejected():
    value = copy.deepcopy(MANIFEST)
    wide = next(case for case in value["cases"] if case["id"] == "underline-dashed-wide")
    wide["cells"][0][1] = {"content": "x"}
    with pytest.raises(ValueError, match="wide=2"):
        MODULE.validate_manifest(value)

    value = copy.deepcopy(MANIFEST)
    value["cases"][0]["scale_120"] = 130
    with pytest.raises(ValueError, match="scale"):
        MODULE.validate_manifest(value)


def test_unfocused_lane_requires_visible_cursor_and_cursor_inside_grid():
    value = copy.deepcopy(MANIFEST)
    unfocused = next(case for case in value["cases"] if case["lane"] == "unfocused")
    unfocused["cursor_visible"] = False
    with pytest.raises(ValueError, match="unfocused"):
        MODULE.validate_manifest(value)

    value = copy.deepcopy(MANIFEST)
    value["cases"][0]["cursor"] = [100, 0]
    with pytest.raises(ValueError, match="outside"):
        MODULE.validate_manifest(value)
