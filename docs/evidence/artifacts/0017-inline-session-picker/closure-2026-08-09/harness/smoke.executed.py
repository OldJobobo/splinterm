#!/usr/bin/env python3
from __future__ import annotations
import hashlib, json, os, pathlib, shlex, shutil, signal, subprocess, sys, time
ROOT = pathlib.Path('/home/oldjobobo/Projects/splinterm')
sys.path.insert(0, str(ROOT / 'tools'))
from phase10_support import V1, run, wait_until, output_screenshot
OUT = pathlib.Path('/tmp/splinterm-plan0017-smoke').resolve()
APP_ID = 'com.oldjobobo.splinterm'
SCALE_COMMAND = "hl.monitor({ output = 'DP-2', mode = '1920x1080@60', position = '640x-1080', scale = 1.2, transform = 0 })"
RESTORE_COMMAND = "hl.monitor({ output = 'DP-2', mode = '1920x1080@60', position = '640x-1080', scale = 1.0, transform = 0 })"
shutil.rmtree(OUT, ignore_errors=True)
(OUT/'config/splinterm').mkdir(parents=True)
(OUT/'runtime').mkdir(mode=0o700)
(OUT/'state').mkdir()
theme = {
  'background':'#0e1216','alpha':1.0,'foreground':'#ebebeb','cursor':'#78d2ff',
  'selection':'#354a60','selection_foreground':'#ffffff','url':'#78beff','ui_accent':'#78d2ff',
  'pane_border':'#928374','pane_border_active':'#78d2ff',
  'ansi':['#1d2021','#cc241d','#98971a','#d79921','#458588','#b16286','#689d6a','#a89984','#928374','#fb4934','#b8bb26','#fabd2f','#83a598','#d3869b','#8ec07c','#ebdbb2']
}
(OUT/'config/splinterm/theme.json').write_text(json.dumps(theme, indent=2)+'\n')
(OUT/'config/splinterm/config.ini').write_text(f'''[main]\nfont=JetBrains Mono Nerd Font:style=Regular\nfont-pixelsize=14\nfont-sizing-policy=output-scale\npadding-left=12\npadding-right=12\npadding-top=12\npadding-bottom=12\ninitial-columns=80\ninitial-rows=24\nlogin-shell=no\nresize-delay-ms=0\ntheme={OUT/'config/splinterm/theme.json'}\n\n[scrollback]\nlines=1000\n\n[multiplexer]\ndivider-style=line\nframe-title=splint\n\n[cursor]\nstyle=block\nblink=no\n''')
env=os.environ.copy(); env.update(SPLINTERM_SOCKET=str(OUT/'runtime/splinterd.sock'),XDG_CONFIG_HOME=str(OUT/'config'),XDG_STATE_HOME=str(OUT/'state'))
focus_before=V1.hyprland_json('activewindow'); cursor_before=V1.hyprland_json('cursorpos'); monitors_before=V1.hyprland_json('monitors all')
V1.assert_test_workspace_isolated(); V1.assert_user_workspace_untouched()
(OUT/'pre-state.json').write_text(json.dumps({'focus':focus_before,'cursor':cursor_before,'monitors':monitors_before,'clients':V1.all_clients()},indent=2)+'\n')
daemon_log=(OUT/'daemon.log').open('w'); daemon=subprocess.Popen(['/usr/bin/splinterd'],env=env,stdin=subprocess.DEVNULL,stdout=daemon_log,stderr=subprocess.STDOUT,start_new_session=True,text=True)
address=None
try:
  def ping():
    p=subprocess.run(['/usr/bin/splinterm','ping'],env=env,capture_output=True,text=True,timeout=3)
    return (OUT/'runtime/splinterd.sock').exists() and p.returncode==0
  wait_until(ping,8,'isolated daemon not ready')
  p=run(['hyprctl','eval',SCALE_COMMAND],capture_output=True,timeout=8)
  if p.returncode: raise RuntimeError(f'scale setup failed: {p.stderr or p.stdout}')
  wait_until(lambda: abs(next(m for m in V1.hyprland_json('monitors all') if m['name']=='DP-2')['scale']-1.2)<0.01,8,'DP-2 scale 1.2 not applied')
  launcher=OUT/'launch.sh'
  cmd=['env']+[f'{k}={env[k]}' for k in ('SPLINTERM_SOCKET','XDG_CONFIG_HOME','XDG_STATE_HOME')]+['/usr/bin/splinterm','launch','--new','--name','plan0017-smoke','--','/usr/bin/bash','--noprofile','--norc']
  launcher.write_text('#!/bin/sh\nexec '+shlex.join(cmd)+' >'+shlex.quote(str(OUT/'client.stdout'))+' 2>'+shlex.quote(str(OUT/'client.stderr'))+'\n'); launcher.chmod(0o700)
  existing={c['address'] for c in V1.all_clients()}
  expr=f"hl.exec_cmd({json.dumps(str(launcher))}, {{ workspace = '8 silent', float = true, size = '960 600', opacity = '1 1', no_initial_focus = true }})"
  p=run(['hyprctl','eval',expr],capture_output=True,timeout=8)
  if p.returncode: raise RuntimeError(f'launch dispatch failed: {p.stderr or p.stdout}')
  win=wait_until(lambda: next((c for c in V1.all_clients() if c.get('class')==APP_ID and c.get('address') not in existing),None),12,'smoke window did not map')
  address=win['address']; pid=win['pid']
  if win['workspace']['id']!=8 or win['monitor']!=V1.test_monitor_id(): raise RuntimeError(f'placement violation: {win}')
  if V1.hyprland_json('activewindow').get('address')!=focus_before.get('address'): raise RuntimeError('initial focus changed')
  # Focus only the freshly revalidated exact Window.
  current=next((c for c in V1.all_clients() if c.get('address')==address and c.get('pid')==pid),None)
  if current is None: raise RuntimeError('exact smoke Window identity disappeared')
  p=run(['hyprctl','dispatch',f'hl.dsp.focus({{ window = "address:{address}" }})'],capture_output=True,timeout=5)
  if p.returncode: raise RuntimeError(f'exact focus dispatch failed: {p.stderr or p.stdout}')
  deadline=time.monotonic()+5
  while time.monotonic()<deadline and V1.hyprland_json('activewindow').get('address')!=address:
    time.sleep(0.05)
  active=V1.hyprland_json('activewindow')
  if active.get('address')!=address or active.get('pid')!=pid:
    raise RuntimeError(f'exact smoke Window did not receive focus: {active}')
  p=run(['wtype','-M','ctrl','-M','shift','-k','s','-m','shift','-m','ctrl'],capture_output=True,timeout=5)
  if p.returncode: raise RuntimeError(f'picker shortcut failed: {p.stderr or p.stdout}')
  time.sleep(1)
  capture=output_screenshot(OUT/'picker-smoke.png')
  p=run(['wtype','-k','Escape'],capture_output=True,timeout=5)
  if p.returncode: raise RuntimeError(f'escape failed: {p.stderr or p.stdout}')
  time.sleep(0.5)
  current=next((c for c in V1.all_clients() if c.get('address')==address),None)
  if current is None or current.get('pid')!=pid: raise RuntimeError('same Window identity did not survive Escape')
  # Restore original focus before cleanup.
  p=run(['hyprctl','dispatch',f'hl.dsp.focus({{ window = "address:{focus_before["address"]}" }})'],capture_output=True,timeout=5)
  if p.returncode: raise RuntimeError(f'focus restoration failed: {p.stderr or p.stdout}')
  wait_until(lambda: V1.hyprland_json('activewindow').get('address')==focus_before.get('address'),5,'original focus not restored')
  (OUT/'summary.json').write_text(json.dumps({'schema':'splinterm.plan0017.smoke.v1','exact':True,'git_head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),'binary_sha256':hashlib.sha256(pathlib.Path('/usr/bin/splinterm').read_bytes()).hexdigest(),'window':{k:win.get(k) for k in ('address','pid','class','title','workspace','monitor','at','size')},'scale':1.2,'initial_focus_preserved':True,'escape_same_window':True,'capture':capture},indent=2)+'\n')
finally:
  if address and any(c.get('address')==address for c in V1.all_clients()):
    V1.kill_oracle_window(address)
    try: wait_until(lambda: not V1.workspace_clients(8),8,'smoke Window cleanup failed')
    except Exception: pass
  try: os.killpg(daemon.pid,signal.SIGTERM)
  except ProcessLookupError: pass
  try: daemon.wait(timeout=8)
  except subprocess.TimeoutExpired:
    try: os.killpg(daemon.pid,signal.SIGKILL)
    except ProcessLookupError: pass
    daemon.wait(timeout=5)
  daemon_log.close()
  run(['hyprctl','eval',RESTORE_COMMAND],capture_output=True,timeout=8)
  try: wait_until(lambda: abs(next(m for m in V1.hyprland_json('monitors all') if m['name']=='DP-2')['scale']-1.0)<0.01,8,'DP-2 scale restore failed')
  except Exception: pass
  if focus_before.get('address') and any(c.get('address')==focus_before.get('address') for c in V1.all_clients()):
    run(['hyprctl','dispatch',f'hl.dsp.focus({{ window = "address:{focus_before["address"]}" }})'],capture_output=True,timeout=5)
  run(['hyprctl','dispatch',f'cursorpos {cursor_before["x"]} {cursor_before["y"]}'],capture_output=True,timeout=5)
  post={'focus':V1.hyprland_json('activewindow'),'cursor':V1.hyprland_json('cursorpos'),'monitors':V1.hyprland_json('monitors all'),'workspace8':V1.workspace_clients(8)}
  (OUT/'post-state.json').write_text(json.dumps(post,indent=2)+'\n')
  if post['workspace8']: raise RuntimeError('workspace 8 residue after smoke')
  if post['focus'].get('address')!=focus_before.get('address'): raise RuntimeError('focus residue after smoke')
  if abs(next(m for m in post['monitors'] if m['name']=='DP-2')['scale']-1.0)>0.01: raise RuntimeError('DP-2 scale residue after smoke')
print(OUT)
