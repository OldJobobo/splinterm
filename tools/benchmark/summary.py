"""Transparent summary statistics for retained benchmark samples."""

from __future__ import annotations

import statistics
from collections import defaultdict
from collections.abc import Iterable
from typing import Any


def percentile(values: list[float], percent: int) -> float:
    """Return the nearest-rank percentile used by the existing Phase 9 reports."""

    if not values:
        raise ValueError("cannot summarize an empty sample")
    ordered = sorted(values)
    index = (len(ordered) - 1) * percent // 100
    return ordered[index]


def summarize_values(values: Iterable[int | float]) -> dict[str, int | float]:
    ordered = sorted(values)
    if not ordered:
        raise ValueError("cannot summarize an empty sample")
    median = statistics.median(ordered)
    deviations = [abs(value - median) for value in ordered]
    return {
        "count": len(ordered),
        "min": ordered[0],
        "median": median,
        "p95": percentile([float(value) for value in ordered], 95),
        "max": ordered[-1],
        "median_absolute_deviation": statistics.median(deviations),
    }


def summarize_samples(samples: list[dict[str, Any]]) -> list[dict[str, Any]]:
    groups: dict[tuple[str, str, str, str], list[int | float]] = defaultdict(list)
    invalid: dict[tuple[str, str, str], int] = defaultdict(int)
    for sample in samples:
        key = (sample["terminal"], sample["case"], sample["boundary"])
        if not sample.get("valid", False):
            invalid[key] += 1
            continue
        for metric, value in sample.get("metrics", {}).items():
            if isinstance(value, (int, float)) and not isinstance(value, bool):
                groups[(*key, metric)].append(value)
    result = []
    represented: set[tuple[str, str, str]] = set()
    for (terminal, case, boundary, metric), values in sorted(groups.items()):
        key = (terminal, case, boundary)
        represented.add(key)
        result.append(
            {
                "terminal": terminal,
                "case": case,
                "boundary": boundary,
                "metric": metric,
                "statistics": summarize_values(values),
                "invalid_samples": invalid[key],
            }
        )
    for terminal, case, boundary in sorted(set(invalid) - represented):
        result.append(
            {
                "terminal": terminal,
                "case": case,
                "boundary": boundary,
                "metric": None,
                "statistics": None,
                "invalid_samples": invalid[(terminal, case, boundary)],
            }
        )
    return result
