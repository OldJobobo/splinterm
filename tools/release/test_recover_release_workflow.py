from __future__ import annotations

from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/recover-release.yml"


class RecoverReleaseWorkflowTests(unittest.TestCase):
    def test_recovery_is_manual_exact_input_and_separately_protected(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("workflow_dispatch:", workflow)
        self.assertNotIn("\n  push:", workflow)
        self.assertIn("promotion_run_id:", workflow)
        self.assertIn("promotion_record_sha256:", workflow)
        self.assertIn("environment: release", workflow)
        self.assertEqual(workflow.count("contents: write"), 1)
        self.assertIn("retained-run", workflow)
        self.assertIn("--kind promotion", workflow)
        self.assertIn("inspect-recovery", workflow)

    def test_recovery_reverifies_retained_candidate_before_and_after_approval(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertEqual(workflow.count("verify-candidate"), 2)
        self.assertGreaterEqual(workflow.count("sha256sum verified/promotion.json"), 1)
        self.assertIn("cmp retained/promotion.json promotion-reverified.json", workflow)
        self.assertIn("cmp verified/promotion.json protected-promotion.json", workflow)
        self.assertIn("retention-days: 1", workflow)
        self.assertIn("retention-days: 90", workflow)
        self.assertEqual(workflow.count("persist-credentials: false"), 2)
        self.assertEqual(workflow.count("ref: ${{ github.sha }}"), 2)

    def test_uppercase_promotion_digest_is_normalized_once_and_reused(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertEqual(workflow.count("${PROMOTION_RECORD_SHA256,,}"), 1)
        normalization = "normalized_digest=${PROMOTION_RECORD_SHA256,,}"
        self.assertIn(normalization, workflow)
        uppercase = "ABCDEF" * 10 + "ABCD"
        result = subprocess.run(
            [
                "bash", "-c",
                f"PROMOTION_RECORD_SHA256={uppercase}; {normalization}; "
                'printf %s "$normalized_digest"',
            ],
            check=True,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.stdout, uppercase.lower())
        self.assertIn(
            "promotion_record_sha256: ${{ steps.identity.outputs.promotion_record_sha256 }}",
            workflow,
        )
        protected = workflow.index("  recover:")
        self.assertNotIn(
            "inputs.promotion_record_sha256", workflow[protected:]
        )
        self.assertGreaterEqual(
            workflow[protected:].count("needs.verify.outputs.promotion_record_sha256"),
            2,
        )

    def test_empty_existing_release_skips_asset_download_but_reaches_planning(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        download = workflow.index("Download every existing asset")
        planning = workflow.index("Compute fail-closed recovery plan")
        step = workflow[download:planning]
        self.assertIn('json.load(open("recovery-release.json")).get("assets")', step)
        self.assertIn("isinstance(assets, list) and assets", step)
        self.assertIn("then\n            gh release download", step)
        self.assertLess(download, planning)

    def test_recovery_never_replaces_or_rebuilds(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        forbidden = (
            "--clobber", "release delete", "git tag -f", "git push --force",
            "cargo build", "makepkg", "build-local-package",
        )
        for value in forbidden:
            self.assertNotIn(value, workflow)
        self.assertIn("upload-assets", workflow)
        self.assertNotIn("create-tag", workflow)
        self.assertIn("create-release", workflow)
        apply_step = workflow[workflow.index("Apply only inspected missing idempotent operations"):]
        self.assertIn("if has_operation create-release; then", apply_step)
        self.assertIn("if (( ${#assets[@]} )); then", apply_step)

    def test_recovery_tests_are_in_ci(self) -> None:
        ci = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        self.assertIn("tools/release/test_recover_release_workflow.py", ci)


if __name__ == "__main__":
    unittest.main()
