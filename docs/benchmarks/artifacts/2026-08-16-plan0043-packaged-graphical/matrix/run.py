#!/usr/bin/env python3
import json
import os
import pathlib
import random
import statistics
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[5]
OUT = pathlib.Path(__file__).resolve().parent
RUNNER = ROOT / "tools/benchmark/run-graphical-retention-v2.py"
SEED = 4304
WARMUPS = 2
SAMPLES = 10
BINARIES = {
    "alpha3.3": pathlib.Path("/usr/bin"),
    "plan0042": pathlib.Path("/tmp/splinterm-plan0043-plan0042-graphical-target/release"),
    "plan0043": pathlib.Path("/tmp/splinterm-plan0043-candidate-graphical-target/release"),
}
HELPERS = {
    "alpha3.3": pathlib.Path("/usr/bin/splinterm-pty-child"),
    "plan0042": BINARIES["plan0042"] / "splinterm-pty-child",
    "plan0043": BINARIES["plan0043"] / "splinterm-pty-child",
}


def median_summary(values):
    return {"median": statistics.median(values), "minimum": min(values), "maximum": max(values)}


def process_growth(result, process_name, key):
    baseline = result["memory_timeline"]["baseline"]["processes"]
    settled = result["memory_timeline"]["settles"][-1]["memory"]["processes"]
    before = next(item[key] for item in baseline if item["name"] == process_name)
    after = next(item[key] for item in settled if item["name"] == process_name)
    return max(0, after - before)


def aggregate_growth(result, key):
    before = result["memory_timeline"]["baseline"]["aggregate"][key]
    after = result["memory_timeline"]["settles"][-1]["memory"]["aggregate"][key]
    return max(0, after - before)


rng = random.Random(SEED)
order = []
measured = []
for phase, count in (("warmup", WARMUPS), ("measured", SAMPLES)):
    cases = [(variant, iteration) for iteration in range(count) for variant in BINARIES]
    rng.shuffle(cases)
    for variant, iteration in cases:
        order.append({"phase": phase, "iteration": iteration, "variant": variant})
        directory = OUT / "raw" / phase / f"{iteration:02d}" / variant
        directory.mkdir(parents=True, exist_ok=True)
        base = BINARIES[variant]
        env = os.environ.copy()
        env.update(
            SPLINTERBENCH_SPLINTERM_CLIENT=str(base / "splinterm"),
            SPLINTERBENCH_SPLINTERM_DAEMON=str(base / "splinterd"),
            SPLINTERM_PTY_HELPER=str(HELPERS[variant]),
        )
        completed = subprocess.run(
            [
                sys.executable,
                str(RUNNER),
                str(directory),
                "--terminal",
                "splinterm",
                "--variant",
                variant,
                "--lines",
                "5000",
                "--settle-points",
                "2",
            ],
            cwd=ROOT,
            env=env,
            text=True,
            capture_output=True,
            timeout=60,
        )
        result_path = directory / "splinterm-retention.json"
        if not result_path.exists():
            raise RuntimeError(f"{variant} produced no result: {completed.stderr}")
        result = json.loads(result_path.read_text())
        if completed.returncode or not result.get("valid") or not result["isolation"]["cleanup_verified"]:
            raise RuntimeError(f"{variant} failed: {result.get('notes')} {completed.stderr}")
        if phase == "measured":
            measured.append({"variant": variant, "iteration": iteration, "result": result})

summary = {}
for variant in BINARIES:
    selected = [item["result"] for item in measured if item["variant"] == variant]
    variant_summary = {}
    for key in ("rss_bytes", "pss_bytes", "private_anon_bytes"):
        variant_summary[f"aggregate_growth_{key}"] = median_summary(
            [aggregate_growth(result, key) for result in selected]
        )
        variant_summary[f"daemon_growth_{key}"] = median_summary(
            [process_growth(result, "splinterd", key) for result in selected]
        )
        variant_summary[f"client_growth_{key}"] = median_summary(
            [process_growth(result, "splinterm", key) for result in selected]
        )
        variant_summary[f"application_growth_{key}"] = median_summary(
            [
                process_growth(result, "splinterd", key)
                + process_growth(result, "splinterm", key)
                for result in selected
            ]
        )
    for key in ("trigger_to_visible_marker_ns", "cpu_ticks", "context_switches"):
        variant_summary[key] = median_summary([result["retention"][key] for result in selected])
    summary[variant] = variant_summary

alpha = summary["alpha3.3"]["application_growth_rss_bytes"]["median"]
baseline = summary["plan0042"]["application_growth_rss_bytes"]["median"]
candidate = summary["plan0043"]["application_growth_rss_bytes"]["median"]
report = {
    "schema": "splinterm.plan0043.graphical-three-variant.v1",
    "seed": SEED,
    "warmups_per_variant": WARMUPS,
    "samples_per_variant": SAMPLES,
    "order": order,
    "summary": summary,
    "candidate_application_rss_reduction_vs_alpha_percent": (alpha - candidate) * 100.0 / alpha if alpha else 0,
    "candidate_application_rss_reduction_vs_plan0042_percent": (baseline - candidate) * 100.0 / baseline if baseline else 0,
    "valid": len(measured) == len(BINARIES) * SAMPLES,
}
(OUT / "summary.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
print(json.dumps(report, indent=2, sort_keys=True))
