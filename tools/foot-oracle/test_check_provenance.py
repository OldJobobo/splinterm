import importlib.util
import subprocess
import sys
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/foot-oracle/check-provenance.py"


def load_checker():
    spec = importlib.util.spec_from_file_location("check_provenance", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def test_portable_provenance_accepts_repository_owned_inputs():
    result = subprocess.run(
        [sys.executable, str(SCRIPT), "--portable"],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert "portable metadata valid" in result.stdout


def test_manifest_declares_complete_faces_and_review_policy():
    checker = load_checker()
    manifest = checker.load_manifest()
    assert {font["role"] for font in manifest["fonts"]} == {
        "regular",
        "bold",
        "italic",
        "bold-italic",
        "cjk",
        "emoji",
    }
    assert manifest["reference_update_policy"]["silent_regeneration"] is False
    assert len(manifest["reference_update_policy"]["required_review"]) >= 4


def test_non_reference_worker_is_an_explicit_unsupported_host(monkeypatch):
    checker = load_checker()
    manifest = checker.load_manifest()
    monkeypatch.setattr(checker, "os_release_id", lambda: "ubuntu")
    monkeypatch.setattr(checker.platform, "machine", lambda: "x86_64")
    with pytest.raises(checker.UnsupportedHost, match="requires omarchy/x86_64"):
        checker.require_supported_host(manifest)
