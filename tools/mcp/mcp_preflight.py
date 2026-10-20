#!/usr/bin/env python3
"""Non-graphical MCP preflight for the Plan 0031 demo harness (Milestone 1).

The preflight performs no Wayland, compositor, or window action. It selects one
coherent `splinterm`/`splinterd`/`splinterm-mcp` set (installed or staged),
records exact identities, starts one isolated daemon with private HOME/XDG
directories, a private socket, and no automation policy (deny-all), then:

- initializes MCP `2025-11-25` exactly and confirms with
  `notifications/initialized`;
- records the discovered catalog and validates the 11 required tools by name
  and wire schema without pinning the total count;
- proves `splinterm.ping` succeeds and `splinterm.inspect_topology` fails
  closed with the structured `unauthorized` code;
- runs each protocol-negative case in a fresh adapter process; and
- cleans up only processes, sockets, and directories it created, then attests
  the result.

It never writes or reloads policy, never calls mutation tools (including
`splinterm.report_agent_status`), and never enables dev-attach.
"""

from __future__ import annotations

from dataclasses import dataclass, field
import contextlib
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import signal
import socket
import stat
import subprocess
import tempfile
import threading
import time
from typing import Any, Callable

from mcp_host import PROTOCOL_VERSION, HostError, McpHost, ToolOutcome, tool_names
from mcp_report import (
    BinaryIdentity,
    CleanupStep,
    Evidence,
    NegativeCase,
    Report,
    new_nonce,
    utc_now,
)

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_SCHEMA_ROOT = REPO_ROOT / "dist" / "schemas" / "mcp" / "v2"
VALIDATE_PACKAGE = REPO_ROOT / "tools" / "package" / "validate-package.py"
PACKAGE_RECORD = Path("share/splinterm/package-provenance-v1.json")
COMMON_PREFIX = "https://splinterm.oldjobobo.com/schemas/mcp/v2/common.schema.json#/$defs/"

REQUIRED_TOOLS = (
    "splinterm.ping",
    "splinterm.inspect_topology",
    "splinterm.request_lair_access",
    "splinterm.acquire_control",
    "splinterm.input",
    "splinterm.resize",
    "splinterm.read_terminal",
    "splinterm.release_control",
    "splinterm.revoke_access",
    "splinterm.split_splint",
    "splinterm.rename_splint",
)
READ_ONLY_REQUIRED = ("splinterm.ping", "splinterm.inspect_topology", "splinterm.read_terminal")
# The only tools the preflight may call. Both are observations; everything
# else (including report_agent_status, a mutation) is discovery-only here.
CALLABLE_TOOLS = frozenset({"splinterm.ping", "splinterm.inspect_topology"})
ERROR_ENVELOPE_KEYS = frozenset({"schema", "tool", "ok", "error", "truncated", "content_trust"})
ROLE_LABELS = {"splinterd": "daemon", "splinterm": "client", "splinterm-mcp": "mcp"}
SEALED_ROLES = {"splinterd": 1, "splinterm": 2}

MCP_MAXIMUM_INBOUND_LINE = 256 * 1024
REQUEST_TIMEOUT = 15.0
DAEMON_READY_TIMEOUT = 30.0
DAEMON_STOP_TIMEOUT = 30.0
EXIT_TIMEOUT = 10.0
MAX_SOCKET_PATH = 100
SOCKET_NAME = "splinterd.sock"
CHILD_PATH = "/usr/local/bin:/usr/bin:/bin"
_VERSION_TEXT = re.compile(r"^[0-9A-Za-z.+\-]{1,64}$")
_CODE_SUFFIX = re.compile(r"^[a-z][a-z0-9_]{0,40}$")


def recordable_version(value: object) -> str | None:
    """A version string fit for evidence, or None when it is not one."""
    if isinstance(value, str) and _VERSION_TEXT.match(value):
        return value
    return None


def code_suffix(value: object, fallback: str) -> str:
    """An identifier suffix for a check code; untrusted text maps to `other`."""
    if value is None:
        return fallback
    if isinstance(value, str) and _CODE_SUFFIX.match(value):
        return value
    return "other"


class PreflightAbort(Exception):
    """Stops the forward path; cleanup still runs."""

    def __init__(self, code: str) -> None:
        super().__init__(code)
        self.code = code


class HarnessInterrupted(BaseException):
    """Raised from a signal handler so ordered cleanup can still run."""


# --------------------------------------------------------------- selection


@dataclass(frozen=True)
class Selection:
    mode: str
    bin_dir: Path
    paths: dict[str, Path]


def select_binaries(mode: str, bin_dir: Path) -> Selection:
    """Name the three sibling executables of one installed or staged set."""
    if mode not in ("installed", "staged"):
        raise ValueError("mode must be installed or staged")
    bin_dir = Path(bin_dir).absolute()
    if mode == "staged":
        canonical = Path(os.path.realpath(bin_dir))
        if canonical == Path("/usr/bin") or str(canonical).startswith("/nix/store/"):
            # Staged mode never claims installed-package validation.
            raise PreflightAbort("staged_dir_is_installed_prefix")
    return Selection(mode, bin_dir, {role: bin_dir / role for role in ROLE_LABELS})


def inspect_binary(role: str, path: Path) -> BinaryIdentity:
    """Hash one executable through a single descriptor and pin its identity."""
    try:
        os.lstat(path)
    except FileNotFoundError as error:
        raise PreflightAbort("binary_missing") from error
    canonical = os.path.realpath(path)
    try:
        descriptor = os.open(canonical, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
    except OSError as error:
        raise PreflightAbort("binary_unreadable") from error
    try:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode):
            raise PreflightAbort("binary_not_regular")
        if before.st_mode & 0o111 == 0:
            raise PreflightAbort("binary_not_executable")
        if before.st_mode & 0o002:
            raise PreflightAbort("binary_world_writable")
        if before.st_uid not in (0, os.geteuid()):
            raise PreflightAbort("binary_foreign_owner")
        digest = hashlib.sha256()
        while block := os.read(descriptor, 1024 * 1024):
            digest.update(block)
        after = os.fstat(descriptor)
    finally:
        os.close(descriptor)
    if (before.st_size, before.st_mtime_ns, before.st_ino) != (after.st_size, after.st_mtime_ns, after.st_ino):
        raise PreflightAbort("binary_changed_during_inspection")
    current = os.stat(canonical)
    if (current.st_dev, current.st_ino) != (before.st_dev, before.st_ino):
        raise PreflightAbort("binary_replaced_during_inspection")
    return BinaryIdentity(
        role=role,
        path=str(path),
        canonical_path=canonical,
        device=before.st_dev,
        inode=before.st_ino,
        size=before.st_size,
        mode=oct(stat.S_IMODE(before.st_mode)).replace("0o", "0", 1),
        sha256=digest.hexdigest(),
    )


