#!/usr/bin/env python3
"""Bounded MCP stdio host for one exact `splinterm-mcp` child (Plan 0031).

The host owns one child process. It speaks newline-delimited JSON-RPC 2.0,
issues monotonic integer request IDs, matches responses by ID while retaining
notifications, enforces per-request and per-write deadlines, drains stdout and
stderr concurrently with fixed bounds, and cleans up only its own child.

Raw protocol data stays in memory. The optional event sink receives only
allowlisted metadata (direction, kind, method, ID, outcome codes, byte counts),
never request or response bodies, so callers can persist events without
retaining terminal text, handles, or capability material.

Standard library only.
"""

from __future__ import annotations

from collections import deque
from dataclasses import dataclass
import json
import os
import re
import select
import signal
import subprocess
import threading
import time
from typing import Any, Callable, Iterable, Mapping

PROTOCOL_VERSION = "2025-11-25"
DEFAULT_MAX_LINE_BYTES = 16 * 1024 * 1024
DEFAULT_STDERR_LIMIT = 64 * 1024
DEFAULT_MAX_NOTIFICATIONS = 256
DEFAULT_TIMEOUT = 15.0

HOST_ERROR_CODES = frozenset({
    "timeout",
    "eof",
    "malformed_output",
    "oversized_output",
    "invalid_envelope",
    "protocol",
    "rpc_error",
    "write_timeout",
    "write_failed",
    "cancelled",
    "lifecycle",
    "notification_overflow",
    "closed",
})

_SAFE_TOKEN = re.compile(r"^[A-Za-z0-9_./\-]{1,96}$")


class HostError(Exception):
    """A host-side failure with one fixed, recordable code."""

    def __init__(self, code: str, message: str, *, rpc_code: int | None = None):
        if code not in HOST_ERROR_CODES:
            raise ValueError(f"unknown host error code {code!r}")
        super().__init__(f"{code}: {message}")
        self.code = code
        self.rpc_code = rpc_code


@dataclass(frozen=True)
class ExitRecord:
    """How the child ended. `forced` is true when the host had to signal it."""

    returncode: int | None
    forced: bool
    signal_sent: str | None
    stderr_bytes: int
    stderr_truncated: bool
    stdout_trailing_bytes: int
    pipes_closed: bool

    @property
    def clean(self) -> bool:
        return self.returncode == 0 and not self.forced


@dataclass(frozen=True)
class ToolOutcome:
    """A validated `tools/call` result. `structured` stays in memory only."""

    is_error: bool
    structured: dict[str, Any] | None
    error_code: str | None


def safe_token(value: object) -> str | None:
    """Return `value` only when it is a short, fixed-alphabet token."""
    if isinstance(value, str) and _SAFE_TOKEN.match(value):
        return value
    return None


def _reject_constant(value: str) -> None:
    raise ValueError(f"non-finite JSON constant {value}")


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON object key")
        result[key] = value
    return result


def decode_line(line: bytes) -> Any:
    """Strictly decode one frame: UTF-8, finite, no duplicate keys."""
    text = line.decode("utf-8")
    return json.loads(text, parse_constant=_reject_constant, object_pairs_hook=_unique_object)


def _valid_id(value: object) -> bool:
    return (isinstance(value, int) and not isinstance(value, bool)) or isinstance(value, str)


