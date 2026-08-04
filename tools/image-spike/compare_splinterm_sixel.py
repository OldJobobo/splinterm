#!/usr/bin/env python3
"""Capture one Splinterm Sixel fixture under the repository graphical guard."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import subprocess
import sys
import time
from typing import Any, Callable

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "docs/spikes/artifacts/0025-terminal-images/fixtures/sixel-v1.json"
FOOT_CAPTURES = ROOT / "docs/spikes/artifacts/0025-terminal-images/foot-sixel-captures"
GUARD_PATH = ROOT / "tools/foot-oracle/run-final-buffer-comparison.py"
APP_ID = "com.oldjobobo.splinterm"
WORKSPACE = 8
PINNED_FOOT = "3c5b584b0eafa772eb4376fb6eaf6643399e190e"


def load_guard():
    spec = importlib.util.spec_from_file_location("splinterm_sixel_guard", GUARD_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


V1 = load_guard()


def run(command: list[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
    kwargs.setdefault("check", False)
    return subprocess.run(command, text=True, **kwargs)


def wait_until(predicate: Callable[[], Any], seconds: float, message: str) -> Any:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        V1.assert_user_workspace_untouched()
        time.sleep(0.05)
    raise RuntimeError(message)


def read_ppm(path: Path) -> tuple[int, int, bytes]:
    header, dimensions, maximum, pixels = path.read_bytes().split(b"\n", 3)
    if header != b"P6" or maximum != b"255":
        raise RuntimeError(f"unexpected PPM header in {path}")
    width, height = (int(value) for value in dimensions.split())
    if len(pixels) != width * height * 3:
        raise RuntimeError(f"truncated PPM payload in {path}")
    return width, height, pixels


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def foot_cell_rgb(case_id: str) -> tuple[int, int, bytes, dict[str, Any]]:
    directory = FOOT_CAPTURES / case_id
    report = json.loads((directory / "report.json").read_text(encoding="utf-8"))
    if not report.get("exact") or report.get("foot_commit") != PINNED_FOOT:
        raise RuntimeError("retained Foot Sixel evidence is not exact and pinned")
    metadata = json.loads((directory / "foot.json").read_text(encoding="utf-8"))
    source = (directory / "foot.argb").read_bytes()
    cell_width = int(metadata["cell"]["width"])
    cell_height = int(metadata["cell"]["height"])
    origin_x = int(metadata["origin"]["x"])
    origin_y = int(metadata["origin"]["y"])
    stride = int(metadata["stride"])
    pixels = bytearray()
    for row in range(cell_height):
        start = (origin_y + row) * stride + origin_x * 4
        for bgra in (source[start : start + cell_width * 4][index : index + 4] for index in range(0, cell_width * 4, 4)):
            pixels.extend((bgra[2], bgra[1], bgra[0]))
    return cell_width, cell_height, bytes(pixels), metadata


def capture_cell_rgb(
    pixels: bytes, width: int, height: int, origin_x: int, origin_y: int,
    cell_width: int, cell_height: int,
) -> bytes:
    if origin_x + cell_width > width or origin_y + cell_height > height:
        raise RuntimeError("captured surface does not contain the oracle cell rectangle")
    result = bytearray()
    for row in range(cell_height):
        start = ((origin_y + row) * width + origin_x) * 3
        result.extend(pixels[start : start + cell_width * 3])
    return bytes(result)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("output_dir", type=Path)
    parser.add_argument("--case", required=True)
    parser.add_argument("--reuse-build", action="store_true")
    args = parser.parse_args()
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        parser.error("a running Hyprland session is required")

    fixtures = json.loads(FIXTURES.read_text(encoding="utf-8"))
    case = next((item for item in fixtures["cases"] if item["id"] == args.case), None)
    if case is None:
        parser.error("unknown Sixel fixture")
    case_dir = args.output_dir.resolve() / args.case
    if case_dir.exists():
        parser.error(f"refusing to overwrite existing evidence: {case_dir}")

    monitor_id = V1.assert_test_workspace_isolated()
    V1.assert_user_workspace_untouched()
    active_before = V1.hyprland_json("activeworkspace")
    active_window_before = V1.hyprland_json("activewindow")
    cursor_before = V1.hyprland_json("cursorpos")
    cell_width, cell_height, expected_cell, foot_metadata = foot_cell_rgb(args.case)

    if not args.reuse_build:
        run(
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
    release_daemon = ROOT / "target/release/splinterd"
    release_client = ROOT / "target/release/splinterm"
    release_pty_helper = ROOT / "target/release/splinterm-pty-child"
    if not all(path.is_file() for path in (release_daemon, release_client, release_pty_helper)):
        parser.error("release Splinterm binaries are missing")

    case_dir.mkdir(parents=True)
    private = Path("/tmp") / f"splinterm-sixel-{args.case}"
    shutil.rmtree(private, ignore_errors=True)
    private.mkdir(parents=True, mode=0o700)
    daemon_binary = private / "splinterd"
    client_binary = private / "splinterm"
    pty_helper_binary = private / "splinterm-pty-child"
    shutil.copy2(release_daemon, daemon_binary)
    shutil.copy2(release_client, client_binary)
    shutil.copy2(release_pty_helper, pty_helper_binary)
    daemon_binary_sha256 = sha256(daemon_binary)
    client_binary_sha256 = sha256(client_binary)
    pty_helper_binary_sha256 = sha256(pty_helper_binary)
    if (
        daemon_binary_sha256 != sha256(release_daemon)
        or client_binary_sha256 != sha256(release_client)
        or pty_helper_binary_sha256 != sha256(release_pty_helper)
    ):
        parser.error("private release binary copy did not verify")
    runtime = private / "runtime"
    state = private / "state"
    config = private / "config/splinterm"
    runtime.mkdir(parents=True, mode=0o700)
    state.mkdir(parents=True, mode=0o700)
    config.mkdir(parents=True)
    (config / "config.ini").write_text(
        "[main]\n"
        "font=JetBrains Mono Nerd Font:style=Regular\n"
        "font-pixelsize=12\n"
        "padding-left=12\npadding-right=12\npadding-top=12\npadding-bottom=12\n"
        "[multiplexer]\ndivider-style=none\n",
        encoding="utf-8",
    )
    (config / "theme.json").write_text(
        json.dumps({
            "background": "#0e1216", "foreground": "#ebebeb", "cursor": "#ebebeb",
            "selection": "#44475a", "url": "#8be9fd", "ui_accent": "#ebebeb",
            "pane_border": "#0e1216", "pane_border_active": "#0e1216",
            "ansi": [
                "#000000", "#800000", "#008000", "#808000", "#000080", "#800080",
                "#008080", "#c0c0c0", "#808080", "#ff0000", "#00ff00", "#ffff00",
                "#0000ff", "#ff00ff", "#00ffff", "#ffffff",
            ],
        }, indent=2) + "\n",
        encoding="utf-8",
    )
    socket = runtime / "splinterd.sock"
    capture = case_dir / "splinterm.ppm"
    environment = os.environ.copy()
    environment.update(
        SPLINTERM_SOCKET=str(socket),
        SPLINTERM_ENABLE_DEV_ATTACH="1",
        XDG_STATE_HOME=str(state),
        XDG_CONFIG_HOME=str(private / "config"),
        SPLINTERM_PANE_CHROME_CAPTURE=str(capture),
        SPLINTERM_CAPTURE_REQUIRE_IMAGE="1",
    )
    daemon_log = (case_dir / "daemon.log").open("w", encoding="utf-8")
    daemon = subprocess.Popen(
        [str(daemon_binary)], env=environment, stdin=subprocess.DEVNULL,
        stdout=daemon_log, stderr=subprocess.STDOUT, start_new_session=True, text=True,
    )
    addresses: set[str] = set()
    splint_id: str | None = None
    dojo_id: str | None = None
    result = False
    error: str | None = None
    observed_cell = b""
    width = height = 0
    workspace_never_active = True
    window_never_active = True
    window_placement_preserved = True

    def client(*arguments: str) -> subprocess.CompletedProcess[str]:
        return run(
            [str(client_binary), *arguments], env=environment,
            capture_output=True, timeout=10,
        )

    def checked_client(*arguments: str) -> str:
        completed = client(*arguments)
        if completed.returncode:
            raise RuntimeError(completed.stderr.strip() or f"client {' '.join(arguments)} failed")
        return completed.stdout

    try:
        wait_until(lambda: socket.exists() and client("ping").returncode == 0, 5, "daemon not ready")
        trigger = private / "emit-image"
        child = (
            "import os,time\n"
            f"trigger={str(trigger)!r}\n"
            "deadline=time.monotonic()+20\n"
            "while not os.path.exists(trigger):\n"
            "    if time.monotonic() >= deadline:\n"
            "        raise SystemExit('image trigger timed out')\n"
            "    time.sleep(0.01)\n"
            f"os.write(1,bytes.fromhex({('1b5b324a1b5b481b5b3f32356c' + case['input_hex'])!r}))\n"
            "time.sleep(30)\n"
        )
        name = f"sixel-{args.case}"
        checked_client("new", name, "--", sys.executable, "-c", child)
        listing = checked_client("list", "--all")
        lair = re.search(rf"^([0-9a-f-]{{36}})  {re.escape(name)} ", listing, re.MULTILINE)
        dojos = re.findall(r"^  Dojo ([0-9a-f-]{36})  ", listing, re.MULTILINE)
        splints = re.findall(r"^  ([0-9a-f-]{36})  ", listing, re.MULTILINE)
        if lair is None or len(dojos) != 1 or len(splints) != 1:
            raise RuntimeError(f"unexpected Sixel topology:\n{listing}")
        dojo_id = dojos[0]
        splint_id = splints[0]

        V1.assert_test_workspace_isolated()
        V1.assert_user_workspace_untouched()
        launcher = case_dir / "launch.sh"
        selected_environment = {
            key: value for key, value in environment.items()
            if key in {
                "SPLINTERM_SOCKET", "SPLINTERM_ENABLE_DEV_ATTACH", "XDG_STATE_HOME",
                "XDG_CONFIG_HOME", "SPLINTERM_PANE_CHROME_CAPTURE",
                "SPLINTERM_CAPTURE_REQUIRE_IMAGE", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR",
            }
        }
        command = [
            "env", *[f"{key}={value}" for key, value in selected_environment.items()],
            str(client_binary), "window", "--lair-id", lair.group(1), "--dojo-id", dojo_id,
        ]
        launcher.write_text(
            "#!/bin/sh\nexec " + shlex.join(command)
            + f" >{shlex.quote(str(case_dir / 'splinterm.stdout'))}"
            + f" 2>{shlex.quote(str(case_dir / 'splinterm.stderr'))}\n",
            encoding="utf-8",
        )
        launcher.chmod(0o700)
        existing = {item["address"] for item in V1.all_clients()}
        expression = (
            f"hl.exec_cmd({json.dumps(str(launcher))}, "
            "{ workspace = '8 silent', float = true, size = '960 600', "
            "opacity = '1 1', no_initial_focus = true, no_focus = true })"
        )
        dispatched = run(["hyprctl", "eval", expression], capture_output=True, timeout=5)
        if dispatched.returncode:
            raise RuntimeError(dispatched.stderr.strip() or dispatched.stdout.strip())
        window = wait_until(
            lambda: next((
                item for item in V1.all_clients()
                if item.get("class") == APP_ID and item.get("address") not in existing
            ), None),
            8,
            "Splinterm Sixel window did not map",
        )
        addresses.add(window["address"])
        if window["workspace"]["id"] != WORKSPACE or window["monitor"] != monitor_id:
            raise RuntimeError("Splinterm Sixel window escaped workspace 8 / DP-2")
        if V1.hyprland_json("activewindow").get("address") == window["address"]:
            window_never_active = False
            raise RuntimeError("Splinterm Sixel window received focus")

        def guarded_window_safe() -> None:
            nonlocal window_never_active, window_placement_preserved
            current = next(
                (item for item in V1.all_clients() if item.get("address") == window["address"]),
                None,
            )
            if current is None:
                raise RuntimeError("Splinterm Sixel window closed before capture")
            if current["workspace"]["id"] != WORKSPACE or current["monitor"] != monitor_id:
                window_placement_preserved = False
                raise RuntimeError("Splinterm Sixel window moved outside workspace 8 / DP-2")
            if V1.hyprland_json("activewindow").get("address") == window["address"]:
                window_never_active = False
                raise RuntimeError("Splinterm Sixel window received focus")
            V1.assert_user_workspace_untouched()

        settle_deadline = time.monotonic() + 0.5
        while time.monotonic() < settle_deadline:
            guarded_window_safe()
            time.sleep(0.02)
        trigger.touch()

        def guarded_capture_ready() -> bool:
            guarded_window_safe()
            if not capture.exists():
                return False
            try:
                read_ppm(capture)
            except (OSError, RuntimeError, ValueError):
                return False
            return True

        wait_until(guarded_capture_ready, 8, "Splinterm Sixel capture was not written completely")
        width, height, pixels = read_ppm(capture)
        observed_cell = capture_cell_rgb(
            pixels, width, height,
            int(foot_metadata["origin"]["x"]), int(foot_metadata["origin"]["y"]),
            cell_width, cell_height,
        )
        if observed_cell != expected_cell:
            raise RuntimeError("Splinterm Sixel cell pixels differ from retained pinned Foot")
        V1.assert_user_workspace_untouched()
        result = True
    except Exception as caught:
        error = str(caught)
        if "stole focus to the reserved test workspace" in error:
            workspace_never_active = False
    finally:
        for address in list(addresses):
            V1.kill_oracle_window(address)
        try:
            wait_until(lambda: not V1.workspace_clients(WORKSPACE), 5, "test window remained mapped")
        except Exception as caught:
            error = error or str(caught)
            result = False
        if splint_id is not None:
            client("kill", splint_id, "--yes")
        if dojo_id is not None:
            client("close-dojo", dojo_id)
        daemon.send_signal(signal.SIGINT)
        try:
            daemon.wait(timeout=8)
        except subprocess.TimeoutExpired:
            daemon.kill()
            daemon.wait(timeout=3)
            error = error or "daemon required forced cleanup"
            result = False
        daemon_log.close()

    active_after = V1.hyprland_json("activeworkspace")
    active_window_after = V1.hyprland_json("activewindow")
    cursor_after = V1.hyprland_json("cursorpos")
    cleanup_clean = not V1.workspace_clients(WORKSPACE) and not socket.exists()
    if not cleanup_clean:
        error = error or "workspace or socket cleanup was incomplete"
        result = False

    report = {
        "schema": "splinterm.phase5.splinterm-sixel-comparison.v1",
        "case": args.case,
        "exact": result,
        "final_buffer_exact": observed_cell == expected_cell,
        "foot_commit": PINNED_FOOT,
        "splinterd_binary_sha256": daemon_binary_sha256,
        "splinterm_binary_sha256": client_binary_sha256,
        "splinterm_pty_child_binary_sha256": pty_helper_binary_sha256,
        "fixture_sha256": sha256(FIXTURES),
        "foot_argb_sha256": sha256(FOOT_CAPTURES / args.case / "foot.argb"),
        "capture_sha256": sha256(capture) if capture.exists() else None,
        "surface": {"width": width, "height": height},
        "compared_cell": {"width": cell_width, "height": cell_height},
        "error": error,
        "isolation": {
            "workspace": WORKSPACE,
            "monitor": "DP-2",
            "no_initial_focus": True,
            "workspace_never_active": workspace_never_active,
            "window_never_active": window_never_active,
            "window_placement_preserved": window_placement_preserved,
            "active_workspace_unchanged": active_after == active_before,
            "active_window_unchanged": active_window_after == active_window_before,
            "pointer_unchanged": cursor_after == cursor_before,
            "user_state_changes_are_informational": True,
            "cleanup_verified": cleanup_clean,
        },
    }
    (case_dir / "report.json").write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    shutil.rmtree(private, ignore_errors=True)
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0 if result else 1


if __name__ == "__main__":
    raise SystemExit(main())