def identity_still_matches(identity: BinaryIdentity) -> bool:
    """Re-inspect the file: same device, inode, size, and digest."""
    try:
        current = inspect_binary(identity.role, Path(identity.canonical_path))
    except PreflightAbort:
        return False
    return (current.device, current.inode, current.size, current.sha256) == (
        identity.device, identity.inode, identity.size, identity.sha256
    )


def path_shadowing(name: str, identity: BinaryIdentity, path_env: str | None) -> str:
    """`ok`, `absent`, or `shadowed` for the first PATH resolution of `name`."""
    found = shutil.which(name, path=path_env)
    if found is None:
        return "absent"
    try:
        current = os.stat(found)
    except OSError:
        return "shadowed"
    if (current.st_dev, current.st_ino) == (identity.device, identity.inode):
        return "ok"
    return "shadowed"


def sealed_report(binary: Path, role: int) -> bytes:
    """The binary's own sealed build-pair report (shared with package checks)."""
    spec = importlib.util.spec_from_file_location("splinterm_validate_package", VALIDATE_PACKAGE)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.sealed_preflight_report(binary, role)


def process_executable(pid: int) -> tuple[int, int] | None:
    """(device, inode) of a live process's executable, if readable."""
    try:
        current = os.stat(f"/proc/{pid}/exe")
    except OSError:
        return None
    return (current.st_dev, current.st_ino)


def source_identity(source_root: Path | None) -> dict[str, Any]:
    """Git commit, dirty flag, and patch digest of the harness source tree."""
    if source_root is not None and (Path(source_root) / ".git").exists():
        def git(*arguments: str) -> bytes:
            return subprocess.run(
                ["git", "-C", str(source_root), *arguments],
                check=True, capture_output=True, timeout=30,
            ).stdout
        try:
            commit = git("rev-parse", "HEAD").decode().strip()
            # Tracked changes only: untracked files (build logs, evidence)
            # are not in the patch digest, so they do not mark the tree dirty.
            dirty = bool(git("status", "--porcelain", "--untracked-files=no").strip())
            result: dict[str, Any] = {
                "source_identity": "git",
                "source_commit": commit,
                "source_dirty": dirty,
            }
            if dirty:
                result["source_patch_sha256"] = hashlib.sha256(git("diff", "HEAD", "--binary")).hexdigest()
            return result
        except (OSError, subprocess.SubprocessError, UnicodeDecodeError):
            pass
    commit = os.environ.get("SPLINTERM_SOURCE_COMMIT", "")
    if len(commit) == 40 and all(character in "0123456789abcdef" for character in commit):
        return {"source_identity": "environment_unverified", "source_commit": commit}
    return {"source_identity": "unavailable"}


# ------------------------------------------------------------ wire schemas


def _localize(value: Any) -> Any:
    if isinstance(value, str) and value.startswith(COMMON_PREFIX):
        return "#/$defs/" + value[len(COMMON_PREFIX):]
    if isinstance(value, list):
        return [_localize(item) for item in value]
    if isinstance(value, dict):
        return {key: _localize(item) for key, item in value.items()}
    return value


def _load_json(path: Path) -> Any:
    with open(path, "rb") as stream:
        payload = stream.read(1024 * 1024 + 1)
    if len(payload) > 1024 * 1024:
        raise ValueError("schema exceeds 1 MiB")
    return json.loads(payload)


def wire_input_schema(schema_root: Path, stem: str) -> dict[str, Any]:
    """The discovery form of a tool input schema (mirrors tools.rs)."""
    value = _load_json(schema_root / "tools" / f"{stem}.input.schema.json")
    value["type"] = "object"
    value["$defs"] = _load_json(schema_root / "common.schema.json")["$defs"]
    return _localize(value)


def wire_output_schema(schema_root: Path, stem: str) -> dict[str, Any]:
    """The discovery form of a tool output schema: success/error `oneOf`."""
    success = _load_json(schema_root / "tools" / f"{stem}.output.schema.json")
    success["type"] = "object"
    success["$defs"] = _load_json(schema_root / "common.schema.json")["$defs"]
    success = _localize(success)
    for key in ("$schema", "$id", "type"):
        success.pop(key, None)
    definitions = success.pop("$defs")
    failure = _load_json(schema_root / "error.schema.json")
    failure.pop("$schema", None)
    failure.pop("$id", None)
    return {"type": "object", "oneOf": [success, _localize(failure)], "$defs": definitions}


@dataclass
class CatalogEvaluation:
    count: int
    names: list[str]
    duplicates: list[str]
    missing: list[str]
    additional: list[str]
    schema_mismatches: list[str]
    contract_mismatches: list[str]
    schemas_compared: bool


def evaluate_catalog(tools: list[Any], schema_root: Path | None) -> CatalogEvaluation:
    """Require the 11 tools and their schemas; accept reviewed additions."""
    names = tool_names(tools)
    seen: set[str] = set()
    duplicates = sorted({name for name in names if name in seen or seen.add(name)})
    by_name = {tool["name"]: tool for tool in tools}
    missing = [name for name in REQUIRED_TOOLS if name not in by_name]
    additional = sorted(name for name in by_name if name not in REQUIRED_TOOLS)
    schema_mismatches: list[str] = []
    contract_mismatches: list[str] = []
    compare = schema_root is not None and (Path(schema_root) / "common.schema.json").is_file()
    for name in REQUIRED_TOOLS:
        tool = by_name.get(name)
        if tool is None:
            continue
        annotations = tool.get("annotations") if isinstance(tool.get("annotations"), dict) else {}
        if tool.get("execution") != {"taskSupport": "forbidden"} or annotations.get("openWorldHint") is not False:
            contract_mismatches.append(name)
        elif name in READ_ONLY_REQUIRED and annotations.get("readOnlyHint") is not True:
            contract_mismatches.append(name)
        elif name == "splinterm.revoke_access" and annotations.get("destructiveHint") is not True:
            contract_mismatches.append(name)
        if not isinstance(tool.get("inputSchema"), dict) or not isinstance(tool.get("outputSchema"), dict):
            schema_mismatches.append(name)
            continue
        if compare:
            assert schema_root is not None
            stem = name.split(".", 1)[1]
            try:
                expected_input = wire_input_schema(Path(schema_root), stem)
                expected_output = wire_output_schema(Path(schema_root), stem)
            except (OSError, ValueError, KeyError):
                schema_mismatches.append(name)
                continue
            if tool["inputSchema"] != expected_input or tool["outputSchema"] != expected_output:
                schema_mismatches.append(name)
    return CatalogEvaluation(
        count=len(names),
        names=names,
        duplicates=duplicates,
        missing=missing,
        additional=additional,
        schema_mismatches=schema_mismatches,
        contract_mismatches=contract_mismatches,
        schemas_compared=compare,
    )


