"""Non-graphical tests for the real-client image harness."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
RUNNER_PATH = HERE / "run_real_client_images.py"


def load_runner():
    spec = importlib.util.spec_from_file_location("real_client_images", RUNNER_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


RUNNER = load_runner()


def test_fixture_and_presentation_are_deterministic_and_bounded() -> None:
    assert RUNNER.fixture_dimensions() == (320, 180)
    assert RUNNER.SHARED.sha256(RUNNER.FIXTURE_IMAGE) == (
        "496e2100118dc3ed899cface6b2c02adfe8d8b61ddf8bd4c04f3964b37c804da"
    )
    assert RUNNER.FIXTURE_IMAGE.stat().st_size < 64 * 1024
    presentation = RUNNER.PRESENTATION.read_text(encoding="utf-8")
    assert "real-client-static.png" in presentation
    assert "image:width:50%" in presentation


def test_case_catalogue_covers_apps_and_only_advertised_static_protocols() -> None:
    cases = RUNNER.case_specs()
    assert {spec["app"] for spec in cases.values()} == {
        "chafa",
        "kitten",
        "presenterm",
        "timg",
        "yazi",
    }
    assert {spec["protocol"] for spec in cases.values()} == {
        "auto",
        "iterm2",
        "kitty",
        "sixel",
    }
    commands = [argument for spec in cases.values() for argument in spec["command"]]
    assert "kitty-local" not in commands
    assert "iterm2-multipart" not in commands
    assert "--transfer-mode=stream" in cases["icat-kitty-smoke"]["command"]
    assert "--frames=1" in cases["timg-kitty"]["command"]
    assert "--animate" in cases["chafa-sixel"]["command"]


def test_plans_are_small_ordered_and_separate_warmups() -> None:
    assert [item["case"] for item in RUNNER.build_plan("smoke")] == ["icat-kitty-smoke"]
    compatibility = RUNNER.build_plan("compatibility")
    assert len(compatibility) == 4
    assert all(not item["warmup"] for item in compatibility)
    benchmark = RUNNER.build_plan("benchmark", samples=3)
    assert len(benchmark) == 12
    for case_name in RUNNER.BENCHMARK_CASES:
        selected = [item for item in benchmark if item["case"] == case_name]
        assert selected[0] == {"case": case_name, "sample": 0, "warmup": True}
        assert [item["sample"] for item in selected[1:]] == [1, 2, 3]


def test_child_command_preserves_argument_boundaries_without_a_shell(
    tmp_path: Path,
) -> None:
    trigger = tmp_path / "trigger"
    status = tmp_path / "status.json"
    command = ["/usr/bin/example", "argument with spaces", "$(not-a-shell)"]
    child = RUNNER.child_command(trigger, status, command, tmp_path)
    assert child[:2] == [sys.executable, "-c"]
    assert "subprocess.Popen(command" in child[2]
    assert "shell=True" not in child[2]
    assert repr(command) in child[2]


def test_trace_and_measurement_summaries_keep_boundaries_separate() -> None:
    trace = RUNNER.parse_trace(
        "phase5-image-trace decode_ns=20 content_bytes=100 content_count=1 placement_count=1\n",
        "phase5-image-trace composition_ns=40 image_count=1\n",
    )
    assert trace == {
        "decode_ns": [20],
        "composition_ns": [40],
        "content": [{"bytes": 100, "contents": 1, "placements": 1}],
        "image_rejected": False,
    }
    reports = [
        {
            "valid": True,
            "latency_ns": {
                "trigger_to_composed_capture": value,
                "decode_samples": [value // 10],
                "composition_samples": [value // 5],
            },
            "resources": {
                "daemon": {"rss_bytes": value * 2},
                "client": {"rss_bytes": value * 3},
            },
        }
        for value in (100, 120, 140)
    ]
    summary = RUNNER.measurement_summary(reports)
    assert summary["samples"] == 3
    assert summary["trigger_to_composed_capture_ns"] == {
        "samples": 3,
        "median": 120,
        "minimum": 100,
        "maximum": 140,
        "spread": pytest.approx(1 / 3),
    }
    assert summary["decode_ns"]["median"] == 12
    assert summary["composition_ns"]["median"] == 24


def test_cli_dry_run_never_requires_hyprland_or_output_directory() -> None:
    completed = subprocess.run(
        [sys.executable, str(RUNNER_PATH), "--benchmark", "--dry-run"],
        check=False,
        capture_output=True,
        text=True,
        env={},
    )
    assert completed.returncode == 0, completed.stderr
    payload = json.loads(completed.stdout)
    assert payload["mode"] == "benchmark"
    assert len(payload["plan"]) == 12
