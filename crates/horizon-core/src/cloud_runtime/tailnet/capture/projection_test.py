import fcntl
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import time
import unittest

PROJECTION = Path(__file__).with_name('projection.py')


class ProjectionTest(unittest.TestCase):
    def setUp(self):
        for path in ['/workspace/.horizon-tailnet', '/run/horizon-tailnet']:
            shutil.rmtree(path, ignore_errors=True)
            Path(path).mkdir(mode=0o700)
        Path('/workspace/.horizon-tailnet/selection').write_text('saved-test-network')
        os.chmod('/workspace/.horizon-tailnet/selection', 0o600)
        Path('/run/horizon-tailnet/control.lock').touch(mode=0o600)
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.bind('/run/horizon-tailnet/tailscaled.sock')
        self.addCleanup(self.socket.close)
        Path('/usr/local/bin/tailscale').write_text("""#!/usr/bin/python3
import json,os,sys,time
from pathlib import Path
Path('/tmp/capture-args').write_text(json.dumps(sys.argv[1:]))
Path('/tmp/capture-pid').write_text(str(os.getpid()))
time.sleep(float(Path('/tmp/capture-delay').read_text()))
sys.stderr.write('synthetic private stderr')
sys.stdout.write(Path('/tmp/capture-status').read_text())
""")
        os.chmod('/usr/local/bin/tailscale', 0o755)
        Path('/tmp/capture-delay').write_text('0')
        self.report = {'BackendState': 'Running', 'Self': {'ID': 'nSyntheticOnly', 'Tags': ['tag:horizon-worker']},
                       'Peer': {'fake': {'ID': 'nNeverCapturePeer'}}, 'User': {'fake': 'synthetic-private-profile'}}

    def run_projection(self, raw=None, expected='saved-test-network', timeout=15):
        Path('/tmp/capture-status').write_text(json.dumps(self.report) if raw is None else raw)
        return subprocess.run(['/usr/bin/python3', '-I', '-', expected], input=PROJECTION.read_text(),
                              capture_output=True, text=True, timeout=timeout)

    def refuse(self, raw=None):
        result = self.run_projection(raw)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, '')
        self.assertEqual(result.stderr, '')

    def test_exact_authority_and_fixed_official_args(self):
        result = self.run_projection()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), {'version': 1, 'tailnet_id': 'saved-test-network', 'device_id': 'nSyntheticOnly'})
        self.assertEqual(json.loads(Path('/tmp/capture-args').read_text()),
                         ['--socket=/run/horizon-tailnet/tailscaled.sock', 'status', '--json', '--peers=false'])
        self.assertEqual(result.stderr, '')

    def test_status_tag_id_and_duplicate_refusals(self):
        for mutation in [
            lambda r: r.update(BackendState='NeedsLogin'),
            lambda r: r.update(Self=None),
            lambda r: r['Self'].update(Tags=[]),
            lambda r: r['Self'].update(Tags=['tag:horizon-worker', 'tag:other']),
            lambda r: r['Self'].update(ID=''),
            lambda r: r['Self'].update(ID=123),
            lambda r: r['Self'].pop('ID'),
            lambda r: r['Self'].update(ID='a' * 257),
        ]:
            original = json.loads(json.dumps(self.report))
            mutation(self.report)
            self.refuse()
            self.report = original
        self.refuse('{"BackendState":"Running","Self":{"ID":"one","ID":"two","Tags":["tag:horizon-worker"]}}')
        self.refuse('x' * 65537)
        self.assertNotEqual(self.run_projection(expected='other').returncode, 0)

    def test_private_permissions_owners_and_symlinks(self):
        for path, good, bad in [('/workspace', 0o755, 0o777),
                                ('/workspace/.horizon-tailnet', 0o700, 0o755),
                                ('/run/horizon-tailnet', 0o700, 0o755),
                                ('/workspace/.horizon-tailnet/selection', 0o600, 0o644),
                                ('/run/horizon-tailnet/control.lock', 0o600, 0o666),
                                ('/usr/local/bin/tailscale', 0o755, 0o777)]:
            os.chmod(path, bad)
            self.refuse()
            os.chmod(path, good)
            os.chown(path, 10001, 10001)
            self.refuse()
            os.chown(path, 0, 0)
        os.chmod('/usr/local/bin', 0o777)
        self.refuse()
        os.chmod('/usr/local/bin', 0o755)
        for path in ['/workspace/.horizon-tailnet/selection', '/run/horizon-tailnet/control.lock',
                     '/run/horizon-tailnet/tailscaled.sock', '/usr/local/bin/tailscale']:
            original = Path(path + '.original')
            Path(path).rename(original)
            Path(path).symlink_to(original)
            self.refuse()
            Path(path).unlink()
            original.rename(path)
        os.chown('/run/horizon-tailnet/tailscaled.sock', 10001, 10001)
        self.refuse()
        os.chown('/run/horizon-tailnet/tailscaled.sock', 0, 0)
        for path in ['/workspace/.horizon-tailnet', '/run/horizon-tailnet']:
            original = Path(path + '.original')
            Path(path).rename(original)
            Path(path).symlink_to(original)
            self.refuse()
            Path(path).unlink()
            original.rename(path)

    def test_agent_uid_cannot_query_or_project_the_private_daemon(self):
        Path('/tmp/capture-status').write_text(json.dumps(self.report))
        for path in ('/tmp/capture-args', '/tmp/capture-pid'):
            Path(path).unlink(missing_ok=True)
        def agent():
            os.setgroups([])
            os.setgid(10001)
            os.setuid(10001)
        result = subprocess.run(['/usr/bin/python3', '-I', '-', 'saved-test-network'],
                                input=PROJECTION.read_text(), capture_output=True, text=True,
                                timeout=3, preexec_fn=agent)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((result.stdout, result.stderr), ('', ''))
        self.assertFalse(Path('/tmp/capture-args').exists())
        self.assertFalse(Path('/tmp/capture-pid').exists())

    def test_locked_projection_and_child_deadline(self):
        with open('/run/horizon-tailnet/control.lock') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            started = time.monotonic()
            self.refuse()
            self.assertLess(time.monotonic() - started, 12)
        Path('/tmp/capture-delay').write_text('30')
        started = time.monotonic()
        self.refuse()
        self.assertLess(time.monotonic() - started, 12)
        self.assertFalse(Path('/proc', Path('/tmp/capture-pid').read_text()).exists())


if __name__ == '__main__':
    if os.geteuid() != 0:
        raise SystemExit('Use the isolated offline root Docker fixture.')
    unittest.main()
