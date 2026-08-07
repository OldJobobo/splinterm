from __future__ import annotations

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("edge-manifest.py")
REPOSITORY = "OldJobobo/splinterm"
COMMIT = "a" * 40


class EdgeManifestTests(unittest.TestCase):
    def run_manifest(self, *arguments: str, check: bool = True) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SCRIPT), *arguments],
            check=check,
            capture_output=True,
            text=True,
        )

    def create_fixture(self, directory: Path) -> tuple[Path, Path, Path]:
        main = directory / f"splinterm-{COMMIT}-x86_64.pkg.tar.zst"
        mcp = directory / f"splinterm-mcp-{COMMIT}-x86_64.pkg.tar.zst"
        manifest = directory / "edge-manifest.json"
        main.write_bytes(b"main-package")
        mcp.write_bytes(b"mcp-package")
        self.run_manifest(
            "create",
            "--repository",
            REPOSITORY,
            "--commit",
            COMMIT,
            "--splinterm",
            str(main),
            "--splinterm-mcp",
            str(mcp),
            "--output",
            str(manifest),
        )
        return main, mcp, manifest

    def test_create_inspect_and_verify_exact_commit_bound_packages(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            main, mcp, manifest = self.create_fixture(directory)
            inspected = self.run_manifest(
                "inspect", "--repository", REPOSITORY, str(manifest)
            ).stdout.splitlines()
            self.assertEqual(
                inspected,
                [
                    COMMIT,
                    f"edge-{COMMIT}",
                    main.name,
                    "01d53777a4af61ddd6cd4c6469de04cf9353eec1876146ab1fb3040d1655cf66",
                    mcp.name,
                    "f37b07a18e3c7a4c0d1057d51778c146d2ba9411753c942cb175e7f3969dbd98",
                ],
            )
            self.run_manifest(
                "verify", "--repository", REPOSITORY, str(manifest), str(directory)
            )

    def test_rejects_repository_mismatch_and_unknown_fields(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            _, _, manifest = self.create_fixture(directory)
            mismatch = self.run_manifest(
                "inspect",
                "--repository",
                "someone/else",
                str(manifest),
                check=False,
            )
            self.assertNotEqual(mismatch.returncode, 0)
            contents = json.loads(manifest.read_text(encoding="utf-8"))
            contents["unexpected"] = True
            manifest.write_text(json.dumps(contents), encoding="utf-8")
            unknown = self.run_manifest(
                "inspect", "--repository", REPOSITORY, str(manifest), check=False
            )
            self.assertNotEqual(unknown.returncode, 0)

    def test_rejects_asset_substitution_and_checksum_drift(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            main, _, manifest = self.create_fixture(directory)
            contents = json.loads(manifest.read_text(encoding="utf-8"))
            contents["release"] = "edge"
            manifest.write_text(json.dumps(contents), encoding="utf-8")
            moving_release = self.run_manifest(
                "inspect", "--repository", REPOSITORY, str(manifest), check=False
            )
            self.assertNotEqual(moving_release.returncode, 0)

            _, _, manifest = self.create_fixture(directory)
            contents = json.loads(manifest.read_text(encoding="utf-8"))
            contents["packages"]["splinterm"]["asset"] = "splinterm-edge.pkg.tar.zst"
            manifest.write_text(json.dumps(contents), encoding="utf-8")
            substituted = self.run_manifest(
                "inspect", "--repository", REPOSITORY, str(manifest), check=False
            )
            self.assertNotEqual(substituted.returncode, 0)

            _, _, manifest = self.create_fixture(directory)
            main.write_bytes(b"changed-package")
            drift = self.run_manifest(
                "verify",
                "--repository",
                REPOSITORY,
                str(manifest),
                str(directory),
                check=False,
            )
            self.assertNotEqual(drift.returncode, 0)


if __name__ == "__main__":
    unittest.main()
