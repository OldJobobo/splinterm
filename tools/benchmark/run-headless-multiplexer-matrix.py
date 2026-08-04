#!/usr/bin/env python3
"""Run the disposable three-implementation headless topology matrix."""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import pathlib
import random
import subprocess
import sys
from typing import Any

from manifest import collect

ROOT = pathlib.Path(__file__).resolve().parents[2]
CASE_RUNNER = pathlib.Path(__file__).with_name("run-headless-multiplexer.py")
IMPLEMENTATIONS = ("splinterm", "tmux", "zellij")
TOPOLOGIES = ("single", "two-columns", "four-grid")
IMPLEMENTATION_FILES = (
    "tools/benchmark/headless_multiplexer.py",
    "tools/benchmark/run-headless-multiplexer.py",
    "tools/benchmark/run-headless-multiplexer-matrix.py",
    "tools/benchmark/headless-multiplexer-schema.json",
    "tools/benchmark/metrics.py",
    "tools/benchmark/multiplexing.py",
    "tools/benchmark/multiplexers/base.py",
    "tools/benchmark/multiplexers/tmux.py",
    "tools/benchmark/multiplexers/zellij.py",
    "tools/benchmark/profiles/splinterm.ini",
    "tools/benchmark/profiles/tmux.conf",
    "tools/benchmark/profiles/zellij.kdl",
    "tools/benchmark/workloads/bench-child.py",
)


def atomic_json(path: pathlib.Path, value: Any) -> None:
    temporary = path.with_name(f".{path.name}.tmp")
    temporary.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    temporary.replace(path)


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_checksums(output: pathlib.Path) -> None:
    paths = sorted(
        path
        for path in output.rglob("*")
        if path.is_file() and path.name != "SHA256SUMS"
    )
    (output / "SHA256SUMS").write_text(
        "".join(f"{sha256(path)}  {path.relative_to(output)}\n" for path in paths),
        encoding="utf-8",
    )


def markdown(records: list[dict[str, Any]], seed: int) -> str:
    lines = [
        "# Splinterbench headless multiplexer orchestration matrix",
        "",
        f"Randomization seed: {seed}",
        "",
        "This is non-graphical orchestration and cleanup evidence, not a performance ranking.",
        "No terminal window was launched. Ambient sessions are represented only by counts.",
        "",
        "| Implementation | Topology | Panes | All children ready | Cleanup | Result |",
        "|---|---|---:|---:|---|---|",
    ]
    for record in sorted(
        records,
        key=lambda item: (
            IMPLEMENTATIONS.index(item["implementation"]),
            TOPOLOGIES.index(item["topology"]),
        ),
    ):
        report = record.get("report") or {}
        duration = report.get("boundary", {}).get("launch_to_all_children_ready_ns")
        duration_text = "n/a" if duration is None else f"{duration / 1_000_000:.1f} ms"
        cleanup = report.get("cleanup", {}).get("verified", False)
        valid = report.get("valid", False)
        lines.append(
            f"| {record['implementation']} | {record['topology']} | "
            f"{record['pane_count']} | {duration_text} | "
            f"{'verified' if cleanup else 'failed'} | {'PASS' if valid else 'FAIL'} |"
        )
    lines.extend(
        [
            "",
            "Each case used a unique socket/session namespace, exact process-incarnation",
            "checks, explicit server/workload roles, and namespace-scoped cleanup.",
            "",
        ]
    )
    return "\n".join(lines)


def parser() -> argparse.ArgumentParser:
    value = argparse.ArgumentParser(
        description="Run every non-graphical multiplexer/topology orchestration case"
    )
    value.add_argument("output", type=pathlib.Path)
    value.add_argument("--seed", type=int, default=20260804)
    value.add_argument("--idle-seconds", type=float, default=30.0)
    value.add_argument("--ready-timeout", type=float, default=10.0)
    return value


def main() -> int:
    args = parser().parse_args()
    output = args.output.resolve()
    if output.exists() and any(output.iterdir()):
        print(f"output directory is not empty: {output}", file=sys.stderr)
        return 2
    output.mkdir(parents=True, exist_ok=True)
    atomic_json(output / "manifest.json", collect(ROOT))
    atomic_json(
        output / "implementation.json",
        {
            "files": [
                {
                    "path": path,
                    "sha256": sha256(ROOT / path),
                    "size_bytes": (ROOT / path).stat().st_size,
                }
                for path in IMPLEMENTATION_FILES
            ]
        },
    )
    cases = [
        (implementation, topology)
        for implementation in IMPLEMENTATIONS
        for topology in TOPOLOGIES
    ]
    random.Random(args.seed).shuffle(cases)
    records = []
    for index, (implementation, topology) in enumerate(cases):
        case_output = output / "raw" / implementation / topology
        command = [
            sys.executable,
            str(CASE_RUNNER),
            str(case_output),
            "--implementation",
            implementation,
            "--topology",
            topology,
            "--run-id",
            f"m{index}-{implementation}-{topology}",
            "--idle-seconds",
            str(args.idle_seconds),
            "--ready-timeout",
            str(args.ready_timeout),
        ]
        result = subprocess.run(
            command,
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
        )
        report_path = case_output / "report.json"
        report = None
        if report_path.is_file():
            try:
                report = json.loads(report_path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                pass
        records.append(
            {
                "execution_index": index,
                "implementation": implementation,
                "topology": topology,
                "pane_count": {"single": 1, "two-columns": 2, "four-grid": 4}[topology],
                "returncode": result.returncode,
                "report_path": str(report_path.relative_to(output)),
                "report": report,
            }
        )
    valid = all(
        record["returncode"] == 0
        and record["report"] is not None
        and record["report"].get("valid", False)
        for record in records
    )
    matrix = {
        "schema": "splinterm.benchmark.multiplexer-headless-matrix.v1",
        "recorded_at": datetime.datetime.now(datetime.UTC).isoformat(),
        "seed": args.seed,
        "execution_order": [
            {
                "index": item["execution_index"],
                "implementation": item["implementation"],
                "topology": item["topology"],
            }
            for item in records
        ],
        "records": records,
        "valid": valid,
    }
    atomic_json(output / "matrix.json", matrix)
    (output / "summary.md").write_text(markdown(records, args.seed), encoding="utf-8")
    write_checksums(output)
    print(f"Headless multiplexer matrix: {output}")
    print(f"Result: {'PASS' if valid else 'FAIL'}")
    return 0 if valid else 1


if __name__ == "__main__":
    raise SystemExit(main())
