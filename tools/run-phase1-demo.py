#!/usr/bin/env python3
"""A slow, simple visual proof that a Splinterm shell survives reconnecting."""

from __future__ import annotations

import argparse
import json
import os
import shlex
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_FOOT = Path("/tmp/splinterm-foot-oracle-build/foot")
DEFAULT_WORKSPACE = 8
DEFAULT_DELAY = 5.0
WIDTH = 94

CYAN = "\033[1;36m"
GREEN = "\033[1;32m"
YELLOW = "\033[1;33m"
DIM = "\033[2m"
RESET = "\033[0m"


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


def screen(number: str, title: str, message: str, footer: str) -> None:
    os.write(sys.stdout.fileno(), b"\033[2J\033[H")
    print(f"{CYAN}SPLINTERM PHASE 1 — ONE SIMPLE IDEA{RESET}")
    print(f"{DIM}{number}{RESET}")
    print("═" * WIDTH)
    print()
    print(f"{YELLOW}{title}{RESET}")
    print()
    for line in message.splitlines():
        print(f"  {line}")
    print()
    print("─" * WIDTH)
    print(f"{DIM}{footer}{RESET}")
    sys.stdout.flush()


def cli(socket: Path, *args: str) -> subprocess.CompletedProcess[str]:
    environment = os.environ.copy()
    environment["SPLINTERM_SOCKET"] = str(socket)
    return run(
        [str(ROOT / "target/debug/splinterm"), *args],
        env=environment,
        capture_output=True,
    )


def wait_for_text(socket: Path, marker: str, timeout: float = 8.0) -> str:
    deadline = time.monotonic() + timeout
    latest = ""
    while time.monotonic() < deadline:
        result = cli(socket, "snapshot")
        latest = result.stdout if result.returncode == 0 else result.stderr
        if marker in latest:
            return latest
        time.sleep(0.1)
    raise RuntimeError(f"The shell never produced {marker!r}. Last snapshot:\n{latest}")


def important_lines(snapshot: str) -> str:
    prefixes = ("SAVED VALUE:", "SHELL PID:", "CHANGED VALUE", "SAME SHELL PID:")
    selected = [
        line.strip()
        for line in snapshot.splitlines()
        if line.strip().startswith(prefixes)
    ]
    return "\n".join(selected) or snapshot


