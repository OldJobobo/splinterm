"""Linux process-tree and cgroup-v2 resource snapshots."""

from __future__ import annotations

import dataclasses
import pathlib
from typing import Any


@dataclasses.dataclass(frozen=True)
class ProcessMetrics:
    process_count: int = 0
    cpu_ticks: int = 0
    rss_bytes: int = 0
    context_switches: int = 0

    def as_dict(self) -> dict[str, int]:
        return dataclasses.asdict(self)


@dataclasses.dataclass(frozen=True)
class ProcessMemory:
    """Body-free per-process residency attribution from procfs."""

    pid: int
    name: str
    rss_bytes: int = 0
    pss_bytes: int = 0
    private_anon_bytes: int = 0
    private_file_bytes: int = 0
    shared_bytes: int = 0
    shmem_bytes: int = 0

    def as_dict(self) -> dict[str, Any]:
        return dataclasses.asdict(self)


def _kib_fields(path: pathlib.Path) -> dict[str, int]:
    values: dict[str, int] = {}
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError:
        return values
    for line in lines:
        if ":" not in line:
            continue
        key, raw = line.split(":", 1)
        fields = raw.split()
        if not fields:
            continue
        try:
            values[key] = int(fields[0]) * 1024
        except ValueError:
            continue
    return values


def process_memory(
    pid: int, proc_root: pathlib.Path = pathlib.Path("/proc")
) -> ProcessMemory | None:
    """Read one process without retaining command arguments or memory bodies.

    ``smaps_rollup`` exposes total anonymous and total private residency but not
    their exact intersection. ``private_anon_bytes`` is therefore the bounded
    intersection of those totals; the remainder is classified as private file.
    Shared residency and shmem mappings are retained separately.
    """

    root = proc_root / str(pid)
    fields = _kib_fields(root / "smaps_rollup")
    if not fields:
        return None
    try:
        name = (root / "comm").read_text(encoding="utf-8").strip()
    except OSError:
        name = "unknown"
    private_total = fields.get("Private_Clean", 0) + fields.get("Private_Dirty", 0)
    private_anon = min(private_total, fields.get("Anonymous", 0))
    return ProcessMemory(
        pid=pid,
        name=name,
        rss_bytes=fields.get("Rss", 0),
        pss_bytes=fields.get("Pss", 0),
        private_anon_bytes=private_anon,
        private_file_bytes=max(0, private_total - private_anon),
        shared_bytes=fields.get("Shared_Clean", 0) + fields.get("Shared_Dirty", 0),
        shmem_bytes=fields.get("ShmemPmdMapped", 0),
    )


def snapshot_process_memory_forest(
    root_pids: list[int], proc_root: pathlib.Path = pathlib.Path("/proc")
) -> dict[str, Any]:
    """Return aggregate and per-process smaps attribution for unique descendants."""

    pids = sorted({pid for root in root_pids for pid in process_tree(proc_root, root)})
    processes = [item for pid in pids if (item := process_memory(pid, proc_root))]
    keys = (
        "rss_bytes",
        "pss_bytes",
        "private_anon_bytes",
        "private_file_bytes",
        "shared_bytes",
        "shmem_bytes",
    )
    return {
        "aggregate": {key: sum(getattr(item, key) for item in processes) for key in keys},
        "processes": [item.as_dict() for item in processes],
    }


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
