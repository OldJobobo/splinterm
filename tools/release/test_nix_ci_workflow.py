"""Keep Nix validation inside the existing required CI boundary."""

import unittest
from pathlib import Path

WORKFLOW = Path(__file__).resolve().parents[2] / ".github/workflows/ci.yml"


class NixCiWorkflowTests(unittest.TestCase):
    def test_required_check_depends_on_nix(self):
        text = WORKFLOW.read_text()
        check = text[text.index("\n  check:\n") :]
        self.assertIn("\n      - nix\n", check)
        self.assertIn("needs.nix.result != 'success'", check)
        self.assertIn("needs.nix.result == 'success'", check)
        nix_job = text.split("\n  nix:\n", 1)[1].split("\n  arch-warning-check:\n", 1)[0]
        self.assertNotIn("continue-on-error", nix_job)
        # Only a pull request may skip the job, and only on an exact nix=false;
        # pushes and merge groups always run it.
        self.assertIn("    needs: [preflight, changes]\n", nix_job)
        self.assertEqual(
            [line.strip() for line in nix_job.splitlines() if line.strip().startswith("if:")],
            [
                "if: github.event_name != 'pull_request' || needs.changes.outputs.nix != 'false'",
                "if: vars.CACHIX_CACHE != ''",
            ],
        )
        self.assertIn("run: test -r /dev/kvm && test -w /dev/kvm", nix_job)
        self.assertIn("run: nix flake check --print-build-logs --max-jobs 2 --cores 4", nix_job)
        self.assertRegex(nix_job, r"cachix/install-nix-action@[0-9a-f]{40}\b")
        self.assertIn("persist-credentials: false", nix_job)
        self.assertNotRegex(nix_job, r"desktop-(smoke|test)")

    def test_cachix_is_pinned_inert_until_configured_and_pull_only_for_prs(self):
        text = WORKFLOW.read_text()
        nix_job = text.split("\n  nix:\n", 1)[1].split("\n  arch-warning-check:\n", 1)[0]
        step = nix_job.split("      - name: Use the Cachix binary cache when configured\n", 1)[1]
        step = step.split("\n      - ", 1)[0]
        self.assertIn("        if: vars.CACHIX_CACHE != ''\n", step)
        self.assertIn(
            "uses: cachix/cachix-action@38b082610b782e7e93e209c35fd730d399dee866 # v17",
            step,
        )
        self.assertIn("name: ${{ vars.CACHIX_CACHE }}", step)
        self.assertIn("authToken: ${{ secrets.CACHIX_AUTH_TOKEN }}", step)
        self.assertIn("skipPush: ${{ github.event_name == 'pull_request' }}", step)
        self.assertNotIn("signingKey", step)
        self.assertEqual(text.count("cachix/cachix-action@"), 1)
        # The cache is configured after Nix is installed and before any build.
        self.assertLess(
            nix_job.index("cachix/install-nix-action@"),
            nix_job.index("cachix/cachix-action@"),
        )
        self.assertLess(
            nix_job.index("cachix/cachix-action@"), nix_job.index("run: nix flake check")
        )

    def test_pull_requests_and_maintenance_pushes_run_validation(self):
        text = WORKFLOW.read_text()
        self.assertIn('branches: [main, "maint/0.1"]', text)
        self.assertRegex(text, r"(?m)^  pull_request:")
        self.assertRegex(text, r"(?m)^  merge_group:")
        self.assertNotRegex(text, r"(?m)^\s+paths(?:-ignore)?:")
        self.assertIn("tools/release/test_nix_ci_workflow.py", text)


if __name__ == "__main__":
    unittest.main()
