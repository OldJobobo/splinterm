#!/usr/bin/env python3
"""Narrow reference client for Splinterm's public JSON/NDJSON CLI contract."""

from __future__ import annotations

import argparse
import json
import os
import selectors
import signal
import subprocess
import sys
import time
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any, NoReturn, Sequence

SCHEMA = "splinterm.cli.v2"
EVENT_SCHEMA = "splinterm.cli.event.v2"
MAX_DOCUMENT_BYTES = 8 * 1024 * 1024
MAX_DIAGNOSTIC_BYTES = 64 * 1024
READ_CHUNK_BYTES = 16 * 1024
ERROR_CODES = {
    "authentication_failed", "handshake_required", "incompatible_version",
    "invalid_request", "unsupported_schema", "consent_unavailable", "consent_denied",
    "unauthorized", "confirmation_required", "controller_unavailable",
    "control_transfer_unavailable", "stale_topology", "not_found", "stale_incarnation",
    "invalid_argument", "resource_limit", "cancelled", "timeout", "internal",
}
TERMINAL_EVENT_TYPES = {"snapshot", "update", "access_revoked", "exited", "resync_required"}
RESYNC_REASONS = {"subscriber_stalled", "revision_gap", "history_replaced"}
CONTEXT_NAMES = (
    "SPLINTERM_LAIR_ID",
    "SPLINTERM_DOJO_ID",
    "SPLINTERM_SPLINT_ID",
    "SPLINTERM_SPLINT_INCARNATION",
)


class ClientError(Exception):
    """A bounded public-contract or selection failure."""

    def __init__(self, message: str, exit_code: int = 5) -> None:
        super().__init__(message[:512])
        self.exit_code = exit_code


@dataclass(frozen=True)
class Context:
    lair_id: str
    dojo_id: str
    splint_id: str
    incarnation: int


def fail(message: str, exit_code: int = 5) -> NoReturn:
    raise ClientError(message, exit_code)


def terminate_process(process: subprocess.Popen[bytes]) -> None:
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    deadline = time.monotonic() + 1
    while time.monotonic() < deadline:
        process.poll()
        try:
            os.killpg(process.pid, 0)
        except ProcessLookupError:
            break
        time.sleep(0.01)
    else:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    if process.poll() is None:
        process.wait()


def run_bounded(argv: Sequence[str], timeout_ms: int) -> tuple[int, bytes, bytes]:
    try:
        process = subprocess.Popen(
            argv,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
    except OSError as error:
        fail(f"cannot execute Splinterm CLI: {error.strerror or error}", 4)
    assert process.stdout is not None and process.stderr is not None
    streams = selectors.DefaultSelector()
    streams.register(process.stdout, selectors.EVENT_READ, ("stdout", MAX_DOCUMENT_BYTES))
    streams.register(process.stderr, selectors.EVENT_READ, ("stderr", MAX_DIAGNOSTIC_BYTES))
    output = {"stdout": bytearray(), "stderr": bytearray()}
    deadline = time.monotonic() + timeout_ms / 1000 + 2
    try:
        while streams.get_map():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                terminate_process(process)
                fail("Splinterm CLI process deadline elapsed", 6)
            ready = streams.select(remaining)
            if not ready:
                continue
            for key, _ in ready:
                name, maximum = key.data
                chunk = os.read(key.fd, READ_CHUNK_BYTES)
                if not chunk:
                    streams.unregister(key.fileobj)
                    continue
                output[name].extend(chunk)
                if len(output[name]) > maximum:
                    terminate_process(process)
                    fail(f"Splinterm {name} exceeds the supported bound", 4)
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            terminate_process(process)
            fail("Splinterm CLI process deadline elapsed", 6)
        process.wait(timeout=remaining)
    except subprocess.TimeoutExpired:
        terminate_process(process)
        fail("Splinterm CLI process deadline elapsed", 6)
    finally:
        streams.close()
    return process.returncode, bytes(output["stdout"]), bytes(output["stderr"])


def valid_decimal_id(value: object) -> bool:
    return isinstance(value, str) and value.isascii() and value.isdigit() and not value.startswith("0")


def parse_json_document(raw: bytes, expected_operation: str) -> dict[str, Any]:
    if len(raw) > MAX_DOCUMENT_BYTES:
        fail("Splinterm JSON document exceeds the supported bound", 4)
    try:
        document = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError):
        fail("Splinterm emitted invalid JSON", 4)
    if not isinstance(document, dict):
        fail("Splinterm JSON envelope is not an object", 4)
    if document.get("schema") != SCHEMA:
        fail("unsupported Splinterm CLI schema", 4)
    if document.get("operation") != expected_operation:
        fail("Splinterm CLI operation did not match the request", 4)
    if not valid_decimal_id(document.get("request_id")):
        fail("Splinterm CLI request ID is invalid", 4)
    if not isinstance(document.get("ok"), bool) or not isinstance(document.get("truncated"), bool):
        fail("Splinterm CLI envelope has an invalid outcome", 4)
    if document["ok"] == ("data" not in document) or document["ok"] == ("error" in document):
        fail("Splinterm CLI envelope has contradictory result fields", 4)
    return document


