"""Foot portable adapter."""

from .base import TerminalAdapter


class FootAdapter(TerminalAdapter):
    name = "foot"
    executable_names = ("foot",)
    version_arguments = ("--version",)
