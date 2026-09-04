import importlib.util
import subprocess
import sys
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/image-spike/capture_foot_sixel.py"


def load_capture():
    name = "capture_foot_sixel"
    spec = importlib.util.spec_from_file_location(name, SCRIPT)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def test_sixel_provenance_requires_strict_schema_v4_host_check(monkeypatch):
    capture = load_capture()

    def reject():
        raise RuntimeError("environment variable drifted: FONTCONFIG_FILE")

    monkeypatch.setattr(capture, "require_pinned_host", reject)
    with pytest.raises(RuntimeError, match="environment variable drifted"):
        capture.sixel_provenance()


def test_strict_host_check_runs_shared_provenance_validator(monkeypatch):
    capture = load_capture()
    calls = []

    def run(command, **kwargs):
        calls.append((command, kwargs))
        return subprocess.CompletedProcess(command, 1, "", "raster options drifted")

    monkeypatch.setattr(capture.subprocess, "run", run)
    with pytest.raises(RuntimeError, match="raster options drifted"):
        capture.require_pinned_host()
    assert calls == [
        (
            [sys.executable, str(capture.PROVENANCE_CHECK)],
            {
                "cwd": ROOT,
                "text": True,
                "capture_output": True,
                "check": False,
            },
        )
    ]
