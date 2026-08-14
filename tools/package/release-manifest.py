#!/usr/bin/env python3
"""Select and validate immutable versioned Splinterm release manifests."""

from __future__ import annotations

import argparse
from datetime import datetime
import json
from pathlib import Path
import re
import sys
from typing import Any

REPOSITORY = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+")
SEMVER_TAG = re.compile(r"v([0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?)")
COMMIT = re.compile(r"[0-9a-f]{40}")
SHA256 = re.compile(r"[0-9a-f]{64}")


def load_json(path: Path) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read JSON from {path}: {error}") from error


def parse_repository(value: str) -> str:
    if REPOSITORY.fullmatch(value) is None:
        raise ValueError("repository must be an owner/name pair")
    return value


def select_release(repository: str, path: Path) -> list[str]:
    repository = parse_repository(repository)
    releases = load_json(path)
    if not isinstance(releases, list):
        raise ValueError("GitHub release index must be an array")

    candidates: list[tuple[datetime, str, str]] = []
    expected_prefix = f"https://github.com/{repository}/releases/download/"
    for release in releases:
        if not isinstance(release, dict) or release.get("draft") is not False:
            continue
        tag = release.get("tag_name")
        if not isinstance(tag, str) or SEMVER_TAG.fullmatch(tag) is None:
            continue
        published = release.get("published_at")
        if not isinstance(published, str):
            continue
        try:
            published_at = datetime.fromisoformat(published.replace("Z", "+00:00"))
        except ValueError:
            continue
        assets = release.get("assets")
        if not isinstance(assets, list):
            continue
        manifests = [
            asset
            for asset in assets
            if isinstance(asset, dict) and asset.get("name") == "candidate-manifest.json"
        ]
        if len(manifests) != 1:
            continue
        digest = manifests[0].get("digest")
        url = manifests[0].get("browser_download_url")
        if not isinstance(digest, str) or not digest.startswith("sha256:"):
            continue
        checksum = digest.removeprefix("sha256:")
        if SHA256.fullmatch(checksum) is None:
            continue
        if not isinstance(url, str) or not url.startswith(f"{expected_prefix}{tag}/"):
            continue
        candidates.append((published_at, tag, checksum))

    if not candidates:
        raise ValueError("no published versioned release has a closed candidate manifest")
    _, tag, checksum = max(candidates, key=lambda candidate: candidate[0])
    return [tag, checksum]


def inspect_manifest(repository: str, tag: str, path: Path) -> list[str]:
    repository = parse_repository(repository)
    tag_match = SEMVER_TAG.fullmatch(tag)
    if tag_match is None:
        raise ValueError("release tag is not a complete versioned SemVer tag")
    manifest = load_json(path)
    if not isinstance(manifest, dict):
        raise ValueError("release manifest must be an object")
    if manifest.get("schema") != 1:
        raise ValueError("release manifest schema is unsupported")
    if manifest.get("state") != "candidate" or manifest.get("publishable") is not False:
        raise ValueError("release manifest does not preserve the reviewed candidate identity")
    if manifest.get("repository") != repository:
        raise ValueError("release manifest repository does not match this installer")
    if manifest.get("tag") != tag or manifest.get("version") != tag_match.group(1):
        raise ValueError("release manifest version and tag do not match the selected release")
    if manifest.get("architecture") != "x86_64":
        raise ValueError("release manifest architecture is unsupported")
    commit = manifest.get("commit")
    if not isinstance(commit, str) or COMMIT.fullmatch(commit) is None:
        raise ValueError("release manifest commit is malformed")

    expected = {
        f"splinterm-{commit}-x86_64.pkg.tar.zst": None,
        f"splinterm-mcp-{commit}-x86_64.pkg.tar.zst": None,
    }
    assets = manifest.get("assets")
    if not isinstance(assets, list):
        raise ValueError("release manifest assets are malformed")
    packages = [asset for asset in assets if isinstance(asset, dict) and asset.get("kind") == "arch-package"]
    if len(packages) != 2:
        raise ValueError("release manifest package set is not exact")
    for package in packages:
        name = package.get("path")
        checksum = package.get("sha256")
        if name not in expected or not isinstance(checksum, str) or SHA256.fullmatch(checksum) is None:
            raise ValueError("release manifest package record is malformed")
        if expected[name] is not None:
            raise ValueError("release manifest contains a duplicate package")
        expected[name] = checksum
    if any(checksum is None for checksum in expected.values()):
        raise ValueError("release manifest package set is incomplete")

    main = f"splinterm-{commit}-x86_64.pkg.tar.zst"
    mcp = f"splinterm-mcp-{commit}-x86_64.pkg.tar.zst"
    return [commit, tag, tag_match.group(1), main, expected[main], mcp, expected[mcp]]


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    commands = result.add_subparsers(dest="command", required=True)
    select = commands.add_parser("select")
    select.add_argument("--repository", required=True)
    select.add_argument("index", type=Path)
    inspect = commands.add_parser("inspect")
    inspect.add_argument("--repository", required=True)
    inspect.add_argument("--tag", required=True)
    inspect.add_argument("manifest", type=Path)
    return result


def main() -> int:
    arguments = parser().parse_args()
    try:
        if arguments.command == "select":
            output = select_release(arguments.repository, arguments.index)
        else:
            output = inspect_manifest(arguments.repository, arguments.tag, arguments.manifest)
    except ValueError as error:
        print(error, file=sys.stderr)
        return 1
    print("\n".join(output))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
