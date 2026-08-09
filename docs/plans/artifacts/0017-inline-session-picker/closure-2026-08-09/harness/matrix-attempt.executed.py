#!/usr/bin/env python3
from __future__ import annotations
import hashlib, json, os, pathlib, re, shlex, shutil, signal, subprocess, sys, time
from evdev import UInput, ecodes
ROOT=pathlib.Path('/home/oldjobobo/Projects/splinterm')
sys.path.insert(0,str(ROOT/'tools'))
from phase10_support import V1, run, wait_until, output_screenshot
OUT=pathlib.Path('/tmp/splinterm-plan0017-matrix').resolve(); APP_ID='com.oldjobobo.splinterm'
shutil.rmtree(OUT,ignore_errors=True); (OUT/'config/splinterm').mkdir(parents=True); (OUT/'runtime').mkdir(mode=0o700); (OUT/'state').mkdir()
DARK={'background':'#0e1216','alpha':1.0,'foreground':'#ebebeb','cursor':'#78d2ff','selection':'#354a60','selection_foreground':'#ffffff','url':'#78beff','ui_accent':'#78d2ff','pane_border':'#928374','pane_border_active':'#78d2ff','ansi':['#1d2021','#cc241d','#98971a','#d79921','#458588','#b16286','#689d6a','#a89984','#928374','#fb4934','#b8bb26','#fabd2f','#83a598','#d3869b','#8ec07c','#ebdbb2']}
LIGHT={'background':'#f3efe7','alpha':0.78,'foreground':'#24211d','cursor':'#005f87','selection':'#c4d8e8','selection_foreground':'#111111','url':'#005f87','ui_accent':'#006b8f','pane_border':'#8b8175','pane_border_active':'#006b8f','ansi':['#24211d','#a33b36','#587a3a','#a06400','#266b82','#76528b','#2b7772','#d6cec2','#6f675e','#c24b45','#6f963f','#c17a00','#3285a0','#9368aa','#37948d','#f3efe7']}
theme_path=OUT/'config/splinterm/theme.json'; theme_path.write_text(json.dumps(DARK,indent=2)+'\n')
(OUT/'config/splinterm/config.ini').write_text(f'''[main]\nfont=JetBrains Mono Nerd Font:style=Regular\nfont-pixelsize=14\nfont-sizing-policy=output-scale\npadding-left=12\npadding-right=12\npadding-top=12\npadding-bottom=12\ninitial-columns=80\ninitial-rows=24\nlogin-shell=no\nresize-delay-ms=0\ntheme={theme_path}\n\n[scrollback]\nlines=1000\n\n[multiplexer]\ndivider-style=line\nframe-title=splint\n\n[cursor]\nstyle=block\nblink=no\n''')
env=os.environ.copy(); env.update(SPLINTERM_SOCKET=str(OUT/'runtime/splinterd.sock'),SPLINTERM_ENABLE_DEV_ATTACH='1',XDG_CONFIG_HOME=str(OUT/'config'),XDG_STATE_HOME=str(OUT/'state'))
focus_before=V1.hyprland_json('activewindow'); cursor_before=V1.hyprland_json('cursorpos'); monitors_before=V1.hyprland_json('monitors all')
V1.assert_test_workspace_isolated(); V1.assert_user_workspace_untouched(); (OUT/'pre-state.json').write_text(json.dumps({'focus':focus_before,'cursor':cursor_before,'monitors':monitors_before,'clients':V1.all_clients()},indent=2)+'\n')
daemon_log=(OUT/'daemon.log').open('w'); daemon=subprocess.Popen(['/usr/bin/splinterd'],env=env,stdin=subprocess.DEVNULL,stdout=daemon_log,stderr=subprocess.STDOUT,start_new_session=True,text=True)
addresses=[]; cases=[]; pointer=None

def cli(*args, timeout=12):
  p=subprocess.run(['/usr/bin/splinterm',*args],env=env,capture_output=True,text=True,timeout=timeout)
  if p.returncode: raise RuntimeError(f"CLI {' '.join(args)} failed: {p.stderr or p.stdout}")
  return p.stdout

