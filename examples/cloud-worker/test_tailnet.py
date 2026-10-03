import importlib.machinery
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

loader = importlib.machinery.SourceFileLoader('tailnet', str(Path(__file__).with_name('horizon-worker-tailnet')))
spec = importlib.util.spec_from_loader(loader.name, loader)
worker = importlib.util.module_from_spec(spec)
loader.exec_module(worker)
KEY = 'tskey-auth-synthetic12345678901234567890'

class TailnetTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name)
        self.state, self.runtime = root / 'state', root / 'run'
        self.state.mkdir(); self.runtime.mkdir()
        self.calls = []
        self.joined = False
        for name, value in [('STATE', self.state), ('RUNTIME', self.runtime), ('PUBLIC', root / 'public')]:
            patcher = patch.object(worker, name, value); patcher.start(); self.addCleanup(patcher.stop)
        for name in ['private_directory', 'prepare_agents', 'start', 'wait_socket']:
            patcher = patch.object(worker, name); patcher.start(); self.addCleanup(patcher.stop)
        patcher = patch.object(worker, 'call', self.call); patcher.start(); self.addCleanup(patcher.stop)

    def call(self, *args, **kwargs):
        self.calls.append(args)
        self.assertNotIn(KEY, ' '.join(args))
        self.assertNotIn('TS_AUTHKEY', os.environ)
        if args[0] == 'status':
            return json.dumps({'BackendState': 'Running' if self.joined else 'NeedsLogin'}).encode()
        if args[0] == 'up':
            fd = kwargs['pass_fds'][0]
            self.assertEqual(os.pread(fd, 4096, 0).decode(), KEY)
            self.joined = True
        if args[0] == 'logout':
            self.joined = False
        return b''

    def configure(self, identity, key):
        request = json.dumps({'tailnet': identity, 'key': key}).encode()
        with patch.object(worker.sys, 'stdin', type('Input', (), {'buffer': io.BytesIO(request)})()), patch('sys.stdout', new_callable=io.StringIO) as output:
            worker.configure()
            result = output.getvalue()
        self.assertNotIn(KEY, result)
        return result

    def test_join_has_no_retained_auth_key_and_retries_do_not_reuse_one_time_key(self):
        self.assertEqual(self.configure('work', None), 'needs_key\n')
        self.assertEqual(self.configure('work', KEY), 'ready\n')
        self.assertEqual(self.configure('work', None), 'ready\n')
        self.assertEqual(sum(args[0] == 'up' for args in self.calls), 1)
        self.assertEqual((self.state / 'selection').read_text(), 'work')
        for item in self.state.iterdir(): self.assertNotIn(KEY, item.read_text())
        self.assertEqual(self.configure(None, None), 'ready\n')
        self.assertFalse((self.state / 'selection').exists())
        self.assertFalse(self.joined)

    def test_lost_join_reply_does_not_reuse_a_one_time_key(self):
        original = self.call
        def lost(*args, **kwargs):
            result = original(*args, **kwargs)
            if args[0] == 'up':
                raise TimeoutError('Synthetic lost reply')
            return result
        with patch.object(worker, 'call', lost):
            with self.assertRaises(TimeoutError): self.configure('work', KEY)
        self.assertEqual(self.configure('work', None), 'ready\n')
        self.assertEqual(sum(args[0] == 'up' for args in self.calls), 1)

    def test_invalid_secret_types_and_extra_fields_do_not_reach_daemon(self):
        for key in ['tskey-api-synthetic12345678901234567890', 'short', KEY + '\n', {'key': KEY}]:
            with self.assertRaises((ValueError, TypeError)): self.configure('work', key)
        self.assertEqual(self.calls, [])

    def test_prelogin_null_device_maps_do_not_kill_the_supervisor(self):
        report = {'BackendState': 'NeedsLogin', 'Self': None, 'Peer': None}
        with patch.object(worker, 'call', return_value=json.dumps(report).encode()):
            worker.publish_devices()
        self.assertEqual(json.loads((worker.PUBLIC / 'devices.json').read_text()), {'devices': []})

    def test_resume_waits_for_persistent_identity_before_requesting_a_key(self):
        (self.state / 'selection').write_text('work')
        reports = [b'{"BackendState":"Starting"}', b'{"BackendState":"Running"}']
        with patch.object(worker, 'call', side_effect=reports) as call, patch.object(worker.time, 'sleep'):
            self.assertEqual(self.configure('work', None), 'ready\n')
        self.assertEqual([args.args for args in call.call_args_list], [('status', '--json')] * 2)

    def test_resume_timeout_and_admission_states_never_request_or_reuse_a_key(self):
        (self.state / 'selection').write_text('work')
        for state in ['Starting', 'NoState', 'Stopped', 'NeedsMachineAuth', 'InUseOtherUser', 'Unknown']:
            for key in [None, KEY]:
                with self.subTest(state=state, key_supplied=key is not None):
                    with patch.object(worker, 'call', return_value=json.dumps({'BackendState': state}).encode()) as call, \
                            patch.object(worker.time, 'monotonic', side_effect=[0, 6]), \
                            patch('sys.stdout', new_callable=io.StringIO) as output:
                        with self.assertRaises(ValueError): self.configure('work', key)
                    self.assertEqual(output.getvalue(), '')
                    self.assertEqual([args.args for args in call.call_args_list], [('status', '--json')])
                    self.assertEqual((self.state / 'selection').read_text(), 'work')
        self.joined = True
        self.assertEqual(self.configure('work', None), 'ready\n')
        self.assertFalse(any(args[0] in {'up', 'logout'} for args in self.calls))

    def test_explicit_login_required_state_can_request_a_key_after_resume(self):
        (self.state / 'selection').write_text('work')
        self.assertEqual(self.configure('work', None), 'needs_key\n')
        self.assertEqual(self.configure('work', KEY), 'ready\n')

    def test_resume_never_needs_a_key_or_touches_the_host_tailscale(self):
        (self.state / 'selection').write_text('work')
        with patch.object(worker.sys, 'argv', ['worker', 'resume']): worker.main()
        worker.start.assert_called_once()
        self.assertTrue((self.runtime / 'agent-isolation').exists())
        self.assertEqual(self.calls, [])

    def test_daemon_uses_persistent_private_state_and_restarts(self):
        class Exit(Exception): pass
        child = type('Child', (), {'poll': lambda self: 0})()
        with patch.object(worker.subprocess, 'Popen', return_value=child) as spawn, patch.object(worker.time, 'sleep', side_effect=Exit), patch.object(worker.signal, 'signal'):
            with self.assertRaises(Exit): worker.serve()
        args = spawn.call_args.args[0]
        self.assertIn('--state=' + str(self.state / 'tailscaled.state'), args)
        self.assertIn('--socket=' + str(self.runtime / 'tailscaled.sock'), args)
        self.assertIn('--tun=userspace-networking', args)
        self.assertNotIn(KEY, str(spawn.call_args))

    def test_agent_drop_hands_off_siblings_and_only_nonsecret_stop_availability(self):
        workspace = Path(self.temp.name) / 'workspace'
        workspace.mkdir()
        manifest = workspace / 'siblings.json'
        manifest.write_text('{}'); manifest.chmod(0o600)
        def path(*parts):
            value = Path(*parts)
            if value.is_relative_to('/workspace'):
                return workspace / value.relative_to('/workspace')
            if value == Path('/run/horizon-credentials'):
                return Path(self.temp.name) / 'credentials'
            return value
        class Exec(Exception): pass
        with patch.object(worker, 'Path', side_effect=path) as paths, \
                patch.object(worker.os, 'geteuid', return_value=0), \
                patch.object(worker.os, 'chdir'), \
                patch.object(worker.subprocess, 'run') as run, \
                patch.object(worker.os, 'execve', side_effect=Exec) as execute, \
                patch.dict(os.environ, {'RUNPOD_API_KEY': 'synthetic-provider-secret',
                                      'RUNPOD_POD_ID': 'synthetic-pod', 'TS_AUTHKEY': KEY}, clear=True):
            paths.cwd.return_value = workspace
            with self.assertRaises(Exec): worker.agent(['/usr/bin/true'])
        self.assertTrue(any(call.args[0][-1] == str(manifest) for call in run.call_args_list))
        binary, args, environment = execute.call_args.args
        self.assertEqual(binary, '/usr/bin/setpriv')
        self.assertIn('--no-new-privs', args)
        self.assertEqual(environment['HORIZON_WORKER_SELF_STOP_AVAILABLE'], '1')
        self.assertNotIn('RUNPOD_API_KEY', environment)
        self.assertNotIn('TS_AUTHKEY', environment)

if __name__ == '__main__': unittest.main()
