#!/usr/bin/env python3
"""Create and clean one isolated multiplexer topology without opening a window."""

from __future__ import annotations

import argparse
import json
import pathlib
import subprocess
import sys
import time
from typing import Any

from headless_multiplexer import (
    ProcessIdentity,
    controller_for,
    pane_commands,
    process_identity,
    same_process,
    terminate_processes_exact,
    verify_process_roles,
    wait_for_ready,
    wait_processes_gone,
)
from metrics import process_tree
from multiplexers.tmux import TmuxAdapter
from multiplexers.zellij import ZellijAdapter
from multiplexing import topology_named

ROOT = pathlib.Path(__file__).resolve().parents[2]
EXPECTED_ERRORS = (OSError, RuntimeError, ValueError, subprocess.SubprocessError)


def ambient_counts(implementation: str) -> dict[str, int | None]:
    if implementation == "tmux":
        identity = TmuxAdapter().probe(ROOT)
    elif implementation == "zellij":
        identity = ZellijAdapter().probe(ROOT)
    else:
        return {"process_count": None, "default_session_count": None}
    return {
        "process_count": identity.ambient_process_count,
        "default_session_count": identity.default_session_count,
    }


def wait_namespace_absent(controller, timeout: float) -> bool:
    deadline = time.monotonic() + timeout
    while not controller.namespace_absent():
        if time.monotonic() >= deadline:
            return False
        time.sleep(0.02)
    return True


def ready_identities(output: pathlib.Path) -> list[ProcessIdentity]:
    identities = []
    for path in sorted(output.glob("pane-*-ready.json")):
        try:
            value = json.loads(path.read_text(encoding="utf-8"))
            identities.append(process_identity(int(value["pid"])))
        except (
            OSError,
            KeyError,
            TypeError,
            ValueError,
            RuntimeError,
            json.JSONDecodeError,
        ):
            continue
    unique = {(item.pid, item.start_ticks): item for item in identities}
    return list(unique.values())


def process_forest_identities(
    server: ProcessIdentity | None,
    proc_root: pathlib.Path = pathlib.Path("/proc"),
) -> list[ProcessIdentity]:
    if server is None or not same_process(server, proc_root):
        return []
    identities = []
    for pid in process_tree(proc_root, server.pid):
        try:
            identities.append(process_identity(pid, proc_root))
        except RuntimeError:
            continue
    if not same_process(server, proc_root):
        return []
    return identities


def role_identities(roles: dict[str, Any] | None) -> list[ProcessIdentity]:
    if roles is None:
        return []
    identities = []
    for role in roles["roles"]:
        identities.extend(
            ProcessIdentity(int(item["pid"]), int(item["start_ticks"]))
            for item in role["processes"]
        )
    unique = {(item.pid, item.start_ticks): item for item in identities}
    return list(unique.values())


def write_report(output: pathlib.Path, value: dict[str, Any]) -> None:
    path = output / "report.json"
    temporary = output / ".report.json.tmp"
    temporary.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    temporary.replace(path)