def public_error(document: dict[str, Any], process_status: int) -> ClientError:
    error = document.get("error")
    if not isinstance(error, dict):
        return ClientError("Splinterm failure omitted its public error", 4)
    code = error.get("code")
    message = error.get("message")
    if (
        code not in ERROR_CODES
        or not isinstance(message, str)
        or len(message) > 4096
        or not isinstance(error.get("retryable"), bool)
    ):
        return ClientError("Splinterm failure has an invalid public error", 4)
    category = process_status if process_status in {3, 4, 5, 6, 70} else 5
    return ClientError(f"{code}: {message}", category)


def valid_uuid(value: object) -> bool:
    if not isinstance(value, str):
        return False
    try:
        return str(uuid.UUID(value)) == value
    except ValueError:
        return False


def validate_topology_data(data: dict[str, Any]) -> None:
    if not all(isinstance(data.get(name), list) for name in ("lairs", "dojos", "splints")):
        fail("topology response has an invalid public shape", 4)
    for lair in data["lairs"]:
        if not (
            isinstance(lair, dict)
            and valid_uuid(lair.get("lair_id"))
            and isinstance(lair.get("name"), str)
            and isinstance(lair.get("dojo_count"), int)
            and lair["dojo_count"] >= 0
        ):
            fail("topology response has an invalid Lair summary", 4)
    for dojo in data["dojos"]:
        if not (
            isinstance(dojo, dict)
            and valid_uuid(dojo.get("lair_id"))
            and valid_uuid(dojo.get("dojo_id"))
            and isinstance(dojo.get("name"), str)
        ):
            fail("topology response has an invalid Dojo summary", 4)
    for splint in data["splints"]:
        current = splint.get("current_incarnation") if isinstance(splint, dict) else None
        last = splint.get("last_incarnation") if isinstance(splint, dict) else None
        lifecycle = splint.get("lifecycle") if isinstance(splint, dict) else None
        valid_last = last is None or (isinstance(last, int) and last > 0)
        if not (
            isinstance(splint, dict)
            and valid_uuid(splint.get("lair_id"))
            and valid_uuid(splint.get("dojo_id"))
            and valid_uuid(splint.get("splint_id"))
            and isinstance(splint.get("title"), str)
            and lifecycle in {"running", "exited", "restorable"}
            and valid_last
            and (
                (
                    lifecycle == "running"
                    and isinstance(current, int)
                    and current > 0
                    and last == current
                )
                or (lifecycle != "running" and current is None)
            )
        ):
            fail("topology response has an invalid Splint summary", 4)


