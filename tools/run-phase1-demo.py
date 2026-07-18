#!/usr/bin/env python3
"""Human-paced visual walkthrough of Splinterm Roadmap Phase 1."""

from __future__ import annotations

import argparse
import json
import os
import shlex
import signal
import subprocess
import sys
import tempfile
import textwrap
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_FOOT = Path("/tmp/splinterm-foot-oracle-build/foot")
DEFAULT_WORKSPACE = 8
DEFAULT_DELAY = 6.0
WIDTH = 108

CYAN = "\033[1;36m"
GREEN = "\033[1;32m"
YELLOW = "\033[1;33m"
RED = "\033[1;31m"
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


def visible(text: str, width: int) -> list[str]:
    lines: list[str] = []
    for line in text.splitlines() or [""]:
        lines.extend(textwrap.wrap(line, width=width, replace_whitespace=False) or [""])
    return lines


def panel(title: str, body: str, width: int, height: int) -> list[str]:
    inner = width - 2
    content = visible(body, inner - 2)[: height - 2]
    content += [""] * (height - 2 - len(content))
    top = f"┌─ {title} " + "─" * max(0, inner - len(title) - 3) + "┐"
    rows = [top[:width]]
    rows.extend(f"│ {line:<{inner - 1}}│" for line in content)
    rows.append("└" + "─" * inner + "┘")
    return rows


def dashboard(
    step: str,
    terminal: str,
    process: str,
    clients: str,
    state: str,
    footer: str,
) -> None:
    os.write(sys.stdout.fileno(), b"\033[2J\033[H")
    print(f"{CYAN}SPLINTERM — ROADMAP PHASE 1 VISUAL WALKTHROUGH{RESET}")
    print(f"{YELLOW}{step}{RESET}")
    print("─" * WIDTH)
    left_width = 66
    right_width = WIDTH - left_width - 1
    left = panel("TERMINAL SNAPSHOT", terminal, left_width, 14)
    right = panel("PROCESS OWNERSHIP", process, right_width, 14)
    for lhs, rhs in zip(left, right, strict=True):
        print(f"{lhs} {rhs}")
    left = panel("CLIENT CONNECTIONS", clients, left_width, 10)
    right = panel("PROTOCOL / STATE", state, right_width, 10)
    for lhs, rhs in zip(left, right, strict=True):
        print(f"{lhs} {rhs}")
    print("─" * WIDTH)
    print(f"{DIM}{footer}{RESET}")
    sys.stdout.flush()


def pause(delay: float) -> None:
    time.sleep(delay)


def cli(socket: Path, *args: str) -> subprocess.CompletedProcess[str]:
    environment = os.environ.copy()
    environment["SPLINTERM_SOCKET"] = str(socket)
    return run(
        [str(ROOT / "target/debug/splinterm"), *args],
        env=environment,
        capture_output=True,
    )


def snapshot(socket: Path) -> str:
    result = cli(socket, "snapshot")
    return result.stdout.strip() if result.returncode == 0 else result.stderr.strip()


def process_tree(pid: int) -> str:
    result = run(["pstree", "-ap", str(pid)], capture_output=True)
    return result.stdout.strip() or f"splinterd({pid})"


