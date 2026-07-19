import importlib.util
import json
from pathlib import Path

PATH = Path(__file__).with_name("slice3-fixtures.py")
SPEC = importlib.util.spec_from_file_location("slice3_fixtures", PATH)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def test_dual_builders_preserve_exact_vt_and_semantic_attributes():
    cases = MODULE.load_manifest()["cases"]
    curly = next(case for case in cases if case["id"] == "underline-curly-rgb")
    payload = MODULE.foot_payload(curly)
    assert payload.startswith(b"\x1b[?25l")
    assert bytes.fromhex(curly["vt_hex"]) in payload
    assert payload.endswith(b"\x1b[1;1H")
    cell = MODULE.splinterm_cells(curly)["rows"][0][0]
    assert cell["attributes"]["underline"] == "curly"
    assert cell["attributes"]["underline_color_source"] == "rgb"
    assert cell["attributes"]["underline_color"] == 0x123456


def test_wide_leader_and_spacer_translate_to_protocol_cells(tmp_path):
    case = next(
        case for case in MODULE.load_manifest()["cases"] if case["id"] == "underline-dashed-wide"
    )
    payload = MODULE.splinterm_cells(case)
    assert payload["rows"][0][0]["content"] == "界"
    assert payload["rows"][0][1]["content"] == ""
    assert payload["rows"][0][1]["spacer_remaining"] == 1
    # The production capture CLI consumes this exact JSON shape.
    path = tmp_path / "cells.json"
    path.write_text(json.dumps(payload))
    assert json.loads(path.read_text()) == payload


def test_cursor_lanes_have_truthful_effective_expectations():
    cases = MODULE.load_manifest()["cases"]
    unfocused = [case for case in cases if case["lane"] == "unfocused"]
    assert {case["configured_cursor"] for case in unfocused} == {"block", "beam", "underline"}
    assert all(case["cursor_visible"] for case in unfocused)
