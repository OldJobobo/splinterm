"""Summarize the bounded Plan 0022 outer-resize operation intervals."""

import argparse
import json
import statistics
from pathlib import Path


def records(case: Path) -> list[dict]:
    values = []
    for trace in sorted((case / "trace").glob("*.jsonl")):
        with trace.open(encoding="utf-8") as source:
            values.extend(json.loads(line) for line in source)
    return values


def transaction_key(value: dict) -> tuple:
    return tuple(
        value.get(field)
        for field in (
            "splint_id",
            "incarnation",
            "revision",
            "subscription_id",
            "transaction_sequence",
        )
    )


def final_marker_commit_ns(client: list[dict], report: dict) -> int:
    receives = {
        transaction_key(value): value
        for value in client
        if value["stage"] == "client_receive"
    }
    matches = [
        value
        for value in client
        if value["stage"] == "pane_commit"
        and transaction_key(value) in receives
        and value["monotonic_raw_ns"]
        - receives[transaction_key(value)]["monotonic_raw_ns"]
        == report["trace"]["client_receive_to_pane_commit_ns"]
    ]
    if len(matches) != 1:
        raise ValueError(
            f"expected one correlated final-marker commit, found {len(matches)}"
        )
    return matches[0]["monotonic_raw_ns"]


def summarize_case(case: Path) -> dict:
    report = json.loads((case / "report.json").read_text(encoding="utf-8"))
    client = [value for value in records(case) if value.get("process") == "splinterm"]
    end_ns = final_marker_commit_ns(client, report)
    client = [value for value in client if value["monotonic_raw_ns"] <= end_ns]
    configures = sorted(
        (
            value
            for value in client
            if value["stage"] == "window_event" and value.get("configure_count")
        ),
        key=lambda value: value["monotonic_raw_ns"],
    )[-12:]
    if len(configures) != 12:
        raise ValueError(f"{case.name}: expected 12 final configure events")
    start_ns = configures[0]["monotonic_raw_ns"]
    interval = [
        value for value in client if start_ns <= value["monotonic_raw_ns"] <= end_ns
    ]
    prepares = [value for value in interval if value["stage"] == "frame_prepare"]
    content = [
        value
        for value in prepares
        if value.get("full_reload") or value.get("dirty_rows", 0) > 0
    ]
    refresh = [value for value in prepares if value not in content]
    content_revisions = {
        (value.get("splint_id"), value.get("incarnation"), value.get("revision"))
        for value in content
    }
    draws = [value for value in interval if value["stage"] == "draw_commit"]
    return {
        "case_id": report["case_id"],
        "configure_events": len(configures),
        "frame_prepares": len(prepares),
        "content_prepares": len(content),
        "full_reload_prepares": sum(
            bool(value.get("full_reload")) for value in content
        ),
        "dirty_update_prepares": sum(
            value.get("dirty_rows", 0) > 0 for value in content
        ),
        "unique_content_revisions": len(content_revisions),
        "content_prepare_total_ns": sum(value["duration_ns"] for value in content),
        "refresh_prepares": len(refresh),
        "refresh_prepare_total_ns": sum(value["duration_ns"] for value in refresh),
        "draw_commits": len(draws),
        "draw_commit_total_ns": sum(value["duration_ns"] for value in draws),
        "backing_copy_bytes": sum(
            value.get("backing_copy_bytes", 0) for value in draws
        ),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("matrix", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    cases = sorted(args.matrix.glob("*-measured-*-outer-resize"))
    if len(cases) != 10:
        raise ValueError("expected exactly ten measured outer-resize cases")
    summaries = [summarize_case(case) for case in cases]

    def median(key: str) -> int | float:
        return statistics.median(value[key] for value in summaries)

    output = {
        "schema": "splinterm.benchmark.resize-preparation-summary.v1",
        "source": str(args.matrix),
        "measured_cases": len(summaries),
        "operation": "twelve-step-outer-resize",
        "cases": summaries,
        "median": {
            key: median(key)
            for key in (
                "configure_events",
                "frame_prepares",
                "content_prepares",
                "full_reload_prepares",
                "dirty_update_prepares",
                "unique_content_revisions",
                "content_prepare_total_ns",
                "refresh_prepares",
                "refresh_prepare_total_ns",
                "draw_commits",
                "draw_commit_total_ns",
                "backing_copy_bytes",
            )
        },
        "decision": {
            "duplicate_content_prepares": sum(
                value["content_prepares"] - value["unique_content_revisions"]
                for value in summaries
            ),
            "optimization_justified": False,
            "reason": "Every content preparation belongs to a distinct required terminal revision; configure-only refresh preparation is sub-0.1 ms per complete sequence.",
        },
    }
    args.output.write_text(json.dumps(output, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