def classify_ping(outcome: ToolOutcome) -> str:
    structured = outcome.structured or {}
    data = structured.get("data") if isinstance(structured.get("data"), dict) else {}
    if (
        not outcome.is_error
        and structured.get("tool") == "splinterm.ping"
        and structured.get("content_trust") == "trusted_metadata"
        and data.get("protocol_version") == PROTOCOL_VERSION
    ):
        return "pong"
    if outcome.is_error:
        return "error_" + code_suffix(outcome.error_code, "unstructured")
    return "unexpected_success_shape"


def classify_deny(outcome: ToolOutcome, tool: str) -> str:
    """`unauthorized` only for a structured, data-free authorization denial."""
    structured = outcome.structured
    if not outcome.is_error:
        return "not_denied"
    if not isinstance(structured, dict) or set(structured) != ERROR_ENVELOPE_KEYS:
        return "unstructured_error"
    if structured.get("ok") is not False or structured.get("tool") != tool:
        return "unstructured_error"
    if outcome.error_code != "unauthorized":
        return "wrong_error_" + code_suffix(outcome.error_code, "unknown")
    return "unauthorized"


# --------------------------------------------------------------- isolation


@dataclass
class PrivateRoot:
    """A nonce-named mode-0700 directory owned by this run."""

    path: Path
    device: int
    inode: int
    run_device: int = -1
    run_inode: int = -1

    @classmethod
    def create(cls, base: Path, nonce: str) -> "PrivateRoot":
        path = Path(base) / f"splinterm-mcp-preflight-{nonce}"
        os.mkdir(path, 0o700)  # exclusive: an existing path is never adopted
        os.chmod(path, 0o700)
        info = os.lstat(path)
        root = cls(path, info.st_dev, info.st_ino)
        try:
            for child in ("home", "config", "state", "cache", "data", "run", "tmp"):
                os.mkdir(path / child, 0o700)
            run = os.lstat(path / "run")
            root.run_device, root.run_inode = run.st_dev, run.st_ino
        except BaseException:
            root.remove()
            raise
        return root

    @property
    def socket(self) -> Path:
        return self.path / "run" / SOCKET_NAME

    def owned(self) -> bool:
        try:
            info = os.lstat(self.path)
        except FileNotFoundError:
            return False
        return (
            stat.S_ISDIR(info.st_mode)
            and (info.st_dev, info.st_ino) == (self.device, self.inode)
            and info.st_uid == os.geteuid()
        )

    def _run_directory(self) -> int | None:
        """A descriptor for the verified `run` directory, or None."""
        if not self.owned():
            return None
        try:
            descriptor = os.open(self.path / "run", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC)
        except OSError:
            return None
        info = os.fstat(descriptor)
        if (info.st_dev, info.st_ino) != (self.run_device, self.run_inode) or info.st_uid != os.geteuid():
            os.close(descriptor)
            return None
        return descriptor

    def socket_stat(self) -> os.stat_result | None:
        """lstat of the socket entry inside the verified run directory."""
        descriptor = self._run_directory()
        if descriptor is None:
            return None
        try:
            return os.stat(SOCKET_NAME, dir_fd=descriptor, follow_symlinks=False)
        except OSError:
            return None
        finally:
            os.close(descriptor)

    def remove_stale_socket(self) -> bool:
        """Unlink a leftover socket by name relative to the verified run dir."""
        descriptor = self._run_directory()
        if descriptor is None:
            return False
        try:
            info = os.stat(SOCKET_NAME, dir_fd=descriptor, follow_symlinks=False)
            if not stat.S_ISSOCK(info.st_mode):
                return False
            os.unlink(SOCKET_NAME, dir_fd=descriptor)
            return True
        except OSError:
            return False
        finally:
            os.close(descriptor)

    def remove(self) -> str:
        """`removed`, `absent`, or a refusal code. Never follows symlinks."""
        if not self.path.exists() and not self.path.is_symlink():
            return "absent"
        if not self.owned():
            return "refused_not_owned"
        try:
            shutil.rmtree(self.path)
        except OSError:
            return "remove_failed"
        return "removed" if not os.path.lexists(self.path) else "remove_failed"


def isolated_environment(root: PrivateRoot) -> dict[str, str]:
    """A from-scratch child environment: nothing ambient is inherited.

    No SPLINTERM_POLICY (the daemon starts deny-all), no Wayland/X11/D-Bus
    session, no Splint discovery hints, and no SPLINTERM_ENABLE_DEV_ATTACH.
    """
    return {
        "PATH": CHILD_PATH,
        "LANG": "C.UTF-8",
        "HOME": str(root.path / "home"),
        "XDG_CONFIG_HOME": str(root.path / "config"),
        "XDG_STATE_HOME": str(root.path / "state"),
        "XDG_CACHE_HOME": str(root.path / "cache"),
        "XDG_DATA_HOME": str(root.path / "data"),
        "XDG_RUNTIME_DIR": str(root.path / "run"),
        "TMPDIR": str(root.path / "tmp"),
        "SPLINTERM_SOCKET": str(root.socket),
        "SPLINTERM_MCP_TIMEOUT_MS": "10000",
    }


# ---------------------------------------------------------- process facts


def _proc_stat(pid: int) -> tuple[str, int, int] | None:
    """(state, ppid, starttime) from /proc, or None when gone."""
    try:
        data = Path(f"/proc/{pid}/stat").read_bytes()
    except OSError:
        return None
    fields = data[data.rindex(b")") + 2:].split()
    return fields[0].decode(), int(fields[1]), int(fields[19])


def process_alive(pid: int, start: int) -> bool:
    facts = _proc_stat(pid)
    return facts is not None and facts[2] == start and facts[0] != "Z"


def kill_exact(pid: int, start: int) -> bool:
    """SIGKILL one exact (pid, starttime) without a PID-reuse race.

    The pidfd pins the process; the start time is re-checked after it is
    opened, so a recycled PID is never signalled.
    """
    try:
        descriptor = os.pidfd_open(pid)
    except OSError:
        return False
    try:
        if not process_alive(pid, start):
            return False
        signal.pidfd_send_signal(descriptor, signal.SIGKILL)
        return True
    except OSError:
        return False
    finally:
        os.close(descriptor)


