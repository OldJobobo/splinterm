#!/usr/bin/env python3
import json, os, pathlib, random, statistics, subprocess, sys, time
HERE = pathlib.Path(__file__).resolve()
ROOT = next((p for p in HERE.parents if (p/'Cargo.toml').is_file() and (p/'tools/benchmark/run-graphical-retention-v2.py').is_file()), None)
if ROOT is None: raise RuntimeError('repository root not found')
OUT=HERE.parent; RUNNER=ROOT/'tools/benchmark/run-graphical-retention-v2.py'; SEED=1106; WARMUPS=2; SAMPLES=10
BINARIES={'control':pathlib.Path('/tmp/splinterm-plan0011-final-control-target/release'),'candidate':pathlib.Path('/tmp/splinterm-plan0011-final-candidate-bin')}
state={'schema':'splinterm.plan0011.matrix-progress.v1','seed':SEED,'status':'running','attempts':[]}
def save(): (OUT/'progress.json').write_text(json.dumps(state,indent=2,sort_keys=True)+'\n')
def fail(msg): state.update(status='aborted',failure=msg); save(); raise RuntimeError(msg)
for variant,base in BINARIES.items():
 missing=[n for n in ('splinterm','splinterd','splinterm-pty-child') if not (base/n).is_file()]
 if missing: fail(f'{variant} missing binaries: {missing}')
rng=random.Random(SEED)
for phase,count in (('warmup',WARMUPS),('measured',SAMPLES)):
 cases=[(v,i) for i in range(count) for v in BINARIES]; rng.shuffle(cases)
 for variant,iteration in cases:
  directory=OUT/'raw'/phase/f'{iteration:02d}'/variant; directory.mkdir(parents=True,exist_ok=True)
  attempt={'phase':phase,'iteration':iteration,'variant':variant,'status':'running','started_ns':time.time_ns(),'output':str(directory.relative_to(OUT))}; state['attempts'].append(attempt); save()
  base=BINARIES[variant]; env=os.environ.copy(); env.update(SPLINTERBENCH_SPLINTERM_CLIENT=str(base/'splinterm'),SPLINTERBENCH_SPLINTERM_DAEMON=str(base/'splinterd'),SPLINTERM_PTY_HELPER=str(base/'splinterm-pty-child'))
  cp=subprocess.run([sys.executable,str(RUNNER),str(directory),'--terminal','splinterm','--variant',variant,'--lines','5000','--settle-points','2'],cwd=ROOT,env=env,text=True,capture_output=True,timeout=60)
  (directory/'runner.stdout').write_text(cp.stdout); (directory/'runner.stderr').write_text(cp.stderr); path=directory/'splinterm-retention.json'
  attempt.update(status='finished',returncode=cp.returncode,finished_ns=time.time_ns(),result_exists=path.exists())
  if not path.exists(): save(); fail(f'{variant} {phase} {iteration} produced no result')
  result=json.loads(path.read_text()); attempt.update(valid=result.get('valid'),notes=result.get('notes',[]),failure=result.get('failure')); save()
  if cp.returncode or not result.get('valid') or not result.get('isolation',{}).get('cleanup_verified'): fail(f'{variant} {phase} {iteration} failed guard')
summary={}
for variant in BINARIES:
 results=[json.loads(p.read_text()) for p in (OUT/'raw'/'measured').glob(f'*/{variant}/splinterm-retention.json')]
 if len(results)!=SAMPLES or any(not d.get('valid') for d in results): fail(f'{variant} measured set incomplete')
 summary[variant]={}
 for key in ('retained_growth_bytes','rss_post_settle_bytes','trigger_to_visible_marker_ns','cpu_ticks'):
  vals=[d['retention'][key] for d in results]; summary[variant][key]={'median':statistics.median(vals),'minimum':min(vals),'maximum':max(vals)}
control=summary['control']['retained_growth_bytes']['median']; candidate=summary['candidate']['retained_growth_bytes']['median']
report={'schema':'splinterm.plan0011.graphical-control-candidate.v3','seed':SEED,'warmups_per_variant':WARMUPS,'samples_per_variant':SAMPLES,'summary':summary,'candidate_reduction_percent':(control-candidate)*100/control,'improvement_gate_percent':40.0,'improvement_established':candidate<=control*.60,'valid':True}
(OUT/'summary.json').write_text(json.dumps(report,indent=2,sort_keys=True)+'\n'); state['status']='complete'; save(); print(json.dumps(report,indent=2,sort_keys=True))