def validate_operation_success(operation: str, document: dict[str, Any]) -> None:
    resource = document.get("resource")
    data = document["data"]
    if operation in {"split_splint", "new_dojo"}:
        if not (
            isinstance(resource, dict)
            and all(valid_uuid(resource.get(name)) for name in ("lair_id", "dojo_id", "splint_id"))
            and isinstance(resource.get("incarnation"), int)
            and resource["incarnation"] > 0
            and isinstance(resource.get("topology_revision"), int)
            and resource["topology_revision"] >= 0
            and data.get("created") is True
        ):
            fail("Splinterm mutation success has an invalid public shape", 4)
    elif operation in {"terminal_snapshot", "input"}:
        if not (
            isinstance(resource, dict)
            and all(valid_uuid(resource.get(name)) for name in ("lair_id", "dojo_id", "splint_id"))
            and isinstance(resource.get("incarnation"), int)
            and resource["incarnation"] > 0
            and isinstance(resource.get("terminal_revision"), int)
            and resource["terminal_revision"] >= 0
            and isinstance(resource.get("history_generation"), int)
            and resource["history_generation"] > 0
        ):
            fail("Splinterm terminal success has invalid provenance", 4)
        if operation == "terminal_snapshot" and not (
            data.get("content_encoding") == "unicode_scalars"
            and isinstance(data.get("rows"), list)
            and isinstance(data.get("cursor"), dict)
        ):
            fail("Splinterm snapshot success has an invalid public shape", 4)
        if operation == "input" and data.get("acknowledged") is not True:
            fail("Splinterm input success was not acknowledged", 4)


def validate_terminal_event(
    event: object,
    expected_sequence: int,
    splint_id: str,
    incarnation: int,
) -> str:
    if not isinstance(event, dict) or event.get("schema") != EVENT_SCHEMA:
        fail("unsupported Splinterm event schema", 4)
    if (
        event.get("subscription_id") != "1"
        or event.get("sequence") != expected_sequence
        or event.get("stream") != "terminal"
        or event.get("event_type") not in TERMINAL_EVENT_TYPES
        or not isinstance(event.get("data"), dict)
        or not isinstance(event.get("truncated"), bool)
    ):
        fail("Splinterm terminal event has an invalid public shape", 4)
    resource = event.get("resource")
    if not (
        isinstance(resource, dict)
        and resource.get("splint_id") == splint_id
        and resource.get("incarnation") == incarnation
    ):
        fail("Splinterm terminal event changed resource identity", 4)
    event_type = event["event_type"]
    data = event["data"]
    terminal_revision = resource.get("terminal_revision")
    if event_type in {"snapshot", "update", "resync_required"} and not (
        isinstance(terminal_revision, int) and terminal_revision >= 0
    ):
        fail("Splinterm terminal event omitted revision provenance", 4)
    if event_type == "snapshot":
        if not (
            data.get("content_encoding") == "unicode_scalars"
            and isinstance(data.get("columns"), int)
            and 1 <= data["columns"] <= 240
            and isinstance(data.get("rows"), int)
            and 1 <= data["rows"] <= 80
            and isinstance(data.get("title"), str)
            and isinstance(data.get("visible_rows"), list)
        ):
            fail("Splinterm snapshot event has an invalid public shape", 4)
    elif event_type == "update":
        if data != {"content_encoding": "unicode_scalars", "changed": True}:
            fail("Splinterm update event has an invalid public shape", 4)
    elif event_type == "access_revoked":
        if not valid_decimal_id(data.get("grant_id")):
            fail("Splinterm revocation event has an invalid public shape", 4)
    elif event_type == "exited":
        if set(data) != {"code", "signal"} or any(
            value is not None and not isinstance(value, int) for value in data.values()
        ):
            fail("Splinterm exit event has an invalid public shape", 4)
    elif event_type == "resync_required":
        resync = event.get("resync")
        if (
            event["truncated"]
            or data
            or not isinstance(resync, dict)
            or set(resync) != {"reason"}
            or resync.get("reason") not in RESYNC_REASONS
            or (
                resync.get("reason") == "history_replaced"
                and not (
                    isinstance(resource.get("history_generation"), int)
                    and resource["history_generation"] > 0
                )
            )
        ):
            fail("Splinterm resync event is incomplete", 4)
    if event_type != "resync_required" and "resync" in event:
        fail("non-resync terminal event contains resync state", 4)
    return event_type


