"""Known multiplexer adapters in stable report order."""

from .base import IsolationPlan, MultiplexerAdapter, MultiplexerIdentity
from .tmux import TmuxAdapter
from .zellij import ZellijAdapter


def all_adapters() -> tuple[MultiplexerAdapter, ...]:
    return (TmuxAdapter(), ZellijAdapter())


__all__ = [
    "IsolationPlan",
    "MultiplexerAdapter",
    "MultiplexerIdentity",
    "all_adapters",
]
