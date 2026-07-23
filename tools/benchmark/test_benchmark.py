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
from metrics import (  # noqa: E402
    process_tree,
    read_cgroup_v2,
    snapshot_process_forest,
    snapshot_process_tree,
)
from summary import summarize_samples, summarize_values  # noqa: E402


class ExampleAdapter(TerminalAdapter):
    name = "foot"
    executable_names = ()
    version_arguments = ("--version",)

    def __init__(self, executable: pathlib.Path):
        self.executable = executable

    def candidates(self, root: pathlib.Path):
        del root
        return (self.executable,)


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


def test_graphical_commands_are_controlled_and_terminal_specific(
    tmp_path: pathlib.Path,
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
    splinterm, environment = module.launch_command(
        "splinterm", tmp_path, tmp_path / "socket", 30
    )
    assert pathlib.Path(splinterm[0]).name == "splinterm"
    assert splinterm[1:4] == ["launch", "--new", "--name"]
    assert environment["SPLINTERM_SOCKET"] == str(tmp_path / "socket")
    assert environment["SPLINTERM_CONFIG"].endswith("profiles/splinterm.ini")

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
