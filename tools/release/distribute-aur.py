#!/usr/bin/env python3
"""Validate and receipt distribution of exact published Splinterm AUR drafts."""

from __future__ import annotations

import argparse
import importlib.util
import json
from pathlib import Path
import re
import sys
from typing import Any

SCRIPT = Path(__file__).with_name("promote-release.py")
SPEC = importlib.util.spec_from_file_location("splinterm_promote_release", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
PROMOTE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROMOTE)

AUR_BASES = {"splinterm": "aur-source", "splinterm-bin": "aur-bin"}
AUR_FILES = {"PKGBUILD", ".SRCINFO", "splinterm.install"}
COMMIT = re.compile(r"[0-9a-f]{40}")
SHA256 = re.compile(r"[0-9a-f]{64}")
SEMVER = re.compile(
    r"(?P<major>[0-9]+)\.(?P<minor>[0-9]+)\.(?P<patch>[0-9]+)"
    r"(?:-(?P<prerelease>[0-9A-Za-z.-]+))?"
)
RELEASE_TAG = re.compile(r"/releases/download/v([0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?)/")


def load_object(path: Path, label: str) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be a JSON object")
    return value


def validate_publication_receipt(
    receipt: dict[str, Any], promotion: dict[str, Any]
) -> None:
    PROMOTE.validate_promotion(promotion)
    required = {
        "schema", "state", "repository", "candidate_run_id",
        "candidate_manifest_sha256", "commit", "version", "tag", "ci",
        "release_title", "release_prerelease", "release_notes_sha256",
        "release_url", "workflow_run", "assets", "promotion_run_id",
        "promotion_record_sha256",
    }
    if set(receipt) != required or receipt.get("schema") != 1 or receipt.get("state") != "published":
        raise ValueError("publication receipt has an unexpected shape")
    for key in (
        "repository", "candidate_run_id", "candidate_manifest_sha256", "commit",
        "version", "tag", "ci", "release_title", "release_prerelease",
        "release_notes_sha256",
    ):
        if receipt.get(key) != promotion.get(key):
            raise ValueError(f"publication receipt {key} differs from the candidate")
    if not isinstance(receipt.get("promotion_run_id"), int) or receipt["promotion_run_id"] <= 0:
        raise ValueError("publication receipt promotion run is malformed")
    if SHA256.fullmatch(receipt.get("promotion_record_sha256", "")) is None:
        raise ValueError("publication receipt promotion digest is malformed")
    assets = receipt.get("assets")
    if not isinstance(assets, list):
        raise ValueError("publication receipt assets are malformed")
    actual: dict[str, str] = {}
    for record in assets:
        if not isinstance(record, dict) or set(record) != {"asset", "sha256"}:
            raise ValueError("publication receipt asset record is malformed")
        name = record["asset"]
        if name in actual or SHA256.fullmatch(record.get("sha256", "")) is None:
            raise ValueError("publication receipt assets are duplicated or malformed")
        actual[name] = record["sha256"]
    if actual != promotion["public_assets"]:
        raise ValueError("publication receipt assets differ from the candidate")


def verify_live_publication(
    receipt: dict[str, Any], promotion: dict[str, Any], release: dict[str, Any],
    ref: dict[str, Any], downloads: Path,
) -> None:
    validate_publication_receipt(receipt, promotion)
    observed = PROMOTE.create_receipt(
        promotion, release, ref, downloads, receipt["workflow_run"],
        promotion_run_id=receipt["promotion_run_id"],
        promotion_record_sha256=receipt["promotion_record_sha256"],
    )
    if observed != receipt:
        raise ValueError("live public release differs from the publication receipt")


def recipe_version(path: Path) -> str:
    matches = re.findall(
        r"^pkgver=([^\n]+)$", path.read_text(encoding="utf-8"), re.MULTILINE
    )
    if len(matches) != 1:
        raise ValueError(f"{path} must define pkgver exactly once")
    return matches[0].strip("'\"")


def srcinfo_version(path: Path) -> str:
    matches = re.findall(
        r"^\s*pkgver\s*=\s*([^\n]+)$", path.read_text(encoding="utf-8"), re.MULTILINE
    )
    if len(matches) != 1:
        raise ValueError(f"{path} must define pkgver exactly once")
    return matches[0].strip()