def run_case(args: argparse.Namespace) -> dict[str, Any]:
    topology = topology_named(args.topology)
    output = args.output.resolve()
    if output.exists() and any(output.iterdir()):
        raise RuntimeError(f"output directory is not empty: {output}")
    output.mkdir(parents=True, exist_ok=True)
    run_id = args.run_id or f"{args.implementation}-{int(time.time_ns() % 10**12)}"
    controller = controller_for(args.implementation, topology, output, run_id)
    ambient_before = ambient_counts(args.implementation)
    started_ns = time.monotonic_ns()
    runtime_ids: dict[str, str] = {}
    readiness: dict[str, dict[str, int | str]] = {}
    inspection: dict[str, Any] | None = None
    roles: dict[str, Any] | None = None
    server: ProcessIdentity | None = None
    failure: dict[str, str] | None = None
    cleanup_failure: dict[str, str] | None = None
    captured_processes: list[ProcessIdentity] = []
    workloads: list[ProcessIdentity] = []

    try:
        commands = pane_commands(topology, output, args.idle_seconds)
        runtime_ids = controller.start(commands)
        readiness = wait_for_ready(topology, output, args.ready_timeout)
        workloads = [
            process_identity(int(readiness[name]["pid"]))
            for name in topology.pane_names
        ]
        server = controller.server_identity
        inspection = controller.inspect()
        roles = verify_process_roles(server, readiness)
        captured_processes = role_identities(roles)
    except EXPECTED_ERRORS as error:  # evidence is serialized before returning nonzero
        failure = {"type": type(error).__name__, "message": str(error)}
    finally:
        if server is None:
            try:
                server = controller.server_identity
            except (RuntimeError, AttributeError):
                pass
        if not workloads:
            workloads = ready_identities(output)
        if not captured_processes:
            captured_processes = process_forest_identities(server)
        try:
            controller.cleanup()
        except EXPECTED_ERRORS as error:
            cleanup_failure = {"type": type(error).__name__, "message": str(error)}
        if not wait_processes_gone(captured_processes, 1.0):
            terminate_processes_exact(captured_processes)

    if not workloads:
        workloads = ready_identities(output)
    namespace_absent = wait_namespace_absent(controller, 5.0)
    server_absent = server is None or not same_process(server)
    workloads_absent = wait_processes_gone(workloads, 5.0)
    process_forest_absent = wait_processes_gone(captured_processes, 5.0)
    ambient_after = ambient_counts(args.implementation)
    ambient_unchanged = ambient_before == ambient_after
    cleanup_verified = (
        cleanup_failure is None
        and namespace_absent
        and server_absent
        and workloads_absent
        and process_forest_absent
        and ambient_unchanged
    )
    if readiness:
        all_ready_ns = max(int(item["monotonic_ns"]) for item in readiness.values())
        launch_to_all_ready_ns: int | None = all_ready_ns - started_ns
    else:
        launch_to_all_ready_ns = None
    panes = [
        {
            "name": name,
            "runtime_id": runtime_ids.get(name),
            "workload": (
                {
                    "pid": int(readiness[name]["pid"]),
                    "ready_monotonic_ns": int(readiness[name]["monotonic_ns"]),
                }
                if name in readiness
                else None
            ),
        }
        for name in topology.pane_names
    ]
    valid = failure is None and cleanup_verified
    report = {
        "schema": "splinterm.benchmark.multiplexer-headless.v1",
        "implementation": args.implementation,
        "topology": {
            "name": topology.name,
            "pane_count": len(topology.pane_names),
            "panes": list(topology.pane_names),
        },
        "boundary": {
            "clock": "CLOCK_MONOTONIC shared host namespace",
            "launch_to_all_children_ready_ns": launch_to_all_ready_ns,
        },
        "server": (
            {"role": controller.server_role, **server.as_dict()} if server else None
        ),
        "panes": panes,
        "process_roles": roles,
        "inspection": inspection,
        "isolation": {
            "run_id": run_id,
            "ambient_before": ambient_before,
            "ambient_after": ambient_after,
            "ambient_counts_unchanged": ambient_unchanged,
            "ambient_names_recorded": False,
            "graphical": False,
        },
        "cleanup": {
            "invoked": True,
            "namespace_absent": namespace_absent,
            "server_absent": server_absent,
            "workloads_absent": workloads_absent,
            "process_forest_absent": process_forest_absent,
            "verified": cleanup_verified,
            "failure": cleanup_failure,
        },
        "failure": failure,
        "valid": valid,
        "notes": [
            "Headless orchestration evidence only; no terminal window was launched.",
            "Suppressed Zellij internal plugin records are not visible plugin UI.",
        ],
    }
    write_report(output, report)
    return report


def parser() -> argparse.ArgumentParser:
    value = argparse.ArgumentParser(
        description="Run one disposable non-graphical multiplexer topology"
    )
    value.add_argument("output", type=pathlib.Path)
    value.add_argument(
        "--implementation", choices=("splinterm", "tmux", "zellij"), required=True
    )
    value.add_argument(
        "--topology", choices=("single", "two-columns", "four-grid"), required=True
    )
    value.add_argument("--run-id")
    value.add_argument("--idle-seconds", type=float, default=30.0)
    value.add_argument("--ready-timeout", type=float, default=10.0)
    return value


def main() -> int:
    args = parser().parse_args()
    if not 1 <= args.idle_seconds <= 300:
        print("--idle-seconds must be between 1 and 300", file=sys.stderr)
        return 2
    if not 1 <= args.ready_timeout <= 60:
        print("--ready-timeout must be between 1 and 60", file=sys.stderr)
        return 2
    try:
        report = run_case(args)
    except EXPECTED_ERRORS as error:
        print(f"headless multiplexer setup failed: {error}", file=sys.stderr)
        return 1
    print(f"Headless multiplexer report: {args.output.resolve() / 'report.json'}")
    print(f"Result: {'PASS' if report['valid'] else 'FAIL'}")
    return 0 if report["valid"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
