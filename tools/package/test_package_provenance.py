from __future__ import annotations

import hashlib
import importlib.util
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "tools/package/generate-package-provenance.py"
SPEC = importlib.util.spec_from_file_location("package_provenance_generator", SOURCE)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)

COMMIT = "1234567890abcdef1234567890abcdef12345678"


class PackageProvenanceTests(unittest.TestCase):
    def test_generates_canonical_exact_pair_record(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            daemon = root / "splinterd"
            client = root / "splinterm"
            output = root / "record.json"
            daemon.write_bytes(b"daemon")
            client.write_bytes(b"client")
            subprocess.run(
                [
                    sys.executable,
                    str(SOURCE),
                    "--commit",
                    COMMIT,
                    "--provenance",
                    "local-committed-head",
                    "--version",
                    "0.1.0-beta1",
                    "--daemon",
                    str(daemon),
                    "--client",
                    str(client),
                    "--output",
                    str(output),
                ],
                check=True,
            )
            record = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(record["schema"], 1)
            self.assertEqual(record["source_commit"], COMMIT)
            self.assertEqual(
                record["daemon_sha256"], hashlib.sha256(b"daemon").hexdigest()
            )
            self.assertEqual(
                record["client_sha256"], hashlib.sha256(b"client").hexdigest()
            )
            self.assertEqual(
                record["pair_build_identity"],
                MODULE.pair_identity(COMMIT, "local-committed-head", "0.1.0-beta1"),
            )
            self.assertEqual(output.stat().st_mode & 0o777, 0o644)
            self.assertEqual(
                output.read_text(encoding="utf-8"),
                json.dumps(record, sort_keys=True, separators=(",", ":")) + "\n",
            )

    def test_rejects_zero_malformed_and_unsupported_provenance(self) -> None:
        for commit, provenance, version in [
            ("0" * 40, "local-committed-head", "0.1.0"),
            ("g" * 40, "local-committed-head", "0.1.0"),
            (COMMIT, "unpackaged", "0.1.0"),
            (COMMIT, "release-candidate", "bad version"),
        ]:
            with self.subTest(commit=commit, provenance=provenance, version=version):
                with self.assertRaises(ValueError):
                    MODULE.pair_identity(commit, provenance, version)

    def test_same_version_different_commit_or_kind_has_distinct_identity(self) -> None:
        baseline = MODULE.pair_identity(COMMIT, "local-committed-head", "0.1.0")
        self.assertNotEqual(
            baseline,
            MODULE.pair_identity("a" * 40, "local-committed-head", "0.1.0"),
        )
        self.assertNotEqual(
            baseline,
            MODULE.pair_identity(COMMIT, "release-candidate", "0.1.0"),
        )
        self.assertNotEqual(
            baseline,
            MODULE.pair_identity(COMMIT, "nix-store", "0.1.0"),
        )

    def test_identity_matches_the_daemon_canonical_fixture(self) -> None:
        # Same inputs as the daemon test's canonical identity fixture, which
        # pins private protocol 35.
        original = MODULE.PRIVATE_PROTOCOL
        MODULE.PRIVATE_PROTOCOL = 35
        try:
            identity = MODULE.pair_identity(
                COMMIT, "local-committed-head", "0.2.3-beta1"
            )
        finally:
            MODULE.PRIVATE_PROTOCOL = original
        self.assertEqual(
            identity,
            "a4a4cef437c25147d8b10c08fd3d5715b6fcbc53274e81f8d4efa700ad4f7cac",
        )

    def test_aur_srcinfo_keeps_the_recipe_options(self) -> None:
        # Records hash the installed binaries, so stripping must stay off.
        # release-doctor checks .SRCINFO only where makepkg exists; this
        # catches the drift on every CI host.
        for directory in ("packaging/aur", "packaging/aur-bin"):
            with self.subTest(directory=directory):
                recipe = (ROOT / directory / "PKGBUILD").read_text(encoding="utf-8")
                srcinfo = (ROOT / directory / ".SRCINFO").read_text(encoding="utf-8")
                declared = re.search(r"^options=\(([^)]*)\)$", recipe, re.MULTILINE)
                self.assertIsNotNone(declared)
                options = [value.strip("'\"") for value in declared.group(1).split()]
                self.assertIn("!strip", options)
                self.assertEqual(
                    re.findall(r"^\toptions = (\S+)$", srcinfo, re.MULTILINE), options
                )


if __name__ == "__main__":
    unittest.main()
