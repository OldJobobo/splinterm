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


def test_fontconfig_raster_option_drift_is_rejected(monkeypatch):
    checker = load_checker()
    manifest = checker.load_manifest()
    font = manifest["fonts"][0]
    monkeypatch.setattr(
        checker,
        "command_output",
        lambda _arguments: f"{font['file']}\n{font['index']}\n3\nTrue\nTrue\n\n1",
    )
    with pytest.raises(checker.ProvenanceError, match="Fontconfig raster options drifted"):
        checker.check_fonts({"fonts": [font]})


def test_matrix_patterns_match_size_qualified_fallback_resolvers():
    checker = load_checker()
    manifest = {
        "oracle": {
            "font_matrix": {
                "logical_sizes_px": [12],
                "scales_120": [150],
            }
        }
    }
    assert checker.matrix_font_patterns(
        manifest,
        {
            "role": "cjk",
            "pattern": "Noto Sans CJK JP:style=Regular",
        },
    ) == ["Noto Sans CJK JP:style=Regular", "Noto Sans CJK JP:pixelsize=15"]
    assert checker.matrix_font_patterns(
        manifest,
        {"role": "emoji", "pattern": "Noto Color Emoji"},
    ) == ["Noto Color Emoji", "Noto Color Emoji:pixelsize=15"]


def test_size_qualified_fontconfig_raster_drift_is_rejected(monkeypatch, tmp_path):
    checker = load_checker()
    font = dict(checker.load_manifest()["fonts"][0])
    font_path = tmp_path / "font.ttf"
    font_path.write_bytes(b"pinned font")
    font["file"] = str(font_path)
    seen_patterns = []

    def output(arguments):
        pattern = arguments[-1]
        seen_patterns.append(pattern)
        raster = dict(font["fontconfig_raster"])
        if pattern.endswith(":pixelsize=7.5"):
            raster["hintstyle"] = "999"
        return "\n".join(
            [
                font["file"],
                str(font["index"]),
                raster["hintstyle"],
                raster["hinting"],
                raster["antialias"],
                raster["rgba"],
                raster["lcdfilter"],
            ]
        )

    monkeypatch.setattr(checker, "command_output", output)
    monkeypatch.setattr(checker, "sha256", lambda _path: font["sha256"])
    manifest = {
        "fonts": [font],
        "oracle": {
            "font_matrix": {
                "logical_sizes_px": [6],
                "scales_120": [120, 150],
            }
        },
    }
    with pytest.raises(
        checker.ProvenanceError,
        match=r"Fontconfig raster options drifted.*pixelsize=7\.5",
    ):
        checker.check_fonts(manifest)
    assert seen_patterns == [
        font["pattern"],
        f"{font['pattern']}:pixelsize=6",
        f"{font['pattern']}:pixelsize=7.5",
    ]


def test_non_reference_worker_is_an_explicit_unsupported_host(monkeypatch):
    checker = load_checker()
    manifest = checker.load_manifest()
    monkeypatch.setattr(checker, "os_release_id", lambda: "ubuntu")
    monkeypatch.setattr(checker.platform, "machine", lambda: "x86_64")
    with pytest.raises(checker.UnsupportedHost, match="requires omarchy/x86_64"):
        checker.require_supported_host(manifest)