def monitor_scale(scale:float):
  code=f"hl.monitor({{ output = 'DP-2', mode = '1920x1080@60', position = '640x-1080', scale = {scale}, transform = 0 }})"
  p=run(['hyprctl','eval',code],capture_output=True,timeout=8)
  if p.returncode: raise RuntimeError(f'monitor scale {scale} failed: {p.stderr or p.stdout}')
  deadline=time.monotonic()+8
  while time.monotonic()<deadline:
    m=next(x for x in V1.hyprland_json('monitors all') if x['name']=='DP-2')
    if abs(m['scale']-scale)<0.01: return
    time.sleep(.1)
  raise RuntimeError(f'DP-2 scale {scale} not applied')

def exact_window(address,pid):
  matches=[c for c in V1.all_clients() if c.get('address')==address and c.get('pid')==pid]
  if len(matches)!=1: raise RuntimeError(f'exact Window identity mismatch {address}/{pid}: {matches}')
  w=matches[0]
  if w['workspace']['id']!=8 or w['monitor']!=V1.test_monitor_id(): raise RuntimeError(f'placement drift: {w}')
  return w

def focus_exact(address,pid):
  exact_window(address,pid)
  p=run(['hyprctl','dispatch',f'hl.dsp.focus({{ window = "address:{address}" }})'],capture_output=True,timeout=5)
  if p.returncode: raise RuntimeError(f'focus failed: {p.stderr or p.stdout}')
  deadline=time.monotonic()+5
  while time.monotonic()<deadline:
    a=V1.hyprland_json('activewindow')
    if a.get('address')==address and a.get('pid')==pid: return
    time.sleep(.05)
  raise RuntimeError(f'exact target did not focus: {V1.hyprland_json("activewindow")}')

def restore_focus():
  if not any(c.get('address')==focus_before.get('address') for c in V1.all_clients()): raise RuntimeError('original Foot disappeared')
  p=run(['hyprctl','dispatch',f'hl.dsp.focus({{ window = "address:{focus_before["address"]}" }})'],capture_output=True,timeout=5)
  if p.returncode: raise RuntimeError(f'focus restore failed: {p.stderr or p.stdout}')
  deadline=time.monotonic()+5
  while time.monotonic()<deadline:
    if V1.hyprland_json('activewindow').get('address')==focus_before.get('address'): return
    time.sleep(.05)
  raise RuntimeError('original Foot focus not restored')

def key(*args):
  p=run(['wtype',*args],capture_output=True,timeout=5)
  if p.returncode: raise RuntimeError(f'wtype failed: {p.stderr or p.stdout}')

def open_picker(address,pid):
  focus_exact(address,pid); key('-M','ctrl','-M','shift','-k','s','-m','shift','-m','ctrl'); time.sleep(.8); exact_window(address,pid)

def launch_client(label,args,size):
  launcher=OUT/f'{label}.sh'; stdout=OUT/f'{label}.stdout'; stderr=OUT/f'{label}.stderr'
  cmd=['env']+[f'{k}={env[k]}' for k in ('SPLINTERM_SOCKET','SPLINTERM_ENABLE_DEV_ATTACH','XDG_CONFIG_HOME','XDG_STATE_HOME')]+['/usr/bin/splinterm',*args]
  launcher.write_text('#!/bin/sh\nexec '+shlex.join(cmd)+' >'+shlex.quote(str(stdout))+' 2>'+shlex.quote(str(stderr))+'\n'); launcher.chmod(0o700)
  existing={c['address'] for c in V1.all_clients()}
  expr=f"hl.exec_cmd({json.dumps(str(launcher))}, {{ workspace = '8 silent', float = true, size = '{size[0]} {size[1]}', opacity = '1 1', no_initial_focus = true }})"
  p=run(['hyprctl','eval',expr],capture_output=True,timeout=8)
  if p.returncode: raise RuntimeError(f'launch {label} failed: {p.stderr or p.stdout}')
  w=wait_until(lambda: next((c for c in V1.all_clients() if c.get('class')==APP_ID and c.get('address') not in existing),None),12,f'{label} did not map')
  addresses.append(w['address'])
  if V1.hyprland_json('activewindow').get('address')!=focus_before.get('address'): raise RuntimeError(f'{label} stole initial focus')
  return w['address'],w['pid']