class SplintermClient:
    def __init__(self, executable: str, timeout_ms: int) -> None:
        self.executable = executable
        self.timeout_ms = timeout_ms

    def machine_argv(self, command: Sequence[str], mode: str = "json") -> list[str]:
        return [
            self.executable,
            "--output",
            mode,
            "--schema-major",
            "2",
            "--timeout-ms",
            str(self.timeout_ms),
            *command,
        ]

    def request(self, operation: str, command: Sequence[str]) -> dict[str, Any]:
        status, stdout, _stderr = run_bounded(
            self.machine_argv(command), self.timeout_ms
        )
        document = parse_json_document(stdout, operation)
        if not document["ok"]:
            raise public_error(document, status)
        if status != 0:
            fail("Splinterm reported success with a failing process status", 4)
        data = document.get("data")
        if not isinstance(data, dict):
            fail("Splinterm success omitted its data object", 4)
        validate_operation_success(operation, document)
        return document

    def topology(self) -> dict[str, Any]:
        document = self.request("inspect_topology", ["topology"])
        data = document["data"]
        validate_topology_data(data)
        return document

    def subscribe_until_resync(self, splint_id: str, expected_incarnation: int) -> bool:
        argv = self.machine_argv(
            [
                "subscribe",
                "terminal",
                splint_id,
                "--expected-incarnation",
                str(expected_incarnation),
            ],
            "ndjson",
        )
        try:
            process = subprocess.Popen(
                argv,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                start_new_session=True,
            )
        except OSError as error:
            fail(f"cannot execute Splinterm CLI: {error.strerror or error}", 4)
        assert process.stdout is not None and process.stderr is not None
        streams = selectors.DefaultSelector()
        streams.register(process.stdout, selectors.EVENT_READ, "stdout")
        streams.register(process.stderr, selectors.EVENT_READ, "stderr")
        stdout = bytearray()
        stderr_bytes = 0
        sequence = 1
        saw_event = False
        saw_resync = False
        clean_terminal_end = False
        deadline = time.monotonic() + self.timeout_ms / 1000 + 2
        try:
            while streams.get_map() and not saw_resync and not clean_terminal_end:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    terminate_process(process)
                    fail("Splinterm subscription deadline elapsed", 6)
                ready = streams.select(remaining)
                if not ready:
                    continue
                for key, _ in ready:
                    chunk = os.read(key.fd, READ_CHUNK_BYTES)
                    if not chunk:
                        streams.unregister(key.fileobj)
                        continue
                    if key.data == "stderr":
                        stderr_bytes += len(chunk)
                        if stderr_bytes > MAX_DIAGNOSTIC_BYTES:
                            terminate_process(process)
                            fail("Splinterm stderr exceeds the supported bound", 4)
                        continue
                    stdout.extend(chunk)
                    if len(stdout) > MAX_DOCUMENT_BYTES:
                        terminate_process(process)
                        fail("Splinterm NDJSON record exceeds the supported bound", 4)
                    while b"\n" in stdout:
                        raw_line, _, remainder = stdout.partition(b"\n")
                        stdout = bytearray(remainder)
                        if not raw_line:
                            terminate_process(process)
                            fail("Splinterm emitted an empty NDJSON record", 4)
                        try:
                            event = json.loads(raw_line)
                        except (UnicodeDecodeError, json.JSONDecodeError):
                            terminate_process(process)
                            fail("Splinterm emitted invalid NDJSON", 4)
                        event_type = validate_terminal_event(
                            event, sequence, splint_id, expected_incarnation
                        )
                        sequence += 1
                        saw_event = True
                        if event_type == "resync_required":
                            saw_resync = True
                            break
                        if event_type == "access_revoked":
                            terminate_process(process)
                            fail("access_revoked: terminal subscription authority was revoked", 3)
                        if event_type == "exited":
                            clean_terminal_end = True
                            break
            if stdout:
                terminate_process(process)
                fail("Splinterm ended with a partial NDJSON record", 4)
            if process.poll() is None:
                terminate_process(process)
            else:
                process.wait()
        finally:
            streams.close()
            terminate_process(process)
        if not saw_resync and not clean_terminal_end:
            if process.returncode not in (0, -signal.SIGTERM):
                category = process.returncode if process.returncode in {3, 4, 5, 6, 70} else 4
                fail("Splinterm subscription process failed", category)
            if saw_event:
                fail("Splinterm subscription ended before a terminal event", 4)
            fail("Splinterm subscription ended without a public event", 4)
        return saw_resync


