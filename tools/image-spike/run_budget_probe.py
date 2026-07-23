#!/usr/bin/env python3
"""Measure touched candidate image budgets with transparent /proc accounting."""

from __future__ import annotations

import argparse
import json
import pathlib
import platform
import subprocess
import time
from typing import Any

ROOT = pathlib.Path(__file__).resolve().parents[2]
MANIFEST = pathlib.Path(__file__).with_name("Cargo.toml")
BINARY = pathlib.Path(__file__).with_name("target") / "debug/budget_probe"


def smaps(pid: int) -> dict[str, int]:
    values: dict[str, int] = {}
    for line in pathlib.Path(f"/proc/{pid}/smaps_rollup").read_text().splitlines():
        key, separator, remainder = line.partition(":")
        if separator and remainder.strip().endswith(" kB"):
            values[key] = int(remainder.split()[0]) * 1024
    return {
        "rss_bytes": values["Rss"],
        "pss_bytes": values["Pss"],
        "private_bytes": values.get("Private_Clean", 0)
        + values.get("Private_Dirty", 0),
        "shared_bytes": values.get("Shared_Clean", 0)
        + values.get("Shared_Dirty", 0),
    }


def launch(role: str, size: int) -> tuple[subprocess.Popen[str], dict[str, Any]]:
    process = subprocess.Popen(
        [str(BINARY), role, str(size), "20"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    assert process.stdout is not None
    identity = json.loads(process.stdout.readline())
    return process, identity


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=pathlib.Path)
    args = parser.parse_args()
    subprocess.run(
        ["cargo", "build", "--manifest-path", str(MANIFEST), "--bin", "budget_probe"],
        check=True,
    )
    sizes = {
        "baseline": 0,
        "daemon_authoritative_full": 64 * 1024 * 1024,
        "client_cache_full": 64 * 1024 * 1024,
    }
    processes: list[subprocess.Popen[str]] = []
    records: dict[str, Any] = {}
    try:
        for role, size in sizes.items():
            process, identity = launch(role, size)
            processes.append(process)
            time.sleep(0.1)
            records[role] = {**identity, **smaps(process.pid)}
    finally:
        for process in processes:
            process.terminate()
        for process in processes:
            process.wait(timeout=3)
    baseline = records["baseline"]
    for role in ("daemon_authoritative_full", "client_cache_full"):
        records[role]["rss_delta_bytes"] = (
            records[role]["rss_bytes"] - baseline["rss_bytes"]
        )
        records[role]["pss_delta_bytes"] = (
            records[role]["pss_bytes"] - baseline["pss_bytes"]
        )
    result = {
        "schema": "splinterm.phase5.image-budget-probe.v1",
        "host": {
            "kernel": platform.release(),
            "architecture": platform.machine(),
        },
        "method": "separate safe-Rust processes touch one byte per 4 KiB page; /proc/PID/smaps_rollup sampled after 100 ms",
        "limitations": [
            "This isolates committed byte charges; it is not renderer throughput evidence.",
            "Wayland SHM is unchanged because image composition reuses existing backing buffers.",
            "Final integrated no-image and image-active measurements remain closure gates.",
        ],
        "records": records,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(f"Budget probe written: {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
