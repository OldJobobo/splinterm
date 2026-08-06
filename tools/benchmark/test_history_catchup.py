from __future__ import annotations

import importlib.util
import json
import pathlib
import subprocess
import sys

import jsonschema
import pytest

ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/performance/summarize-history-catchup.py"
COMPARE_SCRIPT = ROOT / "tools/performance/compare-history-catchup.py"
SCHEMA = ROOT / "tools/performance/history-catchup-schema.json"


def load_module():
    spec = importlib.util.spec_from_file_location("history_catchup_summary", SCRIPT)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


SUMMARY = load_module()


def load_compare_module():
    spec = importlib.util.spec_from_file_location(
        "history_catchup_compare", COMPARE_SCRIPT
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


COMPARE = load_compare_module()


def smoke_report() -> dict[str, object]:
    cases = []
    for index, name in enumerate(sorted(SUMMARY.SMOKE_CASES)):
        history, viewport, panes, activity, shape, updates = SUMMARY.CASE_FIELDS[name]
        cases.append(
            {
                "name": name,
                "history_rows": history,
                "viewport": viewport,
                "pane_count": panes,
                "activity": activity,
                "update_shape": shape,
                "operation_updates": updates,
                "duration_ns": [index + 1],
            }
        )
    return {
        "schema": "splinterm.performance.history-catchup.v1",
        "clock": "std::time::Instant monotonic process clock",
        "build_profile": "release",
        "warmup_runs": 0,
        "sample_runs": 1,
        "history_capacity_rows": 4096,
        "ansi_operation_lines": 2000,
        "smoke": True,
        "cases": cases,
    }


def test_smoke_contract_is_schema_valid_and_semantically_exact() -> None:
    report = smoke_report()
    schema = json.loads(SCHEMA.read_text(encoding="utf-8"))
    jsonschema.Draft202012Validator.check_schema(schema)
    jsonschema.Draft202012Validator(schema).validate(report)
    SUMMARY.validate_semantics(report)
    summary = SUMMARY.summarize(report)
    assert summary["schema"] == "splinterm.performance.history-catchup-summary.v1"
    assert len(summary["cases"]) == 8


def test_semantics_reject_missing_cases_metadata_and_sample_drift() -> None:
    report = smoke_report()
    report["cases"].pop()
    with pytest.raises(ValueError, match="case set"):
        SUMMARY.validate_semantics(report)

    report = smoke_report()
    report["cases"][0]["pane_count"] = 4
    with pytest.raises(ValueError, match="metadata mismatch"):
        SUMMARY.validate_semantics(report)

    report = smoke_report()
    report["cases"][0]["duration_ns"].append(2)
    with pytest.raises(ValueError, match="sample count"):
        SUMMARY.validate_semantics(report)


def full_report(duration_ns: int) -> dict[str, object]:
    report = smoke_report()
    report["smoke"] = False
    report["warmup_runs"] = 5
    report["sample_runs"] = 30
    report["cases"] = [
        {
            "name": name,
            "history_rows": fields[0],
            "viewport": fields[1],
            "pane_count": fields[2],
            "activity": fields[3],
            "update_shape": fields[4],
            "operation_updates": fields[5],
            "duration_ns": [duration_ns] * 30,
        }
        for name, fields in SUMMARY.CASE_FIELDS.items()
    ]
    return report


def test_bootstrap_comparison_is_deterministic_and_preserves_case_identity() -> None:
    COMPARE.BOOTSTRAP_RESAMPLES = 1_000
    comparison = COMPARE.compare(full_report(100), full_report(50))
    assert comparison["bootstrap_seed"] == 220022
    assert len(comparison["cases"]) == 14
    assert all(case["candidate_control_ratio"] == 0.5 for case in comparison["cases"])
    assert all(
        case["candidate_control_ratio_one_sided_95_upper"] == 0.5
        for case in comparison["cases"]
    )


def test_nearest_rank_p95_and_cli_output(tmp_path: pathlib.Path) -> None:
    values = list(range(1, 31))
    assert SUMMARY.duration_summary(values) == {
        "count": 30,
        "min_ns": 1,
        "median_ns": 15.5,
        "p95_ns": 29,
        "max_ns": 30,
    }
    report = tmp_path / "report.json"
    output = tmp_path / "summary.json"
    report.write_text(json.dumps(smoke_report()), encoding="utf-8")
    result = subprocess.run(
        [sys.executable, str(SCRIPT), str(report), str(output)],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert json.loads(output.read_text(encoding="utf-8"))["sample_runs"] == 1
