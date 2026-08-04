"""Stack identities and deterministic pane topologies for multiplexer benchmarks."""

from __future__ import annotations

import dataclasses
import json
from collections.abc import Mapping, Sequence
from typing import Protocol, TypeAlias


@dataclasses.dataclass(frozen=True)
class StackIdentity:
    """One intentionally comparable terminal/multiplexer composition."""

    name: str
    terminal: str
    multiplexer: str | None
    integration: str
    available: bool

    def as_dict(self) -> dict[str, str | bool | None]:
        return dataclasses.asdict(self)


@dataclasses.dataclass(frozen=True)
class Pane:
    name: str


@dataclasses.dataclass(frozen=True)
class Split:
    direction: str
    first: TopologyNode
    second: TopologyNode
    ratio_milli: int = 500


TopologyNode: TypeAlias = Pane | Split


@dataclasses.dataclass(frozen=True)
class Topology:
    name: str
    root: TopologyNode

    @property
    def pane_names(self) -> tuple[str, ...]:
        return tuple(_pane_names(self.root))

    def as_dict(self) -> dict[str, object]:
        return {
            "name": self.name,
            "pane_count": len(self.pane_names),
            "panes": list(self.pane_names),
            "tree": _node_dict(self.root),
        }


class AvailableIdentity(Protocol):
    name: str
    available: bool


def stack_identities(
    terminal_identities: Sequence[AvailableIdentity],
    multiplexer_identities: Sequence[AvailableIdentity],
) -> tuple[StackIdentity, ...]:
    terminals = {str(item.name): bool(item.available) for item in terminal_identities}
    multiplexers = {
        str(item.name): bool(item.available) for item in multiplexer_identities
    }
    return (
        StackIdentity(
            "splinterm-native",
            "splinterm",
            "splinterm",
            "native",
            terminals.get("splinterm", False),
        ),
        StackIdentity(
            "foot-bare",
            "foot",
            None,
            "none",
            terminals.get("foot", False),
        ),
        StackIdentity(
            "foot-tmux",
            "foot",
            "tmux",
            "nested",
            terminals.get("foot", False) and multiplexers.get("tmux", False),
        ),
        StackIdentity(
            "foot-zellij",
            "foot",
            "zellij",
            "nested",
            terminals.get("foot", False) and multiplexers.get("zellij", False),
        ),
    )


def all_topologies() -> tuple[Topology, ...]:
    return (
        Topology("single", Pane("pane-0")),
        Topology(
            "two-columns",
            Split("left-right", Pane("pane-0"), Pane("pane-1")),
        ),
        Topology(
            "four-grid",
            Split(
                "left-right",
                Split("top-bottom", Pane("pane-0"), Pane("pane-1")),
                Split("top-bottom", Pane("pane-2"), Pane("pane-3")),
            ),
        ),
    )


def topology_named(name: str) -> Topology:
    for topology in all_topologies():
        if topology.name == name:
            return topology
    choices = ", ".join(item.name for item in all_topologies())
    raise ValueError(f"unknown topology {name!r}; expected one of: {choices}")


def validate_topology(topology: Topology) -> None:
    names = topology.pane_names
    if not names or len(names) != len(set(names)):
        raise ValueError("topology pane names must be nonempty and unique")

    def visit(node: TopologyNode) -> None:
        if isinstance(node, Pane):
            if not node.name or any(character.isspace() for character in node.name):
                raise ValueError(
                    "pane names must be nonempty and contain no whitespace"
                )
            return
        if node.direction not in ("left-right", "top-bottom"):
            raise ValueError(f"unsupported split direction: {node.direction}")
        if not 1 <= node.ratio_milli <= 999:
            raise ValueError("split ratio must be between 1 and 999")
        visit(node.first)
        visit(node.second)

    visit(topology.root)


def tmux_actions(
    topology: Topology, commands: Mapping[str, Sequence[str]]
) -> tuple[dict[str, object], ...]:
    """Describe tmux creation without assuming runtime-assigned pane IDs."""

    validate_topology(topology)
    normalized = _commands(topology, commands)
    first = _first_pane(topology.root)
    actions: list[dict[str, object]] = [
        {"action": "new-session", "pane": first, "argv": list(normalized[first])}
    ]

    def materialize(node: TopologyNode, existing: str) -> None:
        if isinstance(node, Pane):
            if node.name != existing:
                raise ValueError("topology materialization lost its anchor pane")
            return
        first_anchor = _first_pane(node.first)
        second_anchor = _first_pane(node.second)
        if first_anchor != existing:
            raise ValueError("split subtree does not begin at its existing anchor")
        actions.append(
            {
                "action": "split-pane",
                "target": first_anchor,
                "pane": second_anchor,
                "direction": node.direction,
                "ratio_milli": node.ratio_milli,
                "argv": list(normalized[second_anchor]),
            }
        )
        materialize(node.first, first_anchor)
        materialize(node.second, second_anchor)

    materialize(topology.root, first)
    return tuple(actions)


def zellij_layout(
    topology: Topology,
    commands: Mapping[str, Sequence[str]],
    *,
    close_on_exit: bool = True,
) -> str:
    """Render a plugin-free KDL layout with one exact command per pane."""

    validate_topology(topology)
    normalized = _commands(topology, commands)

    def render(node: TopologyNode, indent: int) -> list[str]:
        prefix = " " * indent
        if isinstance(node, Pane):
            command = normalized[node.name]
            properties = (
                f"name={_kdl_quote(node.name)} command={_kdl_quote(command[0])} "
                f"close_on_exit={'true' if close_on_exit else 'false'}"
            )
            if len(command) == 1:
                return [f"{prefix}pane {properties}"]
            arguments = " ".join(_kdl_quote(item) for item in command[1:])
            return [
                f"{prefix}pane {properties} {{",
                f"{prefix}    args {arguments}",
                f"{prefix}}}",
            ]
        direction = "vertical" if node.direction == "left-right" else "horizontal"
        lines = [f'{prefix}pane split_direction="{direction}" {{']
        lines.extend(render(node.first, indent + 4))
        lines.extend(render(node.second, indent + 4))
        lines.append(f"{prefix}}}")
        return lines

    return "\n".join(["layout {", *render(topology.root, 4), "}", ""])


def _commands(
    topology: Topology, commands: Mapping[str, Sequence[str]]
) -> dict[str, tuple[str, ...]]:
    expected = set(topology.pane_names)
    if set(commands) != expected:
        raise ValueError(
            f"commands must match topology panes exactly: expected {sorted(expected)}"
        )
    normalized = {name: tuple(argv) for name, argv in commands.items()}
    if any(not argv or not argv[0] for argv in normalized.values()):
        raise ValueError("every pane command must contain an executable")
    return normalized


def _pane_names(node: TopologyNode) -> list[str]:
    if isinstance(node, Pane):
        return [node.name]
    return [*_pane_names(node.first), *_pane_names(node.second)]


def _first_pane(node: TopologyNode) -> str:
    return node.name if isinstance(node, Pane) else _first_pane(node.first)


def _node_dict(node: TopologyNode) -> dict[str, object]:
    if isinstance(node, Pane):
        return {"kind": "pane", "name": node.name}
    return {
        "kind": "split",
        "direction": node.direction,
        "ratio_milli": node.ratio_milli,
        "first": _node_dict(node.first),
        "second": _node_dict(node.second),
    }


def _kdl_quote(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)