def descendants(root_pid: int) -> list[tuple[int, int]]:
    """Exact (pid, starttime) of every live descendant of `root_pid`."""
    children: dict[int, list[tuple[int, int]]] = {}
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        facts = _proc_stat(int(entry))
        if facts is None or facts[0] == "Z":
            continue
        children.setdefault(facts[1], []).append((int(entry), facts[2]))
    result: list[tuple[int, int]] = []
    queue = [root_pid]
    while queue:
        for child in children.get(queue.pop(), []):
            result.append(child)
            queue.append(child[0])
    return result


def processes_referencing(root: Path) -> list[int]:
    """Own processes whose environment, argv, or cwd name the private root."""
    needle = os.fsencode(str(root))
    me = os.getpid()
    found = []
    for entry in os.listdir("/proc"):
        if not entry.isdigit() or int(entry) == me:
            continue
        base = Path("/proc") / entry
        try:
            if base.stat().st_uid != os.geteuid():
                continue
            facts = _proc_stat(int(entry))
            if facts is None or facts[0] == "Z":
                continue
            hit = needle in (base / "environ").read_bytes() or needle in (base / "cmdline").read_bytes()
            if not hit:
                try:
                    hit = os.readlink(base / "cwd").startswith(str(root))
                except OSError:
                    hit = False
        except OSError:
            continue
        if hit:
            found.append(int(entry))
    return found


def _fingerprint(path: Path, digest: bool) -> tuple[Any, ...] | None:
    try:
        info = os.lstat(path)
    except OSError:
        return None
    facts: tuple[Any, ...] = (info.st_dev, info.st_ino, stat.S_IFMT(info.st_mode), info.st_uid)
    if digest and stat.S_ISREG(info.st_mode):
        facts += (info.st_size, info.st_mtime_ns)
        if info.st_size <= 16 * 1024 * 1024:
            try:
                facts += (hashlib.sha256(path.read_bytes()).hexdigest(),)
            except OSError:
                facts += ("unreadable",)
    return facts


def production_fingerprints(environment: dict[str, str]) -> dict[str, tuple[Any, ...] | None]:
    """Identity of the user's normal daemon socket, topology, and policy.

    Read-only: nothing here connects to the production socket or parses the
    files. Only equality is reported, never paths or contents.
    """
    home = environment.get("HOME")
    paths: dict[str, tuple[Path, bool]] = {}
    if environment.get("SPLINTERM_SOCKET"):
        paths["socket"] = (Path(environment["SPLINTERM_SOCKET"]), False)
    elif environment.get("XDG_RUNTIME_DIR"):
        paths["socket"] = (Path(environment["XDG_RUNTIME_DIR"]) / "splinterm" / "splinterd.sock", False)
    state = environment.get("XDG_STATE_HOME") or (f"{home}/.local/state" if home else None)
    if state:
        paths["topology"] = (Path(state) / "splinterm" / "topology.json", True)
        paths["topology_previous"] = (Path(state) / "splinterm" / "topology.json.previous", True)
    config = environment.get("XDG_CONFIG_HOME") or (f"{home}/.config" if home else None)
    if config:
        paths["policy_default"] = (Path(config) / "splinterm" / "policy.json", True)
    if environment.get("SPLINTERM_POLICY"):
        paths["policy_configured"] = (Path(environment["SPLINTERM_POLICY"]), True)
    return {label: _fingerprint(path, digest) for label, (path, digest) in paths.items()}


# --------------------------------------------------------------- preflight


@dataclass
class Hooks:
    """Seams for subprocess tests; production uses the real implementations."""

    sealed_report: Callable[[Path, int], bytes] | None = None
    process_executable: Callable[[int], tuple[int, int] | None] | None = None
    after_daemon_ready: Callable[[], None] | None = None


@dataclass
class Options:
    evidence_base: Path | None = None
    schema_root: Path | None = DEFAULT_SCHEMA_ROOT
    source_root: Path | None = REPO_ROOT
    temporary_base: Path | None = None
    request_timeout: float = REQUEST_TIMEOUT
    daemon_ready_timeout: float = DAEMON_READY_TIMEOUT
    daemon_stop_timeout: float = DAEMON_STOP_TIMEOUT
    exit_timeout: float = EXIT_TIMEOUT
    # The environment that locates the user's normal daemon socket, topology,
    # and policy for the unchanged-state check; None means this process's own.
    production_environment: dict[str, str] | None = None
    hooks: Hooks = field(default_factory=Hooks)


