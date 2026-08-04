"""Reproducibility manifest collection for Splinterbench."""

from __future__ import annotations

import datetime
import os
import pathlib
import platform
import subprocess
from typing import Any

from adapters import all_adapters
from multiplexers import all_adapters as all_multiplexer_adapters
from multiplexing import stack_identities


def _output(command: list[str], root: pathlib.Path) -> str | None:
    try:
        result = subprocess.run(
            command,
            cwd=root,
            text=True,
            capture_output=True,
            check=False,
            timeout=10,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    return result.stdout.strip() if result.returncode == 0 else None


def _os_name() -> str | None:
    try:
        lines = pathlib.Path("/etc/os-release").read_text(encoding="utf-8").splitlines()
    except OSError:
        return None
    values = {}
    for line in lines:
        if "=" in line:
            key, value = line.split("=", 1)
            values[key] = value.strip().strip('"')
    return values.get("PRETTY_NAME")


def _cpu_name() -> str:
    try:
        lines = pathlib.Path("/proc/cpuinfo").read_text(encoding="utf-8").splitlines()
    except OSError:
        return platform.processor() or "unknown"
    for line in lines:
        if line.startswith("model name"):
            return line.split(":", 1)[1].strip()
    return platform.processor() or "unknown"


def repository_identity(root: pathlib.Path) -> dict[str, str | bool] | None:
    commit = _output(["git", "rev-parse", "HEAD"], root)
    status = _output(["git", "status", "--porcelain"], root)
    if commit is None or status is None:
        return None
    return {"commit": commit, "dirty": bool(status)}


def collect(root: pathlib.Path) -> dict[str, Any]:
    """Collect a schema-valid manifest with no benchmark samples."""

    terminals = [adapter.probe(root) for adapter in all_adapters()]
    multiplexers = [adapter.probe(root) for adapter in all_multiplexer_adapters()]
    return {
        "schema": "splinterm.benchmark.v1",
        "recorded_at": datetime.datetime.now(datetime.UTC).isoformat(),
        "host": {
            "os": _os_name(),
            "kernel": platform.release(),
            "architecture": platform.machine(),
            "cpu": _cpu_name(),
            "clock_ticks_per_second": os.sysconf("SC_CLK_TCK"),
            "python": platform.python_version(),
        },
        "repository": repository_identity(root),
        "terminals": [identity.as_dict() for identity in terminals],
        "multiplexers": [identity.as_dict() for identity in multiplexers],
        "benchmark_stacks": [
            identity.as_dict() for identity in stack_identities(terminals, multiplexers)
        ],
        "samples": [],
    }
