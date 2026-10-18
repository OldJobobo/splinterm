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
VALIDATOR_SOURCE = ROOT / "tools/package/validate-package.py"
VALIDATOR_SPEC = importlib.util.spec_from_file_location(
    "package_validator", VALIDATOR_SOURCE
)
assert VALIDATOR_SPEC is not None and VALIDATOR_SPEC.loader is not None
VALIDATOR = importlib.util.module_from_spec(VALIDATOR_SPEC)
VALIDATOR_SPEC.loader.exec_module(VALIDATOR)
PROTOCOL_SOURCE = ROOT / "crates/splinterm-protocol/src/lib.rs"
PREFLIGHT_SOURCE = ROOT / "crates/splinterd/src/handoff_preflight.rs"
CHECKPOINT_SOURCE = ROOT / "crates/splinterm-terminal/src/terminal/checkpoint.rs"
SEAMS_SOURCE = ROOT / "crates/splinterd/src/integration_test_seams.rs"

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

    def test_refuses_binaries_built_with_the_integration_test_feature(self) -> None:
        marker = MODULE.INTEGRATION_TEST_MARKER
        rust = re.findall(
            r'^pub const BUILD_SALT_MARKER: &\[u8; 32\] = b"([^"]+)";$',
            SEAMS_SOURCE.read_text(encoding="utf-8"),
            flags=re.MULTILINE,
        )
        self.assertEqual(rust, [marker.decode("ascii")])
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            clean = root / "clean"
            clean.write_bytes(b"x" * 100_000)
            # A marker straddling a read-block boundary must still be found.
            tainted = root / "tainted"
            tainted.write_bytes(b"x" * (64 * 1024 - 5) + marker + bytes(16))
            self.assertFalse(MODULE.contains_integration_test_marker(clean))
            self.assertTrue(MODULE.contains_integration_test_marker(tainted))
            for daemon, client in ((tainted, clean), (clean, tainted)):
                with self.subTest(daemon=daemon.name, client=client.name):
                    with self.assertRaisesRegex(ValueError, "integration-test feature"):
                        MODULE.generate(
                            COMMIT, "local-committed-head", "0.1.0", daemon, client
                        )
            MODULE.generate(COMMIT, "local-committed-head", "0.1.0", clean, clean)

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

    def test_capability_identity_matches_compiled_contract(self) -> None:
        def rust_u16(path: Path, name: str, *, public: bool = False) -> int:
            visibility = "pub " if public else ""
            matches = re.findall(
                rf"^{visibility}const {re.escape(name)}: u16 = (\d+);$",
                path.read_text(encoding="utf-8"),
                flags=re.MULTILINE,
            )
            self.assertEqual(len(matches), 1, f"missing or duplicate Rust authority {name}")
            return int(matches[0])

        private_protocol = rust_u16(PROTOCOL_SOURCE, "PROTOCOL_VERSION", public=True)
        handoff_protocol = rust_u16(PREFLIGHT_SOURCE, "HANDOFF_PROTOCOL_VERSION")
        checkpoint_schema = rust_u16(
            CHECKPOINT_SOURCE,
            "TERMINAL_CHECKPOINT_SCHEMA_V2",
            public=True,
        )
        descriptor_manifest = rust_u16(PREFLIGHT_SOURCE, "DESCRIPTOR_MANIFEST_VERSION")
        self.assertEqual(MODULE.PRIVATE_PROTOCOL, private_protocol)
        self.assertEqual(MODULE.HANDOFF_PROTOCOL, handoff_protocol)
        self.assertEqual(MODULE.TERMINAL_CHECKPOINT, checkpoint_schema)
        self.assertEqual(MODULE.DESCRIPTOR_MANIFEST, descriptor_manifest)
        self.assertEqual(VALIDATOR.PRIVATE_PROTOCOL_VERSION, private_protocol)
        self.assertEqual(
            VALIDATOR.PACKAGE_PAIR_CAPABILITIES,
            (
                private_protocol,
                private_protocol,
                handoff_protocol,
                handoff_protocol,
                checkpoint_schema,
                checkpoint_schema,
                descriptor_manifest,
                descriptor_manifest,
            ),
        )

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
        # pins private protocol 35 and terminal checkpoint schema 1.
        original = (MODULE.PRIVATE_PROTOCOL, MODULE.TERMINAL_CHECKPOINT)
        MODULE.PRIVATE_PROTOCOL = 35
        MODULE.TERMINAL_CHECKPOINT = 1
        try:
            identity = MODULE.pair_identity(
                COMMIT, "local-committed-head", "0.2.3-beta1"
            )
        finally:
            MODULE.PRIVATE_PROTOCOL, MODULE.TERMINAL_CHECKPOINT = original
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
