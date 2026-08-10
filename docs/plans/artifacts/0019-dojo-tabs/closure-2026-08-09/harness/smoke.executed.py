#!/usr/bin/env python3
from __future__ import annotations
import hashlib, json, os, pathlib, shlex, shutil, signal, subprocess, sys, time
from evdev import UInput, ecodes
ROOT=pathlib.Path('/home/oldjobobo/Projects/splinterm')
BIN=ROOT/'target/release'
sys.path.insert(0,str(ROOT/'tools'))
from phase10_support import V1, run, wait_until, output_screenshot
OUT=pathlib.Path('/tmp/splinterm-plan0019-smoke').resolve(); APP_ID='com.oldjobobo.splinterm'
SCALE_COMMAND="hl.monitor({ output = 'DP-2', mode = '1920x1080@60', position = '640x-1080', scale = 1.2, transform = 0 })"
RESTORE_COMMAND="hl.monitor({ output = 'DP-2', mode = '1920x1080@60', position = '640x-1080', scale = 1.0, transform = 0 })"
shutil.rmtree(OUT,ignore_errors=True); (OUT/'config/splinterm').mkdir(parents=True); (OUT/'runtime').mkdir(mode=0o700); (OUT/'state').mkdir()
theme={'background':'#0e1216','alpha':1.0,'foreground':'#ebebeb','cursor':'#78d2ff','selection':'#354a60','selection_foreground':'#ffffff','url':'#78beff','ui_accent':'#78d2ff','pane_border':'#928374','pane_border_active':'#78d2ff','ansi':['#1d2021','#cc241d','#98971a','#d79921','#458588','#b16286','#689d6a','#a89984','#928374','#fb4934','#b8bb26','#fabd2f','#83a598','#d3869b','#8ec07c','#ebdbb2']}
theme_path=OUT/'config/splinterm/theme.json'; theme_path.write_text(json.dumps(theme,indent=2)+'\n')
shell_wrapper=OUT/'clean-shell.sh'; shell_wrapper.write_text('#!/bin/sh\nexec /usr/bin/bash --noprofile --norc\n'); shell_wrapper.chmod(0o700)
(OUT/'config/splinterm/config.ini').write_text(f'''[main]\nfont=JetBrains Mono Nerd Font:style=Regular\nfont-pixelsize=14\nfont-sizing-policy=output-scale\npadding-left=12\npadding-right=12\npadding-top=12\npadding-bottom=12\ninitial-columns=80\ninitial-rows=24\nlogin-shell=no\nshell={shell_wrapper}\nresize-delay-ms=0\ntheme={theme_path}\n\n[scrollback]\nlines=4096\n\n[multiplexer]\ndivider-style=line\nframe-title=splint\n\n[cursor]\nstyle=block\nblink=no\n''')
env=os.environ.copy(); env.update(SPLINTERM_SOCKET=str(OUT/'runtime/splinterd.sock'),SPLINTERM_ENABLE_DEV_ATTACH='1',SPLINTERM_PERF_TRACE='1',XDG_CONFIG_HOME=str(OUT/'config'),XDG_STATE_HOME=str(OUT/'state'))
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

