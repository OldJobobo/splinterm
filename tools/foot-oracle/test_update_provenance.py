from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("update-provenance.py")
SPEC = importlib.util.spec_from_file_location("update_provenance", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class UpdateProvenanceTests(unittest.TestCase):
    def fixture(self, root: Path) -> tuple[Path, Path]:
        lockfile = root / "Cargo.lock"
        lockfile.write_text("lock fixture\n", encoding="utf-8")
        manifest = root / "provenance.json"
        manifest.write_text(
            json.dumps(
                {
                    "rust": {"cargo_lock_sha256": "a" * 64},
                    "default_final_buffer_profile": {
                        "cargo_lock_sha256": "b" * 64
                    },
                },
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )
        return manifest, lockfile

    def test_default_reports_without_writing(self) -> None:
        with tempfile.TemporaryDirectory() as value:
            manifest, lockfile = self.fixture(Path(value))
            original = manifest.read_bytes()
            self.assertTrue(MODULE.update(manifest, lockfile, write=False))
            self.assertEqual(manifest.read_bytes(), original)

    def test_write_atomically_refreshes_every_duplicate_identity(self) -> None:
        with tempfile.TemporaryDirectory() as value:
            manifest, lockfile = self.fixture(Path(value))
            self.assertTrue(MODULE.update(manifest, lockfile, write=True))
            parsed = json.loads(manifest.read_text(encoding="utf-8"))
            expected = MODULE.sha256(lockfile)
            self.assertEqual(parsed["rust"]["cargo_lock_sha256"], expected)
            self.assertEqual(
                parsed["default_final_buffer_profile"]["cargo_lock_sha256"],
                expected,
            )
            self.assertFalse(MODULE.update(manifest, lockfile, write=True))
            self.assertEqual(list(manifest.parent.glob(".*.tmp")), [])

    def test_malformed_or_identity_free_manifest_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as value:
            root = Path(value)
            manifest, lockfile = self.fixture(root)
            manifest.write_text("{}\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "no Cargo.lock identities"):
                MODULE.proposed_text(manifest, lockfile)
            manifest.write_text("{not json", encoding="utf-8")
            with self.assertRaises(json.JSONDecodeError):
                MODULE.proposed_text(manifest, lockfile)


if __name__ == "__main__":
    unittest.main()
