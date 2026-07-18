#!/usr/bin/env python3
"""Build and launch the native Splinterm window on empty workspace 8."""

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


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=int, default=DEFAULT_WORKSPACE)
    parser.add_argument(
        "--capture",
        type=Path,
        help="write the first rendered frame as a binary PPM",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.workspace <= 0:
        print("Workspace must be a positive integer.", file=sys.stderr)
        return 2
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        print("A running Hyprland session is required.", file=sys.stderr)
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

    build_command = ["cargo", "build", "--quiet", "-p", "splinterm"]
    if args.capture is not None:
        build_command.extend(["--example", "wayland-window-spike"])
    build = run(build_command, cwd=ROOT)
    if build.returncode != 0:
        return build.returncode

    if args.capture is None:
        command = [str(ROOT / "target" / "debug" / "splinterm"), "window"]
    else:
        command = [
            str(ROOT / "target" / "debug" / "examples" / "wayland-window-spike"),
            "--capture",
            str(args.capture.resolve()),
        ]
    expression = (
        f"hl.exec_cmd({json.dumps(shlex.join(command))}, "
        f"{{ workspace = {args.workspace} }})"
    )
    dispatched = run(["hyprctl", "eval", expression], capture_output=True)
    if dispatched.returncode != 0:
        print(dispatched.stderr or dispatched.stdout, file=sys.stderr)
        return dispatched.returncode

    print(f"Native Splinterm window launched on workspace {args.workspace}.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
