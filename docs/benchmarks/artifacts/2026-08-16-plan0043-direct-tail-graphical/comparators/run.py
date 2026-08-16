#!/usr/bin/env python3
import json, os, pathlib, random, statistics, subprocess, sys
ROOT=pathlib.Path('/home/oldjobobo/Projects/splinterm-worktrees/0043-beta1-sparse-publication-frames')
OUT=pathlib.Path('/tmp/plan0043-direct-tail-graphical/comparators')
RUNNER=ROOT/'tools/benchmark/run-graphical-retention-v2.py'
SEED=4304; WARMUPS=2; SAMPLES=10; TERMS=('foot','kitty','ghostty')
rng=random.Random(SEED); order=[]; measured=[]
for phase,count in (('warmup',WARMUPS),('measured',SAMPLES)):
 cases=[(term,i) for i in range(count) for term in TERMS]; rng.shuffle(cases)
 for term,i in cases:
  order.append({'phase':phase,'iteration':i,'terminal':term})
  directory=OUT/'raw'/phase/f'{i:02d}'/term; directory.mkdir(parents=True,exist_ok=True)
  cp=subprocess.run([sys.executable,str(RUNNER),str(directory),'--terminal',term,'--variant',term,'--lines','5000','--settle-points','2'],cwd=ROOT,text=True,capture_output=True,timeout=60)
  p=directory/f'{term}-retention.json'
  if not p.exists(): raise RuntimeError(f'{term} no result: {cp.stderr}')
  d=json.loads(p.read_text())
  if cp.returncode or not d.get('valid') or not d['isolation']['cleanup_verified']: raise RuntimeError(f'{term} failed {d.get("notes")} {cp.stderr}')
  if phase=='measured': measured.append({'terminal':term,'iteration':i,'result':d})
def med(xs): return {'median':statistics.median(xs),'minimum':min(xs),'maximum':max(xs)}
def pg(d,name,key):
 b=d['memory_timeline']['baseline']['processes']; s=d['memory_timeline']['settles'][-1]['memory']['processes']
 before=sum(x[key] for x in b if x['name']==name); after=sum(x[key] for x in s if x['name']==name)
 return max(0,after-before)
def ag(d,key):
 return max(0,d['memory_timeline']['settles'][-1]['memory']['aggregate'][key]-d['memory_timeline']['baseline']['aggregate'][key])
summary={}
for term in TERMS:
 ds=[x['result'] for x in measured if x['terminal']==term]; s={}
 for key in ('rss_bytes','pss_bytes','private_anon_bytes'): s[f'aggregate_growth_{key}']=med([ag(d,key) for d in ds])
 for key in ('trigger_to_visible_marker_ns','cpu_ticks','context_switches'): s[key]=med([d['retention'][key] for d in ds])
 summary[term]=s
report={'schema':'splinterm.plan0043.graphical-comparators.v1','seed':SEED,'warmups_per_terminal':WARMUPS,'samples_per_terminal':SAMPLES,'order':order,'summary':summary,'valid':len(measured)==len(TERMS)*SAMPLES}
(OUT/'summary.json').write_text(json.dumps(report,indent=2,sort_keys=True)+'\n'); print(json.dumps(report,indent=2,sort_keys=True))