class Preflight:
    def __init__(self, selection: Selection, options: Options | None = None) -> None:
        self.selection = selection
        self.options = options or Options()
        self.report = Report(nonce=new_nonce(), mode=selection.mode)
        self.evidence = Evidence(self.options.evidence_base, self.report)
        self.identities: dict[str, BinaryIdentity] = {}
        self.pair_version: str | None = None
        self.root: PrivateRoot | None = None
        self.daemon: subprocess.Popen[bytes] | None = None
        self.hosts: list[McpHost] = []
        self.descendants: set[tuple[int, int]] = set()
        self.owned_pids: dict[str, int] = {}
        self.accepting = True
        self.interrupted = False
        self.evidence_error: str | None = None

    # --------------------------------------------------------- driver

    def run(self) -> Report:
        report = self.report
        self.evidence.persist_state()
        production_before = production_fingerprints(self._production_environment())
        try:
            self._identity_phase()
            self._isolate()
            self._start_daemon()
            self._main_session()
            self._negative_cases()
            self._recheck_identities()
            if not report.failed_checks():
                report.transition("preflight_passed", "ok", self.owned_pids)
                self.evidence.persist_state()
        except PreflightAbort as abort:
            if not report.failed_checks():
                report.add_check("preflight_abort", False, abort.code)
        except (KeyboardInterrupt, HarnessInterrupted):
            self.interrupted = True
        except Exception as error:  # noqa: BLE001 - cleanup must still run
            report.add_check("harness_internal", False, "unexpected_exception", type(error).__name__)
        finally:
            # Ignore further signals before cleanup starts. An interrupt that
            # lands while the deferral is being installed is caught and the
            # deferral retried, so it can never skip cleanup.
            while True:
                try:
                    previous = _defer_signals()
                    break
                except (KeyboardInterrupt, HarnessInterrupted):
                    self.interrupted = True
            try:
                self._cleanup(production_before)
            finally:
                _restore_signals(previous)
        return report

    def _process_executable(self, pid: int) -> tuple[int, int] | None:
        return (self.options.hooks.process_executable or process_executable)(pid)

    def _production_environment(self) -> dict[str, str]:
        if self.options.production_environment is not None:
            return dict(self.options.production_environment)
        return dict(os.environ)

    def _abort_if_failed(self, code: str) -> None:
        if self.report.failed_checks():
            raise PreflightAbort(code)

    # --------------------------------------------------------- identity

    def _identity_phase(self) -> None:
        report = self.report
        for role in ROLE_LABELS:
            label = ROLE_LABELS[role]
            try:
                identity = inspect_binary(role, self.selection.paths[role])
            except PreflightAbort as abort:
                report.add_check(f"{label}_binary", False, abort.code)
                continue
            self.identities[role] = identity
            report.identities.append(identity)
            report.add_check(f"{label}_binary", True, "regular_executable")
        self._abort_if_failed("binary_selection_failed")

        parents = {str(Path(identity.canonical_path).parent) for identity in self.identities.values()}
        report.add_check("binary_adjacency", len(parents) == 1, "siblings" if len(parents) == 1 else "not_siblings")

        try:
            report_of = self.options.hooks.sealed_report or sealed_report
            daemon_report = report_of(
                Path(self.identities["splinterd"].canonical_path), SEALED_ROLES["splinterd"]
            )
            client_report = report_of(
                Path(self.identities["splinterm"].canonical_path), SEALED_ROLES["splinterm"]
            )
        except Exception:  # noqa: BLE001 - any failure is a coherence failure
            report.add_check("build_pair_coherent", False, "sealed_report_failed")
        else:
            coherent = len(daemon_report) == 98 and daemon_report[44:] == client_report[44:]
            report.add_check("build_pair_coherent", coherent, "same_pair" if coherent else "pair_mismatch")
            if coherent:
                version = ".".join(str(int.from_bytes(daemon_report[offset:offset + 2], "big")) for offset in (76, 78, 80))
                self.pair_version = version
                report.build["pair_build_identity"] = daemon_report[44:76].hex()
                report.build["pair_build_version"] = version

        if self.selection.mode == "installed":
            self._installed_checks()
        else:
            report.build.update(source_identity(self.options.source_root))
        self._abort_if_failed("identity_failed")

    def _installed_checks(self) -> None:
        report = self.report
        path_env = os.environ.get("PATH")
        verdicts = {role: path_shadowing(role, self.identities[role], path_env) for role in ROLE_LABELS}
        shadowed = [ROLE_LABELS[role] for role, verdict in verdicts.items() if verdict == "shadowed"]
        client_ok = verdicts["splinterm"] == "ok"
        report.add_check(
            "path_resolution",
            not shadowed and client_ok,
            "selected_set_on_path" if not shadowed and client_ok else ("shadowed" if shadowed else "client_not_on_path"),
            ("shadowed: " + ",".join(shadowed)) if shadowed else "",
        )
        record_path = self.selection.bin_dir.parent / PACKAGE_RECORD
        if not record_path.is_file():
            report.build["package_record"] = "absent"
            report.skip_check("package_record", "record_absent", "dirty-tree installs carry no record")
            return
        report.build["package_record"] = "present"
        try:
            record = json.loads(record_path.read_bytes()[: 64 * 1024])
            matches = (
                record.get("daemon_sha256") == self.identities["splinterd"].sha256
                and record.get("client_sha256") == self.identities["splinterm"].sha256
                and record.get("pair_build_identity") == report.build.get("pair_build_identity")
                and str(record.get("build_version", "")).split("-", 1)[0] == self.pair_version
            )
            version = recordable_version(record.get("build_version"))
            if version is not None:
                report.build["package_build_version"] = version
        except (OSError, ValueError, AttributeError):
            matches = False
        report.add_check("package_record", matches, "record_matches" if matches else "record_mismatch")

    def _recheck_identities(self) -> None:
        stable = all(identity_still_matches(identity) for identity in self.identities.values())
        self.report.add_check("binary_identity_stable", stable, "unchanged" if stable else "replaced_during_run")

    # --------------------------------------------------------- daemon

    def _isolate(self) -> None:
        base = Path(self.options.temporary_base or tempfile.gettempdir())
        self.root = PrivateRoot.create(base, self.report.nonce)
        socket_ok = len(os.fsencode(str(self.root.socket))) <= MAX_SOCKET_PATH
        self.report.add_check("private_root", socket_ok, "created_0700" if socket_ok else "socket_path_too_long")
        self._abort_if_failed("isolation_failed")

    def _start_daemon(self) -> None:
        assert self.root is not None
        identity = self.identities["splinterd"]
        environment = isolated_environment(self.root)
        # Daemon output is discarded: nothing reads it, and an uncapped file
        # could grow without bound. M2 diagnostics use Plan 0030 instead.
        self.daemon = subprocess.Popen(
            [identity.canonical_path],
            env=environment,
            cwd=self.root.path / "home",
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            close_fds=True,
            start_new_session=True,
        )
        self.owned_pids["daemon"] = self.daemon.pid
        deadline = time.monotonic() + self.options.daemon_ready_timeout
        ready = False
        while time.monotonic() < deadline:
            if self.daemon.poll() is not None:
                self.report.add_check("daemon_ready", False, "daemon_exited_early")
                raise PreflightAbort("daemon_exited_early")
            if self._socket_accepts(self.root.socket):
                ready = True
                break
            time.sleep(0.05)
        if not ready:
            self.report.add_check("daemon_ready", False, "readiness_timeout")
            raise PreflightAbort("readiness_timeout")
        self.report.add_check("daemon_ready", True, "private_socket_accepts")
        executable = self._process_executable(self.daemon.pid)
        matches = executable == (identity.device, identity.inode)
        self.report.add_check("daemon_executable_identity", matches, "matches_selection" if matches else "identity_mismatch")
        info = self.root.socket_stat()
        private = info is not None and stat.S_ISSOCK(info.st_mode) and info.st_uid == os.geteuid()
        self.report.add_check("private_socket", private, "owned_socket" if private else "foreign_socket")
        self._abort_if_failed("daemon_identity_failed")
        self._remember_descendants()
        self.report.transition("daemon_ready", "ok", self.owned_pids)
        self.evidence.persist_state()
        if self.options.hooks.after_daemon_ready is not None:
            self.options.hooks.after_daemon_ready()

    @staticmethod
    def _socket_accepts(path: Path) -> bool:
        try:
            if not stat.S_ISSOCK(os.lstat(path).st_mode):
                return False
        except OSError:
            return False
        probe = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM | socket.SOCK_CLOEXEC)
        probe.settimeout(1)
        try:
            probe.connect(str(path))
            return True
        except OSError:
            return False
        finally:
            probe.close()

    def _remember_descendants(self) -> None:
        if self.daemon is not None and self.daemon.poll() is None:
            self.descendants.update(descendants(self.daemon.pid))

    # --------------------------------------------------------- adapter

    def _spawn_adapter(self, session: str) -> McpHost:
        if not self.accepting:
            raise PreflightAbort("not_accepting_actions")
        assert self.root is not None
        host = McpHost(
            [self.identities["splinterm-mcp"].canonical_path],
            isolated_environment(self.root),
            cwd=self.root.path / "home",
            event_sink=lambda event: self.evidence.event({**event, "session": session}),
        )
        self.hosts.append(host)
        return host

    def _call(self, host: McpHost, name: str) -> ToolOutcome:
        if name not in CALLABLE_TOOLS:
            raise PreflightAbort("tool_not_permitted_in_preflight")
        return host.call_tool(name, {}, timeout=self.options.request_timeout)

    def _main_session(self) -> None:
        report = self.report
        timeout = self.options.request_timeout
        host = self._spawn_adapter("main")
        identity = self.identities["splinterm-mcp"]
        executable = self._process_executable(host.pid)
        matches = executable == (identity.device, identity.inode)
        report.add_check("mcp_executable_identity", matches, "matches_selection" if matches else "identity_mismatch")
        self._abort_if_failed("mcp_identity_failed")
        try:
            info = host.initialize(timeout=timeout)
        except HostError as error:
            report.add_check("initialize", False, error.code)
            raise PreflightAbort("initialize_failed") from error
        report.protocol_version = info.get("protocolVersion")
        report.add_check("initialize", True, "negotiated_2025_11_25")
        capabilities = info.get("capabilities")
        caps_ok = (
            isinstance(capabilities, dict)
            and isinstance(capabilities.get("tools"), dict)
            and isinstance(capabilities.get("resources"), dict)
            and capabilities["resources"].get("subscribe") is True
        )
        report.add_check("server_capabilities", caps_ok, "tools_and_subscribable_resources" if caps_ok else "unexpected_capabilities")
        server = info.get("serverInfo") if isinstance(info.get("serverInfo"), dict) else {}
        version = recordable_version(server.get("version"))
        if version is not None:
            report.build["mcp_server_version"] = version
        name_ok = server.get("name") == "splinterm-mcp"
        coherent = (
            name_ok
            and version is not None
            and (self.pair_version is None or version.split("-", 1)[0] == self.pair_version)
        )
        report.add_check(
            "mcp_version_coherent",
            coherent,
            "matches_pair_version" if coherent else ("version_skew" if version is not None else "version_unrecordable"),
        )

        try:
            listed = host.request("tools/list", {}, timeout=timeout)
            result = listed.get("result")
            if not isinstance(result, dict) or not isinstance(result.get("tools"), list) or "nextCursor" in result:
                raise HostError("invalid_envelope", "tools/list result is not one complete page")
            evaluation = evaluate_catalog(result["tools"], self.options.schema_root)
        except HostError as error:
            report.add_check("catalog", False, error.code)
            raise PreflightAbort("catalog_failed") from error
        report.catalog.count = evaluation.count
        valid = [name for name in evaluation.names if _is_tool_name(name)]
        report.catalog.invalid_names = len(evaluation.names) - len(valid)
        report.catalog.names = valid
        report.catalog.required_missing = evaluation.missing
        report.catalog.additional = [name for name in evaluation.additional if _is_tool_name(name)]
        report.add_check("catalog_unique", not evaluation.duplicates and not report.catalog.invalid_names,
                         "unique_names" if not evaluation.duplicates else "duplicate_names")
        report.add_check("required_tools_present", not evaluation.missing,
                         "all_11_present" if not evaluation.missing else "required_missing",
                         ",".join(evaluation.missing))
        report.add_check("required_tool_contracts", not evaluation.contract_mismatches,
                         "annotations_match" if not evaluation.contract_mismatches else "annotation_mismatch",
                         ",".join(evaluation.contract_mismatches))
        if evaluation.schemas_compared:
            report.add_check("required_tool_schemas", not evaluation.schema_mismatches,
                             "wire_schemas_match" if not evaluation.schema_mismatches else "schema_mismatch",
                             ",".join(evaluation.schema_mismatches))
        elif self.options.schema_root is not None:
            report.add_check("required_tool_schemas", False, "schema_root_unusable",
                             "the schema root has no common.schema.json")
        else:
            report.skip_check("required_tool_schemas", "no_schema_root")

        try:
            resources = host.request("resources/list", {}, timeout=timeout).get("result")
            listed_resources = resources.get("resources") if isinstance(resources, dict) else None
            uris = [
                item.get("uri") for item in listed_resources if isinstance(item, dict)
            ] if isinstance(listed_resources, list) else []
            report.add_check("topology_resource", "splinterm://topology" in uris, "listed" if "splinterm://topology" in uris else "missing")
            ping = self._call(host, "splinterm.ping")
            verdict = classify_ping(ping)
            report.add_check("daemon_ping", verdict == "pong", verdict)
            denied = self._call(host, "splinterm.inspect_topology")
            verdict = classify_deny(denied, "splinterm.inspect_topology")
            report.add_check("deny_all_topology", verdict == "unauthorized", verdict)
        except HostError as error:
            report.add_check("main_session", False, error.code)
            raise PreflightAbort("main_session_failed") from error
        finally:
            self._remember_descendants()
        record = host.close(timeout=self.options.exit_timeout)
        clean = record.clean and record.stderr_bytes == 0 and record.stdout_trailing_bytes == 0
        report.add_check("adapter_clean_exit", clean, "eof_exit_0" if clean else ("forced" if record.forced else "unclean_exit"))

    # --------------------------------------------------- negative cases

    def _negative_cases(self) -> None:
        for name, terminal, case in NEGATIVE_CASES:
            host = self._spawn_adapter(name)
            code, rpc_code, passed = "unexpected", None, False
            try:
                passed, code, rpc_code = case(self, host)
            except HostError as error:
                code = "host_" + error.code
            finally:
                record = host.close(timeout=self.options.exit_timeout)
                messages = host.stdout_messages
            if record.forced:
                passed, code = False, "forced_cleanup"
            self.report.negative_cases.append(NegativeCase(
                name=name,
                terminal=terminal,
                status="passed" if passed else "failed",
                code=code,
                rpc_code=rpc_code,
                returncode=record.returncode,
                stdout_messages=messages,
                forced=record.forced,
            ))
            self.report.add_check(f"negative_{name}", passed, code)

    def _initialize_params(self, version: str = PROTOCOL_VERSION) -> dict[str, Any]:
        return {
            "protocolVersion": version,
            "capabilities": {},
            "clientInfo": {"name": "splinterm-mcp-demo-negative", "version": "1"},
        }

    def _terminal_exit(self, host: McpHost) -> int | None:
        return host.wait_exit(self.options.exit_timeout)

    # --------------------------------------------------------- cleanup

    def _cleanup(self, production_before: dict[str, Any]) -> None:
        report = self.report
        report.transition("cleaning", "interrupted" if self.interrupted else "ok", self.owned_pids)
        self.evidence.persist_state()
        steps: list[CleanupStep] = []

        self.accepting = False
        steps.append(CleanupStep("stop_new_actions", "passed"))
        steps.append(CleanupStep("release_controller", "skipped", "preflight acquires no controller"))
        steps.append(CleanupStep("revoke_grant", "skipped", "preflight requests no grant"))

        forced_adapters = 0
        for host in self.hosts:
            record = host.close(timeout=self.options.exit_timeout)
            forced_adapters += int(record.forced)
        adapters_gone = all(host.state == "closed" for host in self.hosts)
        steps.append(CleanupStep(
            "close_adapters",
            "passed" if adapters_gone else "failed",
            f"{len(self.hosts)} adapters, {forced_adapters} forced",
        ))

        if self.daemon is None:
            steps.append(CleanupStep("stop_daemon", "skipped", "daemon never started"))
        else:
            self._remember_descendants()
            forced = False
            if self.daemon.poll() is None:
                self.daemon.send_signal(signal.SIGTERM)
                try:
                    self.daemon.wait(timeout=self.options.daemon_stop_timeout)
                except subprocess.TimeoutExpired:
                    forced = True
                    self.daemon.kill()  # exact unreaped child PID only
                    self.daemon.wait()
            returncode = self.daemon.returncode
            steps.append(CleanupStep("stop_daemon", "passed", f"returncode {returncode}{', forced' if forced else ''}"))
            if not any(check.name == "daemon_ready" and check.status == "failed" for check in report.checks):
                report.add_check("daemon_clean_exit", returncode == 0 and not forced,
                                 "sigterm_exit_0" if returncode == 0 and not forced else ("forced" if forced else "nonzero_exit"))

        self._adopt_stragglers()
        remaining = self._reap_descendants()
        steps.append(CleanupStep(
            "descendants_exited",
            "passed" if not remaining else "failed",
            f"{len(self.descendants)} recorded, {len(remaining)} remaining",
        ))

        if self.root is None:
            steps.append(CleanupStep("socket_absent", "skipped", "no private root"))
        elif not os.path.lexists(self.root.socket):
            steps.append(CleanupStep("socket_absent", "passed"))
        else:
            # A daemon that exited (or was killed) without unlinking its socket
            # is a product finding. The stale socket lies inside the owned
            # root, so the harness removes it only once no owned process can
            # still hold it.
            if self.daemon is not None:
                report.add_check("daemon_removed_socket", False, "socket_left_behind")
            daemon_gone = self.daemon is None or self.daemon.returncode is not None
            if daemon_gone and not remaining:
                self.root.remove_stale_socket()
            absent = not os.path.lexists(self.root.socket)
            steps.append(CleanupStep(
                "socket_absent",
                "passed" if absent else "failed",
                "stale socket removed by harness" if absent else "socket remains",
            ))

        steps.append(CleanupStep("owned_windows_absent", "skipped", "preflight maps no window"))
        steps.append(CleanupStep("placement_state_removed", "skipped", "preflight creates no placement"))
        steps.append(CleanupStep("desktop_state_restored", "skipped", "preflight touches no desktop state"))

        production_after = production_fingerprints(self._production_environment())
        unchanged = production_after == production_before
        report.add_check(
            "production_state_unchanged",
            unchanged,
            "unchanged" if unchanged else "changed",
            ",".join(sorted(label for label in production_before if production_before.get(label) != production_after.get(label))),
        )

        if self.root is not None:
            stragglers = processes_referencing(self.root.path)
            steps.append(CleanupStep(
                "nonce_processes_absent",
                "passed" if not stragglers else "failed",
                f"{len(stragglers)} processes still reference the private root" if stragglers else "",
            ))
            if stragglers:
                steps.append(CleanupStep("remove_private_root", "failed", "kept while attributable processes remain"))
            else:
                outcome = self.root.remove()
                steps.append(CleanupStep("remove_private_root", "passed" if outcome == "removed" else "failed", outcome))
        else:
            steps.append(CleanupStep("nonce_processes_absent", "skipped", "no private root"))
            steps.append(CleanupStep("remove_private_root", "skipped", "no private root"))

        report.cleanup = steps
        if not report.cleanup_complete():
            report.result = "cleanup_incomplete"
        elif self.interrupted:
            report.result = "aborted"
        elif report.failed_checks():
            report.result = "preflight_failed"
        else:
            report.result = "passed"
        report.finished_at = utc_now()
        if report.cleanup_complete():
            report.transition("clean", report.result)
        try:
            self.evidence.finalize()
        except Exception as error:  # noqa: BLE001 - the verdict must still be reported
            self.evidence_error = type(error).__name__
            self.evidence.close()

    def _adopt_stragglers(self) -> None:
        """Own any process in this harness's tree that names the private root.

        Only this run spawns children carrying the nonce root in their
        environment, so such a process is attributable to the run (for
        example an adapter started just as an interrupt arrived).
        """
        if self.root is None:
            return
        tree = dict(descendants(os.getpid()))
        for pid in processes_referencing(self.root.path):
            if pid in tree:
                self.descendants.add((pid, tree[pid]))

    def _reap_descendants(self) -> list[tuple[int, int]]:
        deadline = time.monotonic() + 5
        remaining = [entry for entry in self.descendants if process_alive(*entry)]
        while remaining and time.monotonic() < deadline:
            time.sleep(0.05)
            remaining = [entry for entry in remaining if process_alive(*entry)]
        for pid, start in remaining:
            kill_exact(pid, start)
        deadline = time.monotonic() + 5
        while remaining and time.monotonic() < deadline:
            time.sleep(0.05)
            for pid, _ in remaining:
                with contextlib.suppress(ChildProcessError, OSError):
                    os.waitpid(pid, os.WNOHANG)  # reaps only our own direct children
            remaining = [entry for entry in remaining if process_alive(*entry)]
        return remaining


