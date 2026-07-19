import importlib.util
import subprocess
from pathlib import Path

PATH = Path(__file__).with_name("run-slice3-final-buffer-comparison.py")
SPEC = importlib.util.spec_from_file_location("slice3_runner", PATH)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def monitor(scale=1.0):
    return {
        "name": "DP-2",
        "width": 1920,
        "height": 1080,
        "refreshRate": 60.0,
        "x": 640,
        "y": -1080,
        "scale": scale,
        "transform": 0,
    }


def test_monitor_expression_preserves_mode_position_and_transform():
    expression = MODULE.monitor_expression(monitor(), 1.25)
    assert 'output = "DP-2"' in expression
    assert 'mode = "1920x1080@60.00000"' in expression
    assert 'position = "640x-1080"' in expression
    assert "scale = 1.25" in expression
    assert "transform = 0" in expression


def test_apply_scale_rechecks_isolation_and_focus(monkeypatch):
    calls = []
    monkeypatch.setattr(
        MODULE.V1,
        "run",
        lambda command, **_kwargs: subprocess.CompletedProcess(command, 0, "ok", ""),
    )
    monkeypatch.setattr(MODULE, "monitor_state", lambda: monitor(1.5))
    monkeypatch.setattr(MODULE.V1, "assert_test_workspace_isolated", lambda: calls.append("isolated"))
    monkeypatch.setattr(MODULE.V1, "assert_user_workspace_untouched", lambda: calls.append("focus"))
    MODULE.apply_monitor_scale(monitor(), 180)
    assert calls == ["isolated", "focus"]
