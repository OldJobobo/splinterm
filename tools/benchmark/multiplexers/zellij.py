"""Zellij identity and isolated namespace adapter."""

from __future__ import annotations

import pathlib
import subprocess

from .base import IsolationPlan, MultiplexerAdapter, validate_run_id


class ZellijAdapter(MultiplexerAdapter):
    name = "zellij"
    executable_names = ("zellij",)
    version_arguments = ("--version",)
    process_name_prefixes = ("zellij",)

    def default_session_count(self, executable: pathlib.Path) -> int | None:
        try:
            result = subprocess.run(
                [str(executable), "list-sessions", "--short", "--no-formatting"],
                text=True,
                capture_output=True,
                check=False,
                timeout=10,
            )
        except (OSError, subprocess.TimeoutExpired):
            return None
        if result.returncode == 0:
            return len([line for line in result.stdout.splitlines() if line.strip()])
        message = f"{result.stdout}\n{result.stderr}".lower()
        if "no active sessions" in message or "no sessions" in message:
            return 0
        return None

    def isolation_plan(
        self, root: pathlib.Path, runtime_directory: pathlib.Path, run_id: str
    ) -> IsolationPlan:
        run_id = validate_run_id(run_id)
        executable = self.find_executable(root)
        if executable is None:
            raise RuntimeError("Zellij is unavailable")
        runtime_directory = runtime_directory.resolve()
        socket_directory = runtime_directory / "zellij-sockets"
        session_name = f"splinterbench-{run_id}"
        profile = (root / "tools/benchmark/profiles/zellij.kdl").resolve()
        prefix = (
            str(executable),
            "--config",
            str(profile),
            "--session",
            session_name,
        )
        environment = {
            "ZELLIJ_SOCKET_DIR": str(socket_directory),
            "ZELLIJ_CONFIG_FILE": str(profile),
        }
        return IsolationPlan(
            multiplexer=self.name,
            run_id=run_id,
            runtime_directory=runtime_directory,
            session_name=session_name,
            command_prefix=prefix,
            environment=environment,
            cleanup_command=(
                str(executable),
                "--config",
                str(profile),
                "kill-session",
                session_name,
            ),
        )
