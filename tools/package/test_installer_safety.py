#!/usr/bin/env python3
"""Regression tests for installation from daemon-owned shells."""

from __future__ import annotations

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]


class InstallerSafetyTest(unittest.TestCase):
    def daemon_owned_environment(self, directory: Path) -> dict[str, str]:
        proc = directory / "proc"
        binary = directory / "bin" / "splinterm-pty-child"
        binary.parent.mkdir()
        binary.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        binary.chmod(0o700)
        for pid, parent, executable in (
            (101, 100, Path("/usr/bin/bash")),
            (100, 1, binary),
        ):
            process = proc / str(pid)
            process.mkdir(parents=True)
            (process / "status").write_text(
                f"Name:\ttest\nPPid:\t{parent}\n", encoding="utf-8"
            )
            (process / "exe").symlink_to(executable)
        environment = os.environ.copy()
        environment.update(
            {
                "SPLINTERM_INSTALL_PROC_ROOT": str(proc),
                "SPLINTERM_INSTALL_PARENT_PID": "101",
            }
        )
        return environment

    def assert_refuses(self, script: Path, *arguments: str) -> None:
        with tempfile.TemporaryDirectory(prefix="splinterm-installer-safety-") as value:
            result = subprocess.run(
                [str(script), *arguments],
                cwd=ROOT,
                env=self.daemon_owned_environment(Path(value)),
                text=True,
                capture_output=True,
                check=False,
            )
        self.assertEqual(result.returncode, 1, result)
        self.assertEqual(result.stdout, "")
        self.assertIn("Refusing to install from inside a Splinterm-owned shell", result.stderr)
        self.assertIn("Run the installer from Foot", result.stderr)

    def test_source_installer_refuses_before_build_or_service_stop(self) -> None:
        self.assert_refuses(ROOT / "install.sh", "--source", "--yes")

    def test_local_upgrade_refuses_before_package_or_service_work(self) -> None:
        self.assert_refuses(
            ROOT / "tools/package/upgrade-local-package.sh", "--yes"
        )

    def test_unattended_install_never_opts_into_user_screensaver_integration(self) -> None:
        source = (ROOT / "install.sh").read_text(encoding="utf-8")
        self.assertIn("if [[ $assume_yes == true || ! -t 0 ]]; then", source)
        unattended = source.split(
            "if [[ $assume_yes == true || ! -t 0 ]]; then", maxsplit=1
        )[1].split("fi", maxsplit=1)[0]
        self.assertIn("was not enabled", unattended)
        self.assertNotIn("omarchy-screensaver enable\n", unattended)

    def test_installer_never_replaces_an_existing_user_launcher(self) -> None:
        source = (ROOT / "install.sh").read_text(encoding="utf-8")
        collision = source.split(
            "if [[ -e $launcher || -L $launcher ]]; then", maxsplit=1
        )[1].split("if [[ $assume_yes", maxsplit=1)[0]
        self.assertIn("Splinterm did not modify or replace it", collision)
        self.assertIn("return 0", collision)
        for forbidden in ("rm -f \"$launcher\"", "mv \"$launcher\"", "ln -sf"):
            self.assertNotIn(forbidden, source)


if __name__ == "__main__":
    unittest.main()
