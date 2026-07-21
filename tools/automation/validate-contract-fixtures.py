#!/usr/bin/env python3
"""Validate the checked-in Phase 4 public-contract schemas and golden fixtures."""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any

from jsonschema import Draft202012Validator
from jsonschema.exceptions import SchemaError

ROOT = Path(__file__).resolve().parents[2]
SCHEMA_DIR = ROOT / "dist" / "schemas" / "v1"
FIXTURE_DIR = ROOT / "tests" / "automation" / "fixtures"
MAX_JSON_BYTES = 1024 * 1024
EXPECTED_SCHEMAS = {
    "audit-record.schema.json",
    "cli-envelope.schema.json",
    "cli-event.schema.json",
    "policy.schema.json",
}


class ContractError(ValueError):
    """A checked-in contract artifact is malformed or has the wrong expectation."""


def load_json(path: Path) -> Any:
    size = path.stat().st_size
    if size == 0 or size > MAX_JSON_BYTES:
        raise ContractError(f"{path}: JSON size {size} is outside 1..{MAX_JSON_BYTES}")
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ContractError(f"{path}: cannot read JSON: {error}") from error


def load_validators() -> dict[str, Draft202012Validator]:
    actual = {path.name for path in SCHEMA_DIR.glob("*.schema.json")}
    if actual != EXPECTED_SCHEMAS:
        missing = sorted(EXPECTED_SCHEMAS - actual)
        extra = sorted(actual - EXPECTED_SCHEMAS)
        raise ContractError(f"schema inventory mismatch; missing={missing}, extra={extra}")

    validators = {}
    for name in sorted(actual):
        schema = load_json(SCHEMA_DIR / name)
        try:
            Draft202012Validator.check_schema(schema)
        except SchemaError as error:
            raise ContractError(f"{name}: invalid Draft 2020-12 schema: {error.message}") from error
        validators[name] = Draft202012Validator(schema)
    return validators


def validate_fixture(
    path: Path,
    validators: dict[str, Draft202012Validator],
    *,
    should_pass: bool,
) -> None:
    fixture = load_json(path)
    if not isinstance(fixture, dict) or set(fixture) != {"$schema_file", "document"}:
        raise ContractError(f"{path}: fixture must contain only $schema_file and document")
    schema_file = fixture["$schema_file"]
    if schema_file not in validators:
        raise ContractError(f"{path}: unknown schema {schema_file!r}")

    errors = sorted(
        validators[schema_file].iter_errors(fixture["document"]),
        key=lambda error: tuple(str(part) for part in error.absolute_path),
    )
    if should_pass and errors:
        raise ContractError(f"{path}: expected valid document: {errors[0].message}")
    if not should_pass and not errors:
        raise ContractError(f"{path}: invalid fixture unexpectedly passed")


def main() -> int:
    try:
        validators = load_validators()
        valid = sorted((FIXTURE_DIR / "valid").glob("*.json"))
        invalid = sorted((FIXTURE_DIR / "invalid").glob("*.json"))
        if not valid or not invalid:
            raise ContractError("both valid and invalid fixture sets must be non-empty")
        for path in valid:
            validate_fixture(path, validators, should_pass=True)
        for path in invalid:
            validate_fixture(path, validators, should_pass=False)
    except (ContractError, OSError) as error:
        print(f"contract validation failed: {error}", file=sys.stderr)
        return 1

    print(
        f"Validated {len(valid)} valid and {len(invalid)} invalid automation fixtures "
        f"against {len(validators)} schemas."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
