#!/usr/bin/env python3
"""Launch Splinterm inside a nested 2x Hyprland on empty workspace 8."""

from __future__ import annotations

import argparse
import json
import os
import shlex
import subprocess
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_WORKSPACE = 8
DEFAULT_SCALE = 2
STATE_DIR = Path("/tmp/splinterm-nested-wayland")


def run(command: list[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, text=True, check=False, **kwargs)


def workspace_clients(workspace: int) -> list[dict[str, Any]]:
    result = run(["hyprctl", "clients", "-j"], capture_output=True)
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip() or "hyprctl clients failed")
    return [
        client
        for client in json.loads(result.stdout)
        if client.get("workspace", {}).get("id") == workspace
    ]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=int, default=DEFAULT_WORKSPACE)
    parser.add_argument("--scale", type=int, default=DEFAULT_SCALE)
    parser.add_argument(
        "--capture",
        type=Path,
        default=Path("/tmp/splinterm-row-scale-2.ppm"),
    )
    args = parser.parse_args()
    if args.workspace <= 0 or args.scale <= 0:
        print("Workspace and scale must be positive integers.", file=sys.stderr)
        return 2
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        print("A running outer Hyprland session is required.", file=sys.stderr)
        return 2
    try:
        clients = workspace_clients(args.workspace)
    except (RuntimeError, json.JSONDecodeError) as error:
        print(f"Cannot inspect workspace {args.workspace}: {error}", file=sys.stderr)
        return 2
    if clients:
        print(
            f"Refusing to launch: workspace {args.workspace} contains "
            f"{len(clients)} window(s).",
            file=sys.stderr,
        )
        for client in clients:
            print(
                f"  {client.get('class', '?')}: {client.get('title', '?')}",
                file=sys.stderr,
            )
        return 2

    build = run(
        [
            "cargo",
            "build",
            "--quiet",
            "-p",
            "splinterm",
            "--example",
            "wayland-window-spike",
        ],
        cwd=ROOT,
    )
    if build.returncode != 0:
        return build.returncode

    STATE_DIR.mkdir(mode=0o700, parents=True, exist_ok=True)
    capture = args.capture.resolve()
    capture.unlink(missing_ok=True)
    app_log = STATE_DIR / "splinterm.log"
    compositor_log = STATE_DIR / "hyprland.log"
    config = STATE_DIR / "hyprland.lua"
    launcher = STATE_DIR / "launch.sh"
    app = ROOT / "target" / "debug" / "examples" / "wayland-window-spike"
    app_command = shlex.join(
        [
            str(app),
            "--capture",
            str(capture),
            "--capture-scale",
            str(args.scale),
        ]
    ) + f" >{shlex.quote(str(app_log))} 2>&1"

    config.write_text(
        "hl.monitor({ output = '', mode = '1920x1200@60', "
        f"position = '0x0', scale = {args.scale} }})\n"
        "hl.config({\n"
        "  decoration = { rounding = 0, shadow = { enabled = false } },\n"
        "  misc = { disable_hyprland_logo = true, disable_splash_rendering = true },\n"
        "})\n"
        "hl.on('hyprland.start', function()\n"
        f"  hl.exec_cmd({json.dumps(app_command)})\n"
        "end)\n",
        encoding="utf-8",
    )
    verified = run(
        ["Hyprland", "--verify-config", "--config", str(config)],
        capture_output=True,
    )
    if verified.returncode != 0:
        print(verified.stderr or verified.stdout, file=sys.stderr)
        return verified.returncode

    launcher.write_text(
        "#!/usr/bin/env bash\n"
        f"exec Hyprland --config {shlex.quote(str(config))} "
        f">{shlex.quote(str(compositor_log))} 2>&1\n",
        encoding="utf-8",
    )
    launcher.chmod(0o700)
    expression = (
        f"hl.exec_cmd({json.dumps(str(launcher))}, "
        f"{{ workspace = {args.workspace} }})"
    )
    dispatched = run(["hyprctl", "eval", expression], capture_output=True)
    if dispatched.returncode != 0:
        print(dispatched.stderr or dispatched.stdout, file=sys.stderr)
        return dispatched.returncode

    print(
        f"Nested {args.scale}x Hyprland launched on workspace {args.workspace}.\n"
        f"Capture: {capture}\nLogs: {STATE_DIR}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
