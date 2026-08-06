#!/usr/bin/env python3
"""Regenerate the Plan 0016 publication summary from retained raw evidence."""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import statistics
from collections import defaultdict


def sha256(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


STACKS = ("splinterm-native", "foot-bare", "foot-tmux", "foot-zellij")
TOPOLOGIES = ("single", "two-columns", "four-grid")
CASES = (
    "startup",
    "idle",
    "plain",
    "ansi",
    "unicode",
    "outer-resize",
    "divider-resize",
    "input",
    "detach-reattach",
    "lifecycle",
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


def summarize_values(values: list[int]) -> dict[str, int | float]:
    ordered = sorted(values)
    median = statistics.median(ordered)
    return {
        "count": len(ordered),
        "min": ordered[0],
        "median": median,
        "p95": float(ordered[(len(ordered) - 1) * 95 // 100]),
        "max": ordered[-1],
        "median_absolute_deviation": statistics.median(
            abs(value - median) for value in ordered
        ),
    }


def summaries(reports: list[dict]) -> dict:
    values = defaultdict(list)
    resources = defaultdict(list)
    applicability = {}
    for report in reports:
        stack = report["stack"]["name"]
        topology = report["topology"]["name"]
        for operation in report["operations"]:
            case = operation["case"]
            status = operation["applicability"]["status"]
            applicability[(stack, topology, case)] = status
            if status != "measured":
                continue
            for name in DURATION_KEYS:
                value = operation["metrics"].get(name)
                if isinstance(value, int | float):
                    values[(stack, topology, case, name)].append(int(value))
            evidence = operation.get("resources") or {}
            selected = evidence.get("delta") or evidence.get("after") or {}
            for scope in ("infrastructure", "total"):
                scoped = selected.get(scope, {})
                for name in (
                    "rss_bytes",
                    "pss_bytes",
                    "cpu_ticks",
                    "context_switches",
                ):
                    if isinstance(scoped.get(name), int):
                        resources[(stack, topology, case, f"{scope}_{name}")].append(
                            scoped[name]
                        )
    result = {}
    for stack in STACKS:
        result[stack] = {}
        for topology in TOPOLOGIES:
            result[stack][topology] = {}
            for case in CASES:
                item = {
                    "status": applicability.get(
                        (stack, topology, case), "not-recorded"
                    ),
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
                result[stack][topology][case] = item
    return result


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository", type=pathlib.Path, required=True)
    parser.add_argument("--multiplexer", type=pathlib.Path, required=True)
    parser.add_argument("--baseline", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--generated-utc", required=True)
    args = parser.parse_args()

    root = args.repository.resolve()
    output = args.output.resolve()
    mux = args.multiplexer.resolve()
    baseline = args.baseline.resolve()
    aggregator_path = output / "implementation/run-graphical-multiplexer-matrix.py"

    reports = []
    for path in sorted((mux / "raw/measured").glob("*/*/*/report.json")):
        report = json.loads(path.read_text(encoding="utf-8"))
        assert report["valid"] and report["cleanup"]["verified"]
        reports.append(report)
    assert len(reports) == 120

    matrix = json.loads((mux / "matrix.json").read_text(encoding="utf-8"))
    base = json.loads((baseline / "matrix.json").read_text(encoding="utf-8"))
    assert matrix["valid"] and matrix["completed_measured_cells"] == 120
    assert base["valid"] and base["completed_measured_cases"] == 50
    summary = summaries(reports)

    command = (
        "python generate-publication.py --repository ../.. "
        f"--multiplexer ../../{mux.relative_to(root)} "
        f"--baseline ../../{baseline.relative_to(root)} --output . "
        f"--generated-utc {args.generated_utc}"
    )
    provenance = {
        "schema": "splinterm.benchmark.multiplexer-publication-provenance.v1",
        "generation_command": command,
        "inputs": {
            "multiplexer_matrix_sha256": sha256(mux / "matrix.json"),
            "multiplexer_plan_sha256": sha256(mux / "plan.json"),
            "multiplexer_checksums_sha256": sha256(mux / "SHA256SUMS"),
            "five_terminal_matrix_sha256": sha256(baseline / "matrix.json"),
            "aggregator_sha256": sha256(aggregator_path),
            "aggregator_test_sha256": sha256(
                output / "implementation/test_multiplexer_matrix.py"
            ),
        },
    }
    (output / "PROVENANCE.json").write_text(
        json.dumps(provenance, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )

    bundle = {
        "schema": "splinterm.benchmark.multiplexer-publication-review.v1",
        "generated_utc": args.generated_utc,
        "sources": {
            "multiplexer": {
                "path": str(mux.relative_to(root)),
                "matrix_sha256": provenance["inputs"]["multiplexer_matrix_sha256"],
                "plan_sha256": provenance["inputs"]["multiplexer_plan_sha256"],
                "bundle_checksums_sha256": provenance["inputs"][
                    "multiplexer_checksums_sha256"
                ],
                "seed": matrix["seed"],
                "warmup_runs": matrix["warmup_runs"],
                "sample_runs": matrix["sample_runs"],
                "measured_reports": len(reports),
                "valid": matrix["valid"],
            },
            "five_terminal_idle": {
                "path": str(baseline.relative_to(root)),
                "matrix_sha256": provenance["inputs"]["five_terminal_matrix_sha256"],
                "seed": base["seed"],
                "warmup_runs": base["warmup_runs"],
                "sample_runs": base["sample_runs"],
                "measured_cases": base["completed_measured_cases"],
                "valid": base["valid"],
                "launch_model": "Splinterm uses a prestarted daemon; peers use standalone launches.",
            },
            "aggregator": {
                "path": "implementation/run-graphical-multiplexer-matrix.py",
                "sha256": provenance["inputs"]["aggregator_sha256"],
            },
        },
        "interpretation_contract": {
            "stack_values_are_complete": True,
            "foot_overhead_subtracted": False,
            "visible_marker_is_presentation_latency": False,
            "cross_host_claim": False,
            "warmups_in_aggregates": False,
            "bare_foot_divider_and_detach": "not-applicable",
        },
        "multiplexer_summary": summary,
        "five_terminal_idle_summary": base["summary"],
    }
    (output / "summary.json").write_text(
        json.dumps(bundle, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )

    def median(stack: str, topology: str, case: str, key: str, kind="metrics"):
        item = summary[stack][topology][case][kind].get(key)
        return None if item is None else item["median"]

    def milliseconds(value) -> str:
        return "N/A" if value is None else f"{value / 1_000_000:.1f} ms"

    def mebibytes(value) -> str:
        return "N/A" if value is None else f"{value / (1024 * 1024):.1f} MiB"

    lines = [
        "# Plan 0016 multiplexer publication review",
        "",
        "Status: **candidate pending independent review**  ",
        f"Seed: `{matrix['seed']}` · warmups: {matrix['warmup_runs']} · measured samples per stack/topology: {matrix['sample_runs']}",
        "",
        "Values below are medians. Native and nested values measure complete stacks; Foot overhead is not subtracted. Visible-marker polling is a screenshot approximation, not compositor presentation latency. Results describe this host and build only.",
        "",
        "## Startup, idle footprint, and ANSI output",
        "",
        "| Stack | Topology | Children ready | Windows mapped | Idle total RSS | ANSI write complete | ANSI visible marker |",
        "|---|---|---:|---:|---:|---:|---:|",
    ]
    for stack in STACKS:
        for topology in TOPOLOGIES:
            lines.append(
                f"| {stack} | {topology} | {milliseconds(median(stack, topology, 'startup', 'request_to_all_children_ready_ns'))} | "
                f"{milliseconds(median(stack, topology, 'startup', 'request_to_all_windows_mapped_ns'))} | "
                f"{mebibytes(median(stack, topology, 'idle', 'total_rss_bytes', 'resources'))} | "
                f"{milliseconds(median(stack, topology, 'ansi', 'trigger_to_all_write_complete_ns'))} | "
                f"{milliseconds(median(stack, topology, 'ansi', 'trigger_to_all_visible_markers_ns'))} |"
            )
    lines += [
        "",
        "## Interaction and lifecycle",
        "",
        "| Stack | Topology | Input to child | Input visible marker | 12-step outer resize | Divider resize | Detach/reattach | Child exit settled |",
        "|---|---|---:|---:|---:|---:|---:|---:|",
    ]
    for stack in STACKS:
        for topology in TOPOLOGIES:
            lines.append(
                f"| {stack} | {topology} | {milliseconds(median(stack, topology, 'input', 'input_to_child_ns'))} | "
                f"{milliseconds(median(stack, topology, 'input', 'input_to_visible_marker_ns'))} | "
                f"{milliseconds(median(stack, topology, 'outer-resize', 'settled_duration_ns'))} | "
                f"{milliseconds(median(stack, topology, 'divider-resize', 'settled_duration_ns'))} | "
                f"{milliseconds(median(stack, topology, 'detach-reattach', 'detach_reattach_duration_ns'))} | "
                f"{milliseconds(median(stack, topology, 'lifecycle', 'child_exit_to_observed_state_ns'))} |"
            )
    lines += [
        "",
        "## Current bare-terminal idle control",
        "",
        "Splinterm uses a prestarted daemon; Foot, Kitty, Ghostty, and Alacritty use standalone process launches. Startup boundaries are therefore observed independently rather than treated as identical launch models.",
        "",
        "| Terminal | Child ready | Window mapped | Idle RSS |",
        "|---|---:|---:|---:|",
    ]
    for terminal in ("splinterm", "foot", "kitty", "ghostty", "alacritty"):
        item = base["summary"][terminal]
        lines.append(
            f"| {terminal} | {milliseconds(item['launch_to_child_ready_ns']['median'])} | "
            f"{milliseconds(item['launch_to_window_map_ns']['median'])} | "
            f"{mebibytes(item['rss_bytes']['median'])} |"
        )
    lines += [
        "",
        "## Evidence checks",
        "",
        "- Multiplexer matrix: 36 warmup and 120 measured reports; all valid with exact cleanup.",
        "- Five-terminal idle control: 15 warmup and 50 measured cases; all valid with guarded cleanup.",
        "- Multiplexer source bundle: 183 checksum entries verified before this review bundle was generated.",
        "- Corrected post-processing source, test, input hashes, and generation command are retained in this bundle.",
        "- Unsupported independent-Foot divider and detach semantics remain explicit N/A results.",
        "- The earlier guarded focus stop is not treated as a performance sample; the successful immutable plan reused 40 valid cells and completed the remaining schedule under the same execution identity.",
        "",
    ]
    (output / "summary.md").write_text("\n".join(lines), encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
