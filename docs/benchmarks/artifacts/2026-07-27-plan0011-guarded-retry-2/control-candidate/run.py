#!/usr/bin/env python3
import json, os, pathlib, random, statistics, subprocess, sys, time

ROOT = pathlib.Path(__file__).resolve().parents[5]
OUT = pathlib.Path(__file__).resolve().parent
RUNNER = ROOT / "tools/benchmark/run-graphical-retention-v2.py"
SEED, WARMUPS, SAMPLES = 1105, 2, 10
BINARIES = {
    "control": pathlib.Path("/tmp/splinterm-plan0011-final-control-target/release"),
    "candidate": pathlib.Path("/tmp/splinterm-plan0011-final-candidate-bin"),
}
state = {"schema":"splinterm.plan0011.matrix-progress.v1","seed":SEED,"status":"running","attempts":[]}
def save():
    (OUT / "progress.json").write_text(json.dumps(state, indent=2, sort_keys=True) + "\n")
def fail(message):
    state["status"]="aborted"; state["failure"]=message; save(); raise RuntimeError(message)

if not RUNNER.is_file() or not (ROOT / "AGENTS.md").is_file():
    fail(f"invalid repository root or missing runner: {ROOT}")
for variant, base in BINARIES.items():
    missing = [name for name in ("splinterm", "splinterd", "splinterm-pty-child") if not (base / name).is_file()]
    if missing:
        fail(f"{variant} missing binaries: {missing}")

rng = random.Random(SEED)
for phase, count in (("warmup", WARMUPS), ("measured", SAMPLES)):
    cases = [(variant, iteration) for iteration in range(count) for variant in BINARIES]
    rng.shuffle(cases)
    for variant, iteration in cases:
        directory = OUT / "raw" / phase / f"{iteration:02d}" / variant
        directory.mkdir(parents=True, exist_ok=True)
        attempt = {"phase":phase,"iteration":iteration,"variant":variant,"status":"running","started_ns":time.time_ns(),"output":str(directory.relative_to(OUT))}
        state["attempts"].append(attempt); save()
        base = BINARIES[variant]
        env = os.environ.copy()
        env.update(SPLINTERBENCH_SPLINTERM_CLIENT=str(base/"splinterm"), SPLINTERBENCH_SPLINTERM_DAEMON=str(base/"splinterd"), SPLINTERM_PTY_HELPER=str(base/"splinterm-pty-child"))
        completed = subprocess.run([sys.executable,str(RUNNER),str(directory),"--terminal","splinterm","--variant",variant,"--lines","5000","--settle-points","2"],cwd=ROOT,env=env,text=True,capture_output=True,timeout=60)
        (directory/"runner.stdout").write_text(completed.stdout); (directory/"runner.stderr").write_text(completed.stderr)
        path=directory/"splinterm-retention.json"
        attempt.update(status="finished",returncode=completed.returncode,finished_ns=time.time_ns(),result_exists=path.exists())
        if not path.exists(): save(); fail(f"{variant} {phase} {iteration} produced no result")
        result=json.loads(path.read_text()); attempt.update(valid=result.get("valid"),notes=result.get("notes",[]),failure=result.get("failure")); save()
        if completed.returncode or not result.get("valid") or not result.get("isolation",{}).get("cleanup_verified"):
            fail(f"{variant} {phase} {iteration} failed guard")

summary={}
for variant in BINARIES:
    results=[]
    for p in (OUT/"raw"/"measured").glob(f"*/{variant}/splinterm-retention.json"):
        d=json.loads(p.read_text());
        if d.get("valid"): results.append(d)
    if len(results)!=SAMPLES: fail(f"{variant} has {len(results)} valid samples, expected {SAMPLES}")
    summary[variant]={}
    for key in ("retained_growth_bytes","rss_post_settle_bytes","trigger_to_visible_marker_ns","cpu_ticks"):
        values=[d["retention"][key] for d in results]
        summary[variant][key]={"median":statistics.median(values),"minimum":min(values),"maximum":max(values)}
control=summary["control"]["retained_growth_bytes"]["median"]
candidate=summary["candidate"]["retained_growth_bytes"]["median"]
report={"schema":"splinterm.plan0011.graphical-control-candidate.v2","seed":SEED,"warmups_per_variant":WARMUPS,"samples_per_variant":SAMPLES,"summary":summary,"candidate_reduction_percent":(control-candidate)*100/control,"improvement_gate_percent":40.0,"improvement_established":candidate <= control*0.60,"valid":True}
(OUT/"summary.json").write_text(json.dumps(report,indent=2,sort_keys=True)+"\n")
state["status"]="complete"; save(); print(json.dumps(report,indent=2,sort_keys=True))