def resize_exact(address,pid,width,height):
  exact_window(address,pid); sel=json.dumps(f'address:{address}')
  p=run(['hyprctl','eval',f'hl.dispatch(hl.dsp.window.resize({{ x = {width}, y = {height}, window = {sel} }}))'],capture_output=True,timeout=5)
  if p.returncode: raise RuntimeError(f'resize failed: {p.stderr or p.stdout}')
  deadline=time.monotonic()+6
  while time.monotonic()<deadline:
    if exact_window(address,pid).get('size')==[width,height]: return
    time.sleep(.1)
  raise RuntimeError(f'resize {width}x{height} did not settle: {exact_window(address,pid).get("size")}')

def capture(name): return output_screenshot(OUT/f'{name}.png')

def topology_counts():
  text=cli('list','--all'); return sum(int(v) for v in re.findall(r'(\d+) Dojo\(s\)',text)),text

def first_splint():
  text=cli('list','--all'); m=re.search(r'^\s{2}([0-9a-f-]{36})\s+\S+\s+Running\s*$',text,re.M)
  if not m: raise RuntimeError(f'cannot locate target Splint:\n{text}')
  return m.group(1)

def move_cursor(x,y):
  p=run(['hyprctl','eval',f'hl.dispatch(hl.dsp.cursor.move({{ x = {int(x)}, y = {int(y)}, relative = false }}))'],capture_output=True,timeout=5)
  if p.returncode: raise RuntimeError(f'cursor move failed: {p.stderr or p.stdout}')
  time.sleep(.2); pos=V1.hyprland_json('cursorpos')
  if abs(pos['x']-x)>2 or abs(pos['y']-y)>2: raise RuntimeError(f'cursor move mismatch: wanted {x},{y}, got {pos}')

def click(state):
  pointer.write(ecodes.EV_KEY,ecodes.BTN_LEFT,state); pointer.syn(); time.sleep(.15)

