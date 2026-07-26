#!/usr/bin/env python3
"""Run one guarded inactive-workspace smoke for line and frame pane chrome."""

from __future__ import annotations

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

ROOT = Path(__file__).resolve().parents[1]
GUARD_PATH = ROOT / "tools/foot-oracle/run-final-buffer-comparison.py"
APP_ID = "com.oldjobobo.splinterm"
WORKSPACE = 8
INACTIVE = "#445566"
ACTIVE = "#ff00aa"


def load_guard():
    spec = importlib.util.spec_from_file_location("pane_divider_guard", GUARD_PATH)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
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
    header, width_height, maximum, pixels = path.read_bytes().split(b"\n", 3)
    if header != b"P6" or maximum != b"255":
        raise RuntimeError(f"unexpected PPM header in {path}")
    width, height = (int(value) for value in width_height.split())
    if len(pixels) != width * height * 3:
        raise RuntimeError(f"truncated PPM payload in {path}")
    return width, height, pixels


def color_bytes(color: str) -> bytes:
    return bytes.fromhex(color.removeprefix("#"))


def count_color(pixels: bytes, color: str) -> int:
    target = color_bytes(color)
    return sum(pixels[index:index + 3] == target for index in range(0, len(pixels), 3))


def edge_color_count(width: int, height: int, pixels: bytes, colors: tuple[str, ...]) -> int:
    targets = {color_bytes(color) for color in colors}
    count = 0
    for y in range(height):
        for x in range(width):
            if x not in (0, width - 1) and y not in (0, height - 1):
                continue
            index = (y * width + x) * 3
            if pixels[index:index + 3] in targets:
                count += 1
    return count


