"""Post-enrollment tag changes must close the owned network control lane."""
import json
from pathlib import Path
import threading
import unittest
from unittest.mock import patch

import test_tailnet as fixture

worker = fixture.worker


class RevocationTests(unittest.TestCase):
    setUp = fixture.TailnetTests.setUp
    call = fixture.TailnetTests.call
    configure = fixture.TailnetTests.configure

    def enrolled(self):
        self.assertEqual(self.configure('work', fixture.KEY), 'ready\n')
        worker.publish_devices()
        self.assertTrue(json.loads((worker.PUBLIC / 'devices.json').read_text())['devices'])

    def test_tag_removal_or_extra_tag_revokes_an_already_enrolled_worker(self):
        for tags in [[], [worker.WORKER_TAG, 'tag:admin']]:
            with self.subTest(tags=tags):
                self.tags = [worker.WORKER_TAG]
                self.enrolled()
                self.tags = tags
                worker.publish_devices()
                self.assertFalse(self.joined)
                self.assertFalse((self.state / 'selection').exists())
                self.assertFalse((self.state / 'revocation-pending').exists())
                self.assertEqual(json.loads((worker.PUBLIC / 'devices.json').read_text()), {'devices': []})

    def test_failed_logout_keeps_durable_marker_and_blocks_ready_and_agents(self):
        self.enrolled()
        self.tags = []
        original = self.call
        def refused(*args, **kwargs):
            if args[0] == 'logout':
                pending = self.state / 'revocation-pending'
                self.assertTrue(pending.exists(), 'Persist intent before logout')
                self.assertEqual(pending.stat().st_mode & 0o777, 0o600)
                self.assertEqual(json.loads((worker.PUBLIC / 'devices.json').read_text()), {'devices': []})
                raise worker.subprocess.CalledProcessError(1, args)
            return original(*args, **kwargs)
        with patch.object(worker, 'call', refused):
            with self.assertRaises(worker.RevocationPending): worker.publish_devices()
            self.assertEqual((self.state / 'selection').read_text(), 'work')
            self.assertEqual(json.loads((worker.PUBLIC / 'devices.json').read_text()), {'devices': []})
            # Even restoring the valid tag cannot bypass the pending revocation.
            self.tags = [worker.WORKER_TAG]
            for identity in ['work', None]:
                with self.assertRaises(worker.RevocationPending): self.configure(identity, fixture.KEY if identity else None)
            with self.assertRaises(worker.RevocationPending): worker.require_worker_tag()
        worker.publish_devices()
        self.assertFalse(self.joined)
        self.assertFalse((self.state / 'revocation-pending').exists())
        self.assertEqual(self.configure('work', None), 'needs_key\n')

    def test_logout_reply_without_needslogin_retains_selection_and_pending(self):
        self.enrolled()
        self.tags = []
        original = self.call
        def unconfirmed(*args, **kwargs):
            if args[0] == 'logout': return b''
            return original(*args, **kwargs)
        with patch.object(worker, 'call', unconfirmed):
            with self.assertRaises(worker.RevocationPending): worker.publish_devices()
        self.assertTrue(self.joined)
        self.assertTrue((self.state / 'revocation-pending').exists())
        self.assertEqual((self.state / 'selection').read_text(), 'work')

    def test_marker_write_failure_still_demands_owned_daemon_shutdown(self):
        self.enrolled()
        self.tags = []
        original = Path.open
        def fail_pending(path, *args, **kwargs):
            if path.name == 'revocation-pending': raise OSError('Synthetic full storage')
            return original(path, *args, **kwargs)
        with patch.object(Path, 'open', fail_pending):
            with self.assertRaises(worker.RevocationPending): worker.publish_devices()
        self.assertFalse((self.state / 'revocation-pending').exists())
        self.assertTrue(self.joined)
        self.assertEqual(json.loads((worker.PUBLIC / 'devices.json').read_text()), {'devices': []})

    def test_inventory_cannot_publish_stale_peers_after_concurrent_revocation(self):
        self.enrolled()
        entered, release, revoked = threading.Event(), threading.Event(), threading.Event()
        original = self.call
        def paused(*args, **kwargs):
            result = original(*args, **kwargs)
            if args[0] == 'status':
                entered.set()
                self.assertTrue(release.wait(3))
            return result
        def remove():
            self.configure(None, None)
            revoked.set()
        with patch.object(worker, 'call', paused):
            publisher = threading.Thread(target=worker.publish_devices)
            publisher.start(); self.assertTrue(entered.wait(3))
            control = threading.Thread(target=remove); control.start()
            self.assertFalse(revoked.wait(0.05), 'Revocation shares the inventory lock')
            release.set(); publisher.join(3); control.join(3)
        self.assertFalse(publisher.is_alive()); self.assertFalse(control.is_alive())
        self.assertTrue(revoked.is_set())
        self.assertEqual(json.loads((worker.PUBLIC / 'devices.json').read_text()), {'devices': []})

    def test_configure_waits_for_proxy_readiness_after_releasing_control_lock(self):
        def ready(allow_quarantine=False):
            if allow_quarantine: return
            published = threading.Event()
            def inspect():
                worker.publish_devices()
                published.set()
            supervisor = threading.Thread(target=inspect, daemon=True)
            supervisor.start()
            self.assertTrue(published.wait(2), 'Supervisor must acquire the control lock before Ready')
            supervisor.join(2)
        with patch.object(worker, 'wait_socket', side_effect=ready):
            self.assertEqual(self.configure('work', fixture.KEY), 'ready\n')

    def test_malformed_status_objects_are_refused(self):
        for report in [None, [], 'Running', {'BackendState': []},
                       {'BackendState': 'Running', 'Self': []},
                       {'BackendState': 'Running', 'Peer': []},
                       {'BackendState': 'Running', 'Peer': {'peer': None}}]:
            with self.subTest(report=report):
                with patch.object(worker, 'call', return_value=json.dumps(report).encode()):
                    with self.assertRaises(ValueError): worker.publish_devices()


