"""Portable tests for the non-graphical benchmark foundation."""

from __future__ import annotations

import importlib.util
import json
import pathlib
import subprocess
import sys

import pytest

ROOT = pathlib.Path(__file__).resolve().parents[2]
BENCHMARK = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(BENCHMARK))

from adapters.base import TerminalAdapter, file_sha256  # noqa: E402
from correctness import build_report, write_report  # noqa: E402
from metrics import (  # noqa: E402
    process_memory,
    process_tree,
    read_cgroup_v2,
    snapshot_process_forest,
    snapshot_process_memory_forest,
    snapshot_process_tree,
)
from summary import summarize_samples, summarize_values  # noqa: E402
import latency as latency_boundary  # noqa: E402


def load_module(path: pathlib.Path, name: str):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ExampleAdapter(TerminalAdapter):
    name = "foot"
    executable_names = ()
    version_arguments = ("--version",)

    def __init__(self, executable: pathlib.Path):
        self.executable = executable

    def candidates(self, root: pathlib.Path):
        del root
        return (self.executable,)


def test_publication_metrics_overhead_bootstrap_is_deterministic() -> None:
    module = load_module(
        ROOT / "tools/performance/run-publication-metrics-overhead.py",
        "publication_metrics_overhead",
    )
    first = module.bootstrap([10.0, 11.0, 12.0], [10.0, 11.0, 12.0], 7, 100)
    second = module.bootstrap([10.0, 11.0, 12.0], [10.0, 11.0, 12.0], 7, 100)
    assert first == second
    assert first["point_percent"] == 0.0


