#!/usr/bin/env python3
"""Emit deterministic terminal workloads and side-channel timing records."""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import sys
import time
from collections.abc import Callable, Mapping
from typing import Any

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


def write_record(path: pathlib.Path | None, value: Mapping[str, Any]) -> None:
    if path is None:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temporary.write_text(json.dumps(value, sort_keys=True) + "\n", encoding="utf-8")
    temporary.replace(path)


def read_command(path: pathlib.Path) -> dict[str, Any] | None:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    if not isinstance(value, dict):
        raise TypeError("benchmark control command must be a JSON object")
    return value


def run_controlled(
    control_dir: pathlib.Path,
    timeout_seconds: float,
    default_columns: int,
    input_token: str,
) -> int:
    control_dir.mkdir(parents=True, exist_ok=True)
    deadline = time.monotonic() + timeout_seconds
    sequence = 0
    while time.monotonic() < deadline:
        command_path = control_dir / f"command-{sequence:03d}.json"
        command = read_command(command_path)
        if command is None:
            time.sleep(0.002)
            continue
        if command.get("schema") != "splinterm.benchmark.child-command.v1":
            print("unsupported benchmark control command", file=sys.stderr)
            return 1
        if command.get("sequence") != sequence:
            print("benchmark control command sequence mismatch", file=sys.stderr)
            return 1
        action = command.get("action")
        result_path = control_dir / f"result-{sequence:03d}.json"
        started_ns = time.monotonic_ns()
        if action == "output":
            workload = str(command.get("workload"))
            if workload not in WORKLOADS:
                print("unsupported controlled output workload", file=sys.stderr)
                return 1
            lines = int(command.get("lines", 0))
            columns = int(command.get("columns", default_columns))
            if lines <= 0 or columns < 20:
                print("invalid controlled output dimensions", file=sys.stderr)
                return 1
            payload = WORKLOADS[workload](lines, columns)
            output = sys.stdout.buffer
            output.write(b"\x1b[2J\x1b[H")
            output.write(payload)
            output.write(f"\x1b[0m{MARKER}\n".encode())
            output.write(visible_marker(columns))
            output.flush()
            completed_ns = time.monotonic_ns()
            write_record(
                result_path,
                {
                    "schema": "splinterm.benchmark.child-result.v1",
                    "event": "write_complete",
                    "sequence": sequence,
                    "workload": workload,
                    "monotonic_ns": completed_ns,
                    "pid": os.getpid(),
                    "duration_ns": completed_ns - started_ns,
                    "payload_bytes": len(payload),
                    "total_bytes": len(payload)
                    + len(f"\x1b[0m{MARKER}\n".encode())
                    + len(visible_marker(columns)),
                },
            )
        elif action == "clear":
            output = sys.stdout.buffer
            output.write(b"\x1b[2J\x1b[H")
            output.flush()
            write_record(
                result_path,
                {
                    "schema": "splinterm.benchmark.child-result.v1",
                    "event": "cleared",
                    "sequence": sequence,
                    "monotonic_ns": time.monotonic_ns(),
                    "pid": os.getpid(),
                },
            )
        elif action == "input":
            token = str(command.get("token", input_token))
            received = sys.stdin.buffer.readline(16)
            received_ns = time.monotonic_ns()
            if received != f"{token}\n".encode():
                print(
                    f"unexpected controlled benchmark input: {received!r}",
                    file=sys.stderr,
                )
                return 1
            output = sys.stdout.buffer
            output.write(b"\x1b[2J\x1b[H")
            output.write(f"\x1b[0m{MARKER}\n".encode())
            output.write(visible_marker(default_columns))
            output.flush()
            completed_ns = time.monotonic_ns()
            write_record(
                result_path,
                {
                    "schema": "splinterm.benchmark.child-result.v1",
                    "event": "input_received",
                    "sequence": sequence,
                    "monotonic_ns": received_ns,
                    "write_complete_monotonic_ns": completed_ns,
                    "pid": os.getpid(),
                    "token": token,
                },
            )
        elif action == "exit":
            write_record(
                result_path,
                {
                    "schema": "splinterm.benchmark.child-result.v1",
                    "event": "exit_started",
                    "sequence": sequence,
                    "monotonic_ns": time.monotonic_ns(),
                    "pid": os.getpid(),
                },
            )
            return 0
        elif action == "stop":
            return 0
        else:
            print(f"unsupported benchmark control action: {action!r}", file=sys.stderr)
            return 1
        sequence += 1
    print("benchmark control loop timed out", file=sys.stderr)
    return 1


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Emit a deterministic terminal workload"
    )
    parser.add_argument("case", choices=(*WORKLOADS, "idle", "input", "multiplexer"))
    parser.add_argument("--lines", type=int, default=1000)
    parser.add_argument("--idle-seconds", type=float, default=60.0)
    parser.add_argument("--columns", type=int, default=80)
    parser.add_argument("--ready-file", type=pathlib.Path)
    parser.add_argument("--start-file", type=pathlib.Path)
    parser.add_argument("--start-timeout", type=float, default=30.0)
    parser.add_argument("--done-file", type=pathlib.Path)
    parser.add_argument("--received-file", type=pathlib.Path)
    parser.add_argument("--input-token", default="x")
    parser.add_argument("--hold-seconds", type=float, default=0.0)
    parser.add_argument("--control-dir", type=pathlib.Path)
    args = parser.parse_args()
    if args.lines < 0:
        parser.error("--lines must not be negative")
    if args.columns < 20:
        parser.error("--columns must be at least 20")
    if not 0.1 <= args.idle_seconds <= 3600:
        parser.error("--idle-seconds must be between 0.1 and 3600")
    if args.start_timeout <= 0 or not 0 <= args.hold_seconds <= 3600:
        parser.error("start timeout must be positive and hold seconds must be bounded")

    if len(args.input_token) != 1 or not args.input_token.isascii():
        parser.error("--input-token must be one ASCII character")
    if args.case == "multiplexer" and args.control_dir is None:
        parser.error("--control-dir is required for the multiplexer workload")
    payload = (
        b""
        if args.case in ("idle", "input", "multiplexer")
        else WORKLOADS[args.case](args.lines, args.columns)
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
    if args.case == "multiplexer":
        assert args.control_dir is not None
        return run_controlled(
            args.control_dir,
            args.idle_seconds,
            args.columns,
            args.input_token,
        )
    if args.start_file is not None:
        deadline = time.monotonic() + args.start_timeout
        while not args.start_file.exists():
            if time.monotonic() >= deadline:
                print("benchmark start trigger timed out", file=sys.stderr)
                return 1
            time.sleep(0.002)
    started_ns = time.monotonic_ns()
    if args.case == "input":
        received = sys.stdin.buffer.readline(16)
        received_ns = time.monotonic_ns()
        expected = f"{args.input_token}\n".encode()
        if received != expected:
            print(
                f"unexpected benchmark input: {received!r}, expected {expected!r}",
                file=sys.stderr,
            )
            return 1
        write_record(
            args.received_file,
            {
                "schema": "splinterm.benchmark.child.v1",
                "event": "input_received",
                "monotonic_ns": received_ns,
                "pid": os.getpid(),
                "token": args.input_token,
            },
        )
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
