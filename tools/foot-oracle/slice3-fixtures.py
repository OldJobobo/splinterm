#!/usr/bin/env python3
"""Dual payload builders for the validated Slice 3 manifest."""

from __future__ import annotations

import importlib.util
import json
from pathlib import Path
from typing import Any

ROOT = Path(__file__).parent
SPEC = importlib.util.spec_from_file_location("slice3_validator", ROOT / "validate-slice3-fixtures.py")
assert SPEC and SPEC.loader
VALIDATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VALIDATOR)


def load_manifest(path: Path = ROOT / "slice3-final-buffer-fixtures.json") -> dict[str, Any]:
    return VALIDATOR.validate_manifest(json.loads(path.read_text()))


def foot_payload(case: dict[str, Any]) -> bytes:
    payload = bytearray(b"\x1b[?25l")
    payload.extend(bytes.fromhex(case["vt_hex"]))
    column, row = case["cursor"]
    payload.extend(f"\x1b[{row + 1};{column + 1}H".encode())
    if case["cursor_visible"]:
        shape = {"block": 2, "underline": 4, "beam": 6}[case["configured_cursor"]]
        payload.extend(f"\x1b[{shape} q\x1b[?25h".encode())
    return bytes(payload)


def _color(cell: dict[str, Any], key: str) -> tuple[str, int]:
    color = cell.get(key, {"source": "default", "value": 0})
    return color["source"], color["value"]


def splinterm_cells(case: dict[str, Any]) -> dict[str, Any]:
    rows = []
    for source_row in case["cells"]:
        row = []
        for cell in source_row:
            if "spacer_remaining" in cell:
                row.append({
                    "content": "",
                    "spacer_remaining": cell["spacer_remaining"],
                    "attributes": _attributes({}),
                })
                continue
            attributes = _attributes(cell)
            row.append({
                "content": cell["content"],
                "spacer_remaining": None,
                "attributes": attributes,
            })
        rows.append(row)
    return {"rows": rows}


def _attributes(cell: dict[str, Any]) -> dict[str, Any]:
    underline_source, underline_value = _color(cell, "underline_color")
    foreground_source, foreground = _color(cell, "foreground")
    background_source, background = _color(cell, "background")
    return {
        "bold": cell.get("bold", False),
        "dim": cell.get("dim", False),
        "italic": cell.get("italic", False),
        "underline": cell.get("underline", "none"),
        "underline_color_source": underline_source,
        "underline_color": underline_value,
        "strikethrough": cell.get("strikethrough", False),
        "blink": False,
        "conceal": cell.get("conceal", False),
        "reverse": cell.get("reverse", False),
        "foreground_source": foreground_source,
        "foreground": foreground,
        "background_source": background_source,
        "background": background,
    }
