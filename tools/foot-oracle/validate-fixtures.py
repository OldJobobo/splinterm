#!/usr/bin/env python3
"""Validate the language-neutral Foot semantic fixture corpus."""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
FIXTURE_DIR = ROOT / "fixtures" / "terminal" / "v1"
EXPECTED_COMMIT = "3c5b584b0eafa772eb4376fb6eaf6643399e190e"
VALID_STATUSES = {"source_reviewed", "oracle_verified", "intentional_divergence"}
ID_PATTERN = re.compile(r"[a-z0-9]+(?:-[a-z0-9]+)*")
HEX_PATTERN = re.compile(r"(?:[0-9a-f]{2})*")


def require(condition: bool, path: Path, message: str) -> None:
    if not condition:
        raise ValueError(f"{path.relative_to(ROOT)}: {message}")


def object_at(value: Any, path: Path, field: str) -> dict[str, Any]:
    require(isinstance(value, dict), path, f"{field} must be an object")
    return value


def integer_at(value: Any, path: Path, field: str) -> int:
    require(isinstance(value, int) and not isinstance(value, bool), path, f"{field} must be an integer")
    return value


def validate_fixture(path: Path) -> None:
    data = object_at(json.loads(path.read_text(encoding="utf-8")), path, "document")

    require(data.get("schema") == 1, path, "schema must be 1")
    fixture_id = data.get("id")
    require(isinstance(fixture_id, str) and ID_PATTERN.fullmatch(fixture_id) is not None, path, "id must be kebab-case")
    require(path.stem == fixture_id, path, "filename must match id")
    require(isinstance(data.get("description"), str) and data["description"], path, "description is required")

    reference = object_at(data.get("reference"), path, "reference")
    require(reference.get("name") == "foot", path, "reference.name must be foot")
    require(reference.get("version") == "1.27.0", path, "reference.version must be 1.27.0")
    require(reference.get("commit") == EXPECTED_COMMIT, path, "reference commit does not match the pinned oracle")
    require(reference.get("verification") in VALID_STATUSES, path, "invalid verification status")

    initial = object_at(data.get("initial"), path, "initial")
    columns = integer_at(initial.get("columns"), path, "initial.columns")
    rows = integer_at(initial.get("rows"), path, "initial.rows")
    require(columns > 0 and rows > 0, path, "terminal dimensions must be positive")

    input_hex = data.get("input_hex")
    require(isinstance(input_hex, str) and HEX_PATTERN.fullmatch(input_hex) is not None, path, "input_hex must be lowercase byte pairs")
    bytes.fromhex(input_hex)

    expected = object_at(data.get("expected"), path, "expected")
    cursor = object_at(expected.get("cursor"), path, "expected.cursor")
    cursor_column = integer_at(cursor.get("column"), path, "expected.cursor.column")
    cursor_row = integer_at(cursor.get("row"), path, "expected.cursor.row")
    require(0 <= cursor_column < columns, path, "cursor column is out of bounds")
    require(0 <= cursor_row < rows, path, "cursor row is out of bounds")
    require(isinstance(cursor.get("last_column_flag"), bool), path, "cursor last_column_flag must be boolean")

    expected_rows = expected.get("rows")
    require(isinstance(expected_rows, list) and len(expected_rows) == rows, path, "expected.rows must cover every visible row")
    for index, row_value in enumerate(expected_rows):
        row = object_at(row_value, path, f"expected.rows[{index}]")
        text = row.get("text")
        require(isinstance(text, str) and len(text) == columns, path, f"row {index} text must contain exactly {columns} characters")
        require(isinstance(row.get("linebreak"), bool), path, f"row {index} linebreak must be boolean")

    attribute_runs = expected.get("attribute_runs")
    require(isinstance(attribute_runs, list), path, "expected.attribute_runs must be an array")
    for index, run_value in enumerate(attribute_runs):
        run = object_at(run_value, path, f"expected.attribute_runs[{index}]")
        row = integer_at(run.get("row"), path, f"attribute run {index} row")
        start = integer_at(run.get("start"), path, f"attribute run {index} start")
        end = integer_at(run.get("end"), path, f"attribute run {index} end")
        require(0 <= row < rows, path, f"attribute run {index} row is out of bounds")
        require(0 <= start < end <= columns, path, f"attribute run {index} range is invalid")
        attributes = object_at(run.get("attributes"), path, f"attribute run {index} attributes")
        require(bool(attributes), path, f"attribute run {index} must contain non-default attributes")

    require(isinstance(expected.get("events"), list), path, "expected.events must be an array")
    require(isinstance(data.get("intentional_divergences"), list), path, "intentional_divergences must be an array")


def main() -> int:
    fixtures = sorted(FIXTURE_DIR.glob("*.json"))
    if not fixtures:
        print(f"No fixtures found in {FIXTURE_DIR}", file=sys.stderr)
        return 1

    try:
        for fixture in fixtures:
            validate_fixture(fixture)
    except (ValueError, json.JSONDecodeError) as error:
        print(error, file=sys.stderr)
        return 1

    print(f"Validated {len(fixtures)} Foot semantic fixtures.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
