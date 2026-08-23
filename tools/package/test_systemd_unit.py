#!/usr/bin/env python3
"""Regression tests for the packaged splinterd resource guard."""

from __future__ import annotations

import importlib.util
from pathlib import Path
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
VALIDATOR_PATH = ROOT / "tools/package/validate-package.py"
SPEC = importlib.util.spec_from_file_location(
    "splinterm_package_validator", VALIDATOR_PATH
)
assert SPEC is not None and SPEC.loader is not None
VALIDATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VALIDATOR)


class SystemdUnitTest(unittest.TestCase):
    def fixture_root(self, directory: Path, unit: str) -> Path:
        root = directory / "root"
        target = root / "usr/lib/systemd/user/splinterd.service"
        target.parent.mkdir(parents=True)
        target.write_text(unit, encoding="utf-8")
        workload_slice = root / "usr/lib/systemd/user/app-splinterm.slice"
        workload_slice.write_text(
            (ROOT / "dist/systemd/user/app-splinterm.slice").read_text(
                encoding="utf-8"
            ),
            encoding="utf-8",
        )
        return root

    def test_packaged_unit_passes_resource_validation(self) -> None:
        unit = (ROOT / "dist/systemd/user/splinterd.service").read_text(
            encoding="utf-8"
        )
        with tempfile.TemporaryDirectory(prefix="splinterm-unit-") as value:
            VALIDATOR.validate_systemd_unit(self.fixture_root(Path(value), unit))

    def test_validator_rejects_each_missing_resource_guard(self) -> None:
        unit = (ROOT / "dist/systemd/user/splinterd.service").read_text(
            encoding="utf-8"
        )
        for setting in ("TasksMax=2048", "MemoryHigh=75%"):
            with self.subTest(setting=setting):
                candidate = unit.replace(f"{setting}\n", "", 1)
                with tempfile.TemporaryDirectory(prefix="splinterm-unit-") as value:
                    with self.assertRaisesRegex(AssertionError, setting):
                        VALIDATOR.validate_systemd_unit(
                            self.fixture_root(Path(value), candidate)
                        )


if __name__ == "__main__":
    unittest.main()
