from __future__ import annotations

import importlib.util
import json
import pathlib
import subprocess
import sys
from dataclasses import asdict

import graphical_catchup as CATCHUP
import jsonschema
import pytest

ROOT = pathlib.Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools/benchmark"
PLAN_SCHEMA = TOOLS / "graphical-catchup-plan-schema.json"
REPORT_SCHEMA = TOOLS / "graphical-catchup-report-schema.json"
RUNNER = TOOLS / "run-graphical-catchup.py"
CASE_RUNNER = TOOLS / "run-graphical-catchup-case.py"
BENCH_CHILD = TOOLS / "workloads/bench-child.py"


def load_case_runner():
    spec = importlib.util.spec_from_file_location("graphical_catchup_case", CASE_RUNNER)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


CASE = load_case_runner()


def load_bench_child():
    spec = importlib.util.spec_from_file_location(
        "graphical_catchup_child", BENCH_CHILD
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


CHILD = load_bench_child()


def validator(path: pathlib.Path) -> jsonschema.Draft202012Validator:
    return jsonschema.Draft202012Validator(json.loads(path.read_text(encoding="utf-8")))


def valid_report(cell: CATCHUP.CatchupCell) -> dict[str, object]:
    activity = list(CATCHUP.activity_panes(cell))
    return {
        "schema": CATCHUP.REPORT_SCHEMA,
        "case_id": f"measured-00-0000-{cell.name}",
        "plan_sha256": "a" * 64,
        "phase": "measured",
        "iteration": 0,
        "execution_index": 0,
        "cell": asdict(cell),
        "precondition": {
            "panes": [
                {
                    "pane_index": index,
                    "target_cached_rows": cell.cached_rows_per_pane,
                    "actual_cached_rows": cell.cached_rows_per_pane,
                    "cached_bytes": min(
                        cell.cached_rows_per_pane * 80, 16 * 1024 * 1024
                    ),
                    "verified": True,
                }
                for index in range(cell.pane_count)
            ],
            "viewport": {
                "initial": "live",
                "steps": list(CATCHUP.viewport_steps(cell)),
                "final": cell.viewport,
                "proven": True,
            },
            "activity": {
                "mode": cell.activity,
                "target_panes": activity,
                "observed_panes": activity,
                "proven": True,
            },
        },
        "operation_evidence": {
            "unit": CATCHUP.operation_spec(cell)[0],
            "requested_units": CATCHUP.operation_spec(cell)[1],
            "completed_units": CATCHUP.operation_spec(cell)[1],
            "completed": True,
            "panes": [
                {
                    "pane_index": index,
                    "event": {
                        "small-marker": "marker_complete",
                        "continuing-small-output": "output_complete",
                        "same-output-position-movement": "movement_complete",
                        "twelve-step-outer-resize": "resize_complete",
                        "ansi-2000-lines": "output_complete",
                    }[cell.operation],
                    "workload": {
                        "small-marker": "marker",
                        "continuing-small-output": "plain",
                        "same-output-position-movement": "movement",
                        "twelve-step-outer-resize": "resize",
                        "ansi-2000-lines": "ansi",
                    }[cell.operation],
                    "completed_units": CATCHUP.operation_spec(cell)[1],
                    "payload_bytes": (
                        80
                        if cell.operation
                        in {"continuing-small-output", "ansi-2000-lines"}
                        else 0
                    ),
                    "marker_bytes": (
                        80
                        if cell.operation
                        in {
                            "small-marker",
                            "twelve-step-outer-resize",
                            "ansi-2000-lines",
                        }
                        else 0
                    ),
                    "control_bytes": (10 if cell.operation == "ansi-2000-lines" else 0),
                    "raw_result": (
                        None
                        if cell.operation == "same-output-position-movement"
                        else {
                            "schema": "splinterm.benchmark.child-result.v1",
                            "event": (
                                "write_complete"
                                if cell.operation == "ansi-2000-lines"
                                else "output_complete"
                                if cell.operation == "continuing-small-output"
                                else "marker_complete"
                            ),
                            "sequence": index,
                            "pid": 100 + index,
                            "received_monotonic_ns": 105,
                            "completed_monotonic_ns": 125,
                            "workload": (
                                "ansi"
                                if cell.operation == "ansi-2000-lines"
                                else "plain"
                                if cell.operation == "continuing-small-output"
                                else None
                            ),
                            "lines": (
                                2000
                                if cell.operation == "ansi-2000-lines"
                                else 1
                                if cell.operation == "continuing-small-output"
                                else 0
                            ),
                            "completed_units": (
                                CATCHUP.operation_spec(cell)[1]
                                if cell.operation
                                in {"ansi-2000-lines", "continuing-small-output"}
                                else 1
                            ),
                            "payload_bytes": (
                                80
                                if cell.operation
                                in {"ansi-2000-lines", "continuing-small-output"}
                                else 0
                            ),
                            "marker_bytes": (
                                80
                                if cell.operation
                                in {
                                    "small-marker",
                                    "twelve-step-outer-resize",
                                    "ansi-2000-lines",
                                }
                                else 0
                            ),
                            "control_bytes": (
                                10 if cell.operation == "ansi-2000-lines" else 0
                            ),
                            "total_bytes": (
                                170 if cell.operation == "ansi-2000-lines" else 80
                            ),
                        }
                    ),
                }
                for index in (activity or [0])
            ],
        },
        "trace": {
            "run_id": "catchup-test",
            "records": 12,
            "saturated": False,
            "ambiguous": False,
            "client_receive_to_pane_commit_ns": (
                None if cell.operation == "same-output-position-movement" else 20
            ),
            "commit_to_callback_ns": 10,
            "uncommitted_transactions": 0,
        },
        "capture": {
            "marker_trigger_ns": 100,
            "child_receipt_ns": 125,
            "attempts": [
                {
                    "sequence": 0,
                    "capture_start_ns": 130,
                    "capture_end_ns": 140,
                    "decode_scan_complete_ns": 150,
                    "marker_found": True,
                    "pane_marker_pixels": [
                        {"pane_index": index, "pixels": 100}
                        for index in range(cell.pane_count)
                    ],
                }
            ],
            "found_sequence": 0,
            "quantization_ns": 10,
        },
        "movement": {
            "position_changes": (
                1 if cell.operation == "same-output-position-movement" else 0
            ),
            "configure_events": 0,
            "output_enter_events": 0,
            "output_leave_events": 0,
            "semantic_applies": 0,
            "history_clones": 0,
            "pane_frame_rebuilds": 0,
        },
        "isolation": {
            "workspace": 8,
            "monitor": "DP-2",
            "target_owned": True,
            "placement_preserved": True,
            "focus_preserved": True,
            "unrelated_activity_untouched": True,
        },
        "cleanup": {
            "windows_absent": True,
            "processes_absent": True,
            "namespace_absent": True,
            "trace_closed": True,
            "verified": True,
            "failure": None,
        },
        "failure": None,
        "valid": True,
        "notes": [],
    }


def test_plan_is_finite_deterministic_and_complete() -> None:
    first = CATCHUP.plan_document(123)
    second = CATCHUP.plan_document(123)
    assert first == second
    assert len(first["cells"]) == 10
    assert len(first["schedule"]) == 130
    assert [item["execution_index"] for item in first["schedule"]] == list(range(130))
    validator(PLAN_SCHEMA).validate(first)
    CATCHUP.validate_plan_semantics(first)

    changed = dict(first)
    changed["schedule"] = list(first["schedule"][:-1])
    changed["plan_sha256"] = CATCHUP.canonical_sha256(
        {key: value for key, value in changed.items() if key != "plan_sha256"}
    )
    with pytest.raises(ValueError, match="complete matrix|schedule length"):
        CATCHUP.validate_plan_semantics(changed)


def test_case_profile_sets_comparable_width_and_plan_scrollback_bound() -> None:
    source = CASE.SPLINTERM_PROFILE.read_text(encoding="utf-8")
    profile = CASE.catchup_profile_text()
    assert "initial-columns=56" in profile
    assert "[scrollback]\nlines=4096" in profile
    assert profile == source.replace(
        "initial-columns=80", "initial-columns=56"
    ).replace("[scrollback]\nlines=1000", "[scrollback]\nlines=4096")


def test_compact_preload_rows_are_fixed_width_and_below_full_terminal_width() -> None:
    payload = CHILD.compact_history(4096)
    rows = payload.splitlines(keepends=True)
    assert len(rows) == 4096
    assert {len(row) for row in rows} == {2}
    assert len(payload) == 2 * 4096
    chunks = CHILD.compact_history_chunks(4096, 16)
    assert len(chunks) == 256
    assert {len(chunk) for chunk in chunks} == {32}
    assert b"".join(chunks) == payload
    ansi_payload = CHILD.ansi(2000, 80)
    ansi_chunks = CHILD.line_chunks(ansi_payload, 16)
    assert len(ansi_chunks) == 125
    assert b"".join(ansi_chunks) == ansi_payload


def test_preload_viewport_and_activity_contracts_cover_matrix() -> None:
    maximum = CATCHUP.CELL_BY_NAME["ansi-stress"]
    spec = CATCHUP.preload_spec(maximum, visible_rows=24)
    assert spec == {
        "target_cached_rows": 4096,
        "visible_rows": 24,
        "fixed_width_records": True,
        "emitted_rows_per_pane": 4119,
        "verification_required": True,
    }
    detached = CATCHUP.CELL_BY_NAME["detached-history"]
    assert CATCHUP.viewport_steps(detached) == (
        "assert-live",
        "scroll-back-one-page",
        "assert-detached",
    )
    assert CATCHUP.activity_panes(CATCHUP.CELL_BY_NAME["inactive-scaling-4"]) == (0,)
    assert CATCHUP.activity_panes(CATCHUP.CELL_BY_NAME["all-pane-scaling"]) == (
        0,
        1,
        2,
        3,
    )
    assert CATCHUP.activity_panes(CATCHUP.CELL_BY_NAME["static-movement"]) == ()


def test_report_proves_preconditions_and_capture_phase_order() -> None:
    plan = CATCHUP.plan_document(123, warmup_runs=0, sample_runs=1)
    entry = next(
        item for item in plan["schedule"] if item["cell"] == "detached-history"
    )
    report = valid_report(CATCHUP.CELL_BY_NAME["detached-history"])
    report.update(
        case_id=entry["case_id"],
        plan_sha256=plan["plan_sha256"],
        phase=entry["phase"],
        iteration=entry["iteration"],
        execution_index=entry["execution_index"],
    )
    validator(REPORT_SCHEMA).validate(report)
    CATCHUP.validate_report_against_plan(report, plan)
    CATCHUP.validate_report_semantics(report)

    report["precondition"]["panes"][0]["actual_cached_rows"] = 4095
    with pytest.raises(ValueError, match="actual cached rows"):
        CATCHUP.validate_report_semantics(report)
    report["precondition"]["panes"][0]["actual_cached_rows"] = 4096
    report["capture"]["attempts"][0]["capture_end_ns"] = 109
    with pytest.raises(ValueError, match="capture phases"):
        CATCHUP.validate_report_semantics(report)

    report = valid_report(CATCHUP.CELL_BY_NAME["detached-history"])
    report.update(
        case_id=entry["case_id"],
        plan_sha256="b" * 64,
        phase=entry["phase"],
        iteration=entry["iteration"],
        execution_index=entry["execution_index"],
    )
    with pytest.raises(ValueError, match="plan digest"):
        CATCHUP.validate_report_against_plan(report, plan)


def test_report_rejects_raw_byte_drift_and_capture_before_receipt() -> None:
    marker = valid_report(CATCHUP.CELL_BY_NAME["zero-history"])
    marker["operation_evidence"]["panes"][0]["raw_result"]["marker_bytes"] = 0
    marker["operation_evidence"]["panes"][0]["raw_result"]["total_bytes"] = 0
    marker["operation_evidence"]["panes"][0]["marker_bytes"] = 0
    with pytest.raises(ValueError, match="raw marker evidence"):
        CATCHUP.validate_report_semantics(marker)

    marker = valid_report(CATCHUP.CELL_BY_NAME["zero-history"])
    marker["capture"]["attempts"][0]["capture_start_ns"] = 124
    with pytest.raises(ValueError, match="capture phases"):
        CATCHUP.validate_report_semantics(marker)

    ansi = valid_report(CATCHUP.CELL_BY_NAME["ansi-stress"])
    ansi["operation_evidence"]["panes"][0]["raw_result"]["lines"] = 1999
    with pytest.raises(ValueError, match="raw ANSI"):
        CATCHUP.validate_report_semantics(ansi)


def test_report_rejects_unproven_activity_and_invalid_validity() -> None:
    report = valid_report(CATCHUP.CELL_BY_NAME["all-pane-scaling"])
    report["precondition"]["activity"]["observed_panes"] = [0]
    with pytest.raises(ValueError, match="activity mode"):
        CATCHUP.validate_report_semantics(report)
    report = valid_report(CATCHUP.CELL_BY_NAME["history-1000"])
    report["trace"]["saturated"] = True
    with pytest.raises(jsonschema.ValidationError):
        validator(REPORT_SCHEMA).validate(report)
    with pytest.raises(ValueError, match="invalid trace"):
        CATCHUP.validate_report_semantics(report)


def test_case_runner_binds_exact_schedule_entry_and_smoke_scope() -> None:
    plan = CATCHUP.plan_document(220022)
    entry = plan["schedule"][7]
    assert CASE.selected_schedule_entry(plan, 7) == entry
    with pytest.raises(ValueError, match="exactly one"):
        CASE.selected_schedule_entry(plan, len(plan["schedule"]))
    CASE.assert_case_is_smoke_supported("zero-history")
    with pytest.raises(ValueError, match="zero-history"):
        CASE.assert_case_is_smoke_supported("history-4096")


def test_case_outer_geometry_normalizes_single_and_multi_pane_widths() -> None:
    assert CASE.target_outer_size(1) == (480, 601)
    assert CASE.target_outer_size(2) == (960, 601)
    assert CASE.target_outer_size(4) == (960, 601)


def test_case_geometry_uses_the_requested_topology_validator(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    observed = {}
    monkeypatch.setattr(
        CASE.MUX, "capture_window", lambda window: {**window, "captured": True}
    )

    def stable(topology, controller, windows, user_state):
        observed.update(
            topology=topology.name,
            controller=controller,
            windows=windows,
            user_state=user_state,
        )
        return [{"name": "pane-0", "columns": 80, "rows": 24}]

    monkeypatch.setattr(CASE.MUX, "stable_geometry", stable)
    controller = object()
    window = {"address": "0x1"}
    user_state = {"focus_address": "0x2"}
    topology = CASE.topology_for_pane_count(4)
    result = CASE.settle_geometry(topology, controller, window, user_state)
    assert observed == {
        "topology": "four-grid",
        "controller": controller,
        "windows": {"pane-0": {"address": "0x1", "captured": True}},
        "user_state": user_state,
    }
    assert result[0]["name"] == "pane-0"


def test_case_waits_for_advanced_stable_terminal_revisions() -> None:
    class Controller:
        def __init__(self) -> None:
            self.runtime_ids = {"pane-0": "splint-0"}
            self.revisions = iter((1, 2, 2, 2, 2))

        def _json_command(self, _arguments: list[str]) -> dict[str, object]:
            return {
                "resource": {
                    "incarnation": 7,
                    "terminal_revision": next(self.revisions),
                },
                "data": {"columns": 80, "rows": [{}] * 24},
            }

    result = CASE.settled_snapshots(
        Controller(),
        ("pane-0",),
        {"pane-0": 1},
        1.0,
        stable_seconds=0.002,
        poll_seconds=0.001,
    )
    assert result[0]["revision"] == 2
    assert result[0]["incarnation"] == 7


def test_case_movement_delta_counts_only_new_body_free_records() -> None:
    records = [
        {"pid": 10, "sequence": 0, "stage": "window_event", "configure_count": 1},
        {"pid": 10, "sequence": 1, "stage": "client_apply", "copied_history_rows": 0},
        {"pid": 10, "sequence": 2, "stage": "frame_prepare"},
        {"pid": 10, "sequence": 3, "stage": "window_event", "output_enter_events": 1},
    ]
    delta = CASE.records_after(records, {(10, 0)})
    assert CASE.movement_counts(delta, 1) == {
        "position_changes": 1,
        "configure_events": 0,
        "output_enter_events": 1,
        "output_leave_events": 0,
        "semantic_applies": 1,
        "history_clones": 0,
        "pane_frame_rebuilds": 1,
    }


def test_case_trace_correlation_requires_exact_revision_and_callback() -> None:
    identity = {
        "splint_id": "018f4d8c-2a18-4b31-8c2f-9e7c5de77103",
        "incarnation": 2,
        "revision": 9,
    }
    common = {
        **identity,
        "pid": 42,
        "subscription_id": 3,
        "transaction_sequence": 4,
    }
    records = [
        {**common, "stage": "client_receive", "monotonic_raw_ns": 100},
        {
            **common,
            "stage": "client_apply",
            "monotonic_raw_ns": 110,
            "cached_history_rows": 0,
            "cached_history_bytes": 0,
        },
        {
            **common,
            "stage": "pane_commit",
            "monotonic_raw_ns": 130,
            "commit_sequence": 8,
        },
        {
            "stage": "frame_callback",
            "pid": 42,
            "monotonic_raw_ns": 145,
            "commit_sequence": 8,
        },
    ]
    trace = CASE.correlated_trace(records, [identity])
    assert trace["client_receive_to_pane_commit_ns"] == 30
    assert trace["commit_to_callback_ns"] == 15
    assert CASE.pane_preconditions(records, [identity], 0)[0]["verified"] is True
    with pytest.raises(RuntimeError, match="frame callback"):
        CASE.correlated_trace(records[:-1], [identity])


def test_runner_only_builds_or_validates_artifacts(tmp_path: pathlib.Path) -> None:
    plan_path = tmp_path / "plan.json"
    built = subprocess.run(
        [sys.executable, str(RUNNER), "--output", str(plan_path), "--seed", "77"],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert built.returncode == 0, built.stderr
    plan = json.loads(plan_path.read_text(encoding="utf-8"))
    assert len(plan["schedule"]) == 130
    assert "non-graphical plan" in built.stdout

    report_path = tmp_path / "report.json"
    entry = next(item for item in plan["schedule"] if item["cell"] == "zero-history")
    report = valid_report(CATCHUP.CELL_BY_NAME["zero-history"])
    report.update(
        case_id=entry["case_id"],
        plan_sha256=plan["plan_sha256"],
        phase=entry["phase"],
        iteration=entry["iteration"],
        execution_index=entry["execution_index"],
    )
    report_path.write_text(json.dumps(report), encoding="utf-8")
    checked = subprocess.run(
        [
            sys.executable,
            str(RUNNER),
            "--validate-report",
            str(report_path),
            "--plan",
            str(plan_path),
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert checked.returncode == 0, checked.stderr
    assert "valid graphical catch-up report" in checked.stdout