def _is_tool_name(name: str) -> bool:
    return bool(re.match(r"^splinterm\.[a-z][a-z0-9_]{0,63}$", name))


CLEANUP_SIGNALS = (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)


def _defer_signals() -> dict[int, Any]:
    """Ignore SIGINT/SIGTERM/SIGHUP during ordered cleanup (main thread only)."""
    if threading.current_thread() is not threading.main_thread():
        return {}
    previous: dict[int, Any] = {}
    for signum in CLEANUP_SIGNALS:
        previous[signum] = signal.signal(signum, signal.SIG_IGN)
    return previous


def _restore_signals(previous: dict[int, Any]) -> None:
    for signum, handler in previous.items():
        signal.signal(signum, handler if handler is not None else signal.SIG_DFL)


def interrupt_once(signum: int, _frame: object) -> None:
    """One-shot signal handler: ignore further signals, then unwind to cleanup."""
    for other in CLEANUP_SIGNALS:
        signal.signal(other, signal.SIG_IGN)
    raise HarnessInterrupted(signum)


# ------------------------------------------------- negative case bodies
#
# Each runs in a fresh adapter. After an expected terminal violation the host
# sends nothing more; it only waits for the adapter's own exit.


def _case_unsupported_version(preflight: Preflight, host: McpHost) -> tuple[bool, str, int | None]:
    request_id = host.send_request(
        "initialize", preflight._initialize_params("2024-11-05"), enforce_lifecycle=False
    )
    response = host.wait_response(request_id, preflight.options.request_timeout)
    rpc_code = response.get("error", {}).get("code") if isinstance(response.get("error"), dict) else None
    returncode = preflight._terminal_exit(host)
    if rpc_code == -32600 and returncode not in (None, 0):
        return True, "rejected_then_exited", rpc_code
    return False, "not_rejected" if rpc_code != -32600 else "did_not_exit", rpc_code


