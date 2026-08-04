"""Disposable non-graphical topology orchestration for multiplexer benchmarks."""

from __future__ import annotations

import dataclasses
import json
import os
import pathlib
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from collections.abc import Callable, Mapping, Sequence
from typing import Any

from metrics import process_tree
from multiplexers.base import isolated_environment
from multiplexers.tmux import TmuxAdapter
from multiplexers.zellij import ZellijAdapter
from multiplexing import Topology, tmux_actions, zellij_layout

ROOT = pathlib.Path(__file__).resolve().parents[2]
BENCH_CHILD = ROOT / "tools/benchmark/workloads/bench-child.py"
SPLINTERM_PROFILE = ROOT / "tools/benchmark/profiles/splinterm.ini"


@dataclasses.dataclass(frozen=True)
class ProcessIdentity:
    pid: int
    start_ticks: int

    def as_dict(self) -> dict[str, int]:
        return dataclasses.asdict(self)


class HeadlessController:
    implementation: str
    server_role: str

    def start(self, commands: Mapping[str, Sequence[str]]) -> dict[str, str]:
        raise NotImplementedError

    def inspect(self) -> dict[str, Any]:
        raise NotImplementedError

    def cleanup(self) -> None:
        raise NotImplementedError

    def namespace_absent(self) -> bool:
        raise NotImplementedError

    @property
    def server_identity(self) -> ProcessIdentity:
        raise NotImplementedError

    @property
    def server_pid(self) -> int:
        return self.server_identity.pid


