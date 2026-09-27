#!/usr/bin/env python3
"""Real SSH initialization, inspection, recovery, key restart and abandonment smoke. Requires paramiko, OpenSSH, tmux, git-lfs and bubblewrap.

Run from the candidate checkout with its isolated Cargo target directory:
  python scripts/cloud-initialization-smoke.py --worker /frozen/horizon-cloud-worker \
    --evidence /absolute/new/private-directory
The host fixture seeds synthetic provider ownership; no provider API is used.
The actual worker creates its own bootstrap state on an isolated empty mount.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import threading
import tempfile
import time
import traceback

import paramiko
import cloud_attachment_smoke as attachment

LIMIT = 64 * 1024
COMMANDS = {b"horizon-cloud-worker " + name: name.decode() for name in
            [b"initialize-allocation", b"recover-allocation", b"inspect-allocation", b"abandon-bootstrap", b"reserve-project", b"reserve-project-session", b"prepare-project-session", b"start-project-session", b"stop-project-session", b"inspect-project-session", b"cancel-project-reservation", b"prepare-project-namespace", b"prepare-project-source", b"import-project-source"]}
COMMANDS[b"cat /run/sshd/horizon-allocation/runtime.json"] = "runtime"

# This service keeps one task-owned PID namespace alive across SSH commands.
# Killing its bubblewrap parent tears down every fixture-only descendant.
NAMESPACE_SERVICE = r'''
import base64,json,os,signal,socket,subprocess,tempfile,uuid,threading
import sys;sys.path.insert(0,'/control')
from cloud_attachment_smoke import serve_terminal
s=socket.socket(socket.AF_UNIX);s.bind('/control/service.sock');s.listen(4)
while True:
 c,_=s.accept()
 try:
  line=c.makefile('rb').readline(24*1024*1024)
  q=json.loads(line)
  if q['command']=='attach-project-session':
   threading.Thread(target=serve_terminal,args=(c,q)).start();continue
  args=['/worker',q['command']]
  if q['command']=='start-project-session' and os.path.exists('/control/stop-race'):
   args=['/usr/bin/strace','-D','-ff','-o','/control/race-trace','-e','inject=setsid:signal=SIGSTOP']+args
  # Regular files let a detached fault tracer retain descriptors without
  # extending communicate() beyond the actual worker request lifetime.
  with tempfile.TemporaryFile() as out,tempfile.TemporaryFile() as err:
   p=subprocess.run(args,input=base64.b64decode(q['request']),stdout=out,stderr=err,timeout=180,env=dict(os.environ,**q['environment']))
   out.seek(0);err.seek(0);p.stdout=out.read(65537);p.stderr=err.read(65537)
  if q['command']=='stop-project-session' and p.returncode==0 and os.path.exists('/control/stop-race'):
   session=str(uuid.UUID(json.loads(json.loads(base64.b64decode(q['request']))['payload'])['session_id']))
   with open('/workspace/.horizon-allocation/runtime-'+session+'.json') as record:
    supervisor=json.load(record)['supervisor']['pid']
   os.kill(supervisor,signal.SIGCONT)
  if len(p.stdout)>65536 or len(p.stderr)>65536:raise ValueError('worker output exceeded bound')
  reply={'exit':p.returncode,'stdout':base64.b64encode(p.stdout).decode(),'stderr':base64.b64encode(p.stderr).decode()}
 except Exception as e:reply={'exit':125,'stdout':'','stderr':base64.b64encode(str(e).encode()).decode()}
 c.sendall(json.dumps(reply).encode()+b'\n');c.close()
'''

SYNTHETIC_AGENT = r'''#!/usr/bin/python3
import ctypes,os,pathlib,select,signal,sys,time
if sys.argv[1:]==['--version']:
 print('2.1.283 (Claude Code)');sys.exit(0)
assert sys.argv[1:]==['--safe-mode','--strict-mcp-config','--mcp-config','{"mcpServers":{}}','--setting-sources','','--disable-slash-commands','--no-chrome']
assert not any(k in os.environ for k in ['HORIZON','SSH_AUTH_SOCK','ANTHROPIC_API_KEY','OPENAI_API_KEY','DISPLAY'])
home=pathlib.Path(os.environ['HOME']);work=pathlib.Path.cwd()
with (home/'launch-count').open('a') as f:f.write('launch\n')
(home/'runtime-environment.json').write_text(__import__('json').dumps(dict(os.environ)))
# An intermediate subreaper and fast exits exercise repeated adoption at stop.
ready_read,ready_write=os.pipe()
if os.fork()==0:
 os.close(ready_read)
 assert ctypes.CDLL(None).prctl(36,1,0,0,0)==0
 for _ in range(5):
  if os.fork()==0:os._exit(0)
 if os.fork()!=0:
  os.close(ready_write)
  while True:time.sleep(.05)
 os.setsid()
 if os.fork():os._exit(0)
 signal.signal(signal.SIGTERM,signal.SIG_IGN);signal.signal(signal.SIGHUP,signal.SIG_IGN)
 with (home/'descendant-progress').open('a') as f:f.write('x')
 os.write(ready_write,b'1');os.close(ready_write)
 while True:
  with (home/'descendant-progress').open('a') as f:f.write('x')
  time.sleep(.05)
os.close(ready_write)
assert select.select([ready_read],[],[],5)[0] and os.read(ready_read,1)==b'1'
os.close(ready_read)
print('interactive-ready',flush=True)
while True:
 if select.select([sys.stdin],[],[],0)[0]:
  line=sys.stdin.readline().strip()
  with (home/'terminal-input').open('a') as f:f.write(line+'\n')
  print('echo:'+line,flush=True)
 (home/'terminal-size').write_text(str(os.get_terminal_size().columns)+' '+str(os.get_terminal_size().lines))
 with (work/'runtime-progress').open('a') as f:f.write('x')
 if (home/'exit-agent').exists():sys.exit(17)
 if (home/'lose-runtime').exists():
  fault=(home/'lose-runtime').read_text();(home/'lose-runtime').unlink()
  if fault=='supervisor':os.kill(int(str(home).split('/')[2]),signal.SIGKILL)
  elif fault=='server':os.kill(os.getppid(),signal.SIGKILL)
  elif fault=='socket':
   socket=pathlib.Path(os.environ['TMUX'].split(',')[0]);socket.unlink();socket.write_text('foreign socket replacement')
  else:raise ValueError('unknown fixture fault')
 time.sleep(.05)
'''

class RuntimeNamespace:
    def __init__(self, root):
        self.root, self.process = root, None
        self.lock = threading.Lock()
        (root / "control").mkdir(mode=0o700)
        shutil.copy2(Path(__file__).with_name("cloud_attachment_smoke.py"), root / "control/cloud_attachment_smoke.py")
        (root / "managed").mkdir(mode=0o700)

    def exchange(self, arguments, command, request, environment):
        with self.lock:
            if self.process is None:
                args = arguments + ["--bind", str(self.root / "control"), "/control", "/usr/bin/python3", "-u", "-c", NAMESPACE_SERVICE]
                self.process = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                deadline = time.monotonic() + 10
                while not (self.root / "control/service.sock").exists():
                    if self.process.poll() is not None or time.monotonic() > deadline:
                        raise RuntimeError("Persistent fixture namespace failed")
                    time.sleep(.02)
            if hasattr(request, "read"):
                request = request.read(16*1024*1024+1)
            if len(request) > 16*1024*1024:
                raise ValueError("Runtime smoke source exceeds fixture-only bound")
            with socket.socket(socket.AF_UNIX) as client:
                client.settimeout(190)
                client.connect(str(self.root / "control/service.sock"))
                client.sendall(json.dumps({"command":command,"request":base64.b64encode(request).decode(),"environment":environment}).encode()+b"\n")
                reply = json.loads(client.makefile("rb").readline(256*1024))
            return subprocess.CompletedProcess([], reply["exit"], base64.b64decode(reply["stdout"]), base64.b64decode(reply["stderr"]))

    def close(self):
        if self.process is not None:
            self.process.kill()
            self.process.wait(timeout=10)


def run(options):
    os.umask(0o077)
    root = Path(options.evidence)
    if not root.is_absolute():
        raise ValueError("Evidence directory must be absolute and new")
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    (root / "workspace").mkdir(mode=0o700)
    shutil.copyfile(options.worker, root / "worker")
    (root / "worker").chmod(0o700)
    worker_hash = hashlib.sha256((root / "worker").read_bytes()).hexdigest()
    checker_root = Path(__file__).resolve().parent.parent / "examples/cloud-worker"
    sshd = Path(options.sshd or shutil.which("sshd") or "")
    if not sshd.is_absolute() or not sshd.is_file() or not os.access(sshd, os.X_OK):
        raise ValueError("Provide an actual OpenSSH server executable with --sshd")
    (root / "bin").mkdir(mode=0o700)
    for name in ["horizon-worker-check", "horizon-worker-session", "horizon-worker-import", "horizon-worker-source", "horizon-worker-supervise"]:
        shutil.copy2(checker_root / name, root / "bin" / name)
        (root / "bin" / name).chmod(0o700)
    shutil.copy2(sshd, root / "bin/sshd")
    os.link(root / "worker", root / "bin/horizon-cloud-worker")
    image_capabilities = {}
    if options.scenario in ["sources", "runtime", "attachment"]:
        # Version-only probes qualify reservation routing, never agent startup.
        image_capabilities["agents"] = ["codex", "claude"]
        for name in image_capabilities["agents"]:
            probe = root / "bin" / name
            probe.write_text('#!/usr/bin/sh\n[ "$#" -eq 1 ] && [ "$1" = "--version" ] || exit 64\nprintf "%s\\n" "fixture agent 1.0.0"\n')
            probe.chmod(0o700)
        if options.scenario in ["runtime", "attachment"]:
            (root / "bin/claude").write_text(SYNTHETIC_AGENT)
    (root / "image-capabilities.json").write_text(json.dumps(image_capabilities))
    subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "recovery-fixture", "-f", str(root / "id_ed25519")], check=True)
    allowed = base64.b64decode((root / "id_ed25519.pub").read_text().split()[1])
    (root / "run").mkdir(mode=0o700)
    server_lock = threading.Lock()
    host_keys = []
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.settimeout(0.2)
    (root / "fixture.json").write_text(json.dumps({"port": listener.getsockname()[1], "worker_sha256": worker_hash}))
    stop = threading.Event()
    sessions = []
    errors = []
    children = []
    namespace = RuntimeNamespace(root) if options.scenario in ["runtime", "attachment"] else None
    if namespace and options.runtime_fault == "stop-race":
        (root / "control/stop-race").touch()

    class Server(paramiko.ServerInterface):
        def __init__(self):
            self.executing = threading.Event()
            self.command = None
            self.dimensions = (90, 25)
            self.encoded = None

        def get_allowed_auths(self, username):
            return "publickey"

        def check_auth_publickey(self, username, key):
            if username == "root" and key.asbytes() == allowed:
                return paramiko.AUTH_SUCCESSFUL
            return paramiko.AUTH_FAILED

        def check_channel_request(self, kind, chanid):
            return paramiko.OPEN_SUCCEEDED if kind == "session" else paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED

        def check_channel_pty_request(self, channel, term, width, height, pixelwidth, pixelheight, modes):
            self.dimensions = (width, height)
            return namespace is not None

        def check_channel_window_change_request(self, channel, width, height, pixelwidth, pixelheight):
            self.dimensions = (width, height)
            return True

        def check_channel_exec_request(self, channel, command):
            prefix = b"horizon-cloud-worker attach-project-session "
            if command.startswith(prefix) and namespace is not None:
                encoded = command[len(prefix):]
                if not encoded or len(encoded) > 32768 or any(c not in b"0123456789abcdef" for c in encoded):
                    return False
                self.command, self.encoded = "attach-project-session", encoded.decode()
                self.executing.set()
                return True
            if command not in COMMANDS:
                return False
            self.command = COMMANDS[command]
            self.executing.set()
            return True

    def worker(command, request=b"", startup=False):
        arguments = ["bwrap", "--unshare-all", "--die-with-parent", "--new-session",
                     "--ro-bind", "/usr", "/usr", "--ro-bind", "/lib", "/lib", "--ro-bind", "/lib64", "/lib64",
                     "--proc", "/proc", "--dev", "/dev", "--dir", "/tmp",
                     "--ro-bind", "/etc/passwd", "/etc/passwd", "--ro-bind", "/etc/group", "/etc/group",
                     "--bind", str(root / "workspace"), "/workspace", "--bind", str(root / "run"), "/run/sshd", "--clearenv",
                     "--setenv", "PATH", "/usr/bin:/bin", "--setenv", "HOME", "/tmp"]
        runtime = {}
        if startup:
            runtime = json.loads((root / "runtime.json").read_text())
            for key, value in (runtime.items() if namespace is None else []):
                arguments.extend(["--setenv", key, value])
        arguments.extend(["--ro-bind", str(root / "bin"), "/usr/local/bin",
                          "--ro-bind", str(root / "image-capabilities.json"), "/etc/horizon-worker/capabilities.json"])
        arguments.extend(["--ro-bind", str(root / "worker"), "/worker"])
        if namespace is not None:
            arguments.extend(["--symlink", "usr/bin", "/bin", "--bind", str(root / "managed"), "/etc/claude-code"])
            return namespace.exchange(arguments, command, request, runtime)
        arguments.extend(["/worker", command])
        if hasattr(request, "read"):
            return subprocess.run(arguments, stdin=request, capture_output=True, timeout=180)
        return subprocess.run(arguments, input=request, capture_output=True, timeout=180)

    def handle(sock):
        transport = paramiko.Transport(sock)
        try:
            with server_lock:
                restarting = (root / "restart").exists()
                if restarting:
                    if namespace is None:
                        shutil.rmtree(root / "run")
                        (root / "run").mkdir(mode=0o700)
                    (root / "restart").unlink()
                if namespace is None or restarting:
                    prepared = worker("prepare-allocation-ssh", startup=True)
                    if prepared.returncode:
                        raise RuntimeError("Worker startup preparation failed: " + prepared.stderr.decode())
                host_key = paramiko.Ed25519Key.from_private_key_file(str(root / "run/horizon-allocation/ssh-host-key"))
                host_keys.append(host_key.get_base64())
            transport.add_server_key(host_key)
            server = Server()
            transport.start_server(server=server)
            channel = transport.accept(10)
            if channel is None or not server.executing.wait(10):
                raise RuntimeError("No allowed SSH command received")
            channel.settimeout(10)
            if server.encoded is not None:
                code = attachment.forward(channel, root, server.encoded, lambda: server.dimensions)
                sessions.append({"command":server.command,"exit_code":code,"worker_sha256":worker_hash})
                if not channel.closed:
                    channel.send_exit_status(code if code >= 0 else 0)
                    channel.close()
                return
            with tempfile.TemporaryFile() as request:
                request_hash = hashlib.sha256()
                length = 0
                limit = 4 * 1024**3 + 65540 if server.command == "import-project-source" else LIMIT
                while True:
                    chunk = channel.recv(65536)
                    if not chunk:
                        break
                    length += len(chunk)
                    if length > limit:
                        raise RuntimeError("SSH request exceeded its bound")
                    request.write(chunk)
                    request_hash.update(chunk)
                request.seek(0)
                if server.command == "runtime":
                    result = subprocess.CompletedProcess([], 0, (root / "run/horizon-allocation/runtime.json").read_bytes(), b"")
                else:
                    result = worker(server.command, request)
            if len(result.stdout) > LIMIT or len(result.stderr) > LIMIT:
                raise RuntimeError("Worker output exceeded its bound")
            sessions.append({"command": server.command, "request_sha256": request_hash.hexdigest(), "worker_sha256": worker_hash, "exit_code": result.returncode, "stderr": result.stderr.decode(errors="replace")[:2048]})
            channel.sendall(result.stdout)
            channel.send_exit_status(result.returncode)
            channel.shutdown_write()
            channel.close()
            # Let the client consume exit status and disconnect before closing TCP.
            transport.join(timeout=2)
        except Exception as error:
            errors.append(type(error).__name__ + ": " + str(error))
            with (root / "fixture-errors.log").open("a") as log:
                log.write(traceback.format_exc())
        finally:
            transport.close()
            sock.close()

    def accept():
        # Advertise the reserved port before sshd is ready. The owning host must
        # retry read-only enrollment while retaining this live creation attempt.
        while not (root / "runtime.json").exists():
            if stop.wait(0.05):
                return
        if stop.wait(1):
            return
        if namespace is not None:
            # PDEATHSIG follows the creating thread. The accept thread outlives
            # every SSH handler, so disconnecting a client cannot kill the worker.
            prepared = worker("prepare-allocation-ssh", startup=True)
            if prepared.returncode:
                errors.append("Persistent worker startup failed: " + prepared.stderr.decode())
                return
        listener.listen(4)
        while not stop.is_set():
            try:
                sock, _ = listener.accept()
            except socket.timeout:
                continue
            child = threading.Thread(target=handle, args=(sock,))
            children.append(child)
            child.start()

    thread = threading.Thread(target=accept)
    thread.start()
    test_exit = None
    try:
        environment = dict(os.environ, HORIZON_INITIALIZATION_FIXTURE=str(root))
        if namespace is not None:
            environment["HORIZON_RUNTIME_SMOKE"] = "1"
            environment["HORIZON_RUNTIME_FAULT"] = options.runtime_fault
            if options.scenario == "attachment":
                environment["HORIZON_ATTACHMENT_SMOKE"] = "1"
                if options.attachment_race:
                    environment["HORIZON_ATTACHMENT_RACE"] = "1"
        test_name = {"initialization": "native_ssh_worker_initialization",
                     "reservations": "native_ssh_project_reservations",
                     "host-reservations": "native_ssh_host_reservation_recovery",
                     "sources": "native_ssh_project_sources",
                     "runtime": "native_ssh_project_sources",
                     "attachment": "native_ssh_project_sources",
                     "namespaces": "native_ssh_project_namespaces"}[options.scenario]
        with open(root / "test.log", "w") as output:
            test_exit = subprocess.run(["cargo", "test", "-p", "horizon-core", test_name, "--lib", "--", "--ignored", "--nocapture"], env=environment, stdout=output, stderr=subprocess.STDOUT, timeout=600 if options.scenario in ["runtime", "attachment"] else 300).returncode
    except (subprocess.TimeoutExpired, OSError) as error:
        errors.append(type(error).__name__ + ": " + str(error))
    finally:
        stop.set()
        thread.join(timeout=5)
        listener.close()
        for child in children:
            child.join(timeout=25)
        if namespace is not None:
            namespace.close()
        (root / "id_ed25519").unlink(missing_ok=True)
        for path in [root / "run/horizon-allocation/ssh-host-key", root / "workspace/.horizon-allocation/ssh-host-key"]:
            path.unlink(missing_ok=True)
    report = {"worker_sha256": worker_hash, "test_exit": test_exit, "same_host_key": len(set(host_keys)) == 1, "sessions": sessions, "errors": errors, "threads_stopped": not thread.is_alive() and all(not child.is_alive() for child in children), "namespace_stopped": namespace is None or namespace.process is None or namespace.process.poll() is not None}
    (root / "ssh-report.json").write_text(json.dumps(report, indent=2))
    assert test_exit == 0 and not errors and report["threads_stopped"] and report["namespace_stopped"], "Inspect private test.log and ssh-report.json"
    if options.scenario in ["runtime", "attachment"]:
        assert report["same_host_key"]
        expected_starts = 1 if options.runtime_fault in ["stop-race", "early-exit"] else 6
        assert len({s["request_sha256"] for s in sessions if s["command"] == "start-project-session"}) == expected_starts
        assert len([s for s in sessions if s["command"] == "inspect-project-session"]) >= expected_starts
        print(json.dumps({"passed":True,"ssh_sessions":len(sessions),"worker_sha256":worker_hash,"namespace_stopped":True}))
        return
    expected = {"initialization": [0, 0, 0, 0, 0, 0, 1, 0, 0, 1, 1],
                "reservations": [0] * 7 + [1] * 4 + [0, 0, 1, 0, 1, 1, 1],
                "host-reservations": [0] * 11, "namespaces": [0] * 13, "sources": [0] * 42}[options.scenario]
    assert [session["exit_code"] for session in sessions] == expected
    assert report["same_host_key"]
    if options.scenario == "sources":
        agent_sessions = [session for session in sessions if session["command"] == "reserve-project-session"]
        prepared = [session for session in sessions if session["command"] == "prepare-project-session"]
        assert len(prepared) == 11
        assert len({session["request_sha256"] for session in prepared}) == 6
        assert len(agent_sessions) == 11
        assert len({session["request_sha256"] for session in agent_sessions}) == 6
    assert len({session["request_sha256"] for session in sessions if session["command"] == "recover-allocation"}) == 1
    if options.scenario in ["host-reservations", "namespaces", "sources"]:
        assert not any(session["command"] == "abandon-bootstrap" for session in sessions)
        assert len({session["request_sha256"] for session in sessions if session["command"] == "cancel-project-reservation"}) == 1
    else:
        assert len({session["request_sha256"] for session in sessions if session["command"] == "abandon-bootstrap"}) == 1
    print(json.dumps({"passed": True, "ssh_sessions": len(sessions), "same_host_key": True, "worker_sha256": worker_hash}))



if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scenario", choices=["initialization", "reservations", "host-reservations", "namespaces", "sources", "runtime", "attachment"], default="initialization")
    parser.add_argument("--attachment-race", action="store_true", help="Pause attachment after connecting, then commit stop before opening its terminal gate")
    parser.add_argument("--runtime-fault", choices=["supervisor", "server", "socket", "stop-race", "early-exit"], default="supervisor")
    parser.add_argument("--worker", required=True)
    parser.add_argument("--evidence", required=True)
    parser.add_argument("--sshd", help="Actual OpenSSH server binary; may be extracted into a task-local directory")
    run(parser.parse_args())