def _case_malformed_capabilities(preflight: Preflight, host: McpHost) -> tuple[bool, str, int | None]:
    params = preflight._initialize_params()
    params["capabilities"] = None
    host.send_request("initialize", params, enforce_lifecycle=False)
    returncode = preflight._terminal_exit(host)
    host.wait_for_output(preflight.options.exit_timeout)
    if returncode not in (None, 0) and host.stdout_messages == 0:
        return True, "exited_without_output", None
    return False, "did_not_fail_closed", None


def _case_malformed_json(preflight: Preflight, host: McpHost) -> tuple[bool, str, int | None]:
    host.send_bytes(b"not-json\n")
    returncode = preflight._terminal_exit(host)
    host.wait_for_output(preflight.options.exit_timeout)
    if returncode not in (None, 0) and host.stdout_messages == 0:
        return True, "exited_without_output", None
    return False, "did_not_fail_closed", None


def _case_batch_after_initialize(preflight: Preflight, host: McpHost) -> tuple[bool, str, int | None]:
    host.initialize(timeout=preflight.options.request_timeout)
    host.send_bytes(b'[{"jsonrpc":"2.0","id":2,"method":"ping"}]\n')
    returncode = preflight._terminal_exit(host)
    host.wait_for_output(preflight.options.exit_timeout)
    if returncode not in (None, 0) and host.stdout_messages == 1:
        return True, "exited_after_initialize_only", None
    return False, "did_not_fail_closed", None


