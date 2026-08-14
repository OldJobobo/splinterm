from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("release-manifest.py")
SPEC = importlib.util.spec_from_file_location("release_manifest", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)

REPOSITORY = "OldJobobo/splinterm"
COMMIT = "1" * 40
MAIN = f"splinterm-{COMMIT}-x86_64.pkg.tar.zst"
MCP = f"splinterm-mcp-{COMMIT}-x86_64.pkg.tar.zst"


class ReleaseManifestTests(unittest.TestCase):
    def write_json(self, directory: str, name: str, value: object) -> Path:
        path = Path(directory) / name
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    def release(self, tag: str, published: str, *, digest: str = "a" * 64) -> dict[str, object]:
        return {
            "draft": False,
            "tag_name": tag,
            "published_at": published,
            "assets": [
                {
                    "name": "candidate-manifest.json",
                    "digest": f"sha256:{digest}",
                    "browser_download_url": f"https://github.com/{REPOSITORY}/releases/download/{tag}/candidate-manifest.json",
                }
            ],
        }

    def manifest(self) -> dict[str, object]:
        return {
            "schema": 1,
            "state": "candidate",
            "publishable": False,
            "repository": REPOSITORY,
            "commit": COMMIT,
            "version": "1.2.3-alpha4",
            "tag": "v1.2.3-alpha4",
            "architecture": "x86_64",
            "assets": [
                {"kind": "arch-package", "path": MAIN, "sha256": "b" * 64},
                {"kind": "arch-package", "path": MCP, "sha256": "c" * 64},
                {"kind": "source-archive", "path": "source.tar.gz", "sha256": "d" * 64},
            ],
        }

    def test_selects_newest_published_semver_release_and_ignores_edge(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            index = self.write_json(
                directory,
                "releases.json",
                [
                    self.release("edge-" + "2" * 40, "2026-08-15T00:00:00Z"),
                    self.release("v1.2.3-alpha3", "2026-08-13T00:00:00Z"),
                    self.release("v1.2.3-alpha4", "2026-08-14T00:00:00Z", digest="e" * 64),
                ],
            )
            self.assertEqual(
                MODULE.select_release(REPOSITORY, index),
                ["v1.2.3-alpha4", "e" * 64],
            )

    def test_draft_and_unclosed_releases_are_ignored(self) -> None:
        draft = self.release("v1.2.3", "2026-08-14T00:00:00Z")
        draft["draft"] = True
        unclosed = self.release("v1.2.2", "2026-08-13T00:00:00Z")
        unclosed["assets"] = []
        with tempfile.TemporaryDirectory() as directory:
            index = self.write_json(directory, "releases.json", [draft, unclosed])
            with self.assertRaisesRegex(ValueError, "no published versioned release"):
                MODULE.select_release(REPOSITORY, index)

    def test_inspects_exact_commit_bound_package_pair(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            manifest = self.write_json(directory, "manifest.json", self.manifest())
            self.assertEqual(
                MODULE.inspect_manifest(REPOSITORY, "v1.2.3-alpha4", manifest),
                [COMMIT, "v1.2.3-alpha4", "1.2.3-alpha4", MAIN, "b" * 64, MCP, "c" * 64],
            )

    def test_rejects_wrong_repository_tag_architecture_and_package_set(self) -> None:
        mutations = (
            ("repository", "Other/project"),
            ("tag", "v1.2.3-alpha3"),
            ("architecture", "aarch64"),
        )
        for field, value in mutations:
            with self.subTest(field=field), tempfile.TemporaryDirectory() as directory:
                contents = self.manifest()
                contents[field] = value
                manifest = self.write_json(directory, "manifest.json", contents)
                with self.assertRaises(ValueError):
                    MODULE.inspect_manifest(REPOSITORY, "v1.2.3-alpha4", manifest)
        with tempfile.TemporaryDirectory() as directory:
            contents = self.manifest()
            contents["assets"] = contents["assets"][:1]
            manifest = self.write_json(directory, "manifest.json", contents)
            with self.assertRaisesRegex(ValueError, "package set"):
                MODULE.inspect_manifest(REPOSITORY, "v1.2.3-alpha4", manifest)


if __name__ == "__main__":
    unittest.main()