class McpHost:
    """One owned `splinterm-mcp` stdio session."""

    def __init__(
        self,
        argv: list[str],
        env: Mapping[str, str],
        *,
        cwd: str | os.PathLike[str] | None = None,
        max_line_bytes: int = DEFAULT_MAX_LINE_BYTES,
        stderr_limit: int = DEFAULT_STDERR_LIMIT,
        max_notifications: int = DEFAULT_MAX_NOTIFICATIONS,
        event_sink: Callable[[dict[str, Any]], None] | None = None,
        start_new_session: bool = True,
    ) -> None:
        self._max_line_bytes = max_line_bytes
        self._stderr_limit = stderr_limit
        self._max_notifications = max_notifications
        self._event_sink = event_sink
        self._started = time.monotonic()
        self._condition = threading.Condition()
        self._write_lock = threading.Lock()
        self._next_id = 1
        self._pending: dict[int | str, str] = {}
        self._responses: dict[int | str, dict[str, Any]] = {}
        self._completed: set[int | str] = set()
        self._abandoned: set[int | str] = set()
        self._notifications: deque[dict[str, Any]] = deque()
        self._notifications_dropped = 0
        self._late_responses = 0
        self._failure: tuple[str, str] | None = None
        self._eof = False
        self._stdout_messages = 0
        self._trailing_bytes = 0
        self._stderr = bytearray()
        self._stderr_total = 0
        self._state = "new"
        self._exit: ExitRecord | None = None
        self._process = subprocess.Popen(
            [str(part) for part in argv],
            env=dict(env),
            cwd=cwd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            bufsize=0,
            close_fds=True,
            start_new_session=start_new_session,
        )
        assert self._process.stdin is not None
        assert self._process.stdout is not None and self._process.stderr is not None
        os.set_blocking(self._process.stdin.fileno(), False)
        self._stdout_thread = threading.Thread(target=self._read_stdout, daemon=True)
        self._stderr_thread = threading.Thread(target=self._read_stderr, daemon=True)
        self._stdout_thread.start()
        self._stderr_thread.start()
        self._event("lifecycle", "spawn", None)

    # ----------------------------------------------------------------- props

    @property
    def pid(self) -> int:
        return self._process.pid

    @property
    def state(self) -> str:
        return self._state

    @property
    def stdout_messages(self) -> int:
        with self._condition:
            return self._stdout_messages

    @property
    def late_responses(self) -> int:
        with self._condition:
            return self._late_responses

    @property
    def failure(self) -> str | None:
        with self._condition:
            return self._failure[0] if self._failure else None

    @property
    def stderr_bytes(self) -> int:
        with self._condition:
            return self._stderr_total

    def stderr_snapshot(self) -> bytes:
        """Bounded stderr capture, for in-memory diagnosis only."""
        with self._condition:
            return bytes(self._stderr)

    # ---------------------------------------------------------------- events

    def _event(
        self,
        direction: str,
        kind: str,
        method: str | None,
        *,
        request_id: object = None,
        outcome: str | None = None,
        rpc_code: int | None = None,
        tool: str | None = None,
        tool_error_code: str | None = None,
        size: int | None = None,
    ) -> None:
        if self._event_sink is None:
            return
        event: dict[str, Any] = {
            "t_ms": int((time.monotonic() - self._started) * 1000),
            "pid": self._process.pid if hasattr(self, "_process") else None,
            "direction": direction,
            "kind": kind,
        }
        if method is not None:
            event["method"] = safe_token(method) or "<redacted>"
        if isinstance(request_id, int) and not isinstance(request_id, bool):
            event["id"] = request_id
        elif request_id is not None:
            event["id"] = "<non-integer>"
        if outcome is not None:
            event["outcome"] = outcome
        if isinstance(rpc_code, int) and not isinstance(rpc_code, bool):
            event["rpc_code"] = rpc_code
        if tool is not None:
            event["tool"] = safe_token(tool) or "<redacted>"
        if tool_error_code is not None:
            event["tool_error_code"] = safe_token(tool_error_code) or "<redacted>"
        if size is not None:
            event["bytes"] = size
        try:
            self._event_sink(event)
        except Exception:  # noqa: BLE001 - evidence must never break cleanup
            pass

    # --------------------------------------------------------------- readers

    def _fail_locked(self, code: str, message: str) -> None:
        if self._failure is None:
            self._failure = (code, message)
            self._condition.notify_all()

    def _read_stdout(self) -> None:
        assert self._process.stdout is not None
        fd = self._process.stdout.fileno()
        buffer = bytearray()
        while True:
            try:
                chunk = os.read(fd, 64 * 1024)
            except OSError:
                chunk = b""
            if not chunk:
                with self._condition:
                    self._eof = True
                    self._trailing_bytes = len(buffer)
                    self._condition.notify_all()
                return
            with self._condition:
                failed = self._failure is not None
            if failed:
                # Keep draining so the child never blocks on a full pipe, but
                # do not interpret anything after a framing failure.
                buffer.clear()
                continue
            buffer.extend(chunk)
            while True:
                index = buffer.find(b"\n")
                if index < 0:
                    if len(buffer) > self._max_line_bytes:
                        with self._condition:
                            self._fail_locked("oversized_output", "stdout line exceeds the host limit")
                        buffer.clear()
                    break
                line = bytes(buffer[:index])
                del buffer[: index + 1]
                if len(line) > self._max_line_bytes:
                    with self._condition:
                        self._fail_locked("oversized_output", "stdout line exceeds the host limit")
                    buffer.clear()
                    break
                self._accept_line(line)
                with self._condition:
                    if self._failure is not None:
                        buffer.clear()
                        break

    def _read_stderr(self) -> None:
        assert self._process.stderr is not None
        fd = self._process.stderr.fileno()
        while True:
            try:
                chunk = os.read(fd, 64 * 1024)
            except OSError:
                chunk = b""
            if not chunk:
                return
            with self._condition:
                self._stderr_total += len(chunk)
                room = self._stderr_limit - len(self._stderr)
                if room > 0:
                    self._stderr.extend(chunk[:room])

    def _accept_line(self, line: bytes) -> None:
        try:
            message = decode_line(line)
        except (UnicodeDecodeError, ValueError):
            with self._condition:
                self._fail_locked("malformed_output", "stdout frame is not strict UTF-8 JSON")
            self._event("recv", "malformed", None, size=len(line))
            return
        if not isinstance(message, dict) or message.get("jsonrpc") != "2.0":
            with self._condition:
                self._fail_locked("invalid_envelope", "stdout frame is not a JSON-RPC 2.0 object")
            self._event("recv", "invalid", None, size=len(line))
            return
        if "method" in message:
            method = message.get("method")
            params = message.get("params", {})
            if not isinstance(method, str) or not isinstance(params, (dict, list)):
                with self._condition:
                    self._fail_locked("invalid_envelope", "server message has an invalid method")
                return
            if "id" in message:
                # This host advertises no client capabilities, so a server
                # request is a protocol violation, not a prompt to act.
                with self._condition:
                    self._fail_locked("protocol", "server sent an unexpected request")
                self._event("recv", "server_request", method, size=len(line))
                return
            with self._condition:
                self._stdout_messages += 1
                if len(self._notifications) >= self._max_notifications:
                    self._notifications_dropped += 1
                else:
                    self._notifications.append(message)
                self._condition.notify_all()
            self._event("recv", "notification", method, size=len(line))
            return
        request_id = message.get("id")
        has_result = "result" in message
        has_error = "error" in message
        error = message.get("error")
        if (
            not _valid_id(request_id)
            or has_result == has_error
            or (
                has_error
                and not (
                    isinstance(error, dict)
                    and isinstance(error.get("code"), int)
                    and not isinstance(error.get("code"), bool)
                    and isinstance(error.get("message"), str)
                )
            )
        ):
            with self._condition:
                self._fail_locked("invalid_envelope", "response envelope is invalid")
            self._event("recv", "invalid", None, size=len(line))
            return
        assert request_id is not None
        with self._condition:
            self._stdout_messages += 1
            if request_id in self._pending:
                method = self._pending.pop(request_id)
                self._responses[request_id] = message
                self._completed.add(request_id)
                self._condition.notify_all()
            elif request_id in self._abandoned:
                self._abandoned.discard(request_id)
                self._late_responses += 1
                method = None
            elif request_id in self._completed:
                self._fail_locked("protocol", "duplicate response ID")
                method = None
            else:
                self._fail_locked("protocol", "response for an unknown request ID")
                method = None
        rpc_code = error.get("code") if isinstance(error, dict) else None
        self._event(
            "recv",
            "response",
            method,
            request_id=request_id,
            outcome="error" if has_error else "ok",
            rpc_code=rpc_code,
            size=len(line),
        )

    # ---------------------------------------------------------------- writes

    def send_bytes(self, payload: bytes, timeout: float = DEFAULT_TIMEOUT) -> None:
        """Write raw bytes with a deadline (used for protocol-negative cases)."""
        if self._exit is not None or self._process.stdin is None or self._process.stdin.closed:
            raise HostError("closed", "stdin is closed")
        fd = self._process.stdin.fileno()
        deadline = time.monotonic() + timeout
        view = memoryview(payload)
        with self._write_lock:
            while view:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise HostError("write_timeout", "stdin write exceeded its deadline")
                try:
                    _, writable, _ = select.select([], [fd], [], remaining)
                except (OSError, ValueError) as error:
                    raise HostError("write_failed", "stdin is unavailable") from error
                if not writable:
                    continue
                try:
                    written = os.write(fd, view)
                except BlockingIOError:
                    continue
                except OSError as error:
                    raise HostError("write_failed", "stdin write failed") from error
                view = view[written:]

    def _send_message(self, message: dict[str, Any], timeout: float) -> int:
        payload = (json.dumps(message, separators=(",", ":"), allow_nan=False) + "\n").encode()
        if len(payload) > self._max_line_bytes:
            raise HostError("oversized_output", "outbound frame exceeds the host limit")
        self.send_bytes(payload, timeout)
        return len(payload)

    def notify(self, method: str, params: dict[str, Any] | None = None, timeout: float = DEFAULT_TIMEOUT) -> None:
        message: dict[str, Any] = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            message["params"] = params
        size = self._send_message(message, timeout)
        self._event("send", "notification", method, size=size)

    def send_request(
        self,
        method: str,
        params: dict[str, Any] | None = None,
        *,
        enforce_lifecycle: bool = True,
        timeout: float = DEFAULT_TIMEOUT,
        tool: str | None = None,
    ) -> int:
        """Send one request and return its ID without waiting."""
        if enforce_lifecycle:
            if method == "initialize":
                raise HostError("lifecycle", "use initialize() for the handshake")
            if self._state != "initialized":
                raise HostError("lifecycle", f"cannot send {method} while {self._state}")
        with self._condition:
            request_id = self._next_id
            self._next_id += 1
            self._pending[request_id] = method
        message: dict[str, Any] = {"jsonrpc": "2.0", "id": request_id, "method": method}
        if params is not None:
            message["params"] = params
        try:
            size = self._send_message(message, timeout)
        except HostError:
            with self._condition:
                self._pending.pop(request_id, None)
            raise
        self._event("send", "request", method, request_id=request_id, tool=tool, size=size)
        return request_id

    # ----------------------------------------------------------------- waits

    def wait_response(self, request_id: int, timeout: float = DEFAULT_TIMEOUT) -> dict[str, Any]:
        """Wait for one exact response. Raises on timeout, EOF, or failure."""
        deadline = time.monotonic() + timeout
        with self._condition:
            while True:
                if request_id in self._responses:
                    return self._responses.pop(request_id)
                if request_id not in self._pending:
                    if request_id in self._abandoned:
                        raise HostError("cancelled", "request was cancelled or abandoned")
                    raise HostError("lifecycle", "no such outstanding request")
                if self._failure is not None:
                    raise HostError(self._failure[0], self._failure[1])
                if self._eof:
                    raise HostError("eof", "server closed stdout before responding")
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    # A late reply must never satisfy a different request.
                    self._pending.pop(request_id, None)
                    self._abandoned.add(request_id)
                    raise HostError("timeout", "response deadline expired")
                self._condition.wait(remaining)

    def request(
        self,
        method: str,
        params: dict[str, Any] | None = None,
        *,
        enforce_lifecycle: bool = True,
        timeout: float = DEFAULT_TIMEOUT,
    ) -> dict[str, Any]:
        request_id = self.send_request(method, params, enforce_lifecycle=enforce_lifecycle, timeout=timeout)
        return self.wait_response(request_id, timeout)

    def cancel(self, request_id: int, reason: str = "cancelled by host") -> None:
        """Cancel one outstanding request. A late reply is discarded."""
        with self._condition:
            if request_id not in self._pending:
                raise HostError("lifecycle", "no such outstanding request")
            self._pending.pop(request_id)
            self._abandoned.add(request_id)
            self._condition.notify_all()
        self.notify("notifications/cancelled", {"requestId": request_id, "reason": reason})

    def wait_for_output(self, timeout: float) -> bool:
        """Wait until stdout reaches EOF or fails. True when that happened."""
        deadline = time.monotonic() + timeout
        with self._condition:
            while not self._eof and self._failure is None:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    return False
                self._condition.wait(remaining)
            return True

    def take_notifications(
        self, predicate: Callable[[dict[str, Any]], bool] | None = None
    ) -> list[dict[str, Any]]:
        """Remove and return retained notifications matching `predicate`."""
        with self._condition:
            if self._notifications_dropped:
                raise HostError(
                    "notification_overflow",
                    f"{self._notifications_dropped} notifications exceeded the retention bound",
                )
            taken = [item for item in self._notifications if predicate is None or predicate(item)]
            kept = [item for item in self._notifications if not (predicate is None or predicate(item))]
            self._notifications = deque(kept)
            return taken

    # ------------------------------------------------------------- lifecycle

    def initialize(
        self,
        *,
        protocol_version: str = PROTOCOL_VERSION,
        capabilities: dict[str, Any] | None = None,
        client_name: str = "splinterm-mcp-demo-harness",
        client_version: str = "1",
        send_initialized: bool = True,
        timeout: float = DEFAULT_TIMEOUT,
    ) -> dict[str, Any]:
        """Initialize exactly `protocol_version`, then (optionally) confirm it."""
        if self._state != "new":
            raise HostError("lifecycle", f"initialize is not allowed while {self._state}")
        self._state = "initializing"
        request_id = self.send_request(
            "initialize",
            {
                "protocolVersion": protocol_version,
                "capabilities": {} if capabilities is None else capabilities,
                "clientInfo": {"name": client_name, "version": client_version},
            },
            enforce_lifecycle=False,
            timeout=timeout,
        )
        response = self.wait_response(request_id, timeout)
        if "error" in response:
            self._state = "rejected"
            raise HostError("rpc_error", "initialize was rejected", rpc_code=response["error"]["code"])
        result = response.get("result")
        if not isinstance(result, dict) or result.get("protocolVersion") != protocol_version:
            self._state = "rejected"
            raise HostError("protocol", "initialize did not echo the requested protocol version")
        self._state = "initialize_accepted"
        if send_initialized:
            self.send_initialized(timeout=timeout)
        return result

    def send_initialized(self, timeout: float = DEFAULT_TIMEOUT) -> None:
        if self._state != "initialize_accepted":
            raise HostError("lifecycle", f"notifications/initialized is not allowed while {self._state}")
        self.notify("notifications/initialized", timeout=timeout)
        self._state = "initialized"

    def call_tool(
        self, name: str, arguments: dict[str, Any], timeout: float = DEFAULT_TIMEOUT
    ) -> ToolOutcome:
        request_id = self.send_request(
            "tools/call", {"name": name, "arguments": arguments}, timeout=timeout, tool=name
        )
        response = self.wait_response(request_id, timeout)
        if "error" in response:
            raise HostError("rpc_error", "tools/call returned a JSON-RPC error", rpc_code=response["error"]["code"])
        outcome = parse_tool_result(response.get("result"))
        self._event(
            "recv",
            "tool_result",
            "tools/call",
            request_id=request_id,
            outcome="error" if outcome.is_error else "ok",
            tool=name,
            tool_error_code=outcome.error_code,
        )
        return outcome

    def wait_exit(self, timeout: float) -> int | None:
        """Wait for the child to exit on its own, without closing stdin."""
        try:
            return self._process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            return None

    def close_stdin(self) -> None:
        stdin = self._process.stdin
        if stdin is not None and not stdin.closed:
            with self._write_lock:
                try:
                    stdin.close()
                except OSError:
                    pass

    def close(self, timeout: float = DEFAULT_TIMEOUT, grace: float = 3.0) -> ExitRecord:
        """Close stdin, await bounded exit, escalate only against this child."""
        if self._exit is not None:
            return self._exit
        self.close_stdin()
        forced = False
        signal_sent: str | None = None
        try:
            self._process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            forced = True
            for signum, label, wait in (
                (signal.SIGTERM, "SIGTERM", grace),
                (signal.SIGKILL, "SIGKILL", grace),
            ):
                # Popen.send_signal targets only this unreaped child PID.
                self._process.send_signal(signum)
                signal_sent = label
                try:
                    self._process.wait(timeout=wait)
                    break
                except subprocess.TimeoutExpired:
                    continue
            if self._process.poll() is None:
                self._process.wait()
        self._stdout_thread.join(timeout=5)
        self._stderr_thread.join(timeout=5)
        pipes_closed = not self._stdout_thread.is_alive() and not self._stderr_thread.is_alive()
        for stream in (self._process.stdout, self._process.stderr):
            if stream is not None:
                try:
                    stream.close()
                except OSError:
                    pass
        with self._condition:
            self._exit = ExitRecord(
                returncode=self._process.returncode,
                forced=forced,
                signal_sent=signal_sent,
                stderr_bytes=self._stderr_total,
                stderr_truncated=self._stderr_total > len(self._stderr),
                stdout_trailing_bytes=self._trailing_bytes,
                pipes_closed=pipes_closed,
            )
            self._state = "closed"
        self._event("lifecycle", "exit", None, outcome="forced" if forced else "exited")
        return self._exit

    def __enter__(self) -> "McpHost":
        return self

    def __exit__(self, *_: object) -> None:
        self.close()