def recipe_release_version(path: Path) -> str:
    text = path.read_text(encoding="utf-8")
    upstream = re.findall(r"^_upstream_ver=([^\n]+)$", text, re.MULTILINE)
    if len(upstream) > 1:
        raise ValueError(f"{path} must not duplicate _upstream_ver")
    versions = set(RELEASE_TAG.findall(text))
    if upstream:
        versions.add(upstream[0].strip("'\""))
    if len(versions) != 1:
        raise ValueError(f"{path} does not identify exactly one release version")
    version = versions.pop()
    if SEMVER.fullmatch(version) is None:
        raise ValueError(f"{path} release version is not complete SemVer")
    return version


def recipe_identity(directory: Path) -> tuple[str, str]:
    package = directory / "PKGBUILD"
    srcinfo = directory / ".SRCINFO"
    version = recipe_version(package)
    if srcinfo_version(srcinfo) != version:
        raise ValueError("PKGBUILD and .SRCINFO pkgver identities differ")
    release = recipe_release_version(package)
    if recipe_release_version(srcinfo) != release:
        raise ValueError("PKGBUILD and .SRCINFO release identities differ")
    if version != release.replace("-", ""):
        raise ValueError("AUR pkgver differs from its release identity")
    return version, release


def compare_semver(left: str, right: str) -> int:
    def parsed(value: str) -> tuple[tuple[int, int, int], list[tuple[int, int | str]] | None]:
        match = SEMVER.fullmatch(value)
        if match is None:
            raise ValueError(f"release version is not complete SemVer: {value}")
        core = tuple(int(match.group(name)) for name in ("major", "minor", "patch"))
        prerelease = match.group("prerelease")
        if prerelease is None:
            return core, None
        identifiers: list[tuple[int, int | str]] = []
        for identifier in prerelease.split("."):
            identifiers.append(
                (0, int(identifier)) if identifier.isdigit() else (1, identifier)
            )
        return core, identifiers

    left_core, left_pre = parsed(left)
    right_core, right_pre = parsed(right)
    if left_core != right_core:
        return 1 if left_core > right_core else -1
    if left_pre is None or right_pre is None:
        if left_pre is right_pre:
            return 0
        return 1 if left_pre is None else -1
    for left_identifier, right_identifier in zip(left_pre, right_pre):
        if left_identifier != right_identifier:
            return 1 if left_identifier > right_identifier else -1
    if len(left_pre) == len(right_pre):
        return 0
    return 1 if len(left_pre) > len(right_pre) else -1


def exact_recipe_files(directory: Path) -> dict[str, str]:
    actual = {path.name for path in directory.iterdir() if path.is_file() or path.is_symlink()}
    if actual != AUR_FILES:
        raise ValueError(f"AUR recipe file set is not exact: {sorted(actual)}")
    result = {}
    for name in sorted(AUR_FILES):
        path = directory / name
        if not path.is_file() or path.is_symlink():
            raise ValueError(f"AUR recipe file is missing or linked: {name}")
        result[name] = PROMOTE.sha256(path)
    return result


def inspect_aur_state(current: Path, draft: Path) -> dict[str, Any]:
    expected = exact_recipe_files(draft)
    current_files = exact_recipe_files(current)
    version, draft_release = recipe_identity(draft)
    current_version, current_release = recipe_identity(current)
    if current_files == expected:
        return {"state": "already-current", "version": version, "files": expected}
    if current_version == version or compare_semver(draft_release, current_release) <= 0:
        raise ValueError(
            "candidate AUR release is not newer than the differing existing recipe"
        )
    return {"state": "update-required", "version": version, "files": expected}


