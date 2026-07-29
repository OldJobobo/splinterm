#!/usr/bin/env python3
from __future__ import annotations
import hashlib, json, os, pathlib, shlex, shutil, signal, subprocess, sys, time
ROOT = pathlib.Path('/home/oldjobobo/Projects/splinterm')
sys.path.insert(0, str(ROOT / 'tools'))
from phase10_support import V1, run, wait_until, output_screenshot, descendants
APP_ID='com.oldjobobo.splinterm'
out=pathlib.Path('/tmp/splinterm-native-blur-smoke-20260729').resolve()
shutil.rmtree(out, ignore_errors=True)
(out/'config/splinterm').mkdir(parents=True)
(out/'runtime').mkdir(mode=0o700)
(out/'state').mkdir()
base=json.loads(pathlib.Path.home().joinpath('.config/splinterm/theme.json').read_text())
base['alpha']=0.75
base['blur']=True
(out/'config/splinterm/theme.json').write_text(json.dumps(base, indent=2)+'\n')
(out/'config/splinterm/config.ini').write_text('[main]\nfont=JetBrains Mono Nerd Font:style=Regular\nfont-pixelsize=14\nfont-sizing-policy=output-scale\npadding-left=12\npadding-right=12\npadding-top=12\npadding-bottom=12\ninitial-columns=80\ninitial-rows=24\nlogin-shell=no\nresize-delay-ms=0\ntheme='+str(out/'config/splinterm/theme.json')+'\n\n[scrollback]\nlines=1000\n\n[multiplexer]\ndivider-style=line\nframe-title=splint\n\n[cursor]\nstyle=block\nblink=no\n')
socket=out/'runtime/splinterd.sock'
env=os.environ.copy(); env.update(SPLINTERM_SOCKET=str(socket),SPLINTERM_ENABLE_DEV_ATTACH='1',SPLINTERM_BACKGROUND_EFFECT_TRACE='1',XDG_CONFIG_HOME=str(out/'config'),XDG_STATE_HOME=str(out/'state'))
active_before=V1.hyprland_json('activeworkspace')
focus_before=V1.hyprland_json('activewindow')
cursor_before=V1.hyprland_json('cursorpos')
V1.assert_test_workspace_isolated(); V1.assert_user_workspace_untouched()
(out/'pre-state.json').write_text(json.dumps({'active_workspace':active_before,'active_window':focus_before,'cursor':cursor_before,'monitors':V1.hyprland_json('monitors all'),'workspaces':V1.hyprland_json('workspaces'),'clients':V1.all_clients()},indent=2)+'\n')
daemon_log=(out/'daemon.log').open('w')
daemon=subprocess.Popen([str(ROOT/'target/release/splinterd')],env=env,stdin=subprocess.DEVNULL,stdout=daemon_log,stderr=subprocess.STDOUT,start_new_session=True,text=True)
address=None
try:
    def ping():
        p=subprocess.run([str(ROOT/'target/release/splinterm'),'ping'],env=env,text=True,capture_output=True,timeout=3)
        return socket.exists() and p.returncode==0
    wait_until(ping,5,'private daemon not ready')
    child="import sys,time; print('NATIVE BLUR SMOKE'); print('alpha=0.75 blur=yes'); sys.stdout.flush(); time.sleep(60)"
    launcher=out/'launch.sh'
    cmd=['env']+[f'{k}={env[k]}' for k in ('SPLINTERM_SOCKET','SPLINTERM_ENABLE_DEV_ATTACH','SPLINTERM_BACKGROUND_EFFECT_TRACE','XDG_CONFIG_HOME','XDG_STATE_HOME')]+[str(ROOT/'target/release/splinterm'),'launch','--new','--name','native-blur-smoke','--','/usr/bin/python','-c',child]
    launcher.write_text('#!/bin/sh\nexec '+shlex.join(cmd)+' >'+shlex.quote(str(out/'client.stdout'))+' 2>'+shlex.quote(str(out/'client.stderr'))+'\n'); launcher.chmod(0o700)
    existing={c['address'] for c in V1.all_clients()}
    expr=f"hl.exec_cmd({json.dumps(str(launcher))}, {{ workspace = '8 silent', float = true, size = '960 600', opacity = '1 1', no_initial_focus = true, no_focus = true }})"
    dispatched=run(['hyprctl','eval',expr],capture_output=True,timeout=5)
    if dispatched.returncode: raise RuntimeError(dispatched.stderr or dispatched.stdout)
    win=wait_until(lambda: next((c for c in V1.all_clients() if c.get('class')==APP_ID and c.get('address') not in existing),None),10,'smoke window did not map')
    address=win['address']
    if win['workspace']['id']!=8 or win['monitor']!=V1.test_monitor_id(): raise RuntimeError(f'placement violation: {win}')
    V1.assert_user_workspace_untouched()
    focus_now=V1.hyprland_json('activewindow')
    if focus_now.get('address')!=focus_before.get('address'): raise RuntimeError('focus changed during smoke')
    def trace_ready():
        p=out/'client.stderr'
        if not p.exists(): return False
        t=p.read_text(errors='replace')
        return all(s in t for s in ('manager version=1 bound','capabilities=0x1 blur=true','background-effect create','background-effect region=','background-effect commit=Enable'))
    wait_until(trace_ready,10,'native blur protocol trace incomplete')
    capture=output_screenshot(out/'dp2-smoke.png')
    win=next(c for c in V1.all_clients() if c.get('address')==address)
    summary={'schema':'splinterm.native-blur.smoke.v1','exact':True,'git_head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),'binary_sha256':hashlib.sha256((ROOT/'target/release/splinterm').read_bytes()).hexdigest(),'hyprland':subprocess.check_output(['hyprctl','version'],text=True).splitlines()[0],'theme':{'alpha':0.75,'blur':True},'window':{k:win.get(k) for k in ('address','class','title','workspace','monitor','at','size','pid','focusHistoryID')},'focus_unchanged':focus_now.get('address')==focus_before.get('address'),'active_workspace_unchanged':V1.hyprland_json('activeworkspace')==active_before,'capture':capture,'trace_lines':[line for line in (out/'client.stderr').read_text(errors='replace').splitlines() if 'background-effect' in line]}
    (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
finally:
    if address:
        V1.kill_oracle_window(address)
        try: wait_until(lambda: not V1.workspace_clients(8),5,'smoke window cleanup failed')
        except Exception: pass
    try: os.killpg(daemon.pid,signal.SIGTERM)
    except ProcessLookupError: pass
    try: daemon.wait(timeout=5)
    except subprocess.TimeoutExpired:
        try: os.killpg(daemon.pid,signal.SIGKILL)
        except ProcessLookupError: pass
        daemon.wait(timeout=5)
    daemon_log.close()
    V1.assert_user_workspace_untouched()
    if V1.workspace_clients(8): raise RuntimeError('workspace 8 residue after cleanup')
    focus_after=V1.hyprland_json('activewindow')
    if focus_after.get('address')!=focus_before.get('address'): raise RuntimeError('focus residue after cleanup')
    (out/'post-state.json').write_text(json.dumps({'active_workspace':V1.hyprland_json('activeworkspace'),'active_window':focus_after,'cursor':V1.hyprland_json('cursorpos'),'clients':V1.all_clients(),'daemon_descendants':descendants(daemon.pid) if pathlib.Path(f'/proc/{daemon.pid}').exists() else []},indent=2)+'\n')
print(out)
