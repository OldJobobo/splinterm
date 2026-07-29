#!/usr/bin/env python3
from __future__ import annotations
import hashlib,json,os,pathlib,shlex,shutil,signal,subprocess,sys,time
ROOT=pathlib.Path('/home/oldjobobo/Projects/splinterm'); sys.path.insert(0,str(ROOT/'tools'))
from phase10_support import V1,S3,run,wait_until,output_screenshot,apply_monitor_scale_owned,descendants
APP='com.oldjobobo.splinterm'; FOOT_APP='com.oldjobobo.splinterm.FootBlurReference'
out=pathlib.Path('/tmp/splinterm-native-blur-matrix-20260729').resolve(); shutil.rmtree(out,ignore_errors=True)
(out/'config/splinterm').mkdir(parents=True); (out/'runtime').mkdir(mode=0o700); (out/'state').mkdir()
base=json.loads(pathlib.Path.home().joinpath('.config/splinterm/theme.json').read_text())
theme_path=out/'config/splinterm/theme.json'
def write_theme(alpha,blur):
 d=dict(base); d['alpha']=alpha; d['blur']=blur; t=theme_path.with_suffix('.tmp'); t.write_text(json.dumps(d,indent=2)+'\n'); os.replace(t,theme_path)
write_theme(.75,False)
(out/'config/splinterm/config.ini').write_text('[main]\nfont=JetBrains Mono Nerd Font:style=Regular\nfont-pixelsize=14\nfont-sizing-policy=output-scale\npadding-left=12\npadding-right=12\npadding-top=12\npadding-bottom=12\ninitial-columns=80\ninitial-rows=24\nlogin-shell=no\nresize-delay-ms=0\ntheme='+str(theme_path)+'\n\n[scrollback]\nlines=1000\n\n[multiplexer]\ndivider-style=frame\nframe-title=splint\n\n[cursor]\nstyle=block\nblink=no\n')
socket=out/'runtime/splinterd.sock'; env=os.environ.copy(); env.update(SPLINTERM_SOCKET=str(socket),SPLINTERM_ENABLE_DEV_ATTACH='1',SPLINTERM_BACKGROUND_EFFECT_TRACE='1',XDG_CONFIG_HOME=str(out/'config'),XDG_STATE_HOME=str(out/'state'))
active_before=V1.hyprland_json('activeworkspace'); focus_before=V1.hyprland_json('activewindow'); cursor_before=V1.hyprland_json('cursorpos'); V1.assert_test_workspace_isolated(); V1.assert_user_workspace_untouched()
original_monitor=S3.monitor_state(); (out/'pre-state.json').write_text(json.dumps({'active':active_before,'focus':focus_before,'cursor':cursor_before,'monitor':original_monitor,'clients':V1.all_clients()},indent=2)+'\n')
daemon_log=(out/'daemon.log').open('w'); daemon=subprocess.Popen([str(ROOT/'target/release/splinterd')],env=env,stdin=subprocess.DEVNULL,stdout=daemon_log,stderr=subprocess.STDOUT,start_new_session=True,text=True)
current_address=None; scale_changed=False; results=[]; launch_count=0

def assert_guard():
 V1.assert_user_workspace_untouched()
 now=V1.hyprland_json('activewindow')
 if now.get('address')!=focus_before.get('address'): raise RuntimeError('focus changed from guarded baseline')

def log_text(case):
 p=out/case/'client.stderr'; return p.read_text(errors='replace') if p.exists() else ''
