#!/usr/bin/env python3
"""Typed, allowlisted evidence model for the Plan 0031 MCP demo harness.

Every record serializes only named fields with fixed vocabularies or bounded,
validated values. Nothing here accepts an arbitrary request or response
dictionary, so evidence cannot carry terminal text, controller or transfer
handles, capability frames, policy bodies, argv, or environment values.
"""

from __future__ import annotations

from dataclasses import dataclass, field
import datetime as _datetime
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
from typing import Any

REPORT_SCHEMA = "splinterm.mcp-demo.report.v1"
STATE_SCHEMA = "splinterm.mcp-demo.state.v1"
MANIFEST_SCHEMA = "splinterm.mcp-demo.manifest.v1"
CLEANUP_SCHEMA = "splinterm.mcp-demo.cleanup.v1"
IDENTITIES_SCHEMA = "splinterm.mcp-demo.identities.v1"

RESULT_CODES = (
    "passed",
    "denied_by_user",
    "preflight_failed",
    "consent_ui_failed",
    "workflow_failed",
    "cleanup_incomplete",
    "aborted",
)

# The full Plan 0031 state vocabulary. The preflight lane uses the subset
# created -> daemon_ready -> preflight_passed -> cleaning -> clean: its
# `preflight_passed` follows the daemon/protocol checks rather than preceding
# daemon startup, so a pass is never recorded before the checks complete.
STATES = (
    "created",
    "preflight_passed",
    "daemon_ready",
    "demo_mapped",
    "consent_mapped",
    "consent_granted",
    "consent_denied",
    "controller_acquired",
    "sentinel_verified",
    "matrix_complete",
    "revoked",
    "inspection",
    "cleaning",
    "clean",
)

CHECK_STATUSES = ("passed", "failed", "skipped")
BINARY_ROLES = ("splinterd", "splinterm", "splinterm-mcp")
MODES = ("installed", "staged")
LANES = ("preflight",)

MAX_TEXT = 240
MAX_ITEMS = 256
_IDENTIFIER = re.compile(r"^[a-z][a-z0-9_]{0,63}$")
_TOOL_NAME = re.compile(r"^splinterm\.[a-z][a-z0-9_]{0,63}$")
_HEX = re.compile(r"^[0-9a-f]{1,128}$")
_NONCE = re.compile(r"^[0-9a-f]{16,64}$")


