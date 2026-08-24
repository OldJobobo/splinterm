from __future__ import annotations

import copy
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/release/ci-attestation.py"
FIXTURES = ROOT / "tools/release/fixtures"
SPEC = importlib.util.spec_from_file_location("ci_attestation", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)

REPOSITORY = "OldJobobo/splinterm"
COMMIT = "a" * 40
BRANCH = "main"
RUN_ID = 4242


def fixture(name: str):
    return json.loads((FIXTURES / name).read_text(encoding="utf-8"))


class CiAttestationTests(unittest.TestCase):
    def test_success_fixture_attests_exact_push_run_and_check_job(self) -> None:
        selected = MODULE.select_run(
            fixture("ci-runs-success.json"), REPOSITORY, COMMIT, BRANCH
        )
        record = MODULE.attest(
            selected,
            fixture("ci-jobs-success.json"),
            REPOSITORY,
            COMMIT,
            BRANCH,
            RUN_ID,
        )
        self.assertEqual(record["run_id"], RUN_ID)
        self.assertEqual(record["commit"], COMMIT)
        self.assertEqual(record["branch"], BRANCH)
        self.assertEqual(record["check_job"], "check")

    def test_missing_pending_failed_pull_request_and_stale_runs_are_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "no completed successful"):
            MODULE.select_run({"workflow_runs": []}, REPOSITORY, COMMIT, BRANCH)
        with self.assertRaisesRegex(ValueError, "no completed successful"):
            MODULE.select_run(
                fixture("ci-runs-rejected.json"), REPOSITORY, COMMIT, BRANCH
            )

    def test_mismatched_branch_commit_repository_or_run_id_is_rejected(self) -> None:
        run = fixture("ci-runs-success.json")["workflow_runs"][0]
        for key, value in (
            ("head_branch", "maint/0.1"),
            ("head_sha", "b" * 40),
            ("id", 999),
        ):
            with self.subTest(key=key):
                changed = copy.deepcopy(run)
                changed[key] = value
                with self.assertRaises(ValueError):
                    MODULE.attest(
                        changed,
                        fixture("ci-jobs-success.json"),
                        REPOSITORY,
                        COMMIT,
                        BRANCH,
                        RUN_ID,
                    )
        changed = copy.deepcopy(run)
        changed["repository"]["full_name"] = "Other/project"
        with self.assertRaises(ValueError):
            MODULE.attest(
                changed,
                fixture("ci-jobs-success.json"),
                REPOSITORY,
                COMMIT,
                BRANCH,
                RUN_ID,
            )

    def test_cancelled_skipped_or_duplicate_check_jobs_fail_closed(self) -> None:
        run = fixture("ci-runs-success.json")["workflow_runs"][0]
        for conclusion in ("failure", "cancelled", "skipped"):
            with self.subTest(conclusion=conclusion):
                jobs = fixture("ci-jobs-success.json")
                jobs["jobs"][0]["conclusion"] = conclusion
                with self.assertRaisesRegex(ValueError, "conclusion"):
                    MODULE.attest(
                        run, jobs, REPOSITORY, COMMIT, BRANCH, RUN_ID
                    )
        jobs = fixture("ci-jobs-success.json")
        jobs["jobs"].append(copy.deepcopy(jobs["jobs"][0]))
        with self.assertRaisesRegex(ValueError, "exactly one"):
            MODULE.attest(run, jobs, REPOSITORY, COMMIT, BRANCH, RUN_ID)


if __name__ == "__main__":
    unittest.main()
