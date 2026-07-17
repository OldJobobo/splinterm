#!/usr/bin/env python3
"""Build and show the Rust Phase 2 grid demo in Foot on an empty workspace."""

from __future__ import annotations

import argparse
import json
import os
import shlex
import subprocess
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_FOOT = Path("/tmp/splinterm-foot-oracle-build/foot")
DEFAULT_WORKSPACE = 8


def run(command: list[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, text=True, check=False, **kwargs)


def workspace_clients(workspace: int) -> list[dict[str, Any]]:
    result = run(["hyprctl", "clients", "-j"], capture_output=True)
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip() or "hyprctl clients failed")
    clients = json.loads(result.stdout)
    return [
        client
        for client in clients
        if client.get("workspace", {}).get("id") == workspace
    ]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Show Splinterm's Rust Phase 2 grid behavior in Foot."
    )
    parser.add_argument(
        "--workspace",
        type=int,
        default=int(os.environ.get("SPLINTERM_DEMO_WORKSPACE", DEFAULT_WORKSPACE)),
        help=f"empty Hyprland workspace to use (default: {DEFAULT_WORKSPACE})",
    )
    parser.add_argument(
        "--foot-binary",
        type=Path,
        default=Path(os.environ.get("FOOT_ORACLE_BINARY", DEFAULT_FOOT)),
        help="Foot executable used as the presentation window",
    )
    parser.add_argument(
        "--allow-occupied",
        action="store_true",
        help="launch even when the target workspace contains windows",
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
    if not args.foot_binary.is_file():
        print(
            f"Foot oracle binary not found: {args.foot_binary}\n"
            "Run tools/foot-oracle/build-oracle.sh first.",
            file=sys.stderr,
        )
        return 2

    try:
        clients = workspace_clients(args.workspace)
    except (RuntimeError, json.JSONDecodeError) as error:
        print(f"Cannot inspect workspace {args.workspace}: {error}", file=sys.stderr)
        return 2

    if clients and not args.allow_occupied:
        print(
            f"Refusing to open the demo: workspace {args.workspace} contains "
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
            "splinterm-terminal",
            "--example",
            "phase2-demo",
        ],
        cwd=ROOT,
    )
    if build.returncode != 0:
        return build.returncode

    demo = ROOT / "target" / "debug" / "examples" / "phase2-demo"
    command = [
        str(args.foot_binary),
        "--config=/dev/null",
        "--override=pad=12x12",
        "--app-id=splinterm-phase2-demo",
        "--title=Splinterm Phase 2 Grid Demo",
        "--window-size-chars=92x28",
        "--hold",
        str(demo),
    ]
    shell_command = shlex.join(command)
    expression = f"hl.exec_cmd({json.dumps(shell_command)}, {{ workspace = {args.workspace} }})"
    dispatched = run(["hyprctl", "eval", expression], capture_output=True)
    if dispatched.returncode != 0:
        print(dispatched.stderr or dispatched.stdout, file=sys.stderr)
        return dispatched.returncode

    print(
        f"Phase 2 demo launched on workspace {args.workspace}. "
        "It will remain open on the final frame."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
