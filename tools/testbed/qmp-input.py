#!/usr/bin/env python3
"""Bounded QMP keyboard and pointer control for an isolated test VM."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import socket
from typing import Any, Iterable


MAX_TYPED_TEXT_BYTES = 4096
MAX_QMP_LINE_BYTES = 1024 * 1024

SHIFTED = {
    "_": "minus",
    "+": "equal",
    "{": "bracket_left",
    "}": "bracket_right",
    ":": "semicolon",
    '"': "apostrophe",
    "~": "grave_accent",
    "|": "backslash",
    "<": "comma",
    ">": "dot",
    "?": "slash",
    "!": "1",
    "@": "2",
    "#": "3",
    "$": "4",
    "%": "5",
    "^": "6",
    "&": "7",
    "*": "8",
    "(": "9",
    ")": "0",
}
UNSHIFTED = {
    "-": "minus",
    "=": "equal",
    "[": "bracket_left",
    "]": "bracket_right",
    ";": "semicolon",
    "'": "apostrophe",
    "`": "grave_accent",
    "\\": "backslash",
    ",": "comma",
    ".": "dot",
    "/": "slash",
    " ": "spc",
    "\t": "tab",
    "\n": "ret",
}


def key_event(qcode: str, down: bool) -> dict[str, Any]:
    return {
        "type": "key",
        "data": {
            "down": down,
            "key": {"type": "qcode", "data": qcode},
        },
    }


def tap_events(qcode: str, shifted: bool = False) -> list[dict[str, Any]]:
    events: list[dict[str, Any]] = []
    if shifted:
        events.append(key_event("shift", True))
    events.extend((key_event(qcode, True), key_event(qcode, False)))
    if shifted:
        events.append(key_event("shift", False))
    return events


def ascii_events(text: str) -> Iterable[list[dict[str, Any]]]:
    """Encode supported printable ASCII as one bounded key chord per command."""
    if len(text.encode("utf-8")) > MAX_TYPED_TEXT_BYTES:
        raise ValueError(
            f"typed text exceeds {MAX_TYPED_TEXT_BYTES} encoded bytes"
        )
    for character in text:
        if "a" <= character <= "z" or "0" <= character <= "9":
            yield tap_events(character)
        elif "A" <= character <= "Z":
            yield tap_events(character.lower(), shifted=True)
        elif character in UNSHIFTED:
            yield tap_events(UNSHIFTED[character])
        elif character in SHIFTED:
            yield tap_events(SHIFTED[character], shifted=True)
        else:
            raise ValueError(f"unsupported input character: U+{ord(character):04X}")


class QmpClient:
    def __init__(self, path: Path) -> None:
        self.path = path
        self.socket: socket.socket | None = None
        self.stream: Any = None

    def __enter__(self) -> "QmpClient":
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.settimeout(5)
        self.socket.connect(str(self.path))
        self.stream = self.socket.makefile("rwb", buffering=0)
        greeting = self._read_message()
        if "QMP" not in greeting:
            raise RuntimeError("QMP greeting is missing")
        self.execute("qmp_capabilities")
        return self

    def __exit__(self, *_: object) -> None:
        if self.stream is not None:
            self.stream.close()
        if self.socket is not None:
            self.socket.close()

    def _read_message(self) -> dict[str, Any]:
        line = self.stream.readline(MAX_QMP_LINE_BYTES + 1)
        if not line:
            raise RuntimeError("QMP connection closed")
        if len(line) > MAX_QMP_LINE_BYTES:
            raise RuntimeError(
                f"QMP frame exceeds {MAX_QMP_LINE_BYTES} bytes"
            )
        if not line.endswith(b"\n"):
            raise RuntimeError("QMP frame is unterminated")
        return json.loads(line)

    def execute(self, command: str, arguments: dict[str, Any] | None = None) -> Any:
        request: dict[str, Any] = {"execute": command}
        if arguments is not None:
            request["arguments"] = arguments
        self.stream.write((json.dumps(request, separators=(",", ":")) + "\n").encode())
        while True:
            reply = self._read_message()
            if "error" in reply:
                description = reply["error"].get("desc", "unknown QMP error")
                raise RuntimeError(f"{command}: {description}")
            if "return" in reply:
                return reply["return"]

    def send_events(self, events: list[dict[str, Any]]) -> None:
        self.execute("input-send-event", {"events": events})


def absolute_value(position: int, extent: int) -> int:
    if extent <= 1:
        raise ValueError("display extent must be greater than one pixel")
    if position < 0 or position >= extent:
        raise ValueError(f"position {position} is outside 0..{extent - 1}")
    return round(position * 32767 / (extent - 1))


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--socket", required=True, type=Path, help="QMP Unix socket")
    commands = result.add_subparsers(dest="action", required=True)
    commands.add_parser("status", help="report QEMU run state and active pointer device")
    type_parser = commands.add_parser("type", help="type bounded printable ASCII")
    type_parser.add_argument("text")
    type_parser.add_argument("--enter", action="store_true")
    key_parser = commands.add_parser("key", help="tap one QEMU qcode")
    key_parser.add_argument("qcode")
    move_parser = commands.add_parser("move", help="move the absolute guest pointer")
    move_parser.add_argument("x", type=int)
    move_parser.add_argument("y", type=int)
    move_parser.add_argument("width", type=int)
    move_parser.add_argument("height", type=int)
    click_parser = commands.add_parser("click", help="click a guest pointer button")
    click_parser.add_argument("button", choices=("left", "middle", "right"))
    return result


def main() -> int:
    arguments = parser().parse_args()
    typed_events: list[list[dict[str, Any]]] | None = None
    if arguments.action == "type":
        text = arguments.text + ("\n" if arguments.enter else "")
        try:
            typed_events = list(ascii_events(text))
        except ValueError as error:
            raise SystemExit(f"qmp-input: {error}") from error
    if not arguments.socket.is_socket():
        raise SystemExit(f"QMP socket is unavailable: {arguments.socket}")

    with QmpClient(arguments.socket) as client:
        if arguments.action == "status":
            status = client.execute("query-status")
            mice = client.execute("query-mice")
            current = next((mouse for mouse in mice if mouse.get("current")), None)
            print(f"vm: {status['status']}")
            if current is None:
                print("pointer: unavailable")
            else:
                print(
                    f"pointer: {current['name']} "
                    f"({'absolute' if current['absolute'] else 'relative'})"
                )
        elif arguments.action == "type":
            assert typed_events is not None
            for events in typed_events:
                client.send_events(events)
        elif arguments.action == "key":
            client.send_events(tap_events(arguments.qcode))
        elif arguments.action == "move":
            client.send_events(
                [
                    {
                        "type": "abs",
                        "data": {
                            "axis": "x",
                            "value": absolute_value(arguments.x, arguments.width),
                        },
                    },
                    {
                        "type": "abs",
                        "data": {
                            "axis": "y",
                            "value": absolute_value(arguments.y, arguments.height),
                        },
                    },
                ]
            )
        elif arguments.action == "click":
            client.send_events(
                [
                    {"type": "btn", "data": {"button": arguments.button, "down": True}},
                    {"type": "btn", "data": {"button": arguments.button, "down": False}},
                ]
            )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
