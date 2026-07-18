#!/usr/bin/env python3
"""Launch a real daemon snapshot in Splinterm on empty workspace 8."""

from __future__ import annotations

import json
import os
import shlex
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
WORKSPACE = 8
STATE = Path("/tmp/splinterm-static-snapshot-demo")


def run(command: list[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, text=True, check=False, **kwargs)


def workspace_clients() -> list[dict[str, Any]]:
    result = run(["hyprctl", "clients", "-j"], capture_output=True)
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip() or "hyprctl clients failed")
    return [
        client
        for client in json.loads(result.stdout)
        if client.get("workspace", {}).get("id") == WORKSPACE
    ]


def client(socket: Path, *arguments: str) -> subprocess.CompletedProcess[str]:
    environment = os.environ.copy()
    environment["SPLINTERM_SOCKET"] = str(socket)
    return run(
        [str(ROOT / "target/debug/splinterm"), *arguments],
        env=environment,
        capture_output=True,
    )


def main() -> int:
    if not os.environ.get("HYPRLAND_INSTANCE_SIGNATURE"):
        print("A running Hyprland session is required.", file=sys.stderr)
        return 2
    occupied = workspace_clients()
    if occupied:
        print(f"Workspace {WORKSPACE} is occupied; refusing to launch.", file=sys.stderr)
        for window in occupied:
            print(f"  {window.get('class', '?')}: {window.get('title', '?')}", file=sys.stderr)
        return 2

    build = run(["cargo", "build", "--quiet", "--workspace"], cwd=ROOT)
    if build.returncode != 0:
        return build.returncode
    previous_pid = STATE / "daemon.pid"
    if previous_pid.exists():
        try:
            os.kill(int(previous_pid.read_text(encoding="utf-8").strip()), 15)
        except (OSError, ValueError):
            pass
    shutil.rmtree(STATE, ignore_errors=True)
    STATE.mkdir(mode=0o700)
    socket = STATE / "splinterd.sock"
    environment = os.environ.copy()
    environment.update(
        SPLINTERM_SOCKET=str(socket),
        SPLINTERM_ENABLE_DEV_ATTACH="1",
    )
    daemon_log = (STATE / "splinterd.log").open("w", encoding="utf-8")
    daemon = subprocess.Popen(
        [str(ROOT / "target/debug/splinterd")],
        env=environment,
        stdin=subprocess.DEVNULL,
        stdout=daemon_log,
        stderr=subprocess.STDOUT,
        start_new_session=True,
        text=True,
    )
    (STATE / "daemon.pid").write_text(f"{daemon.pid}\n", encoding="utf-8")
    deadline = time.monotonic() + 5
    while not socket.exists() and time.monotonic() < deadline:
        time.sleep(0.02)
    if not socket.exists():
        daemon.terminate()
        print("The isolated daemon did not create its socket.", file=sys.stderr)
        return 1

    created = client(socket, "new", "static-snapshot")
    if created.returncode != 0:
        daemon.terminate()
        print(created.stderr, file=sys.stderr)
        return created.returncode
    payload = (
        "clear\n"
        "printf '\\033[1;36mSPLINTERM STATIC SNAPSHOT\\033[0m\\n'\n"
        "printf 'ASCII  box: ┌─┼─┐  nerd: \\n'\n"
        "printf 'composed: é  wide: 界  emoji: 🙂\\n'\n"
        "printf '\\033[31mred\\033[0m  \\033[32mgreen\\033[0m  \\033[34mblue\\033[0m\\n'\n"
        "printf 'This frame came from the daemon-owned PTY.\\n'\n"
        "printf '%s%s\\n' 'FRAME_' 'READY_7D31'\n"
    )
    sent = client(socket, "send", payload)
    if sent.returncode != 0:
        daemon.terminate()
        print(sent.stderr, file=sys.stderr)
        return sent.returncode
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        snapshot = client(socket, "snapshot")
        if "FRAME_READY_7D31" in snapshot.stdout:
            break
        time.sleep(0.05)
    else:
        daemon.terminate()
        print("The marker did not reach the terminal snapshot.", file=sys.stderr)
        return 1

    window = [
        "env",
        f"SPLINTERM_SOCKET={socket}",
        str(ROOT / "target/debug/splinterm"),
        "window",
    ]
    cleanup = STATE / "cleanup.sh"
    cleanup.write_text(
        "#!/usr/bin/env bash\n"
        f"env SPLINTERM_SOCKET={shlex.quote(str(socket))} "
        f"{shlex.quote(str(ROOT / 'target/debug/splinterm'))} terminate >/dev/null 2>&1 || true\n"
        f"kill {daemon.pid} 2>/dev/null || true\n",
        encoding="utf-8",
    )
    cleanup.chmod(0o700)
    expression = (
        f"hl.exec_cmd({json.dumps(shlex.join(window))}, "
        f"{{ workspace = {WORKSPACE} }})"
    )
    dispatched = run(["hyprctl", "eval", expression], capture_output=True)
    if dispatched.returncode != 0:
        daemon.terminate()
        print(dispatched.stderr or dispatched.stdout, file=sys.stderr)
        return dispatched.returncode
    print(f"Static daemon snapshot window launched on workspace {WORKSPACE}.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
