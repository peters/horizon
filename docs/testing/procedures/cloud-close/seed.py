"""Seeds the private home of the fixture with three synthetic clouds and no provider settings."""
import json, os, sys, time, uuid
from pathlib import Path

tools = Path(__file__).resolve().parent
config = Path(sys.argv[1]).resolve()
home = Path(os.environ['HOME']) / '.horizon'
sessions = home / 'sessions'
if sessions.exists():
    sys.exit(0)

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
    'label': 'demo-api', 'workspace_count': 3, 'panel_count': 3, 'started_at': now, 'last_active_at': now}))
(sessions / 'index.yaml').write_text(json.dumps({'version': 1, 'profiles': [
    {'profile_id': profile, 'last_session_id': session, 'recent_session_ids': [session]}]}))
Path('/tmp/demo-repo').mkdir(exist_ok=True)
profile_spec = {'provider': 'runpod', 'image': 'registry.example/worker', 'cpu': 4, 'memory_gb': 8}
cloud = home / 'cloud' / 'demo-api'
cloud.mkdir(parents=True)
(cloud / 'deployment.json').write_text(json.dumps({
    'version': 1, 'cloud_id': 'demo-api', 'repository': '/tmp/demo-repo', 'revision': 'a' * 40,
    'profile': profile_spec, 'stage': 'Ready',
    'operation': {'state': 'bound', 'worker_id': 'pod-7f3a2c'}, 'sessions': [],
    'spec': {'operation_id': 'fixture', 'image_digest': 'sha256:' + '0' * 64, 'profile': profile_spec,
             'public_key': 'ssh-ed25519 fixture', 'registry_auth_id': None,
             'gpu_types': [], 'cpu_flavors': [], 'data_centers': []}}))
# A cloud whose image push failed before a worker was requested, with storage on record.
pushed = home / 'cloud' / 'image-push'
pushed.mkdir(parents=True)
(pushed / 'deployment.json').write_text(json.dumps({
    'version': 1, 'cloud_id': 'image-push', 'repository': '/tmp/demo-repo', 'revision': 'a' * 40,
    'profile': profile_spec, 'stage': 'Push', 'operation': {'state': 'prepared'}, 'sessions': []}))
(pushed / 'workspace-volume.required').write_text('')