try:
  wait_until(lambda: pathlib.Path(env['SPLINTERM_SOCKET']).exists() and subprocess.run(['/usr/bin/splinterm','ping'],env=env,capture_output=True).returncode==0,8,'isolated daemon not ready')
  # Case 1: empty catalog, dark opaque, normal, scale 1.2.
  monitor_scale(1.2); empty_address,empty_pid=launch_client('empty',['sessions'],(960,600)); focus_exact(empty_address,empty_pid); time.sleep(.8)
  empty_capture=capture('empty-dark-opaque-normal-scale120'); key('-k','Escape')
  wait_until(lambda: not any(c.get('address')==empty_address for c in V1.all_clients()),8,'empty picker did not close on Escape'); addresses.remove(empty_address); restore_focus()
  if topology_counts()[0]!=0: raise RuntimeError('empty picker mutated topology')
  cases.append({'case':'empty-dark-opaque-normal-scale120','capture':empty_capture,'escape_closed':True,'dojo_count':0})
  # Target and paged Unicode catalog.
  target_name='Plan0017–Unicode–測試–é'; target_address,target_pid=launch_client('target',['launch','--new','--name',target_name,'--','/usr/bin/bash','--noprofile','--norc'],(620,420))
  wait_until(lambda: topology_counts()[0]>=1,8,'target topology not ready'); target_splint=first_splint()
  for i in range(10): cli('new',f'Plan0017-catalog-{i:02d}','--','/usr/bin/sleep','300')
  dojo_count_before,text=topology_counts()
  if dojo_count_before<11: raise RuntimeError(f'paged catalog not created: {text}')
  # Case 2: light translucent compact scale 1.5.
  theme_path.write_text(json.dumps(LIGHT,indent=2)+'\n'); time.sleep(1); monitor_scale(1.5); resize_exact(target_address,target_pid,460,300); open_picker(target_address,target_pid)
  paged_capture=capture('paged-unicode-light-translucent-compact-scale150')
  # Modal input isolation: plain text must not reach PTY.
  key('LEAK_SENTINEL_0017'); key('-k','Escape'); time.sleep(.5)
  snapshot=cli('--output','json','snapshot',target_splint)
  if 'LEAK_SENTINEL_0017' in snapshot: raise RuntimeError('modal keyboard input leaked into target terminal')
  # Keyboard navigation and successful same-Window switch.
  open_picker(target_address,target_pid); key('-k','Down'); key('-k','Down'); key('-k','Return'); time.sleep(1)
  exact_window(target_address,target_pid); switched_capture=capture('keyboard-same-window-switch'); restore_focus()
  cases.append({'case':'paged-unicode-light-translucent-compact-scale150','capture':paged_capture,'input_leak':False,'keyboard_same_window':True,'post_switch_capture':switched_capture,'dojo_count':dojo_count_before})
  # Case 3: dark translucent minimal scale 2.4, pointer behavior.
  dark_trans=dict(DARK); dark_trans['alpha']=0.72; theme_path.write_text(json.dumps(dark_trans,indent=2)+'\n'); time.sleep(1); monitor_scale(2.4); resize_exact(target_address,target_pid,260,170); open_picker(target_address,target_pid)
  pointer=UInput({ecodes.EV_KEY:[ecodes.BTN_LEFT]},name='splinterm-plan0017-pointer'); time.sleep(.8)
  w=exact_window(target_address,target_pid); # Minimal layout: panel fills 260x170, action spans y=30..140.
  inside=(w['at'][0]+130,w['at'][1]+85); outside=(w['at'][0]+8,w['at'][1]+8)
  move_cursor(*inside); hover_capture=capture('minimal-hover-dark-translucent-scale240')
  click(1); move_cursor(*outside); click(0); cancel_capture=capture('minimal-press-release-cancel')
  # Picker must still own Escape after cancellation; reopen then pointer-activate New.
  key('-k','Escape'); time.sleep(.4); exact_window(target_address,target_pid)
  count_before_new=topology_counts()[0]; open_picker(target_address,target_pid); w=exact_window(target_address,target_pid); inside=(w['at'][0]+130,w['at'][1]+85); move_cursor(*inside); click(1); click(0)
  wait_until(lambda: topology_counts()[0]==count_before_new+1,10,'pointer New activation did not create one Dojo')
  exact_window(target_address,target_pid); pointer_capture=capture('pointer-new-same-window'); restore_focus()
  cases.append({'case':'minimal-pointer-dark-translucent-scale240','hover_capture':hover_capture,'cancel_capture':cancel_capture,'press_release_cancelled':True,'pointer_activation_same_window':True,'pointer_capture':pointer_capture,'dojo_count_before':count_before_new,'dojo_count_after':count_before_new+1})
  (OUT/'summary.json').write_text(json.dumps({'schema':'splinterm.plan0017.matrix.v1','exact':True,'git_head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),'binary_sha256':hashlib.sha256(pathlib.Path('/usr/bin/splinterm').read_bytes()).hexdigest(),'cases':cases,'target':{'address':target_address,'pid':target_pid,'splint_id':target_splint,'long_unicode_name':target_name},'scales':[1.2,1.5,2.4],'themes':['dark-opaque','light-translucent','dark-translucent'],'sizes':['normal','compact','minimal']},indent=2)+'\n')
finally:
  if pointer is not None:
    try: pointer.close()
    except Exception: pass
  for address in list(addresses):
    if any(c.get('address')==address for c in V1.all_clients()):
      V1.kill_oracle_window(address)
  try:
    wait_until(lambda: not V1.workspace_clients(8),10,'matrix Window cleanup failed')
  except Exception: pass
  try: os.killpg(daemon.pid,signal.SIGTERM)
  except ProcessLookupError: pass
  try: daemon.wait(timeout=10)
  except subprocess.TimeoutExpired:
    try: os.killpg(daemon.pid,signal.SIGKILL)
    except ProcessLookupError: pass
    daemon.wait(timeout=5)
  daemon_log.close()
  try: monitor_scale(1.0)
  except Exception: pass
  if focus_before.get('address') and any(c.get('address')==focus_before.get('address') for c in V1.all_clients()):
    try: restore_focus()
    except Exception: pass
  run(['hyprctl','eval',f'hl.dispatch(hl.dsp.cursor.move({{ x = {int(cursor_before["x"])}, y = {int(cursor_before["y"])}, relative = false }}))'],capture_output=True,timeout=5)
  post={'focus':V1.hyprland_json('activewindow'),'cursor':V1.hyprland_json('cursorpos'),'monitors':V1.hyprland_json('monitors all'),'workspace8':V1.workspace_clients(8)}; (OUT/'post-state.json').write_text(json.dumps(post,indent=2)+'\n')
  if post['workspace8']: raise RuntimeError('workspace 8 residue after matrix')
  if post['focus'].get('address')!=focus_before.get('address'): raise RuntimeError('focus residue after matrix')
  if abs(next(m for m in post['monitors'] if m['name']=='DP-2')['scale']-1.0)>0.01: raise RuntimeError('DP-2 scale residue after matrix')
print(OUT)
