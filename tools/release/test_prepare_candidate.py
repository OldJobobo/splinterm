from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/release/prepare-candidate.py"
SPEC = importlib.util.spec_from_file_location("prepare_candidate", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class PrepareCandidateTests(unittest.TestCase):
    def test_checked_in_versions_and_package_layout_are_consistent(self) -> None:
        version = MODULE.workspace_version()
        self.assertEqual(MODULE.validate_versions(version), version.replace("-", ""))

    def test_website_only_archive_exclusions_are_declared(self) -> None:
        attributes = (ROOT / ".gitattributes").read_text(encoding="utf-8")
        self.assertIn("/site/ export-ignore", attributes)
        self.assertIn("/.github/workflows/site.yml export-ignore", attributes)
        self.assertNotIn("Cargo.toml export-ignore", attributes)

    def test_checksum_replacement_is_exact(self) -> None:
        original = "pkgname=x\nsha256sums=(\n  '" + "a" * 64 + "'\n)\n"
        replaced = MODULE.replace_checksums(original, ["b" * 64, "c" * 64])
        self.assertNotIn("a" * 64, replaced)
        self.assertEqual(replaced.count("b" * 64), 1)
        self.assertEqual(replaced.count("c" * 64), 1)

    def test_binary_draft_urls_target_the_versioned_release(self) -> None:
        original = (
            'source=(\n  "https://example/releases/download/v1.2.3-alpha4/a"\n'
            '  "https://example/releases/download/v1.2.3-alpha4/b"\n)\n'
        )
        validated = MODULE.validate_binary_release_urls(original, "1.2.3-alpha4")
        self.assertEqual(validated, original)
        with self.assertRaisesRegex(ValueError, "versioned release URLs"):
            MODULE.validate_binary_release_urls(original, "1.2.3-alpha5")
        with self.assertRaisesRegex(ValueError, "versioned release URLs"):
            MODULE.validate_binary_release_urls(
                original.replace("v1.2.3-alpha4", "edge-$_commit"),
                "1.2.3-alpha4",
            )

    def test_archive_does_not_read_worktree_attributes(self) -> None:
        with tempfile.TemporaryDirectory(prefix="splinterm-archive-attrs-") as value:
            repository = Path(value)
            subprocess.run(["git", "init", "-q"], cwd=repository, check=True)
            subprocess.run(
                ["git", "config", "user.name", "Archive Test"],
                cwd=repository,
                check=True,
            )
            subprocess.run(
                ["git", "config", "user.email", "archive@example.invalid"],
                cwd=repository,
                check=True,
            )
            (repository / ".gitattributes").write_text(
                "/ignored/ export-ignore\n", encoding="utf-8"
            )
            (repository / "ignored").mkdir()
            (repository / "ignored/input.txt").write_text("ignored", encoding="utf-8")
            (repository / "kept.txt").write_text("kept", encoding="utf-8")
            subprocess.run(["git", "add", "."], cwd=repository, check=True)
            subprocess.run(["git", "commit", "-qm", "fixture"], cwd=repository, check=True)
            commit = subprocess.run(
                ["git", "rev-parse", "HEAD"],
                cwd=repository,
                text=True,
                capture_output=True,
                check=True,
            ).stdout.strip()
            (repository / ".gitattributes").write_text("/kept.txt export-ignore\n", encoding="utf-8")
            archive = repository / "fixture.tar.gz"
            subprocess.run(
                ["git", "archive", "--format=tar.gz", f"--output={archive}", commit],
                cwd=repository,
                check=True,
            )
            listing = subprocess.run(
                ["tar", "-tzf", archive],
                text=True,
                capture_output=True,
                check=True,
            ).stdout.splitlines()
        self.assertIn("kept.txt", listing)
        self.assertFalse(any(entry.startswith("ignored/") for entry in listing))
        self.assertNotIn("--worktree-attributes", SCRIPT.read_text(encoding="utf-8"))

    def test_candidate_manifest_contract_is_closed_and_non_publishing(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        self.assertIn('"state": "candidate"', source)
        self.assertIn('"publishable": False', source)
        self.assertIn('"workflow_run": arguments.workflow_run', source)
        self.assertNotIn("gh release", source)
        self.assertNotIn("git push", source)

    def test_existing_version_tag_is_rejected_before_output(self) -> None:
        version = MODULE.workspace_version()
        tag = f"v{version}"
        tags = MODULE.run(["git", "tag", "--list", tag])
        if not tags:
            self.skipTest(f"repository does not contain historical tag {tag}")
        commit = MODULE.run(["git", "rev-parse", "HEAD"])
        with self.assertRaisesRegex(ValueError, "release tag already exists"):
            MODULE.validate_candidate("OldJobobo/splinterm", commit, version)

    def test_mismatched_version_fails_before_build(self) -> None:
        commit = MODULE.run(["git", "rev-parse", "HEAD"])
        with self.assertRaisesRegex(ValueError, "does not match Cargo.toml"):
            MODULE.validate_candidate("OldJobobo/splinterm", commit, "9.9.9-alpha9")

    def test_manifest_example_is_json_serializable(self) -> None:
        manifest = {
            "schema": MODULE.SCHEMA,
            "state": "candidate",
            "publishable": False,
            "assets": [],
        }
        self.assertFalse(json.loads(json.dumps(manifest))["publishable"])


if __name__ == "__main__":
    unittest.main()
