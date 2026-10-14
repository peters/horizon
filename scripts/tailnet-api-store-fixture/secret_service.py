#!/usr/bin/env python3
"""Task-owned synthetic Secret Service; no graphical process or host bus access."""
import json,os,secrets,signal,subprocess,time,sys
from pathlib import Path

class Fixture:
 def __init__(self,state):
  self.state=Path(state);assert self.state==Path('/fixture/state')
  self.children=[];self.files=[];self.keyring=None
  for name in ['private-home','runtime','runtime/keyring','horizon','horizon/cloud','private-home/.config','private-home/.cache','private-home/.local/share']:
   p=self.state/name;p.mkdir(parents=True,exist_ok=True,mode=0o700);p.chmod(0o700)
  self.env={'PATH':'/usr/bin:/bin','LANG':'C.UTF-8','HOME':str(self.state/'private-home'),'XDG_CONFIG_HOME':str(self.state/'private-home/.config'),'XDG_DATA_HOME':str(self.state/'private-home/.local/share'),'XDG_CACHE_HOME':str(self.state/'private-home/.cache'),'XDG_RUNTIME_DIR':str(self.state/'runtime'),'DBUS_SESSION_BUS_ADDRESS':'unix:path='+str(self.state/'runtime/bus'),'DBUS_SYSTEM_BUS_ADDRESS':'unix:path='+str(self.state/'runtime/no-system-bus'),'HORIZON_HOME':str(self.state/'horizon'),'GNOME_KEYRING_CONTROL':str(self.state/'runtime/keyring')}
  password=self.state/'keyring-password'
  if not password.exists():password.write_text(secrets.token_urlsafe(32))
  password.chmod(0o600);self.password=password
 def spawn(self,name,args,**kw):
  log=(self.state/(name+'.log')).open('ab');self.files.append(log)
  p=subprocess.Popen(args,env=self.env,stdout=log,stderr=log,start_new_session=True,**kw);self.children.append(p);return p
 def bus_call(self,*args):
  return subprocess.run(['/usr/bin/gdbus','call','--session',*args],env=self.env,capture_output=True,text=True,timeout=5,check=True).stdout.strip()
 def secret_owned(self):
  return 'true' in self.bus_call('--dest','org.freedesktop.DBus','--object-path','/org/freedesktop/DBus','--method','org.freedesktop.DBus.NameHasOwner','org.freedesktop.secrets')
 def wait(self,condition,process):
  deadline=time.monotonic()+10
  while not condition():
   if process.poll() is not None or time.monotonic()>deadline:raise RuntimeError('fixture process did not become ready')
   time.sleep(.05)
 def start(self):
  socket=self.state/'runtime/bus'
  if socket.exists():socket.unlink()
  config=self.state/'bus.conf'
  config.write_text('''<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig><type>session</type><listen>unix:path=/fixture/state/runtime/bus</listen><auth>EXTERNAL</auth><policy context="default"><allow own="*"/><allow send_destination="*"/><allow receive_sender="*"/></policy></busconfig>''')
  bus=self.spawn('dbus',['/usr/bin/dbus-daemon','--config-file='+str(config),'--nofork','--nopidfile','--nosyslog'])
  self.wait(lambda:(self.state/'runtime/bus').is_socket(),bus);self.start_keyring()
 def start_keyring(self):
  assert not self.secret_owned()
  self.keyring=self.spawn('keyring',['/usr/bin/gnome-keyring-daemon','--foreground','--unlock','--components=secrets','--control-directory='+str(self.state/'runtime/keyring')],stdin=subprocess.PIPE)
  self.keyring.stdin.write(self.password.read_bytes());self.keyring.stdin.close()
  self.wait(self.secret_owned,self.keyring)
 def stop_keyring(self):
  self.stop(self.keyring);self.keyring=None
  assert not self.secret_owned()
 def stop(self,p):
  if p and p.poll() is None:
   os.killpg(p.pid,signal.SIGTERM)
   try:p.wait(timeout=5)
   except subprocess.TimeoutExpired:os.killpg(p.pid,signal.SIGKILL);p.wait(timeout=5)
 def close(self):
  for p in reversed(self.children):self.stop(p)
  for f in self.files:f.close()
 def run_helper(self,operation):
  environment=dict(self.env, HORIZON_TAILNET_FIXTURE_PHASE=operation)
  r=subprocess.run(['/fixture/bin/tailnet-api-store-fixture','--exact','tailnet::api::tests::real_os_store::qualified_synthetic_store','--ignored','--nocapture','--test-threads=1'],env=environment,text=True,capture_output=True,timeout=30)
  (self.state/(operation+'.stdout.log')).write_text(r.stdout);(self.state/(operation+'.stderr')).write_text(r.stderr)
  if r.returncode:raise RuntimeError('synthetic helper failed: '+operation)
  lines=[line for line in r.stdout.splitlines() if line.startswith('{')]
  if len(lines)!=1 or '1 passed; 0 failed' not in r.stdout:raise RuntimeError('fixture test did not execute exactly once')
  return json.loads(lines[0])

def main():
 os.umask(0o077)
 assert not Path('/run/dbus/system_bus_socket').exists()
 assert 'DISPLAY' not in os.environ and 'WAYLAND_DISPLAY' not in os.environ
 fixture=Fixture('/fixture/state');results=[]
 try:
  fixture.start()
  results.append(fixture.run_helper('exercise'));results.append(fixture.run_helper('probe'))
  fixture.stop_keyring();results.append(fixture.run_helper('unavailable'))
  fixture.start_keyring();results.append(fixture.run_helper('probe'));results.append(fixture.run_helper('cleanup'))
  items=fixture.bus_call('--dest','org.freedesktop.secrets','--object-path','/org/freedesktop/secrets','--method','org.freedesktop.Secret.Service.SearchItems','{}')
  if items!='(@ao [], @ao [])' and items!='([], [])':raise RuntimeError('owned keyring still has items after cleanup')
  record={'state':'passed','task':1335,'synthetic_only':True,'desktop_started':False,'mcp_claimed':False,'system_bus_absent':True,'activation_directories':False,'owned_service_empty_after_cleanup':True,'results':results,'processes':[{'pid':p.pid,'exit':p.poll()} for p in fixture.children]}
 finally:fixture.close()
 record['processes']=[{'pid':p.pid,'exit':p.poll()} for p in fixture.children]
 record['owned_processes_stopped']=all(p.poll() is not None for p in fixture.children)
 (fixture.state/'result.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(record))

if __name__=='__main__':main()
