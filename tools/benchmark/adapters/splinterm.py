"""Splinterm portable adapter."""

from __future__ import annotations

import pathlib
from collections.abc import Sequence

from .base import TerminalAdapter


class SplintermAdapter(TerminalAdapter):
    name = "splinterm"
    executable_names = ("splinterm",)
    version_arguments = ("--version",)

    def candidates(self, root: pathlib.Path) -> Sequence[pathlib.Path]:
        return (root / "target/release/splinterm",)
