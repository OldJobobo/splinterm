#!/usr/bin/env python3
"""Build or validate Plan 0022 artifacts without starting graphical work."""

from __future__ import annotations

import argparse
import json
import pathlib
import sys
from typing import Any

import jsonschema
from graphical_catchup import (
    DEFAULT_SAMPLES,
    DEFAULT_WARMUPS,
    plan_document,
    validate_plan_semantics,
    validate_report_against_plan,
    validate_report_semantics,
)

TOOLS = pathlib.Path(__file__).resolve().parent
PLAN_SCHEMA = TOOLS / "graphical-catchup-plan-schema.json"
REPORT_SCHEMA = TOOLS / "graphical-catchup-report-schema.json"


def load_json(path: pathlib.Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise TypeError(f"{path} does not contain a JSON object")
    return value


def atomic_json(path: pathlib.Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.tmp")
    temporary.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    temporary.replace(path)


def validate(schema_path: pathlib.Path, value: dict[str, Any]) -> None:
    jsonschema.Draft202012Validator(load_json(schema_path)).validate(value)


def main() -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Build the finite graphical catch-up plan or validate a retained report. "
            "This command never launches a window or performs graphical input."
        )
    )
    parser.add_argument("--output", type=pathlib.Path)
    parser.add_argument("--seed", type=int, default=220022)
    parser.add_argument("--warmups", type=int, default=DEFAULT_WARMUPS)
    parser.add_argument("--samples", type=int, default=DEFAULT_SAMPLES)
    parser.add_argument("--validate-report", type=pathlib.Path)
    parser.add_argument("--plan", type=pathlib.Path)
    args = parser.parse_args()

    try:
        if args.validate_report is not None:
            if args.output is not None:
                parser.error("--output and --validate-report are mutually exclusive")
            if args.plan is None:
                parser.error("--plan is required with --validate-report")
            report = load_json(args.validate_report)
            plan = load_json(args.plan)
            validate(REPORT_SCHEMA, report)
            validate(PLAN_SCHEMA, plan)
            validate_report_against_plan(report, plan)
            validate_report_semantics(report)
            print(f"valid graphical catch-up report: {args.validate_report}")
            return 0
        if args.plan is not None:
            parser.error("--plan is only valid with --validate-report")
        if args.output is None:
            parser.error("--output is required when building a plan")
        plan = plan_document(args.seed, args.warmups, args.samples)
        validate(PLAN_SCHEMA, plan)
        validate_plan_semantics(plan)
        atomic_json(args.output, plan)
        print(
            f"wrote {len(plan['schedule'])}-case non-graphical plan to {args.output} "
            f"(sha256 {plan['plan_sha256']})"
        )
        return 0
    except (
        OSError,
        TypeError,
        ValueError,
        json.JSONDecodeError,
        jsonschema.ValidationError,
    ) as error:
        print(f"graphical catch-up artifact error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