def _case_invalid_envelope_after_initialize(preflight: Preflight, host: McpHost) -> tuple[bool, str, int | None]:
    host.initialize(timeout=preflight.options.request_timeout)
    host.send_bytes(b"{}\n")  # valid JSON, not a JSON-RPC message
    returncode = preflight._terminal_exit(host)
    host.wait_for_output(preflight.options.exit_timeout)
    if returncode not in (None, 0) and host.stdout_messages == 1:
        return True, "exited_after_initialize_only", None
    return False, "did_not_fail_closed", None


def _case_oversized_line(preflight: Preflight, host: McpHost) -> tuple[bool, str, int | None]:
    prefix = b'{"jsonrpc":"2.0","id":1,"method":"ping","params":{"padding":"'
    suffix = b'"}}\n'
    line = prefix + b"x" * (MCP_MAXIMUM_INBOUND_LINE + 1 - len(prefix) - len(suffix)) + suffix
    try:
        host.send_bytes(line, timeout=preflight.options.request_timeout)
    except HostError as error:
        if error.code not in ("write_failed",):
            raise
    host.close_stdin()
    returncode = preflight._terminal_exit(host)
    host.wait_for_output(preflight.options.exit_timeout)
    if returncode not in (None, 0) and host.stdout_messages == 0:
        return True, "exited_without_output", None
    return False, "did_not_fail_closed", None


def _case_unterminated_frame(preflight: Preflight, host: McpHost) -> tuple[bool, str, int | None]:
    host.send_bytes(b'{"jsonrpc":"2.0","id":1,"method":"ping"}')
    host.close_stdin()
    returncode = preflight._terminal_exit(host)
    host.wait_for_output(preflight.options.exit_timeout)
    if returncode not in (None, 0) and host.stdout_messages == 0:
        return True, "exited_without_output", None
    return False, "did_not_fail_closed", None


def _case_discovery_before_initialized(preflight: Preflight, host: McpHost) -> tuple[bool, str, int | None]:
    timeout = preflight.options.request_timeout
    host.initialize(send_initialized=False, timeout=timeout)
    early = host.request("tools/list", {}, enforce_lifecycle=False, timeout=timeout)
    rpc_code = early.get("error", {}).get("code") if isinstance(early.get("error"), dict) else None
    host.send_initialized(timeout=timeout)
    later = host.request("tools/list", {}, timeout=timeout)
    tools = later.get("result", {}).get("tools") if isinstance(later.get("result"), dict) else None
    record = host.close(timeout=preflight.options.exit_timeout)
    if rpc_code == -32600 and isinstance(tools, list) and record.clean:
        return True, "rejected_then_recovered", rpc_code
    return False, "unexpected_recovery", rpc_code


def _case_duplicate_initialize(preflight: Preflight, host: McpHost) -> tuple[bool, str, int | None]:
    timeout = preflight.options.request_timeout
    host.initialize(send_initialized=False, timeout=timeout)
    duplicate = host.request(
        "initialize", preflight._initialize_params(), enforce_lifecycle=False, timeout=timeout
    )
    rpc_code = duplicate.get("error", {}).get("code") if isinstance(duplicate.get("error"), dict) else None
    host.send_initialized(timeout=timeout)
    later = host.request("tools/list", {}, timeout=timeout)
    tools = later.get("result", {}).get("tools") if isinstance(later.get("result"), dict) else None
    record = host.close(timeout=preflight.options.exit_timeout)
    if rpc_code == -32600 and isinstance(tools, list) and record.clean:
        return True, "rejected_session_intact", rpc_code
    return False, "session_disrupted", rpc_code


NEGATIVE_CASES: tuple[tuple[str, bool, Callable[[Preflight, McpHost], tuple[bool, str, int | None]]], ...] = (
    ("unsupported_protocol_version", True, _case_unsupported_version),
    ("malformed_capabilities", True, _case_malformed_capabilities),
    ("malformed_json", True, _case_malformed_json),
    ("batch_after_initialize", True, _case_batch_after_initialize),
    ("invalid_envelope_after_initialize", True, _case_invalid_envelope_after_initialize),
    ("oversized_line", True, _case_oversized_line),
    ("unterminated_frame", True, _case_unterminated_frame),
    ("discovery_before_initialized", False, _case_discovery_before_initialized),
    ("duplicate_initialize", False, _case_duplicate_initialize),
)
