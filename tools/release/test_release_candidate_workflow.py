from __future__ import annotations

from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/release-candidate.yml"


class ReleaseCandidateWorkflowTests(unittest.TestCase):
    def test_candidate_is_manual_and_read_only(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("workflow_dispatch:", workflow)
        self.assertNotIn("\n  push:", workflow)
        self.assertNotIn("\n  pull_request:", workflow)
        self.assertIn("permissions:\n  actions: read\n  contents: read", workflow)
        self.assertIn("refs/heads/main|refs/heads/maint/0.1", workflow)
        self.assertNotIn("contents: write", workflow)
        self.assertNotIn("environment:", workflow)
        self.assertNotIn("secrets.", workflow)
        self.assertIn("actions/workflows/ci.yml/runs", workflow)
        for query in (
            '-f branch="$GITHUB_REF_NAME"',
            "-f event=push",
            '-f head_sha="${GITHUB_SHA,,}"',
            "-f status=completed",
        ):
            self.assertIn(query, workflow)
        self.assertIn("tools/release/ci-attestation.py attest", workflow)

    def test_candidate_has_no_publication_surface(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        forbidden = (
            "gh release",
            "git push",
            "git tag",
            "release create",
            "aur.archlinux.org",
            "workflow_call:",
            "su builder -c \"cd '$GITHUB_WORKSPACE' && python tools/release/prepare-candidate.py",
            "bash -c \"python tools/release/prepare-candidate.py",
        )
        for value in forbidden:
            self.assertNotIn(value, workflow)
        self.assertIn('manifest["publishable"] is False', workflow)
        self.assertIn("Manifest SHA-256:", workflow)
        self.assertIn("Nothing was published", workflow)

    def test_candidate_validation_is_unprivileged_and_git_is_bounded_safe(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn(
            'git config --global --add safe.directory "$GITHUB_WORKSPACE"', workflow
        )
        self.assertIn("useradd --create-home validator", workflow)
        doctor = workflow.index("python tools/release/release-doctor.py --version")
        candidate_check = workflow.index("prepare-candidate.py check")
        self.assertLess(doctor, candidate_check)
        self.assertNotIn("tools/foot-oracle/check-provenance.py", workflow)
        self.assertIn("runuser -u validator -- python -m unittest", workflow)
        self.assertNotIn("safe.directory '*'", workflow)

    def test_candidate_builds_once_and_retains_review_artifact(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertEqual(workflow.count("build-local-package.sh"), 2)
        self.assertEqual(
            workflow.count("build-local-package.sh --no-check --skip-system-dependency-check"),
            1,
        )
        preflight = workflow.index("prepare-candidate.py check")
        attestation = workflow.index("name: Attest exact successful authority-branch CI push")
        build = workflow.index("name: Build and validate split packages once")
        create = workflow.index("prepare-candidate.py create")
        self.assertLess(preflight, attestation)
        self.assertLess(attestation, build)
        self.assertLess(build, create)
        self.assertIn("runuser -u builder -- python tools/release/prepare-candidate.py create", workflow)
        self.assertIn('--ci-attestation "$GITHUB_WORKSPACE/ci-attestation.json"', workflow)
        self.assertIn("uses: actions/upload-artifact@v4", workflow)
        self.assertIn("include-hidden-files: true", workflow)
        self.assertIn("retention-days: 14", workflow)
        self.assertIn("fetch-depth: 0", workflow)

    def test_candidate_tests_are_part_of_both_authority_branch_checks(self) -> None:
        ci = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        self.assertIn('branches: [main, "maint/0.1"]', ci)
        self.assertIn("tools/release/test_prepare_candidate.py", ci)
        self.assertIn("tools/release/test_release_candidate_workflow.py", ci)


if __name__ == "__main__":
    unittest.main()