class OwnedChild:
    def __init__(self):
        self.exited = False
        self.events = []
    def poll(self): return 0 if self.exited else None
    def terminate(self): self.events.append('terminate')
    def kill(self): self.events.append('kill')
    def wait(self, timeout):
        self.events.append(('wait', timeout))
        self.exited = True


class SupervisorTests(unittest.TestCase):
    setUp = fixture.TailnetTests.setUp
    call = fixture.TailnetTests.call
    configure = fixture.TailnetTests.configure

    def generations(self, reports):
        class Done(BaseException): pass
        children, commands = [], []
        def spawn(args, **kwargs):
            child = OwnedChild(); children.append(child); commands.append(args)
            return child
        def report():
            if not reports: raise Done()
            result = reports.pop(0)
            if isinstance(result, BaseException): raise result
            return result
        with patch.object(worker.subprocess, 'Popen', side_effect=spawn), \
                patch.object(worker, 'publish_devices', side_effect=report), \
                patch.object(worker.time, 'sleep'), patch.object(worker.signal, 'signal'):
            with self.assertRaises(Done): worker.serve()
        return commands, children

    def test_first_generation_has_no_proxy_until_persistent_identity_is_checked(self):
        commands, children = self.generations([True])
        self.assertEqual(len(commands), 2)
        self.assertFalse(any('proxy' in arg or 'socks5' in arg for arg in commands[0]))
        self.assertIn('--socks5-server=127.0.0.1:1055', commands[1])
        self.assertTrue(children[0].exited)
        self.assertTrue(children[-1].exited, 'Unexpected exit must close the last owned child')

    def test_failed_logout_stops_owned_proxy_and_restarts_without_listeners(self):
        commands, children = self.generations([True, worker.RevocationPending('Synthetic logout timeout')])
        self.assertIn('--socks5-server=127.0.0.1:1055', commands[1])
        self.assertTrue(children[1].exited)
        self.assertFalse(any('proxy' in arg or 'socks5' in arg for arg in commands[2]))
        self.assertFalse((self.runtime / 'proxy-ready').exists())

    def test_status_outage_removes_readiness_and_restarts_without_listeners(self):
        worker.write_devices([{'name': 'Former peer', 'addresses': [], 'online': True}])
        commands, children = self.generations([True, True, OSError('Synthetic status unavailable')])
        self.assertTrue(children[1].exited)
        self.assertFalse(any('proxy' in arg or 'socks5' in arg for arg in commands[2]))
        self.assertFalse((self.runtime / 'proxy-ready').exists())
        self.assertEqual(json.loads((worker.PUBLIC / 'devices.json').read_text()), {'devices': []})

    def test_unexpected_poll_error_closes_proxy_before_reconciliation(self):
        commands, children = self.generations([True, AttributeError('Synthetic malformed report')])
        self.assertTrue(children[1].exited)
        self.assertFalse(any('proxy' in arg or 'socks5' in arg for arg in commands[2]))
        self.assertTrue(children[-1].exited)

    def test_quarantine_inventory_or_readiness_errors_still_stop_owned_child(self):
        for failure in ['inventory', 'readiness']:
            with self.subTest(failure=failure):
                child = OwnedChild()
                ready = self.runtime / 'proxy-ready'
                ready.touch()
                if failure == 'inventory':
                    with patch.object(worker, 'write_devices', side_effect=OSError('Synthetic storage unavailable')):
                        self.assertTrue(worker.quarantine_owned_daemon(child, ready))
                else:
                    with patch.object(Path, 'unlink', side_effect=OSError('Synthetic readiness unavailable')):
                        self.assertTrue(worker.quarantine_owned_daemon(child, ready))
                    self.assertEqual(json.loads((worker.PUBLIC / 'devices.json').read_text()), {'devices': []})
                self.assertTrue(child.exited)
                self.assertEqual(child.events, ['terminate', ('wait', 5)])

    def test_failed_stop_retains_exact_owned_child_and_never_spawns_another(self):
        child = OwnedChild()
        child.wait = lambda timeout: (_ for _ in ()).throw(worker.subprocess.TimeoutExpired('synthetic', timeout))
        self.assertFalse(worker.stop_owned_daemon(child))
        self.assertEqual(child.events, ['terminate', 'kill'])
        self.assertFalse(child.exited)

    def test_supervisor_retries_failed_stop_without_replacing_owned_child(self):
        class Done(BaseException): pass
        first, current = OwnedChild(), OwnedChild()
        current.wait = lambda timeout: (_ for _ in ()).throw(worker.subprocess.TimeoutExpired('synthetic', timeout))
        results = [True, worker.RevocationPending(), worker.RevocationPending(), Done()]
        def report():
            result = results.pop(0)
            if isinstance(result, Done):
                current.wait = lambda timeout: setattr(current, 'exited', True)
            if isinstance(result, BaseException): raise result
            return result
        with patch.object(worker.subprocess, 'Popen', side_effect=[first, current]) as spawn, \
                patch.object(worker, 'publish_devices', side_effect=report), \
                patch.object(worker.time, 'sleep'), patch.object(worker.signal, 'signal'):
            with self.assertRaises(Done): worker.serve()
        self.assertEqual(spawn.call_count, 2)
        self.assertEqual(current.events, ['terminate', 'kill', 'terminate', 'kill', 'terminate'])
        self.assertTrue(current.exited)

    def test_stop_escalates_only_the_owned_child_after_terminate_timeout(self):
        child = OwnedChild()
        waits = []
        def wait(timeout):
            waits.append(timeout)
            if len(waits) == 1: raise worker.subprocess.TimeoutExpired('synthetic', timeout)
            child.exited = True
        child.wait = wait
        self.assertTrue(worker.stop_owned_daemon(child))
        self.assertEqual(child.events, ['terminate', 'kill'])
        self.assertEqual(waits, [5, 5])


if __name__ == '__main__': unittest.main()
