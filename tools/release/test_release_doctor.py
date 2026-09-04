from __future__ import annotations

import importlib.util
from pathlib import Path
import shutil
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

    def test_release_doctor_has_no_external_oracle_gate(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        self.assertNotIn("def check_provenance", source)
        self.assertNotIn("check_provenance(root)", source)

    def test_workflow_policy_keeps_foot_out_of_release_authority(self) -> None:
        with tempfile.TemporaryDirectory() as value:
            root = Path(value)
            shutil.copytree(ROOT / ".github", root / ".github")
            self.assertEqual(MODULE.check_workflows(root), [])

            ci = root / ".github/workflows/ci.yml"
            ci.write_text(
                ci.read_text(encoding="utf-8").replace(
                    "  foot-reference:\n    name: Optional historical Foot differential tooling\n    needs: preflight\n    continue-on-error: true",
                    "  foot-reference:\n    name: Optional historical Foot differential tooling\n    needs: preflight",
                ),
                encoding="utf-8",
            )
            self.assertIn("not advisory", MODULE.check_workflows(root)[0])

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
