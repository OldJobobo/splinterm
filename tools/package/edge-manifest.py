#!/usr/bin/env python3
"""Create and validate the rolling edge-release package manifest."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import sys
from typing import Any

SCHEMA = 1
COMMIT_PATTERN = re.compile(r"[0-9a-f]{40}")
SHA256_PATTERN = re.compile(r"[0-9a-f]{64}")
ASSET_PATTERN = re.compile(
    r"(splinterm|splinterm-mcp)-([0-9a-f]{40})-x86_64\.pkg\.tar\.zst"
)
PACKAGE_KEYS = ("splinterm", "splinterm-mcp")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def package_record(path: Path, package: str, commit: str) -> dict[str, str]:
    expected_name = f"{package}-{commit}-x86_64.pkg.tar.zst"
    if path.name != expected_name:
        raise ValueError(f"{package} asset must be named {expected_name}")
    return {"asset": path.name, "sha256": sha256(path)}


def create_manifest(
    repository: str,
    commit: str,
    splinterm: Path,
    splinterm_mcp: Path,
) -> dict[str, Any]:
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise ValueError("repository must be an owner/name pair")
    if COMMIT_PATTERN.fullmatch(commit) is None:
        raise ValueError("commit must be a lowercase 40-character Git object ID")
    return {
        "schema": SCHEMA,
        "repository": repository,
        "commit": commit,
        "release": f"edge-{commit}",
        "architecture": "x86_64",
        "packages": {
            "splinterm": package_record(splinterm, "splinterm", commit),
            "splinterm-mcp": package_record(splinterm_mcp, "splinterm-mcp", commit),
        },
    }


def load_manifest(path: Path, repository: str) -> dict[str, Any]:
    try:
        manifest = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ValueError(f"invalid edge manifest: {error}") from error
    if not isinstance(manifest, dict) or set(manifest) != {
        "schema",
        "repository",
        "commit",
        "release",
        "architecture",
        "packages",
    }:
        raise ValueError("edge manifest has an unexpected top-level shape")
    if manifest["schema"] != SCHEMA:
        raise ValueError("edge manifest schema is unsupported")
    if manifest["repository"] != repository:
        raise ValueError("edge manifest repository does not match this installer")
    commit = manifest["commit"]
    if not isinstance(commit, str) or COMMIT_PATTERN.fullmatch(commit) is None:
        raise ValueError("edge manifest commit is malformed")
    if manifest["release"] != f"edge-{commit}":
        raise ValueError("edge manifest release is not commit-bound")
    if manifest["architecture"] != "x86_64":
        raise ValueError("edge manifest architecture is unsupported")
    packages = manifest["packages"]
    if not isinstance(packages, dict) or set(packages) != set(PACKAGE_KEYS):
        raise ValueError("edge manifest package set is not exact")
    for package in PACKAGE_KEYS:
        record = packages[package]
        if not isinstance(record, dict) or set(record) != {"asset", "sha256"}:
            raise ValueError(f"edge manifest {package} record is malformed")
        asset = record["asset"]
        checksum = record["sha256"]
        match = ASSET_PATTERN.fullmatch(asset) if isinstance(asset, str) else None
        if match is None or match.group(1) != package or match.group(2) != commit:
            raise ValueError(f"edge manifest {package} asset is not commit-bound")
        if not isinstance(checksum, str) or SHA256_PATTERN.fullmatch(checksum) is None:
            raise ValueError(f"edge manifest {package} checksum is malformed")
    return manifest


def verify_packages(manifest: dict[str, Any], directory: Path) -> None:
    for package in PACKAGE_KEYS:
        record = manifest["packages"][package]
        path = directory / record["asset"]
        if not path.is_file():
            raise ValueError(f"downloaded {package} package is missing")
        if sha256(path) != record["sha256"]:
            raise ValueError(f"downloaded {package} package checksum does not match")


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description=__doc__)
    commands = root.add_subparsers(dest="command", required=True)

    create = commands.add_parser("create")
    create.add_argument("--repository", required=True)
    create.add_argument("--commit", required=True)
    create.add_argument("--splinterm", type=Path, required=True)
    create.add_argument("--splinterm-mcp", type=Path, required=True)
    create.add_argument("--output", type=Path, required=True)

    inspect = commands.add_parser("inspect")
    inspect.add_argument("--repository", required=True)
    inspect.add_argument("manifest", type=Path)

    verify = commands.add_parser("verify")
    verify.add_argument("--repository", required=True)
    verify.add_argument("manifest", type=Path)
    verify.add_argument("directory", type=Path)
    return root


def main() -> int:
    arguments = parser().parse_args()
    try:
        if arguments.command == "create":
            manifest = create_manifest(
                arguments.repository,
                arguments.commit,
                arguments.splinterm,
                arguments.splinterm_mcp,
            )
            arguments.output.write_text(
                json.dumps(manifest, indent=2, sort_keys=True) + "\n",
                encoding="utf-8",
            )
        elif arguments.command == "inspect":
            manifest = load_manifest(arguments.manifest, arguments.repository)
            print(manifest["commit"])
            print(manifest["release"])
            for package in PACKAGE_KEYS:
                print(manifest["packages"][package]["asset"])
                print(manifest["packages"][package]["sha256"])
        else:
            manifest = load_manifest(arguments.manifest, arguments.repository)
            verify_packages(manifest, arguments.directory)
    except ValueError as error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
