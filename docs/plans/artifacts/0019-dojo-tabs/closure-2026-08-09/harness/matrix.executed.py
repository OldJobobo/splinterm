#!/usr/bin/env python3
from __future__ import annotations
import hashlib, json, os, pathlib, shlex, shutil, signal, statistics, subprocess, sys, time
from evdev import UInput, ecodes
ROOT=pathlib.Path('/home/oldjobobo/Projects/splinterm')
BIN=ROOT/'target/release'
sys.path.insert(0,str(ROOT/'tools'))
from phase10_support import V1, run, wait_until, output_screenshot
OUT=pathlib.Path('/tmp/splinterm-plan0019-matrix').resolve(); APP_ID='com.oldjobobo.splinterm'
RESTORE_COMMAND="hl.monitor({ output = 'DP-2', mode = '1920x1080@60', position = '640x-1080', scale = 1.0, transform = 0 })"
shutil.rmtree(OUT,ignore_errors=True); (OUT/'config/splinterm').mkdir(parents=True); (OUT/'runtime').mkdir(mode=0o700); (OUT/'state').mkdir(); (OUT/'trace').mkdir()
DARK={'background':'#0e1216','alpha':1.0,'foreground':'#ebebeb','cursor':'#78d2ff','selection':'#354a60','selection_foreground':'#ffffff','url':'#78beff','ui_accent':'#78d2ff','pane_border':'#928374','pane_border_active':'#78d2ff','ansi':['#1d2021','#cc241d','#98971a','#d79921','#458588','#b16286','#689d6a','#a89984','#928374','#fb4934','#b8bb26','#fabd2f','#83a598','#d3869b','#8ec07c','#ebdbb2']}
LIGHT={'background':'#f3efe7','alpha':0.78,'foreground':'#24211d','cursor':'#005f87','selection':'#c4d8e8','selection_foreground':'#111111','url':'#005f87','ui_accent':'#006b8f','pane_border':'#8b8175','pane_border_active':'#006b8f','ansi':['#24211d','#a33b36','#587a3a','#a06400','#266b82','#76528b','#2b7772','#d6cec2','#6f675e','#c24b45','#6f963f','#c17a00','#3285a0','#9368aa','#37948d','#f3efe7']}
theme_path=OUT/'config/splinterm/theme.json'; theme_path.write_text(json.dumps(DARK,indent=2)+'\n')
shell_wrapper=OUT/'clean-shell.sh'; shell_wrapper.write_text('#!/bin/sh\nexec /usr/bin/bash --noprofile --norc\n'); shell_wrapper.chmod(0o700)
(OUT/'config/splinterm/config.ini').write_text(f'''[main]\nfont=JetBrains Mono Nerd Font:style=Regular\nfont-pixelsize=14\nfont-sizing-policy=output-scale\npadding-left=12\npadding-right=12\npadding-top=12\npadding-bottom=12\ninitial-columns=80\ninitial-rows=24\nlogin-shell=no\nshell={shell_wrapper}\nresize-delay-ms=0\ntheme={theme_path}\n\n[scrollback]\nlines=4096\n\n[multiplexer]\ndivider-style=line\nframe-title=splint\n\n[cursor]\nstyle=block\nblink=no\n''')
env=os.environ.copy(); env.update(SPLINTERM_SOCKET=str(OUT/'runtime/splinterd.sock'),SPLINTERM_ENABLE_DEV_ATTACH='1',SPLINTERM_PERF_TRACE_DIR=str(OUT/'trace'),SPLINTERM_PERF_RUN_ID='plan0019matrix',SPLINTERM_PERF_TRACE_MAX_EVENTS='65536',XDG_CONFIG_HOME=str(OUT/'config'),XDG_STATE_HOME=str(OUT/'state'))
focus_before=V1.hyprland_json('activewindow'); cursor_before=V1.hyprland_json('cursorpos'); monitors_before=V1.hyprland_json('monitors all')
V1.assert_test_workspace_isolated(); V1.assert_user_workspace_untouched(); (OUT/'pre-state.json').write_text(json.dumps({'focus':focus_before,'cursor':cursor_before,'monitors':monitors_before,'clients':V1.all_clients()},indent=2)+'\n')
daemon_log=(OUT/'daemon.log').open('w'); daemon=subprocess.Popen([str(BIN/'splinterd')],env=env,stdin=subprocess.DEVNULL,stdout=daemon_log,stderr=subprocess.STDOUT,start_new_session=True,text=True)
address=None; pid=None; pointer=None; results={}

