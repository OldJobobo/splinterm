"""Ghostty portable adapter."""

from .base import TerminalAdapter


class GhosttyAdapter(TerminalAdapter):
    name = "ghostty"
    executable_names = ("ghostty",)
    version_arguments = ("+version",)
