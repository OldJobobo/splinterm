from __future__ import annotations

import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/release/promote-release.py"
SPEC = importlib.util.spec_from_file_location("promote_release", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)

REPOSITORY = "OldJobobo/splinterm"
COMMIT = "a" * 40
VERSION = "1.2.3-alpha4"
RUN_ID = 12345


class CandidateFixture:
    def __init__(
        self, root: Path, previous_version_tag: str = "v0.1.0-beta1"
    ) -> None:
        self.root = root
        self.assets = MODULE.expected_assets(COMMIT, VERSION)
        records = []
        for relative, kind in self.assets.items():
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(f"{relative}\n".encode())
            records.append(
                {"kind": kind, "path": relative, "sha256": MODULE.sha256(path)}
            )
        records.sort(key=lambda value: value["path"])
        self.manifest = {
            "schema": 1,
            "state": "candidate",
            "publishable": False,
            "repository": REPOSITORY,
            "commit": COMMIT,
            "version": VERSION,
            "package_version": VERSION.replace("-", ""),
            "tag": f"v{VERSION}",
            "architecture": "x86_64",
            "previous_version_tag": previous_version_tag,
            "workflow_run": f"https://github.com/{REPOSITORY}/actions/runs/{RUN_ID}",
            "assets": records,
        }
        manifest_path = root / "candidate-manifest.json"
        manifest_path.write_text(
            json.dumps(self.manifest, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        self.manifest_sha256 = MODULE.sha256(manifest_path)
        lines = [f"{record['sha256']}  {record['path']}" for record in records]
        lines.append(f"{self.manifest_sha256}  candidate-manifest.json")
        (root / "SHA256SUMS").write_text("\n".join(lines) + "\n", encoding="utf-8")


class PromoteReleaseTests(unittest.TestCase):
    def test_source_run_requires_matching_release_branch_and_one_artifact(self) -> None:
        run = {
            "id": RUN_ID,
            "event": "workflow_dispatch",
            "status": "completed",
            "conclusion": "success",
            "path": ".github/workflows/release-candidate.yml",
            "head_branch": "main",
            "head_sha": COMMIT,
            "repository": {"full_name": REPOSITORY},
        }
        artifacts = {
            "artifacts": [
                {
                    "id": 99,
                    "name": f"splinterm-{VERSION}-candidate-{COMMIT}",
                    "expired": False,
                    "workflow_run": {"id": RUN_ID, "head_sha": COMMIT},
                }
            ]
        }
        for branch in sorted(MODULE.RELEASE_BRANCHES):
            run["head_branch"] = branch
            self.assertEqual(
                MODULE.validate_source_run(
                    run, artifacts, REPOSITORY, RUN_ID, branch
                ),
                (99, COMMIT),
            )
        artifacts["artifacts"].append(
            {
                "id": 100,
                "name": "diagnostics",
                "expired": False,
                "workflow_run": {"id": RUN_ID, "head_sha": COMMIT},
            }
        )
        with self.assertRaisesRegex(ValueError, "exactly one unexpired"):
            MODULE.validate_source_run(
                run, artifacts, REPOSITORY, RUN_ID, "maint/0.1"
            )
        artifacts["artifacts"].pop()
        run["head_branch"] = "main"
        with self.assertRaisesRegex(ValueError, "head_branch"):
            MODULE.validate_source_run(
                run, artifacts, REPOSITORY, RUN_ID, "maint/0.1"
            )
        with self.assertRaisesRegex(ValueError, "release authority"):
            MODULE.validate_source_run(
                run, artifacts, REPOSITORY, RUN_ID, "feature"
            )

    def test_candidate_closes_over_exact_files_and_hashes(self) -> None:
        with tempfile.TemporaryDirectory(prefix="splinterm-promotion-") as value:
            fixture = CandidateFixture(Path(value))
            promotion = MODULE.verify_candidate(
                fixture.root, REPOSITORY, RUN_ID, COMMIT, fixture.manifest_sha256
            )
            self.assertEqual(promotion["commit"], COMMIT)
            self.assertEqual(len(promotion["public_assets"]), 5)
            self.assertIn("candidate-manifest.json", promotion["public_assets"])
            self.assertIn("SHA256SUMS", promotion["public_assets"])
            self.assertNotIn("aur-source/PKGBUILD", promotion["public_assets"])

    def test_candidate_rejects_a_stale_predecessor_even_when_well_formed(self) -> None:
        with tempfile.TemporaryDirectory(prefix="splinterm-promotion-") as value:
            fixture = CandidateFixture(Path(value), "v0.1.0-alpha3.3")
            with self.assertRaisesRegex(ValueError, "current public release state"):
                MODULE.verify_candidate(
                    fixture.root,
                    REPOSITORY,
                    RUN_ID,
                    COMMIT,
                    fixture.manifest_sha256,
                )

    def test_candidate_rejects_tampering_extra_files_and_unsafe_paths(self) -> None:
        with tempfile.TemporaryDirectory(prefix="splinterm-promotion-") as value:
            fixture = CandidateFixture(Path(value))
            (fixture.root / f"splinterm-{VERSION}.tar.gz").write_bytes(b"tampered")
            with self.assertRaisesRegex(ValueError, "changed"):
                MODULE.verify_candidate(
                    fixture.root, REPOSITORY, RUN_ID, COMMIT, fixture.manifest_sha256
                )
        with tempfile.TemporaryDirectory(prefix="splinterm-promotion-") as value:
            fixture = CandidateFixture(Path(value))
            (fixture.root / "extra").write_text("unexpected", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "unexpected files"):
                MODULE.verify_candidate(
                    fixture.root, REPOSITORY, RUN_ID, COMMIT, fixture.manifest_sha256
                )
        for unsafe in ("../asset", "/asset", "a/../../asset"):
            with self.assertRaisesRegex(ValueError, "unsafe"):
                MODULE.safe_relative(unsafe)

    def test_manifest_hash_and_source_commit_are_explicit_approval_identity(self) -> None:
        with tempfile.TemporaryDirectory(prefix="splinterm-promotion-") as value:
            fixture = CandidateFixture(Path(value))
            with self.assertRaisesRegex(ValueError, "approval input"):
                MODULE.verify_candidate(fixture.root, REPOSITORY, RUN_ID, COMMIT, "b" * 64)
            with self.assertRaisesRegex(ValueError, "source run"):
                MODULE.verify_candidate(
                    fixture.root, REPOSITORY, RUN_ID, "b" * 40, fixture.manifest_sha256
                )

    def test_receipt_requires_exact_tag_commit_and_public_assets(self) -> None:
        with tempfile.TemporaryDirectory(prefix="splinterm-receipt-") as value:
            candidate = Path(value) / "candidate"
            candidate.mkdir()
            fixture = CandidateFixture(candidate)
            promotion = MODULE.verify_candidate(
                candidate, REPOSITORY, RUN_ID, COMMIT, fixture.manifest_sha256
            )
            downloads = Path(value) / "downloads"
            downloads.mkdir()
            names = set(promotion["public_assets"])
            for name in names:
                source = candidate / name
                (downloads / name).write_bytes(source.read_bytes())
            release = {
                "tagName": f"v{VERSION}",
                "isDraft": False,
                "isPrerelease": True,
                "url": f"https://github.com/{REPOSITORY}/releases/tag/v{VERSION}",
                "assets": [{"name": name} for name in sorted(names)],
            }
            ref = {"object": {"sha": COMMIT}}
            receipt = MODULE.create_receipt(
                promotion, release, ref, downloads, "https://example.invalid/run/1"
            )
            self.assertEqual(receipt["state"], "published")
            self.assertEqual(len(receipt["assets"]), 5)
            (downloads / f"splinterm-{VERSION}.tar.gz").write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "differs from approved"):
                MODULE.create_receipt(
                    promotion, release, ref, downloads, "https://example.invalid/run/1"
                )
            (downloads / f"splinterm-{VERSION}.tar.gz").write_bytes(
                (candidate / f"splinterm-{VERSION}.tar.gz").read_bytes()
            )
            release["assets"].append({"name": "unexpected"})
            with self.assertRaisesRegex(ValueError, "not exact"):
                MODULE.create_receipt(
                    promotion, release, ref, downloads, "https://example.invalid/run/1"
                )


if __name__ == "__main__":
    unittest.main()