def cli(*args,timeout=15,check=True):
 p=subprocess.run([str(BIN/'splinterm'),*args],env=env,capture_output=True,text=True,timeout=timeout)
 if check and p.returncode: raise RuntimeError(f"CLI {' '.join(args)} failed: {p.stderr or p.stdout}")
 return p

def topology(): return json.loads(cli('--output','json','topology').stdout)['data']
def snapshot_text(splint_id):
 data=json.loads(cli('--output','json','snapshot',splint_id).stdout)['data']
 return '\n'.join(''.join(cell.get('text',cell.get('content','')) for cell in row['cells']) for row in data['rows'])
def counts():
 t=topology(); return len(t['lairs']),len(t['dojos']),len(t['splints']),t

def exact_window():
 m=[c for c in V1.all_clients() if c.get('address')==address and c.get('pid')==pid]
 if len(m)!=1: raise RuntimeError(f'exact Window mismatch {address}/{pid}: {m}')
 w=m[0]
 if w['workspace']['id']!=8 or w['monitor']!=V1.test_monitor_id(): raise RuntimeError(f'placement drift: {w}')
 return w

def focus_exact():
 exact_window(); p=run(['hyprctl','dispatch',f'hl.dsp.focus({{ window = "address:{address}" }})'],capture_output=True,timeout=5)
 if p.returncode: raise RuntimeError(f'focus failed: {p.stderr or p.stdout}')
 wait_until(lambda: V1.hyprland_json('activewindow').get('address')==address and V1.hyprland_json('activewindow').get('pid')==pid,5,'exact Window did not focus')

def restore_focus():
 if not any(c.get('address')==focus_before.get('address') for c in V1.all_clients()): raise RuntimeError('original focus Window disappeared')
 p=run(['hyprctl','dispatch',f'hl.dsp.focus({{ window = "address:{focus_before["address"]}" }})'],capture_output=True,timeout=5)
 if p.returncode: raise RuntimeError(f'focus restore failed: {p.stderr or p.stdout}')
 wait_until(lambda: V1.hyprland_json('activewindow').get('address')==focus_before.get('address'),5,'original focus not restored')

def wait_exact(predicate,timeout,message):
 deadline=time.monotonic()+timeout
 while time.monotonic()<deadline:
  exact_window()
  active=V1.hyprland_json('activewindow')
  if active.get('address')!=address or active.get('pid')!=pid: raise RuntimeError(f'exact focused target changed during wait: {active}')
  if predicate(): return
  time.sleep(.1)
 raise RuntimeError(message)

def dojo_count_probe():
 p=cli('--output','json','topology',check=False)
 if p.returncode: return None
 try: return len(json.loads(p.stdout)['data']['dojos'])
 except (KeyError,TypeError,json.JSONDecodeError): return None

def wait_dojo_count(expected,timeout=15):
 wait_exact(lambda: dojo_count_probe()==expected,timeout,f'Dojo count {expected} did not settle')

def key(*args):
 p=run(['wtype',*args],capture_output=True,timeout=5)
 if p.returncode: raise RuntimeError(f'wtype failed: {p.stderr or p.stdout}')

def chord(key_name,shift=True):
 args=['-M','ctrl']
 if shift: args += ['-M','shift']
 args += ['-k',key_name]
 if shift: args += ['-m','shift']
 args += ['-m','ctrl']; key(*args); time.sleep(.7)

def type_command(command):
 key(command); key('-k','Return'); time.sleep(.8)

def capture(name): return output_screenshot(OUT/f'{name}.png')
def move_cursor(x,y):
 p=run(['hyprctl','eval',f'hl.dispatch(hl.dsp.cursor.move({{ x = {int(x)}, y = {int(y)}, relative = false }}))'],capture_output=True,timeout=5)
 if p.returncode: raise RuntimeError(f'cursor move failed: {p.stderr or p.stdout}')
 wait_until(lambda: abs(V1.hyprland_json('cursorpos')['x']-x)<=2 and abs(V1.hyprland_json('cursorpos')['y']-y)<=2,3,'cursor move mismatch')
def click(): pointer.write(ecodes.EV_KEY,ecodes.BTN_LEFT,1); pointer.syn(); time.sleep(.12); pointer.write(ecodes.EV_KEY,ecodes.BTN_LEFT,0); pointer.syn(); time.sleep(.6)

