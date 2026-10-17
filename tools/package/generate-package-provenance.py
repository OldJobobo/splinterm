#!/usr/bin/env python3
"""Generate one canonical Splinterm package/build provenance record."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re

COMMIT = re.compile(r"[0-9a-f]{40}")
VERSION = re.compile(r"[0-9A-Za-z][0-9A-Za-z.+-]{0,63}")
KINDS = {"local-committed-head", "release-candidate", "nix-store"}
PRIVATE_PROTOCOL = 39
HANDOFF_PROTOCOL = 1
TERMINAL_CHECKPOINT = 2
DESCRIPTOR_MANIFEST = 1


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(64 * 1024):
            digest.update(block)
    return digest.hexdigest()


def pair_identity(commit: str, provenance: str, version: str) -> str:
    if COMMIT.fullmatch(commit) is None or commit == "0" * 40:
        raise ValueError("source commit must be nonzero lowercase hexadecimal")
    if provenance not in KINDS:
        raise ValueError("package provenance kind is unsupported")
    if VERSION.fullmatch(version) is None:
        raise ValueError("build version is malformed")
    digest = hashlib.sha256()
    digest.update(b"splinterm-package-pair-v1\0")
    digest.update(commit.encode("ascii"))
    digest.update(b"\0")
    digest.update(provenance.encode("ascii"))
    digest.update(b"\0")
    digest.update(version.encode("ascii"))
    for value in (
        PRIVATE_PROTOCOL,
        PRIVATE_PROTOCOL,
        HANDOFF_PROTOCOL,
        HANDOFF_PROTOCOL,
        TERMINAL_CHECKPOINT,
        TERMINAL_CHECKPOINT,
        DESCRIPTOR_MANIFEST,
        DESCRIPTOR_MANIFEST,
    ):
        digest.update(value.to_bytes(2, "big"))
    return digest.hexdigest()


def generate(
    commit: str,
    provenance: str,
    version: str,
    daemon: Path,
    client: Path,
) -> dict[str, object]:
    if not daemon.is_file() or not client.is_file():
        raise ValueError("daemon and client must be regular files")
    return {
        "architecture": "x86_64",
        "build_version": version,
        "client_sha256": sha256(client),
        "daemon_sha256": sha256(daemon),
        "package": "splinterm",
        "pair_build_identity": pair_identity(commit, provenance, version),
        "provenance": provenance,
        "schema": 1,
        "source_commit": commit,
    }


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--commit", required=True)
    result.add_argument("--provenance", required=True)
    result.add_argument("--version", required=True)
    result.add_argument("--daemon", required=True, type=Path)
    result.add_argument("--client", required=True, type=Path)
    result.add_argument("--output", required=True, type=Path)
    return result


def main() -> int:
    arguments = parser().parse_args()
    try:
        record = generate(
            arguments.commit,
            arguments.provenance,
            arguments.version,
            arguments.daemon,
            arguments.client,
        )
    except (OSError, ValueError) as error:
        print(error, file=__import__("sys").stderr)
        return 1
    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    arguments.output.write_text(
        json.dumps(record, sort_keys=True, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )
    arguments.output.chmod(0o644)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
