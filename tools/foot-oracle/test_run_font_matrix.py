#!/usr/bin/env python3
"""Portable tests for the Slice 4 headless matrix orchestrator."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path

MODULE_PATH = Path(__file__).with_name("run-font-matrix.py")
SPEC = importlib.util.spec_from_file_location("run_font_matrix", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
MATRIX = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MATRIX
SPEC.loader.exec_module(MATRIX)


def test_declared_matrix_is_complete_unique_and_dependency_ordered() -> None:
    cases = MATRIX.matrix_cases()
    assert len(cases) == 4 * 6 * 4 == 96
    assert len({case.identifier for case in cases}) == len(cases)
    assert {case.style for case in cases} == set(MATRIX.STYLES)
    assert {case.logical_size for case in cases} == set(MATRIX.LOGICAL_SIZES)
    assert {case.scale_120 for case in cases} == set(MATRIX.SCALES_120)
    assert cases[0].identifier == "regular-6px-120"
    assert cases[-1].identifier == "bold-italic-96px-240"


def test_effective_sizes_preserve_fractional_scale_boundaries() -> None:
    cases = {case.identifier: case for case in MATRIX.matrix_cases()}
    assert cases["regular-6px-150"].effective_size == 7.5
    assert cases["italic-22px-180"].effective_size == 33.0
    assert cases["bold-italic-96px-240"].effective_size == 192.0


def test_pinned_host_preflight_invokes_strict_provenance(monkeypatch) -> None:
    calls = []

    def fake_run(command, **_kwargs):
        calls.append(command)
        return subprocess.CompletedProcess(command, 0, "", "")

    monkeypatch.setattr(MATRIX, "run", fake_run)
    MATRIX.require_pinned_host()
    assert calls == [[sys.executable, MATRIX.TOOLS / "check-provenance.py"]]


def test_failed_preflight_stops_before_creating_output(
    monkeypatch, capsys, tmp_path: Path
) -> None:
    output = tmp_path / "matrix"

    def failed_run(command, **_kwargs):
        return subprocess.CompletedProcess(
            command,
            1,
            "",
            "provenance error: resolved regular face drifted; install fixture",
        )

    monkeypatch.setattr(MATRIX, "run", failed_run)
    monkeypatch.setattr(sys, "argv", [str(MODULE_PATH), str(output)])
    assert MATRIX.main() == 1
    assert not output.exists()
    captured = capsys.readouterr()
    assert "font matrix prerequisites unavailable" in captured.err
    assert "resolved regular face drifted" in captured.err


def test_progress_summary_is_truthful(tmp_path: Path) -> None:
    case = {
        "id": "regular-6px-120",
        "exact": True,
    }
    MATRIX.write_summary(tmp_path, [case], "bounded failure")
    summary = __import__("json").loads((tmp_path / "summary.json").read_text())
    assert summary["declared_case_count"] == 96
    assert summary["completed_case_count"] == 1
    assert summary["exact"] is False
    assert summary["error"] == "bounded failure"