def monitor_scale(scale):
 code=f"hl.monitor({{ output = 'DP-2', mode = '1920x1080@60', position = '640x-1080', scale = {scale}, transform = 0 }})"
 p=run(['hyprctl','eval',code],capture_output=True,timeout=8)
 if p.returncode: raise RuntimeError(f'scale {scale} failed: {p.stderr or p.stdout}')
 deadline=time.monotonic()+8
 while time.monotonic()<deadline:
  if address is not None: exact_window()
  if abs(next(m for m in V1.hyprland_json('monitors all') if m['name']=='DP-2')['scale']-scale)<.01: return
  time.sleep(.1)
 raise RuntimeError(f'scale {scale} did not settle')

def resize_exact(width,height):
 w=exact_window(); sel=json.dumps(f'address:{address}')
 p=run(['hyprctl','eval',f'hl.dispatch(hl.dsp.window.resize({{ x = {width}, y = {height}, window = {sel} }}))'],capture_output=True,timeout=5)
 if p.returncode: raise RuntimeError(f'resize failed: {p.stderr or p.stdout}')
 deadline=time.monotonic()+8
 while time.monotonic()<deadline:
  if exact_window()['size']==[width,height]: return
  time.sleep(.1)
 raise RuntimeError(f'resize did not settle: {exact_window()["size"]}')

def move_exact(x,y):
 exact_window(); sel=json.dumps(f'address:{address}')
 p=run(['hyprctl','eval',f'hl.dispatch(hl.dsp.window.move({{ x = {x}, y = {y}, relative = false, window = {sel} }}))'],capture_output=True,timeout=5)
 if p.returncode: raise RuntimeError(f'move failed: {p.stderr or p.stdout}')
 deadline=time.monotonic()+8
 while time.monotonic()<deadline:
  if exact_window()['at']==[x,y]: return
  time.sleep(.1)
 raise RuntimeError(f'move did not settle: {exact_window()["at"]}')

def resource_sample(tab_count):
 exact_window(); status=(pathlib.Path('/proc')/str(pid)/'status').read_text(); stat=(pathlib.Path('/proc')/str(pid)/'stat').read_text().split()
 rss_kib=int(next(line.split()[1] for line in status.splitlines() if line.startswith('VmRSS:'))); ticks_before=int(stat[13])+int(stat[14]); started=time.monotonic(); time.sleep(2); exact_window(); stat_after=(pathlib.Path('/proc')/str(pid)/'stat').read_text().split(); ticks_after=int(stat_after[13])+int(stat_after[14])
 return {'tabs':tab_count,'rss_kib':rss_kib,'idle_seconds':time.monotonic()-started,'idle_cpu_ticks':ticks_after-ticks_before,'threads':len(list((pathlib.Path('/proc')/str(pid)/'task').iterdir())),'fds':len(list((pathlib.Path('/proc')/str(pid)/'fd').iterdir()))}

def fast_next():
 started=time.perf_counter_ns(); key('-M','ctrl','-k','Tab','-m','ctrl'); time.sleep(.18); exact_window(); return time.perf_counter_ns()-started

def switch_samples(tab_count,total=12): return {'tabs':tab_count,'wall_ns':[fast_next() for _ in range(total)]}

