#!/usr/bin/env python3
import json, os, pathlib, random, statistics, subprocess, sys

ROOT = pathlib.Path(__file__).resolve().parents[6]
OUT = pathlib.Path(__file__).resolve().parent
RUNNER = ROOT / "tools/benchmark/run-graphical-retention-v2.py"
SEED = 1104
WARMUPS = 2
SAMPLES = 10
BINARIES = {
    "control": pathlib.Path("/tmp/splinterm-plan0011-final-control-target/release"),
    "candidate": pathlib.Path("/tmp/splinterm-plan0011-final-candidate-bin"),
}
rng = random.Random(SEED)
order, measured = [], []
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
            SPLINTERM_PTY_HELPER=str(base / "splinterm-pty-child"),
        )
        completed = subprocess.run(
            [sys.executable, str(RUNNER), str(directory), "--terminal", "splinterm",
             "--variant", variant, "--lines", "5000", "--settle-points", "2"],
            cwd=ROOT, env=env, text=True, capture_output=True, timeout=60,
        )
        path = directory / "splinterm-retention.json"
        if not path.exists():
            raise RuntimeError(f"{variant} produced no result: {completed.stderr}")
        result = json.loads(path.read_text())
        if completed.returncode or not result.get("valid") or not result["isolation"]["cleanup_verified"]:
            raise RuntimeError(f"{variant} failed: {result.get('notes')} {completed.stderr}")
        if phase == "measured":
            measured.append({"variant": variant, "iteration": iteration, "result": result})
summary = {}
for variant in BINARIES:
    selected = [x["result"] for x in measured if x["variant"] == variant]
    summary[variant] = {}
    for key in ("retained_growth_bytes", "rss_post_settle_bytes", "trigger_to_visible_marker_ns", "cpu_ticks"):
        values = [x["retention"][key] for x in selected]
        summary[variant][key] = {"median": statistics.median(values), "minimum": min(values), "maximum": max(values)}
    for key in ("private_anon_bytes", "pss_bytes", "rss_bytes"):
        values = [x["memory_timeline"]["settles"][-1]["memory"]["aggregate"][key] - x["memory_timeline"]["baseline"]["aggregate"][key] for x in selected]
        summary[variant][f"growth_{key}"] = {"median": statistics.median(values), "minimum": min(values), "maximum": max(values)}
control = summary["control"]["retained_growth_bytes"]["median"]
candidate = summary["candidate"]["retained_growth_bytes"]["median"]
report = {
    "schema": "splinterm.plan0011.graphical-control-candidate.v1",
    "seed": SEED,
    "warmups_per_variant": WARMUPS,
    "samples_per_variant": SAMPLES,
    "order": order,
    "summary": summary,
    "candidate_reduction_percent": (control - candidate) * 100.0 / control if control else 0,
    "valid": len(measured) == 2 * SAMPLES,
}
(OUT / "summary.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
print(json.dumps(report, indent=2, sort_keys=True))
