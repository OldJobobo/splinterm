#!/usr/bin/env python3
"""Report or atomically refresh Cargo.lock identities in Foot provenance."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import tempfile

ROOT = Path(__file__).resolve().parents[2]
MANIFEST = ROOT / "tools/foot-oracle/provenance.json"
IDENTITY = re.compile(r'("cargo_lock_sha256"\s*:\s*")([0-9a-f]{64})(")')


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def proposed_text(manifest: Path, lockfile: Path) -> tuple[str, list[str], str]:
    text = manifest.read_text(encoding="utf-8")
    # Parse first so a targeted update can never preserve malformed JSON.
    json.loads(text)
    current = [match.group(2) for match in IDENTITY.finditer(text)]
    if not current:
        raise ValueError("provenance has no Cargo.lock identities")
    digest = sha256(lockfile)
    updated, count = IDENTITY.subn(rf"\g<1>{digest}\g<3>", text)
    if count != len(current):
        raise ValueError("could not update every Cargo.lock identity")
    return updated, current, digest


def atomic_write(path: Path, text: str) -> None:
    descriptor, temporary_name = tempfile.mkstemp(
        dir=path.parent, prefix=f".{path.name}.", suffix=".tmp"
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as stream:
            stream.write(text)
            stream.flush()
            os.fsync(stream.fileno())
        os.chmod(temporary, path.stat().st_mode)
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def update(manifest: Path, lockfile: Path, *, write: bool) -> bool:
    updated, current, digest = proposed_text(manifest, lockfile)
    changed = any(value != digest for value in current)
    mode = "updated" if write and changed else "would update" if changed else "already current"
    print(
        f"Cargo.lock provenance {mode}: {len(current)} identities -> {digest}"
    )
    if write and changed:
        atomic_write(manifest, updated)
    elif changed:
        print("dry run; pass --write to update tools/foot-oracle/provenance.json")
    return changed


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--write", action="store_true", help="atomically replace the provenance manifest"
    )
    arguments = parser.parse_args()
    try:
        update(MANIFEST, ROOT / "Cargo.lock", write=arguments.write)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"provenance update error: {error}", file=__import__("sys").stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