def trace_summary():
 grouped={}
 for path in sorted((OUT/'trace').glob('*.jsonl')):
  for line in path.read_text().splitlines():
   record=json.loads(line)
   if record.get('process')=='splinterm' and record.get('stage')=='tab_switch' and record.get('duration_ns') is not None:
    count=int(record['count']); grouped.setdefault(count,[]).append(int(record['duration_ns']))
 result={}
 for count,values in grouped.items():
  ordered=sorted(values); p95=ordered[min(len(ordered)-1,max(0,(95*len(ordered)+99)//100-1))]
  result[str(count)]={'samples':len(values),'median_ns':int(statistics.median(values)),'p95_ns':p95,'max_ns':max(values)}
 return result

try:
 wait_until(lambda: pathlib.Path(env['SPLINTERM_SOCKET']).exists() and cli('ping',check=False).returncode==0,8,'isolated daemon not ready')
 monitor_scale(1.2)
 launcher=OUT/'launch.sh'; client_stdout=OUT/'client.stdout'; client_stderr=OUT/'client.stderr'
 child=['/usr/bin/bash','--noprofile','--norc']
 cmd=['env']+[f'{k}={env[k]}' for k in ('SPLINTERM_SOCKET','SPLINTERM_ENABLE_DEV_ATTACH','SPLINTERM_PERF_TRACE_DIR','SPLINTERM_PERF_RUN_ID','SPLINTERM_PERF_TRACE_MAX_EVENTS','XDG_CONFIG_HOME','XDG_STATE_HOME')]+[str(BIN/'splinterm'),'launch','--new','--name','Plan0019-Alpha','--',*child]
 launcher.write_text('#!/bin/sh\nexec '+shlex.join(cmd)+' >'+shlex.quote(str(client_stdout))+' 2>'+shlex.quote(str(client_stderr))+'\n'); launcher.chmod(0o700)
 existing={c['address'] for c in V1.all_clients()}; expr=f"hl.exec_cmd({json.dumps(str(launcher))}, {{ workspace = '8 silent', float = true, size = '960 600', opacity = '1 1', no_initial_focus = true }})"
 p=run(['hyprctl','eval',expr],capture_output=True,timeout=8)
 if p.returncode: raise RuntimeError(f'launch failed: {p.stderr or p.stdout}')
 win=wait_until(lambda: next((c for c in V1.all_clients() if c.get('class')==APP_ID and c.get('address') not in existing),None),12,'matrix Window did not map')
 address=win['address']; pid=win['pid']; exact_window()
 if V1.hyprland_json('activewindow').get('address')!=focus_before.get('address'): raise RuntimeError('initial mapping changed focus')
 wait_until(lambda: counts()[1]==1,8,'initial Dojo absent'); focus_exact(); captures=[]; resources=[]; wall_switches=[]
 captures.append(capture('01-one-tab-dark-opaque-normal-scale120')); resources.append(resource_sample(1)); (OUT/'resource-progress.json').write_text(json.dumps(resources,indent=2)+'\n')
 # Create a second Lair outside the Window, then attach it through the picker.
 beta=json.loads(cli('--output','json','new','Plan0019-Beta','--','/usr/bin/bash','--noprofile','--norc').stdout); beta_splint=beta['resource']['splint_id']; wait_exact(lambda: counts()[0]==2 and counts()[1]==2,8,'second Lair was not created')
 before_attach=topology(); chord('s'); key('j'); key('j'); key('-k','Return'); time.sleep(1); exact_window()
 if topology()!=before_attach: raise RuntimeError('cross-Lair picker attach mutated daemon topology')
 type_command("printf 'CROSS_LAIR_ACTIVE\\n'")
 wait_exact(lambda: snapshot_text(beta_splint).count('CROSS_LAIR_ACTIVE')>=2,8,'picker did not activate the second-Lair tab')
 theme_path.write_text(json.dumps(LIGHT,indent=2)+'\n'); time.sleep(1); monitor_scale(1.5); resize_exact(620,360)
 captures.append(capture('02-two-tabs-cross-lair-light-translucent-compact-scale150')); resources.append(resource_sample(2)); wall_switches.append(switch_samples(2)); (OUT/'resource-progress.json').write_text(json.dumps({'resources':resources,'wall_switches':wall_switches},indent=2)+'\n')
 # Grow transactionally through the trusted New-Dojo binding.
 tab_count=2
 while tab_count < 16:
  chord('d'); tab_count+=1; wait_dojo_count(tab_count); time.sleep(.2)
 captures.append(capture('03-sixteen-tabs-light-translucent-compact-scale150')); resources.append(resource_sample(16)); wall_switches.append(switch_samples(16)); (OUT/'resource-progress.json').write_text(json.dumps({'resources':resources,'wall_switches':wall_switches},indent=2)+'\n')
 dark_translucent=dict(DARK); dark_translucent['alpha']=0.72; theme_path.write_text(json.dumps(dark_translucent,indent=2)+'\n'); time.sleep(1)
 while tab_count < 32:
  chord('d'); tab_count+=1; wait_dojo_count(tab_count); time.sleep(.2)
 monitor_scale(2.4); resize_exact(300,190); move_exact(900,-1000)
 captures.append(capture('04-thirty-two-tabs-dark-translucent-minimal-scale240')); resources.append(resource_sample(32)); wall_switches.append(switch_samples(32)); (OUT/'resource-progress.json').write_text(json.dumps({'resources':resources,'wall_switches':wall_switches},indent=2)+'\n')
 # A 33rd request must be rejected before daemon creation.
 before_limit=topology(); chord('d'); time.sleep(2); exact_window(); after_limit=topology()
 if after_limit!=before_limit: raise RuntimeError('tab 33 mutated daemon topology')
 captures.append(capture('05-tab-limit-32-preserved'))
 # Close one active tab, then every remaining client-local reference. Daemon state must survive.
 chord('q'); time.sleep(.6); exact_window()
 if topology()!=before_limit: raise RuntimeError('active tab close mutated daemon topology')
 captures.append(capture('06-active-tab-removed-topology-retained'))
 for _ in range(30): chord('q'); exact_window()
 captures.append(capture('07-one-tab-after-client-local-closes'))
 chord('q'); wait_until(lambda: not any(c.get('address')==address for c in V1.all_clients()),10,'last tab did not close matrix Window')
 wait_until(lambda: not pathlib.Path('/proc',str(pid)).exists(),8,'matrix client process did not exit')
 final_topology=topology()
 if final_topology!=before_limit or any(x['lifecycle']!='running' for x in final_topology['splints']): raise RuntimeError('final close changed retained daemon topology')
 traces=trace_summary()
 for count in (2,16,32):
  if traces.get(str(count),{}).get('samples',0)<10: raise RuntimeError(f'insufficient tab-switch traces at {count}: {traces}')
 restore_focus()
 results={'schema':'splinterm.plan0019.matrix.v1','exact':True,'git_head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),'dirty_diff_sha256':hashlib.sha256(subprocess.check_output(['git','diff'],cwd=ROOT)).hexdigest(),'binary_sha256':hashlib.sha256((BIN/'splinterm').read_bytes()).hexdigest(),'daemon_sha256':hashlib.sha256((BIN/'splinterd').read_bytes()).hexdigest(),'window':{k:win.get(k) for k in ('address','pid','workspace','monitor','at','size')},'captures':captures,'resource_samples':resources,'wall_switch_samples':wall_switches,'trace_tab_switch_ns':traces,'cross_lair_attachment':True,'beta_splint_id':beta_splint,'tab_33_rejected_without_topology_mutation':True,'active_tab_close_topology_unchanged':True,'last_tab_closed_window':True,'final_daemon_topology':final_topology,'themes':['dark-opaque','light-translucent','dark-translucent'],'scales':[1.2,1.5,2.4],'sizes':['normal','compact','minimal'],'tab_counts':[1,2,16,32]}
 (OUT/'summary.json').write_text(json.dumps(results,indent=2)+'\n')
finally:
 if pointer is not None:
  try: pointer.close()
  except Exception: pass
 if address and any(c.get('address')==address for c in V1.all_clients()): V1.kill_oracle_window(address)
 try: wait_until(lambda: not V1.workspace_clients(8),10,'smoke Window cleanup failed')
 except Exception: pass
 try: os.killpg(daemon.pid,signal.SIGTERM)
 except ProcessLookupError: pass
 try: daemon.wait(timeout=10)
 except subprocess.TimeoutExpired:
  try: os.killpg(daemon.pid,signal.SIGKILL)
  except ProcessLookupError: pass
  daemon.wait(timeout=5)
 daemon_log.close(); run(['hyprctl','eval',RESTORE_COMMAND],capture_output=True,timeout=8)
 try: wait_until(lambda: abs(next(m for m in V1.hyprland_json('monitors all') if m['name']=='DP-2')['scale']-1.0)<.01,8,'scale restore failed')
 except Exception: pass
 if focus_before.get('address') and any(c.get('address')==focus_before.get('address') for c in V1.all_clients()):
  try: restore_focus()
  except Exception: pass
 run(['hyprctl','eval',f'hl.dispatch(hl.dsp.cursor.move({{ x = {int(cursor_before["x"])}, y = {int(cursor_before["y"])}, relative = false }}))'],capture_output=True,timeout=5)
 post={'focus':V1.hyprland_json('activewindow'),'cursor':V1.hyprland_json('cursorpos'),'monitors':V1.hyprland_json('monitors all'),'workspace8':V1.workspace_clients(8)}
 for child in (OUT/'config',OUT/'runtime',OUT/'state'):
  shutil.rmtree(child,ignore_errors=True)
 for child in (OUT/'launch.sh',OUT/'clean-shell.sh'):
  child.unlink(missing_ok=True)
 cleanup={'post':post,'focus_restored':post['focus'].get('address')==focus_before.get('address'),'cursor_restored':post['cursor']==cursor_before,'workspace8_empty':not post['workspace8'],'dp2_restored':abs(next(m for m in post['monitors'] if m['name']=='DP-2')['scale']-1.0)<.01 and next(m for m in post['monitors'] if m['name']=='DP-2')['transform']==0,'isolated_paths_absent':all(not p.exists() for p in (OUT/'config',OUT/'runtime',OUT/'state',OUT/'launch.sh',OUT/'clean-shell.sh'))}
 (OUT/'cleanup.json').write_text(json.dumps(cleanup,indent=2)+'\n')
 if not all(cleanup[k] for k in ('focus_restored','cursor_restored','workspace8_empty','dp2_restored','isolated_paths_absent')): raise RuntimeError(f'cleanup failed: {cleanup}')
print(OUT)
