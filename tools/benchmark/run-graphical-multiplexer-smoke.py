#!/usr/bin/env python3
"""Run one guarded two-column multiplexer rendering smoke on workspace 8 / DP-2."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time
from collections.abc import Mapping, Sequence
from typing import Any

ROOT = pathlib.Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools/benchmark"
COMMON_PATH = TOOLS / "run-graphical-idle.py"
FOOT_PROFILE = TOOLS / "profiles/foot.ini"
STACKS = ("splinterm-native", "foot-tmux", "foot-zellij")
APP_IDS = {
    "splinterm-native": "com.oldjobobo.splinterm",
    "foot-tmux": "com.oldjobobo.splinterbench.FootTmux",
    "foot-zellij": "com.oldjobobo.splinterbench.FootZellij",
}
IMPLEMENTATIONS = {
    "splinterm-native": "splinterm",
    "foot-tmux": "tmux",
    "foot-zellij": "zellij",
}

sys.path.insert(0, str(TOOLS))
from headless_multiplexer import (
    HeadlessController,
    ProcessIdentity,
    SplintermController,
    TmuxController,
    ZellijController,
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
from multiplexing import Topology, topology_named


def load_common():
    spec = importlib.util.spec_from_file_location(
        "splinterbench_multiplexer_graphical_common", COMMON_PATH
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


COMMON = load_common()
V1 = COMMON.V1
EXPECTED_ERRORS = (
    OSError,
    RuntimeError,
    TimeoutError,
    ValueError,
    subprocess.SubprocessError,
)


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


def cursor_position() -> dict[str, int]:
    value = V1.hyprland_json("cursorpos")
    return {"x": int(value["x"]), "y": int(value["y"])}


def user_state() -> dict[str, Any]:
    workspace = V1.hyprland_json("activeworkspace")
    window = V1.hyprland_json("activewindow")
    return {
        "workspace": {
            "id": int(workspace["id"]),
            "monitor": str(workspace["monitor"]),
        },
        "focus_address": window.get("address"),
        "cursor": cursor_position(),
    }


def assert_user_state(expected: Mapping[str, Any]) -> None:
    V1.assert_user_workspace_untouched()
    observed = user_state()
    if observed != expected:
        raise RuntimeError(
            "graphical smoke changed host workspace, focus, or pointer: "
            f"expected={dict(expected)!r} observed={observed!r}"
        )


def launch_spec(
    stack: str, controller: HeadlessController
) -> tuple[list[str], dict[str, str]]:
    if stack == "splinterm-native":
        if not isinstance(controller, SplintermController):
            raise TypeError("native stack requires a Splinterm controller")
        if controller.dojo_id is None:
            raise RuntimeError("native stack has no benchmark window identity")
        topology = controller._json_command(["topology"])
        lair_ids = {
            str(item["lair_id"])
            for item in topology["data"]["splints"]
            if item["dojo_id"] == controller.dojo_id
            and item["splint_id"] in controller.runtime_ids.values()
        }
        if len(lair_ids) != 1:
            raise RuntimeError("native benchmark window has no unique dojo identity")
        return (
            [
                str(controller.client),
                "window",
                "--lair-id",
                lair_ids.pop(),
                "--dojo-id",
                controller.dojo_id,
            ],
            dict(controller.environment),
        )

    foot = shutil.which("foot")
    if foot is None:
        raise RuntimeError("Foot is unavailable")
    command = [
        foot,
        "-c",
        str(FOOT_PROFILE),
        "-a",
        APP_IDS[stack],
        "-T",
        f"splinterbench-{stack}",
        "-w",
        "960x600",
    ]
    if stack == "foot-tmux":
        if not isinstance(controller, TmuxController):
            raise TypeError("tmux stack requires a tmux controller")
        child = [
            *controller.plan.command_prefix,
            "attach-session",
            "-t",
            controller.plan.session_name,
        ]
    elif stack == "foot-zellij":
        if not isinstance(controller, ZellijController):
            raise TypeError("Zellij stack requires a Zellij controller")
        child = [
            controller.plan.command_prefix[0],
            "--config",
            controller.plan.command_prefix[2],
            "attach",
            controller.plan.session_name,
        ]
    else:
        raise ValueError(f"unsupported stack: {stack}")
    return [*command, *child], dict(controller.environment)


def owned_window_token(controller: HeadlessController) -> str:
    if isinstance(controller, SplintermController):
        if controller.dojo_id is None:
            raise RuntimeError("native stack has no benchmark window identity")
        return controller.dojo_id
    if isinstance(controller, (TmuxController, ZellijController)):
        return controller.plan.session_name
    raise TypeError("unsupported graphical controller")


def process_has_cmdline_token(pid: int, token: str) -> bool:
    try:
        fields = (
            (pathlib.Path("/proc") / str(pid) / "cmdline").read_bytes().split(b"\0")
        )
    except OSError:
        return False
    return os.fsencode(token) in fields


def wait_window(
    app_id: str,
    existing: set[str],
    observed: set[str],
    owned_token: str,
    expected_user_state: Mapping[str, Any],
    timeout: float = 10.0,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        assert_user_state(expected_user_state)
        candidates = [
            item
            for item in V1.all_clients()
            if item.get("class") == app_id
            and item.get("address") not in existing
            and process_has_cmdline_token(int(item["pid"]), owned_token)
        ]
        observed.update(str(item["address"]) for item in candidates)
        if len(candidates) > 1:
            raise RuntimeError("graphical smoke mapped multiple stack windows")
        if candidates:
            window = candidates[0]
            if (
                window.get("workspace", {}).get("id") != V1.TEST_WORKSPACE
                or window.get("monitor") != V1.test_monitor_id()
            ):
                raise RuntimeError("stack window escaped workspace 8 / DP-2")
            COMMON.assert_owned_window(app_id, str(window["address"]))
            return window
        time.sleep(0.01)
    raise TimeoutError("multiplexer stack window did not map")


def splinterm_geometry(controller: SplintermController) -> list[dict[str, Any]]:
    panes = []
    for name in controller.topology.pane_names:
        splint_id = controller.runtime_ids[name]
        result = subprocess.run(
            [
                str(controller.client),
                "--output",
                "json",
                "snapshot",
                splint_id,
            ],
            env=controller.environment,
            text=True,
            capture_output=True,
            check=False,
            timeout=10,
        )
        if result.returncode:
            raise RuntimeError(
                f"Splinterm snapshot failed: {(result.stderr or result.stdout).strip()}"
            )
        try:
            value = json.loads(result.stdout)
            data = value["data"]
            panes.append(
                {
                    "name": name,
                    "runtime_id": splint_id,
                    "columns": int(data["columns"]),
                    "rows": len(data["rows"]),
                }
            )
        except (KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
            raise RuntimeError("Splinterm snapshot geometry is malformed") from error
    return panes


def external_geometry(
    controller: TmuxController | ZellijController,
) -> list[dict[str, Any]]:
    inspection = controller.inspect()
    by_runtime: dict[str, dict[str, Any]] = {}
    for item in inspection["terminal_panes"]:
        if isinstance(controller, TmuxController):
            runtime_id = str(item["runtime_id"])
        else:
            runtime_id = f"terminal_{int(item['id'])}"
        by_runtime[runtime_id] = item
    panes = []
    for name in controller.topology.pane_names:
        runtime_id = controller.runtime_ids[name]
        if runtime_id not in by_runtime:
            raise RuntimeError(f"missing runtime pane geometry for {name}")
        item = by_runtime[runtime_id]
        panes.append(
            {
                "name": name,
                "runtime_id": runtime_id,
                "x": int(item["x"]),
                "y": int(item["y"]),
                "columns": int(item["columns"]),
                "rows": int(item["rows"]),
            }
        )
    return panes


def geometry(controller: HeadlessController) -> list[dict[str, Any]]:
    if isinstance(controller, SplintermController):
        return splinterm_geometry(controller)
    if isinstance(controller, (TmuxController, ZellijController)):
        return external_geometry(controller)
    raise TypeError("unsupported graphical controller")


def validate_two_columns(panes: Sequence[Mapping[str, Any]]) -> None:
    if [item["name"] for item in panes] != ["pane-0", "pane-1"]:
        raise RuntimeError("two-column geometry has unexpected pane identities")
    if any(int(item["columns"]) <= 0 or int(item["rows"]) <= 0 for item in panes):
        raise RuntimeError("two-column geometry contains an empty pane")
    positioned = [item for item in panes if "x" in item]
    if positioned and not (
        int(positioned[0]["x"]) < int(positioned[1]["x"])
        and int(positioned[0]["y"]) == int(positioned[1]["y"])
    ):
        raise RuntimeError("external multiplexer did not expose two columns")
    if not positioned and (
        abs(int(panes[0]["columns"]) - int(panes[1]["columns"])) > 2
        or int(panes[0]["rows"]) != int(panes[1]["rows"])
    ):
        raise RuntimeError("native pane dimensions do not describe equal columns")


def wait_stable_geometry(
    controller: HeadlessController,
    expected_user_state: Mapping[str, Any],
    timeout: float = 6.0,
    stable_seconds: float = 0.3,
) -> list[dict[str, Any]]:
    deadline = time.monotonic() + timeout
    last: list[dict[str, Any]] | None = None
    last_error: RuntimeError | None = None
    stable_since = time.monotonic()
    while time.monotonic() < deadline:
        assert_user_state(expected_user_state)
        current = geometry(controller)
        try:
            validate_two_columns(current)
        except RuntimeError as error:
            last = None
            last_error = error
            stable_since = time.monotonic()
            time.sleep(0.05)
            continue
        if current != last:
            last = current
            stable_since = time.monotonic()
        elif time.monotonic() - stable_since >= stable_seconds:
            return current
        time.sleep(0.05)
    detail = f": {last_error}" if last_error is not None else ""
    raise TimeoutError(f"pane geometry did not settle{detail}")


def stack_process_roles(
    controller: HeadlessController,
    server: ProcessIdentity,
    readiness: Mapping[str, Mapping[str, int | str]],
    window: ProcessIdentity,
) -> tuple[dict[str, Any], list[ProcessIdentity], list[ProcessIdentity]]:
    base = verify_process_roles(server, readiness)
    workload_pids = {int(item["pid"]) for item in readiness.values()}
    terminal_descendants = set(process_tree(pathlib.Path("/proc"), window.pid))
    client_pids = sorted(
        terminal_descendants - workload_pids - {window.pid, server.pid}
    )
    clients = [process_identity(pid) for pid in client_pids]
    if isinstance(controller, (TmuxController, ZellijController)) and not clients:
        raise RuntimeError("Foot stack has no attached multiplexer client process")
    helpers = [
        ProcessIdentity(int(item["pid"]), int(item["start_ticks"]))
        for item in base["roles"][1]["processes"]
    ]
    identities = [server, window, *clients, *helpers]
    identities.extend(process_identity(pid) for pid in sorted(workload_pids))
    pairs = {(item.pid, item.start_ticks) for item in identities}
    if len(pairs) != len(identities):
        raise RuntimeError("graphical stack process roles overlap")
    roles = [
        base["roles"][0],
        {"role": "terminal-host", "processes": [window.as_dict()]},
        {
            "role": "multiplexer-client",
            "processes": [item.as_dict() for item in clients],
        },
        base["roles"][1],
        base["roles"][2],
    ]
    return {"role_sets_disjoint": True, "roles": roles}, clients, helpers


def screenshot(window: Mapping[str, Any], output: pathlib.Path) -> dict[str, Any]:
    if shutil.which("grim") is None:
        raise RuntimeError("grim is unavailable")
    x, y = (int(value) for value in window["at"])
    width, height = (int(value) for value in window["size"])
    result = subprocess.run(
        ["grim", "-g", f"{x},{y} {width}x{height}", str(output)],
        text=True,
        capture_output=True,
        check=False,
        timeout=10,
    )
    if result.returncode or not output.is_file():
        raise RuntimeError(result.stderr.strip() or "window screenshot failed")
    digest = hashlib.sha256(output.read_bytes()).hexdigest()
    return {
        "path": output.name,
        "sha256": digest,
        "width": width,
        "height": height,
    }


def wait_namespace_absent(controller: HeadlessController, timeout: float) -> bool:
    deadline = time.monotonic() + timeout
    while not controller.namespace_absent():
        if time.monotonic() >= deadline:
            return False
        time.sleep(0.02)
    return True


def copy_diagnostics(state: pathlib.Path, output: pathlib.Path) -> None:
    diagnostics = output / "diagnostics"
    for source in state.rglob("*"):
        if not source.is_file() or source.name.endswith("ready.json"):
            continue
        relative = source.relative_to(state)
        destination = diagnostics / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)


def run_case(args: argparse.Namespace) -> dict[str, Any]:
    output = args.output.resolve()
    if output.exists() and any(output.iterdir()):
        raise RuntimeError(f"output directory is not empty: {output}")
    output.mkdir(parents=True, exist_ok=True)
    state = pathlib.Path(tempfile.mkdtemp(prefix="sb-graphical-mux-"))
    controller_output = state / "controller"
    controller_output.mkdir()
    topology: Topology = topology_named("two-columns")
    implementation = IMPLEMENTATIONS[args.stack]
    run_id = args.run_id or f"gm-{implementation}-{time.time_ns() % 10**10}"
    controller: HeadlessController | None = None
    address: str | None = None
    existing_addresses: set[str] = set()
    observed_addresses: set[str] = set()
    original_user_state: dict[str, Any] | None = None
    server: ProcessIdentity | None = None
    workload_identities: list[ProcessIdentity] = []
    window_identity: ProcessIdentity | None = None
    client_identities: list[ProcessIdentity] = []
    helper_identities: list[ProcessIdentity] = []
    ambient_before = ambient_counts(implementation)
    ambient_after: dict[str, int | None] | None = None
    cleanup_error: str | None = None
    failure: str | None = None
    runtime_ids: dict[str, str] = {}
    readiness: dict[str, dict[str, int | str]] = {}
    pre_geometry: list[dict[str, Any]] | None = None
    post_geometry: list[dict[str, Any]] | None = None
    roles: dict[str, Any] | None = None
    capture: dict[str, Any] | None = None
    cleanup = {
        "window_absent": False,
        "namespace_absent": False,
        "server_absent": False,
        "clients_absent": False,
        "workloads_absent": False,
        "process_forest_absent": False,
        "ambient_counts_unchanged": False,
        "verified": False,
    }

    try:
        V1.assert_test_workspace_isolated()
        original_user_state = user_state()
        assert_user_state(original_user_state)
        controller = controller_for(implementation, topology, controller_output, run_id)
        runtime_ids = controller.start(
            pane_commands(topology, controller_output, args.idle_seconds)
        )
        readiness = wait_for_ready(topology, controller_output, args.ready_timeout)
        workload_identities = [
            process_identity(int(readiness[name]["pid"]))
            for name in topology.pane_names
        ]
        server = controller.server_identity
        verify_process_roles(server, readiness)
        pre_geometry = geometry(controller)
        validate_two_columns(pre_geometry)

        command, environment = launch_spec(args.stack, controller)
        owned_token = owned_window_token(controller)
        launcher = state / "launch.sh"
        COMMON.write_launcher(launcher, command, environment)
        existing_addresses = {str(item["address"]) for item in V1.all_clients()}
        COMMON.dispatch_launcher(launcher)
        window = wait_window(
            APP_IDS[args.stack],
            existing_addresses,
            observed_addresses,
            owned_token,
            original_user_state,
            args.ready_timeout,
        )
        address = str(window["address"])
        window_identity = process_identity(int(window["pid"]))
        post_geometry = wait_stable_geometry(controller, original_user_state)
        COMMON.assert_owned_window(APP_IDS[args.stack], address)
        assert_user_state(original_user_state)
        roles, client_identities, helper_identities = stack_process_roles(
            controller, server, readiness, window_identity
        )
        time.sleep(args.settle_seconds)
        COMMON.assert_owned_window(APP_IDS[args.stack], address)
        assert_user_state(original_user_state)
        if not all(
            same_process(identity)
            for identity in [
                server,
                window_identity,
                *client_identities,
                *helper_identities,
                *workload_identities,
            ]
        ):
            raise RuntimeError("a stack process changed incarnation before capture")
        capture = screenshot(window, output / f"{args.stack}.png")
        assert_user_state(original_user_state)
    except EXPECTED_ERRORS as error:
        failure = f"{type(error).__name__}: {error}"
    finally:
        if address is not None:
            observed_addresses.add(address)
        try:
            observed_addresses.update(
                str(item["address"])
                for item in V1.all_clients()
                if item.get("class") == APP_IDS[args.stack]
                and str(item.get("address")) not in existing_addresses
                and controller is not None
                and process_has_cmdline_token(
                    int(item["pid"]),
                    owned_window_token(controller),
                )
            )
        except EXPECTED_ERRORS:
            pass
        for observed_address in sorted(observed_addresses):
            V1.kill_oracle_window(observed_address)
        try:
            COMMON.wait_cleanup()
            cleanup["window_absent"] = True
        except EXPECTED_ERRORS as error:
            cleanup_error = f"window cleanup: {type(error).__name__}: {error}"
        if controller is not None:
            try:
                controller.cleanup()
            except EXPECTED_ERRORS as error:
                detail = f"namespace cleanup: {type(error).__name__}: {error}"
                cleanup_error = (
                    f"{cleanup_error}; {detail}" if cleanup_error else detail
                )
            cleanup["namespace_absent"] = wait_namespace_absent(controller, 5.0)
        captured_forest = [
            *([server] if server is not None else []),
            *([window_identity] if window_identity is not None else []),
            *client_identities,
            *helper_identities,
            *workload_identities,
        ]
        if not wait_processes_gone(captured_forest, 1.0):
            terminate_processes_exact(captured_forest)
        cleanup["server_absent"] = server is None or not same_process(server)
        cleanup["clients_absent"] = wait_processes_gone(client_identities, 5.0)
        cleanup["workloads_absent"] = wait_processes_gone(workload_identities, 5.0)
        cleanup["process_forest_absent"] = wait_processes_gone(captured_forest, 5.0)
        ambient_after = ambient_counts(implementation)
        cleanup["ambient_counts_unchanged"] = ambient_before == ambient_after
        try:
            if original_user_state is not None:
                assert_user_state(original_user_state)
        except EXPECTED_ERRORS as error:
            detail = f"host state cleanup: {type(error).__name__}: {error}"
            cleanup_error = f"{cleanup_error}; {detail}" if cleanup_error else detail
        cleanup["verified"] = (
            cleanup_error is None
            and cleanup["window_absent"]
            and cleanup["namespace_absent"]
            and cleanup["server_absent"]
            and cleanup["clients_absent"]
            and cleanup["workloads_absent"]
            and cleanup["process_forest_absent"]
            and cleanup["ambient_counts_unchanged"]
        )

    valid = failure is None and cleanup["verified"]
    report = {
        "schema": "splinterm.benchmark.multiplexer-graphical-smoke.v1",
        "stack": args.stack,
        "implementation": implementation,
        "topology": {
            "name": topology.name,
            "pane_count": len(topology.pane_names),
            "panes": list(topology.pane_names),
        },
        "runtime_ids": runtime_ids,
        "readiness": readiness,
        "geometry": {
            "before_graphical_attach": pre_geometry,
            "after_graphical_attach": post_geometry,
            "stable_seconds": 0.3,
        },
        "process_roles": roles,
        "capture": capture,
        "isolation": {
            "workspace": 8,
            "monitor": "DP-2",
            "no_initial_focus": True,
            "host_state_before": original_user_state,
            "host_state_preserved": original_user_state is not None
            and cleanup_error is None,
            "run_id": run_id,
            "ambient_before": ambient_before,
            "ambient_after": ambient_after,
            "ambient_names_recorded": False,
        },
        "cleanup": {**cleanup, "failure": cleanup_error},
        "failure": failure,
        "valid": valid,
        "notes": [
            "Guarded development smoke only; readiness values are not a performance ranking.",
            "The terminal was mapped silently on inactive workspace 8 / DP-2.",
        ],
    }
    temporary = output / ".report.json.tmp"
    temporary.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    temporary.replace(output / "report.json")
    if not valid:
        copy_diagnostics(state, output)
    shutil.rmtree(state, ignore_errors=True)
    return report


def parser() -> argparse.ArgumentParser:
    value = argparse.ArgumentParser(
        description="Run one guarded two-column multiplexer graphical smoke"
    )
    value.add_argument("output", type=pathlib.Path)
    value.add_argument("--stack", choices=STACKS, required=True)
    value.add_argument("--run-id")
    value.add_argument("--idle-seconds", type=float, default=45.0)
    value.add_argument("--ready-timeout", type=float, default=10.0)
    value.add_argument("--settle-seconds", type=float, default=0.5)
    return value


def main() -> int:
    args = parser().parse_args()
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        print("a running Hyprland session is required", file=sys.stderr)
        return 2
    if not 10 <= args.idle_seconds <= 300:
        print("--idle-seconds must be between 10 and 300", file=sys.stderr)
        return 2
    if not 1 <= args.ready_timeout <= 60:
        print("--ready-timeout must be between 1 and 60", file=sys.stderr)
        return 2
    if not 0 <= args.settle_seconds <= 10:
        print("--settle-seconds must be between 0 and 10", file=sys.stderr)
        return 2
    try:
        report = run_case(args)
    except EXPECTED_ERRORS as error:
        print(f"graphical multiplexer setup failed: {error}", file=sys.stderr)
        return 1
    print(f"Graphical multiplexer report: {args.output.resolve() / 'report.json'}")
    print(f"Result: {'PASS' if report['valid'] else 'FAIL'}")
    return 0 if report["valid"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