def validated_context(topology: dict[str, Any], environment: dict[str, str]) -> Context:
    missing = [name for name in CONTEXT_NAMES if not environment.get(name)]
    if missing:
        fail("in-Splint context is absent; select an explicit resource")
    try:
        incarnation = int(environment["SPLINTERM_SPLINT_INCARNATION"])
    except ValueError:
        fail("in-Splint incarnation hint is invalid")
    if incarnation <= 0:
        fail("in-Splint incarnation hint is invalid")
    context = Context(
        lair_id=environment["SPLINTERM_LAIR_ID"],
        dojo_id=environment["SPLINTERM_DOJO_ID"],
        splint_id=environment["SPLINTERM_SPLINT_ID"],
        incarnation=incarnation,
    )
    splints = topology["data"]["splints"]
    matching = [
        item
        for item in splints
        if isinstance(item, dict)
        and item.get("lair_id") == context.lair_id
        and item.get("dojo_id") == context.dojo_id
        and item.get("splint_id") == context.splint_id
        and item.get("current_incarnation") == context.incarnation
        and item.get("last_incarnation") == context.incarnation
        and item.get("lifecycle") == "running"
    ]
    if len(matching) != 1:
        fail("in-Splint hints are stale, unauthorized, or absent from topology")
    return context


def find_dojo(topology: dict[str, Any], dojo_id: str) -> tuple[str, str]:
    matching = [
        item
        for item in topology["data"]["dojos"]
        if isinstance(item, dict) and item.get("dojo_id") == dojo_id
    ]
    if len(matching) != 1 or not isinstance(matching[0].get("lair_id"), str):
        fail(f"Dojo not found: {dojo_id}")
    return matching[0]["lair_id"], dojo_id


def child_argv(values: list[str]) -> list[str]:
    values = values[1:] if values[:1] == ["--"] else values
    if not values:
        fail("a direct child argv is required", 2)
    if any("\x00" in value for value in values):
        fail("child argv contains NUL", 2)
    return values


def print_context(context: Context) -> None:
    print(
        json.dumps(
            {
                "lair_id": context.lair_id,
                "dojo_id": context.dojo_id,
                "splint_id": context.splint_id,
                "incarnation": context.incarnation,
            },
            separators=(",", ":"),
        )
    )


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument(
        "--splinterm",
        default=os.environ.get("SPLINTERM_CLI", "splinterm"),
        help="Splinterm CLI executable (default: splinterm)",
    )
    result.add_argument("--timeout-ms", type=int, default=5000)
    commands = result.add_subparsers(dest="command", required=True)
    commands.add_parser("list", help="list authoritative Dojos")
    commands.add_parser("context", help="validate and print the current in-Splint hints")

    open_command = commands.add_parser("open", help="open one existing Dojo in a native window")
    open_command.add_argument("dojo_id", nargs="?")

    start = commands.add_parser("start", help="start a direct argv in a new Dojo")
    start.add_argument("lair_id")
    start.add_argument("--name", default="editor")
    start.add_argument("--cwd", type=Path, default=Path.cwd())

    split = commands.add_parser("split-context", help="split the validated current Splint")
    split.add_argument("--axis", choices=("horizontal", "vertical"), required=True)
    split.add_argument("--side", choices=("first", "second"), default="second")
    split.add_argument("--ratio", type=int, default=500)
    split.add_argument("--cwd", type=Path, default=Path.cwd())

    commands.add_parser("snapshot-context", help="read one bounded current snapshot")
    send = commands.add_parser("send-context", help="send literal text through atomic control")
    send.add_argument("text")
    commands.add_parser(
        "watch-context",
        help="watch current terminal metadata and rebuild state after explicit resync",
    )
    return result


