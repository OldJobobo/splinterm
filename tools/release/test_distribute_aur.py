from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/release/distribute-aur.py"
SPEC = importlib.util.spec_from_file_location("distribute_aur", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)

REPOSITORY = "OldJobobo/splinterm"
COMMIT = "a" * 40
DIGEST = "b" * 64


def promotion() -> dict:
    return {
        "schema": 1,
        "repository": REPOSITORY,
        "candidate_run_id": 12,
        "candidate_manifest_sha256": "c" * 64,
        "commit": COMMIT,
        "version": "1.2.3-alpha4",
        "tag": "v1.2.3-alpha4",
        "ci": {"run_id": 34},
        "release_title": "Splinterm 1.2.3-alpha4",
        "release_prerelease": True,
        "release_notes": "RELEASE-NOTES.md",
        "release_notes_sha256": "d" * 64,
        "public_assets": {"asset.tar.gz": "e" * 64},
    }


def receipt(value: dict) -> dict:
    return {
        "schema": 1,
        "state": "published",
        "repository": value["repository"],
        "candidate_run_id": value["candidate_run_id"],
        "candidate_manifest_sha256": value["candidate_manifest_sha256"],
        "commit": value["commit"],
        "version": value["version"],
        "tag": value["tag"],
        "ci": value["ci"],
        "release_title": value["release_title"],
        "release_prerelease": value["release_prerelease"],
        "release_notes_sha256": value["release_notes_sha256"],
        "release_url": "https://github.com/OldJobobo/splinterm/releases/tag/v1.2.3-alpha4",
        "workflow_run": "https://github.com/OldJobobo/splinterm/actions/runs/56",
        "assets": [{"asset": "asset.tar.gz", "sha256": "e" * 64}],
        "promotion_run_id": 56,
        "promotion_record_sha256": "f" * 64,
    }


def write_recipe(directory: Path, version: str, marker: str = "same") -> None:
    directory.mkdir()
    (directory / "PKGBUILD").write_text(f"pkgver={version}\n# {marker}\n")
    (directory / ".SRCINFO").write_text(f"pkgver = {version}\n# {marker}\n")
    (directory / "splinterm.install").write_text(f"# {marker}\n")


class DistributeAurTests(unittest.TestCase):
    def test_publication_receipt_is_bound_to_exact_candidate_and_assets(self) -> None:
        expected = promotion()
        publication = receipt(expected)
        MODULE.validate_publication_receipt(publication, expected)
        publication["commit"] = "9" * 40
        with self.assertRaisesRegex(ValueError, "commit"):
            MODULE.validate_publication_receipt(publication, expected)
        publication = receipt(expected)
        publication["assets"].append({"asset": "extra", "sha256": "1" * 64})
        with self.assertRaisesRegex(ValueError, "differ"):
            MODULE.validate_publication_receipt(publication, expected)

    def test_live_publication_reverification_accepts_rest_shape_only_when_exact(self) -> None:
        with tempfile.TemporaryDirectory(prefix="splinterm-public-") as value:
            downloads = Path(value)
            asset = downloads / "asset.tar.gz"
            asset.write_bytes(b"exact release bytes")
            expected = promotion()
            expected["public_assets"][asset.name] = MODULE.PROMOTE.sha256(asset)
            publication = receipt(expected)
            publication["assets"] = [
                {"asset": asset.name, "sha256": MODULE.PROMOTE.sha256(asset)}
            ]
            release = {
                "tag_name": expected["tag"],
                "name": expected["release_title"],
                "body": "release notes\n",
                "draft": False,
                "prerelease": True,
                "html_url": publication["release_url"],
                "url": "https://api.github.com/repos/OldJobobo/splinterm/releases/1",
                "assets": [{"name": asset.name}],
            }
            expected["release_notes_sha256"] = MODULE.PROMOTE.sha256_text(
                release["body"]
            )
            publication["release_notes_sha256"] = expected["release_notes_sha256"]
            ref = {"object": {"sha": COMMIT}}
            MODULE.verify_live_publication(
                publication, expected, release, ref, downloads
            )
            release["body"] = "altered"
            with self.assertRaisesRegex(ValueError, "notes"):
                MODULE.verify_live_publication(
                    publication, expected, release, ref, downloads
                )

    def test_aur_inspection_is_idempotent_but_refuses_altered_same_version(self) -> None:
        with tempfile.TemporaryDirectory(prefix="splinterm-aur-") as value:
            root = Path(value)
            current = root / "current"
            draft = root / "draft"
            write_recipe(current, "1.2.3alpha4")
            write_recipe(draft, "1.2.3alpha4")
            self.assertEqual(
                MODULE.inspect_aur_state(current, draft)["state"], "already-current"
            )
            (current / "PKGBUILD").write_text("pkgver=1.2.3alpha4\n# altered\n")
            with self.assertRaisesRegex(ValueError, "differs"):
                MODULE.inspect_aur_state(current, draft)
            write_recipe(root / "old", "1.2.3alpha3", "old")
            self.assertEqual(
                MODULE.inspect_aur_state(root / "old", draft)["state"],
                "update-required",
            )

    def test_distribution_receipt_requires_both_exact_package_bases(self) -> None:
        publication = receipt(promotion())
        records = [
            {
                "package_base": base,
                "commit": COMMIT,
                "version": "1.2.3alpha4",
                "files": {name: DIGEST for name in MODULE.AUR_FILES},
            }
            for base in sorted(MODULE.AUR_BASES)
        ]
        value = MODULE.create_distribution_receipt(
            publication, 56, DIGEST, "https://example.invalid/run/1", records
        )
        self.assertEqual(value["state"], "distributed")
        with self.assertRaisesRegex(ValueError, "package set"):
            MODULE.create_distribution_receipt(
                publication, 56, DIGEST, "https://example.invalid/run/1", records[:1]
            )

    def test_receipt_run_accepts_only_successful_publication_workflows(self) -> None:
        run = {
            "id": 56,
            "event": "workflow_dispatch",
            "status": "completed",
            "conclusion": "success",
            "path": ".github/workflows/recover-release.yml",
            "head_branch": "main",
            "head_sha": COMMIT,
            "repository": {"full_name": REPOSITORY},
        }
        artifacts = {"artifacts": [{
            "id": 99,
            "name": "release-receipt-v1.2.3-alpha4",
            "expired": False,
            "workflow_run": {"id": 56, "head_sha": COMMIT},
        }]}
        self.assertEqual(
            MODULE.PROMOTE.validate_retained_artifact(
                run, artifacts, REPOSITORY, 56, "main",
                MODULE.PROMOTE.RECEIPT_WORKFLOWS, r"release-receipt-v.+", {"success"},
            ),
            (99, COMMIT),
        )
        run["conclusion"] = "failure"
        with self.assertRaisesRegex(ValueError, "conclusion"):
            MODULE.PROMOTE.validate_retained_artifact(
                run, artifacts, REPOSITORY, 56, "main",
                MODULE.PROMOTE.RECEIPT_WORKFLOWS, r"release-receipt-v.+", {"success"},
            )


if __name__ == "__main__":
    unittest.main()
