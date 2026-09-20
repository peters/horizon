"""Remote credentials stay runtime-only and capability requirements are explicit."""
import base64
import copy
import importlib.machinery
import importlib.util
from pathlib import Path
import unittest
import json
import io
import tempfile
import subprocess
from types import SimpleNamespace
from unittest.mock import patch, MagicMock

loader = importlib.machinery.SourceFileLoader('worker_remote', str(Path(__file__).with_name('horizon-worker-browserstack')))
spec = importlib.util.spec_from_loader(loader.name, loader)
worker = importlib.util.module_from_spec(spec)
loader.exec_module(worker)

class RemoteTests(unittest.TestCase):
    def fixture(self):
        target = {'provider':'account','capability_extensions':{'bstack:options':{'local':True,'localIdentifier':'horizon-test'}}}
        return {'version':1,'remote':{'providers':{'account':{'adapter':'browserstack'}},'targets':{'iphone':target}},
                'authorization':{'account':'Basic '+base64.b64encode(b'user:synthetic-key').decode()},
                'quota_keys':{'account':'private-slot'},'local_identifier':'horizon-test','local_ports':[8080]}

    def test_declaration_and_transferred_target_must_match(self):
        value = self.fixture()
        selected = {'provider':'account','targets':['iphone'],'local_ports':[8080]}
        worker.validate(value, selected)
        for changed in [None, {'provider':'account','targets':['android'],'local_ports':[8080]}, {'provider':'account','targets':['iphone'],'local_ports':[22]}]:
            with self.assertRaises(ValueError): worker.validate(value, changed)
        value['remote']['providers']['account']['credential_bindings'] = {'key':{'store':'environment'}}
        with self.assertRaises(ValueError): worker.validate(value, selected)

    def test_account_grant_allows_other_devices_without_target_allowlisting(self):
        value = self.fixture()
        value['remote']['targets']['tablet'] = copy.deepcopy(value['remote']['targets']['iphone'])
        worker.validate(value, {'provider':'account','local_ports':[8080]})
        worker.validate(value, {'provider':'account','targets':['iphone'],'local_ports':[8080]})
        with self.assertRaises(ValueError):
            worker.validate(value, {'provider':'different-account','local_ports':[8080]})

    def test_catalog_account_needs_no_preconfigured_devices(self):
        value = self.fixture()
        value['remote']['targets'] = {}
        worker.validate(value, {'provider':'account', 'local_ports':[8080]})
        del value['remote']['targets']
        worker.validate(value, {'provider':'account', 'local_ports':[8080]})
        with self.assertRaises(ValueError):
            worker.validate(value, {'provider':'account', 'targets':['phone'], 'local_ports':[8080]})
        value['remote']['targets'] = None
        with self.assertRaises(ValueError):
            worker.validate(value, {'provider':'account', 'local_ports':[8080]})

    def test_tunnel_is_scoped_to_explicit_loopback_ports_and_has_no_force_flag(self):
        value = worker.tunnel_configuration(self.fixture())
        self.assertEqual(value['key'], 'synthetic-key')
        self.assertEqual(value['local-identifier'], 'horizon-test')
        self.assertTrue(value['only-automate'])
        self.assertTrue(value['disable-dashboard'])
        self.assertNotIn('force', value)
        self.assertNotIn('force-local', value)
        self.assertNotIn(',22,', value['only'])
        self.assertIn('127.0.0.1,8080,0', value['only'])
        self.assertIn('localhost,8080,1', value['only'])
        self.assertNotIn('log-file', value)

    def test_mismatched_tunnel_and_malformed_auth_are_rejected(self):
        value = self.fixture()
        bad = copy.deepcopy(value)
        bad['remote']['targets']['iphone']['capability_extensions']['bstack:options']['localIdentifier'] = 'other'
        with self.assertRaises(ValueError): worker.validate(bad, {'provider':'account','targets':['iphone'],'local_ports':[8080]})
        for header in ['Bearer secret', 'Basic !!!', 'Basic '+base64.b64encode(b'user:bad\nkey').decode()]:
            value['authorization']['account'] = header
            with self.assertRaises(ValueError): worker.tunnel_configuration(value)

    def test_failed_device_release_retains_credentials_and_tunnel(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ['browserstack.json', 'browserstack-local.yml']:
                (root / name).write_text('private-runtime')
            connection = MagicMock()
            connection.__enter__.return_value = connection
            connection.makefile.return_value = io.BytesIO(json.dumps({'error':'release uncertain'}).encode())
            with patch.object(worker, 'ROOT', root), patch.object(worker.socket, 'create_connection', return_value=connection), patch.object(worker, 'alive') as alive:
                with self.assertRaises(ValueError): worker.revoke()
                alive.assert_not_called()
            self.assertEqual((root / 'browserstack.json').read_text(), 'private-runtime')
            self.assertTrue((root / 'browserstack-local.yml').exists())

    def test_confirmed_release_removes_private_copies(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ['browserstack.json', 'browserstack-local.yml', 'browserstack-ready']:
                (root / name).write_text('private-runtime')
            connection = MagicMock()
            connection.__enter__.return_value = connection
            connection.makefile.return_value = io.BytesIO(b'{"closed":["phone"]}\n')
            with patch.object(worker, 'ROOT', root), patch.object(worker.socket, 'create_connection', return_value=connection), patch.object(worker, 'alive', return_value=False), patch.object(worker, 'STARTED', root / 'browserstack-started'):
                worker.revoke()
            self.assertEqual(list(root.iterdir()), [])
            connection.sendall.assert_called_once_with(b'{"operation":"revoke_remote"}\n')

    def test_lost_supervisor_cannot_clear_credentials_while_child_lives(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            child = subprocess.Popen(['sleep', '30'])
            try:
                (root / 'browserstack.json').write_text('private-runtime')
                (root / 'started').write_text(json.dumps({'generation':'new','pid':child.pid}))
                (root / 'stopped').write_text(json.dumps({'generation':'previous','stopped':True}))
                connection = MagicMock()
                connection.__enter__.return_value = connection
                connection.makefile.return_value = io.BytesIO(b'{"closed":[]}\n')
                with patch.object(worker, 'ROOT', root), patch.object(worker, 'STARTED', root / 'started'), patch.object(worker, 'STOPPED', root / 'stopped'), patch.object(worker.socket, 'create_connection', return_value=connection), patch.object(worker, 'alive', return_value=False):
                    with self.assertRaises(ValueError): worker.revoke()
                self.assertIsNone(child.poll())
                self.assertTrue((root / 'browserstack.json').exists())
            finally:
                child.terminate()
                child.wait(timeout=5)

    def test_new_container_retires_only_a_valid_previous_incarnation(self):
        current = worker.container_incarnation()
        self.assertTrue(worker.valid_incarnation(current))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.object(worker, 'STARTED', root / 'started'), patch.object(worker, 'STOPPED', root / 'stopped'), patch.object(worker, 'container_incarnation', return_value=current):
                for pid in [None, 333]:
                    for key, changed in [('init_start_ticks', current['init_start_ticks'] + 1),
                                         ('pid_namespace', current['pid_namespace'] + 1),
                                         ('boot_id', '00000000-0000-0000-0000-000000000000')]:
                        previous = dict(current, **{key: changed})
                        (root / 'started').write_text(json.dumps({'generation':'old', 'pid':pid, 'container_incarnation':previous}))
                        self.assertTrue(worker.shutdown_confirmed(), (pid, key))
                for previous in [current, None, {}, dict(current, version=True), dict(current, init_start_ticks='1')]:
                    (root / 'started').write_text(json.dumps({'generation':'old', 'pid':333, 'container_incarnation':previous}))
                    (root / 'stopped').write_text(json.dumps({'generation':'unrelated'}))
                    self.assertFalse(worker.shutdown_confirmed())
                (root / 'stopped').write_text(json.dumps({'generation':'old'}))
                self.assertTrue(worker.shutdown_confirmed())

    def test_uncertain_old_tunnel_preserves_original_account_binding(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            original = json.dumps(self.fixture())
            config = root / 'browserstack.json'
            config.write_text(original)
            (root / 'browserstack-local.yml').write_text('original private tunnel binding')
            (root / 'capabilities.json').write_text(json.dumps({'browserstack':{'provider':'account','local_ports':[8080]}}))
            replacement = self.fixture()
            replacement['authorization']['account'] = 'Basic '+base64.b64encode(b'user:replacement-key').decode()
            resolve = lambda value: root / 'capabilities.json' if value == '/workspace/capabilities.json' else Path(value)
            for confirmed in [False, True]:
                stdin = SimpleNamespace(buffer=io.BytesIO(json.dumps(replacement).encode()))
                with patch.object(worker, 'ROOT', root), patch.object(worker, 'CONFIG', config), patch.object(worker, 'Path', side_effect=resolve), patch.object(worker.sys, 'stdin', stdin), patch.object(worker, 'alive', return_value=False), patch.object(worker, 'shutdown_confirmed', return_value=confirmed):
                    with self.assertRaisesRegex(ValueError, 'Revoke existing remote browsers'):
                        worker.install()
            self.assertEqual(config.read_text(), original)
            self.assertEqual((root / 'browserstack-local.yml').read_text(), 'original private tunnel binding')

    def test_same_binding_does_not_overwrite_an_unconfirmed_tunnel(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            original = json.dumps(self.fixture())
            config = root / 'browserstack.json'
            config.write_text(original)
            tunnel = root / 'browserstack-local.yml'
            tunnel.write_text('original private tunnel binding')
            (root / 'capabilities.json').write_text(json.dumps({'browserstack':{'provider':'account','local_ports':[8080]}}))
            resolve = lambda value: root / 'capabilities.json' if value == '/workspace/capabilities.json' else Path(value)
            stdin = SimpleNamespace(buffer=io.BytesIO(original.encode()))
            with patch.object(worker, 'ROOT', root), patch.object(worker, 'CONFIG', config), patch.object(worker, 'Path', side_effect=resolve), patch.object(worker.sys, 'stdin', stdin), patch.object(worker, 'alive', return_value=False), patch.object(worker, 'shutdown_confirmed', return_value=False), patch.object(worker, 'private_write') as write:
                with self.assertRaisesRegex(ValueError, 'shutdown is unconfirmed'):
                    worker.install()
                write.assert_not_called()
            self.assertEqual(config.read_text(), original)
            self.assertEqual(tunnel.read_text(), 'original private tunnel binding')

    def test_supervision_observes_foreground_readiness_and_proves_child_exit(self):
        import os
        import time
        import signal
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / 'BrowserStackLocal'
            binary.write_text("#!/usr/bin/python3\nimport time\nprint('You can now access your local server(s)', flush=True)\ntime.sleep(30)\n")
            binary.chmod(0o700)
            script = root / 'supervisor'
            script.write_text(Path(worker.__file__).read_text().replace('/run/horizon-credentials', str(root)))
            (root / 'browserstack-local.yml').write_text('{}')
            process = subprocess.Popen(['python3', str(script), 'supervise'], env=dict(os.environ, PATH=str(root)+':'+os.environ['PATH']), stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            try:
                deadline = time.monotonic() + 5
                while not (root / 'browserstack-ready').exists() and time.monotonic() < deadline: time.sleep(.02)
                self.assertTrue((root / 'browserstack-ready').exists())
                self.assertEqual(json.loads((root/'browserstack-started').read_text())['container_incarnation'], worker.container_incarnation())
                process.send_signal(signal.SIGHUP)
                stdout, stderr = process.communicate(timeout=5)
                self.assertEqual(stdout, b'')
                self.assertEqual(stderr, b'')
                self.assertFalse((root / 'browserstack-ready').exists())
                self.assertEqual(json.loads((root/'browserstack-started').read_text())['generation'], json.loads((root/'browserstack-stopped').read_text())['generation'])
            finally:
                if process.poll() is None: process.terminate(); process.wait(timeout=5)

if __name__ == '__main__': unittest.main()
