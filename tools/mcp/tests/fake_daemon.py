#!/usr/bin/env python3
"""Deterministic stand-in for `splinterd` in preflight subprocess tests.

It binds `$SPLINTERM_SOCKET`, answers `ping` with `pong`, and removes the
socket on SIGTERM. Modes inject early exit, SIGTERM resistance, and a
surviving descendant so cleanup can be proven at each boundary.
"""

from __future__ import annotations

import argparse
import os
import signal
import socket
import subprocess
import sys
import time


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", default="normal")
    parser.add_argument("--role", default="daemon")
    arguments = parser.parse_args()
    if arguments.mode == "exit-early":
        sys.exit(5)
    path = os.environ["SPLINTERM_SOCKET"]
    if arguments.mode == "spawn-child":
        # A descendant that outlives a graceful daemon exit and ignores
        # SIGTERM; the harness must reap it by exact (pid, starttime).
        subprocess.Popen(
            [sys.executable, "-c", "import signal,time\nsignal.signal(signal.SIGTERM, signal.SIG_IGN)\ntime.sleep(120)"],
            start_new_session=False,
        )
    stopping = False

    def stop(_signum: int, _frame: object) -> None:
        nonlocal stopping
        if arguments.mode != "ignore-term":
            stopping = True

    signal.signal(signal.SIGTERM, stop)
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    listener.bind(path)
    listener.listen(8)
    listener.settimeout(0.05)
    while not stopping:
        try:
            connection, _ = listener.accept()
        except (socket.timeout, InterruptedError):
            continue
        with connection:
            connection.settimeout(2)
            try:
                if connection.recv(16) == b"ping\n":
                    connection.sendall(b"pong\n")
            except OSError:
                pass
    listener.close()
    if arguments.mode != "leave-socket":
        os.unlink(path)
    time.sleep(0.01)
    sys.exit(0)


if __name__ == "__main__":
    main()
