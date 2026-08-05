from __future__ import annotations

import importlib.util
import json
import pathlib
import subprocess
import sys
import time
import types

import graphical_multiplexer as GRAPHICAL
import headless_multiplexer as HEADLESS
import jsonschema
import metrics as METRICS
import multiplexer_matrix as MATRIX
import pytest

ROOT = pathlib.Path(__file__).resolve().parents[2]
SCHEMA = ROOT / "tools/benchmark/multiplexer-matrix-plan-schema.json"
CELL_SCHEMA = ROOT / "tools/benchmark/graphical-multiplexer-schema.json"
MATRIX_RUNNER = ROOT / "tools/benchmark/run-graphical-multiplexer-matrix.py"


def load_matrix_runner():
    spec = importlib.util.spec_from_file_location(
        "splinterbench_test_mux_matrix", MATRIX_RUNNER
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


MATRIX_RUN = load_matrix_runner()


def parameters() -> dict[str, int | float]:
    return {
        "idle_warmup_seconds": 1.0,
        "idle_sample_seconds": 2.0,
        "lines": 2000,
        "columns": 80,
        "settle_seconds": 0.5,
        "ready_timeout_seconds": 10.0,
        "operation_timeout_seconds": 20.0,
        "lifetime_seconds": 300.0,
        "cell_timeout_seconds": 300.0,
        "timeout_cleanup_grace_seconds": 30.0,
    }


def test_schedule_is_seeded_complete_and_round_balanced() -> None:
    first = MATRIX.build_schedule(13_372_075, 3, 10)
    second = MATRIX.build_schedule(13_372_075, 3, 10)
    assert first == second
    assert len(first) == 13 * 4 * 3
    assert [cell.execution_index for cell in first] == list(range(len(first)))
    for phase, count in (("warmup", 3), ("measured", 10)):
        for iteration in range(count):
            cells = [
                (cell.stack, cell.topology)
                for cell in first
                if cell.phase == phase and cell.iteration == iteration
            ]
            assert len(cells) == 12
            assert set(cells) == {
                (stack, topology)
                for stack in MATRIX.STACKS
                for topology in MATRIX.TOPOLOGIES
            }


def test_plan_schema_and_resume_identity_are_strict() -> None:
    plan = MATRIX.plan_document(
        seed=13_372_075,
        warmup_runs=3,
        sample_runs=10,
        implementation_sha256="a" * 64,
        execution_identity={
            "host": {},
            "repository": None,
            "terminals": [{}, {}],
            "multiplexers": [{}, {}],
            "benchmark_stacks": [{}, {}, {}, {}],
            "extra_executables": [{}, {}, {}],
        },
        parameters=parameters(),
    )
    validator = jsonschema.Draft202012Validator(
        json.loads(SCHEMA.read_text(encoding="utf-8"))
    )
    validator.validate(plan)
    MATRIX.validate_plan_semantics(plan)
    MATRIX.assert_resume_compatible(plan, plan)

    changed = json.loads(json.dumps(plan))
    changed["parameters"]["lines"] = 1000
    with pytest.raises(ValueError, match="parameters"):
        MATRIX.assert_resume_compatible(changed, plan)

    reordered = json.loads(json.dumps(plan))
    reordered["schedule"][0], reordered["schedule"][1] = (
        reordered["schedule"][1],
        reordered["schedule"][0],
    )
    with pytest.raises(ValueError, match="schedule"):
        MATRIX.validate_plan_semantics(reordered)

    extra = json.loads(json.dumps(plan))
    extra["unexpected"] = True
    assert list(validator.iter_errors(extra))


def test_cell_schema_enforces_operations_stack_topology_and_bare_windows() -> None:
    resource = {
        "process_count": 2,
        "cpu_ticks": 0,
        "context_switches": 0,
        "rss_bytes": 100,
        "pss_bytes": 80,
    }
    operations = []
    for case in MATRIX.CASES:
        operation = GRAPHICAL.case_stub("foot-bare", "two-columns", case)
        if operation["applicability"]["status"] == "measured":
            operation.update(
                boundary="bounded-test",
                resources={
                    "before": {"infrastructure": resource, "total": resource},
                    "after": {"infrastructure": resource, "total": resource},
                    "before_membership": {"infrastructure": [], "workload": []},
                    "after_membership": {"infrastructure": [], "workload": []},
                    "membership_changed": False,
                    "delta": {"infrastructure": resource, "total": resource},
                },
                valid=True,
            )
        operations.append(operation)
    report = {
        "schema": "splinterm.benchmark.multiplexer-cell.v1",
        "case_id": "measured-00-0000-foot-bare-two-columns",
        "plan_sha256": "a" * 64,
        "phase": "measured",
        "iteration": 0,
        "execution_index": 0,
        "stack": GRAPHICAL.stack_identity("foot-bare"),
        "topology": {
            "name": "two-columns",
            "pane_count": 2,
            "panes": ["pane-0", "pane-1"],
            "tree": {},
        },
        "runtime_ids": {"pane-0": "a", "pane-1": "b"},
        "windows": [
            {
                "pane": name,
                "address": f"0x{index}",
                "pid": 10 + index,
                "start_ticks": 100 + index,
                "class": "FootBare",
                "workspace": 8,
                "monitor": 1,
                "geometry": [index * 480, 0, 480, 600],
            }
            for index, name in enumerate(("pane-0", "pane-1"))
        ],
        "processes": {
            "infrastructure_pids": [10, 11],
            "workload_pids": [20, 21],
            "roles": [],
            "role_history": [
                {
                    "stage": "initial-attach",
                    "infrastructure_pids": [10, 11],
                    "workload_pids": [20, 21],
                    "roles": [],
                }
            ],
            "role_sets_disjoint": True,
        },
        "operations": operations,
        "isolation": {
            "namespace": "foot-bare:test",
            "workspace": 8,
            "monitor": "DP-2",
            "no_initial_focus": True,
            "host_state_before": {},
            "host_state_after": {},
            "host_state_preserved": True,
            "ambient_before": {"process_count": None, "default_session_count": None},
            "ambient_after": {"process_count": None, "default_session_count": None},
            "ambient_names_recorded": False,
        },
        "cleanup": {
            "windows_absent": True,
            "namespace_absent": True,
            "server_absent": True,
            "clients_absent": True,
            "workloads_absent": True,
            "process_forest_absent": True,
            "ambient_counts_unchanged": True,
            "verified": True,
            "failure": None,
        },
        "failure": None,
        "valid": True,
        "notes": [],
    }
    validator = jsonschema.Draft202012Validator(
        json.loads(CELL_SCHEMA.read_text(encoding="utf-8"))
    )
    validator.validate(report)

    wrong_stack = json.loads(json.dumps(report))
    wrong_stack["stack"]["multiplexer"] = "tmux"
    assert list(validator.iter_errors(wrong_stack))
    missing_window = json.loads(json.dumps(report))
    missing_window["windows"].pop()
    assert list(validator.iter_errors(missing_window))
    invalid_operation = json.loads(json.dumps(report))
    invalid_operation["operations"][0]["valid"] = False
    assert list(validator.iter_errors(invalid_operation))

    failed_before_launch = json.loads(json.dumps(report))
    failed_before_launch.update(
        runtime_ids={},
        windows=[],
        processes=None,
        failure="bounded setup failure",
        valid=False,
    )
    for operation in failed_before_launch["operations"]:
        if operation["applicability"]["status"] == "measured":
            operation.update(
                boundary=None,
                metrics={},
                pane_metrics={},
                resources=None,
                valid=False,
            )
    validator.validate(failed_before_launch)


def test_bare_foot_applicability_is_explicit() -> None:
    assert MATRIX.case_applicability("foot-bare", "four-grid", "divider-resize") == {
        "status": "not-applicable",
        "reason": "independent Foot windows have no shared multiplexer divider",
    }
    assert (
        MATRIX.case_applicability("foot-bare", "two-columns", "detach-reattach")[
            "status"
        ]
        == "not-applicable"
    )
    assert (
        MATRIX.case_applicability("splinterm-native", "single", "divider-resize")[
            "status"
        ]
        == "not-applicable"
    )
    assert MATRIX.case_applicability("foot-tmux", "four-grid", "input") == {
        "status": "measured",
        "reason": "supported by this stack and topology",
    }


def test_resume_reuses_only_exact_valid_cleaned_cell() -> None:
    cell = MATRIX.build_schedule(7, 0, 1)[0]
    report = {
        "schema": "splinterm.benchmark.multiplexer-cell.v1",
        "case_id": cell.case_id,
        "plan_sha256": "b" * 64,
        "phase": cell.phase,
        "iteration": cell.iteration,
        "execution_index": cell.execution_index,
        "stack": {"name": cell.stack},
        "topology": {"name": cell.topology},
        "valid": True,
        "cleanup": {"verified": True},
    }
    assert MATRIX.completed_cell_is_reusable(report, cell, "b" * 64)
    report["cleanup"]["verified"] = False
    assert not MATRIX.completed_cell_is_reusable(report, cell, "b" * 64)


def test_splinterm_final_lifecycle_accepts_both_documented_client_outcomes() -> None:
    assert (
        GRAPHICAL.splinterm_lifecycle_window_state(
            "exited-retained-restorable",
            final_leaf=True,
            server_alive=True,
            window_alive=False,
        )
        == "final-window-exited-with-retained-restorable-leaf"
    )
    assert (
        GRAPHICAL.splinterm_lifecycle_window_state(
            "exited-auto-closed-by-graphical-client",
            final_leaf=True,
            server_alive=True,
            window_alive=False,
        )
        == "final-close-committed-unmap-complete"
    )
    assert (
        GRAPHICAL.splinterm_lifecycle_window_state(
            "exited-retained-restorable",
            final_leaf=False,
            server_alive=True,
            window_alive=True,
        )
        == "running-with-retained-exited-leaf"
    )
    assert (
        GRAPHICAL.splinterm_lifecycle_window_state(
            "exited-retained-restorable",
            final_leaf=True,
            server_alive=False,
            window_alive=False,
        )
        is None
    )


def test_topology_rectangles_and_geometry_preserve_four_grid() -> None:
    rectangles = GRAPHICAL.topology_rectangles("four-grid", 100, 200, 961, 601)
    assert rectangles == {
        "pane-0": (100, 200, 480, 300),
        "pane-1": (100, 500, 480, 301),
        "pane-2": (580, 200, 481, 300),
        "pane-3": (580, 500, 481, 301),
    }
    topology = types.SimpleNamespace(
        name="four-grid", pane_names=("pane-0", "pane-1", "pane-2", "pane-3")
    )
    geometry = [
        {"name": "pane-0", "x": 0, "y": 0, "columns": 60, "rows": 20},
        {"name": "pane-1", "x": 0, "y": 22, "columns": 60, "rows": 17},
        {"name": "pane-2", "x": 61, "y": 0, "columns": 59, "rows": 20},
        {"name": "pane-3", "x": 61, "y": 21, "columns": 59, "rows": 19},
    ]
    GRAPHICAL.validate_topology_geometry(topology, geometry)
    with pytest.raises(RuntimeError, match="equal-ratio"):
        GRAPHICAL.validate_equal_topology_geometry(topology, geometry)


def test_exact_resource_snapshot_separates_infrastructure_and_total(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        GRAPHICAL,
        "snapshot_processes",
        lambda pids: METRICS.ProcessMetrics(
            len(pids), sum(pids), sum(pids) * 2, sum(pids) * 3
        ),
    )
    monkeypatch.setattr(
        GRAPHICAL,
        "snapshot_process_memory",
        lambda pids: {
            "aggregate": {
                "rss_bytes": sum(pids) * 5,
                "pss_bytes": sum(pids) * 4,
            }
        },
    )
    snapshot = GRAPHICAL.exact_resource_snapshot([10, 11], [20, 21])
    assert snapshot["infrastructure"]["process_count"] == 2
    assert snapshot["total"]["process_count"] == 4
    assert snapshot["infrastructure"]["pss_bytes"] == 84
    assert snapshot["total"]["pss_bytes"] == 248
    with pytest.raises(RuntimeError, match="overlap"):
        GRAPHICAL.exact_resource_snapshot([10, 11], [11, 20])


def test_exact_pid_metrics_do_not_expand_descendants(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    observed: list[int] = []

    def one(_root: pathlib.Path, pid: int) -> METRICS.ProcessMetrics:
        observed.append(pid)
        return METRICS.ProcessMetrics(1, pid, pid * 2, pid * 3)

    monkeypatch.setattr(METRICS, "_process_metrics", one)
    result = METRICS.snapshot_processes([3, 2, 3], pathlib.Path("/proc-test"))
    assert observed == [2, 3]
    assert result == METRICS.ProcessMetrics(2, 5, 10, 15)


def wait_json(path: pathlib.Path, timeout: float = 3.0) -> dict[str, object]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists():
            return json.loads(path.read_text(encoding="utf-8"))
        time.sleep(0.005)
    raise TimeoutError(f"timed out waiting for {path}")


def write_command(path: pathlib.Path, value: dict[str, object]) -> None:
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps(value), encoding="utf-8")
    temporary.replace(path)


def test_controlled_child_runs_output_input_and_lifecycle(
    tmp_path: pathlib.Path,
) -> None:
    ready = tmp_path / "ready.json"
    control = tmp_path / "control"
    process = subprocess.Popen(
        [
            sys.executable,
            str(ROOT / "tools/benchmark/workloads/bench-child.py"),
            "multiplexer",
            "--ready-file",
            str(ready),
            "--control-dir",
            str(control),
            "--idle-seconds",
            "5",
        ],
        stdin=subprocess.PIPE,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )
    try:
        assert wait_json(ready)["event"] == "ready"
        write_command(
            control / "command-000.json",
            {
                "schema": "splinterm.benchmark.child-command.v1",
                "sequence": 0,
                "action": "output",
                "workload": "unicode",
                "lines": 2,
                "columns": 80,
            },
        )
        output = wait_json(control / "result-000.json")
        assert output["event"] == "write_complete"
        assert output["workload"] == "unicode"
        assert int(output["payload_bytes"]) > 0

        write_command(
            control / "command-001.json",
            {
                "schema": "splinterm.benchmark.child-command.v1",
                "sequence": 1,
                "action": "input",
                "token": "x",
            },
        )
        assert process.stdin is not None
        process.stdin.write(b"x\n")
        process.stdin.flush()
        received = wait_json(control / "result-001.json")
        assert received["event"] == "input_received"
        assert received["token"] == "x"

        write_command(
            control / "command-002.json",
            {
                "schema": "splinterm.benchmark.child-command.v1",
                "sequence": 2,
                "action": "exit",
            },
        )
        assert wait_json(control / "result-002.json")["event"] == "exit_started"
        assert process.wait(timeout=3) == 0
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=2)


