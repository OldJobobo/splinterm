"""Non-graphical tests for the guarded tmux image stack matrix."""

from __future__ import annotations

import importlib.util
import inspect
import json
import subprocess
import sys
from pathlib import Path

import pytest
from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent
RUNNER_PATH = HERE / "run_tmux_image_matrix.py"


def load_runner():
    spec = importlib.util.spec_from_file_location("tmux_image_matrix", RUNNER_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


RUNNER = load_runner()


def test_stack_catalogue_is_pairwise_and_protocol_explicit() -> None:
    assert RUNNER.STACKS == (
        "splinterm-sixel",
        "splinterm-kitty",
        "foot-tmux-sixel",
        "kitty-tmux-kitty",
    )
    assert RUNNER.PROTOCOLS == {
        "splinterm-sixel": "sixel",
        "splinterm-kitty": "kitty",
        "foot-tmux-sixel": "sixel",
        "kitty-tmux-kitty": "kitty",
    }


def test_plan_is_restrained_and_balanced() -> None:
    plan = RUNNER.build_plan(samples=3)
    compatibility = [item for item in plan if item["kind"] == "compatibility"]
    benchmark = [item for item in plan if item["kind"] == "benchmark"]
    assert len(plan) == 28
    assert len(compatibility) == 12
    assert len(benchmark) == 16
    assert {item["fixture"] for item in compatibility} == {
        "synthetic",
        "ui-detail",
        "alpha",
    }
    for stack in RUNNER.STACKS:
        selected = [item for item in benchmark if item["stack"] == stack]
        assert sum(item["warmup"] for item in selected) == 1
        assert sorted(item["sample"] for item in selected if not item["warmup"]) == [
            1,
            2,
            3,
        ]
        assert {item["fixture"] for item in selected} == {"photo"}


def test_chafa_commands_hold_client_geometry_and_passthrough_boundaries(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(RUNNER.shutil, "which", lambda name: f"/usr/bin/{name}")
    image = Path("/tmp/image with spaces.png")
    for stack in RUNNER.STACKS:
        command = RUNNER.chafa_command(stack, image)
        assert command[0] == "/usr/bin/chafa"
        assert command[command.index("--size") + 1] == "40x20"
        assert command[command.index("--view-size") + 1] == "80x24"
        expected = "sixels" if RUNNER.PROTOCOLS[stack] == "sixel" else "kitty"
        assert command[command.index("--format") + 1] == expected
        if "tmux" in stack:
            assert command[command.index("--passthrough") + 1] == "tmux"
        else:
            assert "--passthrough" not in command
        assert command[-1] == str(image)


def test_child_command_is_shell_free_and_preserves_arguments(tmp_path: Path) -> None:
    command = ["/usr/bin/chafa", "argument with spaces", "$(not-a-shell)"]
    child = RUNNER.child_command(
        tmp_path / "ready.json",
        tmp_path / "trigger",
        tmp_path / "status.json",
        command,
    )
    assert child[:2] == [sys.executable, "-c"]
    assert repr(command) in child[2]
    assert "subprocess.Popen(command)" in child[2]
    assert "shell=True" not in child[2]


def test_alpha_input_requires_full_and_partial_transparency(tmp_path: Path) -> None:
    image = Image.new("RGBA", (3, 1))
    image.putdata([(255, 255, 255, 0), (255, 255, 255, 128), (255, 255, 255, 255)])
    path = tmp_path / "alpha.png"
    image.save(path)
    metadata = RUNNER.image_metadata(path)
    assert metadata["alpha"] == {
        "minimum": 0,
        "maximum": 255,
        "transparent_pixels": 2,
        "partial_alpha_pixels": 1,
    }


def test_changed_image_summary_checks_real_pixel_change() -> None:
    import io

    before = Image.new("RGB", (120, 80), "black")
    after = before.copy()
    draw = ImageDraw.Draw(after)
    draw.rectangle((5, 5, 35, 70), fill=(240, 20, 20))
    draw.rectangle((40, 5, 75, 70), fill=(20, 220, 20))
    draw.rectangle((80, 5, 115, 70), fill=(20, 40, 240))
    for y in range(10, 70):
        draw.line((5, y, 115, y), fill=(y * 3, 255 - y * 2, y * 2))
    first = io.BytesIO()
    second = io.BytesIO()
    before.save(first, format="PNG")
    after.save(second, format="PNG")
    summary = RUNNER.changed_image_summary(
        first.getvalue(), second.getvalue(), "synthetic"
    )
    assert summary["changed_pixels"] > 1000
    assert summary["distinct_colors"] >= 32
    assert min(summary["sampled_primary_pixels"].values()) >= 10


def test_stable_capture_uses_measured_window_lookup() -> None:
    source = inspect.getsource(RUNNER.wait_for_stable_capture)
    assert "MEASURED.window_by_address" in source
    assert "SMOKE.window_by_address" not in source


def test_benchmark_summary_keeps_external_boundaries_separate() -> None:
    reports = []
    for value in (100, 120, 140):
        reports.append(
            {
                "valid": True,
                "stack": "splinterm-sixel",
                "kind": "benchmark",
                "warmup": False,
                "capture": {
                    "trigger_to_first_change_ns": value,
                    "trigger_to_stable_ns": value * 2,
                    "application": {"runtime_ns": value // 2},
                },
                "resources": {
                    "after": {
                        "infrastructure": {"rss_bytes": value * 3},
                        "total": {"rss_bytes": value * 4},
                    }
                },
            }
        )
    summary = RUNNER.benchmark_summary(reports, "splinterm-sixel")
    assert summary["samples"] == 3
    assert summary["trigger_to_first_change_ns"]["median"] == 120
    assert summary["trigger_to_stable_ns"]["median"] == 240
    assert summary["application_runtime_ns"]["median"] == 60


def test_cli_dry_run_needs_no_graphical_session_or_private_inputs() -> None:
    completed = subprocess.run(
        [sys.executable, str(RUNNER_PATH), "--all", "--dry-run"],
        text=True,
        capture_output=True,
        env={},
        check=False,
    )
    assert completed.returncode == 0, completed.stderr
    payload = json.loads(completed.stdout)
    assert payload["mode"] == "all"
    assert len(payload["plan"]) == 28