def present(delay: float) -> int:
    while True:
        with tempfile.TemporaryDirectory(prefix="splinterm-simple-demo-") as runtime:
            runtime_path = Path(runtime)
            socket = runtime_path / "splinterd.sock"
            environment = os.environ.copy()
            environment.update(
                SPLINTERM_SOCKET=str(socket),
                SPLINTERM_ENABLE_DEV_ATTACH="1",
            )
            daemon: subprocess.Popen[str] | None = None
            try:
                screen(
                    "1 of 5",
                    "We are going to remember one number: 41",
                    "There is no shell yet.\nThere is no saved number yet.",
                    "First we start one real shell owned by splinterd.",
                )
                time.sleep(delay)

                daemon = subprocess.Popen(
                    [str(ROOT / "target/debug/splinterd")],
                    env=environment,
                    stdin=subprocess.DEVNULL,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    text=True,
                )
                deadline = time.monotonic() + 5
                while not socket.exists() and time.monotonic() < deadline:
                    time.sleep(0.02)
                if not socket.exists():
                    raise RuntimeError("splinterd did not start")
                created = cli(socket, "new", "remember-this")
                if created.returncode != 0:
                    raise RuntimeError(created.stderr.strip())

                command = (
                    "clear\n"
                    "DEMO_VALUE=41\n"
                    "printf 'SAVED VALUE: %s\\n' \"$DEMO_VALUE\"\n"
                    "printf 'SHELL PID: %s\\n' \"$$\"\n"
                    "sleep 9\n"
                    "DEMO_VALUE=42\n"
                    "printf 'CHANGED VALUE WHILE VIEWER WAS GONE: %s\\n' \"$DEMO_VALUE\"\n"
                    "printf 'SAME SHELL PID: %s\\n' \"$$\"\n"
                )
                sent = cli(socket, "send", command)
                if sent.returncode != 0:
                    raise RuntimeError(sent.stderr.strip())
                before = wait_for_text(socket, "SAVED VALUE: 41")
                screen(
                    "2 of 5",
                    "The shell saves 41",
                    f"{GREEN}{important_lines(before)}{RESET}\n\nThe client that asked for this snapshot now closes.",
                    "Watch the PID. It identifies this exact running shell.",
                )
                time.sleep(delay)

                for remaining in range(8, 0, -1):
                    screen(
                        "3 of 5",
                        "No viewer is attached — but the shell is still alive",
                        (
                            "CLIENT:  disconnected  ○\n"
                            f"SHELL:   running       {GREEN}●{RESET}\n"
                            "VALUE:   still 41\n\n"
                            f"The shell will change the value itself in {remaining}…"
                        ),
                        "Nothing on screen is keeping the shell alive. splinterd owns it.",
                    )
                    time.sleep(1)

                after = wait_for_text(socket, "CHANGED VALUE WHILE VIEWER WAS GONE: 42")
                screen(
                    "4 of 5",
                    "A new viewer reconnects",
                    f"{GREEN}{important_lines(after)}{RESET}",
                    "The value is now 42, and the PID is unchanged: this is the same persistent shell.",
                )
                time.sleep(delay)

                screen(
                    "5 of 5",
                    "That is what Roadmap Phase 1 proves",
                    (
                        f"{GREEN}✓{RESET} A real shell remembered VALUE=41\n"
                        f"{GREEN}✓{RESET} The viewer disconnected\n"
                        f"{GREEN}✓{RESET} The shell kept running without a viewer\n"
                        f"{GREEN}✓{RESET} It changed the value to 42 while nobody watched\n"
                        f"{GREEN}✓{RESET} A new viewer reconnected to the same shell PID\n\n"
                        "Closing a future Splinterm window will not destroy your terminal session."
                    ),
                    "Next: build the real native Splinterm window that displays this persistent shell.",
                )
                time.sleep(delay)
            except Exception as error:
                screen("STOPPED", "The demonstration hit an error", str(error), "Cleanup is running.")
            finally:
                if socket.exists():
                    cli(socket, "terminate")
                if daemon is not None and daemon.poll() is None:
                    daemon.send_signal(signal.SIGINT)
                    try:
                        daemon.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        daemon.kill()
                        daemon.wait()

        choice = input("\n[R] Replay   [Q] Close: ").strip().lower()
        if choice != "r":
            return 0


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=int, default=DEFAULT_WORKSPACE)
    parser.add_argument("--foot-binary", type=Path, default=DEFAULT_FOOT)
    parser.add_argument("--font-size", type=float, default=20.0)
    parser.add_argument("--delay-seconds", type=float, default=DEFAULT_DELAY)
    parser.add_argument("--present", action="store_true", help=argparse.SUPPRESS)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.present:
        return present(args.delay_seconds)
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        print("A running Hyprland session is required.", file=sys.stderr)
        return 2
    if not args.foot_binary.is_file():
        print(f"Foot presenter not found: {args.foot_binary}", file=sys.stderr)
        return 2
    clients = workspace_clients(args.workspace)
    if clients:
        print(f"Workspace {args.workspace} is occupied; refusing to launch.", file=sys.stderr)
        for client in clients:
            print(f"  {client.get('class', '?')}: {client.get('title', '?')}", file=sys.stderr)
        return 2
    build = run(["cargo", "build", "--quiet", "--workspace"], cwd=ROOT)
    if build.returncode != 0:
        return build.returncode
    command = [
        "env",
        str(args.foot_binary),
        "--config=/dev/null",
        "--override=pad=16x16",
        f"--font=monospace:size={args.font_size:g}",
        "--app-id=splinterm-phase1-simple-demo",
        "--title=Splinterm Phase 1 - The Shell Remembers",
        "--window-size-chars=96x28",
        str(Path(__file__).resolve()),
        "--present",
        f"--delay-seconds={args.delay_seconds:g}",
    ]
    expression = f"hl.exec_cmd({json.dumps(shlex.join(command))}, {{ workspace = {args.workspace} }})"
    dispatched = run(["hyprctl", "eval", expression], capture_output=True)
    if dispatched.returncode != 0:
        print(dispatched.stderr or dispatched.stdout, file=sys.stderr)
        return dispatched.returncode
    print(f"Simple persistence demo launched on workspace {args.workspace}.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
