"""Linux process-tree and cgroup-v2 resource snapshots."""

from __future__ import annotations

import dataclasses
import pathlib


@dataclasses.dataclass(frozen=True)
class ProcessMetrics:
    process_count: int = 0
    cpu_ticks: int = 0
    rss_bytes: int = 0
    context_switches: int = 0

    def as_dict(self) -> dict[str, int]:
        return dataclasses.asdict(self)


def _children(proc_root: pathlib.Path, pid: int) -> list[int]:
    path = proc_root / str(pid) / "task" / str(pid) / "children"
    try:
        return [int(value) for value in path.read_text(encoding="utf-8").split()]
    except (OSError, ValueError):
        return []


def process_tree(proc_root: pathlib.Path, root_pid: int) -> list[int]:
    """Return a stable root-first snapshot of currently visible descendants."""

    found: list[int] = []
    seen: set[int] = set()
    pending = [root_pid]
    while pending:
        pid = pending.pop(0)
        if pid in seen or not (proc_root / str(pid)).exists():
            continue
        seen.add(pid)
        found.append(pid)
        pending.extend(_children(proc_root, pid))
    return found


def _process_metrics(proc_root: pathlib.Path, pid: int) -> ProcessMetrics | None:
    try:
        stat = (proc_root / str(pid) / "stat").read_text(encoding="utf-8")
        status = (proc_root / str(pid) / "status").read_text(encoding="utf-8")
        fields = stat[stat.rfind(")") + 2 :].split()
        cpu_ticks = int(fields[11]) + int(fields[12])
    except (OSError, ValueError, IndexError):
        return None
    rss_bytes = 0
    context_switches = 0
    for line in status.splitlines():
        if line.startswith("VmRSS:"):
            rss_bytes = int(line.split()[1]) * 1024
        elif line.startswith(
            ("voluntary_ctxt_switches:", "nonvoluntary_ctxt_switches:")
        ):
            context_switches += int(line.split()[1])
    return ProcessMetrics(1, cpu_ticks, rss_bytes, context_switches)


def snapshot_process_forest(
    root_pids: list[int], proc_root: pathlib.Path = pathlib.Path("/proc")
) -> ProcessMetrics:
    """Aggregate unique processes reachable from one or more architecture roots."""

    pids = {pid for root_pid in root_pids for pid in process_tree(proc_root, root_pid)}
    total = ProcessMetrics()
    for pid in sorted(pids):
        item = _process_metrics(proc_root, pid)
        if item is None:
            continue
        total = ProcessMetrics(
            process_count=total.process_count + item.process_count,
            cpu_ticks=total.cpu_ticks + item.cpu_ticks,
            rss_bytes=total.rss_bytes + item.rss_bytes,
            context_switches=total.context_switches + item.context_switches,
        )
    return total


def snapshot_process_tree(
    root_pid: int, proc_root: pathlib.Path = pathlib.Path("/proc")
) -> ProcessMetrics:
    return snapshot_process_forest([root_pid], proc_root)


def read_cgroup_v2(path: pathlib.Path) -> dict[str, int | None]:
    """Read common cgroup-v2 counters without creating or mutating a cgroup."""

    def integer(name: str) -> int | None:
        try:
            value = (path / name).read_text(encoding="utf-8").strip()
            return None if value == "max" else int(value)
        except (OSError, ValueError):
            return None

    cpu: dict[str, int] = {}
    try:
        for line in (path / "cpu.stat").read_text(encoding="utf-8").splitlines():
            key, value = line.split()
            cpu[key] = int(value)
    except (OSError, ValueError):
        pass
    return {
        "memory_current_bytes": integer("memory.current"),
        "memory_peak_bytes": integer("memory.peak"),
        "process_count": integer("pids.current"),
        "cpu_usage_usec": cpu.get("usage_usec"),
        "cpu_user_usec": cpu.get("user_usec"),
        "cpu_system_usec": cpu.get("system_usec"),
    }
