"""Deterministic planning and applicability rules for multiplexer measurements."""

from __future__ import annotations

import dataclasses
import hashlib
import json
import pathlib
import random
from collections.abc import Mapping
from typing import Any

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
STACK_IDENTITIES: dict[str, dict[str, str | None]] = {
    "splinterm-native": {
        "name": "splinterm-native",
        "terminal": "splinterm",
        "multiplexer": "splinterm",
        "integration": "native",
    },
    "foot-bare": {
        "name": "foot-bare",
        "terminal": "foot",
        "multiplexer": None,
        "integration": "none",
    },
    "foot-tmux": {
        "name": "foot-tmux",
        "terminal": "foot",
        "multiplexer": "tmux",
        "integration": "nested",
    },
    "foot-zellij": {
        "name": "foot-zellij",
        "terminal": "foot",
        "multiplexer": "zellij",
        "integration": "nested",
    },
}
NOT_APPLICABLE = {
    ("foot-bare", "divider-resize"): (
        "independent Foot windows have no shared multiplexer divider"
    ),
    ("foot-bare", "detach-reattach"): (
        "independent Foot windows have no persistent multiplexer session to detach"
    ),
}


@dataclasses.dataclass(frozen=True)
class MatrixCell:
    phase: str
    iteration: int
    execution_index: int
    stack: str
    topology: str
    case_id: str

    def as_dict(self) -> dict[str, int | str]:
        return dataclasses.asdict(self)


def case_applicability(stack: str, topology: str, case: str) -> dict[str, str]:
    _validate_identity(stack, topology, case)
    if case == "divider-resize" and topology == "single":
        return {
            "status": "not-applicable",
            "reason": "single-pane topology has no divider",
        }
    if reason := NOT_APPLICABLE.get((stack, case)):
        return {"status": "not-applicable", "reason": reason}
    return {"status": "measured", "reason": "supported by this stack and topology"}


def build_schedule(
    seed: int, warmup_runs: int = 3, sample_runs: int = 10
) -> list[MatrixCell]:
    if warmup_runs < 0 or sample_runs <= 0:
        raise ValueError("warmup runs must be nonnegative and samples must be positive")
    generator = random.Random(seed)
    schedule: list[MatrixCell] = []
    execution_index = 0
    for phase, count in (("warmup", warmup_runs), ("measured", sample_runs)):
        for iteration in range(count):
            cells = [(stack, topology) for stack in STACKS for topology in TOPOLOGIES]
            generator.shuffle(cells)
            for stack, topology in cells:
                case_id = (
                    f"{phase}-{iteration:02d}-{execution_index:04d}-{stack}-{topology}"
                )
                schedule.append(
                    MatrixCell(
                        phase=phase,
                        iteration=iteration,
                        execution_index=execution_index,
                        stack=stack,
                        topology=topology,
                        case_id=case_id,
                    )
                )
                execution_index += 1
    return schedule


def implementation_digest(files: list[pathlib.Path], root: pathlib.Path) -> str:
    digest = hashlib.sha256()
    for path in sorted(files):
        relative = path.resolve().relative_to(root.resolve())
        digest.update(str(relative).encode())
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def plan_document(
    *,
    seed: int,
    warmup_runs: int,
    sample_runs: int,
    implementation_sha256: str,
    execution_identity: Mapping[str, Any],
    parameters: Mapping[str, int | float | str],
) -> dict[str, Any]:
    schedule = build_schedule(seed, warmup_runs, sample_runs)
    document = {
        "schema": "splinterm.benchmark.multiplexer-matrix-plan.v1",
        "seed": seed,
        "warmup_runs": warmup_runs,
        "sample_runs": sample_runs,
        "stacks": list(STACKS),
        "topologies": list(TOPOLOGIES),
        "cases": list(CASES),
        "implementation_sha256": implementation_sha256,
        "execution_identity": dict(execution_identity),
        "execution_identity_sha256": document_sha256(execution_identity),
        "parameters": dict(parameters),
        "schedule": [cell.as_dict() for cell in schedule],
    }
    document["plan_sha256"] = document_sha256(document)
    return document


def validate_plan_semantics(plan: Mapping[str, Any]) -> None:
    expected_schedule = [
        cell.as_dict()
        for cell in build_schedule(
            int(plan["seed"]), int(plan["warmup_runs"]), int(plan["sample_runs"])
        )
    ]
    if plan.get("schedule") != expected_schedule:
        raise ValueError("matrix plan schedule is incomplete, duplicated, or reordered")
    identity = plan.get("execution_identity")
    if not isinstance(identity, Mapping) or plan.get(
        "execution_identity_sha256"
    ) != document_sha256(identity):
        raise ValueError("matrix plan execution identity digest is invalid")
    unsigned = dict(plan)
    recorded = unsigned.pop("plan_sha256", None)
    if recorded != document_sha256(unsigned):
        raise ValueError("matrix plan digest is invalid")


def assert_resume_compatible(
    existing: Mapping[str, Any], expected: Mapping[str, Any]
) -> None:
    fields = (
        "schema",
        "seed",
        "warmup_runs",
        "sample_runs",
        "stacks",
        "topologies",
        "cases",
        "implementation_sha256",
        "execution_identity",
        "execution_identity_sha256",
        "parameters",
        "schedule",
        "plan_sha256",
    )
    mismatches = [
        field for field in fields if existing.get(field) != expected.get(field)
    ]
    if mismatches:
        raise ValueError(
            "resume plan does not match current implementation or parameters: "
            + ", ".join(mismatches)
        )


def completed_cell_is_reusable(
    report: Mapping[str, Any], cell: MatrixCell, plan_sha256: str
) -> bool:
    return (
        report.get("schema") == "splinterm.benchmark.multiplexer-cell.v1"
        and report.get("case_id") == cell.case_id
        and report.get("plan_sha256") == plan_sha256
        and report.get("phase") == cell.phase
        and report.get("iteration") == cell.iteration
        and report.get("execution_index") == cell.execution_index
        and report.get("stack", {}).get("name") == cell.stack
        and report.get("topology", {}).get("name") == cell.topology
        and report.get("valid") is True
        and report.get("cleanup", {}).get("verified") is True
    )


def document_sha256(value: Mapping[str, Any]) -> str:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(encoded).hexdigest()


def _validate_identity(stack: str, topology: str, case: str) -> None:
    if stack not in STACKS:
        raise ValueError(f"unsupported stack: {stack}")
    if topology not in TOPOLOGIES:
        raise ValueError(f"unsupported topology: {topology}")
    if case not in CASES:
        raise ValueError(f"unsupported multiplexer case: {case}")
