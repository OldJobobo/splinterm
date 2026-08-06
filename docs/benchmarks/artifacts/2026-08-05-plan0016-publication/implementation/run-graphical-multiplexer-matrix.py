#!/usr/bin/env python3
"""Run or plan the guarded Plan 0016 multiplexer development matrix."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import shutil
import signal
import subprocess
import sys
from collections import defaultdict
from typing import Any

import jsonschema
from manifest import collect
from multiplexer_matrix import (
    CASES,
    STACKS,
    TOPOLOGIES,
    MatrixCell,
    assert_resume_compatible,
    case_applicability,
    completed_cell_is_reusable,
    implementation_digest,
    plan_document,
    validate_plan_semantics,
)
from summary import summarize_values

ROOT = pathlib.Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools/benchmark"
CASE_RUNNER = TOOLS / "run-graphical-multiplexer.py"
CELL_SCHEMA = TOOLS / "graphical-multiplexer-schema.json"
PLAN_SCHEMA = TOOLS / "multiplexer-matrix-plan-schema.json"
IMPLEMENTATION_FILES = (
    "tools/benchmark/graphical_multiplexer.py",
    "tools/benchmark/run-graphical-idle.py",
    "tools/benchmark/run-graphical-output.py",
    "tools/benchmark/run-graphical-multiplexer-smoke.py",
    "tools/benchmark/run-graphical-multiplexer.py",
    "tools/benchmark/run-graphical-multiplexer-matrix.py",
    "tools/benchmark/graphical-multiplexer-schema.json",
    "tools/benchmark/multiplexer-matrix-plan-schema.json",
    "tools/benchmark/multiplexer_matrix.py",
    "tools/benchmark/headless_multiplexer.py",
    "tools/benchmark/metrics.py",
    "tools/benchmark/multiplexing.py",
    "tools/benchmark/multiplexers/base.py",
    "tools/benchmark/multiplexers/tmux.py",
    "tools/benchmark/multiplexers/zellij.py",
    "tools/benchmark/profiles/foot.ini",
    "tools/benchmark/profiles/splinterm.ini",
    "tools/benchmark/profiles/tmux.conf",
    "tools/benchmark/profiles/zellij.kdl",
    "tools/benchmark/workloads/bench-child.py",
    "tools/foot-oracle/run-final-buffer-comparison.py",
)
DURATION_KEYS = (
    "request_to_all_children_ready_ns",
    "request_to_all_windows_mapped_ns",
    "trigger_to_all_write_complete_ns",
    "trigger_to_all_visible_markers_ns",
    "settled_duration_ns",
    "input_to_child_ns",
    "input_to_visible_marker_ns",
    "detach_reattach_duration_ns",
    "child_exit_to_observed_state_ns",
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


def execution_identity(manifest: dict[str, Any]) -> dict[str, Any]:
    terminals = {
        item["name"]: {
            key: item.get(key)
            for key in ("name", "available", "executable", "version", "sha256")
        }
        for item in manifest["terminals"]
        if item["name"] in ("splinterm", "foot")
    }
    multiplexers = {
        item["name"]: {
            key: item.get(key)
            for key in ("name", "available", "executable", "version", "sha256")
        }
        for item in manifest["multiplexers"]
    }
    extra = []
    for name, path in (
        ("splinterd", ROOT / "target/release/splinterd"),
        ("grim", pathlib.Path(shutil.which("grim") or "/usr/bin/grim")),
        ("Hyprland", pathlib.Path(shutil.which("Hyprland") or "/usr/bin/Hyprland")),
    ):
        extra.append(
            {
                "name": name,
                "executable": str(path.resolve()),
                "sha256": sha256(path),
                "size_bytes": path.stat().st_size,
            }
        )
    return {
        "host": manifest["host"],
        "repository": manifest["repository"],
        "terminals": [terminals[name] for name in ("splinterm", "foot")],
        "multiplexers": [multiplexers[name] for name in ("tmux", "zellij")],
        "benchmark_stacks": manifest["benchmark_stacks"],
        "extra_executables": extra,
    }


def retain_implementation_snapshot(
    output: pathlib.Path, files: list[pathlib.Path]
) -> None:
    snapshot = output / "implementation-snapshot"
    for source in files:
        destination = snapshot / source.relative_to(ROOT)
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)


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


def run_cell_command(
    command: list[str], timeout: float, cleanup_grace: float
) -> tuple[subprocess.CompletedProcess[str], bool, bool]:
    process = subprocess.Popen(
        command,
        cwd=ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    try:
        stdout, stderr = process.communicate(timeout=timeout)
        return (
            subprocess.CompletedProcess(command, process.returncode, stdout, stderr),
            False,
            True,
        )
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            stdout, stderr = process.communicate(timeout=cleanup_grace)
            cleaned_gracefully = True
        except subprocess.TimeoutExpired:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            stdout, stderr = process.communicate()
            cleaned_gracefully = False
        return (
            subprocess.CompletedProcess(command, process.returncode, stdout, stderr),
            True,
            cleaned_gracefully,
        )


def process_identity_matches(value: dict[str, Any]) -> bool:
    try:
        stat = pathlib.Path(f"/proc/{int(value['pid'])}/stat").read_text(
            encoding="utf-8"
        )
        fields = stat[stat.rfind(")") + 2 :].split()
        return int(fields[19]) == int(value["start_ticks"])
    except (OSError, ValueError, IndexError, KeyError):
        return False


def timed_out_cleanup_is_independently_verified(report: dict[str, Any]) -> bool:
    cleanup = report.get("cleanup", {})
    required = (
        "windows_absent",
        "namespace_absent",
        "server_absent",
        "clients_absent",
        "workloads_absent",
        "process_forest_absent",
        "ambient_counts_unchanged",
        "verified",
    )
    if not all(cleanup.get(name) is True for name in required):
        return False
    if not workspace_is_empty():
        return False
    namespace = str(report.get("isolation", {}).get("namespace", ""))
    if namespace.startswith("/") and pathlib.Path(namespace).exists():
        return False
    identities = []
    processes = report.get("processes") or {}
    for history in processes.get("role_history", []):
        for role in history.get("roles", []):
            identities.extend(role.get("processes", []))
    identities.extend(report.get("windows", []))
    return not any(process_identity_matches(item) for item in identities)


def workspace_is_empty() -> bool:
    result = subprocess.run(
        ["hyprctl", "clients", "-j"],
        text=True,
        capture_output=True,
        check=False,
        timeout=5,
    )
    if result.returncode:
        return False
    try:
        clients = json.loads(result.stdout)
    except json.JSONDecodeError:
        return False
    return not any(item.get("workspace", {}).get("id") == 8 for item in clients)


def cell_from_dict(value: dict[str, Any]) -> MatrixCell:
    return MatrixCell(
        phase=str(value["phase"]),
        iteration=int(value["iteration"]),
        execution_index=int(value["execution_index"]),
        stack=str(value["stack"]),
        topology=str(value["topology"]),
        case_id=str(value["case_id"]),
    )


def case_directory(output: pathlib.Path, cell: MatrixCell) -> pathlib.Path:
    return (
        output
        / "raw"
        / cell.phase
        / f"{cell.iteration:02d}"
        / cell.stack
        / cell.topology
    )


def validate_cell_semantics(report: dict[str, Any], cell: MatrixCell) -> None:
    if not completed_cell_is_reusable(report, cell, str(report.get("plan_sha256"))):
        raise ValueError("cell report identity or cleanup does not match its schedule")
    panes = [
        f"pane-{index}"
        for index in range(
            {"single": 1, "two-columns": 2, "four-grid": 4}[cell.topology]
        )
    ]
    if list(report["runtime_ids"]) != panes:
        raise ValueError("cell runtime IDs do not match ordered topology panes")
    operations = report["operations"]
    if [item["case"] for item in operations] != list(CASES):
        raise ValueError("cell operations are incomplete or reordered")
    required_metrics = {
        "startup": (
            "request_to_all_children_ready_ns",
            "request_to_all_windows_mapped_ns",
        ),
        "idle": ("sample_seconds",),
        "plain": (
            "trigger_to_all_write_complete_ns",
            "trigger_to_all_visible_markers_ns",
        ),
        "ansi": (
            "trigger_to_all_write_complete_ns",
            "trigger_to_all_visible_markers_ns",
        ),
        "unicode": (
            "trigger_to_all_write_complete_ns",
            "trigger_to_all_visible_markers_ns",
        ),
        "outer-resize": ("resize_count", "settled_duration_ns", "steps"),
        "divider-resize": ("target_ratio_milli", "after_ratio_milli"),
        "input": ("input_to_child_ns", "input_to_visible_marker_ns"),
        "detach-reattach": ("detach_reattach_duration_ns",),
        "lifecycle": ("child_exit_to_observed_state_ns", "settled_state"),
    }
    for operation in operations:
        case = str(operation["case"])
        expected = case_applicability(cell.stack, cell.topology, case)
        if operation["applicability"] != expected or operation["valid"] is not True:
            raise ValueError(
                f"cell applicability or validity is inconsistent for {case}"
            )
        if expected["status"] == "not-applicable":
            continue
        if any(name not in operation["metrics"] for name in required_metrics[case]):
            raise ValueError(f"cell metrics are incomplete for {case}")
        evidence = operation["resources"]
        before_membership = evidence["before_membership"]
        after_membership = evidence["after_membership"]
        changed = before_membership != after_membership
        if evidence["membership_changed"] is not changed:
            raise ValueError(f"resource membership flag is inconsistent for {case}")
        if (evidence["delta"] is None) is not changed:
            raise ValueError(f"resource delta applicability is inconsistent for {case}")
        for membership in (before_membership, after_membership):
            infrastructure = {
                (item["pid"], item["start_ticks"])
                for item in membership["infrastructure"]
            }
            workloads = {
                (item["pid"], item["start_ticks"]) for item in membership["workload"]
            }
            if infrastructure & workloads:
                raise ValueError(f"resource role membership overlaps for {case}")
        if case in ("plain", "ansi", "unicode") and set(
            operation["pane_metrics"]
        ) != set(panes):
            raise ValueError(f"output pane evidence is incomplete for {case}")
        if case == "outer-resize" and len(operation["metrics"]["steps"]) != 12:
            raise ValueError("outer resize does not retain twelve settled steps")


def load_reusable(
    path: pathlib.Path,
    cell: MatrixCell,
    plan_sha256: str,
    validator: jsonschema.Draft202012Validator,
) -> dict[str, Any] | None:
    try:
        report = json.loads(path.read_text(encoding="utf-8"))
        validator.validate(report)
        if report.get("plan_sha256") != plan_sha256:
            return None
        validate_cell_semantics(report, cell)
    except (OSError, ValueError, json.JSONDecodeError, jsonschema.ValidationError):
        return None
    return report


def operation_metrics(operation: dict[str, Any]) -> dict[str, int]:
    return {
        key: int(value)
        for key in DURATION_KEYS
        if isinstance((value := operation["metrics"].get(key)), int | float)
    }


def summaries(reports: list[dict[str, Any]]) -> dict[str, Any]:
    values: dict[tuple[str, str, str, str], list[int]] = defaultdict(list)
    resources: dict[tuple[str, str, str, str], list[int]] = defaultdict(list)
    applicability: dict[tuple[str, str, str], str] = {}
    for report in reports:
        stack = str(report["stack"]["name"])
        topology = str(report["topology"]["name"])
        for operation in report["operations"]:
            case = str(operation["case"])
            status = str(operation["applicability"]["status"])
            applicability[(stack, topology, case)] = status
            if status != "measured":
                continue
            for name, value in operation_metrics(operation).items():
                values[(stack, topology, case, name)].append(value)
            evidence = operation.get("resources") or {}
            selected_resources = evidence.get("delta") or evidence.get("after") or {}
            for scope in ("infrastructure", "total"):
                scoped = selected_resources.get(scope, {})
                for name in (
                    "rss_bytes",
                    "pss_bytes",
                    "cpu_ticks",
                    "context_switches",
                ):
                    if isinstance(scoped.get(name), int):
                        resources[(stack, topology, case, f"{scope}_{name}")].append(
                            int(scoped[name])
                        )
    summary: dict[str, Any] = {}
    for stack in STACKS:
        summary[stack] = {}
        for topology in TOPOLOGIES:
            summary[stack][topology] = {}
            for case in CASES:
                status = applicability.get((stack, topology, case), "not-recorded")
                item: dict[str, Any] = {
                    "status": status,
                    "metrics": {},
                    "resources": {},
                }
                for (
                    item_stack,
                    item_topology,
                    item_case,
                    name,
                ), samples in values.items():
                    if (item_stack, item_topology, item_case) == (
                        stack,
                        topology,
                        case,
                    ):
                        item["metrics"][name] = summarize_values(samples)
                for (
                    item_stack,
                    item_topology,
                    item_case,
                    name,
                ), samples in resources.items():
                    if (item_stack, item_topology, item_case) == (
                        stack,
                        topology,
                        case,
                    ):
                        item["resources"][name] = summarize_values(samples)
                summary[stack][topology][case] = item
    return summary


def markdown(matrix: dict[str, Any]) -> str:
    lines = [
        "# Splinterbench multiplexer development matrix",
        "",
        f"Measured samples per stack/topology: {matrix['sample_runs']}  ",
        f"Warmups per stack/topology: {matrix['warmup_runs']}  ",
        f"Randomization seed: {matrix['seed']}",
        "",
        "Native and nested values are complete stack measurements; Foot overhead is not subtracted.",
        "Screenshot polling is a visible-marker approximation, not presentation latency.",
        "Independent Foot uses 1/2/4 windows at equivalent aggregate geometry.",
        "",
        "| Stack | Topology | Completed | Expected | Result |",
        "|---|---|---:|---:|---|",
    ]
    counts = defaultdict(int)
    for record in matrix["records"]:
        if record["phase"] == "measured" and record["valid"]:
            counts[(record["stack"], record["topology"])] += 1
    for stack in STACKS:
        for topology in TOPOLOGIES:
            completed = counts[(stack, topology)]
            expected = matrix["sample_runs"]
            lines.append(
                f"| {stack} | {topology} | {completed} | {expected} | "
                f"{'PASS' if completed == expected else 'INCOMPLETE'} |"
            )
    lines.extend(
        [
            "",
            "Divider resize is not applicable to single-pane cells or independent Foot windows.",
            "Detach/reattach is not applicable to independent Foot windows. These are explicit",
            "applicability results, not failed or silently omitted samples.",
            "",
            "Raw cell reports, the immutable execution plan, implementation snapshot, manifest,",
            "execution attempts, and checksums are retained beside this summary.",
            "",
        ]
    )
    if matrix.get("error"):
        lines.extend([f"Matrix stopped: `{matrix['error']}`", ""])
    return "\n".join(lines)


def save_matrix(
    output: pathlib.Path,
    plan: dict[str, Any],
    records: list[dict[str, Any]],
    attempts: list[dict[str, Any]],
    error: str | None,
) -> dict[str, Any]:
    measured = [item for item in records if item["phase"] == "measured"]
    expected = int(plan["sample_runs"]) * len(STACKS) * len(TOPOLOGIES)
    document = {
        "schema": "splinterm.benchmark.multiplexer-matrix.v1",
        "seed": plan["seed"],
        "warmup_runs": plan["warmup_runs"],
        "sample_runs": plan["sample_runs"],
        "plan_sha256": plan["plan_sha256"],
        "execution_order": plan["schedule"],
        "attempts": attempts,
        "records": records,
        "completed_measured_cells": sum(item["valid"] for item in measured),
        "expected_measured_cells": expected,
        "summary": summaries([item["report"] for item in measured if item["valid"]]),
        "error": error,
        "valid": error is None
        and len(measured) == expected
        and all(item["valid"] for item in measured),
    }
    atomic_json(output / "matrix.json", document)
    (output / "summary.md").write_text(markdown(document), encoding="utf-8")
    write_checksums(output)
    return document


def parser() -> argparse.ArgumentParser:
    value = argparse.ArgumentParser(
        description="Plan or run the randomized four-stack multiplexer development matrix"
    )
    value.add_argument("output", type=pathlib.Path)
    value.add_argument("--warmup-runs", type=int, default=3)
    value.add_argument("--samples", type=int, default=10)
    value.add_argument("--seed", type=int, default=13_372_075)
    value.add_argument("--idle-warmup-seconds", type=float, default=1.0)
    value.add_argument("--idle-sample-seconds", type=float, default=2.0)
    value.add_argument("--lines", type=int, default=2000)
    value.add_argument("--columns", type=int, default=80)
    value.add_argument("--settle-seconds", type=float, default=0.5)
    value.add_argument("--ready-timeout", type=float, default=10.0)
    value.add_argument("--operation-timeout", type=float, default=20.0)
    value.add_argument("--lifetime-seconds", type=float, default=300.0)
    value.add_argument("--cell-timeout", type=float, default=300.0)
    value.add_argument("--timeout-cleanup-grace", type=float, default=30.0)
    value.add_argument(
        "--resume",
        action="store_true",
        help="reuse only schema-valid cells matching the immutable plan and implementation",
    )
    value.add_argument(
        "--plan-only",
        action="store_true",
        help="write and validate the immutable plan without opening graphical windows",
    )
    return value


def main() -> int:
    args = parser().parse_args()
    if (
        args.warmup_runs < 0
        or args.samples <= 0
        or args.idle_warmup_seconds < 0
        or args.idle_sample_seconds <= 0
        or args.lines <= 0
        or args.columns < 20
        or args.settle_seconds < 0
        or min(
            args.ready_timeout,
            args.operation_timeout,
            args.lifetime_seconds,
            args.cell_timeout,
            args.timeout_cleanup_grace,
        )
        <= 0
    ):
        print("invalid matrix count, dimensions, duration, or timeout", file=sys.stderr)
        return 2
    output = args.output.resolve()
    if output.exists() and any(output.iterdir()) and not args.resume:
        print(
            "output directory is not empty; use --resume for an exact plan",
            file=sys.stderr,
        )
        return 2
    output.mkdir(parents=True, exist_ok=True)
    files = [ROOT / path for path in IMPLEMENTATION_FILES]
    parameters = {
        "idle_warmup_seconds": args.idle_warmup_seconds,
        "idle_sample_seconds": args.idle_sample_seconds,
        "lines": args.lines,
        "columns": args.columns,
        "settle_seconds": args.settle_seconds,
        "ready_timeout_seconds": args.ready_timeout,
        "operation_timeout_seconds": args.operation_timeout,
        "lifetime_seconds": args.lifetime_seconds,
        "cell_timeout_seconds": args.cell_timeout,
        "timeout_cleanup_grace_seconds": args.timeout_cleanup_grace,
    }
    manifest = collect(ROOT)
    identity = execution_identity(manifest)
    plan = plan_document(
        seed=args.seed,
        warmup_runs=args.warmup_runs,
        sample_runs=args.samples,
        implementation_sha256=implementation_digest(files, ROOT),
        execution_identity=identity,
        parameters=parameters,
    )
    plan_validator = jsonschema.Draft202012Validator(
        json.loads(PLAN_SCHEMA.read_text(encoding="utf-8"))
    )
    plan_validator.validate(plan)
    validate_plan_semantics(plan)
    plan_path = output / "plan.json"
    if args.resume and plan_path.exists():
        try:
            existing = json.loads(plan_path.read_text(encoding="utf-8"))
            plan_validator.validate(existing)
            validate_plan_semantics(existing)
            assert_resume_compatible(existing, plan)
        except (
            OSError,
            ValueError,
            json.JSONDecodeError,
            jsonschema.ValidationError,
        ) as error:
            print(f"cannot resume matrix: {error}", file=sys.stderr)
            return 2
    else:
        atomic_json(plan_path, plan)
        atomic_json(output / "manifest.json", manifest)
        retain_implementation_snapshot(output, files)
        atomic_json(
            output / "implementation.json",
            {
                "sha256": plan["implementation_sha256"],
                "files": [
                    {
                        "path": str(path.relative_to(ROOT)),
                        "sha256": sha256(path),
                        "size_bytes": path.stat().st_size,
                    }
                    for path in files
                ],
            },
        )
    if args.plan_only:
        write_checksums(output)
        print(f"Matrix plan: {plan_path}")
        print(f"Cells: {len(plan['schedule'])} (no graphical actions performed)")
        return 0

    cell_validator = jsonschema.Draft202012Validator(
        json.loads(CELL_SCHEMA.read_text(encoding="utf-8"))
    )
    records: list[dict[str, Any]] = []
    attempts: list[dict[str, Any]] = []
    error = None
    try:
        for value in plan["schedule"]:
            current_manifest = collect(ROOT)
            if execution_identity(current_manifest) != plan["execution_identity"]:
                raise RuntimeError(
                    "execution identity changed after the matrix plan was written"
                )
            cell = cell_from_dict(value)
            case_dir = case_directory(output, cell)
            report_path = case_dir / "report.json"
            report = (
                load_reusable(
                    report_path, cell, str(plan["plan_sha256"]), cell_validator
                )
                if args.resume
                else None
            )
            reused = report is not None
            if report is None:
                if case_dir.exists():
                    shutil.rmtree(case_dir)
                case_dir.mkdir(parents=True)
                command = [
                    sys.executable,
                    str(CASE_RUNNER),
                    str(case_dir),
                    "--stack",
                    cell.stack,
                    "--topology",
                    cell.topology,
                    "--case-id",
                    cell.case_id,
                    "--plan-sha256",
                    str(plan["plan_sha256"]),
                    "--phase",
                    cell.phase,
                    "--iteration",
                    str(cell.iteration),
                    "--execution-index",
                    str(cell.execution_index),
                    "--idle-warmup-seconds",
                    str(args.idle_warmup_seconds),
                    "--idle-sample-seconds",
                    str(args.idle_sample_seconds),
                    "--lines",
                    str(args.lines),
                    "--columns",
                    str(args.columns),
                    "--settle-seconds",
                    str(args.settle_seconds),
                    "--ready-timeout",
                    str(args.ready_timeout),
                    "--operation-timeout",
                    str(args.operation_timeout),
                    "--lifetime-seconds",
                    str(args.lifetime_seconds),
                ]
                completed, timed_out, graceful_cleanup = run_cell_command(
                    command, args.cell_timeout, args.timeout_cleanup_grace
                )
                attempt = {
                    "execution_index": cell.execution_index,
                    "case_id": cell.case_id,
                    "returncode": completed.returncode,
                    "reused": False,
                    "timed_out": timed_out,
                    "graceful_cleanup": graceful_cleanup,
                }
                attempts.append(attempt)
                if not report_path.exists():
                    if timed_out and not workspace_is_empty():
                        attempt["workspace_cleanup_verified"] = False
                    raise RuntimeError(f"{cell.case_id} produced no report")
                report = json.loads(report_path.read_text(encoding="utf-8"))
                if timed_out:
                    workspace_clean = workspace_is_empty()
                    report_clean = report.get("cleanup", {}).get("verified") is True
                    independently_clean = timed_out_cleanup_is_independently_verified(
                        report
                    )
                    attempt["workspace_cleanup_verified"] = workspace_clean
                    attempt["report_cleanup_verified"] = report_clean
                    attempt["independent_cleanup_verified"] = independently_clean
                    if not (
                        graceful_cleanup
                        and workspace_clean
                        and report_clean
                        and independently_clean
                    ):
                        raise RuntimeError(
                            f"{cell.case_id} timed out and cleanup was not verified"
                        )
                    raise RuntimeError(
                        f"{cell.case_id} timed out after verified bounded cleanup"
                    )
                if completed.returncode or not report.get("valid"):
                    raise RuntimeError(
                        f"{cell.case_id} failed: {report.get('failure') or report.get('notes')}"
                    )
                cell_validator.validate(report)
                validate_cell_semantics(report, cell)
            else:
                attempts.append(
                    {
                        "execution_index": cell.execution_index,
                        "case_id": cell.case_id,
                        "returncode": 0,
                        "reused": True,
                        "timed_out": False,
                        "graceful_cleanup": True,
                    }
                )
            records.append(
                {
                    **cell.as_dict(),
                    "report_path": str(report_path.relative_to(output)),
                    "reused": reused,
                    "valid": True,
                    "report": report,
                }
            )
            print(
                f"[{cell.execution_index + 1:03d}/{len(plan['schedule']):03d}] "
                f"{cell.phase} {cell.stack}/{cell.topology}: "
                f"{'reused' if reused else 'done'}"
            )
    except (
        OSError,
        ValueError,
        RuntimeError,
        json.JSONDecodeError,
        jsonschema.ValidationError,
        subprocess.TimeoutExpired,
    ) as caught:
        error = str(caught)
        print(f"Matrix stopped: {error}", file=sys.stderr)
    document = save_matrix(output, plan, records, attempts, error)
    print(f"Matrix report: {output / 'summary.md'}")
    print(f"Result: {'PASS' if document['valid'] else 'INCOMPLETE'}")
    return 0 if document["valid"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
