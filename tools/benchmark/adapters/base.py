"""Terminal identity probes for the portable benchmark foundation."""

from __future__ import annotations

import dataclasses
import hashlib
import pathlib
import shutil
import subprocess
from collections.abc import Sequence


@dataclasses.dataclass(frozen=True)
class TerminalIdentity:
    """Auditable identity of one terminal executable."""

    name: str
    available: bool
    executable: str | None
    version: str | None
    sha256: str | None

    def as_dict(self) -> dict[str, str | bool | None]:
        return dataclasses.asdict(self)


class TerminalAdapter:
    """Portable portion of a terminal adapter; graphical launch comes later."""

    name: str
    executable_names: tuple[str, ...]
    version_arguments: tuple[str, ...]

    def candidates(self, root: pathlib.Path) -> Sequence[pathlib.Path]:
        del root
        return ()

    def find_executable(self, root: pathlib.Path) -> pathlib.Path | None:
        for candidate in self.candidates(root):
            if candidate.is_file() and candidate.stat().st_mode & 0o111:
                return candidate.resolve()
        for name in self.executable_names:
            found = shutil.which(name)
            if found:
                return pathlib.Path(found).resolve()
        return None

    def probe(self, root: pathlib.Path) -> TerminalIdentity:
        executable = self.find_executable(root)
        if executable is None:
            return TerminalIdentity(self.name, False, None, None, None)
        version = self.read_version(executable)
        return TerminalIdentity(
            name=self.name,
            available=True,
            executable=str(executable),
            version=version,
            sha256=file_sha256(executable),
        )

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


def file_sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()