def create_distribution_receipt(
    publication: dict[str, Any], publication_run_id: int,
    publication_receipt_sha256: str, workflow_run: str,
    package_records: list[dict[str, Any]],
) -> dict[str, Any]:
    if publication_run_id <= 0 or SHA256.fullmatch(publication_receipt_sha256) is None:
        raise ValueError("publication receipt run identity is malformed")
    bases = {record.get("package_base") for record in package_records if isinstance(record, dict)}
    if bases != set(AUR_BASES) or len(package_records) != len(AUR_BASES):
        raise ValueError("distribution receipt package set is not exact")
    for record in package_records:
        if set(record) != {"package_base", "commit", "version", "files"}:
            raise ValueError("distribution package record has an unexpected shape")
        if COMMIT.fullmatch(record.get("commit", "")) is None:
            raise ValueError("distribution package commit is malformed")
        if not isinstance(record.get("files"), dict) or set(record["files"]) != AUR_FILES:
            raise ValueError("distribution package file hashes are not exact")
    return {
        "schema": 1,
        "state": "distributed",
        "repository": publication["repository"],
        "candidate_run_id": publication["candidate_run_id"],
        "candidate_manifest_sha256": publication["candidate_manifest_sha256"],
        "commit": publication["commit"],
        "version": publication["version"],
        "tag": publication["tag"],
        "publication_run_id": publication_run_id,
        "publication_receipt_sha256": publication_receipt_sha256,
        "workflow_run": workflow_run,
        "packages": sorted(package_records, key=lambda value: value["package_base"]),
    }


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__)
    commands = root.add_subparsers(dest="command", required=True)
    source = commands.add_parser("receipt-run")
    source.add_argument("--run", type=Path, required=True)
    source.add_argument("--artifacts", type=Path, required=True)
    source.add_argument("--repository", required=True)
    source.add_argument("--run-id", type=int, required=True)
    source.add_argument("--expected-branch", choices=sorted(PROMOTE.RELEASE_BRANCHES), required=True)
    verify = commands.add_parser("verify-publication")
    verify.add_argument("--receipt", type=Path, required=True)
    verify.add_argument("--promotion", type=Path, required=True)
    verify.add_argument("--release", type=Path, required=True)
    verify.add_argument("--ref", type=Path, required=True)
    verify.add_argument("--downloads", type=Path, required=True)
    inspect = commands.add_parser("inspect-recipe")
    inspect.add_argument("--current", type=Path, required=True)
    inspect.add_argument("--draft", type=Path, required=True)
    inspect.add_argument("--output", type=Path, required=True)
    receipt = commands.add_parser("receipt")
    receipt.add_argument("--publication", type=Path, required=True)
    receipt.add_argument("--publication-run-id", type=int, required=True)
    receipt.add_argument("--publication-receipt-sha256", required=True)
    receipt.add_argument("--workflow-run", required=True)
    receipt.add_argument("--package-record", type=Path, action="append", required=True)
    receipt.add_argument("--output", type=Path, required=True)
    return root


def main() -> int:
    arguments = parser().parse_args()
    try:
        if arguments.command == "receipt-run":
            artifact_inventory = PROMOTE.load_json_value(
                arguments.artifacts, "publication artifacts"
            )
            artifact_id, commit = PROMOTE.validate_retained_artifact(
                load_object(arguments.run, "publication workflow run"),
                artifact_inventory,
                arguments.repository, arguments.run_id, arguments.expected_branch,
                PROMOTE.RECEIPT_WORKFLOWS, r"release-receipt-v.+", {"success"},
            )
            print(json.dumps({
                "artifact_id": artifact_id,
                "artifact_name": PROMOTE.artifact_name_by_id(
                    artifact_inventory, artifact_id
                ),
                "commit": commit,
            }))
        elif arguments.command == "verify-publication":
            verify_live_publication(
                load_object(arguments.receipt, "publication receipt"),
                load_object(arguments.promotion, "promotion record"),
                load_object(arguments.release, "public release"),
                load_object(arguments.ref, "public tag"), arguments.downloads,
            )
            print(json.dumps({"publication": "verified"}))
        elif arguments.command == "inspect-recipe":
            value = inspect_aur_state(arguments.current, arguments.draft)
            PROMOTE.write_json(arguments.output, value)
            print(json.dumps(value, sort_keys=True))
        else:
            value = create_distribution_receipt(
                load_object(arguments.publication, "publication receipt"),
                arguments.publication_run_id, arguments.publication_receipt_sha256,
                arguments.workflow_run,
                [load_object(path, "AUR package record") for path in arguments.package_record],
            )
            PROMOTE.write_json(arguments.output, value)
            print(json.dumps(value, sort_keys=True))
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
