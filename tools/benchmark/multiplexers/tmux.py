"""tmux identity and isolated namespace adapter."""

from __future__ import annotations

import pathlib
import subprocess

from .base import IsolationPlan, MultiplexerAdapter, validate_run_id


class TmuxAdapter(MultiplexerAdapter):
    name = "tmux"
    executable_names = ("tmux",)
    version_arguments = ("-V",)
    process_name_prefixes = ("tmux",)

    def default_session_count(self, executable: pathlib.Path) -> int | None:
        try:
            result = subprocess.run(
                [str(executable), "list-sessions", "-F", "#{session_id}"],
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
        if "no server running" in message or "error connecting" in message:
            return 0
        return None

    def isolation_plan(
        self, root: pathlib.Path, runtime_directory: pathlib.Path, run_id: str
    ) -> IsolationPlan:
        run_id = validate_run_id(run_id)
        executable = self.find_executable(root)
        if executable is None:
            raise RuntimeError("tmux is unavailable")
        runtime_directory = runtime_directory.resolve()
        socket_name = f"splinterbench-{run_id}"
        session_name = f"splinterbench-{run_id}"
        profile = (root / "tools/benchmark/profiles/tmux.conf").resolve()
        prefix = (str(executable), "-L", socket_name, "-f", str(profile))
        return IsolationPlan(
            multiplexer=self.name,
            run_id=run_id,
            runtime_directory=runtime_directory,
            session_name=session_name,
            command_prefix=prefix,
            environment={"TMUX_TMPDIR": str(runtime_directory)},
            cleanup_command=(*prefix, "kill-server"),
        )
