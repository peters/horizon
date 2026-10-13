"""One Upkeep exchange on a worker, as root: install the loop, keep its session running,
apply Run now and Pause, and print what the board shows. Idempotent."""
import base64
import json
import os
import subprocess
import sys

HOME = '/workspace/home'
DIR = HOME + '/upkeep'
STATUS_LIMIT = 2 * 1024 * 1024

payload = json.loads(base64.b64decode(sys.argv[1]))
session = payload['session']
owner = os.stat(HOME)
uid, gid = owner.st_uid, owner.st_gid
# Sessions run as the agent account where the worker isolates agents: their bindings and
# their tmux server belong to it, so the loop's must too, or no panel can attach to it.
ISOLATED = os.path.exists('/run/horizon-tailnet/agent-isolation')


def as_agent(command):
    return ['horizon-worker-tailnet', 'agent', *command] if ISOLATED else command


def own(path):
    os.chown(path, uid, gid)


def write(path, text, mode=0o600):
    data = text.encode()
    try:
        with open(path, 'rb') as current:
            if current.read() == data:
                return
    except OSError:
        pass
    temporary = path + '.new'
    with open(temporary, 'wb') as handle:
        handle.write(data)
    os.chmod(temporary, mode)
    own(temporary)
    os.replace(temporary, path)


os.makedirs(DIR, exist_ok=True)
own(DIR)
write(DIR + '/config.json', payload['config'])
write(DIR + '/prompt.md', payload['prompt'])
write(DIR + '/run.sh', payload['run'], 0o700)
write(DIR + '/run.py', payload['scheduler'], 0o600)

# The binding that makes the session a Codex session to the worker's services: its
# processes may ask for GitHub access and open browser panels for this cloud.
binding = '/workspace/sessions/' + session
os.makedirs(binding, exist_ok=True)
os.chmod(binding, 0o700)
own(binding)
for name, text in (('agent', 'codex\n'), ('revision', payload['revision'] + '\n'),
                   ('shared-checkout', ''), ('launch-requested', '')):
    path = os.path.join(binding, name)
    if not os.path.exists(path) or name in ('agent', 'revision'):
        with open(path, 'w') as handle:
            handle.write(text)
    os.chmod(path, 0o600)
    own(path)
data = '/workspace/session-data/' + session
os.makedirs(data, exist_ok=True)
os.chmod(data, 0o700)
own(data)

if payload.get('run_now'):
    open(DIR + '/run-now', 'a').close()
    own(DIR + '/run-now')
paused = payload.get('paused')
if paused is True:
    open(DIR + '/paused', 'a').close()
elif paused is False and os.path.exists(DIR + '/paused'):
    os.remove(DIR + '/paused')


def tmux(*args):
    return subprocess.run(as_agent(['tmux', '-L', 'horizon-cloud', *args]), capture_output=True, text=True)


if ISOLATED:
    # An earlier exchange may have started the loop on root's server, where no panel finds it.
    subprocess.run(['tmux', '-L', 'horizon-cloud', 'kill-session', '-t', '=' + session], capture_output=True)
# A reconnect relaunches recorded sessions as interactive agents; only the loop belongs here.
start = tmux('display-message', '-p', '-t', '=' + session + ':', '#{pane_start_command}')
loop = start.returncode == 0 and 'upkeep/run.sh' in start.stdout
# A loop that already ran before this exchange has survived its start; only then is its
# session ready for a panel to attach.
settled = loop
if start.returncode == 0 and not loop:
    tmux('kill-session', '-t', '=' + session)
if not loop:
    command = ['/bin/bash', '--noprofile', '--norc', DIR + '/run.sh', session]
    started = tmux('new-session', '-d', '-s', session, '-c', DIR,
                   '-e', 'PATH=' + os.environ.get('PATH', '/usr/local/bin:/usr/bin:/bin'),
                   '-e', 'BASH_ENV=', '-e', 'ENV=', *command)
    loop = started.returncode == 0

status = None
try:
    with open(DIR + '/status.json', 'rb') as handle:
        status = handle.read(STATUS_LIMIT).decode('utf-8', 'replace')
except OSError:
    pass
runs = []
try:
    with open(DIR + '/runs.jsonl') as handle:
        for line in handle.readlines()[-200:]:
            try:
                runs.append(json.loads(line))
            except ValueError:
                pass
except OSError:
    pass
next_run = None
try:
    with open(DIR + '/next-run') as handle:
        next_run = int(handle.read().strip())
except (OSError, ValueError):
    pass
print(json.dumps({'loop': loop, 'settled': settled, 'status': status, 'runs': runs, 'next_run': next_run,
                  'paused': os.path.exists(DIR + '/paused')}))
