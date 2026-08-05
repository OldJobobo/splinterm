#!/usr/bin/env python3
"""Run one guarded measured multiplexer stack/topology cell."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import pathlib
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from collections.abc import Mapping, Sequence
from typing import Any

ROOT = pathlib.Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools/benchmark"
SMOKE_PATH = TOOLS / "run-graphical-multiplexer-smoke.py"
OUTPUT_PATH = TOOLS / "run-graphical-output.py"
BENCH_CHILD = TOOLS / "workloads/bench-child.py"
FOOT_PROFILE = TOOLS / "profiles/foot.ini"
APP_IDS = {
    "splinterm-native": "com.oldjobobo.splinterm",
    "foot-bare": "com.oldjobobo.splinterbench.FootBare",
    "foot-tmux": "com.oldjobobo.splinterbench.FootTmux",
    "foot-zellij": "com.oldjobobo.splinterbench.FootZellij",
}
IMPLEMENTATIONS = {
    "splinterm-native": "splinterm",
    "foot-tmux": "tmux",
    "foot-zellij": "zellij",
}
SIZES = ((800, 500), (1200, 700))
EXPECTED_ERRORS = (
    OSError,
    RuntimeError,
    TimeoutError,
    TypeError,
    ValueError,
    subprocess.SubprocessError,
)

sys.path.insert(0, str(TOOLS))
from graphical_multiplexer import (
    advance_sequences,
    case_stub,
    controlled_pane_commands,
    exact_resource_snapshot,
    resource_delta,
    splinterm_lifecycle_window_state,
    stack_identity,
    topology_rectangles,
    validate_equal_topology_geometry,
    validate_topology_geometry,
    wait_child_results,
    write_child_command,
)
from headless_multiplexer import (
    HeadlessController,
    ProcessIdentity,
    SplintermController,
    ZellijController,
    process_identity,
    same_process,
    terminate_processes_exact,
    wait_for_ready,
    wait_processes_gone,
)
from multiplexer_matrix import CASES, STACKS, TOPOLOGIES
from multiplexing import Topology, topology_named


def load(path: pathlib.Path, name: str):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


SMOKE = load(SMOKE_PATH, "splinterbench_measured_multiplexer_smoke")
OUTPUT = load(OUTPUT_PATH, "splinterbench_measured_multiplexer_output")
COMMON = SMOKE.COMMON
V1 = COMMON.V1


def atomic_json(path: pathlib.Path, value: Any) -> None:
    temporary = path.with_name(f".{path.name}.tmp")
    temporary.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    temporary.replace(path)


def assert_host_isolation(benchmark_addresses: set[str] | None = None) -> None:
    V1.assert_user_workspace_untouched()
    active = V1.hyprland_json("activewindow").get("address")
    if benchmark_addresses and active in benchmark_addresses:
        raise RuntimeError("a benchmark window gained focus")


def assert_window_group(
    app_id: str, addresses: set[str], _expected_user_state: Mapping[str, Any]
) -> None:
    assert_host_isolation(addresses)
    clients = [
        item
        for item in V1.all_clients()
        if item.get("workspace", {}).get("id") == V1.TEST_WORKSPACE
    ]
    observed = {str(item["address"]) for item in clients}
    if observed != addresses:
        raise RuntimeError(
            f"reserved workspace window set changed: expected={sorted(addresses)} "
            f"observed={sorted(observed)}"
        )
    if any(
        item.get("class") != app_id or item.get("monitor") != V1.test_monitor_id()
        for item in clients
    ):
        raise RuntimeError("benchmark window group changed class or monitor")


def window_by_address(address: str) -> dict[str, Any] | None:
    return next(
        (item for item in V1.all_clients() if str(item.get("address")) == address),
        None,
    )


def capture_window(window: Mapping[str, Any]) -> dict[str, Any]:
    captured = dict(window)
    captured["start_ticks"] = process_identity(int(window["pid"])).start_ticks
    return captured


def wait_window_size(address: str, size: tuple[int, int], timeout: float = 3.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        window = window_by_address(address)
        if window is None:
            raise RuntimeError("benchmark window exited during resize")
        if tuple(int(value) for value in window["size"]) == size:
            return
        V1.assert_user_workspace_untouched()
        time.sleep(0.01)
    raise TimeoutError(f"window {address} did not settle at {size}")


def resize_window(address: str, width: int, height: int) -> None:
    selector = json.dumps(f"address:{address}")
    expression = (
        "hl.dispatch(hl.dsp.window.resize("
        f"{{ x = {width}, y = {height}, window = {selector} }}))"
    )
    result = V1.run(["hyprctl", "eval", expression], capture_output=True, timeout=5)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or result.stdout.strip())
    wait_window_size(address, (width, height))


def monitor_origin() -> tuple[int, int]:
    monitor = next(
        (item for item in V1.hyprland_json("monitors") if item.get("name") == "DP-2"),
        None,
    )
    if monitor is None:
        raise RuntimeError("DP-2 is unavailable")
    width, height = int(monitor["width"]), int(monitor["height"])
    return (
        int(monitor["x"]) + max(0, (width - 960) // 2),
        int(monitor["y"]) + max(0, (height - 600) // 2),
    )


def arrange_bare_windows(
    topology: Topology,
    pane_windows: Mapping[str, dict[str, Any]],
    width: int,
    height: int,
) -> None:
    origin_x, origin_y = monitor_origin()
    rectangles = topology_rectangles(topology.name, origin_x, origin_y, width, height)
    for name in topology.pane_names:
        x, y, pane_width, pane_height = rectangles[name]
        address = str(pane_windows[name]["address"])
        resize_window(address, pane_width, pane_height)
        COMMON.move_window_absolute(address, x, y)


def wait_stack_window(
    app_id: str,
    existing: set[str],
    observed: set[str],
    owned_token: str,
    timeout: float,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        assert_host_isolation(observed)
        candidates = [
            item
            for item in V1.all_clients()
            if item.get("class") == app_id
            and str(item.get("address")) not in existing
            and SMOKE.process_has_cmdline_token(int(item["pid"]), owned_token)
        ]
        observed.update(str(item["address"]) for item in candidates)
        if len(candidates) > 1:
            raise RuntimeError("one stack mapped multiple owned windows")
        if candidates:
            window = candidates[0]
            if (
                window.get("workspace", {}).get("id") != V1.TEST_WORKSPACE
                or window.get("monitor") != V1.test_monitor_id()
            ):
                raise RuntimeError("stack window escaped workspace 8 / DP-2")
            COMMON.assert_owned_window(app_id, str(window["address"]))
            assert_host_isolation({str(window["address"])})
            return window
        time.sleep(0.01)
    raise TimeoutError("multiplexer stack window did not map")


def wait_bare_window(
    app_id: str,
    existing: set[str],
    ready_token: str,
    expected_user_state: Mapping[str, Any],
    timeout: float,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        assert_host_isolation()
        candidates = [
            item
            for item in V1.all_clients()
            if item.get("class") == app_id
            and str(item.get("address")) not in existing
            and SMOKE.process_has_cmdline_token(int(item["pid"]), ready_token)
        ]
        if len(candidates) > 1:
            raise RuntimeError("one bare Foot pane mapped multiple windows")
        if candidates:
            window = candidates[0]
            if (
                window.get("workspace", {}).get("id") != V1.TEST_WORKSPACE
                or window.get("monitor") != V1.test_monitor_id()
            ):
                raise RuntimeError("bare Foot window escaped workspace 8 / DP-2")
            return window
        time.sleep(0.01)
    raise TimeoutError("bare Foot pane window did not map")


def launch_bare(
    topology: Topology,
    commands: Mapping[str, Sequence[str]],
    state: pathlib.Path,
    expected_user_state: Mapping[str, Any],
    timeout: float,
) -> tuple[dict[str, dict[str, Any]], dict[str, dict[str, int | str]]]:
    foot = shutil.which("foot")
    if foot is None:
        raise RuntimeError("Foot is unavailable")
    existing = {str(item["address"]) for item in V1.all_clients()}
    pane_windows: dict[str, dict[str, Any]] = {}
    for name in topology.pane_names:
        command = [
            foot,
            "-c",
            str(FOOT_PROFILE),
            "-a",
            APP_IDS["foot-bare"],
            "-T",
            f"splinterbench-foot-bare-{name}",
            "-w",
            "960x600",
            *commands[name],
        ]
        launcher = state / f"launch-{name}.sh"
        COMMON.write_launcher(launcher, command, {})
        COMMON.dispatch_launcher(launcher)
        ready_token = str(state / "controller" / f"{name}-ready.json")
        window = wait_bare_window(
            APP_IDS["foot-bare"],
            existing,
            ready_token,
            expected_user_state,
            timeout,
        )
        pane_windows[name] = capture_window(window)
        existing.add(str(window["address"]))
    arrange_bare_windows(topology, pane_windows, 960, 600)
    assert_window_group(
        APP_IDS["foot-bare"],
        {str(item["address"]) for item in pane_windows.values()},
        expected_user_state,
    )
    readiness = wait_for_ready(topology, state / "controller", timeout)
    return pane_windows, readiness


def geometry_for_bare(
    topology: Topology, pane_windows: Mapping[str, dict[str, Any]]
) -> list[dict[str, Any]]:
    panes = []
    for name in topology.pane_names:
        window = window_by_address(str(pane_windows[name]["address"]))
        if window is None:
            raise RuntimeError(f"bare Foot window disappeared: {name}")
        panes.append(
            {
                "name": name,
                "runtime_id": str(window["address"]),
                "x": int(window["at"][0]),
                "y": int(window["at"][1]),
                "columns": int(window["size"][0]),
                "rows": int(window["size"][1]),
            }
        )
    return panes


def stable_geometry(
    topology: Topology,
    controller: HeadlessController | None,
    pane_windows: Mapping[str, dict[str, Any]],
    expected_user_state: Mapping[str, Any],
    timeout: float = 5.0,
    *,
    require_equal: bool = True,
) -> list[dict[str, Any]]:
    deadline = time.monotonic() + timeout
    last: list[dict[str, Any]] | None = None
    stable_since = time.monotonic()
    while time.monotonic() < deadline:
        assert_host_isolation({str(item["address"]) for item in pane_windows.values()})
        current = (
            geometry_for_bare(topology, pane_windows)
            if controller is None
            else SMOKE.geometry(controller)
        )
        validate_topology_geometry(topology, current)
        if require_equal:
            validate_equal_topology_geometry(topology, current)
        if current != last:
            last = current
            stable_since = time.monotonic()
        elif time.monotonic() - stable_since >= 0.3:
            return current
        time.sleep(0.03)
    raise TimeoutError("multiplexer pane geometry did not settle")


def divider_ratio_milli(topology: Topology, panes: Sequence[Mapping[str, Any]]) -> int:
    by_name = {str(item["name"]): item for item in panes}
    if topology.name == "two-columns":
        first = int(by_name["pane-0"]["columns"])
        second = int(by_name["pane-1"]["columns"])
    elif topology.name == "four-grid":
        first = int(by_name["pane-0"]["rows"])
        second = int(by_name["pane-1"]["rows"])
    else:
        raise ValueError("single-pane topology has no divider ratio")
    return round(first * 1000 / (first + second))


def process_document(
    controller: HeadlessController | None,
    server: ProcessIdentity | None,
    readiness: Mapping[str, Mapping[str, int | str]],
    pane_windows: Mapping[str, Mapping[str, Any]],
) -> tuple[dict[str, Any], list[ProcessIdentity], list[ProcessIdentity]]:
    workloads = [process_identity(int(readiness[name]["pid"])) for name in readiness]
    windows = [process_identity(int(item["pid"])) for item in pane_windows.values()]
    clients: list[ProcessIdentity] = []
    helpers: list[ProcessIdentity] = []
    roles: list[dict[str, Any]] = []
    if controller is not None:
        assert server is not None
        window = windows[0]
        raw, clients, helpers = SMOKE.stack_process_roles(
            controller, server, readiness, window
        )
        server_role = (
            "daemon"
            if isinstance(controller, SplintermController)
            else "multiplexer-server"
        )
        role_names = {
            "server": server_role,
            "terminal-host": "terminal",
            "multiplexer-client": "multiplexer-client",
            "helper": "helper",
            "workload": "workload",
        }
        roles = [
            {"role": role_names[item["role"]], "processes": item["processes"]}
            for item in raw["roles"]
        ]
    else:
        roles = [
            {"role": "terminal", "processes": [item.as_dict() for item in windows]},
            {"role": "workload", "processes": [item.as_dict() for item in workloads]},
        ]
    infrastructure = [
        *([server] if server is not None else []),
        *windows,
        *clients,
        *helpers,
    ]
    pairs = {(item.pid, item.start_ticks) for item in [*infrastructure, *workloads]}
    if len(pairs) != len(infrastructure) + len(workloads):
        raise RuntimeError("measured process roles overlap")
    return (
        {
            "infrastructure_pids": sorted(item.pid for item in infrastructure),
            "workload_pids": sorted(item.pid for item in workloads),
            "roles": roles,
            "role_sets_disjoint": True,
        },
        infrastructure,
        workloads,
    )


def resource_pair(
    infrastructure: Sequence[ProcessIdentity], workloads: Sequence[ProcessIdentity]
) -> dict[str, Any]:
    identities = [*infrastructure, *workloads]
    missing = [item.as_dict() for item in identities if not same_process(item)]
    if missing:
        raise RuntimeError(
            f"process incarnation disappeared before sampling: {missing}"
        )
    resources = exact_resource_snapshot(
        [item.pid for item in infrastructure],
        [item.pid for item in workloads],
    )
    if any(not same_process(item) for item in identities):
        raise RuntimeError("process incarnation changed during resource sampling")
    return {
        "membership": {
            "infrastructure": [item.as_dict() for item in infrastructure],
            "workload": [item.as_dict() for item in workloads],
        },
        "resources": resources,
    }


def pane_marker_counts(
    output: pathlib.Path,
    topology: Topology,
    pane_windows: Mapping[str, Mapping[str, Any]],
) -> dict[str, int]:
    if OUTPUT.Image is None:
        raise RuntimeError("Pillow is required for pane marker detection")
    if len(pane_windows) == len(topology.pane_names):
        counts = {}
        for name in topology.pane_names:
            window = window_by_address(str(pane_windows[name]["address"]))
            if window is None:
                raise RuntimeError(f"pane window disappeared before capture: {name}")
            path = output / f"marker-{name}.png"
            OUTPUT.screenshot_marker(window, path)
            with OUTPUT.Image.open(path) as image:
                counts[name] = marker_pixels(image.convert("RGB"))
        return counts
    window = window_by_address(str(next(iter(pane_windows.values()))["address"]))
    if window is None:
        raise RuntimeError("multiplexer window disappeared before pane capture")
    path = output / "marker-stack.png"
    OUTPUT.screenshot_marker(window, path)
    with OUTPUT.Image.open(path) as image:
        rgb = image.convert("RGB")
        rectangles = topology_rectangles(topology.name, 0, 0, rgb.width, rgb.height)
        return {
            name: marker_pixels(rgb.crop((x, y, x + width, y + height)))
            for name, (x, y, width, height) in rectangles.items()
        }


def marker_pixels(image: Any) -> int:
    pixels = (
        image.get_flattened_data()
        if hasattr(image, "get_flattened_data")
        else image.getdata()
    )
    return sum(
        1
        for red, green, blue in pixels
        if OUTPUT.is_visible_marker_pixel(red, green, blue)
    )


def clear_markers(
    output: pathlib.Path,
    topology: Topology,
    pane_names: Sequence[str],
    sequences: dict[str, int],
    pane_windows: Mapping[str, Mapping[str, Any]],
    timeout: float,
) -> None:
    used = advance_sequences(sequences, pane_names)
    for name, sequence in used.items():
        write_child_command(output, name, sequence, "clear")
    wait_child_results(output, used, timeout)
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        counts = pane_marker_counts(output, topology, pane_windows)
        if all(counts[name] < 100 for name in pane_names):
            return
        time.sleep(0.01)
    raise TimeoutError("visible marker did not clear before the next trigger")


def wait_visible_markers(
    output: pathlib.Path,
    topology: Topology,
    pane_windows: Mapping[str, Mapping[str, Any]],
    pane_names: Sequence[str],
    trigger_ns: int,
    timeout: float,
) -> tuple[int, dict[str, dict[str, int]]]:
    deadline = time.monotonic() + timeout
    detected: dict[str, dict[str, int]] = {}
    while time.monotonic() < deadline:
        counts = pane_marker_counts(output, topology, pane_windows)
        observed_ns = time.monotonic_ns()
        for name in pane_names:
            if name not in detected and counts[name] >= 100:
                detected[name] = {
                    "detected_monotonic_ns": observed_ns,
                    "trigger_to_visible_marker_ns": observed_ns - trigger_ns,
                    "visible_marker_pixels": counts[name],
                }
        if len(detected) == len(pane_names):
            aggregate = max(
                item["trigger_to_visible_marker_ns"] for item in detected.values()
            )
            return aggregate, detected
        time.sleep(0.01)
    missing = sorted(set(pane_names) - set(detected))
    raise TimeoutError(f"visible marker polling timed out for panes: {missing}")


def wait_lifecycle_settled(
    controller: HeadlessController | None,
    topology: Topology,
    selected: str,
    pane_windows: Mapping[str, Mapping[str, Any]],
    server: ProcessIdentity | None,
    workloads: Sequence[ProcessIdentity],
    readiness: Mapping[str, Mapping[str, int | str]],
    timeout: float,
) -> dict[str, Any]:
    unaffected = [
        item for item in workloads if item.pid != int(readiness[selected]["pid"])
    ]
    deadline = time.monotonic() + timeout
    last_error: RuntimeError | None = None
    last_observation: dict[str, Any] | None = None
    while time.monotonic() < deadline:
        if not all(same_process(item) for item in unaffected):
            raise RuntimeError(
                "an unaffected workload exited during lifecycle settlement"
            )
        if controller is None:
            selected_address = str(pane_windows[selected]["address"])
            unaffected_windows = [
                str(pane_windows[name]["address"])
                for name in topology.pane_names
                if name != selected
            ]
            if window_by_address(selected_address) is None and all(
                window_by_address(address) is not None for address in unaffected_windows
            ):
                return {
                    "selected_runtime_id": selected_address,
                    "selected_state": "window-exited",
                    "remaining_runtime_ids": unaffected_windows,
                    "session_state": "not-applicable",
                    "server_state": "not-applicable",
                    "window_state": "selected-absent-unaffected-present",
                }
        else:
            try:
                state = controller.lifecycle_state(selected)
            except RuntimeError as error:
                last_error = error
                time.sleep(0.02)
                continue
            final_leaf = len(topology.pane_names) == 1
            server_alive = server is not None and same_process(server)
            window_alive = any(
                window_by_address(str(item["address"])) is not None
                for item in pane_windows.values()
            )
            last_observation = {
                "state": state,
                "server_alive": server_alive,
                "window_alive": window_alive,
            }
            if isinstance(controller, SplintermController):
                window_state = splinterm_lifecycle_window_state(
                    state["selected_state"],
                    final_leaf=final_leaf,
                    server_alive=server_alive,
                    window_alive=window_alive,
                )
                if window_state is not None:
                    return {
                        **state,
                        "server_state": "running",
                        "window_state": window_state,
                    }
            elif not final_leaf and server_alive and window_alive:
                return {
                    **state,
                    "server_state": "running",
                    "window_state": "running-with-unaffected-panes",
                }
            elif (
                final_leaf
                and isinstance(controller, ZellijController)
                and server_alive
                and window_alive
            ):
                return {
                    **state,
                    "server_state": "running-empty-session",
                    "window_state": "running-without-terminal-panes",
                }
            elif final_leaf and not server_alive and not window_alive:
                return {
                    **state,
                    "server_state": "exited",
                    "window_state": "exited-with-final-pane",
                }
        time.sleep(0.02)
    detail = f": {last_error}" if last_error is not None else ""
    observation = (
        f"; last_observation={json.dumps(last_observation, sort_keys=True)}"
        if last_observation is not None
        else ""
    )
    raise TimeoutError(f"stack lifecycle state did not settle{detail}{observation}")


def measured_operation(
    operation: dict[str, Any],
    boundary: str,
    metrics: Mapping[str, Any],
    pane_metrics: Mapping[str, Any],
    before: Mapping[str, Any],
    after: Mapping[str, Any],
) -> None:
    operation.update(
        boundary=boundary,
        metrics=dict(metrics),
        pane_metrics=dict(pane_metrics),
        resources=resource_delta(before, after),
        valid=True,
    )


def run_case(args: argparse.Namespace) -> dict[str, Any]:
    output = args.output.resolve()
    if output.exists() and any(output.iterdir()):
        raise RuntimeError(f"output directory is not empty: {output}")
    output.mkdir(parents=True, exist_ok=True)
    state = pathlib.Path(tempfile.mkdtemp(prefix="sb-mux-cell-"))
    controller_output = state / "controller"
    controller_output.mkdir()
    topology = topology_named(args.topology)
    operations = [case_stub(args.stack, args.topology, case) for case in CASES]
    by_case = {item["case"]: item for item in operations}
    controller: HeadlessController | None = None
    server: ProcessIdentity | None = None
    pane_windows: dict[str, dict[str, Any]] = {}
    observed_addresses: set[str] = set()
    readiness: dict[str, dict[str, int | str]] = {}
    runtime_ids: dict[str, str] = {}
    processes: dict[str, Any] | None = None
    infrastructure: list[ProcessIdentity] = []
    workloads: list[ProcessIdentity] = []
    captured: list[ProcessIdentity] = []
    original_user_state: dict[str, Any] | None = None
    final_user_state: dict[str, Any] | None = None
    ambient_before = SMOKE.ambient_counts(IMPLEMENTATIONS.get(args.stack, "foot"))
    ambient_after: dict[str, int | None] | None = None
    failure: str | None = None
    cleanup_failure: str | None = None
    sequences = {name: 0 for name in topology.pane_names}
    cleanup = {
        "windows_absent": False,
        "namespace_absent": False,
        "server_absent": False,
        "clients_absent": False,
        "workloads_absent": False,
        "process_forest_absent": False,
        "ambient_counts_unchanged": False,
        "verified": False,
    }
    started_ns = 0
    all_ready_ns = 0
    all_mapped_ns = 0

    try:
        V1.assert_test_workspace_isolated()
        original_user_state = SMOKE.user_state()
        assert_host_isolation()
        commands = controlled_pane_commands(
            topology,
            controller_output,
            BENCH_CHILD,
            args.lifetime_seconds,
            args.columns,
        )
        started_ns = time.monotonic_ns()
        if args.stack == "foot-bare":
            pane_windows, readiness = launch_bare(
                topology, commands, state, original_user_state, args.ready_timeout
            )
            runtime_ids = {
                name: str(pane_windows[name]["address"]) for name in topology.pane_names
            }
        else:
            namespace_run_id = (
                f"c{args.execution_index}-"
                f"{hashlib.sha256(args.case_id.encode()).hexdigest()[:12]}"
            )
            controller = SMOKE.controller_for(
                IMPLEMENTATIONS[args.stack],
                topology,
                controller_output,
                namespace_run_id,
            )
            runtime_ids = controller.start(commands)
            readiness = wait_for_ready(topology, controller_output, args.ready_timeout)
            server = controller.server_identity
            command, environment = SMOKE.launch_spec(args.stack, controller)
            launcher = state / "launch.sh"
            COMMON.write_launcher(launcher, command, environment)
            existing = {str(item["address"]) for item in V1.all_clients()}
            COMMON.dispatch_launcher(launcher)
            window = wait_stack_window(
                APP_IDS[args.stack],
                existing,
                observed_addresses,
                SMOKE.owned_window_token(controller),
                args.ready_timeout,
            )
            pane_windows = {"pane-0": capture_window(window)}
        observed_addresses.update(
            str(item["address"]) for item in pane_windows.values()
        )
        all_ready_ns = max(int(item["monotonic_ns"]) for item in readiness.values())
        all_mapped_ns = time.monotonic_ns()
        stable_geometry(topology, controller, pane_windows, original_user_state)
        time.sleep(args.settle_seconds)
        processes, infrastructure, workloads = process_document(
            controller, server, readiness, pane_windows
        )
        processes["role_history"] = [
            {
                "stage": "initial-attach",
                "infrastructure_pids": processes["infrastructure_pids"],
                "workload_pids": processes["workload_pids"],
                "roles": processes["roles"],
            }
        ]
        captured = [*infrastructure, *workloads]
        current = resource_pair(infrastructure, workloads)
        measured_operation(
            by_case["startup"],
            "topology-request-to-all-ready-and-mapped",
            {
                "topology_request_monotonic_ns": started_ns,
                "all_children_ready_monotonic_ns": all_ready_ns,
                "all_windows_mapped_monotonic_ns": all_mapped_ns,
                "request_to_all_children_ready_ns": all_ready_ns - started_ns,
                "request_to_all_windows_mapped_ns": all_mapped_ns - started_ns,
            },
            {
                name: {
                    "ready_monotonic_ns": int(readiness[name]["monotonic_ns"]),
                    "request_to_ready_ns": int(readiness[name]["monotonic_ns"])
                    - started_ns,
                }
                for name in topology.pane_names
            },
            current,
            current,
        )

        time.sleep(args.idle_warmup_seconds)
        before = resource_pair(infrastructure, workloads)
        time.sleep(args.idle_sample_seconds)
        assert_window_group(
            APP_IDS[args.stack], observed_addresses, original_user_state
        )
        after = resource_pair(infrastructure, workloads)
        measured_operation(
            by_case["idle"],
            "settled-idle-interval",
            {
                "warmup_seconds": args.idle_warmup_seconds,
                "sample_seconds": args.idle_sample_seconds,
            },
            {},
            before,
            after,
        )

        for workload_case in ("plain", "ansi", "unicode"):
            clear_markers(
                controller_output,
                topology,
                topology.pane_names,
                sequences,
                pane_windows,
                args.ready_timeout,
            )
            used = advance_sequences(sequences, topology.pane_names)
            before = resource_pair(infrastructure, workloads)
            trigger_ns = time.monotonic_ns()
            for name, sequence in used.items():
                write_child_command(
                    controller_output,
                    name,
                    sequence,
                    "output",
                    workload=workload_case,
                    lines=args.lines,
                    columns=args.columns,
                )
            records = wait_child_results(
                controller_output, used, args.operation_timeout
            )
            visible_ns, visible_panes = wait_visible_markers(
                controller_output,
                topology,
                pane_windows,
                topology.pane_names,
                trigger_ns,
                args.operation_timeout,
            )
            after = resource_pair(infrastructure, workloads)
            all_done = max(int(item["monotonic_ns"]) for item in records.values())
            measured_operation(
                by_case[workload_case],
                "trigger-to-write-complete-and-visible-marker-approximation",
                {
                    "trigger_monotonic_ns": trigger_ns,
                    "trigger_to_all_write_complete_ns": all_done - trigger_ns,
                    "trigger_to_all_visible_markers_ns": visible_ns,
                    "visible_marker_pixels": sum(
                        item["visible_marker_pixels"] for item in visible_panes.values()
                    ),
                    "lines_per_pane": args.lines,
                    "columns": args.columns,
                },
                {
                    name: {
                        "write_complete_monotonic_ns": int(record["monotonic_ns"]),
                        "trigger_to_write_complete_ns": int(record["monotonic_ns"])
                        - trigger_ns,
                        "child_write_duration_ns": int(record["duration_ns"]),
                        "payload_bytes": int(record["payload_bytes"]),
                        "total_bytes": int(record["total_bytes"]),
                        **visible_panes[name],
                    }
                    for name, record in records.items()
                },
                before,
                after,
            )

        before = resource_pair(infrastructure, workloads)
        resize_started = time.monotonic_ns()
        resize_steps = []
        for step_index, (width, height) in enumerate(SIZES * 6):
            step_started = time.monotonic_ns()
            if args.stack == "foot-bare":
                arrange_bare_windows(topology, pane_windows, width, height)
            else:
                address = next(iter(observed_addresses))
                resize_window(address, width, height)
            assert_window_group(
                APP_IDS[args.stack], observed_addresses, original_user_state
            )
            settled = stable_geometry(
                topology, controller, pane_windows, original_user_state
            )
            settled_ns = time.monotonic_ns()
            resize_steps.append(
                {
                    "step": step_index,
                    "aggregate_size": [width, height],
                    "dispatch_to_all_panes_settled_ns": settled_ns - step_started,
                    "panes": settled,
                }
            )
        resize_finished = time.monotonic_ns()
        after = resource_pair(infrastructure, workloads)
        measured_operation(
            by_case["outer-resize"],
            "outer-resize-dispatch-to-all-pane-geometry-settled",
            {
                "resize_count": 12,
                "settled_duration_ns": resize_finished - resize_started,
                "sizes": [list(item) for item in SIZES],
                "steps": resize_steps,
            },
            {
                name: {
                    "settled_steps": [
                        {
                            "step": step["step"],
                            "geometry": next(
                                pane for pane in step["panes"] if pane["name"] == name
                            ),
                            "dispatch_to_all_panes_settled_ns": step[
                                "dispatch_to_all_panes_settled_ns"
                            ],
                        }
                        for step in resize_steps
                    ]
                }
                for name in topology.pane_names
            },
            before,
            after,
        )

        divider = by_case["divider-resize"]
        if divider["applicability"]["status"] == "measured":
            assert controller is not None
            before_geometry = stable_geometry(
                topology, controller, pane_windows, original_user_state
            )
            before_ratio = divider_ratio_milli(topology, before_geometry)
            before = resource_pair(infrastructure, workloads)
            divider_started = time.monotonic_ns()
            controller.resize_divider("pane-0")
            deadline = time.monotonic() + args.operation_timeout
            after_geometry = before_geometry
            while time.monotonic() < deadline:
                after_geometry = stable_geometry(
                    topology,
                    controller,
                    pane_windows,
                    original_user_state,
                    require_equal=False,
                )
                if after_geometry != before_geometry:
                    break
            if after_geometry == before_geometry:
                raise TimeoutError("divider resize did not change pane geometry")
            after_ratio = divider_ratio_milli(topology, after_geometry)
            if not (450 <= before_ratio <= 550 and 520 <= after_ratio <= 580):
                raise RuntimeError(
                    "divider resize did not settle at the shared 550/1000 target: "
                    f"before={before_ratio} after={after_ratio}"
                )
            divider_finished = time.monotonic_ns()
            after = resource_pair(infrastructure, workloads)
            measured_operation(
                divider,
                "divider-dispatch-to-affected-pane-geometry-settled",
                {
                    "settled_duration_ns": divider_finished - divider_started,
                    "target_ratio_milli": 550,
                    "before_ratio_milli": before_ratio,
                    "after_ratio_milli": after_ratio,
                    "rounding_tolerance_milli": 30,
                },
                {
                    "before": before_geometry,
                    "after": after_geometry,
                },
                before,
                after,
            )

        selected = "pane-0"
        clear_markers(
            controller_output,
            topology,
            topology.pane_names,
            sequences,
            pane_windows,
            args.ready_timeout,
        )
        used = advance_sequences(sequences, (selected,))
        sequence = used[selected]
        write_child_command(controller_output, selected, sequence, "input", token="x")
        if controller is not None:
            controller.focus_pane(selected)
        before = resource_pair(infrastructure, workloads)
        input_started = time.monotonic_ns()
        address = str(
            pane_windows[selected if controller is None else "pane-0"]["address"]
        )
        for key in ("x", "Return"):
            send_window_key(address, key)
        input_result = wait_child_results(
            controller_output, used, args.operation_timeout
        )[selected]
        visible_ns, visible_panes = wait_visible_markers(
            controller_output,
            topology,
            pane_windows,
            (selected,),
            input_started,
            args.operation_timeout,
        )
        after = resource_pair(infrastructure, workloads)
        received_ns = int(input_result["monotonic_ns"])
        measured_operation(
            by_case["input"],
            "targeted-input-to-child-and-visible-marker-approximation",
            {
                "selected_pane": selected,
                "input_protocol": "Hyprland hl.dsp.send_key_state exact window address",
                "pane_selection": (
                    "captured runtime pane ID before exact-window injection"
                    if controller is not None
                    else "exact independent Foot window address"
                ),
                "injected_monotonic_ns": input_started,
                "input_to_child_ns": received_ns - input_started,
                "input_to_visible_marker_ns": visible_ns,
                "visible_marker_pixels": visible_panes[selected][
                    "visible_marker_pixels"
                ],
            },
            {selected: {**input_result, **visible_panes[selected]}},
            before,
            after,
        )

        detach = by_case["detach-reattach"]
        if detach["applicability"]["status"] == "measured":
            assert controller is not None and original_user_state is not None
            before = resource_pair(infrastructure, workloads)
            detach_started = time.monotonic_ns()
            old_addresses = set(observed_addresses)
            for address in sorted(old_addresses):
                V1.kill_oracle_window(address)
            wait_addresses_absent(old_addresses, args.ready_timeout)
            if server is not None and not same_process(server):
                raise RuntimeError("multiplexer server exited while detached")
            if not all(same_process(item) for item in workloads):
                raise RuntimeError("workload exited while detached")
            command, environment = SMOKE.launch_spec(args.stack, controller)
            launcher = state / "reattach.sh"
            COMMON.write_launcher(launcher, command, environment)
            existing = {str(item["address"]) for item in V1.all_clients()}
            COMMON.dispatch_launcher(launcher)
            observed_addresses.clear()
            window = wait_stack_window(
                APP_IDS[args.stack],
                existing,
                observed_addresses,
                SMOKE.owned_window_token(controller),
                args.ready_timeout,
            )
            pane_windows = {"pane-0": capture_window(window)}
            observed_addresses.add(str(window["address"]))
            stable_geometry(
                topology,
                controller,
                pane_windows,
                original_user_state,
                require_equal=topology.name == "single",
            )
            reattached_ns = time.monotonic_ns()
            refreshed, new_infrastructure, _ = process_document(
                controller, server, readiness, pane_windows
            )
            assert processes is not None
            role_history = [
                *processes["role_history"],
                {
                    "stage": "reattach",
                    "infrastructure_pids": refreshed["infrastructure_pids"],
                    "workload_pids": refreshed["workload_pids"],
                    "roles": refreshed["roles"],
                },
            ]
            processes = {**refreshed, "role_history": role_history}
            captured.extend(new_infrastructure)
            infrastructure = new_infrastructure
            after = resource_pair(infrastructure, workloads)
            measured_operation(
                detach,
                "detach-to-client-exit-and-reattach-to-all-panes-visible",
                {
                    "detach_started_monotonic_ns": detach_started,
                    "reattached_monotonic_ns": reattached_ns,
                    "detach_reattach_duration_ns": reattached_ns - detach_started,
                    "server_survived": True,
                    "all_workloads_survived": True,
                },
                {},
                before,
                after,
            )

        used = advance_sequences(sequences, (selected,))
        sequence = used[selected]
        before = resource_pair(infrastructure, workloads)
        lifecycle_started = time.monotonic_ns()
        write_child_command(controller_output, selected, sequence, "exit")
        exit_result = wait_child_results(
            controller_output, used, args.operation_timeout
        )[selected]
        selected_identity = next(
            item for item in workloads if item.pid == int(readiness[selected]["pid"])
        )
        if not wait_processes_gone([selected_identity], args.operation_timeout):
            raise TimeoutError("selected workload did not exit")
        lifecycle_state = wait_lifecycle_settled(
            controller,
            topology,
            selected,
            pane_windows,
            server,
            workloads,
            readiness,
            args.operation_timeout,
        )
        lifecycle_finished = time.monotonic_ns()
        surviving_infrastructure = [
            item for item in infrastructure if same_process(item)
        ]
        surviving_workloads = [item for item in workloads if same_process(item)]
        after = resource_pair(surviving_infrastructure, surviving_workloads)
        measured_operation(
            by_case["lifecycle"],
            "child-exit-to-documented-stack-lifecycle-state",
            {
                "selected_pane": selected,
                "exit_started_monotonic_ns": int(exit_result["monotonic_ns"]),
                "exit_observed_monotonic_ns": lifecycle_finished,
                "child_exit_to_observed_state_ns": lifecycle_finished
                - int(exit_result["monotonic_ns"]),
                "trigger_to_observed_state_ns": lifecycle_finished - lifecycle_started,
                "workload_absent": True,
                "settled_state": lifecycle_state,
                "unaffected_workloads_running": len(surviving_workloads)
                == len(workloads) - 1,
            },
            {selected: exit_result},
            before,
            after,
        )
    except EXPECTED_ERRORS as error:
        failure = f"{type(error).__name__}: {error}"
    finally:
        try:
            for pane_name, captured_window in tuple(pane_windows.items()):
                current_window = window_by_address(str(captured_window["address"]))
                if current_window is not None:
                    pane_windows[pane_name] = capture_window(current_window)
            for address in sorted(observed_addresses):
                if window_by_address(address) is not None:
                    V1.kill_oracle_window(address)
            wait_addresses_absent(observed_addresses, 5.0)
            cleanup["windows_absent"] = True
        except EXPECTED_ERRORS as error:
            cleanup_failure = f"window cleanup: {type(error).__name__}: {error}"
        if controller is not None:
            try:
                controller.cleanup()
            except EXPECTED_ERRORS as error:
                detail = f"namespace cleanup: {type(error).__name__}: {error}"
                cleanup_failure = (
                    f"{cleanup_failure}; {detail}" if cleanup_failure else detail
                )
            cleanup["namespace_absent"] = SMOKE.wait_namespace_absent(controller, 5.0)
        else:
            cleanup["namespace_absent"] = True
        if not wait_processes_gone(captured, 1.0):
            terminate_processes_exact(captured)
        cleanup["server_absent"] = server is None or not same_process(server)
        cleanup["clients_absent"] = wait_processes_gone(infrastructure, 5.0)
        cleanup["workloads_absent"] = wait_processes_gone(workloads, 5.0)
        cleanup["process_forest_absent"] = wait_processes_gone(captured, 5.0)
        ambient_after = SMOKE.ambient_counts(IMPLEMENTATIONS.get(args.stack, "foot"))
        cleanup["ambient_counts_unchanged"] = ambient_before == ambient_after
        try:
            if original_user_state is not None:
                assert_host_isolation(observed_addresses)
                final_user_state = SMOKE.user_state()
        except EXPECTED_ERRORS as error:
            detail = f"host state cleanup: {type(error).__name__}: {error}"
            cleanup_failure = (
                f"{cleanup_failure}; {detail}" if cleanup_failure else detail
            )
        cleanup["verified"] = cleanup_failure is None and all(
            cleanup[key]
            for key in (
                "windows_absent",
                "namespace_absent",
                "server_absent",
                "clients_absent",
                "workloads_absent",
                "process_forest_absent",
                "ambient_counts_unchanged",
            )
        )

    report = {
        "schema": "splinterm.benchmark.multiplexer-cell.v1",
        "case_id": args.case_id,
        "plan_sha256": args.plan_sha256,
        "phase": args.phase,
        "iteration": args.iteration,
        "execution_index": args.execution_index,
        "stack": stack_identity(args.stack),
        "topology": topology.as_dict(),
        "runtime_ids": runtime_ids,
        "windows": window_records(pane_windows, args.stack),
        "processes": processes,
        "operations": operations,
        "isolation": {
            "namespace": namespace_name(controller, args.case_id),
            "workspace": 8,
            "monitor": "DP-2",
            "no_initial_focus": True,
            "host_state_before": original_user_state,
            "host_state_after": final_user_state,
            "host_state_preserved": original_user_state is not None
            and final_user_state is not None
            and cleanup_failure is None,
            "ambient_before": ambient_before,
            "ambient_after": ambient_after,
            "ambient_names_recorded": False,
        },
        "cleanup": {**cleanup, "failure": cleanup_failure},
        "failure": failure,
        "valid": failure is None
        and cleanup["verified"]
        and all(item["valid"] for item in operations),
        "notes": [
            "Independent Foot uses one window per pane at equivalent aggregate geometry.",
            "Screenshot polling is a visible-marker approximation, not presentation latency.",
            "Warmup cell operations are retained but excluded from measured summaries."
            if args.phase == "warmup"
            else "Measured cell operations contribute to development summaries.",
        ],
    }
    atomic_json(output / "report.json", report)
    if not report["valid"]:
        SMOKE.copy_diagnostics(state, output)
    shutil.rmtree(state, ignore_errors=True)
    return report


def send_window_key(address: str, key: str) -> None:
    selector = json.dumps(f"address:{address}")
    for state in ("down", "up"):
        expression = (
            "hl.dsp.send_key_state({ "
            f"mods = '', key = {json.dumps(key)}, state = {json.dumps(state)}, "
            f"window = {selector} }})"
        )
        result = V1.run(
            ["hyprctl", "dispatch", expression], capture_output=True, timeout=5
        )
        if result.returncode:
            raise RuntimeError(result.stderr.strip() or result.stdout.strip())


def wait_addresses_absent(addresses: set[str], timeout: float) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        current = {str(item["address"]) for item in V1.all_clients()}
        if not addresses & current:
            return
        time.sleep(0.02)
    raise TimeoutError(f"benchmark windows survived cleanup: {sorted(addresses)}")


def window_records(
    pane_windows: Mapping[str, Mapping[str, Any]], stack: str
) -> list[dict[str, Any]]:
    records = []
    for pane, captured in pane_windows.items():
        current = window_by_address(str(captured["address"])) or dict(captured)
        identity = ProcessIdentity(
            int(current["pid"]),
            int(
                captured.get("start_ticks")
                or process_identity(int(current["pid"])).start_ticks
            ),
        )
        x, y = (int(value) for value in current["at"])
        width, height = (int(value) for value in current["size"])
        records.append(
            {
                "pane": pane if stack == "foot-bare" else None,
                "address": str(current["address"]),
                **identity.as_dict(),
                "class": str(current["class"]),
                "workspace": int(current["workspace"]["id"]),
                "monitor": int(current["monitor"]),
                "geometry": [x, y, width, height],
            }
        )
    return records


def namespace_name(controller: HeadlessController | None, case_id: str) -> str:
    if controller is None:
        return f"foot-bare:{case_id}"
    if isinstance(controller, SplintermController):
        return str(controller.socket)
    return str(controller.plan.runtime_directory)


def parser() -> argparse.ArgumentParser:
    value = argparse.ArgumentParser(
        description="Run one guarded multiplexer stack/topology measurement cell"
    )
    value.add_argument("output", type=pathlib.Path)
    value.add_argument("--stack", choices=STACKS, required=True)
    value.add_argument("--topology", choices=TOPOLOGIES, required=True)
    value.add_argument("--case-id", required=True)
    value.add_argument("--plan-sha256", required=True)
    value.add_argument("--phase", choices=("warmup", "measured"), required=True)
    value.add_argument("--iteration", type=int, required=True)
    value.add_argument("--execution-index", type=int, required=True)
    value.add_argument("--idle-warmup-seconds", type=float, default=1.0)
    value.add_argument("--idle-sample-seconds", type=float, default=2.0)
    value.add_argument("--lines", type=int, default=2000)
    value.add_argument("--columns", type=int, default=80)
    value.add_argument("--ready-timeout", type=float, default=10.0)
    value.add_argument("--settle-seconds", type=float, default=0.5)
    value.add_argument("--operation-timeout", type=float, default=20.0)
    value.add_argument("--lifetime-seconds", type=float, default=300.0)
    return value


def interrupt_cell(_signum: int, _frame: Any) -> None:
    raise InterruptedError("matrix requested bounded cell cleanup")


def main() -> int:
    args = parser().parse_args()
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        print("a running Hyprland session is required", file=sys.stderr)
        return 2
    if not (
        args.iteration >= 0
        and args.execution_index >= 0
        and args.idle_warmup_seconds >= 0
        and args.idle_sample_seconds > 0
        and args.lines > 0
        and args.columns >= 20
        and args.ready_timeout > 0
        and args.settle_seconds >= 0
        and args.operation_timeout > 0
        and args.lifetime_seconds > 0
    ):
        print("invalid benchmark dimensions or timeout", file=sys.stderr)
        return 2
    if len(args.plan_sha256) != 64:
        print("--plan-sha256 must be a SHA-256 digest", file=sys.stderr)
        return 2
    signal.signal(signal.SIGTERM, interrupt_cell)
    try:
        report = run_case(args)
    except EXPECTED_ERRORS as error:
        print(f"graphical multiplexer setup failed: {error}", file=sys.stderr)
        return 1
    print(f"Multiplexer cell: {args.output.resolve() / 'report.json'}")
    print(f"Result: {'PASS' if report['valid'] else 'FAIL'}")
    return 0 if report["valid"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
