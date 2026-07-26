#!/usr/bin/env python3
"""Run one guarded output case under an explicit scrollback policy."""

from __future__ import annotations

import argparse
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
RUNNER = ROOT / "tools/benchmark/run-graphical-output.py"


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run one guarded scrollback profile case"
    )
    parser.add_argument("output_dir", type=pathlib.Path)
    parser.add_argument(
        "--terminal",
        choices=("splinterm", "foot", "kitty", "ghostty", "alacritty"),
        required=True,
    )
    parser.add_argument("--profile", choices=("disabled", "large"), required=True)
    parser.add_argument("--lines", type=int, default=5000)
    args = parser.parse_args()
    scrollback = 0 if args.profile == "disabled" else 100_000
    inner = args.output_dir / "inner"
    inner.mkdir(parents=True, exist_ok=True)
    completed = subprocess.run(
        [
            sys.executable,
            str(RUNNER),
            str(inner),
            "--terminal",
            args.terminal,
            "--case",
            "plain",
            "--lines",
            str(args.lines),
            "--scrollback-lines",
            str(scrollback),
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
        timeout=30,
    )
    inner_path = inner / f"{args.terminal}-plain.json"
    result = (
        json.loads(inner_path.read_text(encoding="utf-8"))
        if inner_path.exists()
        else {}
    )
    document = {
        "schema": "splinterm.benchmark.scrollback.v1",
        "terminal": args.terminal,
        "scrollback_lines": scrollback,
        "profile": args.profile,
        "result": result,
        "valid": completed.returncode == 0 and bool(result.get("valid")),
    }
    output = args.output_dir / f"{args.terminal}-{args.profile}.json"
    output.write_text(
        json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    __import__("shutil").rmtree(inner, ignore_errors=True)
    print(f"Guarded scrollback result: {output}")
    return 0 if document["valid"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
