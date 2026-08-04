"""Portable identity and isolation contracts for terminal multiplexers."""

from __future__ import annotations

import dataclasses
import os
import pathlib
import re
import shutil
import subprocess
from collections.abc import Mapping, Sequence

from adapters.base import file_sha256

_RUN_ID = re.compile(r"[a-z0-9][a-z0-9-]{0,47}")


@dataclasses.dataclass(frozen=True)
class MultiplexerIdentity:
    """Auditable identity without retaining ambient session names."""

    name: str
    available: bool
    executable: str | None
    version: str | None
    sha256: str | None
    ambient_process_count: int
    default_session_count: int | None

    def as_dict(self) -> dict[str, str | bool | int | None]:
        return dataclasses.asdict(self)


@dataclasses.dataclass(frozen=True)
class IsolationPlan:
    """Dedicated namespace and exact cleanup command for one benchmark run."""

    multiplexer: str
    run_id: str
    runtime_directory: pathlib.Path
    session_name: str
    command_prefix: tuple[str, ...]
    environment: Mapping[str, str]
    cleanup_command: tuple[str, ...]

    def as_dict(self) -> dict[str, object]:
        return {
            "multiplexer": self.multiplexer,
            "run_id": self.run_id,
            "runtime_directory": str(self.runtime_directory),
            "session_name": self.session_name,
            "command_prefix": list(self.command_prefix),
            "environment": dict(self.environment),
            "cleanup_command": list(self.cleanup_command),
        }


class MultiplexerAdapter:
    """Non-graphical identity probe and isolated command construction."""

    name: str
    executable_names: tuple[str, ...]
    version_arguments: tuple[str, ...]
    process_name_prefixes: tuple[str, ...]

    def candidates(self, root: pathlib.Path) -> Sequence[pathlib.Path]:
        del root
        return ()

    def find_executable(self, root: pathlib.Path) -> pathlib.Path | None:
        for candidate in self.candidates(root):
            if candidate.is_file() and candidate.stat().st_mode & 0o111:
                return candidate.resolve()
        for name in self.executable_names:
            if found := shutil.which(name):
                return pathlib.Path(found).resolve()
        return None

    def read_version(self, executable: pathlib.Path) -> str | None:
        try:
            result = subprocess.run(
                [str(executable), *self.version_arguments],
                text=True,
                capture_output=True,
                check=False,
                timeout=10,
            )
        except (OSError, subprocess.TimeoutExpired):
            return None
        text = "\n".join(
            part.strip() for part in (result.stdout, result.stderr) if part.strip()
        )
        return text or None

    def default_session_count(self, executable: pathlib.Path) -> int | None:
        del executable
        return None

    def ambient_process_count(
        self, proc_root: pathlib.Path = pathlib.Path("/proc")
    ) -> int:
        count = 0
        try:
            processes = list(proc_root.iterdir())
        except OSError:
            return 0
        for process in processes:
            if not process.name.isdigit():
                continue
            try:
                name = (process / "comm").read_text(encoding="utf-8").strip()
            except OSError:
                continue
            if any(
                name == prefix or name.startswith(f"{prefix}:")
                for prefix in self.process_name_prefixes
            ):
                count += 1
        return count

    def probe(
        self,
        root: pathlib.Path,
        proc_root: pathlib.Path = pathlib.Path("/proc"),
    ) -> MultiplexerIdentity:
        executable = self.find_executable(root)
        if executable is None:
            return MultiplexerIdentity(
                name=self.name,
                available=False,
                executable=None,
                version=None,
                sha256=None,
                ambient_process_count=self.ambient_process_count(proc_root),
                default_session_count=None,
            )
        return MultiplexerIdentity(
            name=self.name,
            available=True,
            executable=str(executable),
            version=self.read_version(executable),
            sha256=file_sha256(executable),
            ambient_process_count=self.ambient_process_count(proc_root),
            default_session_count=self.default_session_count(executable),
        )

    def isolation_plan(
        self, root: pathlib.Path, runtime_directory: pathlib.Path, run_id: str
    ) -> IsolationPlan:
        raise NotImplementedError


def validate_run_id(run_id: str) -> str:
    """Keep socket/session selectors bounded and free of path syntax."""

    if not _RUN_ID.fullmatch(run_id):
        raise ValueError(
            "run ID must be 1-48 lowercase letters, digits, or hyphens, "
            "starting with a letter or digit"
        )
    return run_id


def isolated_environment(
    overrides: Mapping[str, str], *, remove: Sequence[str] = ()
) -> dict[str, str]:
    """Copy the host environment while dropping ambient multiplexer selectors."""

    environment = {**os.environ, **overrides}
    for name in remove:
        environment.pop(name, None)
    return environment
