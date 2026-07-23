"""Alacritty portable adapter."""

from .base import TerminalAdapter


class AlacrittyAdapter(TerminalAdapter):
    name = "alacritty"
    executable_names = ("alacritty",)
    version_arguments = ("--version",)