def main() -> int:
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        print("A running Hyprland session is required.", file=sys.stderr)
        return 2

    output = Path("/tmp/splinterm-phase3-pane-divider-smoke")
    shutil.rmtree(output, ignore_errors=True)
    runtime = output / "runtime"
    state = output / "state"
    config = output / "config/splinterm"
    runtime.mkdir(parents=True, mode=0o700)
    state.mkdir(parents=True, mode=0o700)
    config.mkdir(parents=True)
    (config / "theme.json").write_text(
        json.dumps({
            "background": "#101820", "foreground": "#e8edf2",
            "cursor": ACTIVE, "selection": "#304050", "url": "#55aaff",
            "ui_accent": ACTIVE, "pane_border": INACTIVE,
            "pane_border_active": ACTIVE,
            "ansi": [
                "#101820", "#cc6666", "#99cc99", "#f0c674",
                "#81a2be", "#b294bb", "#8abeb7", "#c5c8c6",
                "#666666", "#d54e53", "#b9ca4a", "#e7c547",
                "#7aa6da", "#c397d8", "#70c0b1", "#eaeaea",
            ],
        }, indent=2) + "\n",
        encoding="utf-8",
    )

    V1.assert_test_workspace_isolated()
    V1.assert_user_workspace_untouched()
    active_before = V1.hyprland_json("activeworkspace")
    active_window_before = V1.hyprland_json("activewindow")
    cursor_before = V1.hyprland_json("cursorpos")

    run(
        ["cargo", "build", "--release", "-q", "-p", "splinterd", "-p", "splinterm"],
        cwd=ROOT,
        check=True,
    )

    socket = runtime / "splinterd.sock"
    environment = os.environ.copy()
    environment.update(
        SPLINTERM_SOCKET=str(socket),
        SPLINTERM_ENABLE_DEV_ATTACH="1",
        XDG_STATE_HOME=str(state),
        XDG_CONFIG_HOME=str(output / "config"),
    )
    daemon_log = (output / "daemon.log").open("w", encoding="utf-8")
    daemon = subprocess.Popen(
        [str(ROOT / "target/release/splinterd")],
        env=environment,
        stdin=subprocess.DEVNULL,
        stdout=daemon_log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
        text=True,
    )
    addresses: set[str] = set()
    splints: list[str] = []
    window_id: str | None = None
    result = False
    error: str | None = None
    captures: dict[str, dict[str, int]] = {}

    def write_config(style: str) -> None:
        (config / "config.ini").write_text(
            "[main]\nfont-pixelsize=18\npadding-left=4\npadding-right=4\n"
            "padding-top=4\npadding-bottom=4\n"
            "[multiplexer]\n"
            f"divider-style={style}\nframe-title=splint\n",
            encoding="utf-8",
        )

    def client(*arguments: str) -> subprocess.CompletedProcess[str]:
        return run(
            [str(ROOT / "target/release/splinterm"), *arguments],
            env=environment,
            capture_output=True,
            timeout=10,
        )

    def checked_client(*arguments: str) -> str:
        completed = client(*arguments)
        if completed.returncode:
            raise RuntimeError(completed.stderr.strip() or f"client {' '.join(arguments)} failed")
        return completed.stdout

    def topology() -> tuple[str, str, list[str]]:
        listing = checked_client("list")
        dojo = re.search(r"^([0-9a-f-]{36})  divider-smoke ", listing, re.MULTILINE)
        windows = re.findall(r"^  window ([0-9a-f-]{36})  ", listing, re.MULTILINE)
        ids = re.findall(r"^  ([0-9a-f-]{36})  ", listing, re.MULTILINE)
        if dojo is None or len(windows) != 1:
            raise RuntimeError(f"unexpected divider topology:\n{listing}")
        return dojo.group(1), windows[0], ids

    def launch(style: str, dojo_id: str, selected_window: str) -> tuple[dict[str, Any], Path]:
        # Required before every graphical launch, not only once per script.
        V1.assert_test_workspace_isolated()
        V1.assert_user_workspace_untouched()
        write_config(style)
        capture = output / f"{style}.ppm"
        launcher = output / f"launch-{style}.sh"
        selected_environment = {
            key: value for key, value in environment.items()
            if key in {
                "SPLINTERM_SOCKET", "SPLINTERM_ENABLE_DEV_ATTACH", "XDG_STATE_HOME",
                "XDG_CONFIG_HOME", "WAYLAND_DISPLAY", "XDG_RUNTIME_DIR",
            }
        }
        selected_environment["SPLINTERM_PANE_CHROME_CAPTURE"] = str(capture)
        command = [
            "env", *[f"{key}={value}" for key, value in selected_environment.items()],
            str(ROOT / "target/release/splinterm"), "window",
            "--dojo-id", dojo_id, "--window-id", selected_window,
        ]
        stdout = output / f"{style}.stdout"
        stderr = output / f"{style}.stderr"
        launcher.write_text(
            "#!/bin/sh\nexec " + shlex.join(command)
            + f" >{shlex.quote(str(stdout))} 2>{shlex.quote(str(stderr))}\n",
            encoding="utf-8",
        )
        launcher.chmod(0o700)
        existing = {item["address"] for item in V1.all_clients()}
        expression = (
            f"hl.exec_cmd({json.dumps(str(launcher))}, "
            "{ workspace = '8 silent', float = true, size = '900 600', "
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
            f"{style} window did not map",
        )
        if window["workspace"]["id"] != WORKSPACE or window["monitor"] != V1.test_monitor_id():
            raise RuntimeError(f"{style} window escaped workspace 8 / DP-2")
        addresses.add(window["address"])
        wait_until(capture.exists, 8, f"{style} capture was not written")
        return window, capture

    try:
        wait_until(lambda: socket.exists() and client("ping").returncode == 0, 5, "daemon not ready")
        checked_client("new", "divider-smoke", "--", "/bin/sh")
        dojo_id, window_id, splints = topology()
        checked_client("split", splints[0], "--axis", "horizontal", "--side", "second", "--", "/bin/sh")
        _, _, two = topology()
        new_second = next(item for item in two if item not in splints)
        checked_client("split", new_second, "--axis", "vertical", "--side", "second", "--", "/bin/sh")
        _, _, splints = topology()
        for splint, title in zip(splints, ["editor", "build logs", "監視 monitor"], strict=True):
            checked_client("rename-splint", splint, title)
            checked_client("send", splint, f"printf '{title} ready\\n'\n")

        line_window, line_path = launch("line", dojo_id, window_id)
        line_width, line_height, line_pixels = read_ppm(line_path)
        captures["line"] = {
            "width": line_width,
            "height": line_height,
            "inactive_pixels": count_color(line_pixels, INACTIVE),
            "active_pixels": count_color(line_pixels, ACTIVE),
            "edge_pixels": edge_color_count(line_width, line_height, line_pixels, (INACTIVE, ACTIVE)),
        }
        V1.kill_oracle_window(line_window["address"])
        addresses.remove(line_window["address"])
        wait_until(lambda: not V1.workspace_clients(WORKSPACE), 5, "line window did not close")

        frame_window, frame_path = launch("frame", dojo_id, window_id)
        frame_width, frame_height, frame_pixels = read_ppm(frame_path)
        captures["frame"] = {
            "width": frame_width,
            "height": frame_height,
            "inactive_pixels": count_color(frame_pixels, INACTIVE),
            "active_pixels": count_color(frame_pixels, ACTIVE),
            "edge_pixels": edge_color_count(frame_width, frame_height, frame_pixels, (INACTIVE, ACTIVE)),
        }
        if captures["line"]["inactive_pixels"] == 0 or captures["line"]["active_pixels"] == 0:
            raise RuntimeError("line capture omitted inactive or active divider colors")
        if captures["frame"]["inactive_pixels"] == 0 or captures["frame"]["active_pixels"] == 0:
            raise RuntimeError("frame capture omitted inactive or active frame colors")
        if captures["frame"]["edge_pixels"] != 0:
            raise RuntimeError("frame corner overdraw leaked strokes to the surface boundary")
        frame_chrome = captures["frame"]["inactive_pixels"] + captures["frame"]["active_pixels"]
        line_chrome = captures["line"]["inactive_pixels"] + captures["line"]["active_pixels"]
        if frame_chrome <= line_chrome * 2:
            raise RuntimeError("frame capture did not produce complete panel chrome")
        if (line_width, line_height) != (frame_width, frame_height):
            raise RuntimeError("style relaunch changed the configured surface dimensions")
        result = True
    except Exception as caught:
        error = str(caught)
    finally:
        for address in list(addresses):
            V1.kill_oracle_window(address)
        try:
            wait_until(lambda: not V1.workspace_clients(WORKSPACE), 5, "test window remained mapped")
        except Exception as caught:
            error = error or str(caught)
            result = False
        for splint in splints:
            client("kill", splint, "--yes")
        if window_id is not None:
            client("close-window", window_id)
        daemon.send_signal(signal.SIGINT)
        try:
            daemon.wait(timeout=8)
        except subprocess.TimeoutExpired:
            daemon.kill()
            daemon.wait(timeout=3)
            result = False
            error = error or "daemon required forced cleanup"
        daemon_log.close()

    active_after = V1.hyprland_json("activeworkspace")
    active_window_after = V1.hyprland_json("activewindow")
    cursor_after = V1.hyprland_json("cursorpos")
    cleanup_clean = not V1.workspace_clients(WORKSPACE) and not socket.exists()
    if active_after != active_before or active_window_after != active_window_before or cursor_after != cursor_before:
        result = False
        error = error or "focus, active workspace, or pointer changed"
    if not cleanup_clean:
        result = False
        error = error or "workspace or socket cleanup was incomplete"

    summary = {
        "schema": "splinterm.phase3.pane-divider-smoke.v1",
        "result": "pass" if result else "fail",
        "error": error,
        "workspace": WORKSPACE,
        "monitor": "DP-2",
        "styles": ["line", "frame"],
        "frame_titles": ["editor", "build logs", "監視 monitor"],
        "captures": captures,
        "active_workspace_unchanged": active_after == active_before,
        "active_window_unchanged": active_window_after == active_window_before,
        "pointer_unchanged": cursor_after == cursor_before,
        "cleanup_clean": cleanup_clean,
    }
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary, indent=2))
    return 0 if result else 1


if __name__ == "__main__":
    raise SystemExit(main())