try:
 wait_until(lambda: pathlib.Path(env['SPLINTERM_SOCKET']).exists() and cli('ping',check=False).returncode==0,8,'isolated daemon not ready')
 p=run(['hyprctl','eval',SCALE_COMMAND],capture_output=True,timeout=8)
 if p.returncode: raise RuntimeError(f'scale setup failed: {p.stderr or p.stdout}')
 wait_until(lambda: abs(next(m for m in V1.hyprland_json('monitors all') if m['name']=='DP-2')['scale']-1.2)<.01,8,'scale 1.2 not applied')
 launcher=OUT/'launch.sh'; client_stdout=OUT/'client.stdout'; client_stderr=OUT/'client.stderr'
 child=['/usr/bin/bash','--noprofile','--norc']
 cmd=['env']+[f'{k}={env[k]}' for k in ('SPLINTERM_SOCKET','SPLINTERM_ENABLE_DEV_ATTACH','SPLINTERM_PERF_TRACE','XDG_CONFIG_HOME','XDG_STATE_HOME')]+[str(BIN/'splinterm'),'launch','--new','--name','plan0019-smoke','--',*child]
 launcher.write_text('#!/bin/sh\nexec '+shlex.join(cmd)+' >'+shlex.quote(str(client_stdout))+' 2>'+shlex.quote(str(client_stderr))+'\n'); launcher.chmod(0o700)
 existing={c['address'] for c in V1.all_clients()}; expr=f"hl.exec_cmd({json.dumps(str(launcher))}, {{ workspace = '8 silent', float = true, size = '960 600', opacity = '1 1', no_initial_focus = true }})"
 p=run(['hyprctl','eval',expr],capture_output=True,timeout=8)
 if p.returncode: raise RuntimeError(f'launch failed: {p.stderr or p.stdout}')
 win=wait_until(lambda: next((c for c in V1.all_clients() if c.get('class')==APP_ID and c.get('address') not in existing),None),12,'smoke Window did not map')
 address=win['address']; pid=win['pid']; exact_window()
 if V1.hyprland_json('activewindow').get('address')!=focus_before.get('address'): raise RuntimeError('initial mapping changed focus')
 wait_until(lambda: counts()[1]==1,8,'initial Dojo absent'); initial=topology()['splints'][0]['splint_id']; focus_exact(); type_command("printf 'ONE_TAB_OPAQUE_OK\\n'"); wait_exact(lambda: snapshot_text(initial).count('ONE_TAB_OPAQUE_OK')>=2,8,'one-tab terminal did not commit before capture'); time.sleep(1); initial_capture=capture('01-one-tab')
 # Keyboard creation, next/previous, and open-or-activate.
 chord('d'); wait_until(lambda: counts()[1]==2,10,'Ctrl+Shift+D did not create second Dojo'); second=next(x['splint_id'] for x in topology()['splints'] if x['splint_id']!=initial); exact_window(); two_capture=capture('02-two-tabs')
 chord('Tab',shift=False); type_command("printf 'FIRST_TAB_ACTIVE\\n'"); chord('Tab'); type_command("printf 'SECOND_TAB_ACTIVE\\n'")
 # Picker selection must activate an existing tab without topology growth.
 before_picker=counts()[1]; chord('s'); key('j'); key('-k','Return'); time.sleep(1); exact_window(); picker_capture=capture('03-picker-existing-session')
 if counts()[1]!=before_picker: raise RuntimeError('picker existing-session activation mutated topology')
 # Select the second committed tab deterministically through trusted chrome.
 pointer=UInput({ecodes.EV_KEY:[ecodes.BTN_LEFT]},name='splinterm-plan0019-smoke-pointer'); time.sleep(.5)
 w=exact_window(); move_cursor(w['at'][0]+220,w['at'][1]+17); click(); exact_window()
 # The second tab is now active. Inject a bounded burst into the hidden initial tab.
 cli('send',initial,"printf 'HIDDEN_START\\n'; seq 1 2000; printf 'HIDDEN_DONE\\n'\n",timeout=20)
 type_command("printf 'ACTIVE_RESPONSIVE\\n'")
 wait_exact(lambda: snapshot_text(second).count('ACTIVE_RESPONSIVE')>=2,8,'active tab did not remain responsive during hidden burst')
 wait_exact(lambda: snapshot_text(initial).count('HIDDEN_DONE')>=1,12,'hidden burst process did not complete')
 chord('Tab',shift=False); time.sleep(1)
 if snapshot_text(initial).count('HIDDEN_DONE')<1: raise RuntimeError('hidden burst did not drain before activation')
 hidden_capture=capture('04-hidden-burst-activated')
 # Pointer activation and exact close target on the trusted strip.
 w=exact_window(); move_cursor(w['at'][0]+220,w['at'][1]+17); click(); exact_window(); pointer_activation=capture('05-pointer-activated-second')
 topology_before_close=topology(); w=exact_window(); move_cursor(w['at'][0]+345,w['at'][1]+17); click(); wait_until(lambda: len(topology()['dojos'])==2,5,'pointer close changed daemon topology'); exact_window(); pointer_close=capture('06-pointer-closed-second-tab')
 if topology()!=topology_before_close: raise RuntimeError('pointer tab close mutated daemon topology')
 # Final client-local close must close only the Window.
 focus_exact(); chord('q'); wait_until(lambda: not any(c.get('address')==address for c in V1.all_clients()),8,'final tab did not close Window')
 final_topology=topology()
 if len(final_topology['dojos'])!=2 or any(x['lifecycle']!='running' for x in final_topology['splints']): raise RuntimeError(f'final tab close changed daemon processes: {final_topology}')
 restore_focus()
 results={'schema':'splinterm.plan0019.smoke.v1','exact':True,'git_head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),'dirty_diff_sha256':hashlib.sha256(subprocess.check_output(['git','diff'],cwd=ROOT)).hexdigest(),'binary_sha256':hashlib.sha256((BIN/'splinterm').read_bytes()).hexdigest(),'daemon_sha256':hashlib.sha256((BIN/'splinterd').read_bytes()).hexdigest(),'window':{k:win.get(k) for k in ('address','pid','workspace','monitor','at','size')},'scale':1.2,'initial_focus_preserved':True,'captures':[initial_capture,two_capture,picker_capture,hidden_capture,pointer_activation,pointer_close],'keyboard_create_switch_previous':True,'picker_open_or_activate_no_topology_growth':True,'hidden_burst_drained':True,'active_responsive_during_hidden_burst':True,'pointer_activation_close':True,'pointer_close_topology_unchanged':True,'final_tab_closed_window':True,'final_daemon_topology':final_topology}
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
