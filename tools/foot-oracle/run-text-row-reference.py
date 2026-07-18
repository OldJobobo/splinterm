#!/usr/bin/env python3
"""Launch the pinned Foot/fcft text-row reference on empty workspace 8."""

from __future__ import annotations

import argparse
import json
import os
import shlex
import subprocess
import sys
from pathlib import Path
from typing import Any

DEFAULT_FOOT = Path("/tmp/splinterm-foot-build/foot")
DEFAULT_WORKSPACE = 8
CORPUS = "ASCII ┌─┼─┐ \uf120 e\u0301 界 🙂"


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
    parser.add_argument("--foot-binary", type=Path, default=DEFAULT_FOOT)
    args = parser.parse_args()
    if args.workspace <= 0:
        print("Workspace must be a positive integer.", file=sys.stderr)
        return 2
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        print("A running Hyprland session is required.", file=sys.stderr)
        return 2
    if not args.foot_binary.is_file():
        print(f"Pinned Foot binary not found: {args.foot_binary}", file=sys.stderr)
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
        return 2

    child = "import sys,time; print(sys.argv[1], flush=True); time.sleep(60)"
    command = [
        str(args.foot_binary),
        "--config=/dev/null",
        "--override=pad=0x0",
        "--override=colors.background=0e1216",
        "--override=colors.foreground=ebebeb",
        "--font=JetBrains Mono Nerd Font:pixelsize=22,Noto Sans CJK JP:pixelsize=22,Noto Color Emoji:pixelsize=22",
        "--app-id=com.oldjobobo.splinterm.FootTextRowReference",
        "--title=Splinterm - Pinned Foot Text Row Reference",
        "--window-size-chars=80x24",
        sys.executable,
        "-c",
        child,
        CORPUS,
    ]
    launcher = Path("/tmp/splinterm-foot-text-row-reference.sh")
    launcher.write_text(
        "#!/usr/bin/env bash\n"
        f"exec {shlex.join(command)} >/tmp/splinterm-foot-text-row-reference.log 2>&1\n",
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
    print(f"Pinned Foot text row launched on workspace {args.workspace}.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