def utc_now() -> str:
    return _datetime.datetime.now(_datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%fZ")


def new_nonce() -> str:
    """A cryptographically random run nonce."""
    return secrets.token_hex(8)


def identifier(value: object, what: str = "identifier") -> str:
    if not isinstance(value, str) or not _IDENTIFIER.match(value):
        raise ValueError(f"invalid {what}: {value!r}")
    return value


def choice(value: object, allowed: tuple[str, ...], what: str) -> str:
    if value not in allowed:
        raise ValueError(f"invalid {what}: {value!r}")
    return str(value)


def bounded_text(value: object) -> str:
    """Harness-authored explanation: printable ASCII, bounded length."""
    if not isinstance(value, str):
        raise ValueError("text must be a string")
    cleaned = "".join(character if 32 <= ord(character) < 127 else "?" for character in value)
    return cleaned[:MAX_TEXT]


def hex_digest(value: object, what: str = "digest") -> str:
    if not isinstance(value, str) or not _HEX.match(value):
        raise ValueError(f"invalid {what}")
    return value


def non_negative(value: object, what: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise ValueError(f"invalid {what}: {value!r}")
    return value


def tool_name(value: object) -> str:
    if not isinstance(value, str) or not _TOOL_NAME.match(value):
        raise ValueError("invalid tool name")
    return value


@dataclass(frozen=True)
class BinaryIdentity:
    role: str
    path: str
    canonical_path: str
    device: int
    inode: int
    size: int
    mode: str
    sha256: str

    def __post_init__(self) -> None:
        choice(self.role, BINARY_ROLES, "binary role")
        for value in (self.path, self.canonical_path):
            if not isinstance(value, str) or not value.startswith("/") or len(value) > 4096:
                raise ValueError("binary paths must be bounded absolute paths")
            if any(ord(character) < 32 for character in value):
                raise ValueError("binary paths must not contain control characters")
        for name in ("device", "inode", "size"):
            non_negative(getattr(self, name), name)
        if not re.match(r"^0[0-7]{3,4}$", self.mode):
            raise ValueError("invalid mode")
        if not re.match(r"^[0-9a-f]{64}$", self.sha256):
            raise ValueError("invalid sha256")

    def to_json(self) -> dict[str, Any]:
        return {
            "role": self.role,
            "path": self.path,
            "canonical_path": self.canonical_path,
            "device": self.device,
            "inode": self.inode,
            "size": self.size,
            "mode": self.mode,
            "sha256": self.sha256,
        }


@dataclass(frozen=True)
class Check:
    name: str
    status: str
    code: str
    detail: str = ""

    def __post_init__(self) -> None:
        identifier(self.name, "check name")
        choice(self.status, CHECK_STATUSES, "check status")
        identifier(self.code, "check code")
        object.__setattr__(self, "detail", bounded_text(self.detail))

    def to_json(self) -> dict[str, Any]:
        return {"name": self.name, "status": self.status, "code": self.code, "detail": self.detail}


@dataclass(frozen=True)
class Transition:
    state: str
    at: str
    result: str
    pids: tuple[tuple[str, int], ...] = ()

    def __post_init__(self) -> None:
        choice(self.state, STATES, "state")
        identifier(self.result, "transition result")
        for label, pid in self.pids:
            identifier(label, "pid label")
            non_negative(pid, "pid")

    def to_json(self) -> dict[str, Any]:
        return {
            "state": self.state,
            "at": self.at,
            "result": self.result,
            "pids": {label: pid for label, pid in self.pids},
        }


@dataclass(frozen=True)
class NegativeCase:
    name: str
    terminal: bool
    status: str
    code: str
    rpc_code: int | None
    returncode: int | None
    stdout_messages: int
    forced: bool

    def __post_init__(self) -> None:
        identifier(self.name, "negative case")
        choice(self.status, CHECK_STATUSES, "case status")
        identifier(self.code, "case code")
        non_negative(self.stdout_messages, "stdout messages")

    def to_json(self) -> dict[str, Any]:
        return {
            "name": self.name,
            "terminal": self.terminal,
            "status": self.status,
            "code": self.code,
            "rpc_code": self.rpc_code,
            "returncode": self.returncode,
            "stdout_messages": self.stdout_messages,
            "forced": self.forced,
        }


@dataclass
class Catalog:
    count: int = 0
    names: list[str] = field(default_factory=list)
    required_missing: list[str] = field(default_factory=list)
    additional: list[str] = field(default_factory=list)
    invalid_names: int = 0

    def to_json(self) -> dict[str, Any]:
        return {
            "count": non_negative(self.count, "catalog count"),
            "names": [tool_name(name) for name in self.names[:MAX_ITEMS]],
            "required_missing": [tool_name(name) for name in self.required_missing],
            "additional": [tool_name(name) for name in self.additional[:MAX_ITEMS]],
            "invalid_names": non_negative(self.invalid_names, "invalid names"),
        }


@dataclass(frozen=True)
class CleanupStep:
    name: str
    status: str
    detail: str = ""

    def __post_init__(self) -> None:
        identifier(self.name, "cleanup step")
        choice(self.status, CHECK_STATUSES, "cleanup status")
        object.__setattr__(self, "detail", bounded_text(self.detail))

    def to_json(self) -> dict[str, Any]:
        return {"name": self.name, "status": self.status, "detail": self.detail}


@dataclass
class Report:
    """One run's report. `result` stays `aborted` until set explicitly."""

    nonce: str
    mode: str
    lane: str = "preflight"
    started_at: str = field(default_factory=utc_now)
    finished_at: str | None = None
    result: str = "aborted"
    state: str = "created"
    transitions: list[Transition] = field(default_factory=list)
    checks: list[Check] = field(default_factory=list)
    negative_cases: list[NegativeCase] = field(default_factory=list)
    identities: list[BinaryIdentity] = field(default_factory=list)
    build: dict[str, Any] = field(default_factory=dict)
    catalog: Catalog = field(default_factory=Catalog)
    cleanup: list[CleanupStep] = field(default_factory=list)
    protocol_version: str | None = None

    def __post_init__(self) -> None:
        if not _NONCE.match(self.nonce):
            raise ValueError("invalid nonce")
        choice(self.mode, MODES, "mode")
        choice(self.lane, LANES, "lane")

    def add_check(self, name: str, passed: bool, code: str, detail: str = "") -> Check:
        check = Check(name, "passed" if passed else "failed", code, detail)
        self.checks.append(check)
        return check

    def skip_check(self, name: str, code: str, detail: str = "") -> Check:
        check = Check(name, "skipped", code, detail)
        self.checks.append(check)
        return check

    def failed_checks(self) -> list[Check]:
        return [check for check in self.checks if check.status == "failed"]

    def transition(self, state: str, result: str = "ok", pids: dict[str, int] | None = None) -> Transition:
        entry = Transition(state, utc_now(), result, tuple(sorted((pids or {}).items())))
        self.transitions.append(entry)
        self.state = state
        return entry

    def cleanup_complete(self) -> bool:
        return bool(self.cleanup) and all(step.status != "failed" for step in self.cleanup)

    def to_json(self) -> dict[str, Any]:
        choice(self.result, RESULT_CODES, "result")
        return {
            "schema": REPORT_SCHEMA,
            "lane": self.lane,
            "mode": self.mode,
            "nonce": self.nonce,
            "started_at": self.started_at,
            "finished_at": self.finished_at,
            "result": self.result,
            "state": self.state,
            "protocol_version": self.protocol_version if self.protocol_version in (None, "2025-11-25") else "<other>",
            "checks": [check.to_json() for check in self.checks],
            "negative_cases": [case.to_json() for case in self.negative_cases],
            "catalog": self.catalog.to_json(),
            "build": sanitize_build(self.build),
            "cleanup_complete": self.cleanup_complete(),
        }

    def state_json(self) -> dict[str, Any]:
        return {
            "schema": STATE_SCHEMA,
            "nonce": self.nonce,
            "state": self.state,
            "transitions": [transition.to_json() for transition in self.transitions],
        }

    def identities_json(self) -> dict[str, Any]:
        return {
            "schema": IDENTITIES_SCHEMA,
            "mode": self.mode,
            "binaries": [identity.to_json() for identity in self.identities],
        }

    def cleanup_json(self) -> dict[str, Any]:
        return {
            "schema": CLEANUP_SCHEMA,
            "nonce": self.nonce,
            "complete": self.cleanup_complete(),
            "steps": [step.to_json() for step in self.cleanup],
        }

    def summary_markdown(self) -> str:
        lines = [
            f"# MCP demo {self.lane}: {self.result}",
            "",
            f"- Nonce: `{self.nonce}`",
            f"- Mode: `{self.mode}`",
            f"- Started: {self.started_at}",
            f"- Finished: {self.finished_at}",
            f"- Final state: `{self.state}`",
            f"- Catalog: {self.catalog.count} tools, {len(self.catalog.required_missing)} required missing",
            f"- Cleanup complete: {self.cleanup_complete()}",
            "",
            "| Check | Status | Code |",
            "|---|---|---|",
        ]
        lines += [f"| {check.name} | {check.status} | {check.code} |" for check in self.checks]
        if self.negative_cases:
            lines += ["", "| Negative case | Terminal | Status | Code |", "|---|---|---|---|"]
            lines += [
                f"| {case.name} | {case.terminal} | {case.status} | {case.code} |"
                for case in self.negative_cases
            ]
        lines += ["", "| Cleanup step | Status |", "|---|---|"]
        lines += [f"| {step.name} | {step.status} |" for step in self.cleanup]
        return "\n".join(lines) + "\n"

    def one_line(self) -> dict[str, Any]:
        failed = self.failed_checks()
        return {
            "result": self.result,
            "nonce": self.nonce,
            "mode": self.mode,
            "checks_passed": sum(1 for check in self.checks if check.status == "passed"),
            "checks_failed": [check.name for check in failed],
            "negative_cases": len(self.negative_cases),
            "catalog_count": self.catalog.count,
            "cleanup_complete": self.cleanup_complete(),
        }


_BUILD_KEYS = {
    "pair_build_identity": "hex",
    "pair_build_version": "version",
    "mcp_server_version": "version",
    "source_commit": "hex",
    "source_dirty": "bool",
    "source_patch_sha256": "hex",
    "source_identity": "identifier",
    "package_record": "identifier",
    "package_build_version": "version",
    "rustc_version": "version",
}
_VERSION = re.compile(r"^[0-9A-Za-z.+\-]{1,64}$")


def sanitize_build(build: dict[str, Any]) -> dict[str, Any]:
    """Keep only allowlisted build-identity facts in fixed formats."""
    result: dict[str, Any] = {}
    for key, kind in _BUILD_KEYS.items():
        if key not in build or build[key] is None:
            continue
        value = build[key]
        if kind == "hex":
            result[key] = hex_digest(value, key)
        elif kind == "bool":
            if not isinstance(value, bool):
                raise ValueError(f"{key} must be boolean")
            result[key] = value
        elif kind == "identifier":
            result[key] = identifier(value, key)
        elif kind == "version":
            if not isinstance(value, str) or not _VERSION.match(value):
                raise ValueError(f"invalid {key}")
            result[key] = value
    return result


def write_private_json(path: Path, value: object) -> None:
    """Atomically write mode-0600 JSON next to `path` and replace it."""
    write_private_bytes(path, (json.dumps(value, indent=2, sort_keys=True) + "\n").encode())


def write_private_bytes(path: Path, payload: bytes) -> None:
    temporary = path.with_name(f".{path.name}.{secrets.token_hex(4)}.tmp")
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    except BaseException:
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass
        raise


NOT_APPLICABLE_TO_PREFLIGHT = ("hyprland-before.json", "hyprland-after.json", "screenshots/")


class Evidence:
    """Writes the Plan 0031 evidence layout, or keeps it in memory only.

    With no base directory nothing is written: the caller prints the summary
    and the in-memory record is discarded, so a default run leaves no files.
    """

    FILES = (
        "manifest.json",
        "state.json",
        "report.json",
        "summary.md",
        "identities.json",
        "mcp-events.jsonl",
        "cleanup.json",
    )

    def __init__(self, base: Path | None, report: Report) -> None:
        self.report = report
        self.directory: Path | None = None
        self.events_written = 0
        self._events_stream = None
        if base is None:
            return
        base = Path(base)
        if base.is_symlink() or not base.is_dir():
            raise ValueError("evidence base must be an existing, non-symlink directory")
        stamp = _datetime.datetime.now(_datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        directory = base / f"mcp-demo-{stamp}-{report.nonce}"
        os.mkdir(directory, 0o700)
        os.chmod(directory, 0o700)
        self.directory = directory
        descriptor = os.open(
            directory / "mcp-events.jsonl",
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_APPEND,
            0o600,
        )
        self._events_stream = os.fdopen(descriptor, "a", encoding="utf-8")
        self._write_manifest(complete=False)
        self.persist_state()

    def event(self, event: dict[str, Any]) -> None:
        """Append one already-allowlisted host event (see mcp_host._event)."""
        self.events_written += 1
        if self._events_stream is None:
            return
        allowed = {
            key: event[key]
            for key in ("t_ms", "pid", "direction", "kind", "method", "id", "outcome", "rpc_code", "tool", "tool_error_code", "bytes", "session")
            if key in event
        }
        self._events_stream.write(json.dumps(allowed, sort_keys=True) + "\n")
        self._events_stream.flush()

    def persist_state(self) -> None:
        if self.directory is not None:
            write_private_json(self.directory / "state.json", self.report.state_json())

    def _write_manifest(self, complete: bool) -> None:
        assert self.directory is not None
        write_private_json(self.directory / "manifest.json", {
            "schema": MANIFEST_SCHEMA,
            "lane": self.report.lane,
            "nonce": self.report.nonce,
            "mode": self.report.mode,
            "created_at": self.report.started_at,
            "complete": complete,
            "files": list(self.FILES) + ["SHA256SUMS"],
            "not_applicable": list(NOT_APPLICABLE_TO_PREFLIGHT),
        })

    def finalize(self) -> None:
        if self.directory is None:
            return
        if self._events_stream is not None:
            self._events_stream.close()
            self._events_stream = None
        write_private_json(self.directory / "report.json", self.report.to_json())
        write_private_json(self.directory / "identities.json", self.report.identities_json())
        write_private_json(self.directory / "cleanup.json", self.report.cleanup_json())
        write_private_bytes(self.directory / "summary.md", self.report.summary_markdown().encode())
        self.persist_state()
        self._write_manifest(complete=True)
        sums = []
        for name in self.FILES:
            digest = hashlib.sha256((self.directory / name).read_bytes()).hexdigest()
            sums.append(f"{digest}  {name}\n")
        write_private_bytes(self.directory / "SHA256SUMS", "".join(sums).encode())

    def close(self) -> None:
        if self._events_stream is not None:
            self._events_stream.close()
            self._events_stream = None
