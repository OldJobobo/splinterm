#!/usr/bin/env python3
"""Run semantic fixtures through the patched Foot reference executable."""

from __future__ import annotations

import difflib
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
FIXTURE_DIR = ROOT / "fixtures" / "terminal" / "v1"
DEFAULT_BINARY = Path("/tmp/splinterm-foot-oracle-build/foot")


def canonical(value: Any) -> str:
    return json.dumps(value, indent=2, ensure_ascii=False, sort_keys=True) + "\n"


def run_fixture(binary: Path, fixture_path: Path, output_dir: Path) -> bool:
    fixture = json.loads(fixture_path.read_text(encoding="utf-8"))
    columns = fixture["initial"]["columns"]
    rows = fixture["initial"]["rows"]
    output_path = output_dir / f"{fixture['id']}.json"

    environment = os.environ.copy()
    environment["SPLINTERM_FOOT_STATE_DUMP"] = str(output_path)
    environment["SPLINTERM_FOOT_ORACLE_SIZE"] = f"{columns}x{rows}"

    child = (
        "import os,sys,time; "
        "os.write(1, bytes.fromhex(sys.argv[1])); "
        "time.sleep(0.05)"
    )
    command = [
        str(binary),
        "--config=/dev/null",
        "--override=pad=0x0",
        "--log-level=error",
        sys.executable,
        "-c",
        child,
        fixture["input_hex"],
    ]

    try:
        result = subprocess.run(
            command,
            env=environment,
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
    except subprocess.TimeoutExpired:
        print(f"FAIL {fixture['id']}: Foot timed out", file=sys.stderr)
        return False

    if result.returncode != 0:
        print(
            f"FAIL {fixture['id']}: Foot exited {result.returncode}\n{result.stderr}",
            file=sys.stderr,
        )
        return False
    if not output_path.exists():
        print(f"FAIL {fixture['id']}: Foot produced no state dump", file=sys.stderr)
        return False

    actual = json.loads(output_path.read_text(encoding="utf-8"))
    expected = fixture["expected"]
    if actual != expected:
        difference = difflib.unified_diff(
            canonical(expected).splitlines(keepends=True),
            canonical(actual).splitlines(keepends=True),
            fromfile=f"{fixture['id']}.expected",
            tofile=f"{fixture['id']}.foot",
        )
        print(f"FAIL {fixture['id']}", file=sys.stderr)
        print("".join(difference), file=sys.stderr)
        return False

    print(f"PASS {fixture['id']}")
    return True


def main() -> int:
    if not os.environ.get("WAYLAND_DISPLAY"):
        print("WAYLAND_DISPLAY is unset; the current Foot oracle requires a compositor.", file=sys.stderr)
        return 2

    binary = Path(os.environ.get("FOOT_ORACLE_BINARY", DEFAULT_BINARY))
    if not binary.is_file():
        print(
            f"Foot oracle binary not found: {binary}\n"
            "Run tools/foot-oracle/build-oracle.sh first.",
            file=sys.stderr,
        )
        return 2

    fixtures = sorted(FIXTURE_DIR.glob("*.json"))
    if not fixtures:
        print(f"No fixtures found in {FIXTURE_DIR}", file=sys.stderr)
        return 2

    with tempfile.TemporaryDirectory(prefix="splinterm-foot-oracle-") as directory:
        output_dir = Path(directory)
        passed = sum(run_fixture(binary, fixture, output_dir) for fixture in fixtures)

    print(f"{passed}/{len(fixtures)} fixtures matched Foot.")
    return 0 if passed == len(fixtures) else 1


if __name__ == "__main__":
    raise SystemExit(main())
