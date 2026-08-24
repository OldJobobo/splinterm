from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/release/release-doctor.py"
SPEC = importlib.util.spec_from_file_location("release_doctor", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ReleaseDoctorTests(unittest.TestCase):
    def test_checked_in_release_state_is_consistent(self) -> None:
        self.assertEqual(MODULE.diagnose(ROOT, generated=False), [])

    def test_versions_detect_split_recipe_drift(self) -> None:
        with tempfile.TemporaryDirectory() as value:
            root = Path(value)
            (root / "Cargo.toml").write_text(
                '[workspace.package]\nversion = "1.2.3-alpha4"\n', encoding="utf-8"
            )
            for relative, version in (
                ("packaging/PKGBUILD", "1.2.3alpha4"),
                ("packaging/aur/PKGBUILD", "1.2.3alpha4\n_upstream_ver=1.2.3-alpha4"),
                ("packaging/aur-bin/PKGBUILD", "1.2.3alpha3"),
            ):
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(f"pkgver={version}\n", encoding="utf-8")
            self.assertTrue(any("aur-bin" in error for error in MODULE.check_versions(root)))

    def test_provenance_requires_all_duplicate_lock_identities(self) -> None:
        with tempfile.TemporaryDirectory() as value:
            root = Path(value)
            (root / "tools/foot-oracle/patches").mkdir(parents=True)
            lockfile = root / "Cargo.lock"
            lockfile.write_text("lock\n", encoding="utf-8")
            digest = MODULE.sha256(lockfile)
            manifest = {
                "reference": {"commit": "3c5b584b0eafa772eb4376fb6eaf6643399e190e"},
                "rust": {"cargo_lock_sha256": digest},
                "oracle": {"patches": []},
            }
            path = root / "tools/foot-oracle/provenance.json"
            path.write_text(json.dumps(manifest), encoding="utf-8")
            self.assertIn("duplicate", MODULE.check_provenance(root)[0])
            manifest["default_final_buffer_profile"] = {
                "cargo_lock_sha256": "0" * 64
            }
            path.write_text(json.dumps(manifest), encoding="utf-8")
            self.assertIn("update-provenance.py", MODULE.check_provenance(root)[0])

    def test_markdown_rejects_missing_links_and_private_paths(self) -> None:
        with tempfile.TemporaryDirectory() as value:
            root = Path(value)
            (root / "doc.md").write_text(
                "[missing](missing.md) splinterm-brain\n", encoding="utf-8"
            )
            errors = MODULE.check_markdown(root)
            self.assertTrue(any("missing local link" in error for error in errors))
            self.assertTrue(any("prohibited private path" in error for error in errors))

    def test_generated_srcinfo_is_compared_when_makepkg_exists(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        self.assertIn('shutil.which("makepkg")', source)
        self.assertIn('["makepkg", "--printsrcinfo"]', source)


if __name__ == "__main__":
    unittest.main()
