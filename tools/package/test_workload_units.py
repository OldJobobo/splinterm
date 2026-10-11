#!/usr/bin/env python3
"""Focused package tests for nested workload cgroup units."""

from __future__ import annotations

import importlib.util
from pathlib import Path
import tempfile
import unittest

MODULE_PATH = Path(__file__).with_name("validate-package.py")
SPEC = importlib.util.spec_from_file_location("validate_package", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
validate_package = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(validate_package)


SERVICE = """\
[Unit]
Description=Splinterm persistent terminal daemon

[Service]
EnvironmentFile=-%h/.config/splinterm/daemon.env
UnsetEnvironment=SPLINTERM_ENABLE_DEV_ATTACH
ExecStart=/usr/bin/splinterd --require-workload-cgroups
ExecReload=/usr/bin/kill -HUP $MAINPID
TasksMax=2048
MemoryHigh=75%
KillSignal=SIGINT
KillMode=mixed
TimeoutStopSec=90
"""

WORKLOAD_SLICE = """\
[Unit]
Description=Splinterm terminal workloads
StopWhenUnneeded=yes

[Slice]
MemoryHigh=75%
"""


def unit_root(root: Path, *, service: str = SERVICE, workload_slice: str = WORKLOAD_SLICE) -> Path:
    units = root / "usr/lib/systemd/user"
    units.mkdir(parents=True)
    (units / "splinterd.service").write_text(service, encoding="utf-8")
    (units / "app-splinterm.slice").write_text(workload_slice, encoding="utf-8")
    return root


class WorkloadUnitPackageTests(unittest.TestCase):
    def test_nested_workload_units_are_required_and_valid(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            self.assertIn(
                "usr/lib/systemd/user/app-splinterm.slice",
                validate_package.REQUIRED,
            )
            validate_package.validate_systemd_unit(unit_root(Path(directory)))

    def test_packaged_daemon_cannot_silently_disable_containment(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = unit_root(
                Path(directory),
                service=SERVICE.replace(
                    "ExecStart=/usr/bin/splinterd --require-workload-cgroups",
                    "ExecStart=/usr/bin/splinterd",
                ),
            )
            with self.assertRaisesRegex(
                AssertionError, "headless safety or resource settings"
            ):
                validate_package.validate_systemd_unit(root)

    def test_workload_slice_rejects_explicit_task_ceiling(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = unit_root(
                Path(directory),
                workload_slice=WORKLOAD_SLICE + "TasksMax=2048\n",
            )
            with self.assertRaisesRegex(
                AssertionError, "inherit the user manager task limit"
            ):
                validate_package.validate_systemd_unit(root)

    def test_workload_slice_rejects_destructive_hard_memory_limit(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = unit_root(
                Path(directory),
                workload_slice=WORKLOAD_SLICE + "MemoryMax=80%\n",
            )
            with self.assertRaises(AssertionError):
                validate_package.validate_systemd_unit(root)


if __name__ == "__main__":
    unittest.main()
