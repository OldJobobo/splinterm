"""Kitty portable adapter."""

from .base import TerminalAdapter


class KittyAdapter(TerminalAdapter):
    name = "kitty"
    executable_names = ("kitty",)
    version_arguments = ("--version",)