def parse_tool_result(result: object) -> ToolOutcome:
    """Validate one `tools/call` result envelope."""
    if not isinstance(result, dict):
        raise HostError("invalid_envelope", "tool result is not an object")
    is_error = result.get("isError", False)
    if not isinstance(is_error, bool):
        raise HostError("invalid_envelope", "tool result isError is not boolean")
    content = result.get("content", [])
    if not isinstance(content, list):
        raise HostError("invalid_envelope", "tool result content is not a list")
    structured = result.get("structuredContent")
    if structured is None:
        for item in content:
            if isinstance(item, dict) and item.get("type") == "text" and isinstance(item.get("text"), str):
                try:
                    decoded = decode_line(item["text"].encode())
                except (UnicodeDecodeError, ValueError):
                    continue
                if isinstance(decoded, dict):
                    structured = decoded
                    break
    if structured is not None and not isinstance(structured, dict):
        raise HostError("invalid_envelope", "structuredContent is not an object")
    error_code = None
    if is_error and isinstance(structured, dict):
        error = structured.get("error")
        if isinstance(error, dict):
            error_code = safe_token(error.get("code"))
    return ToolOutcome(is_error=is_error, structured=structured, error_code=error_code)


def tool_names(tools: Iterable[object]) -> list[str]:
    """Names from a `tools/list` result, rejecting malformed entries."""
    names = []
    for tool in tools:
        if not isinstance(tool, dict) or not isinstance(tool.get("name"), str):
            raise HostError("invalid_envelope", "tools/list entry has no name")
        names.append(tool["name"])
    return names
