#!/usr/bin/python3 -I
"""Local worker proof with a synthetic Git gate around real repository setup."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time


def wait_for(predicate, seconds=20):
    deadline = time.monotonic()+seconds
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.1)
    raise AssertionError('task-owned setup smoke did not settle')


def git_gate():
    root = Path('/probe-state')
    if (root / 'enabled').exists():
        with (root / 'entering').open('x') as output:
            json.dump({'git': os.getpid(), 'setup': os.getppid(), 'sid': os.getsid(0)}, output)
        (root / 'entering').rename(root / 'entered')
        wait_for(lambda: (root / 'release').exists(), 15)
    os.execv('/probe-git', ['/usr/bin/git', *sys.argv[1:]])


class LauncherSmoke:
    def __init__(self, smoke):
        self.smoke = smoke
        self.root = smoke.root
        self.container = None
        self.ssh = None
        self.state = self.root / 'probe-state'

    def execute(self, args, **kwargs):
        return subprocess.run(args, check=True, capture_output=True, timeout=120, **kwargs).stdout

    def worker(self, *args):
        self.smoke.inspect(self.container)
        return self.execute(self.smoke.docker + ['exec', self.container, *args])

    def no_clients(self):
        processes = self.worker('ps', '-eo', 'pid=,stat=,comm=').decode().splitlines()
        return not any(parts[0] != '1' and parts[2].startswith('sshd') and not parts[1].startswith('Z')
                       for line in processes if len(parts := line.split()) == 3)

    def request(self, command, request, code):
        output = subprocess.run(self.ssh+[command], input=json.dumps(request).encode(),
                                capture_output=True, timeout=45, check=False)
        assert output.returncode == code and not output.stderr, (output.returncode, output.stderr)
        if code == 3:
            assert not output.stdout
            return None
        assert output.stdout.endswith(b'\n') and len(output.stdout) <= 256*1024
        return json.loads(output.stdout)

    def start(self):
        self.state.mkdir(mode=0o700)
        gate = self.root / 'git-gate'
        shutil.copyfile(__file__, gate)
        gate.chmod(0o755)
        self.smoke.command('/usr/bin/true', [])
        original = self.root / 'git-original'
        self.execute(self.smoke.docker + ['cp', self.smoke.containers[-1]+':/usr/bin/git', str(original)])
        assert original.is_file() and not original.is_symlink()
        original.chmod(0o755)
        self.smoke.retire_completed()
        key = self.root / 'client'
        self.execute(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-f', str(key)])
        mounts = [(self.root / 'source/objects', '/objects', True),
                  (self.root / 'bundles', '/bundles', True), (self.root / 'retained', '/retained', False),
                  (gate, '/usr/bin/git', True), (original, '/probe-git', True),
                  (self.state, '/probe-state', False)]
        arguments = self.smoke.docker + ['create', '--pull=never', '--publish', '127.0.0.1::22',
            '--label', 'horizon.repository-image-smoke='+self.smoke.label,
            '--env', 'HORIZON_SSH_PUBLIC_KEY='+key.with_suffix('.pub').read_text().strip()]
        for source, target, readonly in mounts:
            arguments += ['--mount', f'type=bind,src={source},dst={target}'+(',readonly' if readonly else '')]
        self.container = self.execute(arguments+[self.smoke.image]).decode().strip()
        self.smoke.containers.append(self.container)
        self.smoke.inspect(self.container)
        self.execute(self.smoke.docker + ['start', self.container])
        ports = wait_for(lambda: self.smoke.inspect(self.container)['NetworkSettings']['Ports'].get('22/tcp'))
        port = ports[0]['HostPort']
        assert ports[0]['HostIp'] == '127.0.0.1' and port.isdecimal()

        def scan():
            result = subprocess.run(['ssh-keyscan', '-T', '1', '-p', port, '-t', 'ed25519', '127.0.0.1'],
                                    capture_output=True, timeout=3, check=False)
            return result.stdout if result.returncode == 0 and result.stdout else None

        known = self.root / 'known-hosts'
        known.write_bytes(wait_for(scan))
        self.ssh = ['ssh', '-T', '-F', '/dev/null', '-i', str(key), '-o', 'BatchMode=yes',
            '-o', 'IdentitiesOnly=yes', '-o', 'StrictHostKeyChecking=yes',
            '-o', 'UserKnownHostsFile='+str(known), '-p', port, 'root@127.0.0.1']
        wait_for(self.no_clients)

    def test(self):
        from test_repository_image import snapshot
        before_inputs = snapshot(self.root / 'source'), snapshot(self.root / 'bundles')
        self.start()
        retained = self.root / 'retained' / 'detached'
        retained.mkdir(mode=0o700)
        expected = dict(self.smoke.request, retained_root='/retained/detached', workspace_local_id='workspace_1')
        del expected['scratch_parent']
        absent = self.request('/usr/local/bin/horizon-repository setup-status', expected, 0)
        assert absent['status'] == 'absent' and not list(retained.iterdir())
        rejected = self.request('/usr/local/bin/horizon-setup-launch', {}, 2)
        assert rejected['state'] == 'observed' and rejected['observation']['status'] == 'rejected'
        assert not list(retained.iterdir())
        # This gate delays the real Git subprocess after the real setup child admitted.
        # It changes no source bytes and later execs the image's original Git binary.
        (self.state / 'enabled').touch()
        self.request('/usr/local/bin/horizon-setup-launch > /dev/full', expected, 3)
        wait_for(lambda: (self.state / 'entered').exists())
        entered = json.loads((self.state / 'entered').read_text())
        assert entered['setup'] == entered['sid'] and entered['git'] != entered['setup']
        assert self.worker('readlink', f'/proc/{entered["setup"]}/exe').decode().strip() == '/usr/local/bin/horizon-repository'
        wait_for(self.no_clients, 5)
        claim = retained / 'setup-claim.json'
        identity = claim.stat().st_dev, claim.stat().st_ino, claim.read_bytes()
        assert not (retained / 'setup-result.json').exists()
        observed = self.request('/usr/local/bin/horizon-setup-launch', expected, 4)
        assert observed['state'] == 'observed' and observed['observation']['status'] == 'claimed_unknown'
        wait_for(self.no_clients, 5)
        assert not (retained / 'setup-result.json').exists()
        # The original request channel is gone while admitted setup is still gated.
        (self.state / 'enabled').unlink()
        (self.state / 'release').touch()
        wait_for(lambda: (retained / 'setup-result.json').exists(), 30)
        assert self.no_clients() and self.smoke.inspect(self.container)['State']['Running']
        self.smoke.verify_checkout(retained / 'setup-data/published')
        finished = self.request('/usr/local/bin/horizon-repository setup-status', expected, 0)
        assert finished['status'] == 'completed' and finished['recording'] == 'observed'
        assert finished['execution']['state'] == 'published'
        assert finished['execution']['base_commit'] == self.smoke.base
        assert finished['execution']['bundle_manifest'] == self.smoke.manifest
        retained_state = snapshot(retained)
        again = self.request('/usr/local/bin/horizon-setup-launch', expected, 0)
        assert again == {'version': 1, 'state': 'observed', 'observation': finished}
        wrong = self.request('/usr/local/bin/horizon-setup-launch', dict(expected, workspace_local_id='different'), 1)
        assert wrong['state'] == 'observed' and wrong['observation']['status'] == 'error'
        assert (claim.stat().st_dev, claim.stat().st_ino, claim.read_bytes()) == identity
        assert snapshot(retained) == retained_state
        wait_for(lambda: subprocess.run(self.smoke.docker + ['exec', self.container, 'test', '!', '-e',
            '/proc/'+str(entered['setup'])], capture_output=True, timeout=5, check=False).returncode == 0, 5)
        # A separate ungated setup exercises normal submitted output on the same image.
        ordinary = self.root / 'retained' / 'ordinary'
        ordinary.mkdir(mode=0o700)
        normal = dict(expected, retained_root='/retained/ordinary')
        submitted = self.request('/usr/local/bin/horizon-setup-launch', normal, 0)
        assert submitted == {'version': 1, 'state': 'submitted', 'observation': None}
        wait_for(lambda: (ordinary / 'setup-result.json').exists(), 30)
        self.smoke.verify_checkout(ordinary / 'setup-data/published')
        assert before_inputs == (snapshot(self.root / 'source'), snapshot(self.root / 'bundles'))
        print('PASS actual detached setup: admitted Git-gated execution outlives request/output loss; '
              'separate status verifies exact repository; claimed/complete/conflicting requests never replay; '
              'setup child reaped; ungated submission and exact checkout pass. '
              'Git gate is synthetic, not cloud/PC-off or crash-durability proof.', flush=True)


def main():
    # Host invocation uses python3 explicitly; only the mounted Git gate uses -I.
    from test_repository_image import ImageSmoke
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image', required=True)
    parser.add_argument('--docker-host', required=True)
    parser.add_argument('--fixture-parent', default='/tmp')
    smoke = ImageSmoke(parser.parse_args())
    print(f'Task-owned setup fixture: {smoke.root}; label: {smoke.label}; image: {smoke.image}', flush=True)
    try:
        if smoke.user != '0:0':
            raise SystemExit('SSH setup smoke requires local rootless Docker or a root host caller; '
                             'the SSH worker runs as container root and must own its private fixtures')
        smoke.fixture()
        LauncherSmoke(smoke).test()
    finally:
        smoke.close()


if __name__ == '__main__':
    if Path(sys.argv[0]).name == 'git':
        git_gate()
    else:
        main()
