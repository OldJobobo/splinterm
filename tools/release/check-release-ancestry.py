#!/usr/bin/env python3
"""Require every published release tag to be an ancestor of the checked-out commit.

Maintenance releases reach main by merging maint/0.1, never by squash or
cherry-pick. A release tag that is missing from main's history means a
maintenance release has not been merged forward.
"""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
SEMVER = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?")


def git(root: Path, *arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", *arguments], cwd=root, text=True, capture_output=True, check=False
    )


def release_tags(root: Path) -> list[str]:
    listed = git(root, "tag", "--list", "v*")
    if listed.returncode != 0:
        raise ValueError(f"cannot list release tags: {listed.stderr.strip()}")
    return sorted(
        tag for tag in listed.stdout.split() if SEMVER.fullmatch(tag[1:]) is not None
    )


def missing_release_tags(root: Path, revision: str = "HEAD") -> list[str]:
    tags = release_tags(root)
    if not tags:
        raise ValueError("no release tags are available; fetch full history and tags")
    missing = []
    for tag in tags:
        result = git(root, "merge-base", "--is-ancestor", f"refs/tags/{tag}", revision)
        if result.returncode == 1:
            missing.append(tag)
        elif result.returncode != 0:
            raise ValueError(f"cannot compare {tag} with {revision}: {result.stderr.strip()}")
    return missing


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--revision", default="HEAD")
    arguments = parser.parse_args()
    try:
        missing = missing_release_tags(ROOT, arguments.revision)
    except ValueError as error:
        print(f"release ancestry: {error}", file=sys.stderr)
        return 1
    if missing:
        print(
            "release ancestry: these release tags are not ancestors of "
            f"{arguments.revision}: {', '.join(missing)}. Merge the maintenance "
            "branch into main with a merge commit before landing other work.",
            file=sys.stderr,
        )
        return 1
    print("release ancestry: every release tag is an ancestor")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
