#!/usr/bin/env python3
from __future__ import annotations
import hashlib,json,os,pathlib,shlex,signal,subprocess,sys,time
ROOT=pathlib.Path('/home/oldjobobo/Projects/splinterm'); sys.path.insert(0,str(ROOT/'tools'))
from phase10_support import V1,run,wait_until,output_screenshot
APP='com.oldjobobo.splinterm.FootBlurReference'
out=pathlib.Path('/tmp/splinterm-native-blur-matrix-20260729'); case=out/'foot-reference-run';
if case.exists(): raise RuntimeError(f'refusing to reuse {case}')
case.mkdir(parents=True); config_dir=case/'config'; config_dir.mkdir(); foot_ini=config_dir/'foot.ini'
foot_ini.write_text('[main]\napp-id='+APP+'\nfont=JetBrains Mono Nerd Font:size=14\npad=12x12\ninitial-window-size-chars=80x24\n\n[colors-dark]\nbackground=053632\nforeground=fee9e3\nalpha=0.75\nblur=yes\n')
V1.assert_test_workspace_isolated(); V1.assert_user_workspace_untouched(); active_before=V1.hyprland_json('activeworkspace'); focus_before=V1.hyprland_json('activewindow'); cursor_before=V1.hyprland_json('cursorpos')
(case/'pre-state.json').write_text(json.dumps({'active':active_before,'focus':focus_before,'cursor':cursor_before,'clients':V1.all_clients()},indent=2)+'\n')
launcher=case/'launch.sh'; cmd=['env','WAYLAND_DEBUG=client','/usr/bin/foot','--config='+str(foot_ini),'/usr/bin/python','-c',"import sys,time; print('FOOT BLUR REFERENCE'); sys.stdout.flush(); time.sleep(120)"]
launcher.write_text('#!/bin/sh\nexec '+shlex.join(cmd)+' >'+shlex.quote(str(case/'client.stdout'))+' 2>'+shlex.quote(str(case/'client.stderr'))+'\n'); launcher.chmod(0o700)
address=None
try:
 existing={c['address'] for c in V1.all_clients()}; expr=f"hl.exec_cmd({json.dumps(str(launcher))}, {{ workspace = '8 silent', float = true, size = '960 600', opacity = '1 1', no_initial_focus = true, no_focus = true }})"; p=run(['hyprctl','eval',expr],capture_output=True,timeout=5)
 if p.returncode: raise RuntimeError(p.stderr or p.stdout)
 win=wait_until(lambda: next((c for c in V1.all_clients() if c.get('class')==APP and c.get('address') not in existing),None),10,'Foot reference did not map'); address=win['address']
 if win['workspace']['id']!=8 or win['monitor']!=V1.test_monitor_id(): raise RuntimeError(f'Foot placement violation: {win}')
 V1.assert_user_workspace_untouched()
 if V1.hyprland_json('activewindow').get('address')!=focus_before.get('address'): raise RuntimeError('Foot stole focus')
 def protocol_ready():
  p=case/'client.stderr'
  if not p.exists(): return False
  t=p.read_text(errors='replace')
  return 'ext_background_effect_manager_v1' in t and '.get_background_effect(' in t and '.set_blur_region(' in t and '.commit(' in t
 wait_until(protocol_ready,10,'Foot background-effect protocol lifecycle missing')
 capture=output_screenshot(case/'dp2.png'); text=(case/'client.stderr').read_text(errors='replace'); protocol=[line for line in text.splitlines() if 'ext_background_effect' in line or ('wl_surface@' in line and '.commit(' in line)]
 source=pathlib.Path('/home/oldjobobo/Playground/foot'); source_commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=source,text=True).strip()
 if source_commit!='3c5b584b0eafa772eb4376fb6eaf6643399e190e': raise RuntimeError('Foot source oracle commit drifted')
 summary={'schema':'splinterm.native-blur.foot-reference.v1','exact':True,'foot_version':subprocess.check_output(['/usr/bin/foot','--version'],text=True,stderr=subprocess.STDOUT).strip(),'foot_binary_sha256':hashlib.sha256(pathlib.Path('/usr/bin/foot').read_bytes()).hexdigest(),'foot_source_commit':source_commit,'theme':{'alpha':.75,'blur':True},'window':{k:win.get(k) for k in ('address','class','workspace','monitor','at','size','focusHistoryID')},'focus_unchanged':V1.hyprland_json('activewindow').get('address')==focus_before.get('address'),'active_workspace_unchanged':V1.hyprland_json('activeworkspace')==active_before,'capture':capture,'protocol_excerpt':protocol[-40:]}
 (case/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
finally:
 if address:
  V1.kill_oracle_window(address)
  try: wait_until(lambda:not V1.workspace_clients(8),5,'Foot cleanup failed')
  except Exception: pass
 V1.assert_user_workspace_untouched()
 if V1.workspace_clients(8): raise RuntimeError('Foot window residue')
 if V1.hyprland_json('activewindow').get('address')!=focus_before.get('address'): raise RuntimeError('focus residue after Foot')
 (case/'post-state.json').write_text(json.dumps({'active':V1.hyprland_json('activeworkspace'),'focus':V1.hyprland_json('activewindow'),'cursor':V1.hyprland_json('cursorpos'),'workspace8_clients':V1.workspace_clients(8)},indent=2)+'\n')
print(case)
