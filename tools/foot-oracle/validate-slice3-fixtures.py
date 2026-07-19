#!/usr/bin/env python3
"""Validate the bounded, structured Slice 3 decoration/cursor manifest."""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
from typing import Any

SCHEMA = "splinterm.final-buffer.slice3-fixtures.v2"
ID = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")
UNDERLINES = {"none", "single", "double", "curly", "dotted", "dashed"}
SHAPES = {"block", "beam", "underline"}
SCALES = {120, 150, 180, 240}
LANES = {"focused-steady", "unfocused"}
COLOR_SOURCES = {"default", "base16", "base256", "rgb"}


def _fail(message: str) -> None:
    raise ValueError(message)


def validate_cell(cell: Any, row: list[dict[str, Any]], index: int) -> None:
    if not isinstance(cell, dict):
        _fail("cell must be an object")
    allowed = {
        "content", "spacer_remaining", "wide", "bold", "dim", "italic",
        "underline", "underline_color", "strikethrough", "conceal", "reverse",
        "foreground", "background",
    }
    unknown = set(cell) - allowed
    if unknown:
        _fail(f"unknown cell keys: {sorted(unknown)}")
    spacer = cell.get("spacer_remaining")
    if spacer is not None:
        if set(cell) != {"spacer_remaining"} or not isinstance(spacer, int) or spacer <= 0:
            _fail("wide spacer must contain only a positive spacer_remaining")
        if index == 0 or row[index - 1].get("wide") != spacer + 1:
            _fail("wide spacer must follow its declared leader")
        return
    content = cell.get("content")
    if not isinstance(content, str) or not content or len(content.encode()) > 64:
        _fail("leader content must be bounded non-empty UTF-8")
    wide = cell.get("wide", 1)
    if not isinstance(wide, int) or wide not in (1, 2):
        _fail("wide must be 1 or 2")
    if wide == 2 and (index + 1 >= len(row) or row[index + 1].get("spacer_remaining") != 1):
        _fail("wide=2 leader requires one following spacer")
    if cell.get("underline", "none") not in UNDERLINES:
        _fail("unknown underline style")
    for flag in ("bold", "dim", "italic", "strikethrough", "conceal", "reverse"):
        if flag in cell and not isinstance(cell[flag], bool):
            _fail(f"{flag} must be boolean")
    for key in ("underline_color", "foreground", "background"):
        if key not in cell:
            continue
        color = cell[key]
        if not isinstance(color, dict) or set(color) != {"source", "value"}:
            _fail(f"{key} must contain source and value")
        if color["source"] not in COLOR_SOURCES or not isinstance(color["value"], int):
            _fail(f"invalid {key}")
        maximum = 255 if color["source"] in {"base16", "base256"} else 0xFFFFFF
        if not 0 <= color["value"] <= maximum:
            _fail(f"{key} value is out of range")


def validate_manifest(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != {"schema", "profile", "cases"}:
        _fail("manifest keys are invalid")
    if value["schema"] != SCHEMA:
        _fail("unsupported Slice 3 schema")
    profile = value["profile"]
    required_profile = {
        "font", "font_size", "font_size_unit", "scales_120", "padding",
        "foreground", "background", "cursor_text", "cursor_background",
        "composition_model",
    }
    if (
        not isinstance(profile, dict)
        or set(profile) != required_profile
        or profile.get("composition_model") != "foot-cell-rtl-v1"
    ):
        _fail("profile composition model or keys are invalid")
    if set(profile.get("scales_120", [])) != SCALES:
        _fail("profile must declare exactly the Slice 3 scales")
    for key in ("foreground", "background", "cursor_text", "cursor_background"):
        if not isinstance(profile.get(key), str) or not re.fullmatch(r"[0-9a-f]{6}", profile[key]):
            _fail(f"profile {key} must be lowercase RRGGBB")
    cases = value["cases"]
    if not isinstance(cases, list) or not 1 <= len(cases) <= 64:
        _fail("cases must be a bounded non-empty list")
    seen: set[str] = set()
    for case in cases:
        required = {"id", "scale_120", "lane", "unfocused_style", "configured_cursor", "cursor_visible", "cursor", "vt_hex", "cells"}
        if not isinstance(case, dict) or set(case) != required:
            _fail("case keys are invalid")
        case_id = case["id"]
        if not isinstance(case_id, str) or not ID.fullmatch(case_id) or case_id in seen:
            _fail("case id is invalid or duplicated")
        seen.add(case_id)
        if case["scale_120"] not in SCALES or case["lane"] not in LANES:
            _fail("case scale or lane is invalid")
        expected_unfocused = "unchanged" if case["lane"] == "focused-steady" else "hollow"
        if case["unfocused_style"] != expected_unfocused:
            _fail("lane and unfocused_style are inconsistent")
        if case["configured_cursor"] not in SHAPES or not isinstance(case["cursor_visible"], bool):
            _fail("configured cursor is invalid")
        cursor = case["cursor"]
        if not isinstance(cursor, list) or len(cursor) != 2 or not all(isinstance(v, int) and v >= 0 for v in cursor):
            _fail("cursor position is invalid")
        vt_hex = case["vt_hex"]
        if not isinstance(vt_hex, str) or len(vt_hex) > 8192 or len(vt_hex) % 2 or not re.fullmatch(r"[0-9a-f]+", vt_hex):
            _fail("vt_hex must be bounded lowercase hexadecimal")
        vt = bytes.fromhex(vt_hex)
        if b"\x1b[?25" in vt or re.search(rb"\x1b\[[0-9]+ q", vt):
            _fail("vt_hex body must not override runner-owned cursor visibility or shape")
        rows = case["cells"]
        if not isinstance(rows, list) or not 1 <= len(rows) <= 80:
            _fail("cell rows are invalid")
        columns = len(rows[0]) if isinstance(rows[0], list) else 0
        if not 1 <= columns <= 240 or any(not isinstance(row, list) or len(row) != columns for row in rows):
            _fail("cell grid must be rectangular and protocol bounded")
        if cursor[0] >= columns or cursor[1] >= len(rows):
            _fail("cursor is outside the cell grid")
        for row in rows:
            for index, cell in enumerate(row):
                validate_cell(cell, row, index)
        if case["lane"] == "unfocused" and not case["cursor_visible"]:
            _fail("unfocused lane must attest a visible effective hollow cursor")
    return value


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("manifest", type=Path, nargs="?", default=Path(__file__).with_name("slice3-final-buffer-fixtures.json"))
    args = parser.parse_args()
    if args.manifest.stat().st_size > 1024 * 1024:
        parser.error("manifest exceeds 1 MiB")
    try:
        value = validate_manifest(json.loads(args.manifest.read_text()))
    except (OSError, json.JSONDecodeError, ValueError) as error:
        parser.error(str(error))
    print(f"Validated {len(value['cases'])} Slice 3 fixtures")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
