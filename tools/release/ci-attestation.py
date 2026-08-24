#!/usr/bin/env python3
"""Select and attest one exact successful CI/check authority-branch push run."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys
from typing import Any

COMMIT = re.compile(r"[0-9a-f]{40}")
REPOSITORY = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+")
RELEASE_BRANCHES = {"main", "maint/0.1"}
WORKFLOW_PATH = ".github/workflows/ci.yml"


def load(path: Path, label: str) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ValueError(f"invalid {label}: {error}") from error


def pages(value: Any, key: str, label: str) -> list[dict[str, Any]]:
    values = value if isinstance(value, list) else [value]
    records: list[dict[str, Any]] = []
    for page in values:
        if not isinstance(page, dict) or not isinstance(page.get(key), list):
            raise ValueError(f"{label} response is malformed")
        if not all(isinstance(record, dict) for record in page[key]):
            raise ValueError(f"{label} response contains a malformed record")
        records.extend(page[key])
    return records


def validate_inputs(repository: str, commit: str, branch: str) -> None:
    if REPOSITORY.fullmatch(repository) is None:
        raise ValueError("repository must be an owner/name pair")
    if COMMIT.fullmatch(commit) is None:
        raise ValueError("commit must be a lowercase 40-character object ID")
    if branch not in RELEASE_BRANCHES:
        raise ValueError("CI branch is not a release authority")


def run_matches(run: dict[str, Any], repository: str, commit: str, branch: str) -> bool:
    run_repository = run.get("repository")
    return (
        run.get("name") == "CI"
        and run.get("path") == WORKFLOW_PATH
        and run.get("event") == "push"
        and run.get("status") == "completed"
        and run.get("conclusion") == "success"
        and run.get("head_sha") == commit
        and run.get("head_branch") == branch
        and isinstance(run_repository, dict)
        and run_repository.get("full_name", "").lower() == repository.lower()
        and isinstance(run.get("id"), int)
        and run["id"] > 0
    )


def select_run(
    response: Any, repository: str, commit: str, branch: str
) -> dict[str, Any]:
    validate_inputs(repository, commit, branch)
    runs = pages(response, "workflow_runs", "workflow runs")
    matches = [run for run in runs if run_matches(run, repository, commit, branch)]
    if not matches:
        raise ValueError(
            "no completed successful exact-SHA exact-authority-branch CI push run"
        )
    selected = max(matches, key=lambda run: run["id"])
    expected_url = f"https://github.com/{repository}/actions/runs/{selected['id']}"
    if selected.get("html_url", "").lower() != expected_url.lower():
        raise ValueError("selected CI run URL does not match repository and run ID")
    return selected


def attest(
    run: dict[str, Any],
    jobs_response: Any,
    repository: str,
    commit: str,
    branch: str,
    run_id: int,
) -> dict[str, Any]:
    validate_inputs(repository, commit, branch)
    if run.get("id") != run_id or not run_matches(run, repository, commit, branch):
        raise ValueError("CI run metadata does not match the selected exact run")
    selected = select_run({"workflow_runs": [run]}, repository, commit, branch)
    jobs = pages(jobs_response, "jobs", "workflow jobs")
    checks = [job for job in jobs if job.get("name") == "check"]
    if len(checks) != 1:
        raise ValueError("CI run must contain exactly one required check job")
    check = checks[0]
    expected = {
        "run_id": run_id,
        "head_sha": commit,
        "status": "completed",
        "conclusion": "success",
    }
    for key, value in expected.items():
        if check.get(key) != value:
            raise ValueError(f"CI/check job {key} is not {value!r}")
    return {
        "workflow": "CI",
        "workflow_path": WORKFLOW_PATH,
        "event": "push",
        "branch": branch,
        "commit": commit,
        "run_id": run_id,
        "run_url": selected["html_url"],
        "check_job": "check",
        "status": "completed",
        "conclusion": "success",
    }


def write_json(path: Path, value: dict[str, Any]) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__)
    commands = root.add_subparsers(dest="command", required=True)
    select = commands.add_parser("select")
    select.add_argument("--runs", type=Path, required=True)
    attest_parser = commands.add_parser("attest")
    attest_parser.add_argument("--run", type=Path, required=True)
    attest_parser.add_argument("--jobs", type=Path, required=True)
    attest_parser.add_argument("--run-id", type=int, required=True)
    attest_parser.add_argument("--output", type=Path, required=True)
    for command in (select, attest_parser):
        command.add_argument("--repository", required=True)
        command.add_argument("--commit", required=True)
        command.add_argument("--branch", choices=sorted(RELEASE_BRANCHES), required=True)
    return root


def main() -> int:
    arguments = parser().parse_args()
    try:
        if arguments.command == "select":
            selected = select_run(
                load(arguments.runs, "workflow runs"),
                arguments.repository,
                arguments.commit,
                arguments.branch,
            )
            print(selected["id"])
        else:
            record = attest(
                load(arguments.run, "workflow run"),
                load(arguments.jobs, "workflow jobs"),
                arguments.repository,
                arguments.commit,
                arguments.branch,
                arguments.run_id,
            )
            write_json(arguments.output, record)
            print(json.dumps(record, sort_keys=True))
    except (OSError, ValueError) as error:
        print(f"CI attestation error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
