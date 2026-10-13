"""Seeds the private home of the fixture with a ready cloud on the stand-in worker.

The cloud record has no provider operation in progress and the settings name no provider
key, so Horizon sends no request to a provider. worker.py writes worker.json next to
this file with the port of the stand-in SSH server and its client key.
"""
import json
import os
import shutil
import sys
import time
import uuid
from pathlib import Path

tools = Path(__file__).resolve().parent
config = Path(sys.argv[1]).resolve()
home = Path(os.environ['HOME']) / '.horizon'
sessions = home / 'sessions'
if sessions.exists():
    sys.exit(0)
worker = json.loads((tools / 'worker.json').read_text())


def fnv(text):
    value = 0xcbf29ce484222325
    for byte in text.encode():
        value ^= byte
        value = (value * 0x100000001b3) & 0xFFFFFFFFFFFFFFFF
    return f'{value:016x}'


session = str(uuid.uuid4())
profile = fnv(str(config))
now = int(time.time() * 1000)
(sessions / session).mkdir(parents=True)
(sessions / session / 'runtime.yaml').write_text((tools / 'runtime.yaml').read_text())
(sessions / session / 'meta.yaml').write_text(json.dumps({
    'version': 1, 'session_id': session, 'profile_id': profile, 'config_path': str(config),
    'label': 'drop-demo', 'workspace_count': 2, 'panel_count': 1, 'started_at': now, 'last_active_at': now}))
(sessions / 'index.yaml').write_text(json.dumps({'version': 1, 'profiles': [
    {'profile_id': profile, 'last_session_id': session, 'recent_session_ids': [session]}]}))

cloud_root = home / 'cloud'
cloud_root.mkdir(parents=True)
identity = cloud_root / 'stand-in-identity'
shutil.copyfile(worker['identity'], identity)
identity.chmod(0o600)
(cloud_root / 'settings.json').write_text(json.dumps({
    'runpod_key_file': str(cloud_root / 'no-provider-key'), 'ssh_identity_file': str(identity),
    'docker_config': str(cloud_root / 'docker'), 'registry_pull_auth_id': None,
    'cpu_flavors': [], 'gpu_types': []}))
repository = home / 'drop-demo-repository'
repository.mkdir()
profile_spec = {'provider': 'runpod', 'image': 'registry.example/worker', 'cpu': 4, 'memory_gb': 8}
cloud = cloud_root / 'drop-demo'
cloud.mkdir()
(cloud / 'deployment.json').write_text(json.dumps({
    'version': 1, 'cloud_id': 'drop-demo', 'repository': str(repository), 'revision': 'a' * 40,
    'profile': profile_spec, 'stage': 'Ready', 'operation': {'state': 'prepared'}, 'spec': None,
    'worker': {'id': 'stand-in', 'name': 'stand-in', 'imageName': 'registry.example/worker',
               'desiredStatus': 'RUNNING', 'publicIp': '127.0.0.1', 'portMappings': {'22': worker['port']}},
    'sessions': [], 'source_ready': True}))
