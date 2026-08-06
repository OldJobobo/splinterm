from __future__ import annotations

import importlib.util
import json
import pathlib
import subprocess
import sys

import jsonschema
import pytest

ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/performance/summarize-pane-reducer.py"
SCHEMA = ROOT / "tools/performance/pane-reducer-schema.json"


def load_module():
    spec = importlib.util.spec_from_file_location("pane_reducer_summary", SCRIPT)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


SUMMARY = load_module()


def smoke_report() -> dict[str, object]:
    return {
        "schema": "splinterm.performance.pane-reducer.v1",
        "clock": "std::time::Instant monotonic process clock",
        "build_profile": "release",
        "warmup_runs": 0,
        "sample_runs": 1,
        "history_capacity_rows": 4096,
        "smoke": True,
        "focused_role_scope": SUMMARY.FOCUSED_SCOPE,
        "cases": [
            {
                "name": name,
                "mode": fields[0],
                "history_rows": fields[1],
                "viewport": fields[2],
                "batch_size": fields[3],
                "duration_ns": [index + 1],
            }
            for index, (name, fields) in enumerate(
                sorted(SUMMARY.expected_cases(True).items())
            )
        ],
    }


def test_smoke_report_is_strict_and_summarizable() -> None:
    report = smoke_report()
    schema = json.loads(SCHEMA.read_text(encoding="utf-8"))
    jsonschema.Draft202012Validator.check_schema(schema)
    jsonschema.Draft202012Validator(schema).validate(report)
    SUMMARY.validate_semantics(report)
    summary = SUMMARY.summarize(report)
    assert summary["schema"] == "splinterm.performance.pane-reducer-summary.v1"
    assert len(summary["cases"]) == 8
    assert summary["focused_role_scope"] == SUMMARY.FOCUSED_SCOPE


def test_report_rejects_scope_case_and_sample_drift() -> None:
    report = smoke_report()
    report["focused_role_scope"] = "full active path"
    with pytest.raises(ValueError, match="disclaimer"):
        SUMMARY.validate_semantics(report)
    report = smoke_report()
    report["cases"][0]["batch_size"] = 64
    with pytest.raises(ValueError, match="metadata mismatch"):
        SUMMARY.validate_semantics(report)
    report = smoke_report()
    report["cases"][0]["duration_ns"].append(2)
    with pytest.raises(ValueError, match="sample count"):
        SUMMARY.validate_semantics(report)


def test_cli_writes_summary(tmp_path: pathlib.Path) -> None:
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
