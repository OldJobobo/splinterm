#!/usr/bin/env python3
# pyright: reportMissingImports=false
"""Run a guarded pairwise static-image matrix through native and tmux stacks."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import pathlib
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from collections.abc import Mapping, Sequence
from typing import Any, cast

ROOT = pathlib.Path(__file__).resolve().parents[2]
IMAGE_SPIKE = pathlib.Path(__file__).resolve().parent
TOOLS = ROOT / "tools/benchmark"
SYNTHETIC = IMAGE_SPIKE / "fixtures/real-client-static.png"
UI_DETAIL = ROOT / "docs/plans/artifacts/0025-action-menus/palette-filter-split.png"
DEFAULT_PHOTO = (
    pathlib.Path.home()
    / "Pictures/wallpaper/Mountain Dew/lake-lucerne-landscape-mountains-sunset-switzerland-3840x2160-49.jpg"
)
DEFAULT_ALPHA = (
    pathlib.Path.home()
    / "Pictures/vecteezy_white-smooth-clouds-realistic-meteorology-isolate_47309405.png"
)
FOOT_PROFILE = TOOLS / "profiles/foot.ini"
KITTY_PROFILE = TOOLS / "profiles/kitty.conf"
TMUX_PROFILE = TOOLS / "profiles/tmux.conf"
SCHEMA = "splinterm.tmux-image-matrix.v1"
WORKSPACE = 8
SURFACE = (960, 600)
STACKS = (
    "splinterm-sixel",
    "splinterm-kitty",
    "foot-tmux-sixel",
    "kitty-tmux-kitty",
)
APP_IDS = {
    "splinterm-sixel": "com.oldjobobo.splinterm",
    "splinterm-kitty": "com.oldjobobo.splinterm",
    "foot-tmux-sixel": "com.oldjobobo.splinterbench.FootTmuxImages",
    "kitty-tmux-kitty": "com.oldjobobo.splinterbench.KittyTmuxImages",
}
PROTOCOLS = {
    "splinterm-sixel": "sixel",
    "splinterm-kitty": "kitty",
    "foot-tmux-sixel": "sixel",
    "kitty-tmux-kitty": "kitty",
}
FIXTURE_KINDS = ("synthetic", "ui-detail", "alpha", "photo")

sys.path.insert(0, str(TOOLS))
from graphical_multiplexer import resource_delta  # type: ignore[reportMissingImports]
from headless_multiplexer import (  # type: ignore[reportMissingImports]
    HeadlessController,
    ProcessIdentity,
    SplintermController,
    TmuxController,
    controller_for,
    process_identity,
    same_process,
    terminate_processes_exact,
    wait_for_ready,
    wait_processes_gone,
)
from multiplexers.tmux import TmuxAdapter  # type: ignore[reportMissingImports]
from multiplexing import topology_named  # type: ignore[reportMissingImports]


def load(path: pathlib.Path, name: str):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


MEASURED = load(TOOLS / "run-graphical-multiplexer.py", "image_stack_measured")
SMOKE = MEASURED.SMOKE
COMMON = SMOKE.COMMON
V1 = COMMON.V1


class MatrixError(RuntimeError):
    """A bounded benchmark setup, execution, or validation failure."""


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


def app_version(name: str) -> dict[str, str]:
    executable = shutil.which(name)
    if executable is None:
        raise MatrixError(f"required application is unavailable: {name}")
    arguments = {
        "chafa": ["--version"],
        "foot": ["--version"],
        "kitty": ["--version"],
        "tmux": ["-V"],
    }[name]
    result = subprocess.run(
        [executable, *arguments],
        text=True,
        capture_output=True,
        timeout=5,
        check=False,
    )
    output = (result.stdout or result.stderr).strip().splitlines()
    return {
        "path": str(pathlib.Path(executable).resolve()),
        "sha256": sha256(pathlib.Path(executable).resolve()),
        "version": output[0] if output else f"exit {result.returncode}",
    }


def image_metadata(path: pathlib.Path) -> dict[str, Any]:
    try:
        from PIL import Image
    except ImportError as error:
        raise MatrixError("Pillow is required for image metadata") from error
    with Image.open(path) as image:
        image.load()
        alpha = None
        if "A" in image.getbands() or "transparency" in image.info:
            alpha_channel = image.convert("RGBA").getchannel("A")
            minimum, maximum = cast(tuple[int, int], alpha_channel.getextrema())
            histogram = alpha_channel.histogram()
            alpha = {
                "minimum": minimum,
                "maximum": maximum,
                "transparent_pixels": int(sum(histogram[:255])),
                "partial_alpha_pixels": int(sum(histogram[1:255])),
            }
        return {
            "width": image.width,
            "height": image.height,
            "format": image.format,
            "mode": image.mode,
            "alpha": alpha,
        }


def source_inputs(photo: pathlib.Path, alpha: pathlib.Path) -> dict[str, pathlib.Path]:
    return {
        "synthetic": SYNTHETIC,
        "ui-detail": UI_DETAIL,
        "alpha": alpha.expanduser().resolve(),
        "photo": photo.expanduser().resolve(),
    }


def validate_inputs(inputs: Mapping[str, pathlib.Path]) -> None:
    for name, path in inputs.items():
        if not path.is_file():
            raise MatrixError(f"{name} fixture is unavailable: {path}")
    alpha = image_metadata(inputs["alpha"])["alpha"]
    if (
        alpha is None
        or alpha["minimum"] != 0
        or alpha["maximum"] != 255
        or alpha["partial_alpha_pixels"] <= 0
    ):
        raise MatrixError(
            "alpha fixture must contain transparent, partial, and opaque pixels"
        )


def prepare_inputs(
    output: pathlib.Path, inputs: Mapping[str, pathlib.Path]
) -> tuple[dict[str, pathlib.Path], dict[str, dict[str, Any]]]:
    directory = output / "inputs"
    directory.mkdir(parents=True)
    prepared: dict[str, pathlib.Path] = {}
    records: dict[str, dict[str, Any]] = {}
    for name, source in inputs.items():
        suffix = source.suffix.lower() or ".image"
        destination = directory / f"{name}{suffix}"
        shutil.copy2(source, destination)
        if sha256(source) != sha256(destination):
            raise MatrixError(f"copied fixture hash mismatch: {name}")
        prepared[name] = destination
        records[name] = {
            "source": str(source),
            "source_sha256": sha256(source),
            "evidence_path": str(destination.relative_to(output)),
            "evidence_sha256": sha256(destination),
            "size_bytes": destination.stat().st_size,
            **image_metadata(destination),
        }
    return prepared, records


def chafa_command(stack: str, image: pathlib.Path) -> list[str]:
    executable = shutil.which("chafa")
    if executable is None:
        raise MatrixError("Chafa is unavailable")
    protocol = "sixels" if PROTOCOLS[stack] == "sixel" else "kitty"
    command = [
        executable,
        "--probe",
        "off",
        "--format",
        protocol,
        "--animate",
        "off",
        "--view-size",
        "80x24",
        "--size",
        "40x20",
    ]
    if "tmux" in stack:
        command.extend(["--passthrough", "tmux"])
    return [*command, str(image)]


def build_plan(samples: int = 3) -> list[dict[str, Any]]:
    plan: list[dict[str, Any]] = []
    compatibility_order = (
        "foot-tmux-sixel",
        "kitty-tmux-kitty",
        "splinterm-sixel",
        "splinterm-kitty",
    )
    for fixture in ("synthetic", "ui-detail", "alpha"):
        for stack in compatibility_order:
            plan.append(
                {
                    "stack": stack,
                    "fixture": fixture,
                    "kind": "compatibility",
                    "sample": 1,
                    "warmup": False,
                }
            )
    for stack in STACKS:
        plan.append(
            {
                "stack": stack,
                "fixture": "photo",
                "kind": "benchmark",
                "sample": 0,
                "warmup": True,
            }
        )
    for sample in range(1, samples + 1):
        offset = (sample - 1) % len(STACKS)
        order = (*STACKS[offset:], *STACKS[:offset])
        for stack in order:
            plan.append(
                {
                    "stack": stack,
                    "fixture": "photo",
                    "kind": "benchmark",
                    "sample": sample,
                    "warmup": False,
                }
            )
    return plan


def selected_plan(mode: str, samples: int) -> list[dict[str, Any]]:
    plan = build_plan(samples)
    if mode == "all":
        return plan
    if mode == "smoke":
        return [plan[0]]
    if mode == "compatibility":
        return [item for item in plan if item["kind"] == "compatibility"]
    if mode == "benchmark":
        return [item for item in plan if item["kind"] == "benchmark"]
    if mode in STACKS:
        return [item for item in plan if item["stack"] == mode]
    raise ValueError(f"unknown mode: {mode}")


def child_command(
    ready: pathlib.Path,
    trigger: pathlib.Path,
    status: pathlib.Path,
    command: Sequence[str],
) -> list[str]:
    code = (
        "import json,os,pathlib,subprocess,time\n"
        f"ready=pathlib.Path({str(ready)!r})\n"
        f"trigger=pathlib.Path({str(trigger)!r})\n"
        f"status=pathlib.Path({str(status)!r})\n"
        f"command={list(command)!r}\n"
        "def atomic(path,value):\n"
        " p=path.with_name('.'+path.name+'.tmp'); p.write_text(json.dumps(value)+'\\n'); p.replace(path)\n"
        "atomic(ready,{'schema':'splinterm.benchmark.child.v1','event':'ready','monotonic_ns':time.monotonic_ns(),'pid':os.getpid()})\n"
        "deadline=time.monotonic()+30\n"
        "while not trigger.exists():\n"
        "  if time.monotonic()>=deadline: raise SystemExit('trigger timeout')\n"
        "  time.sleep(0.01)\n"
        "os.write(1,b'\\x1b[2J\\x1b[H\\x1b[?25l')\n"
        "started=time.monotonic_ns()\n"
        "process=subprocess.Popen(command)\n"
        "atomic(status,{'state':'running','pid':process.pid,'started_ns':started})\n"
        "returncode=process.wait()\n"
        "atomic(status,{'state':'exited','pid':process.pid,'returncode':returncode,'runtime_ns':time.monotonic_ns()-started})\n"
        "time.sleep(30)\n"
    )
    return [sys.executable, "-c", code]


def implementation_for(stack: str) -> str:
    return "tmux" if "tmux" in stack else "splinterm"


def configure_tmux(controller: TmuxController) -> None:
    result = subprocess.run(
        [
            *controller.plan.command_prefix,
            "set-option",
            "-g",
            "allow-passthrough",
            "all",
        ],
        env=controller.environment,
        text=True,
        capture_output=True,
        timeout=5,
        check=False,
    )
    if result.returncode:
        raise MatrixError(result.stderr.strip() or "tmux passthrough setup failed")


def launch_spec(
    stack: str, controller: HeadlessController
) -> tuple[list[str], dict[str, str]]:
    if isinstance(controller, SplintermController):
        return SMOKE.launch_spec("splinterm-native", controller)
    if not isinstance(controller, TmuxController):
        raise TypeError("image stack requires Splinterm or tmux controller")
    attach = [
        *controller.plan.command_prefix,
        "attach-session",
        "-t",
        controller.plan.session_name,
    ]
    if stack == "foot-tmux-sixel":
        foot = shutil.which("foot")
        if foot is None:
            raise MatrixError("Foot is unavailable")
        return (
            [
                foot,
                "-c",
                str(FOOT_PROFILE),
                "-a",
                APP_IDS[stack],
                "-T",
                "splinterbench-foot-tmux-images",
                "-w",
                "960x600",
                "--override=tweak.sixel=yes",
                *attach,
            ],
            dict(controller.environment),
        )
    if stack == "kitty-tmux-kitty":
        kitty = shutil.which("kitty")
        if kitty is None:
            raise MatrixError("Kitty is unavailable")
        return (
            [
                kitty,
                "--config",
                str(KITTY_PROFILE),
                "--class",
                APP_IDS[stack],
                "--title",
                "splinterbench-kitty-tmux-images",
                *attach,
            ],
            dict(controller.environment),
        )
    raise ValueError(f"unsupported tmux image stack: {stack}")


def single_geometry(controller: HeadlessController) -> dict[str, Any]:
    panes = SMOKE.geometry(controller)
    if len(panes) != 1 or panes[0]["name"] != "pane-0":
        raise MatrixError("image stack does not expose exactly one pane")
    if int(panes[0]["columns"]) <= 0 or int(panes[0]["rows"]) <= 0:
        raise MatrixError("image pane geometry is empty")
    return panes[0]


def wait_stable_geometry(
    controller: HeadlessController,
    user_state: Mapping[str, Any],
    timeout: float = 6.0,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    previous: dict[str, Any] | None = None
    stable_since = time.monotonic()
    while time.monotonic() < deadline:
        SMOKE.assert_user_state(user_state)
        current = single_geometry(controller)
        if current != previous:
            previous = current
            stable_since = time.monotonic()
        elif time.monotonic() - stable_since >= 0.3:
            return current
        time.sleep(0.03)
    raise TimeoutError("single-pane geometry did not settle")


def capture_bytes(window: Mapping[str, Any]) -> bytes:
    x, y = (int(value) for value in window["at"])
    width, height = (int(value) for value in window["size"])
    result = subprocess.run(
        ["grim", "-g", f"{x},{y} {width}x{height}", "-"],
        capture_output=True,
        timeout=10,
        check=False,
    )
    if result.returncode or not result.stdout.startswith(b"\x89PNG\r\n\x1a\n"):
        raise MatrixError(
            result.stderr.decode(errors="replace").strip() or "grim capture failed"
        )
    return result.stdout


def pixel_digest(payload: bytes) -> str:
    try:
        from PIL import Image
    except ImportError as error:
        raise MatrixError("Pillow is required for screenshot hashing") from error
    import io

    with Image.open(io.BytesIO(payload)) as image:
        return hashlib.sha256(image.convert("RGB").tobytes()).hexdigest()


def changed_image_summary(before: bytes, after: bytes, fixture: str) -> dict[str, Any]:
    try:
        from PIL import Image, ImageChops
    except ImportError as error:
        raise MatrixError("Pillow is required for screenshot validation") from error
    import io

    first = Image.open(io.BytesIO(before)).convert("RGB")
    second = Image.open(io.BytesIO(after)).convert("RGB")
    if first.size != second.size:
        raise MatrixError("screenshot dimensions changed")
    difference = ImageChops.difference(first, second)
    bbox = difference.getbbox()
    if bbox is None:
        raise MatrixError("image display did not change the terminal screenshot")
    histogram = difference.convert("L").histogram()
    changed = sum(histogram[1:])
    if changed < 1000:
        raise MatrixError(f"too few changed image pixels: {changed}")
    crop = second.crop(bbox)
    colors = crop.getcolors(maxcolors=1_000_000)
    distinct = len(colors) if colors is not None else 1_000_001
    if distinct < 32:
        raise MatrixError(
            f"rendered image has too little color/detail variation: {distinct}"
        )
    pixels = cast(
        Sequence[tuple[int, int, int]],
        list(  # type: ignore[arg-type]
            crop.resize(
                (min(160, crop.width), min(120, crop.height))
            ).get_flattened_data()
        ),
    )
    red = sum(1 for r, g, b in pixels if r > 150 and r > g * 3 // 2 and r > b * 3 // 2)
    green = sum(
        1 for r, g, b in pixels if g > 130 and g > r * 3 // 2 and g > b * 3 // 2
    )
    blue = sum(1 for r, g, b in pixels if b > 130 and b > r * 3 // 2 and b > g * 3 // 2)
    if fixture == "synthetic" and min(red, green, blue) < 10:
        raise MatrixError("synthetic render lacks expected RGB regions")
    return {
        "changed_pixels": changed,
        "bbox": list(bbox),
        "distinct_colors": distinct,
        "sampled_primary_pixels": {"red": red, "green": green, "blue": blue},
    }


def wait_for_stable_capture(
    window_address: str,
    app_id: str,
    owned_token: str,
    expected_user_state: Mapping[str, Any],
    baseline: bytes,
    status: pathlib.Path,
    output: pathlib.Path,
    timeout: float = 15.0,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    baseline_hash = pixel_digest(baseline)
    first_ns: int | None = None
    first_bytes: bytes | None = None
    previous_hash: str | None = None
    stable_count = 0
    stable_bytes: bytes | None = None
    while time.monotonic() < deadline:
        SMOKE.assert_user_state(expected_user_state)
        COMMON.assert_owned_window(app_id, window_address)
        window = MEASURED.window_by_address(window_address)
        if window is None:
            raise MatrixError("image stack window closed before stable capture")
        if (
            window.get("workspace", {}).get("id") != WORKSPACE
            or window.get("monitor") != V1.test_monitor_id()
            or window.get("class") != app_id
            or not SMOKE.process_has_cmdline_token(int(window["pid"]), owned_token)
        ):
            raise MatrixError("image stack window identity or placement changed")
        payload = capture_bytes(window)
        digest = pixel_digest(payload)
        if digest != baseline_hash:
            if first_ns is None:
                first_ns = time.monotonic_ns()
                first_bytes = payload
            status_exited = False
            if status.exists():
                try:
                    status_exited = (
                        json.loads(status.read_text(encoding="utf-8")).get("state")
                        == "exited"
                    )
                except (OSError, json.JSONDecodeError):
                    pass
            if status_exited and digest == previous_hash:
                stable_count += 1
            else:
                stable_count = 0
            previous_hash = digest
            stable_bytes = payload
            if stable_count >= 2:
                break
        time.sleep(0.03)
    if (
        first_ns is None
        or first_bytes is None
        or stable_bytes is None
        or stable_count < 2
    ):
        raise TimeoutError("image screenshot did not reach a stable changed frame")
    (output / "first-change.png").write_bytes(first_bytes)
    (output / "stable.png").write_bytes(stable_bytes)
    return {
        "first_change_ns": first_ns,
        "stable_ns": time.monotonic_ns(),
        "first_change_sha256": hashlib.sha256(first_bytes).hexdigest(),
        "first_change_pixel_sha256": pixel_digest(first_bytes),
        "stable_sha256": hashlib.sha256(stable_bytes).hexdigest(),
        "stable_pixel_sha256": pixel_digest(stable_bytes),
        "stable_bytes": stable_bytes,
    }


def ambient_tmux() -> dict[str, int | None]:
    identity = TmuxAdapter().probe(ROOT)
    return {
        "process_count": identity.ambient_process_count,
        "default_session_count": identity.default_session_count,
    }


def run_case(
    output_root: pathlib.Path,
    item: Mapping[str, Any],
    image: pathlib.Path,
    idle_seconds: float,
) -> dict[str, Any]:
    stack = str(item["stack"])
    fixture = str(item["fixture"])
    if item["warmup"]:
        run_name = f"{stack}-{fixture}-warmup"
    elif item["kind"] == "benchmark":
        run_name = f"{stack}-{fixture}-sample-{int(item['sample']):02d}"
    else:
        run_name = f"{stack}-{fixture}"
    output = output_root / run_name
    if output.exists():
        raise MatrixError(f"refusing to overwrite evidence: {output}")
    output.mkdir(parents=True)
    state = pathlib.Path(tempfile.mkdtemp(prefix="sb-tmux-image-"))
    controller_output = state / "controller"
    controller_output.mkdir()
    ready = controller_output / "pane-0-ready.json"
    trigger = state / "trigger"
    status = output / "app-status.json"
    topology = topology_named("single")
    command = chafa_command(stack, image)
    run_id = re.sub(
        r"[^a-z0-9-]", "-", f"img-{run_name}-{time.time_ns() % 10**8}".lower()
    )
    controller: HeadlessController | None = None
    server: ProcessIdentity | None = None
    window_identity: ProcessIdentity | None = None
    infrastructure: list[ProcessIdentity] = []
    workloads: list[ProcessIdentity] = []
    observed: set[str] = set()
    existing: set[str] = set()
    address: str | None = None
    user_before: dict[str, Any] | None = None
    ambient_before = ambient_tmux()
    ambient_after: dict[str, int | None] | None = None
    resources: dict[str, Any] | None = None
    process_roles: dict[str, Any] | None = None
    geometry: dict[str, Any] | None = None
    capture: dict[str, Any] | None = None
    failure: str | None = None
    cleanup_error: str | None = None
    cleanup = {
        "window_absent": False,
        "namespace_absent": False,
        "server_absent": False,
        "workloads_absent": False,
        "process_forest_absent": False,
        "ambient_counts_unchanged": False,
        "verified": False,
    }
    try:
        V1.assert_test_workspace_isolated()
        current_user_state: dict[str, Any] = SMOKE.user_state()
        user_before = current_user_state
        SMOKE.assert_user_state(current_user_state)
        active_controller: HeadlessController = controller_for(
            implementation_for(stack), topology, controller_output, run_id
        )
        controller = active_controller
        active_controller.start(
            {"pane-0": child_command(ready, trigger, status, command)}
        )
        if isinstance(active_controller, TmuxController):
            configure_tmux(active_controller)
        readiness = wait_for_ready(topology, controller_output, 10)
        server = active_controller.server_identity
        launch, environment = launch_spec(stack, active_controller)
        launcher = state / "launch.sh"
        COMMON.write_launcher(launcher, launch, environment)
        existing = {str(item["address"]) for item in V1.all_clients()}
        COMMON.dispatch_launcher(launcher)
        owned_token = SMOKE.owned_window_token(active_controller)
        window = SMOKE.wait_window(
            APP_IDS[stack], existing, observed, owned_token, user_before, 10
        )
        address = str(window["address"])
        window_identity = process_identity(int(window["pid"]))
        geometry = wait_stable_geometry(active_controller, current_user_state)
        process_roles, infrastructure, workloads = MEASURED.process_document(
            active_controller, server, readiness, {"pane-0": window}
        )
        baseline_resources = MEASURED.resource_pair(infrastructure, workloads)
        time.sleep(0.2)
        SMOKE.assert_user_state(user_before)
        COMMON.assert_owned_window(APP_IDS[stack], address)
        baseline = capture_bytes(window)
        (output / "baseline.png").write_bytes(baseline)
        triggered_ns = time.monotonic_ns()
        trigger.touch()
        captured = wait_for_stable_capture(
            address,
            APP_IDS[stack],
            owned_token,
            current_user_state,
            baseline,
            status,
            output,
        )
        stable_payload = captured.pop("stable_bytes")
        validation = changed_image_summary(baseline, stable_payload, fixture)
        app_status = json.loads(status.read_text(encoding="utf-8"))
        if app_status.get("state") != "exited" or app_status.get("returncode") != 0:
            raise MatrixError(f"Chafa did not exit successfully: {app_status}")
        stable_resources = MEASURED.resource_pair(infrastructure, workloads)
        resources = resource_delta(baseline_resources, stable_resources)
        idle_before = MEASURED.resource_pair(infrastructure, workloads)
        deadline = time.monotonic() + idle_seconds
        while time.monotonic() < deadline:
            SMOKE.assert_user_state(user_before)
            time.sleep(0.05)
        idle_after = MEASURED.resource_pair(infrastructure, workloads)
        capture = {
            **captured,
            "trigger_to_first_change_ns": captured["first_change_ns"] - triggered_ns,
            "trigger_to_stable_ns": captured["stable_ns"] - triggered_ns,
            "validation": validation,
            "baseline_sha256": hashlib.sha256(baseline).hexdigest(),
            "application": app_status,
            "idle_seconds": idle_seconds,
            "idle_resources": resource_delta(idle_before, idle_after),
        }
    except Exception as error:  # noqa: BLE001 - every failure must persist evidence
        failure = f"{type(error).__name__}: {error}"
    finally:
        if address is not None:
            observed.add(address)
        if controller is not None:
            try:
                token = SMOKE.owned_window_token(controller)
                observed.update(
                    str(item["address"])
                    for item in V1.all_clients()
                    if item.get("class") == APP_IDS[stack]
                    and str(item.get("address")) not in existing
                    and SMOKE.process_has_cmdline_token(int(item["pid"]), token)
                )
            except Exception as error:  # noqa: BLE001 - continue cleanup
                cleanup_error = cleanup_error or f"window discovery: {error}"
        for observed_address in sorted(observed):
            try:
                V1.kill_oracle_window(observed_address)
            except Exception as error:  # noqa: BLE001 - continue cleanup
                cleanup_error = cleanup_error or f"window cleanup: {error}"
        try:
            COMMON.wait_cleanup()
            cleanup["window_absent"] = True
        except Exception as error:  # noqa: BLE001 - persist cleanup failure
            cleanup_error = cleanup_error or f"window cleanup wait: {error}"
        if controller is not None:
            try:
                controller.cleanup()
            except Exception as error:  # noqa: BLE001 - continue cleanup
                cleanup_error = cleanup_error or f"namespace cleanup: {error}"
            try:
                cleanup["namespace_absent"] = SMOKE.wait_namespace_absent(controller, 5)
            except Exception as error:  # noqa: BLE001 - continue cleanup
                cleanup_error = cleanup_error or f"namespace verification: {error}"
        forest = [
            *([server] if server is not None else []),
            *([window_identity] if window_identity is not None else []),
            *infrastructure,
            *workloads,
        ]
        deduped = list({(item.pid, item.start_ticks): item for item in forest}.values())
        if not wait_processes_gone(deduped, 1):
            terminate_processes_exact(deduped)
        cleanup["server_absent"] = server is None or not same_process(server)
        cleanup["workloads_absent"] = wait_processes_gone(workloads, 5)
        cleanup["process_forest_absent"] = wait_processes_gone(deduped, 5)
        ambient_after = ambient_tmux()
        cleanup["ambient_counts_unchanged"] = ambient_before == ambient_after
        try:
            if user_before is not None:
                SMOKE.assert_user_state(user_before)
        except Exception as error:  # noqa: BLE001 - persist host-state failure
            cleanup_error = cleanup_error or f"host-state cleanup: {error}"
        cleanup["verified"] = (
            cleanup_error is None
            and cleanup["window_absent"]
            and cleanup["namespace_absent"]
            and cleanup["server_absent"]
            and cleanup["workloads_absent"]
            and cleanup["process_forest_absent"]
            and cleanup["ambient_counts_unchanged"]
        )
    valid = failure is None and cleanup["verified"]
    if not valid:
        try:
            SMOKE.copy_diagnostics(state, output)
        except Exception as error:  # noqa: BLE001 - retain report after copy failure
            cleanup_error = cleanup_error or f"diagnostic copy: {error}"
            cleanup["verified"] = False
    report = {
        "schema": SCHEMA,
        "run": run_name,
        "stack": stack,
        "protocol": PROTOCOLS[stack],
        "fixture": fixture,
        "kind": item["kind"],
        "sample": item["sample"],
        "warmup": item["warmup"],
        "command": command,
        "geometry": geometry,
        "process_roles": process_roles,
        "resources": resources,
        "capture": capture,
        "isolation": {
            "workspace": WORKSPACE,
            "monitor": "DP-2",
            "no_initial_focus": True,
            "host_state_before": user_before,
            "host_state_preserved": user_before is not None and cleanup_error is None,
            "ambient_before": ambient_before,
            "ambient_after": ambient_after,
        },
        "cleanup": {**cleanup, "failure": cleanup_error},
        "failure": failure,
        "valid": valid,
    }
    atomic_json(output / "report.json", report)
    shutil.rmtree(state, ignore_errors=True)
    return report


def summarize_values(values: list[int]) -> dict[str, Any]:
    if not values:
        return {
            "samples": 0,
            "median": None,
            "minimum": None,
            "maximum": None,
            "spread": None,
        }
    median = statistics.median(values)
    return {
        "samples": len(values),
        "median": int(median),
        "minimum": min(values),
        "maximum": max(values),
        "spread": (max(values) - min(values)) / median if median else 0.0,
    }


def benchmark_summary(
    reports: Sequence[Mapping[str, Any]], stack: str
) -> dict[str, Any]:
    selected = [
        report
        for report in reports
        if report.get("valid")
        and report["stack"] == stack
        and report["kind"] == "benchmark"
        and not report["warmup"]
    ]
    return {
        "samples": len(selected),
        "trigger_to_first_change_ns": summarize_values(
            [
                int(report["capture"]["trigger_to_first_change_ns"])
                for report in selected
            ]
        ),
        "trigger_to_stable_ns": summarize_values(
            [int(report["capture"]["trigger_to_stable_ns"]) for report in selected]
        ),
        "application_runtime_ns": summarize_values(
            [int(report["capture"]["application"]["runtime_ns"]) for report in selected]
        ),
        "infrastructure_rss_bytes": summarize_values(
            [
                int(report["resources"]["after"]["infrastructure"]["rss_bytes"])
                for report in selected
            ]
        ),
        "total_rss_bytes": summarize_values(
            [
                int(report["resources"]["after"]["total"]["rss_bytes"])
                for report in selected
            ]
        ),
    }


def prepare_binaries(output: pathlib.Path) -> dict[str, pathlib.Path]:
    sources = {
        name: ROOT / "target/release" / name
        for name in ("splinterd", "splinterm", "splinterm-pty-child")
    }
    if not all(path.is_file() for path in sources.values()):
        raise MatrixError("release Splinterm suite is incomplete")
    directory = output / "binaries"
    directory.mkdir(parents=True)
    prepared: dict[str, pathlib.Path] = {}
    for name, source in sources.items():
        destination = directory / name
        shutil.copy2(source, destination)
        if sha256(source) != sha256(destination):
            raise MatrixError(f"copied release binary hash mismatch: {name}")
        prepared[name] = destination
    os.environ["SPLINTERBENCH_SPLINTERM_DAEMON"] = str(prepared["splinterd"])
    os.environ["SPLINTERBENCH_SPLINTERM_CLIENT"] = str(prepared["splinterm"])
    os.environ["SPLINTERM_PTY_HELPER"] = str(prepared["splinterm-pty-child"])
    return prepared


def write_manifest(
    output: pathlib.Path,
    inputs: Mapping[str, Mapping[str, Any]],
    binaries: Mapping[str, pathlib.Path],
    samples: int,
) -> None:
    atomic_json(
        output / "manifest.json",
        {
            "schema": SCHEMA,
            "surface": {"width": SURFACE[0], "height": SURFACE[1]},
            "terminal_geometry": {"columns": 80, "rows": 24},
            "image_geometry": {"columns": 40, "rows": 20},
            "samples": samples,
            "applications": {
                name: app_version(name) for name in ("chafa", "foot", "kitty", "tmux")
            },
            "splinterm_binaries": {
                name: {
                    "evidence_path": str(path.relative_to(output)),
                    "sha256": sha256(path),
                    "size_bytes": path.stat().st_size,
                }
                for name, path in binaries.items()
            },
            "profiles": {
                str(path.relative_to(ROOT)): sha256(path)
                for path in (FOOT_PROFILE, KITTY_PROFILE, TMUX_PROFILE)
            },
            "tmux_runtime_options": {"allow-passthrough": "all"},
            "inputs": inputs,
            "stacks": {
                stack: {
                    "protocol": PROTOCOLS[stack],
                    "tmux": "tmux" in stack,
                    "chafa_passthrough": "tmux" if "tmux" in stack else "none",
                }
                for stack in STACKS
            },
            "measurement_boundaries": {
                "trigger_to_first_change": "trigger file creation to first changed full-window grim screenshot",
                "trigger_to_stable": "trigger file creation to three identical changed full-window grim screenshots after application status exists",
                "application_runtime": "Chafa subprocess monotonic runtime",
                "presentation_feedback": "not measured",
            },
        },
    )


def print_catalogue() -> None:
    for stack in STACKS:
        print(
            f"{stack:24} {PROTOCOLS[stack]:6} {'tmux' if 'tmux' in stack else 'native'}"
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", nargs="?", type=pathlib.Path)
    selector = parser.add_mutually_exclusive_group()
    selector.add_argument("--list", action="store_true")
    selector.add_argument("--smoke", action="store_true")
    selector.add_argument("--compatibility", action="store_true")
    selector.add_argument("--benchmark", action="store_true")
    selector.add_argument("--all", action="store_true")
    selector.add_argument("--stack", choices=STACKS)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--samples", type=int, choices=range(1, 6), default=3)
    parser.add_argument("--photo", type=pathlib.Path, default=DEFAULT_PHOTO)
    parser.add_argument("--alpha", type=pathlib.Path, default=DEFAULT_ALPHA)
    parser.add_argument("--idle-seconds", type=float, default=1.0)
    parser.add_argument("--reuse-build", action="store_true")
    args = parser.parse_args()
    if args.list:
        print_catalogue()
        return 0
    mode = args.stack or (
        "smoke"
        if args.smoke
        else "compatibility"
        if args.compatibility
        else "benchmark"
        if args.benchmark
        else "all"
        if args.all
        else ""
    )
    if not mode:
        parser.error("select --smoke, --compatibility, --benchmark, --all, or --stack")
    plan = selected_plan(mode, args.samples)
    if args.dry_run:
        print(json.dumps({"schema": SCHEMA, "mode": mode, "plan": plan}, indent=2))
        return 0
    if args.output is None:
        parser.error("output is required for graphical execution")
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        parser.error("a running Hyprland session is required")
    if args.idle_seconds <= 0:
        parser.error("idle duration must be positive")
    output = args.output.resolve()
    if output.exists():
        parser.error(f"refusing to overwrite output: {output}")
    if not args.reuse_build:
        subprocess.run(
            [
                "cargo",
                "build",
                "--release",
                "-q",
                "-p",
                "splinterd",
                "-p",
                "splinterm",
                "-p",
                "splinterm-pty",
            ],
            cwd=ROOT,
            check=True,
        )
    inputs = source_inputs(args.photo, args.alpha)
    validate_inputs(inputs)
    output.mkdir(parents=True)
    prepared, input_records = prepare_inputs(output, inputs)
    binaries = prepare_binaries(output)
    write_manifest(output, input_records, binaries, args.samples)
    reports: list[dict[str, Any]] = []
    mutable_plan = list(plan)
    for item in mutable_plan:
        report = run_case(
            output, item, prepared[str(item["fixture"])], args.idle_seconds
        )
        reports.append(report)
        print(json.dumps(report, indent=2, sort_keys=True))
        if not report["valid"]:
            break
    if (
        mode in ("benchmark", "all")
        and args.samples >= 3
        and args.samples < 5
        and len(reports) == len(mutable_plan)
        and all(report["valid"] for report in reports)
    ):
        extension_failed = False
        for stack in STACKS:
            summary = benchmark_summary(reports, stack)["trigger_to_stable_ns"]
            if summary["spread"] is None or summary["spread"] <= 0.20:
                continue
            for sample in range(args.samples + 1, min(5, args.samples + 2) + 1):
                item = {
                    "stack": stack,
                    "fixture": "photo",
                    "kind": "benchmark",
                    "sample": sample,
                    "warmup": False,
                }
                mutable_plan.append(item)
                report = run_case(output, item, prepared["photo"], args.idle_seconds)
                reports.append(report)
                print(json.dumps(report, indent=2, sort_keys=True))
                if not report["valid"]:
                    extension_failed = True
                    break
            if extension_failed:
                break
    summary = {
        "schema": SCHEMA,
        "mode": mode,
        "valid": len(reports) == len(mutable_plan)
        and all(report["valid"] for report in reports),
        "completed_runs": len(reports),
        "planned_runs": len(mutable_plan),
        "benchmark": {
            stack: benchmark_summary(reports, stack)
            for stack in STACKS
            if any(
                report["stack"] == stack and report["kind"] == "benchmark"
                for report in reports
            )
        },
    }
    atomic_json(output / "summary.json", summary)
    print(json.dumps(summary, indent=2, sort_keys=True))
    return 0 if summary["valid"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