def main(arguments: Sequence[str] | None = None) -> int:
    raw_arguments = list(sys.argv[1:] if arguments is None else arguments)
    separator = raw_arguments.index("--") if "--" in raw_arguments else None
    child_arguments = raw_arguments[separator + 1 :] if separator is not None else []
    picker_arguments = raw_arguments[:separator] if separator is not None else raw_arguments
    options = parser().parse_args(picker_arguments)
    if options.timeout_ms < 1 or options.timeout_ms > 300_000:
        fail("timeout must be between 1 and 300000 milliseconds", 2)
    client = SplintermClient(options.splinterm, options.timeout_ms)

    if options.command == "list":
        topology = client.topology()
        for dojo in topology["data"]["dojos"]:
            print(f"{dojo['dojo_id']}\t{dojo['lair_id']}\t{dojo['name']}")
        return 0

    topology = client.topology()
    if options.command == "context":
        print_context(validated_context(topology, dict(os.environ)))
        return 0
    if options.command == "open":
        if options.dojo_id:
            lair_id, dojo_id = find_dojo(topology, options.dojo_id)
        else:
            context = validated_context(topology, dict(os.environ))
            lair_id, dojo_id = context.lair_id, context.dojo_id
        try:
            os.execvp(
                options.splinterm,
                [
                    options.splinterm,
                    "window",
                    "--lair-id",
                    lair_id,
                    "--dojo-id",
                    dojo_id,
                ],
            )
        except OSError as error:
            fail(f"cannot open Splinterm window: {error.strerror or error}", 4)
    if options.command == "start":
        argv = child_argv(child_arguments)
        document = client.request(
            "new_dojo",
            [
                "new-dojo",
                options.lair_id,
                "--name",
                options.name,
                "--cwd",
                str(options.cwd),
                "--",
                *argv,
            ],
        )
        print(json.dumps(document, separators=(",", ":")))
        return 0

    context = validated_context(topology, dict(os.environ))
    if options.command == "split-context":
        if not 1 <= options.ratio <= 999:
            fail("split ratio must be between 1 and 999", 2)
        argv = child_argv(child_arguments)
        document = client.request(
            "split_splint",
            [
                "split",
                context.splint_id,
                "--axis",
                options.axis,
                "--side",
                options.side,
                "--ratio",
                str(options.ratio),
                "--expected-incarnation",
                str(context.incarnation),
                "--cwd",
                str(options.cwd),
                "--",
                *argv,
            ],
        )
        print(json.dumps(document, separators=(",", ":")))
        return 0
    if options.command == "snapshot-context":
        document = client.request(
            "terminal_snapshot",
            [
                "snapshot",
                context.splint_id,
                "--expected-incarnation",
                str(context.incarnation),
            ],
        )
        print(json.dumps(document, separators=(",", ":")))
        return 0
    if options.command == "send-context":
        document = client.request(
            "input",
            [
                "send",
                context.splint_id,
                options.text,
                "--expected-incarnation",
                str(context.incarnation),
            ],
        )
        print(json.dumps(document, separators=(",", ":")))
        return 0
    if options.command == "watch-context":
        if client.subscribe_until_resync(context.splint_id, context.incarnation):
            # Context is only a hint. Rebuild both location and terminal state from
            # fresh public CLI calls after the subscription declares a gap.
            rebuilt = client.topology()
            refreshed = validated_context(rebuilt, dict(os.environ))
            snapshot = client.request(
                "terminal_snapshot",
                [
                    "snapshot",
                    refreshed.splint_id,
                    "--expected-incarnation",
                    str(refreshed.incarnation),
                ],
            )
            print(
                json.dumps(
                    {
                        "status": "reconciled_after_resync",
                        "context": refreshed.__dict__,
                        "snapshot": snapshot,
                    },
                    separators=(",", ":"),
                )
            )
        return 0
    fail("unsupported command", 2)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ClientError as error:
        print(f"splinterm-dojo-picker: {error}", file=sys.stderr)
        raise SystemExit(error.exit_code) from None
