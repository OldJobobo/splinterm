#!/usr/bin/env python3
"""Check one installed package record against what its binaries report.

The record generator and the binaries derive the pair identity separately, so
regenerating the record cannot catch drift between them. This runs both
binaries through the sealed preflight and compares their reported identity
and version with the record.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import struct
import sys

VALIDATOR = Path(__file__).resolve().with_name("validate-package.py")


def sealed_preflight_report(binary: Path, role: int) -> bytes:
    spec = importlib.util.spec_from_file_location("splinterm_validate_package", VALIDATOR)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.sealed_preflight_report(binary, role)


def check(record_path: Path, daemon: Path, client: Path) -> None:
    record = json.loads(record_path.read_text(encoding="utf-8"))
    for binary, key in ((daemon, "daemon_sha256"), (client, "client_sha256")):
        if hashlib.sha256(binary.read_bytes()).hexdigest() != record[key]:
            raise ValueError(f"{binary} does not match the record's {key}")
    daemon_report = sealed_preflight_report(daemon, 1)
    client_report = sealed_preflight_report(client, 2)
    if daemon_report[44:] != client_report[44:]:
        raise ValueError("daemon and client report different builds")
    if daemon_report[44:76].hex() != record["pair_build_identity"]:
        raise ValueError("binaries report a different pair identity than the record")
    release = record["build_version"].split("-", 1)[0]
    if struct.unpack(">HHH", daemon_report[76:82]) != tuple(
        int(part) for part in release.split(".")
    ):
        raise ValueError("binaries report a different version than the record")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("record", type=Path)
    parser.add_argument("--daemon", required=True, type=Path)
    parser.add_argument("--client", required=True, type=Path)
    arguments = parser.parse_args()
    try:
        check(arguments.record, arguments.daemon, arguments.client)
    except (OSError, KeyError, ValueError, AssertionError) as error:
        print(f"package provenance check failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
