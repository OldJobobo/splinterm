#!/usr/bin/env python3
"""Verify and record promotion of one exact Splinterm release candidate."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import sys
from typing import Any

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


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


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
    return {
        "schema": SCHEMA,
        "repository": repository,
        "candidate_run_id": run_id,
        "candidate_manifest_sha256": manifest_sha256,
        "commit": commit,
        "version": version,
        "tag": manifest["tag"],
        "ci": ci,
        "release_notes": "RELEASE-NOTES.md",
        "public_assets": public_assets,
    }


def create_receipt(
    promotion: dict[str, Any],
    release: dict[str, Any],
    ref: dict[str, Any],
    downloads: Path,
    workflow_run: str,
) -> dict[str, Any]:
    if release.get("tagName") != promotion["tag"] or release.get("isDraft") is not False:
        raise ValueError("published release identity or state does not match")
    if release.get("isPrerelease") is not True:
        raise ValueError("alpha release must remain marked prerelease")
    ref_object = ref.get("object")
    if not isinstance(ref_object, dict) or ref_object.get("sha") != promotion["commit"]:
        raise ValueError("published tag does not resolve to the candidate commit")
    assets = release.get("assets")
    if not isinstance(assets, list):
        raise ValueError("published release asset response is malformed")
    expected = promotion.get("public_assets")
    if (
        not isinstance(expected, dict)
        or not expected
        or any(
            not isinstance(name, str)
            or not isinstance(checksum, str)
            or SHA256.fullmatch(checksum) is None
            for name, checksum in expected.items()
        )
    ):
        raise ValueError("promotion public asset checksums are malformed")
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
    return {
        "schema": SCHEMA,
        "state": "published",
        "repository": promotion["repository"],
        "candidate_run_id": promotion["candidate_run_id"],
        "candidate_manifest_sha256": promotion["candidate_manifest_sha256"],
        "commit": promotion["commit"],
        "version": promotion["version"],
        "tag": promotion["tag"],
        "ci": promotion["ci"],
        "release_url": release.get("url"),
        "workflow_run": workflow_run,
        "assets": records,
    }


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
    receipt.add_argument("--output", type=Path, required=True)
    return root


def write_json(path: Path, value: dict[str, Any]) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def main() -> int:
    arguments = parser().parse_args()
    try:
        if arguments.command == "source-run":
            artifact_id, commit = validate_source_run(
                load_json(arguments.run, "candidate workflow run"),
                load_json_value(arguments.artifacts, "candidate artifacts"),
                arguments.repository,
                arguments.run_id,
                arguments.expected_branch,
            )
            print(json.dumps({"artifact_id": artifact_id, "commit": commit}))
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
        else:
            receipt = create_receipt(
                load_json(arguments.promotion, "promotion record"),
                load_json(arguments.release, "published release"),
                load_json(arguments.ref, "published tag ref"),
                arguments.downloads,
                arguments.workflow_run,
            )
            write_json(arguments.output, receipt)
            print(json.dumps(receipt, sort_keys=True))
    except (OSError, ValueError) as error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
