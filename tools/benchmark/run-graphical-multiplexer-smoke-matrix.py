#!/usr/bin/env python3
"""Run the approved guarded native smoke, then equivalent Foot peer smokes."""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import subprocess
import sys
from typing import Any

import jsonschema
from manifest import collect

ROOT = pathlib.Path(__file__).resolve().parents[2]
CASE_RUNNER = pathlib.Path(__file__).with_name("run-graphical-multiplexer-smoke.py")
SCHEMA = pathlib.Path(__file__).with_name("graphical-multiplexer-smoke-schema.json")
STACKS = ("splinterm-native", "foot-tmux", "foot-zellij")
IMPLEMENTATION_FILES = (
    "tools/benchmark/run-graphical-multiplexer-smoke.py",
    "tools/benchmark/run-graphical-multiplexer-smoke-matrix.py",
    "tools/benchmark/graphical-multiplexer-smoke-schema.json",
    "tools/benchmark/headless_multiplexer.py",
    "tools/benchmark/multiplexing.py",
    "tools/benchmark/metrics.py",
    "tools/benchmark/multiplexers/base.py",
    "tools/benchmark/multiplexers/tmux.py",
    "tools/benchmark/multiplexers/zellij.py",
    "tools/benchmark/profiles/foot.ini",
    "tools/benchmark/profiles/tmux.conf",
    "tools/benchmark/profiles/zellij.kdl",
)


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def atomic_json(path: pathlib.Path, value: Any) -> None:
    temporary = path.with_name(f".{path.name}.tmp")
    temporary.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    temporary.replace(path)


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


def markdown(records: list[dict[str, Any]], error: str | None) -> str:
    lines = [
        "# Splinterbench guarded graphical multiplexer smoke",
        "",
        "This is topology, isolation, and cleanup evidence—not a performance ranking.",
        "Every window was silently mapped to inactive workspace 8 on DP-2.",
        "",
        "| Stack | Topology | Pane geometry | Host state | Cleanup | Result |",
        "|---|---|---|---|---|---|",
    ]
    by_stack = {record["stack"]: record for record in records}
    for stack in STACKS:
        record = by_stack.get(stack)
        if record is None:
            lines.append(f"| {stack} | not run | — | — | — | SKIPPED |")
            continue
        report = record["report"]
        panes = report.get("geometry", {}).get("after_graphical_attach") or []
        geometry = ", ".join(
            f"{pane['name']}={pane['columns']}×{pane['rows']}" for pane in panes
        )
        isolation = report.get("isolation", {})
        cleanup = report.get("cleanup", {})
        lines.append(
            f"| {stack} | two columns | {geometry or 'unavailable'} | "
            f"{'preserved' if isolation.get('host_state_preserved') else 'failed'} | "
            f"{'verified' if cleanup.get('verified') else 'failed'} | "
            f"{'PASS' if report.get('valid') else 'FAIL'} |"
        )
    if error:
        lines.extend(["", f"Sequence stopped: `{error}`"])
    lines.extend(
        [
            "",
            "The Splinterm-native case gated both Foot peer cases. A placement, focus,",
            "pointer, topology, process-incarnation, or cleanup violation stops the sequence.",
            "",
        ]
    )
    return "\n".join(lines)


def parser() -> argparse.ArgumentParser:
    value = argparse.ArgumentParser(
        description="Run the approved three-stack guarded graphical smoke sequence"
    )
    value.add_argument("output", type=pathlib.Path)
    value.add_argument("--idle-seconds", type=float, default=45.0)
    value.add_argument("--ready-timeout", type=float, default=10.0)
    value.add_argument("--settle-seconds", type=float, default=0.5)
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
    validator = jsonschema.Draft202012Validator(
        json.loads(SCHEMA.read_text(encoding="utf-8"))
    )
    records: list[dict[str, Any]] = []
    error = None
    for index, stack in enumerate(STACKS):
        case_output = output / "raw" / stack
        command = [
            sys.executable,
            str(CASE_RUNNER),
            str(case_output),
            "--stack",
            stack,
            "--run-id",
            f"gsmoke-{index}-{stack}",
            "--idle-seconds",
            str(args.idle_seconds),
            "--ready-timeout",
            str(args.ready_timeout),
            "--settle-seconds",
            str(args.settle_seconds),
        ]
        result = subprocess.run(command, cwd=ROOT, check=False)
        report_path = case_output / "report.json"
        if not report_path.is_file():
            error = f"{stack} produced no report"
            break
        try:
            report = json.loads(report_path.read_text(encoding="utf-8"))
            validator.validate(report)
        except (OSError, json.JSONDecodeError, jsonschema.ValidationError) as caught:
            error = f"{stack} produced an invalid report: {caught}"
            break
        records.append(
            {
                "execution_index": index,
                "stack": stack,
                "returncode": result.returncode,
                "report_path": str(report_path.relative_to(output)),
                "report": report,
            }
        )
        if result.returncode or not report.get("valid"):
            error = f"{stack} guarded smoke failed: {report.get('failure')}"
            break
        if not report.get("cleanup", {}).get("verified"):
            error = f"{stack} cleanup was not verified"
            break
    valid = error is None and len(records) == len(STACKS)
    atomic_json(
        output / "matrix.json",
        {
            "schema": "splinterm.benchmark.multiplexer-graphical-smoke-matrix.v1",
            "sequence": list(STACKS),
            "records": records,
            "error": error,
            "valid": valid,
        },
    )
    (output / "summary.md").write_text(markdown(records, error), encoding="utf-8")
    write_checksums(output)
    print(f"Graphical multiplexer smoke: {output}")
    print(f"Result: {'PASS' if valid else 'FAIL'}")
    return 0 if valid else 1


if __name__ == "__main__":
    raise SystemExit(main())