class TmuxController(HeadlessController):
    implementation = "tmux"
    server_role = "multiplexer-server"

    def __init__(self, topology: Topology, output: pathlib.Path, run_id: str):
        self.topology = topology
        self.output = output
        short_runtime = pathlib.Path(tempfile.mkdtemp(prefix="sb-tmux-"))
        try:
            self.plan = TmuxAdapter().isolation_plan(ROOT, short_runtime, run_id)
        except (OSError, RuntimeError, ValueError):
            shutil.rmtree(short_runtime, ignore_errors=True)
            raise
        self.environment = isolated_environment(
            self.plan.environment, remove=("TMUX", "TMUX_PANE")
        )
        self._server_identity: ProcessIdentity | None = None
        self.runtime_ids: dict[str, str] = {}

    @property
    def server_identity(self) -> ProcessIdentity:
        if self._server_identity is None:
            raise RuntimeError("tmux server identity is not available")
        return self._server_identity

    def start(self, commands: Mapping[str, Sequence[str]]) -> dict[str, str]:
        for action in tmux_actions(self.topology, commands):
            name = str(action["pane"])
            argv = [str(item) for item in action["argv"]]
            if action["action"] == "new-session":
                command = [
                    *self.plan.command_prefix,
                    "new-session",
                    "-d",
                    "-s",
                    self.plan.session_name,
                    "-x",
                    "120",
                    "-y",
                    "40",
                    "-P",
                    "-F",
                    "#{pid}\t#{pane_id}",
                    *argv,
                ]
            else:
                target = self.runtime_ids[str(action["target"])]
                direction = "-h" if action["direction"] == "left-right" else "-v"
                ratio = int(action["ratio_milli"])
                if ratio % 10:
                    raise RuntimeError(
                        "tmux percentage requires a ratio divisible by ten"
                    )
                command = [
                    *self.plan.command_prefix,
                    "split-window",
                    direction,
                    "-p",
                    str((1000 - ratio) // 10),
                    "-t",
                    target,
                    "-P",
                    "-F",
                    "#{pane_id}",
                    *argv,
                ]
            result = checked_run(command, self.environment)
            response = result.stdout.strip()
            if action["action"] == "new-session":
                fields = response.split("\t")
                if len(fields) != 2:
                    raise RuntimeError(
                        f"tmux returned an invalid session record: {response!r}"
                    )
                server_pid = positive_integer(fields[0], "tmux server PID")
                runtime_id = fields[1]
                self._server_identity = process_identity(server_pid)
            else:
                runtime_id = response
            if not runtime_id.startswith("%"):
                raise RuntimeError(f"tmux returned an invalid pane ID: {runtime_id!r}")
            self.runtime_ids[name] = runtime_id
        return dict(self.runtime_ids)

    def inspect(self) -> dict[str, Any]:
        result = checked_run(
            [
                *self.plan.command_prefix,
                "list-panes",
                "-a",
                "-F",
                "#{pane_id}\t#{pane_pid}\t#{pane_left}\t#{pane_top}\t#{pane_width}\t#{pane_height}",
            ],
            self.environment,
        )
        panes = []
        for line in result.stdout.splitlines():
            fields = line.split("\t")
            if len(fields) != 6:
                raise RuntimeError(f"tmux returned a malformed pane record: {line!r}")
            panes.append(
                {
                    "runtime_id": fields[0],
                    "pty_child_pid": positive_integer(fields[1], "tmux pane PID"),
                    "x": int(fields[2]),
                    "y": int(fields[3]),
                    "columns": int(fields[4]),
                    "rows": int(fields[5]),
                }
            )
        if {pane["runtime_id"] for pane in panes} != set(self.runtime_ids.values()):
            raise RuntimeError("tmux pane inventory does not match created topology")
        return {"terminal_panes": panes, "visible_plugin_panes": 0}

    def cleanup(self) -> None:
        server = self._server_identity
        command_succeeded = False
        try:
            result = subprocess.run(
                self.plan.cleanup_command,
                env=self.environment,
                text=True,
                capture_output=True,
                check=False,
                timeout=10,
            )
            command_succeeded = result.returncode == 0
        except (OSError, subprocess.SubprocessError):
            pass
        if server is not None and (
            not command_succeeded or not wait_processes_gone([server], 3.0)
        ):
            terminate_processes_exact([server])
        server_survived = server is not None and same_process(server)
        self._socket_path().unlink(missing_ok=True)
        shutil.rmtree(self.plan.runtime_directory, ignore_errors=False)
        if server_survived:
            raise RuntimeError("tmux server survived exact-incarnation cleanup")

    def namespace_absent(self) -> bool:
        return (
            not self._socket_path().exists()
            and not self.plan.runtime_directory.exists()
        )

    def _socket_path(self) -> pathlib.Path:
        return (
            self.plan.runtime_directory
            / f"tmux-{os.getuid()}"
            / f"splinterbench-{self.plan.run_id}"
        )


class ZellijController(HeadlessController):
    implementation = "zellij"
    server_role = "multiplexer-server"

    def __init__(self, topology: Topology, output: pathlib.Path, run_id: str):
        self.topology = topology
        self.output = output
        short_runtime = pathlib.Path(tempfile.mkdtemp(prefix="sb-zellij-"))
        try:
            self.plan = ZellijAdapter().isolation_plan(ROOT, short_runtime, run_id)
        except (OSError, RuntimeError, ValueError):
            shutil.rmtree(short_runtime, ignore_errors=True)
            raise
        self.environment = isolated_environment(
            self.plan.environment,
            remove=("ZELLIJ", "ZELLIJ_SESSION_NAME", "ZELLIJ_PANE_ID"),
        )
        self._server_identity: ProcessIdentity | None = None
        self._socket: pathlib.Path | None = None
        self.runtime_ids: dict[str, str] = {}

    @property
    def server_identity(self) -> ProcessIdentity:
        if self._server_identity is None:
            raise RuntimeError("Zellij server identity is not available")
        return self._server_identity

    def start(self, commands: Mapping[str, Sequence[str]]) -> dict[str, str]:
        layout = self.output / "layout.kdl"
        layout.write_text(zellij_layout(self.topology, commands), encoding="utf-8")
        executable = self.plan.command_prefix[0]
        profile = self.plan.command_prefix[2]
        checked_run(
            [
                executable,
                "--config",
                profile,
                "attach",
                "--create-background",
                self.plan.session_name,
                "options",
                "--default-layout",
                str(layout),
            ],
            self.environment,
        )
        socket_root = pathlib.Path(self.plan.environment["ZELLIJ_SOCKET_DIR"])
        self._socket = wait_for_socket(socket_root, self.plan.session_name, 5.0)
        server_pid = wait_for_exact_cmdline_token(str(self._socket), 5.0)
        self._server_identity = process_identity(server_pid)
        inventory = self._wait_for_inventory(5.0)
        terminal = inventory["terminal_panes"]
        by_name = {str(pane["title"]): f"terminal_{pane['id']}" for pane in terminal}
        if set(by_name) != set(self.topology.pane_names):
            raise RuntimeError(
                "Zellij terminal pane titles do not match created topology"
            )
        self.runtime_ids = by_name
        return dict(self.runtime_ids)

    def inspect(self) -> dict[str, Any]:
        executable = self.plan.command_prefix[0]
        profile = self.plan.command_prefix[2]
        result = checked_run(
            [
                executable,
                "--config",
                profile,
                "--session",
                self.plan.session_name,
                "action",
                "list-panes",
                "--json",
                "--all",
            ],
            self.environment,
        )
        try:
            panes = json.loads(result.stdout)
        except json.JSONDecodeError as error:
            raise RuntimeError("Zellij returned malformed pane JSON") from error
        terminal = [pane for pane in panes if not pane.get("is_plugin", False)]
        visible_plugins = [
            pane
            for pane in panes
            if pane.get("is_plugin", False) and not pane.get("is_suppressed", False)
        ]
        if len(terminal) != len(self.topology.pane_names):
            raise RuntimeError("Zellij terminal pane count does not match topology")
        if visible_plugins:
            raise RuntimeError("Zellij benchmark layout exposed plugin UI")
        bounded = [
            {
                "id": int(pane["id"]),
                "title": str(pane["title"]),
                "x": int(pane["pane_x"]),
                "y": int(pane["pane_y"]),
                "columns": int(pane["pane_columns"]),
                "rows": int(pane["pane_rows"]),
            }
            for pane in terminal
        ]
        return {
            "terminal_panes": bounded,
            "visible_plugin_panes": len(visible_plugins),
            "suppressed_internal_plugin_panes": len(panes) - len(terminal),
        }

    def _wait_for_inventory(self, timeout: float) -> dict[str, Any]:
        deadline = time.monotonic() + timeout
        last_error: RuntimeError | None = None
        while time.monotonic() < deadline:
            try:
                return self.inspect()
            except RuntimeError as error:
                last_error = error
                time.sleep(0.02)
        raise RuntimeError("Zellij pane inventory did not settle") from last_error

    def cleanup(self) -> None:
        server = self._server_identity
        command_succeeded = False
        try:
            result = subprocess.run(
                self.plan.cleanup_command,
                env=self.environment,
                text=True,
                capture_output=True,
                check=False,
                timeout=10,
            )
            command_succeeded = result.returncode == 0
        except (OSError, subprocess.SubprocessError):
            pass
        if server is not None and (
            not command_succeeded or not wait_processes_gone([server], 3.0)
        ):
            terminate_processes_exact([server])
        server_survived = server is not None and same_process(server)
        if self._socket is not None:
            self._socket.unlink(missing_ok=True)
        shutil.rmtree(self.plan.runtime_directory, ignore_errors=False)
        if server_survived:
            raise RuntimeError("Zellij server survived exact-incarnation cleanup")

    def namespace_absent(self) -> bool:
        socket_absent = self._socket is None or not self._socket.exists()
        return socket_absent and not self.plan.runtime_directory.exists()


class SplintermController(HeadlessController):
    implementation = "splinterm"
    server_role = "daemon"

    def __init__(self, topology: Topology, output: pathlib.Path, run_id: str):
        self.topology = topology
        self.output = output
        self.run_id = run_id
        self.client = benchmark_executable(
            "SPLINTERBENCH_SPLINTERM_CLIENT", "splinterm"
        )
        self.daemon = benchmark_executable(
            "SPLINTERBENCH_SPLINTERM_DAEMON", "splinterd"
        )
        self.state = output / "state"
        self.state.mkdir(parents=True, exist_ok=False)
        self.runtime_directory = pathlib.Path(tempfile.mkdtemp(prefix="sb-splinterm-"))
        self.socket = self.runtime_directory / "splinterd.sock"
        self.environment = isolated_environment(
            {
                "SPLINTERM_SOCKET": str(self.socket),
                "SPLINTERM_ENABLE_DEV_ATTACH": "1",
                "SPLINTERM_CONFIG": str(SPLINTERM_PROFILE),
                "XDG_STATE_HOME": str(self.state),
            }
        )
        self.process: subprocess.Popen[str] | None = None
        self._server_identity: ProcessIdentity | None = None
        self.runtime_ids: dict[str, str] = {}
        self.window_id: str | None = None

    @property
    def server_identity(self) -> ProcessIdentity:
        if self._server_identity is None:
            raise RuntimeError("Splinterm daemon identity is not available")
        return self._server_identity

    def start(self, commands: Mapping[str, Sequence[str]]) -> dict[str, str]:
        daemon_stdout = (self.output / "daemon.stdout").open("w", encoding="utf-8")
        daemon_stderr = (self.output / "daemon.stderr").open("w", encoding="utf-8")
        self.process = subprocess.Popen(
            [str(self.daemon)],
            env=self.environment,
            text=True,
            stdout=daemon_stdout,
            stderr=daemon_stderr,
        )
        self._server_identity = process_identity(self.process.pid)
        wait_for_path(self.socket, 5.0, self._daemon_failure)
        actions = tmux_actions(self.topology, commands)
        for action in actions:
            name = str(action["pane"])
            argv = [str(item) for item in action["argv"]]
            if action["action"] == "new-session":
                value = self._json_command(
                    ["new", f"splinterbench-{self.run_id}", "--", *argv]
                )
                self.window_id = str(value["resource"]["window_id"])
            else:
                axis = (
                    "horizontal" if action["direction"] == "left-right" else "vertical"
                )
                value = self._json_command(
                    [
                        "split",
                        "--axis",
                        axis,
                        "--side",
                        "second",
                        self.runtime_ids[str(action["target"])],
                        "--",
                        *argv,
                    ]
                )
            self.runtime_ids[name] = str(value["resource"]["splint_id"])
        return dict(self.runtime_ids)

    def inspect(self) -> dict[str, Any]:
        value = self._json_command(["topology"])
        splints = [
            item
            for item in value["data"]["splints"]
            if item["window_id"] == self.window_id
        ]
        if {item["splint_id"] for item in splints} != set(self.runtime_ids.values()):
            raise RuntimeError(
                "Splinterm topology inventory does not match created IDs"
            )
        if any(item["lifecycle"] != "running" for item in splints):
            raise RuntimeError(
                "Splinterm topology contains a non-running benchmark pane"
            )
        return {
            "terminal_panes": [
                {
                    "runtime_id": str(item["splint_id"]),
                    "incarnation": int(item["current_incarnation"]),
                }
                for item in splints
            ],
            "visible_plugin_panes": 0,
            "topology_revision": int(value["resource"]["topology_revision"]),
        }

    def cleanup(self) -> None:
        if self.process is not None and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                if self._server_identity is not None and same_process(
                    self._server_identity
                ):
                    self.process.kill()
                self.process.wait(timeout=2)
        self.socket.unlink(missing_ok=True)
        self.socket.with_name(self.socket.name + ".content").unlink(missing_ok=True)
        shutil.rmtree(self.runtime_directory, ignore_errors=False)

    def namespace_absent(self) -> bool:
        return (
            not self.socket.exists()
            and not self.socket.with_name(self.socket.name + ".content").exists()
            and not self.runtime_directory.exists()
        )

    def _json_command(self, arguments: Sequence[str]) -> dict[str, Any]:
        result = checked_run(
            [str(self.client), "--output", "json", *arguments], self.environment
        )
        try:
            value = json.loads(result.stdout)
        except json.JSONDecodeError as error:
            raise RuntimeError("Splinterm returned malformed JSON") from error
        if not value.get("ok", False):
            raise RuntimeError(f"Splinterm operation failed: {value.get('error', {})}")
        return value

    def _daemon_failure(self) -> str | None:
        if self.process is None or self.process.poll() is None:
            return None
        stderr = self.output / "daemon.stderr"
        try:
            detail = stderr.read_text(encoding="utf-8")[-1000:]
        except OSError:
            detail = ""
        return f"Splinterm daemon exited early ({self.process.returncode}): {detail}"


def controller_for(
    implementation: str, topology: Topology, output: pathlib.Path, run_id: str
) -> HeadlessController:
    if implementation == "splinterm":
        return SplintermController(topology, output, run_id)
    if implementation == "tmux":
        return TmuxController(topology, output, run_id)
    if implementation == "zellij":
        return ZellijController(topology, output, run_id)
    raise ValueError(f"unsupported multiplexer implementation: {implementation}")


def pane_commands(
    topology: Topology, output: pathlib.Path, idle_seconds: float
) -> dict[str, tuple[str, ...]]:
    return {
        name: (
            sys.executable,
            str(BENCH_CHILD),
            "idle",
            "--ready-file",
            str(output / f"{name}-ready.json"),
            "--idle-seconds",
            str(idle_seconds),
        )
        for name in topology.pane_names
    }


def wait_for_ready(
    topology: Topology, output: pathlib.Path, deadline_seconds: float
) -> dict[str, dict[str, int | str]]:
    deadline = time.monotonic() + deadline_seconds
    pending = set(topology.pane_names)
    records: dict[str, dict[str, int | str]] = {}
    while pending:
        for name in tuple(pending):
            path = output / f"{name}-ready.json"
            if not path.exists():
                continue
            try:
                value = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                continue
            if value.get("schema") != "splinterm.benchmark.child.v1":
                raise RuntimeError(f"{name} readiness has an unsupported schema")
            if value.get("event") != "ready":
                raise RuntimeError(f"{name} readiness has the wrong event")
            pid = int(value["pid"])
            if pid <= 0:
                raise RuntimeError(f"{name} readiness has an invalid PID")
            records[name] = {
                "schema": str(value["schema"]),
                "event": str(value["event"]),
                "monotonic_ns": int(value["monotonic_ns"]),
                "pid": pid,
            }
            pending.remove(name)
        if not pending:
            break
        if time.monotonic() >= deadline:
            raise TimeoutError(
                f"timed out waiting for pane readiness: {sorted(pending)}"
            )
        time.sleep(0.01)
    if len({int(item["pid"]) for item in records.values()}) != len(records):
        raise RuntimeError("pane readiness records do not have unique PIDs")
    return records


def verify_process_roles(
    server: ProcessIdentity,
    readiness: Mapping[str, Mapping[str, int | str]],
    proc_root: pathlib.Path = pathlib.Path("/proc"),
) -> dict[str, Any]:
    workloads = [
        process_identity(int(item["pid"]), proc_root) for item in readiness.values()
    ]
    workload_pids = {item.pid for item in workloads}
    if server.pid in workload_pids:
        raise RuntimeError("server and workload process roles overlap")
    descendants = set(process_tree(proc_root, server.pid))
    missing = workload_pids - descendants
    if missing:
        raise RuntimeError(
            f"workload PIDs are not server descendants: {sorted(missing)}"
        )
    helper_pids = sorted(descendants - workload_pids - {server.pid})
    helpers = [process_identity(pid, proc_root) for pid in helper_pids]
    return {
        "role_sets_disjoint": True,
        "roles": [
            {"role": "server", "processes": [server.as_dict()]},
            {
                "role": "helper",
                "processes": [item.as_dict() for item in helpers],
            },
            {
                "role": "workload",
                "processes": [item.as_dict() for item in workloads],
            },
        ],
    }


def process_identity(
    pid: int, proc_root: pathlib.Path = pathlib.Path("/proc")
) -> ProcessIdentity:
    try:
        stat = (proc_root / str(pid) / "stat").read_text(encoding="utf-8")
        fields = stat[stat.rfind(")") + 2 :].split()
        start_ticks = int(fields[19])
    except (OSError, ValueError, IndexError) as error:
        raise RuntimeError(
            f"process {pid} disappeared before identity capture"
        ) from error
    return ProcessIdentity(pid, start_ticks)


def same_process(
    identity: ProcessIdentity, proc_root: pathlib.Path = pathlib.Path("/proc")
) -> bool:
    try:
        return process_identity(identity.pid, proc_root) == identity
    except RuntimeError:
        return False


def wait_processes_gone(identities: Sequence[ProcessIdentity], timeout: float) -> bool:
    deadline = time.monotonic() + timeout
    while any(same_process(identity) for identity in identities):
        if time.monotonic() >= deadline:
            return False
        time.sleep(0.02)
    return True


def terminate_processes_exact(
    identities: Sequence[ProcessIdentity], timeout: float = 2.0
) -> bool:
    """Terminate only still-matching process incarnations, then bound escalation."""

    remaining = [identity for identity in identities if same_process(identity)]
    for identity in remaining:
        try:
            os.kill(identity.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
    if wait_processes_gone(remaining, timeout):
        return True
    remaining = [identity for identity in remaining if same_process(identity)]
    for identity in remaining:
        try:
            os.kill(identity.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    return wait_processes_gone(remaining, timeout)


def wait_for_exact_cmdline_token(
    token: str, timeout: float, proc_root: pathlib.Path = pathlib.Path("/proc")
) -> int:
    deadline = time.monotonic() + timeout
    while True:
        matches = processes_with_exact_cmdline_token(token, proc_root)
        if len(matches) == 1:
            return matches[0]
        if len(matches) > 1:
            raise RuntimeError(
                f"multiple processes own exact namespace token {token!r}"
            )
        if time.monotonic() >= deadline:
            raise TimeoutError(f"no process owns exact namespace token {token!r}")
        time.sleep(0.02)


def processes_with_exact_cmdline_token(
    token: str, proc_root: pathlib.Path = pathlib.Path("/proc")
) -> list[int]:
    encoded = os.fsencode(token)
    matches = []
    try:
        processes = list(proc_root.iterdir())
    except OSError:
        return []
    for process in processes:
        if not process.name.isdigit():
            continue
        try:
            fields = (process / "cmdline").read_bytes().split(b"\0")
        except OSError:
            continue
        if encoded in fields:
            matches.append(int(process.name))
    return sorted(matches)


def wait_for_socket(root: pathlib.Path, name: str, timeout: float) -> pathlib.Path:
    deadline = time.monotonic() + timeout
    while True:
        matches = [path for path in root.glob(f"*/{name}") if path.exists()]
        if len(matches) == 1:
            return matches[0]
        if len(matches) > 1:
            raise RuntimeError(
                "Zellij created multiple sockets for one benchmark session"
            )
        if time.monotonic() >= deadline:
            raise TimeoutError("Zellij benchmark socket did not appear")
        time.sleep(0.02)


def wait_for_path(
    path: pathlib.Path,
    timeout: float,
    failure: Callable[[], str | None] | None = None,
) -> None:
    deadline = time.monotonic() + timeout
    while not path.exists():
        if failure and (message := failure()):
            raise RuntimeError(message)
        if time.monotonic() >= deadline:
            raise TimeoutError(f"timed out waiting for {path}")
        time.sleep(0.02)


def checked_run(
    command: Sequence[str], environment: Mapping[str, str], timeout: float = 10
) -> subprocess.CompletedProcess[str]:
    try:
        result = subprocess.run(
            command,
            env=environment,
            text=True,
            capture_output=True,
            check=False,
            timeout=timeout,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise RuntimeError(f"command failed to execute: {command[0]}") from error
    if result.returncode:
        detail = (result.stderr or result.stdout).strip()[-1000:]
        raise RuntimeError(
            f"command exited {result.returncode}: {pathlib.Path(command[0]).name}: {detail}"
        )
    return result


def positive_integer(value: str, label: str) -> int:
    try:
        parsed = int(value.strip())
    except ValueError as error:
        raise RuntimeError(f"{label} is not an integer") from error
    if parsed <= 0:
        raise RuntimeError(f"{label} is not positive")
    return parsed


def benchmark_executable(variable: str, name: str) -> pathlib.Path:
    candidates = []
    if override := os.environ.get(variable):
        candidates.append(pathlib.Path(override).expanduser())
    candidates.append(ROOT / "target/release" / name)
    if found := shutil.which(name):
        candidates.append(pathlib.Path(found))
    for candidate in candidates:
        if candidate.is_file() and candidate.stat().st_mode & 0o111:
            return candidate.resolve()
    raise RuntimeError(f"{name} executable is unavailable")
