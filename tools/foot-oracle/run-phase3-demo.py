#!/usr/bin/env python3
"""Build and show the Rust Phase 3 VT demo in Foot on an empty workspace."""

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
        description="Show Splinterm's Rust Phase 3 VT behavior in Foot."
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
        "--font-size",
        type=float,
        default=22.0,
        help="font size used only by this demo window (default: 22)",
    )
    parser.add_argument(
        "--delay-seconds",
        type=float,
        default=6.0,
        help="seconds each frame remains visible (default: 6)",
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
    if not 1 <= args.font_size <= 72:
        print("Font size must be between 1 and 72 points.", file=sys.stderr)
        return 2
    if not 0.1 <= args.delay_seconds <= 60:
        print("Frame delay must be between 0.1 and 60 seconds.", file=sys.stderr)
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
            "phase3-demo",
        ],
        cwd=ROOT,
    )
    if build.returncode != 0:
        return build.returncode

    demo = ROOT / "target" / "debug" / "examples" / "phase3-demo"
    delay_ms = round(args.delay_seconds * 1_000)
    command = [
        "env",
        f"SPLINTERM_DEMO_DELAY_MS={delay_ms}",
        str(args.foot_binary),
        "--config=/dev/null",
        "--override=pad=12x12",
        f"--font=monospace:size={args.font_size:g}",
        "--app-id=splinterm-phase3-demo",
        "--title=Splinterm Phase 3 VT Demo",
        "--window-size-chars=96x32",
        str(demo),
    ]
    expression = (
        f"hl.exec_cmd({json.dumps(shlex.join(command))}, "
        f"{{ workspace = {args.workspace} }})"
    )
    dispatched = run(["hyprctl", "eval", expression], capture_output=True)
    if dispatched.returncode != 0:
        print(dispatched.stderr or dispatched.stdout, file=sys.stderr)
        return dispatched.returncode

    print(
        f"Phase 3 demo launched on workspace {args.workspace}. "
        "Use the final prompt to replay or close it."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
