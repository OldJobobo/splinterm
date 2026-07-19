import importlib.util
import json
import subprocess
from pathlib import Path

import pytest

MODULE_PATH = Path(__file__).with_name("run-final-buffer-comparison.py")
SPEC = importlib.util.spec_from_file_location("run_final_buffer_comparison", MODULE_PATH)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def manifest():
    return {
        "schema": "splinterm.final-buffer-fixtures.v1",
        "profile": {
            "font": "JetBrains Mono Nerd Font:style=Regular",
            "font_size": 12.0,
            "scale_120": 120,
            "padding": 12,
            "foreground": "ebebeb",
            "background": "0e1216",
        },
        "cases": [
            {
                "id": "case",
                "columns": 2,
                "rows": 1,
                "text": "ab",
                "style": "normal",
                "cursor": {"visible": False},
            }
        ],
    }


def write_manifest(tmp_path, value):
    path = tmp_path / "fixtures.json"
    path.write_text(json.dumps(value), encoding="utf-8")
    return path


def test_checked_in_manifest_is_valid_and_complete():
    loaded = MODULE.load_manifest(MODULE.DEFAULT_MANIFEST)
    ids = {case["id"] for case in loaded["cases"]}
    assert {
        "ascii",
        "spacing-punctuation",
        "narrow-iiii",
        "wide-WWWW",
        "dots",
        "underscores",
        "bars",
        "drift-80",
        "drift-240",
        "edge-cells",
        "reverse",
        "dim",
        "conceal",
        "cursor-block",
        "cursor-beam",
        "cursor-underline",
    } <= ids


def test_manifest_rejects_duplicate_ids_and_bad_cursor(tmp_path):
    value = manifest()
    value["cases"].append(dict(value["cases"][0]))
    with pytest.raises(ValueError, match="duplicate"):
        MODULE.load_manifest(write_manifest(tmp_path, value))

    value = manifest()
    value["cases"][0]["cursor"] = {
        "visible": True,
        "column": 2,
        "row": 0,
        "shape": "block",
    }
    with pytest.raises(ValueError, match="cursor"):
        MODULE.load_manifest(write_manifest(tmp_path, value))


def test_workspace_safety_requires_inactive_workspace_8_on_dp2(monkeypatch):
    responses = {
        "monitors all": [
            {
                "id": 1,
                "name": "DP-2",
                "disabled": False,
                "activeWorkspace": {"id": 8},
            }
        ],
        "workspaces": [{"id": 8, "monitor": "DP-2"}],
        "activeworkspace": {"id": 1, "monitor": "DP-1"},
    }
    monkeypatch.setattr(MODULE, "hyprland_json", responses.__getitem__)
    monkeypatch.setattr(MODULE, "workspace_clients", lambda _workspace: [])
    assert MODULE.assert_test_workspace_isolated() == 1

    responses["activeworkspace"] = {"id": 8, "monitor": "DP-2"}
    with pytest.raises(RuntimeError, match="refusing"):
        MODULE.assert_test_workspace_isolated()


def test_failed_comparator_cannot_reuse_stale_success(tmp_path):
    stale = tmp_path / "comparison.json"
    stale.write_text(json.dumps({"exact": True}), encoding="utf-8")
    failed = subprocess.CompletedProcess([], 2, stdout="", stderr="parser failed")
    with pytest.raises(RuntimeError, match="parser failed"):
        MODULE.read_comparison_result(failed, stale)


def test_foot_payload_encodes_style_and_cursor():
    case = manifest()["cases"][0]
    case["style"] = "reverse"
    case["cursor"] = {
        "visible": True,
        "column": 1,
        "row": 0,
        "shape": "beam",
    }
    payload = MODULE.foot_payload(case)
    assert payload.startswith(b"\x1b[?25l\x1b[7mab")
    assert payload.endswith(b"\x1b[1;2H\x1b[6 q\x1b[?25h")
