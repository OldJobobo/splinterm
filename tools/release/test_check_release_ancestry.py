from __future__ import annotations

import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/release/check-release-ancestry.py"
SPEC = importlib.util.spec_from_file_location("check_release_ancestry", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def git(root: Path, *arguments: str) -> str:
    return subprocess.run(
        [
            "git", "-c", "user.name=test", "-c", "user.email=test@example.invalid",
            "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false", *arguments,
        ],
        cwd=root, text=True, capture_output=True, check=True,
    ).stdout.strip()


def commit(root: Path, message: str) -> None:
    git(root, "commit", "--quiet", "--allow-empty", "-m", message)


class ReleaseAncestryTests(unittest.TestCase):
    def test_merged_maintenance_releases_pass_and_unmerged_ones_are_named(self) -> None:
        with tempfile.TemporaryDirectory(prefix="splinterm-ancestry-") as value:
            root = Path(value)
            git(root, "init", "--quiet", "--initial-branch=main")
            commit(root, "base")
            git(root, "tag", "v0.1.0")
            git(root, "tag", "not-a-release")
            git(root, "branch", "maint")
            commit(root, "main work")
            self.assertEqual(MODULE.missing_release_tags(root), [])

            git(root, "checkout", "--quiet", "maint")
            commit(root, "maintenance fix")
            git(root, "tag", "v0.1.1")
            git(root, "tag", "vnext")
            git(root, "checkout", "--quiet", "main")
            self.assertEqual(MODULE.missing_release_tags(root), ["v0.1.1"])

            # A cherry-pick or squash carries the change but not the ancestry.
            commit(root, "maintenance fix")
            self.assertEqual(MODULE.missing_release_tags(root), ["v0.1.1"])

            git(root, "merge", "--quiet", "--no-ff", "-m", "merge maint", "maint")
            self.assertEqual(MODULE.missing_release_tags(root), [])

    def test_a_checkout_without_release_tags_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory(prefix="splinterm-ancestry-") as value:
            root = Path(value)
            git(root, "init", "--quiet", "--initial-branch=main")
            commit(root, "base")
            with self.assertRaisesRegex(ValueError, "no release tags"):
                MODULE.missing_release_tags(root)

    def test_main_ci_runs_the_check_with_full_history(self) -> None:
        workflow = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        preflight = workflow.split("\n  preflight:\n", 1)[1].split("\n  static:\n", 1)[0]
        self.assertIn("fetch-depth: 0", preflight)
        self.assertIn("python tools/release/check-release-ancestry.py", preflight)
        self.assertIn(
            "if: github.ref == 'refs/heads/main' || github.base_ref == 'main'", preflight
        )
        self.assertNotIn("continue-on-error", preflight)
        self.assertIn("tools/release/test_check_release_ancestry.py", workflow)


if __name__ == "__main__":
    unittest.main()
