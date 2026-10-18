"""Early provisioner guards only; never run a root apply or modify host state."""

import os
import socket
import subprocess
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("provision-arch-worker.sh")


class ProvisioningGuards(unittest.TestCase):
    def invoke(self, *args):
        return subprocess.run(
            ["bash", str(SCRIPT), *args],
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )

    def test_missing_arguments_print_usage(self):
        result = self.invoke()
        self.assertEqual(result.returncode, 2)
        self.assertIn("Usage:", result.stderr)

    def test_unknown_mode_is_not_an_apply(self):
        result = self.invoke("--force", "host", "operator", "/archive", "/key")
        self.assertEqual(result.returncode, 2)

    def test_check_stops_before_any_archive_or_account_action(self):
        result = self.invoke(
            "--check",
            socket.gethostname() + "-not-this-host",
            "operator",
            "/archive",
            "/key",
        )
        self.assertEqual(result.returncode, 1)
        # Root must stop at the earlier caller guard, before hostname validation.
        diagnostic = (
            "Run the check as the operator, not root."
            if os.geteuid() == 0
            else "Host mismatch"
        )
        self.assertIn(diagnostic, result.stderr)
        self.assertNotIn("staging:", result.stderr)

    @unittest.skipIf(
        os.geteuid() == 0, "non-root guard requires a non-root test runner"
    )
    def test_apply_requires_explicit_operator_sudo(self):
        result = self.invoke(
            "--apply", socket.gethostname(), "operator", "/archive", "/key"
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("normal interactive sudo", result.stderr)
        self.assertNotIn("staging:", result.stderr)


if __name__ == "__main__":
    unittest.main()
