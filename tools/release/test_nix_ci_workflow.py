"""Keep Nix validation inside the existing required CI boundary."""

import unittest
from pathlib import Path

WORKFLOW = Path(__file__).resolve().parents[2] / ".github/workflows/ci.yml"


class NixCiWorkflowTests(unittest.TestCase):
    def test_required_check_depends_on_nix(self):
        text = WORKFLOW.read_text()
        self.assertRegex(text, r"(?m)^  check:\n    needs: nix\n")
        nix_job = text.split("\n  nix:\n", 1)[1].split("\n  check:\n", 1)[0]
        self.assertNotIn("continue-on-error", nix_job)
        self.assertNotRegex(nix_job, r"(?m)^\s+if:")
        self.assertIn("run: test -r /dev/kvm && test -w /dev/kvm", nix_job)
        self.assertIn("run: nix flake check --print-build-logs --max-jobs 2 --cores 4", nix_job)
        self.assertRegex(nix_job, r"cachix/install-nix-action@[0-9a-f]{40}\b")
        self.assertIn("persist-credentials: false", nix_job)
        self.assertNotRegex(nix_job, r"desktop-(smoke|test)")

    def test_pull_requests_and_maintenance_pushes_run_validation(self):
        text = WORKFLOW.read_text()
        self.assertIn('branches: [main, "maint/0.1"]', text)
        self.assertRegex(text, r"(?m)^  pull_request:")
        self.assertNotRegex(text, r"(?m)^\s+paths(?:-ignore)?:")
        self.assertIn("tools/release/test_nix_ci_workflow.py", text)


if __name__ == "__main__":
    unittest.main()