def trace_lines(case): return [x for x in log_text(case).splitlines() if 'background-effect' in x]
def launch(case,alpha,blur,command=None,app=APP,extra_env=None):
 global current_address,launch_count
 assert not V1.workspace_clients(8); assert_guard(); write_theme(alpha,blur); case_dir=out/case; case_dir.mkdir()
 launch_count+=1; launcher=case_dir/'launch.sh'; cmd=command or [str(ROOT/'target/release/splinterm'),'launch','--new','--name',case,'--','/usr/bin/python','-c',f"import sys,time; print({case!r}); sys.stdout.flush(); time.sleep(120)"]
 runenv={k:env[k] for k in ('SPLINTERM_SOCKET','SPLINTERM_ENABLE_DEV_ATTACH','SPLINTERM_BACKGROUND_EFFECT_TRACE','XDG_CONFIG_HOME','XDG_STATE_HOME')}; runenv.update(extra_env or {})
 launcher.write_text('#!/bin/sh\nexec env '+shlex.join([f'{k}={v}' for k,v in runenv.items()]+cmd)+' >'+shlex.quote(str(case_dir/'client.stdout'))+' 2>'+shlex.quote(str(case_dir/'client.stderr'))+'\n'); launcher.chmod(0o700)
 existing={c['address'] for c in V1.all_clients()}; expr=f"hl.exec_cmd({json.dumps(str(launcher))}, {{ workspace = '8 silent', float = true, size = '960 600', opacity = '1 1', no_initial_focus = true, no_focus = true }})"
 p=run(['hyprctl','eval',expr],capture_output=True,timeout=5)
 if p.returncode: raise RuntimeError(p.stderr or p.stdout)
 win=wait_until(lambda: next((c for c in V1.all_clients() if c.get('class')==app and c.get('address') not in existing),None),10,f'{case} did not map'); current_address=win['address']
 if win['workspace']['id']!=8 or win['monitor']!=V1.test_monitor_id(): raise RuntimeError(f'{case} placement violation')
 assert_guard(); return win

def cleanup_case():
 global current_address
 if current_address:
  V1.kill_oracle_window(current_address); wait_until(lambda: not V1.workspace_clients(8),5,'window cleanup failed'); current_address=None
 assert_guard()
def wait_log(case,pred,msg): return wait_until(lambda: (t:=log_text(case)) and pred(t) and t,12,msg)
def capture(case,label='dp2'):
 assert_guard(); return output_screenshot(out/case/f'{label}.png')
def record(case,alpha,blur,extra=None):
 w=next(c for c in V1.all_clients() if c['address']==current_address); item={'case':case,'alpha':alpha,'blur':blur,'window':{k:w.get(k) for k in ('address','workspace','monitor','at','size','focusHistoryID')},'trace':trace_lines(case),'capture':capture(case)}; item.update(extra or {}); results.append(item)
def client_json(*args):
 p=run([str(ROOT/'target/release/splinterm'),'--output=json',*args],env=env,capture_output=True,timeout=10)
 if p.returncode: raise RuntimeError(p.stderr); return json.loads(p.stdout)
 return json.loads(p.stdout)
