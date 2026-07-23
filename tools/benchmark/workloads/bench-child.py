#!/usr/bin/env python3
"""Emit deterministic terminal workloads and side-channel timing records."""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import sys
import time
from collections.abc import Callable

MARKER = "SPLINTERBENCH_DONE"
VISIBLE_MARKER_RGB = (17, 239, 113)


def visible_marker(columns: int) -> bytes:
    red, green, blue = VISIBLE_MARKER_RGB
    return f"\x1b[48;2;{red};{green};{blue}m".encode() + b" " * columns + b"\x1b[0m\n"


def _fixed_line(prefix: str, index: int, columns: int) -> str:
    label = f"{prefix}-{index:08d} "
    body = ("0123456789abcdef" * ((columns // 16) + 2))[: max(0, columns - len(label))]
    return (label + body)[:columns] + "\n"


def plain(lines: int, columns: int) -> bytes:
    return "".join(
        _fixed_line("plain", index, columns) for index in range(lines)
    ).encode()


def ansi(lines: int, columns: int) -> bytes:
    rendered = []
    for index in range(lines):
        content = _fixed_line("ansi", index, columns).rstrip("\n")
        rendered.append(f"\x1b[3{index % 8};4{(index // 8) % 8};1m{content}\x1b[0m\n")
    return "".join(rendered).encode()


def unicode_text(lines: int, columns: int) -> bytes:
    phrases = ("naïve café é", "日本語 漢字", "λ→∞ ≤ ≥ ≠", "emoji 🙂 🚀")
    rendered = []
    for index in range(lines):
        prefix = f"unicode-{index:08d} {phrases[index % len(phrases)]} "
        rendered.append((prefix + "·" * columns)[:columns] + "\n")
    return "".join(rendered).encode()


def retention(lines: int, columns: int) -> bytes:
    rendered = []
    phrases = ("naïve café é", "日本語 漢字", "λ→∞ ≤ ≥ ≠", "emoji 🙂 🚀")
    for index in range(lines):
        if index and index % 500 == 0:
            rendered.append("\x1b[2J\x1b[H")
        if index % 3 == 0:
            rendered.append(_fixed_line("retain", index, columns))
        elif index % 3 == 1:
            content = _fixed_line("retain", index, columns).rstrip("\n")
            rendered.append(f"\x1b[3{index % 8};1m{content}\x1b[0m\n")
        else:
            prefix = f"retain-{index:08d} {phrases[index % len(phrases)]} "
            rendered.append((prefix + "·" * columns)[:columns] + "\n")
    return "".join(rendered).encode()


WORKLOADS: dict[str, Callable[[int, int], bytes]] = {
    "plain": plain,
    "ansi": ansi,
    "unicode": unicode_text,
    "retention": retention,
}


def write_record(path: pathlib.Path | None, value: dict[str, int | str]) -> None:
    if path is None:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temporary.write_text(json.dumps(value, sort_keys=True) + "\n", encoding="utf-8")
    temporary.replace(path)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Emit a deterministic terminal workload"
    )
    parser.add_argument("case", choices=(*WORKLOADS, "idle"))
    parser.add_argument("--lines", type=int, default=1000)
    parser.add_argument("--idle-seconds", type=float, default=60.0)
    parser.add_argument("--columns", type=int, default=80)
    parser.add_argument("--ready-file", type=pathlib.Path)
    parser.add_argument("--start-file", type=pathlib.Path)
    parser.add_argument("--start-timeout", type=float, default=30.0)
    parser.add_argument("--done-file", type=pathlib.Path)
    parser.add_argument("--hold-seconds", type=float, default=0.0)
    args = parser.parse_args()
    if args.lines < 0:
        parser.error("--lines must not be negative")
    if args.columns < 20:
        parser.error("--columns must be at least 20")
    if not 0.1 <= args.idle_seconds <= 3600:
        parser.error("--idle-seconds must be between 0.1 and 3600")
    if args.start_timeout <= 0 or not 0 <= args.hold_seconds <= 3600:
        parser.error("start timeout must be positive and hold seconds must be bounded")

    payload = (
        b"" if args.case == "idle" else WORKLOADS[args.case](args.lines, args.columns)
    )
    ready_ns = time.monotonic_ns()
    write_record(
        args.ready_file,
        {
            "schema": "splinterm.benchmark.child.v1",
            "event": "ready",
            "monotonic_ns": ready_ns,
            "pid": os.getpid(),
        },
    )
    if args.case == "idle":
        time.sleep(args.idle_seconds)
        return 0
    if args.start_file is not None:
        deadline = time.monotonic() + args.start_timeout
        while not args.start_file.exists():
            if time.monotonic() >= deadline:
                print("benchmark start trigger timed out", file=sys.stderr)
                return 1
            time.sleep(0.002)
    started_ns = time.monotonic_ns()
    output = sys.stdout.buffer
    output.write(payload)
    output.write(f"\x1b[0m{MARKER}\n".encode())
    output.write(visible_marker(args.columns))
    output.flush()
    completed_ns = time.monotonic_ns()
    write_record(
        args.done_file,
        {
            "schema": "splinterm.benchmark.child.v1",
            "event": "write_complete",
            "monotonic_ns": completed_ns,
            "pid": os.getpid(),
            "duration_ns": completed_ns - started_ns,
            "payload_bytes": len(payload),
            "total_bytes": (
                len(payload)
                + len(f"\x1b[0m{MARKER}\n".encode())
                + len(visible_marker(args.columns))
            ),
        },
    )
    if args.hold_seconds:
        time.sleep(args.hold_seconds)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
