from concurrent.futures import ThreadPoolExecutor
import importlib.machinery
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch

loader = importlib.machinery.SourceFileLoader('tailnet', str(Path(__file__).with_name('horizon-worker-tailnet')))
spec = importlib.util.spec_from_loader(loader.name, loader)
worker = importlib.util.module_from_spec(spec)
loader.exec_module(worker)
KEY = 'tskey-auth-synthetic12345678901234567890'
CLOUD = '3f2b8c1e-9a4d-4e6f-8b7a-1c2d3e4f5a6b'
NAME = 'horizon-cloud-' + CLOUD

class TailnetTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name)
        self.state, self.runtime = root / 'state', root / 'run'
        self.state.mkdir(); self.runtime.mkdir()
        self.calls = []
        self.joined = False
        # The daemon's host name: the container's own until a preference names the device.
        self.hostname = 'd1021da2f9b5'
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
            return json.dumps({'BackendState': 'Running' if self.joined else 'NeedsLogin',
                               'Self': {'HostName': self.hostname}}).encode()
        if args[0] in {'up', 'set'}:
            for arg in args:
                if arg.startswith('--hostname='):
                    self.hostname = arg.removeprefix('--hostname=')
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

    def resume(self, cloud=CLOUD, container='02f08702ac0a'):
        # A resumed cloud gets a new container with a new host name, as a Hetzner resume does.
        self.hostname = self.hostname if self.hostname.startswith('horizon-') else container
        environment = {} if cloud is None else {'HORIZON_CLOUD_OPERATION': cloud}
        with patch.object(worker.sys, 'argv', ['worker', 'resume']), patch.dict(os.environ, environment, clear=True):
            worker.main()

    def test_device_name_is_a_deterministic_dns_label_from_the_cloud_id(self):
        self.assertEqual(worker.device_name(CLOUD), 'horizon-cloud-' + CLOUD)
        self.assertEqual(worker.device_name(CLOUD), worker.device_name(CLOUD))
        names = {}
        for cloud in [CLOUD, 'Cloud_A', 'cloud-a', 'cloud_a', 'CLOUD-A', 'a' * 100, 'a' * 99 + 'b', 'trailing-', 'x_']:
            with self.subTest(cloud=cloud):
                name = worker.device_name(cloud)
                self.assertRegex(name, r'^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$')
                self.assertLessEqual(len(name), 63)
                self.assertEqual(name, worker.device_name(cloud))
                self.assertNotIn(name, names.values())
                names[cloud] = name
        self.assertEqual(names['cloud-a'], 'horizon-cloud-cloud-a')
        for cloud in [None, '', 'a b', 'a.b', 'cloud/a', 'a' * 101, 'ä', 'a\n', 7]:
            with self.subTest(cloud=cloud):
                self.assertIsNone(worker.device_name(cloud))

    def test_first_enrollment_names_the_device_after_the_cloud(self):
        self.resume()
        self.assertEqual((self.runtime / 'device-name').read_text(), NAME)
        self.assertEqual(self.configure('work', None), 'needs_key\n')
        self.assertEqual(self.configure('work', KEY), 'ready\n')
        [up] = [args for args in self.calls if args[0] == 'up']
        self.assertIn('--hostname=' + NAME, up)
        self.assertEqual(self.hostname, NAME)
        self.assertFalse(any(args[0] == 'set' for args in self.calls))

    def test_resume_keeps_the_name_on_a_new_container_without_a_key(self):
        self.resume()
        self.configure('work', None); self.configure('work', KEY)
        for container in ['02f08702ac0a', '5a1c4e9f7b20']:
            with self.subTest(container=container):
                self.calls.clear()
                (self.runtime / 'device-name').unlink()
                self.resume(container=container)
                self.assertEqual(self.configure('work', None), 'ready\n')
                self.assertEqual(self.hostname, NAME)
                self.assertEqual(self.calls, [('status', '--json'), ('status', '--json')])

    def test_resume_renames_a_cloud_enrolled_with_the_container_host_name(self):
        self.configure('work', None); self.configure('work', KEY)
        self.assertEqual(self.hostname, 'd1021da2f9b5')
        self.calls.clear()
        self.resume()
        self.assertEqual(self.configure('work', None), 'ready\n')
        self.assertEqual(self.hostname, NAME)
        self.assertIn(('set', '--hostname=' + NAME), self.calls)
        self.assertFalse(any(args[0] in {'up', 'logout'} for args in self.calls))
        self.calls.clear()
        self.assertEqual(self.configure('work', None), 'ready\n')
        self.assertFalse(any(args[0] == 'set' for args in self.calls))

    def test_a_failed_rename_keeps_a_joined_cloud_usable(self):
        self.configure('work', None); self.configure('work', KEY)
        self.resume()
        original = self.call
        def refused(*args, **kwargs):
            if args[0] == 'set':
                raise worker.subprocess.CalledProcessError(1, args)
            return original(*args, **kwargs)
        with patch.object(worker, 'call', refused):
            self.assertEqual(self.configure('work', None), 'ready\n')
        self.assertEqual(self.hostname, '02f08702ac0a')
        self.assertEqual(self.configure('work', None), 'ready\n')
        self.assertEqual(self.hostname, NAME)

    def test_reenrollment_after_resume_also_names_the_device(self):
        (self.state / 'selection').write_text('work')
        self.resume()
        self.assertEqual(self.configure('work', None), 'needs_key\n')
        self.assertEqual(self.configure('work', KEY), 'ready\n')
        [up] = [args for args in self.calls if args[0] == 'up']
        self.assertIn('--hostname=' + NAME, up)
        self.calls.clear()
        self.assertEqual(self.configure('other', None), 'needs_key\n')
        self.assertEqual(self.configure('other', KEY), 'ready\n')
        [up] = [args for args in self.calls if args[0] == 'up']
        self.assertIn('--hostname=' + NAME, up)

    def test_a_worker_without_a_cloud_id_keeps_the_daemon_host_name(self):
        (self.runtime / 'device-name').write_text('stale-name')
        self.resume(cloud=None)
        self.assertFalse((self.runtime / 'device-name').exists())
        self.configure('work', None); self.configure('work', KEY)
        [up] = [args for args in self.calls if args[0] == 'up']
        self.assertFalse(any(arg.startswith('--hostname') for arg in up))
        self.resume(cloud='invalid cloud id')
        self.assertFalse((self.runtime / 'device-name').exists())

    def test_malformed_recorded_name_never_reaches_the_daemon(self):
        for name in ['', 'UPPER', '-leading', 'trailing-', 'a' * 64, 'dot.name', 'name\n']:
            with self.subTest(name=name):
                self.calls.clear()
                (self.runtime / 'device-name').write_text(name)
                with self.assertRaises(ValueError): self.configure('work', KEY)
                self.assertFalse(any(args[0] in {'up', 'set'} for args in self.calls))

    def test_helper_declares_the_stable_name_contract(self):
        with patch.object(worker.sys, 'argv', ['worker', '--stable-name-contract']), \
                patch('sys.stdout', new_callable=io.StringIO) as output:
            worker.main()
        self.assertEqual(output.getvalue(), 'horizon-tailnet-contract=2\n')
        self.assertEqual(self.calls, [])

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

    def test_concurrent_inventory_publishers_use_distinct_atomic_files(self):
        barrier = threading.Barrier(2)
        replace = os.replace
        pending = []
        def publish(source, destination):
            pending.append(source)
            barrier.wait(timeout=5)
            replace(source, destination)
        report = json.dumps({'Self': {'HostName': 'Synthetic device'}}).encode()
        with patch.object(worker, 'call', return_value=report), patch.object(worker.os, 'replace', side_effect=publish):
            with ThreadPoolExecutor(max_workers=2) as pool:
                tasks = [pool.submit(worker.publish_devices) for _ in range(2)]
                for task in tasks: task.result(timeout=10)
        self.assertEqual(len(set(pending)), 2)
        self.assertEqual(json.loads((worker.PUBLIC / 'devices.json').read_text())['devices'][0]['name'], 'Synthetic device')
        self.assertEqual(list(worker.PUBLIC.glob('devices-*')), [])

    def test_inventory_write_failure_keeps_prior_snapshot_and_removes_pending_file(self):
        worker.PUBLIC.mkdir()
        snapshot = worker.PUBLIC / 'devices.json'
        snapshot.write_text('{"devices":[]}')
        with patch.object(worker.json, 'dump', side_effect=OSError('Synthetic full disk')):
            with self.assertRaises(OSError): worker.publish_devices()
        self.assertEqual(snapshot.read_text(), '{"devices":[]}')
        self.assertEqual(list(worker.PUBLIC.glob('devices-*')), [])

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

    def test_status_reports_joined_only_for_running_selected_identity(self):
        for selected, state, expected in [(False, 'Running', 'none'), (True, 'Running', 'joined'),
                                           (True, 'NeedsLogin', 'none')]:
            with self.subTest(selected=selected, state=state):
                selection = self.state / 'selection'
                if selected: selection.write_text('work')
                else: selection.unlink(missing_ok=True)
                with patch.object(worker.sys, 'argv', ['worker', 'status']), \
                        patch.object(worker, 'call', return_value=json.dumps({'BackendState': state}).encode()), \
                        patch('sys.stdout', new_callable=io.StringIO) as output:
                    worker.main()
                self.assertEqual(output.getvalue(), expected + '\n')

    def test_resume_never_needs_a_key_or_touches_the_host_tailscale(self):
        (self.state / 'selection').write_text('work')
        self.resume()
        worker.start.assert_called_once()
        self.assertTrue((self.runtime / 'agent-isolation').exists())
        self.assertEqual((self.runtime / 'device-name').stat().st_mode & 0o777, 0o600)
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

    def test_failed_ownership_migration_retries_before_publishing_ready(self):
        with patch.object(worker, 'handoff_paths', side_effect=OSError):
            with self.assertRaises(OSError): worker.handoff()
        self.assertFalse((self.state / 'ownership-v1').exists())
        with patch.object(worker, 'handoff_paths') as migrate:
            worker.handoff()
            (self.runtime / 'handoff.lock').unlink()
            self.runtime.rmdir(); self.runtime.mkdir()
            worker.handoff()
        self.assertEqual([call.args for call in migrate.call_args_list], [(True,), (False,)])

    def test_nonconsumer_launch_leaves_a_partial_upload_untouched(self):
        workspace = Path(self.temp.name) / 'workspace'; workspace.mkdir()
        pack = workspace / 'horizon-transfer.pack'; pack.write_text('partial upload')
        def path(*parts):
            value = Path(*parts)
            return workspace / value.relative_to('/workspace') if value.is_relative_to('/workspace') else value
        with patch.object(worker, 'Path', side_effect=path), patch.object(worker.subprocess, 'run') as run:
            worker.handoff_uploads(['/usr/bin/true'])
        run.assert_not_called()
        self.assertEqual(pack.read_text(), 'partial upload')

    def test_upload_source_symlinks_are_refused_before_any_handoff(self):
        workspace = Path(self.temp.name) / 'workspace'; workspace.mkdir()
        protected = workspace / 'protected'; protected.write_text('unchanged')
        (workspace / 'horizon-transfer.pack').symlink_to(protected)
        def path(*parts):
            value = Path(*parts)
            return workspace / value.relative_to('/workspace') if value.is_relative_to('/workspace') else value
        with patch.object(worker, 'Path', side_effect=path), patch.object(worker.subprocess, 'run') as run:
            with self.assertRaises(OSError): worker.handoff_uploads(['horizon-worker-import', 'revision'])
        run.assert_not_called()
        self.assertEqual(protected.read_text(), 'unchanged')

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
                patch.object(worker, 'handoff_uploads') as handoff, \
                patch.object(worker.subprocess, 'run') as run, \
                patch.object(worker.os, 'execve', side_effect=Exec) as execute, \
                patch.dict(os.environ, {'RUNPOD_API_KEY': 'synthetic-provider-secret',
                                      'RUNPOD_POD_ID': 'synthetic-pod', 'TS_AUTHKEY': KEY}, clear=True):
            paths.cwd.return_value = workspace
            with self.assertRaises(Exec): worker.agent(['/usr/bin/true'])
            self.assertTrue(any('-R' in call.args[0] for call in run.call_args_list))
            self.assertTrue(any(call.args[0][-1] == str(manifest) for call in run.call_args_list))
            run.reset_mock()
            (workspace / 'horizon-transfer.pack').write_text('new root upload')
            with self.assertRaises(Exec): worker.agent(['/usr/bin/true'])
            self.assertFalse(any('-R' in call.args[0] for call in run.call_args_list))
            self.assertEqual((workspace / 'horizon-transfer.pack').read_text(), 'new root upload')
            self.assertEqual(handoff.call_count, 2)
        binary, args, environment = execute.call_args.args
        self.assertEqual(binary, '/usr/bin/setpriv')
        self.assertIn('--no-new-privs', args)
        self.assertEqual(environment['HORIZON_WORKER_SELF_STOP_AVAILABLE'], '1')
        self.assertNotIn('RUNPOD_API_KEY', environment)
        self.assertNotIn('TS_AUTHKEY', environment)

if __name__ == '__main__': unittest.main()


class SupervisorRecordTests(unittest.TestCase):
    def test_malformed_stale_record_is_replaced_but_live_record_is_preserved(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(worker, 'RUNTIME', Path(temp)), patch.object(worker, 'private_directory'), patch.object(worker.subprocess, 'Popen') as spawn:
            spawn.return_value.pid = os.getpid()
            started = Path('/proc', str(os.getpid()), 'stat').read_text().rsplit(') ', 1)[1].split()[19]
            record = Path(temp) / 'supervisor.pid'
            for value in ['', '123', '123 456 extra', 'not-a-pid stamp', '999999999 0']:
                record.write_text(value); worker.start()
                self.assertEqual(record.read_text(), f'{os.getpid()} {started}')
            self.assertEqual(spawn.call_count, 5)
            worker.start(); self.assertEqual(spawn.call_count, 5)
