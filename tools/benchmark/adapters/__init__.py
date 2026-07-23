"""Known terminal adapters in stable report order."""

from .alacritty import AlacrittyAdapter
from .base import TerminalAdapter, TerminalIdentity
from .foot import FootAdapter
from .ghostty import GhosttyAdapter
from .kitty import KittyAdapter
from .splinterm import SplintermAdapter


def all_adapters() -> tuple[TerminalAdapter, ...]:
    return (
        SplintermAdapter(),
        FootAdapter(),
        KittyAdapter(),
        GhosttyAdapter(),
        AlacrittyAdapter(),
    )


__all__ = ["TerminalAdapter", "TerminalIdentity", "all_adapters"]
