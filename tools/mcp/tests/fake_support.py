#!/usr/bin/env python3
"""Shared helpers for preflight tests: fake executables and fake hooks."""

from __future__ import annotations

import os
from pathlib import Path
import sys
import time
from typing import Callable

HERE = Path(__file__).resolve().parent
FAKE_ADAPTER = HERE / "fake_adapter.py"
FAKE_DAEMON = HERE / "fake_daemon.py"
ROLES = {"daemon": "splinterd", "client": "splinterm", "mcp": "splinterm-mcp"}


def fake_report(version: tuple[int, int, int] = (0, 2, 0), identity: bytes = b"\x11" * 32) -> bytes:
    """A 98-byte sealed build-pair report with the given pair identity/version."""
    frame = bytearray(98)
    frame[:8] = b"SPLTPF01"
    frame[44:76] = identity
    for index, part in enumerate(version):
        frame[76 + index * 2:78 + index * 2] = part.to_bytes(2, "big")
    return bytes(frame)


def executable_for(bin_dir: Path) -> Callable[[int], tuple[int, int] | None]:
    """Map a fake's /proc cmdline back to the staged file it stands for."""
    bin_dir = Path(bin_dir)

    def executable(pid: int) -> tuple[int, int] | None:
        for _ in range(100):
            try:
                argv = [part.decode() for part in Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0") if part]
            except OSError:
                return None
            name = None
            if "--role" in argv:
                name = ROLES[argv[argv.index("--role") + 1]]
            else:
                for part in argv:
                    if Path(part).parent == bin_dir:
                        name = Path(part).name
            if name is not None:
                current = os.stat(bin_dir / name)
                return (current.st_dev, current.st_ino)
            time.sleep(0.01)
        return None

    return executable


class FakeSet:
    """A staged bin dir of fake executables plus matching hooks."""

    def __init__(self, base: Path, daemon_mode: str = "normal", adapter_mode: str = "server") -> None:
        self.bin = Path(base) / "bin"
        self.bin.mkdir()
        self.write("splinterd", FAKE_DAEMON, daemon_mode, "daemon")
        self.write("splinterm-mcp", FAKE_ADAPTER, adapter_mode, "mcp")
        client = self.bin / "splinterm"
        client.write_text("#!/bin/sh\nexit 0\n")
        client.chmod(0o755)
        self.executable = executable_for(self.bin)

    def write(self, name: str, script: Path, mode: str, role: str) -> None:
        path = self.bin / name
        path.write_text(f'#!/bin/sh\nexec "{sys.executable}" "{script}" --mode {mode} --role {role} "$@"\n')
        path.chmod(0o755)
