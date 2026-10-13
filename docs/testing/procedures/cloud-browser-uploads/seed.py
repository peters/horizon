"""Seeds the private home of the fixture with the cloud that cloud_deploy deployed.

The directory of this file holds `cloud-id`, `cloud/<cloud-id>/` (the state that
cloud_deploy wrote), `identity` (the SSH key of the cloud settings) and
`hetzner-token` (the Hetzner token file). The fixture copies them into its
private home and opens a workspace bound to the cloud.
"""
import json, os, shutil, sys, time, uuid
from pathlib import Path

here = Path(__file__).resolve().parent
config = Path(sys.argv[1]).resolve()
home = Path(os.environ['HOME']) / '.horizon'
sessions = home / 'sessions'
if sessions.exists():
    sys.exit(0)
cloud_id = (here / 'cloud-id').read_text().strip()
source = here / 'cloud' / cloud_id
deployment = json.loads((source / 'deployment.json').read_text())

def fnv(text):
    value = 0xcbf29ce484222325
    for byte in text.encode():
        value ^= byte
        value = (value * 0x100000001b3) & 0xFFFFFFFFFFFFFFFF
    return f'{value:016x}'

workspace = str(uuid.uuid4())
session = str(uuid.uuid4())
profile = fnv(str(config))
now = int(time.time() * 1000)
runtime = {
    'version': 3,
    'window': {'width': 1600.0, 'height': 1000.0, 'x': None, 'y': None},
    'canvas_view': {'pan_offset': [0.0, 0.0], 'zoom': 1.0},
    'active_workspace_local_id': workspace,
    'focused_panel_local_id': None,
    'workspaces': [{'local_id': workspace, 'name': 'upload-demo', 'cwd': deployment['repository'], 'position': [0.0, 40.0],
                    'template': None, 'layout': None, 'panels': []}],
    'cloud_groups': [{
        'remote': {'deployment_started': True, 'id': cloud_id, 'revision': deployment['revision'],
                   'profile_name': 'uploads', 'profile': deployment['profile']},
        'issue': 101, 'title': 'upload-demo', 'workspace': workspace,
        'environment': {'id': 'issue-101', 'image': 'development-image (mock)', 'connection': 'LocalPrototype',
                        'profile': None, 'provider': None},
        'cwd': deployment['repository'], 'position': [40.0, 40.0], 'workspace_position': [40.0, 0.0], 'size': [1200.0, 820.0],
        'collapsed': False, 'layout': None, 'panels': [], 'hidden': []}],
}
(sessions / session).mkdir(parents=True)
(sessions / session / 'runtime.yaml').write_text(json.dumps(runtime))
(sessions / session / 'meta.yaml').write_text(json.dumps({
    'version': 1, 'session_id': session, 'profile_id': profile, 'config_path': str(config),
    'label': 'upload-demo', 'workspace_count': 1, 'panel_count': 0, 'started_at': now, 'last_active_at': now}))
(sessions / 'index.yaml').write_text(json.dumps({'version': 1, 'profiles': [
    {'profile_id': profile, 'last_session_id': session, 'recent_session_ids': [session]}]}))
cloud_root = home / 'cloud'
cloud_root.mkdir(parents=True, exist_ok=True)
identity = cloud_root / 'smoke-identity'
shutil.copyfile(here / 'identity', identity)
identity.chmod(0o600)
token = cloud_root / 'hetzner-token'
shutil.copyfile(here / 'hetzner-token', token)
token.chmod(0o600)
(cloud_root / 'settings.json').write_text(json.dumps({
    'runpod_key_file': str(cloud_root / 'no-provider-key'), 'ssh_identity_file': str(identity),
    'docker_config': str(cloud_root / 'docker'), 'registry_pull_auth_id': None,
    'cpu_flavors': [], 'gpu_types': [],
    'hetzner': {'token_file': str(token), 'server_types': ['cx23'], 'locations': ['fsn1']}}))
shutil.copytree(source, cloud_root / cloud_id)