def test_cell_timeout_requests_graceful_process_group_cleanup(
    tmp_path: pathlib.Path,
) -> None:
    marker = tmp_path / "cleaned"
    script = tmp_path / "cell.py"
    script.write_text(
        "import pathlib, signal, time\n"
        f"marker = pathlib.Path({str(marker)!r})\n"
        "def stop(_signum, _frame):\n"
        "    marker.write_text('yes')\n"
        "    raise SystemExit(7)\n"
        "signal.signal(signal.SIGTERM, stop)\n"
        "time.sleep(30)\n",
        encoding="utf-8",
    )
    completed, timed_out, graceful = MATRIX_RUN.run_cell_command(
        [sys.executable, str(script)], 0.05, 2.0
    )
    assert timed_out is True
    assert graceful is True
    assert completed.returncode == 7
    assert marker.read_text(encoding="utf-8") == "yes"


def test_controller_actions_target_captured_runtime_ids(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls: list[list[str]] = []

    def run(command, _environment, timeout=10):
        del timeout
        calls.append(list(command))
        return types.SimpleNamespace(stdout="", stderr="", returncode=0)

    monkeypatch.setattr(HEADLESS, "checked_run", run)

    tmux = HEADLESS.TmuxController.__new__(HEADLESS.TmuxController)
    tmux.plan = types.SimpleNamespace(command_prefix=("tmux", "-L", "owned"))
    tmux.environment = {}
    tmux.topology = types.SimpleNamespace(name="two-columns")
    tmux.runtime_ids = {"pane-1": "%7"}
    tmux.focus_pane("pane-1")
    tmux.send_input("pane-1", "x")
    tmux.resize_divider("pane-1")
    tmux.close_pane("pane-1")
    assert calls[:5] == [
        ["tmux", "-L", "owned", "select-pane", "-t", "%7"],
        ["tmux", "-L", "owned", "send-keys", "-t", "%7", "-l", "x"],
        ["tmux", "-L", "owned", "send-keys", "-t", "%7", "Enter"],
        ["tmux", "-L", "owned", "resize-pane", "-t", "%7", "-R", "6"],
        ["tmux", "-L", "owned", "kill-pane", "-t", "%7"],
    ]

    zellij = HEADLESS.ZellijController.__new__(HEADLESS.ZellijController)
    zellij.plan = types.SimpleNamespace(
        command_prefix=("zellij", "--config", "owned.kdl"),
        session_name="owned-session",
    )
    zellij.environment = {}
    zellij.topology = types.SimpleNamespace(name="two-columns")
    zellij.runtime_ids = {"pane-1": "terminal_9"}
    zellij.focus_pane("pane-1")
    zellij.send_input("pane-1", "x")
    zellij.resize_divider("pane-1")
    zellij.close_pane("pane-1")
    assert calls[5:] == [
        [
            "zellij",
            "--config",
            "owned.kdl",
            "--session",
            "owned-session",
            "action",
            "focus-pane-id",
            "terminal_9",
        ],
        [
            "zellij",
            "--config",
            "owned.kdl",
            "--session",
            "owned-session",
            "action",
            "write-chars",
            "--pane-id",
            "terminal_9",
            "x\n",
        ],
        [
            "zellij",
            "--config",
            "owned.kdl",
            "--session",
            "owned-session",
            "action",
            "resize",
            "--pane-id",
            "terminal_9",
            "increase",
            "right",
        ],
        [
            "zellij",
            "--config",
            "owned.kdl",
            "--session",
            "owned-session",
            "action",
            "close-pane",
            "--pane-id",
            "terminal_9",
        ],
    ]
