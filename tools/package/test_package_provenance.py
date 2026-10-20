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
REQUESTS_SOURCE = ROOT / "crates/splinterd/src/handoff_requests.rs"

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

    def test_vm_acceptance_marker_matches_the_rust_authority(self) -> None:
        rust = re.findall(
            r'^pub\(crate\) static VM_ACCEPTANCE_MARKER: \[u8; (\d+)\] = \*b"([^"]+)";$',
            REQUESTS_SOURCE.read_text(encoding="utf-8"),
            flags=re.MULTILINE,
        )
        marker = MODULE.VM_ACCEPTANCE_MARKER
        self.assertEqual(rust, [(str(len(marker)), marker.decode("ascii"))])
        # The marker is compiled only into the feature build.
        source = REQUESTS_SOURCE.read_text(encoding="utf-8")
        declaration = source.index("pub(crate) static VM_ACCEPTANCE_MARKER")
        self.assertIn(
            '#[cfg(feature = "vm-acceptance-live-handoff")]\n#[used]\n',
            source[declaration - 80 : declaration],
        )
        self.assertNotIn(marker.decode("ascii"), source.replace(
            f'*b"{marker.decode("ascii")}"', ""
        ))

    def test_refuses_vm_acceptance_builds_outside_explicit_local_packages(self) -> None:
        marker = MODULE.VM_ACCEPTANCE_MARKER
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            clean = root / "clean"
            clean.write_bytes(b"x" * 100_000)
            # A marker straddling a read-block boundary must still be found.
            marked = root / "marked"
            marked.write_bytes(b"x" * (64 * 1024 - 7) + marker + bytes(16))
            self.assertFalse(MODULE.contains_vm_acceptance_marker(clean))
            self.assertTrue(MODULE.contains_vm_acceptance_marker(marked))
            # Release and Nix packages never, even when asked.
            for provenance in ("release-candidate", "nix-store"):
                for allow in (False, True):
                    with self.subTest(provenance=provenance, allow=allow):
                        with self.assertRaisesRegex(
                            ValueError, "release and Nix packages must never enable it"
                        ):
                            MODULE.generate(
                                COMMIT, provenance, "0.2.0", marked, clean, allow
                            )
                # Asking without a marked daemon is refused too.
                with self.assertRaisesRegex(ValueError, "local-committed-head provenance"):
                    MODULE.generate(COMMIT, provenance, "0.2.0", clean, clean, True)
            # A local package only when explicitly requested.
            for daemon, client in ((marked, clean), (clean, marked)):
                with self.assertRaisesRegex(
                    ValueError, "--allow-vm-acceptance-live-handoff"
                ):
                    MODULE.generate(
                        COMMIT, "local-committed-head", "0.2.0", daemon, client
                    )
            record = MODULE.generate(
                COMMIT, "local-committed-head", "0.2.0", marked, clean, True
            )
            self.assertEqual(record["provenance"], "local-committed-head")
            # A request whose daemon lacks the marker is a mislabelled build.
            with self.assertRaisesRegex(ValueError, "was not built with the"):
                MODULE.generate(
                    COMMIT, "local-committed-head", "0.2.0", clean, marked, True
                )
            with self.assertRaisesRegex(ValueError, "was not built with the"):
                MODULE.generate(COMMIT, "local-committed-head", "0.2.0", clean, clean, True)
            # Ordinary local builds are unaffected.
            MODULE.generate(COMMIT, "local-committed-head", "0.2.0", clean, clean)

    def test_the_generator_cli_refuses_a_marked_release_daemon(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            daemon = root / "splinterd"
            client = root / "splinterm"
            daemon.write_bytes(b"daemon" + MODULE.VM_ACCEPTANCE_MARKER)
            client.write_bytes(b"client")
            for extra in ([], ["--allow-vm-acceptance-live-handoff"]):
                with self.subTest(extra=extra):
                    result = subprocess.run(
                        [
                            sys.executable, str(SOURCE), "--commit", COMMIT,
                            "--provenance", "release-candidate", "--version", "0.2.0",
                            "--daemon", str(daemon), "--client", str(client),
                            "--output", str(root / "record.json"), *extra,
                        ],
                        capture_output=True,
                        text=True,
                        check=False,
                    )
                    self.assertEqual(result.returncode, 1, result)
                    self.assertIn("vm-acceptance-live-handoff", result.stderr)
                    self.assertFalse((root / "record.json").exists())

    def test_package_validation_refuses_test_only_builds(self) -> None:
        def package_root(directory: Path, provenance: str, daemon: bytes) -> Path:
            root = directory / provenance
            (root / "usr/bin").mkdir(parents=True)
            (root / "usr/share/splinterm").mkdir(parents=True)
            (root / "usr/share/splinterm/package-provenance-v1.json").write_text(
                json.dumps({"provenance": provenance}), encoding="utf-8"
            )
            (root / "usr/bin/splinterd").write_bytes(daemon)
            for name in ("splinterm", "splinterm-relay", "splinterm-pty-child"):
                (root / "usr/bin" / name).write_bytes(b"clean " + name.encode())
            return root

        marker = MODULE.VM_ACCEPTANCE_MARKER
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            release = package_root(base, "release-candidate", b"daemon" + marker)
            for allow in (False, True):
                with self.assertRaisesRegex(AssertionError, "must never enable it"):
                    VALIDATOR.validate_test_only_builds(release, allow)
            local = package_root(base, "local-committed-head", b"daemon" + marker)
            with self.assertRaisesRegex(
                AssertionError, "--allow-vm-acceptance-live-handoff"
            ):
                VALIDATOR.validate_test_only_builds(local, False)
            self.assertTrue(VALIDATOR.validate_test_only_builds(local, True))
            # No other packaged binary may carry it.
            (local / "usr/bin/splinterm-relay").write_bytes(b"relay" + marker)
            with self.assertRaisesRegex(AssertionError, "splinterm-relay"):
                VALIDATOR.validate_test_only_builds(local, True)
            plain = base / "plain"
            plain.mkdir()
            plain = package_root(plain, "local-committed-head", b"daemon")
            self.assertFalse(VALIDATOR.validate_test_only_builds(plain, False))
            with self.assertRaisesRegex(AssertionError, "was not built with the"):
                VALIDATOR.validate_test_only_builds(plain, True)
            # The integration-test salt marker is refused in every binary.
            (plain / "usr/bin/splinterm-pty-child").write_bytes(
                b"pty" + MODULE.INTEGRATION_TEST_MARKER
            )
            with self.assertRaisesRegex(AssertionError, "integration-test feature"):
                VALIDATOR.validate_test_only_builds(plain, False)

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
