from __future__ import annotations

from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/ci.yml"
MANDATORY = (
    "preflight",
    "static",
    "workspace-tests",
    "daemon-tests",
    "mcp-tests",
    "package-automation",
    "oracle-fixtures",
)


class CiWorkflowTests(unittest.TestCase):
    def test_required_check_is_always_run_and_fail_closed(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        check = workflow[workflow.index("  check:") : workflow.index("  oracle:")]
        self.assertIn("if: ${{ always() }}", check)
        for job in MANDATORY:
            self.assertIn(f"- {job}", check)
        failed = " || ".join(
            f"needs.{job}.result != 'success'" for job in MANDATORY
        )
        succeeded = " && ".join(
            f"needs.{job}.result == 'success'" for job in MANDATORY
        )
        self.assertIn(f"if: ${{{{ {failed} }}}}", check)
        self.assertIn(f"if: ${{{{ {succeeded} }}}}", check)

    def test_aggregator_model_rejects_failed_cancelled_and_skipped_dependencies(self) -> None:
        def succeeds(results: dict[str, str]) -> bool:
            return all(results[job] == "success" for job in MANDATORY)

        successful = {job: "success" for job in MANDATORY}
        self.assertTrue(succeeds(successful))
        for result in ("failure", "cancelled", "skipped"):
            with self.subTest(result=result):
                changed = dict(successful)
                changed["daemon-tests"] = result
                self.assertFalse(succeeds(changed))

    def test_each_frozen_rust_job_fetches_the_locked_graph_first(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        jobs = ("static", "workspace-tests", "daemon-tests", "mcp-tests")
        boundaries = (*jobs, "package-automation")
        self.assertEqual(workflow.count("run: cargo fetch --locked"), len(jobs))
        for job, following in zip(jobs, boundaries[1:]):
            block = workflow[
                workflow.index(f"  {job}:") : workflow.index(f"  {following}:")
            ]
            self.assertLess(
                block.index("run: cargo fetch --locked"),
                block.index("cargo ", block.index("run: cargo fetch --locked") + 1),
            )
            self.assertIn("--frozen", block)

    def test_daemon_test_jobs_build_the_required_pty_helper_first(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        helper = "cargo build --frozen -p splinterm-pty --bin splinterm-pty-child"
        self.assertEqual(workflow.count(helper), 2)
        for job, following, test_step in (
            ("workspace-tests", "daemon-tests", "Run workspace unit"),
            ("daemon-tests", "mcp-tests", "Run daemon integration target"),
        ):
            block = workflow[
                workflow.index(f"  {job}:") : workflow.index(f"  {following}:")
            ]
            self.assertLess(block.index(helper), block.index(test_step))

    def test_every_rust_integration_target_is_named_exactly_once(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        targets = []
        for path in (ROOT / "crates").glob("*/tests/*.rs"):
            if "data" not in path.parts:
                targets.append(path.stem)
        self.assertGreater(len(targets), 10)
        for target in sorted(targets):
            with self.subTest(target=target):
                occurrences = re.findall(
                    rf"--test\s+{re.escape(target)}(?=\s|\\|$)", workflow
                )
                self.assertEqual(len(occurrences), 1)
        self.assertIn(
            "cargo test --frozen --workspace --lib --bins --examples", workflow
        )
        self.assertIn("cargo test --frozen --workspace --doc", workflow)

    def test_pkgbuild_check_python_commands_have_exact_ci_equivalents(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        pkgbuild = (ROOT / "packaging/PKGBUILD").read_text(encoding="utf-8")
        check_body = pkgbuild.split("check() {", 1)[1].split("\n}", 1)[0]
        python_targets = re.findall(r"python -m unittest ([^\s]+)", check_body)
        self.assertEqual(len(python_targets), 4)
        for target in python_targets:
            with self.subTest(target=target):
                self.assertEqual(workflow.count(target), 1)
        self.assertIn("cargo test --frozen --workspace", check_body)
        self.assertTrue(
            all("--frozen" in line for line in workflow.splitlines() if "cargo test " in line)
        )
        self.assertTrue(
            all(
                "--test-threads=1" in line or line.rstrip().endswith("\\")
                for line in workflow.splitlines()
                if "cargo test " in line
            )
        )

    def test_timing_sensitive_jobs_upload_logs_without_retry(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("daemon-end-to-end.log", workflow)
        self.assertIn("mcp-protocol.log", workflow)
        self.assertNotIn("continue-on-error", workflow)
        self.assertNotIn("nick-fields/retry", workflow)

    def test_flake_stress_is_manual_scheduled_bounded_and_stops_on_failure(self) -> None:
        workflow = (ROOT / ".github/workflows/flake-stress.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("workflow_dispatch:", workflow)
        self.assertIn("schedule:", workflow)
        self.assertIn("REQUESTED_ITERATIONS <= 25", workflow)
        self.assertIn("set -o pipefail", workflow)
        self.assertNotIn("continue-on-error", workflow)
        self.assertNotIn("nick-fields/retry", workflow)


if __name__ == "__main__":
    unittest.main()
