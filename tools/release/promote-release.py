#!/usr/bin/env python3
"""Verify and record promotion of one exact Splinterm release candidate."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import os
import re
import sys
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

SCHEMA = 1
COMMIT = re.compile(r"[0-9a-f]{40}")
SHA256 = re.compile(r"[0-9a-f]{64}")
SEMVER = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?")
REPOSITORY = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+")
RUN_URL = re.compile(r"https://github\.com/([^/]+/[^/]+)/actions/runs/([0-9]+)")
PUBLIC_KINDS = {"source-archive", "arch-package"}
EXPECTED_KINDS = {
    "source-archive": 1,
    "arch-package": 2,
    "aur-source-draft": 3,
    "aur-bin-draft": 3,
    "release-notes-draft": 1,
}
RELEASE_BRANCHES = {"main", "maint/0.1"}
PROMOTION_WORKFLOW = ".github/workflows/promote-release.yml"
RECEIPT_WORKFLOWS = {
    PROMOTION_WORKFLOW,
    ".github/workflows/recover-release.yml",
}
RECOVERY_PROMOTION_CONCLUSIONS = {"failure", "cancelled", "timed_out"}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def sha256_text(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def load_json_value(path: Path, label: str) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ValueError(f"invalid {label}: {error}") from error


def load_json(path: Path, label: str) -> dict[str, Any]:
    value = load_json_value(path, label)
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be a JSON object")
    return value


def safe_relative(value: Any) -> str:
    if not isinstance(value, str):
        raise ValueError("asset path must be a string")
    path = PurePosixPath(value)
    if path.is_absolute() or not path.parts or any(part in ("", ".", "..") for part in path.parts):
        raise ValueError(f"asset path is unsafe: {value}")
    return value


def validate_source_run(
    run: dict[str, Any],
    artifacts: Any,
    repository: str,
    run_id: int,
    expected_branch: str,
) -> tuple[int, str]:
    if REPOSITORY.fullmatch(repository) is None:
        raise ValueError("repository must be an owner/name pair")
    if expected_branch not in RELEASE_BRANCHES:
        raise ValueError("candidate branch is not a release authority")
    expected_run = {
        "id": run_id,
        "event": "workflow_dispatch",
        "status": "completed",
        "conclusion": "success",
        "path": ".github/workflows/release-candidate.yml",
        "head_branch": expected_branch,
    }
    for key, expected in expected_run.items():
        if run.get(key) != expected:
            raise ValueError(f"candidate run {key} is not {expected!r}")
    run_repository = run.get("repository")
    if not isinstance(run_repository, dict) or run_repository.get("full_name") != repository:
        raise ValueError("candidate run repository does not match")
    commit = run.get("head_sha")
    if not isinstance(commit, str) or COMMIT.fullmatch(commit) is None:
        raise ValueError("candidate run commit is malformed")
    pages = artifacts if isinstance(artifacts, list) else [artifacts]
    values = []
    for page in pages:
        if not isinstance(page, dict) or not isinstance(page.get("artifacts"), list):
            raise ValueError("candidate artifact response is malformed")
        values.extend(page["artifacts"])
    unexpired = [
        artifact
        for artifact in values
        if isinstance(artifact, dict) and artifact.get("expired") is False
    ]
    if len(unexpired) != 1:
        raise ValueError("candidate run must expose exactly one unexpired artifact")
    artifact = unexpired[0]
    name = artifact.get("name")
    name_match = re.fullmatch(
        rf"splinterm-({SEMVER.pattern})-candidate-{commit}",
        name if isinstance(name, str) else "",
    )
    workflow_run = artifact.get("workflow_run")
    if (
        name_match is None
        or not isinstance(workflow_run, dict)
        or workflow_run.get("id") != run_id
        or workflow_run.get("head_sha") != commit
    ):
        raise ValueError("candidate artifact identity does not match its source run")
    artifact_id = artifact.get("id")
    if not isinstance(artifact_id, int) or artifact_id <= 0:
        raise ValueError("candidate artifact ID is malformed")
    return artifact_id, commit


def expected_assets(commit: str, version: str) -> dict[str, str]:
    return {
        f"splinterm-{version}.tar.gz": "source-archive",
        f"splinterm-{commit}-x86_64.pkg.tar.zst": "arch-package",
        f"splinterm-mcp-{commit}-x86_64.pkg.tar.zst": "arch-package",
        "aur-source/PKGBUILD": "aur-source-draft",
        "aur-source/.SRCINFO": "aur-source-draft",
        "aur-source/splinterm.install": "aur-source-draft",
        "aur-bin/PKGBUILD": "aur-bin-draft",
        "aur-bin/.SRCINFO": "aur-bin-draft",
        "aur-bin/splinterm.install": "aur-bin-draft",
        "RELEASE-NOTES.md": "release-notes-draft",
    }


def verify_candidate(
    directory: Path,
    repository: str,
    run_id: int,
    expected_commit: str,
    expected_branch: str,
    manifest_sha256: str,
) -> dict[str, Any]:
    if SHA256.fullmatch(manifest_sha256) is None:
        raise ValueError("candidate manifest SHA-256 is malformed")
    manifest_path = directory / "candidate-manifest.json"
    if sha256(manifest_path) != manifest_sha256:
        raise ValueError("candidate manifest SHA-256 does not match approval input")
    manifest = load_json(manifest_path, "candidate manifest")
    expected_keys = {
        "schema", "state", "publishable", "repository", "commit", "version",
        "package_version", "tag", "architecture", "previous_version_tag",
        "workflow_run", "ci", "assets",
    }
    if set(manifest) != expected_keys:
        raise ValueError("candidate manifest has an unexpected top-level shape")
    if manifest["schema"] != SCHEMA or manifest["state"] != "candidate" or manifest["publishable"] is not False:
        raise ValueError("candidate manifest state is invalid")
    if manifest["repository"] != repository:
        raise ValueError("candidate manifest repository does not match")
    commit = manifest["commit"]
    version = manifest["version"]
    if not isinstance(expected_commit, str) or COMMIT.fullmatch(expected_commit) is None:
        raise ValueError("source run commit is malformed")
    if not isinstance(commit, str) or COMMIT.fullmatch(commit) is None:
        raise ValueError("candidate commit is malformed")
    if commit != expected_commit:
        raise ValueError("candidate manifest commit does not match its source run")
    if not isinstance(version, str) or SEMVER.fullmatch(version) is None:
        raise ValueError("candidate version is malformed")
    if manifest["package_version"] != version.replace("-", ""):
        raise ValueError("candidate package version does not match")
    if manifest["tag"] != f"v{version}" or manifest["architecture"] != "x86_64":
        raise ValueError("candidate tag or architecture does not match")
    previous = manifest["previous_version_tag"]
    if previous is not None and (not isinstance(previous, str) or not previous.startswith("v") or SEMVER.fullmatch(previous[1:]) is None):
        raise ValueError("candidate previous version tag is malformed")
    run_match = RUN_URL.fullmatch(manifest["workflow_run"] if isinstance(manifest["workflow_run"], str) else "")
    if run_match is None or run_match.group(1).lower() != repository.lower() or int(run_match.group(2)) != run_id:
        raise ValueError("candidate workflow run does not match approval input")
    ci = manifest["ci"]
    expected_ci = {
        "workflow": "CI",
        "workflow_path": ".github/workflows/ci.yml",
        "event": "push",
        "branch": expected_branch,
        "commit": commit,
        "check_job": "check",
        "status": "completed",
        "conclusion": "success",
    }
    if expected_branch not in RELEASE_BRANCHES:
        raise ValueError("candidate CI branch is not a release authority")
    if not isinstance(ci, dict) or set(ci) != set(expected_ci) | {"run_id", "run_url"}:
        raise ValueError("candidate CI provenance has an unexpected shape")
    for key, expected_value in expected_ci.items():
        if ci.get(key) != expected_value:
            raise ValueError(f"candidate CI provenance {key} does not match")
    ci_run_id = ci.get("run_id")
    ci_match = RUN_URL.fullmatch(ci.get("run_url", ""))
    if (
        not isinstance(ci_run_id, int)
        or ci_run_id <= 0
        or ci_match is None
        or ci_match.group(1).lower() != repository.lower()
        or int(ci_match.group(2)) != ci_run_id
    ):
        raise ValueError("candidate CI provenance run identity is malformed")

    expected = expected_assets(commit, version)
    records = manifest["assets"]
    if not isinstance(records, list) or len(records) != len(expected):
        raise ValueError("candidate asset set is not exact")
    seen: dict[str, dict[str, str]] = {}
    kind_counts = {kind: 0 for kind in EXPECTED_KINDS}
    for record in records:
        if not isinstance(record, dict) or set(record) != {"kind", "path", "sha256"}:
            raise ValueError("candidate asset record is malformed")
        path = safe_relative(record["path"])
        kind = record["kind"]
        checksum = record["sha256"]
        if path in seen or expected.get(path) != kind:
            raise ValueError(f"candidate asset is unexpected or duplicated: {path}")
        if not isinstance(checksum, str) or SHA256.fullmatch(checksum) is None:
            raise ValueError(f"candidate asset checksum is malformed: {path}")
        asset_path = directory / path
        if not asset_path.is_file() or asset_path.is_symlink() or sha256(asset_path) != checksum:
            raise ValueError(f"candidate asset is missing, linked, or changed: {path}")
        seen[path] = record
        kind_counts[kind] += 1
    if kind_counts != EXPECTED_KINDS:
        raise ValueError("candidate asset kinds are not exact")

    expected_files = set(expected) | {"candidate-manifest.json", "SHA256SUMS"}
    actual_files = {
        path.relative_to(directory).as_posix()
        for path in directory.rglob("*")
        if path.is_file() or path.is_symlink()
    }
    if actual_files != expected_files:
        raise ValueError("candidate directory contains missing or unexpected files")
    checksums = (directory / "SHA256SUMS").read_text(encoding="utf-8").splitlines()
    expected_lines = [f"{record['sha256']}  {path}" for path, record in sorted(seen.items())]
    expected_lines.append(f"{manifest_sha256}  candidate-manifest.json")
    if sorted(checksums) != sorted(expected_lines):
        raise ValueError("candidate SHA256SUMS does not close over the artifact")

    public_assets = {
        path: seen[path]["sha256"]
        for path, kind in sorted(expected.items())
        if kind in PUBLIC_KINDS
    }
    public_assets["candidate-manifest.json"] = manifest_sha256
    public_assets["SHA256SUMS"] = sha256(directory / "SHA256SUMS")
    notes_path = directory / "RELEASE-NOTES.md"
    return {
        "schema": SCHEMA,
        "repository": repository,
        "candidate_run_id": run_id,
        "candidate_manifest_sha256": manifest_sha256,
        "commit": commit,
        "version": version,
        "tag": manifest["tag"],
        "ci": ci,
        "release_title": f"Splinterm {version}",
        "release_prerelease": True,
        "release_notes": "RELEASE-NOTES.md",
        "release_notes_sha256": sha256(notes_path),
        "public_assets": public_assets,
    }


def normalize_release(release: dict[str, Any]) -> dict[str, Any]:
    """Normalize gh CLI and REST release shapes without weakening exact checks."""
    return {
        "tag": release.get("tagName", release.get("tag_name")),
        "title": release.get("name"),
        "body": release.get("body"),
        "draft": release.get("isDraft", release.get("draft")),
        "prerelease": release.get("isPrerelease", release.get("prerelease")),
        "url": release.get("url") if "tagName" in release else release.get("html_url"),
        "assets": release.get("assets"),
    }


def validate_promotion(promotion: dict[str, Any]) -> dict[str, str]:
    expected_keys = {
        "schema", "repository", "candidate_run_id", "candidate_manifest_sha256",
        "commit", "version", "tag", "ci", "release_title", "release_prerelease",
        "release_notes", "release_notes_sha256", "public_assets",
    }
    if set(promotion) != expected_keys or promotion.get("schema") != SCHEMA:
        raise ValueError("promotion record has an unexpected shape")
    if promotion.get("release_title") != f"Splinterm {promotion.get('version')}":
        raise ValueError("promotion release title is not exact")
    if promotion.get("release_prerelease") is not True:
        raise ValueError("promotion prerelease state is not exact")
    if promotion.get("release_notes") != "RELEASE-NOTES.md" or SHA256.fullmatch(
        promotion.get("release_notes_sha256", "")
    ) is None:
        raise ValueError("promotion release notes identity is malformed")
    expected = promotion.get("public_assets")
    if (
        not isinstance(expected, dict)
        or not expected
        or any(
            safe_relative(name) != name
            or not isinstance(checksum, str)
            or SHA256.fullmatch(checksum) is None
            for name, checksum in expected.items()
        )
    ):
        raise ValueError("promotion public asset checksums are malformed")
    return expected


def validate_release_metadata(promotion: dict[str, Any], release: dict[str, Any]) -> dict[str, Any]:
    normalized = normalize_release(release)
    if normalized["tag"] != promotion["tag"] or normalized["draft"] is not False:
        raise ValueError("published release identity or state does not match")
    if normalized["title"] != promotion["release_title"]:
        raise ValueError("published release title differs from the promotion record")
    if normalized["prerelease"] is not promotion["release_prerelease"]:
        raise ValueError("published release prerelease state differs from the promotion record")
    body = normalized["body"]
    if not isinstance(body, str) or sha256_text(body) != promotion["release_notes_sha256"]:
        raise ValueError("published release notes differ from the promotion record")
    if not isinstance(normalized["assets"], list):
        raise ValueError("published release asset response is malformed")
    return normalized


def create_receipt(
    promotion: dict[str, Any],
    release: dict[str, Any],
    ref: dict[str, Any],
    downloads: Path,
    workflow_run: str,
    *,
    promotion_run_id: int | None = None,
    promotion_record_sha256: str | None = None,
) -> dict[str, Any]:
    expected = validate_promotion(promotion)
    normalized = validate_release_metadata(promotion, release)
    ref_object = ref.get("object")
    if not isinstance(ref_object, dict) or ref_object.get("sha") != promotion["commit"]:
        raise ValueError("published tag does not resolve to the candidate commit")
    assets = normalized["assets"]
    names = [asset.get("name") for asset in assets if isinstance(asset, dict)]
    if set(names) != set(expected) or len(names) != len(expected):
        raise ValueError("published release asset set is not exact")
    records = []
    for name, expected_sha256 in sorted(expected.items()):
        path = downloads / name
        if not path.is_file() or path.is_symlink():
            raise ValueError(f"published asset download is unavailable: {name}")
        actual_sha256 = sha256(path)
        if actual_sha256 != expected_sha256:
            raise ValueError(f"published asset differs from approved candidate: {name}")
        records.append({"asset": name, "sha256": actual_sha256})
    receipt = {
        "schema": SCHEMA,
        "state": "published",
        "repository": promotion["repository"],
        "candidate_run_id": promotion["candidate_run_id"],
        "candidate_manifest_sha256": promotion["candidate_manifest_sha256"],
        "commit": promotion["commit"],
        "version": promotion["version"],
        "tag": promotion["tag"],
        "ci": promotion["ci"],
        "release_title": promotion["release_title"],
        "release_prerelease": promotion["release_prerelease"],
        "release_notes_sha256": promotion["release_notes_sha256"],
        "release_url": normalized["url"],
        "workflow_run": workflow_run,
        "assets": records,
    }
    if promotion_run_id is not None or promotion_record_sha256 is not None:
        if not isinstance(promotion_run_id, int) or promotion_run_id <= 0:
            raise ValueError("promotion run ID is malformed")
        if not isinstance(promotion_record_sha256, str) or SHA256.fullmatch(promotion_record_sha256) is None:
            raise ValueError("promotion record SHA-256 is malformed")
        receipt["promotion_run_id"] = promotion_run_id
        receipt["promotion_record_sha256"] = promotion_record_sha256
    return receipt


def validate_environment(environment: dict[str, Any], branch_pages: Any, name: str) -> None:
    if environment.get("name") != name:
        raise ValueError(f"{name} environment response does not match")
    rules = environment.get("protection_rules")
    if not isinstance(rules, list):
        raise ValueError(f"{name} environment protection rules are unavailable")
    reviewer_rules = [rule for rule in rules if isinstance(rule, dict) and rule.get("type") == "required_reviewers"]
    if len(reviewer_rules) != 1 or not reviewer_rules[0].get("reviewers"):
        raise ValueError(f"{name} environment requires at least one reviewer")
    if environment.get("deployment_branch_policy") != {
        "protected_branches": False,
        "custom_branch_policies": True,
    }:
        raise ValueError(f"{name} environment requires custom deployment branch policies")
    pages = branch_pages if isinstance(branch_pages, list) else [branch_pages]
    policies = [
        item
        for page in pages
        if isinstance(page, dict)
        for item in page.get("branch_policies", [])
        if isinstance(item, dict)
    ]
    names = [item.get("name") for item in policies]
    if sorted(names) != ["main", "maint/0.1"] or len(names) != 2:
        raise ValueError(f"{name} environment must permit exactly main and maint/0.1")


def artifact_name_by_id(artifacts: Any, artifact_id: int) -> str:
    pages = artifacts if isinstance(artifacts, list) else [artifacts]
    names = [
        item.get("name")
        for page in pages if isinstance(page, dict)
        for item in page.get("artifacts", []) if isinstance(item, dict)
        if item.get("id") == artifact_id
    ]
    if len(names) != 1 or not isinstance(names[0], str) or not names[0]:
        raise ValueError("validated artifact name is unavailable")
    return names[0]


def validate_retained_artifact(
    run: dict[str, Any], artifacts: Any, repository: str, run_id: int,
    expected_branch: str, workflow_paths: set[str], artifact_pattern: str,
    allowed_conclusions: set[str],
) -> tuple[int, str]:
    expected = {
        "id": run_id, "event": "workflow_dispatch", "status": "completed",
        "head_branch": expected_branch,
    }
    for key, value in expected.items():
        if run.get(key) != value:
            raise ValueError(f"source run {key} is not {value!r}")
    if run.get("conclusion") not in allowed_conclusions or run.get("path") not in workflow_paths:
        raise ValueError("source run workflow or conclusion is not allowed")
    run_repository = run.get("repository")
    if not isinstance(run_repository, dict) or run_repository.get("full_name") != repository:
        raise ValueError("source run repository does not match")
    commit = run.get("head_sha")
    if not isinstance(commit, str) or COMMIT.fullmatch(commit) is None:
        raise ValueError("source run commit is malformed")
    pages = artifacts if isinstance(artifacts, list) else [artifacts]
    values = [
        item for page in pages if isinstance(page, dict)
        for item in page.get("artifacts", []) if isinstance(item, dict)
    ]
    matches = [
        item for item in values
        if item.get("expired") is False
        and re.fullmatch(artifact_pattern, item.get("name", ""))
        and isinstance(item.get("workflow_run"), dict)
        and item["workflow_run"].get("id") == run_id
        and item["workflow_run"].get("head_sha") == commit
    ]
    if len(matches) != 1:
        raise ValueError("source run must expose exactly one matching unexpired artifact")
    artifact_id = matches[0].get("id")
    if not isinstance(artifact_id, int) or artifact_id <= 0:
        raise ValueError("source artifact ID is malformed")
    return artifact_id, commit


def inspect_recovery(
    promotion: dict[str, Any], ref: dict[str, Any] | None,
    release: dict[str, Any] | None, downloads: Path,
) -> dict[str, Any]:
    expected = validate_promotion(promotion)
    operations: list[dict[str, Any]] = []
    if ref is None:
        if release is None:
            raise ValueError("no partial publication exists; use normal promotion")
        raise ValueError("release exists while its versioned tag is missing")
    else:
        ref_object = ref.get("object")
        if not isinstance(ref_object, dict) or ref_object.get("sha") != promotion["commit"]:
            raise ValueError("existing tag target differs from the promotion record")
    if release is None:
        operations.append({"operation": "create-release"})
        missing = sorted(expected)
    else:
        normalized = validate_release_metadata(promotion, release)
        names = [asset.get("name") for asset in normalized["assets"] if isinstance(asset, dict)]
        if len(names) != len(set(names)) or not set(names).issubset(expected):
            raise ValueError("existing release contains duplicate or extra assets")
        for name in names:
            path = downloads / name
            if not path.is_file() or path.is_symlink() or sha256(path) != expected[name]:
                raise ValueError(f"existing release asset differs from the promotion record: {name}")
        missing = sorted(set(expected) - set(names))
    if missing:
        operations.append({"operation": "upload-assets", "assets": missing})
    state = "recoverable" if operations else "receipt-only"
    return {"schema": SCHEMA, "state": state, "tag": promotion["tag"], "operations": operations}


def github_get(repository: str, endpoint: str, *, allow_not_found: bool = False) -> dict[str, Any] | None:
    token = os.environ.get("GH_TOKEN")
    if not token:
        raise ValueError("GH_TOKEN is required")
    request = Request(
        f"https://api.github.com/repos/{repository}/{endpoint}",
        headers={"Accept": "application/vnd.github+json", "Authorization": f"Bearer {token}", "X-GitHub-Api-Version": "2022-11-28"},
    )
    try:
        with urlopen(request, timeout=30) as response:
            value = json.load(response)
    except HTTPError as error:
        if allow_not_found and error.code == 404:
            return None
        raise ValueError(f"GitHub API {endpoint} returned HTTP {error.code}") from error
    except (URLError, TimeoutError, json.JSONDecodeError) as error:
        raise ValueError(f"GitHub API {endpoint} failed: {error}") from error
    if not isinstance(value, dict):
        raise ValueError(f"GitHub API {endpoint} returned a non-object")
    return value


def fetch_release_state(repository: str, tag: str, ref_output: Path, release_output: Path) -> tuple[bool, bool]:
    if REPOSITORY.fullmatch(repository) is None or not tag.startswith("v") or SEMVER.fullmatch(tag[1:]) is None:
        raise ValueError("release repository or tag is malformed")
    ref = github_get(repository, f"git/ref/tags/{tag}", allow_not_found=True)
    release = github_get(repository, f"releases/tags/{tag}", allow_not_found=True)
    ref_output.write_text(json.dumps(ref, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    release_output.write_text(json.dumps(release, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return ref is not None, release is not None


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__)
    commands = root.add_subparsers(dest="command", required=True)
    source = commands.add_parser("source-run")
    source.add_argument("--run", type=Path, required=True)
    source.add_argument("--artifacts", type=Path, required=True)
    source.add_argument("--repository", required=True)
    source.add_argument("--run-id", type=int, required=True)
    source.add_argument(
        "--expected-branch", choices=sorted(RELEASE_BRANCHES), required=True
    )
    verify = commands.add_parser("verify-candidate")
    verify.add_argument("--directory", type=Path, required=True)
    verify.add_argument("--repository", required=True)
    verify.add_argument("--run-id", type=int, required=True)
    verify.add_argument("--expected-commit", required=True)
    verify.add_argument(
        "--expected-branch", choices=sorted(RELEASE_BRANCHES), required=True
    )
    verify.add_argument("--manifest-sha256", required=True)
    verify.add_argument("--output", type=Path, required=True)
    receipt = commands.add_parser("receipt")
    receipt.add_argument("--promotion", type=Path, required=True)
    receipt.add_argument("--release", type=Path, required=True)
    receipt.add_argument("--ref", type=Path, required=True)
    receipt.add_argument("--downloads", type=Path, required=True)
    receipt.add_argument("--workflow-run", required=True)
    receipt.add_argument("--promotion-run-id", type=int)
    receipt.add_argument("--promotion-record-sha256")
    receipt.add_argument("--output", type=Path, required=True)
    environment = commands.add_parser("environment")
    environment.add_argument("--environment", type=Path, required=True)
    environment.add_argument("--branch-policies", type=Path, required=True)
    environment.add_argument("--name", choices=("release", "aur-release"), required=True)
    retained = commands.add_parser("retained-run")
    retained.add_argument("--run", type=Path, required=True)
    retained.add_argument("--artifacts", type=Path, required=True)
    retained.add_argument("--repository", required=True)
    retained.add_argument("--run-id", type=int, required=True)
    retained.add_argument("--expected-branch", choices=sorted(RELEASE_BRANCHES), required=True)
    retained.add_argument("--kind", choices=("promotion", "receipt"), required=True)
    inspect = commands.add_parser("inspect-recovery")
    inspect.add_argument("--promotion", type=Path, required=True)
    inspect.add_argument("--ref", type=Path, required=True)
    inspect.add_argument("--release", type=Path, required=True)
    inspect.add_argument("--downloads", type=Path, required=True)
    inspect.add_argument("--output", type=Path, required=True)
    absent = commands.add_parser("require-absent")
    absent.add_argument("--ref", type=Path, required=True)
    absent.add_argument("--release", type=Path, required=True)
    remote = commands.add_parser("fetch-release-state")
    remote.add_argument("--repository", required=True)
    remote.add_argument("--tag", required=True)
    remote.add_argument("--ref-output", type=Path, required=True)
    remote.add_argument("--release-output", type=Path, required=True)
    remote.add_argument("--github-output", type=Path)
    return root


def write_json(path: Path, value: dict[str, Any]) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def main() -> int:
    arguments = parser().parse_args()
    try:
        if arguments.command == "source-run":
            artifact_inventory = load_json_value(arguments.artifacts, "candidate artifacts")
            artifact_id, commit = validate_source_run(
                load_json(arguments.run, "candidate workflow run"),
                artifact_inventory,
                arguments.repository,
                arguments.run_id,
                arguments.expected_branch,
            )
            print(json.dumps({
                "artifact_id": artifact_id,
                "artifact_name": artifact_name_by_id(artifact_inventory, artifact_id),
                "commit": commit,
            }))
        elif arguments.command == "verify-candidate":
            promotion = verify_candidate(
                arguments.directory,
                arguments.repository,
                arguments.run_id,
                arguments.expected_commit,
                arguments.expected_branch,
                arguments.manifest_sha256,
            )
            write_json(arguments.output, promotion)
            print(json.dumps(promotion, sort_keys=True))
        elif arguments.command == "receipt":
            receipt = create_receipt(
                load_json(arguments.promotion, "promotion record"),
                load_json(arguments.release, "published release"),
                load_json(arguments.ref, "published tag ref"),
                arguments.downloads,
                arguments.workflow_run,
                promotion_run_id=arguments.promotion_run_id,
                promotion_record_sha256=arguments.promotion_record_sha256,
            )
            write_json(arguments.output, receipt)
            print(json.dumps(receipt, sort_keys=True))
        elif arguments.command == "environment":
            validate_environment(
                load_json(arguments.environment, "environment"),
                load_json_value(arguments.branch_policies, "branch policies"),
                arguments.name,
            )
            print(json.dumps({"environment": arguments.name, "protected": True}))
        elif arguments.command == "retained-run":
            if arguments.kind == "promotion":
                workflows = {PROMOTION_WORKFLOW}
                pattern = r"verified-release-candidate-[1-9][0-9]*"
                conclusions = RECOVERY_PROMOTION_CONCLUSIONS
            else:
                workflows = RECEIPT_WORKFLOWS
                pattern = r"release-receipt-v.+"
                conclusions = {"success"}
            artifact_inventory = load_json_value(arguments.artifacts, "source artifacts")
            artifact_id, commit = validate_retained_artifact(
                load_json(arguments.run, "source workflow run"),
                artifact_inventory,
                arguments.repository,
                arguments.run_id,
                arguments.expected_branch,
                workflows,
                pattern,
                conclusions,
            )
            print(json.dumps({
                "artifact_id": artifact_id,
                "artifact_name": artifact_name_by_id(artifact_inventory, artifact_id),
                "commit": commit,
            }))
        elif arguments.command == "inspect-recovery":
            ref_value = load_json_value(arguments.ref, "tag state")
            release_value = load_json_value(arguments.release, "release state")
            if ref_value is not None and not isinstance(ref_value, dict):
                raise ValueError("tag state must be an object or null")
            if release_value is not None and not isinstance(release_value, dict):
                raise ValueError("release state must be an object or null")
            plan = inspect_recovery(
                load_json(arguments.promotion, "promotion record"),
                ref_value,
                release_value,
                arguments.downloads,
            )
            write_json(arguments.output, plan)
            print(json.dumps(plan, sort_keys=True))
        elif arguments.command == "require-absent":
            ref_value = load_json_value(arguments.ref, "tag state")
            release_value = load_json_value(arguments.release, "release state")
            if ref_value is not None or release_value is not None:
                raise ValueError("refusing to replace an existing tag or release")
            print(json.dumps({"tag_absent": True, "release_absent": True}))
        else:
            ref_exists, release_exists = fetch_release_state(
                arguments.repository, arguments.tag,
                arguments.ref_output, arguments.release_output,
            )
            state = {"ref_exists": ref_exists, "release_exists": release_exists}
            if arguments.github_output is not None:
                with arguments.github_output.open("a", encoding="utf-8") as stream:
                    for key, value in state.items():
                        stream.write(f"{key}={str(value).lower()}\n")
            print(json.dumps(state))
    except (OSError, ValueError) as error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