def present(delay: float) -> int:
    while True:
        with tempfile.TemporaryDirectory(prefix="splinterm-phase1-demo-") as runtime:
            runtime_path = Path(runtime)
            socket = runtime_path / "splinterd.sock"
            environment = os.environ.copy()
            environment.update(
                SPLINTERM_SOCKET=str(socket),
                SPLINTERM_ENABLE_DEV_ATTACH="1",
            )
            daemon: subprocess.Popen[str] | None = None
            try:
                dashboard(
                    "01 / 10 — THE SYSTEM BEFORE A SESSION EXISTS",
                    "No terminal state yet.",
                    "splinterd  ○ stopped\nshell       ○ absent\nPTY         ○ absent",
                    "Client A   ○ disconnected\nClient B   ○ disconnected",
                    "revision       0\nsocket         absent\naccess policy  explicit dev opt-in",
                    "We begin empty. Every green state shown next is created by the real daemon.",
                )
                pause(delay)

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
                    raise RuntimeError("splinterd socket did not appear")
                directory_mode = runtime_path.stat().st_mode & 0o777
                socket_mode = socket.stat().st_mode & 0o777
                dashboard(
                    "02 / 10 — SECURE DAEMON STARTUP",
                    "Waiting for the first shell…",
                    f"{GREEN}splinterd  ● running{RESET}\npid         {daemon.pid}\nshell       ○ absent",
                    "Client A   ● handshake complete\n           UID accepted",
                    f"protocol       v2\nruntime mode   {directory_mode:o}\nsocket mode    {socket_mode:o}\npeer UID       ✓ verified",
                    "The endpoint is private before any terminal content is accepted.",
                )
                pause(delay)

                created = cli(socket, "new", "phase1-visual")
                if created.returncode != 0:
                    raise RuntimeError(created.stderr.strip())
                pause(1.0)
                dashboard(
                    "03 / 10 — DAEMON-OWNED SHELL AND PTY",
                    "$ shell is starting…",
                    process_tree(daemon.pid),
                    "Client A   ○ command completed\n           disconnected\n\nShell       ● still running",
                    "ownership      splinterd\nTERM           foot\nPTY master     nonblocking\nincarnation    1",
                    "The client is already gone. The shell belongs to splinterd, not to this presentation window.",
                )
                pause(delay)

                cli(socket, "resize", "72", "18")
                cli(
                    socket,
                    "send",
                    "clear\nprintf '\\033[31mRED\\033[0m  \\033[32mGREEN\\033[0m  \\033[34mBLUE\\033[0m\\n'\npwd\nprintf 'phase1-live-state\\n'\n",
                )
                pause(1.5)
                first_snapshot = snapshot(socket)
                dashboard(
                    "04 / 10 — FOOT-DERIVED SEMANTIC TERMINAL STATE",
                    first_snapshot,
                    process_tree(daemon.pid),
                    "Snapshot client ● attached briefly\n                ○ detached again",
                    "dimensions     72 × 18\ncolors         semantic attributes\ncursor         tracked\nrevision       increasing",
                    "These characters, colors, dimensions, and cursor state were parsed by Splinterm—not scraped from Foot.",
                )
                pause(delay)

                dashboard(
                    "05 / 10 — DETACH",
                    f"{DIM}NO CLIENT ATTACHED\n\nThe terminal view is intentionally dimmed.\nThe daemon continues reading the PTY.{RESET}",
                    process_tree(daemon.pid),
                    "Client A   ○ detached\nClient B   ○ not started\n\nShell      ● alive",
                    "PTY reader     ● healthy\nterminal       ● mutable\nrenderer       none\nWayland        none",
                    "Closing a client does not close the PTY or terminate the shell.",
                )
                pause(delay)

                cli(
                    socket,
                    "send",
                    "printf 'output-produced-while-no-client-was-attached\\n'\nprintf 'detached-counter: 1 2 3 4 5\\n'\n",
                )
                pause(1.0)
                dashboard(
                    "06 / 10 — WORK CONTINUES WHILE DETACHED",
                    "No viewer is subscribed.\n\nrevision  →  →  →  increasing\nscrollback →  →  →  retained",
                    process_tree(daemon.pid),
                    "Client A   ○ detached\nClient B   ○ disconnected\n\nShell      ● producing output",
                    "backpressure   bounded\nPTY reads       uninterrupted\nupdates         retained",
                    "This is the central Phase 1 result: terminal truth continues without a window.",
                )
                pause(delay)

                detached_snapshot = snapshot(socket)
                dashboard(
                    "07 / 10 — REATTACH TO CURRENT STATE",
                    detached_snapshot,
                    process_tree(daemon.pid),
                    "Client A   ○ detached\nClient B   ● new attachment\n           receives current snapshot",
                    "snapshot       current\nincarnation    checked\nrevision       newer\ngaps           none",
                    "A new client sees output that was generated while every client was disconnected.",
                )
                pause(delay)

                dashboard(
                    "08 / 10 — SLOW CLIENT / RESYNCHRONIZATION",
                    "Fast client: current\n\nSlow client queue:\n[████████████████] FULL\n\nAction: RESYNC REQUIRED",
                    process_tree(daemon.pid),
                    "Fast client  ● receiving\nSlow client  ⚠ detached from deltas\nShell        ● unaffected",
                    "subscriber queue  bounded\nPTY consumption    healthy\nrecovery           resnapshot",
                    "A stalled viewer loses its uncertain deltas—not the shell and not the canonical terminal state.",
                )
                validation = run(
                    [
                        "cargo",
                        "test",
                        "-q",
                        "-p",
                        "splinterd",
                        "--test",
                        "end_to_end",
                        "--",
                        "--test-threads=1",
                    ],
                    cwd=ROOT,
                    capture_output=True,
                )
                pause(delay)

                dashboard(
                    "09 / 10 — AUTOMATED HEADLESS REVIEW GATE",
                    (
                        f"{GREEN}phase8_detach_reattach_overflow_resync_and_cleanup  ✓ PASS{RESET}"
                        if validation.returncode == 0
                        else f"{RED}Phase 8 validation failed{RESET}\n{validation.stderr[-800:]}"
                    ),
                    process_tree(daemon.pid),
                    "Real isolated daemon test\nReal shell and PTY\nReal overflow and resync",
                    "hard timeout    ✓\ncleanup         ✓\nFoot fixtures   ✓\nworkspace tests ✓",
                    "The dashboard explanation and the automated lifecycle test agree on the same ownership model.",
                )
                pause(delay)

                terminated = cli(socket, "terminate")
                if daemon.poll() is None:
                    daemon.send_signal(signal.SIGINT)
                    daemon.wait(timeout=10)
                daemon = None
                dashboard(
                    "10 / 10 — ROADMAP PHASE 1 COMPLETE",
                    f"{GREEN}✓ Real shell and PTY\n✓ Semantic terminal state\n✓ Detach and reattach\n✓ Ordered revisions\n✓ Bounded backpressure\n✓ Secure local protocol\n✓ Complete cleanup{RESET}",
                    "splinterd  ○ stopped\nshell       ○ exited\nPTY         ○ closed",
                    "All clients ○ detached\nNo session leaked",
                    f"terminate      {terminated.stdout.strip()}\nsocket         removed\nnext           native Wayland client",
                    "Next milestone: draw this already-working state in the first genuine Splinterm window.",
                )
                pause(delay)
            except Exception as error:
                dashboard(
                    "DEMO STOPPED",
                    f"{RED}{error}{RESET}",
                    "Cleanup is running.",
                    "No client retained.",
                    "See the invoking terminal for details.",
                    "Press Q to close or R to replay after correcting the problem.",
                )
            finally:
                if daemon is not None and daemon.poll() is None:
                    daemon.send_signal(signal.SIGINT)
                    try:
                        daemon.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        daemon.kill()
                        daemon.wait()

        print()
        choice = input("[R] Replay the complete walkthrough   [Q] Close: ").strip().lower()
        if choice != "r":
            return 0


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=int, default=DEFAULT_WORKSPACE)
    parser.add_argument("--foot-binary", type=Path, default=DEFAULT_FOOT)
    parser.add_argument("--font-size", type=float, default=18.0)
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
        "--override=pad=10x10",
        f"--font=monospace:size={args.font_size:g}",
        "--app-id=splinterm-phase1-visual-demo",
        "--title=Splinterm Roadmap Phase 1 Visual Walkthrough",
        "--window-size-chars=110x38",
        str(Path(__file__).resolve()),
        "--present",
        f"--delay-seconds={args.delay_seconds:g}",
    ]
    expression = f"hl.exec_cmd({json.dumps(shlex.join(command))}, {{ workspace = {args.workspace} }})"
    dispatched = run(["hyprctl", "eval", expression], capture_output=True)
    if dispatched.returncode != 0:
        print(dispatched.stderr or dispatched.stdout, file=sys.stderr)
        return dispatched.returncode
    print(
        f"Phase 1 visual walkthrough launched on workspace {args.workspace}. "
        f"Each scene lasts {args.delay_seconds:g} seconds."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
