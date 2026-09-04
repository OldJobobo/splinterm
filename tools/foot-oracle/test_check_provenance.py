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
    assert "portable inputs valid" in result.stdout


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


def test_patch_drift_remains_rejected(monkeypatch):
    checker = load_checker()
    manifest = checker.load_manifest()
    monkeypatch.setattr(checker, "sha256", lambda _path: "0" * 64)
    with pytest.raises(checker.ProvenanceError, match="oracle patch drifted"):
        checker.check_repository_files(manifest)


def test_host_checks_ignore_rust_and_ambient_fontconfig_inventory(monkeypatch):
    checker = load_checker()
    manifest = checker.load_manifest()
    commands = []

    def output(arguments):
        commands.append(arguments)
        return {
            "fcft": manifest["build"]["fcft_version"],
            "freetype2": manifest["build"]["freetype_version"],
            "fontconfig": manifest["build"]["fontconfig_version"],
            "pixman-1": manifest["build"]["pixman_version"],
        }[arguments[-1]]

    monkeypatch.setattr(checker, "command_output", output)
    checker.check_versions(manifest)
    assert commands == [
        ["pkg-config", "--modversion", "fcft"],
        ["pkg-config", "--modversion", "freetype2"],
        ["pkg-config", "--modversion", "fontconfig"],
        ["pkg-config", "--modversion", "pixman-1"],
    ]
    assert "rust" not in manifest
    assert "fontconfig_active_config_sha256" not in manifest["environment"]


def test_non_reference_worker_is_an_explicit_unsupported_host(monkeypatch):
    checker = load_checker()
    manifest = checker.load_manifest()
    monkeypatch.setattr(checker, "os_release_id", lambda: "ubuntu")
    monkeypatch.setattr(checker.platform, "machine", lambda: "x86_64")
    with pytest.raises(checker.UnsupportedHost, match="requires omarchy/x86_64"):
        checker.require_supported_host(manifest)
