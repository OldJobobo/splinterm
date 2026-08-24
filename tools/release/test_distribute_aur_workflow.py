from __future__ import annotations

from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/distribute-aur.yml"


class DistributeAurWorkflowTests(unittest.TestCase):
    def test_distribution_binds_candidate_and_successful_publication_receipt(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("workflow_dispatch:", workflow)
        self.assertNotIn("\n  push:", workflow)
        for value in (
            "candidate_run_id:", "candidate_manifest_sha256:",
            "publication_run_id:", "publication_receipt_sha256:",
            "receipt-run", "verify-publication",
        ):
            self.assertIn(value, workflow)
        self.assertIn("refs/heads/main|refs/heads/maint/0.1", workflow)
        self.assertIn(
            "publication_receipt_sha256: ${{ steps.candidate.outputs.publication_receipt_sha256 }}",
            workflow,
        )

    def test_distribution_is_separately_protected_and_policy_is_fail_closed(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        distribute = workflow.index("  distribute:")
        self.assertIn("environment: aur-release", workflow[distribute:])
        self.assertEqual(workflow.count("environment: aur-release"), 1)
        self.assertIn("--name aur-release", workflow[distribute:])
        self.assertIn("aur-release/deployment-branch-policies", workflow[distribute:])
        self.assertEqual(workflow.count("persist-credentials: false"), 2)
        self.assertEqual(workflow.count("ref: ${{ github.sha }}"), 2)

    def test_ssh_identity_and_credential_scope_are_bounded(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        secret = "secrets.SPLINTERM_AUR_SSH_PRIVATE_KEY"
        self.assertEqual(workflow.count(secret), 2)
        self.assertEqual(workflow.count("AUR_SSH_PRIVATE_KEY:"), 2)
        self.assertEqual(
            workflow.count(
                "aur.archlinux.org ssh-ed25519 "
                "AAAAC3NzaC1lZDI1NTE5AAAAIEuBKrPzbawxA/k2g6NcyV5jmqwJ2s+zpgZGZ7tpLIcN"
            ),
            2,
        )
        self.assertEqual(workflow.count("StrictHostKeyChecking=yes"), 2)
        self.assertNotIn("StrictHostKeyChecking=no", workflow)

    def test_distribution_uses_closed_drafts_without_force_or_rebuild(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        for value in ("cargo build", "makepkg", "--clobber", "release delete"):
            self.assertNotIn(value, workflow)
        # Keep this assembled to ensure even a future textual force option is caught.
        self.assertNotIn("git push " + "--" + "force", workflow)
        self.assertNotIn("git push -f", workflow)
        self.assertIn("verified/candidate/aur-source", workflow)
        self.assertIn("verified/candidate/aur-bin", workflow)
        self.assertEqual(workflow.count("git -C aur-splinterm push origin HEAD:master"), 1)
        self.assertEqual(workflow.count("git -C aur-splinterm-bin push origin HEAD:master"), 1)

    def test_public_state_is_verified_before_push_and_receipt_is_retained(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        verifier_public = workflow.index("Download and reverify exact public tag")
        distribute = workflow.index("  distribute:")
        policy = workflow.index(
            "Require live protected aur-release environment policy", distribute
        )
        protected_public = workflow.index(
            "Reverify exact live public release immediately before AUR inspection",
            distribute,
        )
        inspect = workflow.index(
            "Inspect live AUR repositories before credentials are available", distribute
        )
        push = workflow.index("Push source package", distribute)
        receipt = workflow.index("Retain AUR distribution receipt", distribute)
        self.assertLess(verifier_public, distribute)
        self.assertLess(policy, protected_public)
        self.assertLess(protected_public, inspect)
        self.assertLess(inspect, push)
        self.assertLess(push, receipt)
        self.assertEqual(workflow.count("verify-publication"), 2)
        protected_step = workflow[protected_public:inspect]
        for value in (
            "fetch-release-state",
            "protected-public-ref.json",
            "protected-public-release.json",
            "protected-public-assets",
            "verified/publication/release-receipt.json",
            "verified/promotion.json",
            "verify-publication",
        ):
            self.assertIn(value, protected_step)
        self.assertEqual(protected_step.count("\n      - name:"), 1)
        self.assertIn("retention-days: 90", workflow[receipt:])
        receipt_creation = workflow.index("Create retained AUR distribution receipt")
        receipt_retention = workflow.index("Retain AUR distribution receipt")
        receipt_step = workflow[receipt_creation:receipt_retention]
        self.assertIn(
            "needs.verify.outputs.publication_receipt_sha256", receipt_step
        )
        self.assertNotIn("inputs.publication_receipt_sha256", receipt_step)

    def test_distribution_tests_are_in_ci(self) -> None:
        ci = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        self.assertIn("tools/release/test_distribute_aur.py", ci)
        self.assertIn("tools/release/test_distribute_aur_workflow.py", ci)


if __name__ == "__main__":
    unittest.main()
