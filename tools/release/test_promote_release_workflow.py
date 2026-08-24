from __future__ import annotations

from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/promote-release.yml"


class PromoteReleaseWorkflowTests(unittest.TestCase):
    def test_manual_inputs_bind_run_and_manifest(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("workflow_dispatch:", workflow)
        self.assertNotIn("\n  push:", workflow)
        self.assertIn("candidate_run_id:", workflow)
        self.assertIn("candidate_manifest_sha256:", workflow)
        self.assertIn("refs/heads/main|refs/heads/maint/0.1", workflow)
        self.assertIn('--expected-branch "$GITHUB_REF_NAME"', workflow)
        self.assertIn("group: versioned-release-promotion", workflow)
        self.assertIn("cancel-in-progress: false", workflow)

    def test_only_protected_publish_job_can_write(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        verify = workflow.index("  verify:")
        publish = workflow.index("  publish:")
        self.assertIn("permissions:\n  actions: read\n  contents: read", workflow[:verify])
        self.assertNotIn("contents: write", workflow[verify:publish])
        self.assertIn("environment: release", workflow[publish:])
        self.assertIn("permissions:\n      contents: write", workflow[publish:])
        self.assertEqual(workflow.count("environment: release"), 1)
        self.assertEqual(workflow.count("contents: write"), 1)
        publish = workflow.index("  publish:")
        release_token = "secrets.SPLINTERM_RELEASE_TOKEN"
        self.assertNotIn(release_token, workflow[:publish])
        self.assertEqual(workflow[publish:].count(release_token), 5)
        self.assertNotIn("GH_TOKEN: ${{ github.token }}", workflow[publish:])

    def test_publish_consumes_candidate_without_building(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertNotIn("cargo build", workflow)
        self.assertNotIn("build-local-package", workflow)
        self.assertNotIn("makepkg", workflow)
        self.assertIn("actions/download-artifact@v4", workflow)
        self.assertIn("artifact-ids: ${{ steps.source.outputs.artifact_id }}", workflow)
        self.assertIn("merge-multiple: true", workflow)
        self.assertIn("gh api --paginate --slurp", workflow)
        self.assertIn("--expected-commit \"${{ steps.source.outputs.commit }}\"", workflow)
        self.assertIn("run-id: ${{ inputs.candidate_run_id }}", workflow)
        self.assertNotIn("python -m zipfile", workflow)
        self.assertIn("verify-candidate", workflow)
        self.assertGreaterEqual(workflow.count("verify-candidate"), 2)
        self.assertEqual(workflow.count('--expected-branch "$GITHUB_REF_NAME"'), 3)
        self.assertIn("cmp verified/promotion.json promotion-reverified.json", workflow)
        self.assertIn("name: Reattest candidate CI provenance through the Actions API", workflow)
        self.assertIn("promotion-ci-attestation.json", workflow)
        self.assertIn('promotion["ci"] != attestation', workflow)
        publish = workflow.index("  publish:")
        self.assertIn("ref: ${{ github.sha }}", workflow[publish:])
        self.assertNotIn("ref: ${{ needs.verify.outputs.commit }}", workflow[publish:])
        self.assertEqual(workflow.count("persist-credentials: false"), 2)

    def test_publication_refuses_unprotected_environment_and_replacement(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        environment = workflow.index("name: Require configured protected release environment")
        existing = workflow.index("name: Refuse existing tag or release and fail closed on API errors")
        create = workflow.index("name: Create versioned tag and prerelease")
        self.assertLess(environment, existing)
        self.assertLess(existing, create)
        self.assertIn('rule.get("type") == "required_reviewers"', workflow[environment:existing])
        self.assertIn('"custom_branch_policies": True', workflow[environment:existing])
        self.assertIn(
            'sorted(item.get("name") for item in policies) != ["main", "maint/0.1"]',
            workflow[environment:existing],
        )
        self.assertIn("Refuse existing tag or release and fail closed on API errors", workflow)
        self.assertIn("HTTP/2.0 404", workflow)
        self.assertIn("HTTP/1.1 404", workflow)
        self.assertNotIn("--clobber", workflow)
        self.assertNotIn("release delete", workflow)
        self.assertNotIn("git push --force", workflow)
        self.assertNotIn("git tag -f", workflow)

    def test_published_assets_are_downloaded_and_receipted(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        create = workflow.index("name: Create versioned tag and prerelease")
        upload = workflow.index("name: Upload exact approved assets without replacement")
        verify = workflow.index("name: Download and verify published release")
        receipt = workflow.index("name: Retain durable publication receipt")
        self.assertLess(create, upload)
        self.assertLess(upload, verify)
        self.assertLess(verify, receipt)
        self.assertIn("gh release download", workflow[verify:receipt])
        self.assertIn("promote-release.py receipt", workflow[verify:receipt])
        self.assertIn("retention-days: 90", workflow[receipt:])

    def test_promotion_tests_are_part_of_ci(self) -> None:
        ci = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        self.assertIn("tools/release/test_promote_release.py", ci)
        self.assertIn("tools/release/test_promote_release_workflow.py", ci)


if __name__ == "__main__":
    unittest.main()
