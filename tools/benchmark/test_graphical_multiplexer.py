from __future__ import annotations

import copy
import importlib.util
import json
import pathlib
import subprocess
import types

import jsonschema
import pytest

ROOT = pathlib.Path(__file__).resolve().parents[2]
RUNNER = ROOT / "tools/benchmark/run-graphical-multiplexer-smoke.py"
SCHEMA = ROOT / "tools/benchmark/graphical-multiplexer-smoke-schema.json"


def load_runner():
    spec = importlib.util.spec_from_file_location(
        "splinterbench_test_graphical_multiplexer", RUNNER
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


RUN = load_runner()
VALIDATOR = jsonschema.Draft202012Validator(
    json.loads(SCHEMA.read_text(encoding="utf-8"))
)


def identity(pid: int) -> dict[str, int]:
    return {"pid": pid, "start_ticks": pid * 10}


def valid_report() -> dict[str, object]:
    panes = [
        {"name": "pane-0", "runtime_id": "a", "columns": 59, "rows": 30},
        {"name": "pane-1", "runtime_id": "b", "columns": 59, "rows": 30},
    ]
    ready = {
        name: {
            "schema": "splinterm.benchmark.child.v1",
            "event": "ready",
            "monotonic_ns": 100 + index,
            "pid": 20 + index,
        }
        for index, name in enumerate(("pane-0", "pane-1"))
    }
    return {
        "schema": "splinterm.benchmark.multiplexer-graphical-smoke.v1",
        "stack": "splinterm-native",
        "implementation": "splinterm",
        "topology": {
            "name": "two-columns",
            "pane_count": 2,
            "panes": ["pane-0", "pane-1"],
        },
        "runtime_ids": {"pane-0": "a", "pane-1": "b"},
        "readiness": ready,
        "geometry": {
            "before_graphical_attach": panes,
            "after_graphical_attach": panes,
            "stable_seconds": 0.3,
        },
        "process_roles": {
            "role_sets_disjoint": True,
            "roles": [
                {"role": "server", "processes": [identity(10)]},
                {"role": "terminal-host", "processes": [identity(11)]},
                {"role": "multiplexer-client", "processes": []},
                {"role": "helper", "processes": []},
                {
                    "role": "workload",
                    "processes": [identity(20), identity(21)],
                },
            ],
        },
        "capture": {
            "path": "splinterm-native.png",
            "sha256": "a" * 64,
            "width": 960,
            "height": 600,
        },
        "isolation": {
            "workspace": 8,
            "monitor": "DP-2",
            "no_initial_focus": True,
            "host_state_before": {
                "workspace": {"id": 1, "monitor": "DP-1"},
                "focus_address": "0x123",
                "cursor": {"x": 100, "y": 200},
            },
            "host_state_preserved": True,
            "run_id": "gm-splinterm-1",
            "ambient_before": {
                "process_count": None,
                "default_session_count": None,
            },
            "ambient_after": {
                "process_count": None,
                "default_session_count": None,
            },
            "ambient_names_recorded": False,
        },
        "cleanup": {
            "window_absent": True,
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


def assert_invalid(document: dict[str, object]) -> None:
    assert list(VALIDATOR.iter_errors(document))


def test_native_snapshot_geometry_counts_cli_row_array(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    controller = RUN.SplintermController.__new__(RUN.SplintermController)
    controller.topology = types.SimpleNamespace(pane_names=("pane-0", "pane-1"))
    controller.runtime_ids = {"pane-0": "a", "pane-1": "b"}
    controller.client = pathlib.Path("/benchmark/splinterm")
    controller.environment = {}

    def snapshot(command, **kwargs):
        assert kwargs["env"] == {}
        splint_id = command[-1]
        rows = [{"cells": []}] * (3 if splint_id == "a" else 4)
        return subprocess.CompletedProcess(
            command,
            0,
            json.dumps({"data": {"columns": 80, "rows": rows}}),
            "",
        )

    monkeypatch.setattr(RUN.subprocess, "run", snapshot)

    assert RUN.splinterm_geometry(controller) == [
        {"name": "pane-0", "runtime_id": "a", "columns": 80, "rows": 3},
        {"name": "pane-1", "runtime_id": "b", "columns": 80, "rows": 4},
    ]


def test_graphical_smoke_schema_is_strict() -> None:
    jsonschema.Draft202012Validator.check_schema(VALIDATOR.schema)
    report = valid_report()
    VALIDATOR.validate(report)

    mismatch = copy.deepcopy(report)
    mismatch["stack"] = "foot-tmux"
    assert_invalid(mismatch)

    incomplete_cleanup = copy.deepcopy(report)
    incomplete_cleanup["cleanup"]["namespace_absent"] = False  # type: ignore[index]
    assert_invalid(incomplete_cleanup)

    reordered_roles = copy.deepcopy(report)
    roles = reordered_roles["process_roles"]["roles"]  # type: ignore[index]
    roles[0], roles[1] = roles[1], roles[0]
    assert_invalid(reordered_roles)

    missing_external_client = copy.deepcopy(report)
    missing_external_client["stack"] = "foot-tmux"
    missing_external_client["implementation"] = "tmux"
    assert_invalid(missing_external_client)


def test_validate_two_columns_rejects_wrong_orientation() -> None:
    RUN.validate_two_columns(
        [
            {
                "name": "pane-0",
                "runtime_id": "%0",
                "x": 0,
                "y": 0,
                "columns": 60,
                "rows": 30,
            },
            {
                "name": "pane-1",
                "runtime_id": "%1",
                "x": 61,
                "y": 0,
                "columns": 59,
                "rows": 30,
            },
        ]
    )
    with pytest.raises(RuntimeError, match="two columns"):
        RUN.validate_two_columns(
            [
                {
                    "name": "pane-0",
                    "runtime_id": "%0",
                    "x": 0,
                    "y": 0,
                    "columns": 60,
                    "rows": 15,
                },
                {
                    "name": "pane-1",
                    "runtime_id": "%1",
                    "x": 0,
                    "y": 16,
                    "columns": 60,
                    "rows": 14,
                },
            ]
        )


def test_wait_stable_geometry_tolerates_transient_native_mismatch(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    transient = [
        {"name": "pane-0", "runtime_id": "a", "columns": 59, "rows": 30},
        {"name": "pane-1", "runtime_id": "b", "columns": 80, "rows": 24},
    ]
    settled = [
        {"name": "pane-0", "runtime_id": "a", "columns": 59, "rows": 30},
        {"name": "pane-1", "runtime_id": "b", "columns": 59, "rows": 30},
    ]
    samples = iter((transient, settled, settled))
    monkeypatch.setattr(RUN, "assert_user_state", lambda _expected: None)
    monkeypatch.setattr(RUN, "geometry", lambda _controller: next(samples))

    assert (
        RUN.wait_stable_geometry(object(), {}, timeout=1.0, stable_seconds=0.0)
        == settled
    )


def test_launch_specs_target_only_owned_namespaces(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(RUN.shutil, "which", lambda name: f"/usr/bin/{name}")

    native = RUN.SplintermController.__new__(RUN.SplintermController)
    native.client = pathlib.Path("/usr/bin/splinterm")
    native.runtime_ids = {"pane-0": "splint-a", "pane-1": "splint-b"}
    native.window_id = "window-a"
    native.environment = {"SPLINTERM_SOCKET": "/tmp/owned.sock"}
    native._json_command = lambda arguments: {
        "data": {
            "splints": [
                {
                    "dojo_id": "dojo-a",
                    "window_id": "window-a",
                    "splint_id": "splint-a",
                },
                {
                    "dojo_id": "dojo-a",
                    "window_id": "window-a",
                    "splint_id": "splint-b",
                },
            ]
        }
    }
    command, environment = RUN.launch_spec("splinterm-native", native)
    assert command == [
        "/usr/bin/splinterm",
        "window",
        "--dojo-id",
        "dojo-a",
        "--window-id",
        "window-a",
    ]
    assert RUN.owned_window_token(native) == "window-a"
    assert environment == {"SPLINTERM_SOCKET": "/tmp/owned.sock"}

    tmux = RUN.TmuxController.__new__(RUN.TmuxController)
    tmux.plan = types.SimpleNamespace(
        command_prefix=("/usr/bin/tmux", "-L", "owned", "-f", "/dev/null"),
        session_name="splinterbench-owned",
    )
    tmux.environment = {"TMUX_TMPDIR": "/tmp/owned"}
    command, environment = RUN.launch_spec("foot-tmux", tmux)
    assert command[-8:] == [
        "/usr/bin/tmux",
        "-L",
        "owned",
        "-f",
        "/dev/null",
        "attach-session",
        "-t",
        "splinterbench-owned",
    ]
    assert environment == {"TMUX_TMPDIR": "/tmp/owned"}

    zellij = RUN.ZellijController.__new__(RUN.ZellijController)
    zellij.plan = types.SimpleNamespace(
        command_prefix=("/usr/bin/zellij", "--config", "/repo/zellij.kdl"),
        session_name="splinterbench-owned",
    )
    zellij.environment = {"ZELLIJ_SOCKET_DIR": "/tmp/owned"}
    command, environment = RUN.launch_spec("foot-zellij", zellij)
    assert command[-5:] == [
        "/usr/bin/zellij",
        "--config",
        "/repo/zellij.kdl",
        "attach",
        "splinterbench-owned",
    ]
    assert environment == {"ZELLIJ_SOCKET_DIR": "/tmp/owned"}