try:
 wait_until(lambda: socket.exists() and run([str(ROOT/'target/release/splinterm'),'ping'],env=env,capture_output=True,timeout=3).returncode==0,5,'daemon not ready')
 # translucent, blur disabled
 launch('translucent-blur-off',.75,False); wait_log('translucent-blur-off',lambda t:'capabilities=0x1 blur=true' in t,'off capability trace missing'); time.sleep(.4); assert_guard();
 if 'background-effect create' in log_text('translucent-blur-off'): raise RuntimeError('blur-off created effect')
 record('translucent-blur-off',.75,False); cleanup_case()
 # translucent, blur enabled
 launch('translucent-blur-on',.75,True); wait_log('translucent-blur-on',lambda t:all(x in t for x in ('background-effect create','background-effect region=','background-effect commit=Enable')),'on lifecycle missing')
 if log_text('translucent-blur-on').count('background-effect create')!=1: raise RuntimeError('blur-on create count')
 record('translucent-blur-on',.75,True); cleanup_case()
 # opaque, blur requested
 launch('opaque-blur-on',1.0,True); wait_log('opaque-blur-on',lambda t:'capabilities=0x1 blur=true' in t,'opaque capability trace missing'); time.sleep(.4); assert_guard()
 if 'background-effect create' in log_text('opaque-blur-on'): raise RuntimeError('opaque created effect')
 record('opaque-blur-on',1.0,True); cleanup_case()
 # live blur no -> yes -> no
 launch('live-blur-toggle',.75,False); wait_log('live-blur-toggle',lambda t:'capabilities=0x1 blur=true' in t,'live blur capability missing'); write_theme(.75,True); wait_log('live-blur-toggle',lambda t:'commit=Enable' in t and 'background-effect create' in t,'live blur enable missing'); cap_on=capture('live-blur-toggle','enabled'); write_theme(.75,False); wait_log('live-blur-toggle',lambda t:'background-effect destroy' in t and 'commit=Disable' in t,'live blur disable missing')
 t=log_text('live-blur-toggle');
 if t.count('background-effect create')!=1 or t.count('background-effect destroy')!=1: raise RuntimeError('live blur lifecycle count')
 record('live-blur-toggle',.75,False,{'enabled_capture':cap_on}); cleanup_case()
 # live alpha opaque -> translucent -> opaque
 launch('live-alpha-toggle',1.0,True); wait_log('live-alpha-toggle',lambda t:'capabilities=0x1 blur=true' in t,'live alpha capability missing'); write_theme(.75,True); wait_log('live-alpha-toggle',lambda t:'background-effect create' in t and 'commit=Enable' in t,'alpha enable missing'); cap_trans=capture('live-alpha-toggle','translucent'); write_theme(1.0,True); wait_log('live-alpha-toggle',lambda t:'background-effect destroy' in t and 'commit=Disable' in t,'alpha disable missing')
 t=log_text('live-alpha-toggle');
 if t.count('background-effect create')!=1 or t.count('background-effect destroy')!=1: raise RuntimeError('live alpha lifecycle count')
 record('live-alpha-toggle',1.0,True,{'translucent_capture':cap_trans}); cleanup_case()
 # resize while active
 launch('active-resize',.75,True); wait_log('active-resize',lambda t:'commit=Enable' in t,'resize enable missing'); sel=json.dumps(f'address:{current_address}'); expr=f"hl.dispatch(hl.dsp.window.resize({{ x = 1100, y = 700, window = {sel} }}))"; p=run(['hyprctl','eval',expr],capture_output=True,timeout=5)
 if p.returncode: raise RuntimeError('targeted resize dispatch failed')
 wait_until(lambda: next((c for c in V1.all_clients() if c['address']==current_address and c['size']==[1100,700]),None),8,'target size missing'); wait_log('active-resize',lambda t:'region=1100x700' in t and 'commit=Resize' in t,'finite resized region missing')
 if log_text('active-resize').count('background-effect create')!=1: raise RuntimeError('resize recreated effect')
 record('active-resize',.75,True); cleanup_case()
 # fractional output scale, restored immediately
 launch('fractional-scale-150',.75,True); wait_log('fractional-scale-150',lambda t:'commit=Enable' in t,'scale enable missing'); scale_changed=True; apply_monitor_scale_owned(original_monitor,150,current_address); time.sleep(.5); assert_guard(); w=next(c for c in V1.all_clients() if c['address']==current_address); lines=trace_lines('fractional-scale-150');
 if sum('background-effect create' in x for x in lines)!=1: raise RuntimeError('scale recreated effect')
 frac_cap=capture('fractional-scale-150'); S3.restore_monitor(original_monitor); scale_changed=False; wait_until(lambda: abs(float(S3.monitor_state()['scale'])-float(original_monitor['scale']))<.001,5,'scale restore failed'); results.append({'case':'fractional-scale-150','alpha':.75,'blur':True,'scale':1.25,'window':{k:w.get(k) for k in ('address','workspace','monitor','at','size')},'trace':lines,'capture':frac_cap}); cleanup_case()
 # multi-pane daemon-owned window
 created=client_json('new','native-blur-multipane','--cwd','/tmp','--','/usr/bin/python','-c','import time; time.sleep(120)')['resource']; client_json('split',created['splint_id'],'--axis','horizontal','--side','second','--cwd','/tmp','--','/usr/bin/python','-c','import time; time.sleep(120)')
 cmd=[str(ROOT/'target/release/splinterm'),'window','--dojo-id',created['dojo_id'],'--window-id',created['window_id']]; launch('multi-pane',.75,True,command=cmd); wait_log('multi-pane',lambda t:'commit=Enable' in t,'multi-pane enable missing');
 if log_text('multi-pane').count('background-effect create')!=1: raise RuntimeError('multi-pane effect count')
 record('multi-pane',.75,True,{'dojo_id':created['dojo_id'],'window_id':created['window_id'],'pane_count':2}); cleanup_case()
 # Foot 1.27 reference with Wayland protocol debug
 footdir=out/'foot-reference'; footdir.mkdir(); foot_ini=footdir/'foot.ini'; foot_ini.write_text('[main]\napp-id='+FOOT_APP+'\nfont=JetBrains Mono Nerd Font:size=14\npad=12x12\ninitial-window-size-chars=80x24\n\n[colors-dark]\nbackground=053632\nforeground=fee9e3\nalpha=0.75\nblur=yes\n')
 fcmd=['/usr/bin/foot','--config='+str(foot_ini),'/usr/bin/python','-c',"import time; print('FOOT BLUR REFERENCE'); time.sleep(120)"]
 launch('foot-reference',.75,True,command=fcmd,app=FOOT_APP,extra_env={'WAYLAND_DEBUG':'client'}); wait_log('foot-reference',lambda t:'ext_background_effect' in t and 'set_blur_region' in t,'Foot background-effect requests missing'); ft=log_text('foot-reference'); protocol=[x for x in ft.splitlines() if 'ext_background_effect' in x or ('wl_surface@' in x and '.commit(' in x)]; record('foot-reference',.75,True,{'foot_version':subprocess.check_output(['/usr/bin/foot','--version'],text=True,stderr=subprocess.STDOUT).strip(),'foot_sha256':hashlib.sha256(pathlib.Path('/usr/bin/foot').read_bytes()).hexdigest(),'foot_source_commit':subprocess.check_output(['git','rev-parse','HEAD'],cwd='/home/oldjobobo/Playground/foot',text=True).strip(),'protocol_excerpt':protocol[-30:]}); cleanup_case()
 summary={'schema':'splinterm.native-blur.matrix.v1','exact':True,'git_head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),'binary_sha256':hashlib.sha256((ROOT/'target/release/splinterm').read_bytes()).hexdigest(),'hyprland':subprocess.check_output(['hyprctl','version'],text=True).splitlines()[0],'active_workspace_unchanged':V1.hyprland_json('activeworkspace')==active_before,'focus_unchanged':V1.hyprland_json('activewindow').get('address')==focus_before.get('address'),'cases':results}; (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
finally:
 if scale_changed:
  try: S3.restore_monitor(original_monitor)
  except Exception: pass
 if current_address:
  V1.kill_oracle_window(current_address)
  try: wait_until(lambda:not V1.workspace_clients(8),5,'final window cleanup failed')
  except Exception: pass
 try: os.killpg(daemon.pid,signal.SIGTERM)
 except ProcessLookupError: pass
 try: daemon.wait(timeout=5)
 except subprocess.TimeoutExpired:
  try: os.killpg(daemon.pid,signal.SIGKILL)
  except ProcessLookupError: pass
  daemon.wait(timeout=5)
 daemon_log.close(); V1.assert_user_workspace_untouched()
 if V1.workspace_clients(8): raise RuntimeError('workspace residue after matrix')
 if V1.hyprland_json('activewindow').get('address')!=focus_before.get('address'): raise RuntimeError('focus residue after matrix')
 (out/'post-state.json').write_text(json.dumps({'active':V1.hyprland_json('activeworkspace'),'focus':V1.hyprland_json('activewindow'),'cursor':V1.hyprland_json('cursorpos'),'monitor':S3.monitor_state(),'workspace8_clients':V1.workspace_clients(8),'daemon_alive':pathlib.Path(f'/proc/{daemon.pid}').exists()},indent=2)+'\n')
print(out)