def test_phase9_binary_identity_records_exact_release_artifact(
    tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module = load_module(
        ROOT / "tools/performance/run-phase9-baseline.py", "phase9_baseline_identity"
    )
    monkeypatch.setattr(module, "ROOT", tmp_path)
    executable = tmp_path / "target/release/example"
    executable.parent.mkdir(parents=True)
    executable.write_bytes(b"release artifact")

    assert module.binary_identity(executable) == {
        "path": "target/release/example",
        "sha256": "133cfccb5b503cf4040c95f3dfad56d07c1574283a1e39066b594f6ee33711ba",
        "size_bytes": 16,
    }


def test_phase9_baseline_rejects_dirty_repository(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module = load_module(
        ROOT / "tools/performance/run-phase9-baseline.py", "phase9_baseline_clean"
    )
    responses = {
        ("git", "rev-parse", "HEAD"): "0" * 40,
        ("git", "status", "--porcelain"): " M source.rs",
    }
    monkeypatch.setattr(module, "output", lambda command: responses[tuple(command)])

    with pytest.raises(RuntimeError, match="requires a clean repository"):
        module.require_clean_repository()


def test_adapter_probe_records_exact_executable(tmp_path: pathlib.Path) -> None:
    executable = tmp_path / "terminal"
    executable.write_text("#!/bin/sh\nprintf 'example 1.2.3\\n'\n", encoding="utf-8")
    executable.chmod(0o700)
    identity = ExampleAdapter(executable).probe(tmp_path)
    assert identity.available is True
    assert identity.executable == str(executable.resolve())
    assert identity.version == "example 1.2.3"
    assert identity.sha256 == file_sha256(executable)


def _write_process(
    proc: pathlib.Path, pid: int, children: str, rss_kib: int, ticks: tuple[int, int]
) -> None:
    process = proc / str(pid)
    task = process / "task" / str(pid)
    task.mkdir(parents=True)
    (task / "children").write_text(children, encoding="utf-8")
    fields = ["S", *(["0"] * 10), str(ticks[0]), str(ticks[1]), "0"]
    (process / "stat").write_text(
        f"{pid} (benchmark child) {' '.join(fields)}\n", encoding="utf-8"
    )
    (process / "status").write_text(
        f"VmRSS:\t{rss_kib} kB\nvoluntary_ctxt_switches:\t2\n"
        "nonvoluntary_ctxt_switches:\t3\n",
        encoding="utf-8",
    )


def test_process_tree_snapshot_aggregates_descendants(tmp_path: pathlib.Path) -> None:
    _write_process(tmp_path, 10, "11 12\n", 4, (7, 3))
    _write_process(tmp_path, 11, "\n", 5, (2, 1))
    _write_process(tmp_path, 12, "\n", 6, (4, 2))
    assert process_tree(tmp_path, 10) == [10, 11, 12]
    metrics = snapshot_process_tree(10, tmp_path)
    assert metrics.process_count == 3
    assert metrics.cpu_ticks == 19
    assert metrics.rss_bytes == 15 * 1024
    assert metrics.context_switches == 15
    forest = snapshot_process_forest([10, 11], tmp_path)
    assert forest == metrics


def test_smaps_rollup_attributes_processes_without_body_content(
    tmp_path: pathlib.Path,
) -> None:
    _write_process(tmp_path, 10, "11\n", 4, (1, 2))
    _write_process(tmp_path, 11, "\n", 5, (3, 4))
    for pid, name, values in (
        (10, "splinterd", (100, 90, 4, 66, 70, 8, 2)),
        (11, "splinterm", (50, 40, 5, 15, 12, 6, 1)),
    ):
        root = tmp_path / str(pid)
        (root / "comm").write_text(name + "\n", encoding="utf-8")
        rss, pss, private_clean, private_dirty, anonymous, shared, shmem = values
        (root / "smaps_rollup").write_text(
            f"Rss: {rss} kB\nPss: {pss} kB\n"
            f"Private_Clean: {private_clean} kB\nPrivate_Dirty: {private_dirty} kB\n"
            f"Anonymous: {anonymous} kB\nShared_Clean: {shared} kB\n"
            f"Shared_Dirty: 0 kB\nShmemPmdMapped: {shmem} kB\n",
            encoding="utf-8",
        )
    first = process_memory(10, tmp_path)
    assert first is not None
    assert first.name == "splinterd"
    assert first.private_anon_bytes == 70 * 1024
    assert first.private_file_bytes == 0
    forest = snapshot_process_memory_forest([10], tmp_path)
    assert [item["pid"] for item in forest["processes"]] == [10, 11]
    assert forest["aggregate"] == {
        "rss_bytes": 150 * 1024,
        "pss_bytes": 130 * 1024,
        "private_anon_bytes": 82 * 1024,
        "private_file_bytes": 8 * 1024,
        "shared_bytes": 14 * 1024,
        "shmem_bytes": 3 * 1024,
    }


def test_cgroup_reader_handles_max_and_cpu_stat(tmp_path: pathlib.Path) -> None:
    (tmp_path / "memory.current").write_text("1024\n", encoding="utf-8")
    (tmp_path / "memory.peak").write_text("2048\n", encoding="utf-8")
    (tmp_path / "pids.current").write_text("max\n", encoding="utf-8")
    (tmp_path / "cpu.stat").write_text(
        "usage_usec 90\nuser_usec 60\nsystem_usec 30\n", encoding="utf-8"
    )
    assert read_cgroup_v2(tmp_path) == {
        "memory_current_bytes": 1024,
        "memory_peak_bytes": 2048,
        "process_count": None,
        "cpu_usage_usec": 90,
        "cpu_user_usec": 60,
        "cpu_system_usec": 30,
    }


def test_workload_child_writes_side_channel_records(tmp_path: pathlib.Path) -> None:
    ready = tmp_path / "ready.json"
    done = tmp_path / "done.json"
    result = subprocess.run(
        [
            sys.executable,
            str(BENCHMARK / "workloads/bench-child.py"),
            "ansi",
            "--lines",
            "4",
            "--columns",
            "40",
            "--ready-file",
            str(ready),
            "--done-file",
            str(done),
        ],
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0
    assert b"\x1b[0mSPLINTERBENCH_DONE\n" in result.stdout
    assert b"\x1b[48;2;17;239;113m" in result.stdout
    assert json.loads(ready.read_text())["event"] == "ready"
    completion = json.loads(done.read_text())
    assert completion["event"] == "write_complete"
    assert completion["total_bytes"] == len(result.stdout)
    assert completion["duration_ns"] >= 0
    assert completion["pid"] > 0


def test_retention_v2_settle_points_are_sorted_bounded_and_unique() -> None:
    module = load_module(
        BENCHMARK / "run-graphical-retention-v2.py", "graphical_retention_v2"
    )
    assert module.parse_settle_points("120,2,10,30,2") == [2.0, 10.0, 30.0, 120.0]
    with pytest.raises(ValueError, match="between zero and 120"):
        module.parse_settle_points("121")


def test_graphical_launcher_preserves_exec_and_records_output(
    tmp_path: pathlib.Path,
) -> None:
    idle = load_module(BENCHMARK / "run-graphical-idle.py", "graphical_launcher")
    launcher = tmp_path / "launch.sh"
    idle.write_launcher(
        launcher,
        ["bash", "-c", "printf stdout; printf stderr >&2; exit 7"],
        {"EXAMPLE": "value"},
    )
    completed = subprocess.run([str(launcher)], check=False)
    assert completed.returncode == 7
    assert launcher.with_suffix(".stdout").read_text() == "stdout"
    assert launcher.with_suffix(".stderr").read_text() == "stderr"
    source = launcher.read_text()
    assert "exec env " in source
    assert not launcher.with_suffix(".status.json").exists()


def test_graphical_cardinality_failure_records_expected_and_observed_windows(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    idle = load_module(BENCHMARK / "run-graphical-idle.py", "graphical_idle_guard")
    clients = [
        {
            "address": "0x2",
            "class": "unexpected",
            "title": "second",
            "pid": 12,
            "monitor": 1,
            "workspace": {"id": 8},
            "mapped": True,
            "hidden": False,
        },
        {
            "address": "0x1",
            "class": "expected",
            "title": "first",
            "pid": 11,
            "monitor": 1,
            "workspace": {"id": 8},
            "mapped": True,
            "hidden": False,
        },
    ]

    class FakeGuard:
        TEST_WORKSPACE = 8

        @staticmethod
        def all_clients():
            return clients

        @staticmethod
        def test_monitor_id() -> int:
            return 1

        @staticmethod
        def assert_user_workspace_untouched() -> None:
            raise AssertionError("cardinality failure must occur first")

    monkeypatch.setattr(idle, "V1", FakeGuard)
    with pytest.raises(idle.WindowIsolationError) as caught:
        idle.assert_owned_window("expected", "0x1")
    assert caught.value.reason == (
        "reserved workspace does not contain exactly one benchmark window"
    )
    assert caught.value.details == {
        "expected": {
            "address": "0x1",
            "class": "expected",
            "monitor": 1,
            "workspace": 8,
        },
        "expected_address_observed_globally": [
            {
                "address": "0x1",
                "class": "expected",
                "hidden": False,
                "mapped": True,
                "monitor": 1,
                "pid": 11,
                "title": "first",
                "workspace": 8,
            }
        ],
        "observed": [
            {
                "address": "0x1",
                "class": "expected",
                "hidden": False,
                "mapped": True,
                "monitor": 1,
                "pid": 11,
                "title": "first",
                "workspace": 8,
            },
            {
                "address": "0x2",
                "class": "unexpected",
                "hidden": False,
                "mapped": True,
                "monitor": 1,
                "pid": 12,
                "title": "second",
                "workspace": 8,
            },
        ],
        "observed_count": 2,
    }


def test_graphical_guard_distinguishes_escaped_from_closed_window(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    idle = load_module(BENCHMARK / "run-graphical-idle.py", "graphical_idle_escape")
    escaped = {
        "address": "0x1",
        "class": "expected",
        "title": "escaped",
        "pid": 11,
        "monitor": 0,
        "workspace": {"id": 3},
        "mapped": True,
        "hidden": False,
    }

    class FakeGuard:
        TEST_WORKSPACE = 8

        @staticmethod
        def all_clients():
            return [escaped]

        @staticmethod
        def test_monitor_id() -> int:
            return 1

        @staticmethod
        def assert_user_workspace_untouched() -> None:
            raise AssertionError("cardinality failure must occur first")

    monkeypatch.setattr(idle, "V1", FakeGuard)
    with pytest.raises(idle.WindowIsolationError) as caught:
        idle.assert_owned_window("expected", "0x1")
    assert caught.value.details["observed"] == []
    assert caught.value.details["observed_count"] == 0
    assert caught.value.details["expected_address_observed_globally"][0] == {
        "address": "0x1",
        "class": "expected",
        "hidden": False,
        "mapped": True,
        "monitor": 0,
        "pid": 11,
        "title": "escaped",
        "workspace": 3,
    }


def test_retention_v2_failure_record_preserves_phase_and_isolation_details() -> None:
    retention = load_module(
        BENCHMARK / "run-graphical-retention-v2.py",
        "graphical_retention_failure_record",
    )

    class ExampleIsolationError(RuntimeError):
        details = {"observed_count": 0}

    error = ExampleIsolationError("window disappeared")
    assert retention.failure_record("settle_sampling", error) == {
        "phase": "settle_sampling",
        "type": "ExampleIsolationError",
        "message": "window disappeared",
        "isolation": {"observed_count": 0},
    }


def test_retention_v2_main_serializes_pretrigger_and_cleanup_failures(
    tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    retention = load_module(
        BENCHMARK / "run-graphical-retention-v2.py",
        "graphical_retention_main_failure",
    )
    output = tmp_path / "report"
    calls = {"ownership": 0, "cleanup": 0, "killed": []}
    isolation_details = {
        "expected": {"address": "0x1", "workspace": 8},
        "observed": [],
        "observed_count": 0,
    }

    monkeypatch.setenv("HYPRLAND_INSTANCE_SIGNATURE", "test-instance")
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "run-graphical-retention-v2.py",
            str(output),
            "--terminal",
            "foot",
            "--settle-seconds",
            "0",
            "--settle-points",
            "2",
        ],
    )
    monkeypatch.setattr(retention.V1, "assert_test_workspace_isolated", lambda: None)
    monkeypatch.setattr(retention.V1, "assert_user_workspace_untouched", lambda: None)
    monkeypatch.setattr(retention.V1, "all_clients", lambda: [])
    monkeypatch.setattr(
        retention.V1,
        "kill_oracle_window",
        lambda address: calls["killed"].append(address),
    )
    monkeypatch.setattr(
        retention.COMMON,
        "launch_command",
        lambda *args, **kwargs: (["terminal"], {}),
    )
    monkeypatch.setattr(retention.COMMON, "write_launcher", lambda *args: None)
    monkeypatch.setattr(retention.COMMON, "dispatch_launcher", lambda *args: None)
    monkeypatch.setattr(
        retention.COMMON,
        "wait_launch",
        lambda *args: (
            {"address": "0x1", "pid": 40},
            {"pid": 41},
            0,
            0,
        ),
    )

    def assert_owned_window(app_id: str, address: str) -> None:
        assert app_id == retention.COMMON.APP_IDS["foot"]
        assert address == "0x1"
        calls["ownership"] += 1
        if calls["ownership"] == 3:
            raise retention.COMMON.WindowIsolationError(
                "benchmark window disappeared", isolation_details
            )

    def fail_cleanup() -> None:
        calls["cleanup"] += 1
        raise RuntimeError("workspace did not empty")

    monkeypatch.setattr(retention.COMMON, "assert_owned_window", assert_owned_window)
    monkeypatch.setattr(retention.COMMON, "wait_cleanup", fail_cleanup)
    monkeypatch.setattr(retention, "snapshot_process_forest", lambda roots: object())
    monkeypatch.setattr(
        retention,
        "snapshot_process_memory_forest",
        lambda roots: {"aggregate": {}, "processes": []},
    )

    assert retention.main() == 1
    report = json.loads((output / "foot-retention.json").read_text())
    assert report["valid"] is False
    assert report["failure"] == {
        "phase": "pre_trigger_ownership",
        "type": "WindowIsolationError",
        "message": (
            "benchmark window disappeared: "
            + json.dumps(isolation_details, sort_keys=True)
        ),
        "isolation": isolation_details,
    }
    assert report["cleanup_failure"] == {
        "phase": "cleanup",
        "type": "RuntimeError",
        "message": "workspace did not empty",
    }
    assert calls == {"ownership": 3, "cleanup": 1, "killed": ["0x1"]}


def test_marker_capture_refreshes_stale_launch_geometry_by_owned_address() -> None:
    output = load_module(
        BENCHMARK / "run-graphical-output.py", "graphical_output_geometry"
    )
    expected = {"address": "0x1", "at": [1, 2], "size": [3, 4]}
    clients = [
        {"address": "0x2", "at": [20, 30], "size": [40, 50]},
        {"address": "0x1", "at": [100, -900], "size": [960, 600]},
    ]
    assert output.current_window_geometry(expected, clients) == (100, -900, 960, 600)
    with pytest.raises(RuntimeError, match="disappeared before screenshot"):
        output.current_window_geometry(expected, clients[:1])


def test_visible_marker_detection_survives_inactive_window_composition() -> None:
    module = load_module(
        BENCHMARK / "run-graphical-output.py", "graphical_output_marker"
    )
    assert module.is_visible_marker_pixel(17, 239, 113)
    assert module.is_visible_marker_pixel(14, 189, 90)
    assert module.is_visible_marker_pixel(14, 196, 93)
    assert not module.is_visible_marker_pixel(9, 10, 9)
    assert not module.is_visible_marker_pixel(200, 200, 200)
    assert not module.is_visible_marker_pixel(14, 196, 180)


def test_input_child_records_receipt_before_visible_marker(
    tmp_path: pathlib.Path,
) -> None:
    ready = tmp_path / "ready.json"
    start = tmp_path / "start"
    received = tmp_path / "received.json"
    done = tmp_path / "done.json"
    start.touch()
    result = subprocess.run(
        [
            sys.executable,
            str(BENCHMARK / "workloads/bench-child.py"),
            "input",
            "--ready-file",
            str(ready),
            "--start-file",
            str(start),
            "--received-file",
            str(received),
            "--done-file",
            str(done),
        ],
        input=b"x\n",
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    receipt = json.loads(received.read_text())
    completion = json.loads(done.read_text())
    assert receipt["event"] == "input_received"
    assert receipt["token"] == "x"
    assert receipt["monotonic_ns"] <= completion["monotonic_ns"]
    assert b"SPLINTERBENCH_DONE" in result.stdout
    assert b"\x1b[48;2;17;239;113m" in result.stdout


def test_latency_boundary_is_targeted_and_does_not_require_nested_tools() -> None:
    value = latency_boundary.probe()
    assert value["backend"] == "host-hyprland-targeted-shortcut"
    assert value["input_protocol"] == "Hyprland hl.dsp.send_shortcut targeted window"
    assert value["visible_boundary"] == "host_window_screenshot_polling_approximation"
    assert "wtype" not in value["tools"]
    assert "gamescope" not in value["tools"]
    assert value["tools"]["Pillow"]["available"] is True

    source = (BENCHMARK / "run-graphical-latency.py").read_text()
    assert "hl.dsp.send_shortcut" in source
    assert "window = {selector}" in source
    assert "focused_address() != original_focus" in source


def test_latency_matrix_keeps_input_and_visible_boundaries_separate() -> None:
    module = load_module(BENCHMARK / "run-latency-matrix.py", "latency_matrix")
    record = {
        "terminal": "splinterm",
        "result": {
            "input": {"input_to_child_ns": 10},
            "visible": {"input_to_visible_marker_ns": 30},
        },
    }
    summary = module.summaries([record])
    assert summary["splinterm"]["input_to_child_ns"]["median"] == 10
    assert summary["splinterm"]["input_to_visible_marker_ns"]["median"] == 30
    markdown = module.markdown(summary, 1, 7)
    assert "not compositor presentation" in markdown

    safe = {
        "schema": "splinterm.benchmark.input-latency.v1",
        "terminal": "splinterm",
        "valid": True,
        "notes": [],
        "boundary": {
            "backend": "host-hyprland-targeted-shortcut",
            "width": 960,
            "height": 600,
            "refresh_hz": 60,
            "scale": 1,
            "input_protocol": "Hyprland hl.dsp.send_shortcut targeted window",
            "capture_protocol": "zwlr_screencopy_manager_v1 via grim",
            "targeted_window_verified": True,
        },
        "input": {
            "token": "x",
            "clock": "CLOCK_MONOTONIC shared host namespace",
            "injector_returncode": 0,
            "input_to_child_ns": 10,
        },
        "isolation": {
            "workspace": 8,
            "monitor": "DP-2",
            "no_initial_focus": True,
            "targeted_input_without_focus": True,
            "host_focus_unchanged": True,
            "host_workspace_unchanged": True,
            "cleanup_verified": True,
        },
        "presentation": {
            "status": "not-measured",
            "input_to_compositor_presentation_ns": None,
        },
        "visible": {
            "boundary": "host_window_screenshot_polling_approximation",
            "poll_interval_ms": 10,
            "input_to_visible_marker_ns": 30,
        },
    }
    module.validate_case(safe)
    mutations = (
        ("host_workspace_unchanged", ("isolation", "host_workspace_unchanged"), False),
        ("workspace", ("isolation", "workspace"), 9),
        ("monitor", ("isolation", "monitor"), "DP-1"),
        ("backend", ("boundary", "backend"), "global-input"),
        ("input_protocol", ("boundary", "input_protocol"), "global"),
    )
    for expected, path, value in mutations:
        unsafe = json.loads(json.dumps(safe))
        unsafe[path[0]][path[1]] = value
        with pytest.raises(RuntimeError, match=expected):
            module.validate_case(unsafe)


def test_latency_snapshot_includes_isolated_binary_selection_provenance() -> None:
    path = BENCHMARK / "run-latency-matrix.py"
    spec = importlib.util.spec_from_file_location("latency_snapshot_test", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    assert "tools/benchmark/manifest.py" in module.IMPLEMENTATION_FILES
    assert "tools/benchmark/adapters/splinterm.py" in module.IMPLEMENTATION_FILES


def test_graphical_launchers_apply_permanent_no_focus_rules() -> None:
    launchers = []
    for launcher in (ROOT / "tools").rglob("*.py"):
        if launcher.name.startswith("test_"):
            continue
        source = launcher.read_text(encoding="utf-8")
        if "workspace = '8 silent'" in source and "no_initial_focus = true" in source:
            launchers.append(launcher)
            no_initial_focus = source.count("no_initial_focus = true")
            assert source.count("no_focus = true") >= no_initial_focus, launcher
            assert source.count("opacity = '1 1'") >= no_initial_focus, launcher
    assert len(launchers) >= 7


def test_cava_side_by_side_is_native_tiled_pipewire_and_focus_safe() -> None:
    source = (ROOT / "tools/performance/run-cava-side-by-side.py").read_text(
        encoding="utf-8"
    )
    assert "method = pipewire" in source
    assert "source = auto" in source
    assert '["pgrep", "-x", "cava"]' in source
    assert "ambient Cava processes would pollute the comparison" in source
    assert 'stty cols 120 rows 40' in source
    assert "float = false" in source
    assert "no_initial_focus = true" in source
    assert "no_focus = true" in source
    assert "movewindowpixel" not in source
    assert "produce-audio" not in source
    assert "audio.fifo" not in source


def test_hyprland_056_absolute_window_move_uses_lua_dispatcher(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = BENCHMARK / "run-graphical-idle.py"
    spec = importlib.util.spec_from_file_location("graphical_move_test", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    calls = []

    def run(command, **kwargs):
        calls.append((command, kwargs))
        return subprocess.CompletedProcess(command, 0, "", "")

    monkeypatch.setattr(module.V1, "run", run)
    module.move_window_absolute("0xabc", 650, -1060)
    assert calls == [
        (
            [
                "hyprctl",
                "eval",
                'hl.dispatch(hl.dsp.window.move({ x = 650, y = -1060, relative = false, window = "address:0xabc" }))',
            ],
            {"capture_output": True, "timeout": 5},
        )
    ]
    assert "movewindowpixel" not in calls[0][0][2]


def test_graphical_commands_are_controlled_and_terminal_specific(
    tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    path = BENCHMARK / "run-graphical-idle.py"
    spec = importlib.util.spec_from_file_location("graphical_idle_test", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    foot, foot_environment = module.launch_command(
        "foot", tmp_path, tmp_path / "socket", 30
    )
    assert pathlib.Path(foot[0]).name == "foot"
    assert "com.oldjobobo.splinterbench.Foot" in foot
    assert foot_environment == {}
    monkeypatch.setenv("SPLINTERM_PERF_TRACE_DIR", str(tmp_path / "trace"))
    monkeypatch.setenv("SPLINTERM_PERF_RUN_ID", "graphical-test")
    monkeypatch.setenv("SPLINTERM_PERF_TRACE_MAX_EVENTS", "123")
    splinterm, environment = module.launch_command(
        "splinterm", tmp_path, tmp_path / "socket", 30
    )
    assert pathlib.Path(splinterm[0]).name == "splinterm"
    assert splinterm[1:4] == ["launch", "--new", "--name"]
    assert environment["SPLINTERM_SOCKET"] == str(tmp_path / "socket")
    assert environment["SPLINTERM_CONFIG"].endswith("profiles/splinterm.ini")
    assert environment["SPLINTERM_PERF_TRACE_DIR"] == str(tmp_path / "trace")
    assert environment["SPLINTERM_PERF_RUN_ID"] == "graphical-test"
    assert environment["SPLINTERM_PERF_TRACE_MAX_EVENTS"] == "123"
    isolated_client = tmp_path / "isolated-splinterm"
    isolated_daemon = tmp_path / "isolated-splinterd"
    monkeypatch.setenv("SPLINTERBENCH_SPLINTERM_CLIENT", str(isolated_client))
    monkeypatch.setenv("SPLINTERBENCH_SPLINTERM_DAEMON", str(isolated_daemon))
    overridden, _ = module.launch_command(
        "splinterm", tmp_path, tmp_path / "socket", 30
    )
    assert pathlib.Path(overridden[0]) == isolated_client
    assert module.splinterd_executable() == isolated_daemon
    no_hold, _ = module.launch_command(
        "foot", tmp_path, tmp_path / "socket", 1, hold_window=False
    )
    assert "--hold" not in no_hold
    input_command, _ = module.launch_command(
        "foot", tmp_path, tmp_path / "socket", 1, case="input"
    )
    assert "--received-file" in input_command
    assert str(tmp_path / "input-received.json") in input_command

    kitty, _ = module.launch_command("kitty", tmp_path, tmp_path / "socket", 30)
    assert pathlib.Path(kitty[0]).name == "kitty"
    assert "com.oldjobobo.splinterbench.Kitty" in kitty
    ghostty, _ = module.launch_command("ghostty", tmp_path, tmp_path / "socket", 30)
    assert pathlib.Path(ghostty[0]).name == "ghostty"
    assert any(argument.endswith("profiles/ghostty.conf") for argument in ghostty)
    alacritty, _ = module.launch_command("alacritty", tmp_path, tmp_path / "socket", 30)
    assert pathlib.Path(alacritty[0]).name == "alacritty"
    assert "--hold" in alacritty
    assert alacritty.index("--hold") < alacritty.index("-e")


def test_trigger_gates_workload_output(tmp_path: pathlib.Path) -> None:
    ready = tmp_path / "ready.json"
    start = tmp_path / "start"
    done = tmp_path / "done.json"
    process = subprocess.Popen(
        [
            sys.executable,
            str(BENCHMARK / "workloads/bench-child.py"),
            "plain",
            "--lines",
            "1",
            "--ready-file",
            str(ready),
            "--start-file",
            str(start),
            "--done-file",
            str(done),
        ],
        stdout=subprocess.PIPE,
    )
    for _ in range(100):
        if ready.exists():
            break
        __import__("time").sleep(0.005)
    assert ready.exists() and not done.exists()
    start.touch()
    stdout, _ = process.communicate(timeout=5)
    assert process.returncode == 0
    assert done.exists() and b"SPLINTERBENCH_DONE" in stdout


def test_matrix_summary_preserves_terminal_metrics() -> None:
    path = BENCHMARK / "run-graphical-matrix.py"
    spec = importlib.util.spec_from_file_location("graphical_matrix_test", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    record = {
        "terminal": "foot",
        "iteration": 0,
        "result": {
            "boundaries": {
                "launch_to_child_ready_ns": 10,
                "launch_to_window_map_ns": 20,
            },
            "idle": {"rss_bytes": 30, "cpu_ticks": 1, "context_switches": 2},
        },
    }
    summary = module.summaries([record])
    assert summary["foot"]["launch_to_window_map_ns"]["median"] == 20
    assert summary["foot"]["rss_bytes"]["median"] == 30


def test_statistics_retain_invalid_counts_and_use_nearest_rank() -> None:
    assert summarize_values([1, 2, 3, 100]) == {
        "count": 4,
        "min": 1,
        "median": 2.5,
        "p95": 3.0,
        "max": 100,
        "median_absolute_deviation": 1.0,
    }
    samples = [
        {
            "terminal": "foot",
            "case": "plain",
            "boundary": "pty_write_complete",
            "valid": True,
            "metrics": {"duration_ns": 10},
        },
        {
            "terminal": "foot",
            "case": "plain",
            "boundary": "pty_write_complete",
            "valid": False,
            "metrics": {"duration_ns": 1},
        },
    ]
    groups = summarize_samples(samples)
    assert groups[0]["statistics"]["median"] == 10
    assert groups[0]["invalid_samples"] == 1

    invalid_only = summarize_samples(
        [
            {
                "terminal": "kitty",
                "case": "ansi",
                "boundary": "child_ready",
                "valid": False,
                "metrics": {},
            }
        ]
    )
    assert invalid_only[0]["metric"] is None
    assert invalid_only[0]["statistics"] is None
    assert invalid_only[0]["invalid_samples"] == 1


def test_omarchy_theme_generator_uses_foot_alpha_and_legacy_roles(
    tmp_path: pathlib.Path,
) -> None:
    module = load_module(
        ROOT / "tools/generate-omarchy-theme.py", "generate_omarchy_theme"
    )
    legacy = {
        "accent": "#010203",
        "background": "#101112",
        "foreground": "#d0d1d2",
        "selection_background": "#202122",
        **{f"color{index}": f"#{index:06x}" for index in range(16)},
    }
    generated = module.generate(legacy, 0.85)
    assert generated["alpha"] == 0.85
    assert generated["background"] == legacy["background"]
    assert generated["ansi"][0] == legacy["color0"]
    assert generated["ansi"][15] == legacy["color15"]

    assert module.theme_alpha(tmp_path) == 1.0
    (tmp_path / "foot.ini").write_text(
        "[colors-dark]\nalpha=0.72\n", encoding="utf-8"
    )
    assert module.theme_alpha(tmp_path) == 0.72
    (tmp_path / "foot.ini").write_text(
        "[colors-dark]\nalpha=1.1\n", encoding="utf-8"
    )
    with pytest.raises(ValueError, match="between"):
        module.theme_alpha(tmp_path)


def test_stage_overhead_bootstrap_is_deterministic_and_one_sided() -> None:
    module = load_module(
        ROOT / "tools/performance/run-stage-overhead.py", "stage_overhead"
    )
    first = module.bootstrap_regression([100.0] * 10, [101.0] * 10, 17, 1000)
    second = module.bootstrap_regression([100.0] * 10, [101.0] * 10, 17, 1000)
    assert first == second
    assert first["point_percent"] == pytest.approx(1.0)
    assert first["one_sided_95_upper_percent"] == pytest.approx(1.0)
    assert module.percentile(list(range(1, 11)), 95) == 10


def test_stage_trace_summary_correlates_body_free_revision_records(
    tmp_path: pathlib.Path,
) -> None:
    run_id = "test-run"
    trace = tmp_path / f"{run_id}-10.jsonl"
    common = {
        "schema": "splinterm.performance.stage.v1",
        "run_id": run_id,
        "process": "splinterm",
        "pid": 10,
        "clock": "CLOCK_MONOTONIC_RAW shared host namespace",
        "splint_id": "00000000-0000-0000-0000-000000000001",
        "incarnation": 2,
        "subscription_id": 3,
        "revision": 4,
    }
    trace.write_text(
        "\n".join(
            json.dumps(
                {
                    **common,
                    "sequence": sequence,
                    "monotonic_raw_ns": timestamp,
                    "stage": stage,
                    "duration_ns": duration,
                }
            )
            for sequence, timestamp, stage, duration in (
                (0, 100, "wire_materialize", 10),
                (1, 160, "draw_commit", 20),
            )
        )
        + "\n",
        encoding="utf-8",
    )
    output = tmp_path / "summary.json"
    result = subprocess.run(
        [
            sys.executable,
            str(ROOT / "tools/performance/summarize-stage-trace.py"),
            str(tmp_path),
            str(output),
            "--run-id",
            run_id,
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    summary = json.loads(output.read_text(encoding="utf-8"))
    assert summary["record_count"] == 2
    assert summary["correlated_wire_to_commit"]["count"] == 1
    assert summary["correlated_wire_to_commit"]["duration"]["median_ns"] == 60

    with trace.open("a", encoding="utf-8") as stream:
        stream.write(
            json.dumps(
                {
                    **common,
                    "sequence": 2,
                    "monotonic_raw_ns": 10_000,
                    "stage": "draw_commit",
                    "duration_ns": 20,
                }
            )
            + "\n"
        )
    result = subprocess.run(
        [
            sys.executable,
            str(ROOT / "tools/performance/summarize-stage-trace.py"),
            str(tmp_path),
            str(output),
            "--run-id",
            run_id,
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    ambiguous = json.loads(output.read_text(encoding="utf-8"))
    assert ambiguous["correlated_wire_to_commit"]["count"] == 0

    with trace.open("a", encoding="utf-8") as stream:
        stream.write(
            json.dumps(
                {
                    **common,
                    "sequence": 3,
                    "monotonic_raw_ns": 10_001,
                    "stage": "trace_saturated",
                }
            )
            + "\n"
        )
    saturated = subprocess.run(
        [
            sys.executable,
            str(ROOT / "tools/performance/summarize-stage-trace.py"),
            str(tmp_path),
            str(output),
            "--run-id",
            run_id,
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert saturated.returncode != 0
    assert "trace event bound was exhausted" in saturated.stderr


def test_graphical_cava_progress_counts_distinct_body_free_revisions(
    tmp_path: pathlib.Path,
) -> None:
    module = load_module(
        ROOT / "tools/performance/run-graphical-cava.py", "graphical_cava"
    )
    run_id = "cava-test"
    trace = tmp_path / f"{run_id}-1.jsonl"
    trace.write_text(
        "\n".join(
            json.dumps(
                {
                    "run_id": run_id,
                    "stage": stage,
                    "revision": revision,
                }
            )
            for stage, revision in (
                ("client_apply", 1),
                ("client_apply", 2),
                ("draw_commit", 1),
                ("draw_commit", 2),
                ("draw_commit", 2),
            )
        )
        + "\n",
        encoding="utf-8",
    )
    progress = module.trace_progress(tmp_path, run_id)
    assert progress["records"] == 5
    assert progress["distinct_revisions"] == {"client_apply": 2, "draw_commit": 2}
    assert module.advanced_revision_counts(
        {"client_apply": {1, 2}, "draw_commit": {1, 2}},
        {"client_apply": {1, 2, 3}, "draw_commit": {1, 2, 4}},
    ) == {"client_apply": 1, "draw_commit": 1}


def test_manifest_matches_result_schema(tmp_path: pathlib.Path) -> None:
    jsonschema = pytest.importorskip("jsonschema")
    output = tmp_path / "manifest.json"
    result = subprocess.run(
        [sys.executable, str(BENCHMARK / "run.py"), "manifest", str(output)],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    schema = json.loads((BENCHMARK / "result-schema.json").read_text())
    document = json.loads(output.read_text())
    jsonschema.Draft202012Validator(
        schema, format_checker=jsonschema.FormatChecker()
    ).validate(document)


def _successful_correctness_run(
    command: list[str] | tuple[str, ...],
) -> subprocess.CompletedProcess[str]:
    if list(command) == ["git", "rev-parse", "HEAD"]:
        return subprocess.CompletedProcess(command, 0, "0" * 40 + "\n", "")
    if list(command) == ["git", "status", "--porcelain"]:
        return subprocess.CompletedProcess(command, 0, "", "")
    return subprocess.CompletedProcess(command, 0, "checked\n", "")


def test_semantic_fixture_vectors_are_current() -> None:
    result = subprocess.run(
        [
            sys.executable,
            str(BENCHMARK / "generate-semantic-fixture-vectors.py"),
            "--check",
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert "match 5 canonical Foot fixtures" in result.stdout


def test_correctness_report_is_evidence_bounded_and_schema_valid(
    tmp_path: pathlib.Path,
) -> None:
    jsonschema = pytest.importorskip("jsonschema")
    report = build_report(_successful_correctness_run)

    assert report["valid"] is True
    assert report["oracle"]["commit"] == "3c5b584b0eafa772eb4376fb6eaf6643399e190e"
    assert report["semantic_fixtures"]["fixture_count"] == 5
    assert all(item["exact"] for item in report["final_buffer_evidence"])
    assert report["fuzzing"]["status"] == "available-not-run"
    assert report["fuzzing"]["recorded_duration_seconds"] is None
    assert {item["status"] for item in report["external_observations"]} == {
        "observed"
    }
    assert all(
        "private" in item["claim_limit"]
        or "does not prove" in item["claim_limit"]
        for item in report["external_observations"]
    )
    sixel = next(
        item for item in report["capability_matrix"] if item["capability"] == "sixel"
    )
    assert sixel["statuses"]["splinterm"] == "partial"
    assert sixel["statuses"]["foot"] == "unknown"

    output = tmp_path / "correctness"
    write_report(output, report)
    document = json.loads((output / "report.json").read_text())
    schema = json.loads((BENCHMARK / "correctness-schema.json").read_text())
    jsonschema.Draft202012Validator(schema).validate(document)
    markdown = (output / "README.md").read_text()
    assert "Correctness is reported separately from performance" in markdown
    assert "available-not-run" in markdown


def test_correctness_report_does_not_hide_failed_checks() -> None:
    def failing_run(command: list[str] | tuple[str, ...]) -> subprocess.CompletedProcess[str]:
        if list(command)[:2] == ["git", "rev-parse"]:
            return subprocess.CompletedProcess(command, 0, "1" * 40 + "\n", "")
        if list(command)[:2] == ["git", "status"]:
            return subprocess.CompletedProcess(command, 0, " M file\n", "")
        return subprocess.CompletedProcess(command, 7, "", "failed")

    report = build_report(failing_run)
    assert report["valid"] is False
    assert report["repository"]["dirty"] is True
    assert all(check["status"] == "failed" for check in report["checks"])
    assert all(check["returncode"] == 7 for check in report["checks"])
