"""Required services remain owned and healthy across every bootstrap phase."""
import json
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock


MODULE = runpy.run_path(str(Path(__file__).with_name('horizon-worker-supervise')))
Supervisor = MODULE['Supervisor']
CHECK = MODULE['check_ready']
DESKTOP_READY = MODULE['desktop_ready']


class SupervisionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.supervisor = Supervisor(self.root, self.root)
        self.addCleanup(self.supervisor.close)

    def start(self, name, code='import time; time.sleep(60)'):
        return self.supervisor.start(name, [sys.executable, '-c', code])

    def capabilities(self, desktop):
        (self.root / 'capabilities.json').write_text(json.dumps({'desktop': desktop}))

    def ready(self, desktop):
        self.capabilities(desktop)
        for name in ['control', 'sshd'] + (['xvfb', 'openbox', 'vnc'] if desktop else []):
            self.start(name)
        self.supervisor.publish(desktop)

    def test_early_desktop_exit_cannot_be_hidden_by_successful_configuration(self):
        for service in ['xvfb', 'openbox', 'vnc']:
            with self.subTest(service=service):
                child = self.start(service)
                child.terminate()
                child.wait()
                with self.assertRaisesRegex(ValueError, service):
                    self.supervisor.configure([sys.executable, '-c', 'pass'])
                self.supervisor.close()
                self.supervisor.children.clear()
                self.assertFalse((self.root / 'services.json').exists())

    def test_service_exit_during_configuration_aborts_without_waiting_for_completion(self):
        for service in ['xvfb', 'openbox', 'vnc']:
            with self.subTest(service=service):
                child = self.start(service)
                started = self.root / ('configure-' + service)
                command = [sys.executable, '-c',
                           'from pathlib import Path; import time; Path(' + repr(str(started)) + ').touch(); time.sleep(60)']
                errors = []
                def configure():
                    try:
                        self.supervisor.configure(command)
                    except ValueError as error:
                        errors.append(str(error))
                thread = threading.Thread(target=configure)
                thread.start()
                deadline = time.monotonic() + 3
                while not started.exists() and time.monotonic() < deadline:
                    time.sleep(.01)
                self.assertTrue(started.exists())
                child.terminate()
                thread.join(timeout=3)
                self.assertFalse(thread.is_alive())
                self.assertTrue(errors and service in errors[0], errors)
                self.supervisor.close()
                self.supervisor.children.clear()

    def test_configuration_completion_is_not_a_required_service_exit(self):
        self.supervisor.configure([sys.executable, '-c', 'pass'])
        self.ready(False)
        CHECK(self.root, self.root)
        self.supervisor.assert_running()

    def test_completed_configuration_retires_its_group_before_pid_can_be_reused(self):
        child_pid = self.root / 'configuration-descendant'
        command = [sys.executable, '-c', 'import subprocess; from pathlib import Path; '
                   'child=subprocess.Popen(["sleep","60"]); '
                   'Path(' + repr(str(child_pid)) + ').write_text(str(child.pid))']
        self.supervisor.configure(command)
        self.assertNotIn('configure', self.supervisor.children)
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            try:
                MODULE['process_identity'](int(child_pid.read_text()))
            except (OSError, ValueError):
                break
            time.sleep(.01)
        else:
            self.fail('Configuration left an unowned background process')

    def test_failed_or_cancelled_configuration_never_publishes_readiness(self):
        with self.assertRaisesRegex(ValueError, 'configuration failed'):
            self.supervisor.configure([sys.executable, '-c', 'raise SystemExit(2)'])
        self.supervisor.stopping = True
        with self.assertRaisesRegex(ValueError, 'shutdown requested'):
            self.supervisor.wait_until(lambda: True, 'unused')
        self.assertFalse((self.root / 'services.json').exists())

    def test_bootstrap_timeout_checks_real_readiness_without_replacing_services(self):
        child = self.start('xvfb')
        with self.assertRaisesRegex(ValueError, 'display timeout'):
            self.supervisor.wait_until(lambda: False, 'display timeout', timeout=.02)
        self.assertEqual(self.supervisor.children['xvfb'].pid, child.pid)
        self.assertIsNone(child.poll())

    def test_desktop_health_requires_all_owned_processes_even_with_listening_transports(self):
        self.ready(True)
        with mock.patch.dict(CHECK.__globals__, desktop_ready=lambda pid: True):
            CHECK(self.root, self.root)
            self.supervisor.children['openbox'].terminate()
            self.supervisor.children['openbox'].wait()
            self.assertIsNone(self.supervisor.children['vnc'].poll())
            self.assertIsNone(self.supervisor.children['control'].poll())
            with self.assertRaises((ValueError, OSError)):
                CHECK(self.root, self.root)

    def test_every_required_service_failure_after_configuration_is_unhealthy(self):
        for service in ['xvfb', 'openbox', 'vnc', 'control', 'sshd']:
            with self.subTest(service=service):
                self.ready(True)
                child = self.supervisor.children[service]
                child.terminate()
                child.wait()
                with self.assertRaisesRegex(ValueError, service):
                    self.supervisor.assert_running()
                with self.assertRaises((OSError, ValueError)):
                    CHECK(self.root, self.root)
                self.supervisor.close()
                self.supervisor.children.clear()

    def test_minimal_readiness_has_no_desktop_requirement(self):
        self.ready(False)
        with mock.patch.dict(CHECK.__globals__, desktop_ready=mock.Mock(side_effect=AssertionError)):
            CHECK(self.root, self.root)
        self.capabilities(True)
        with self.assertRaisesRegex(ValueError, 'incomplete'):
            CHECK(self.root, self.root)

    def test_stale_reused_pid_and_incomplete_ownership_are_rejected(self):
        self.ready(False)
        path = self.root / 'services.json'
        state = json.loads(path.read_text())
        state['services']['control']['start'] = '0'
        path.write_text(json.dumps(state))
        with self.assertRaisesRegex(ValueError, 'identity changed'):
            CHECK(self.root, self.root)
        del state['services']['sshd']
        path.write_text(json.dumps(state))
        with self.assertRaisesRegex(ValueError, 'incomplete'):
            CHECK(self.root, self.root)

    def test_window_manager_must_own_its_live_root_registration(self):
        good = '_NET_SUPPORTING_WM_CHECK(WINDOW): window id # 0x20020b'
        for pid, properties, expected in [(42, good, True), (99, good, False),
                                          (42, good.replace('0x20020b', '0xdead'), False), (42, '', False)]:
            root = good + '\n_OPENBOX_PID(CARDINAL) = ' + str(pid)
            with mock.patch.dict(DESKTOP_READY.__globals__, probe=mock.Mock(side_effect=['display', root, properties])):
                self.assertEqual(DESKTOP_READY(42), expected)
        with mock.patch.dict(DESKTOP_READY.__globals__, probe=mock.Mock(side_effect=subprocess.TimeoutExpired('xprop', 2))):
            self.assertFalse(DESKTOP_READY(42))

    def test_health_probes_use_the_worker_display_without_inheriting_caller_display(self):
        replies = ['display', '_NET_SUPPORTING_WM_CHECK(WINDOW): window id # 0x20\n_OPENBOX_PID(CARDINAL) = 42',
                   '_NET_SUPPORTING_WM_CHECK(WINDOW): window id # 0x20']
        calls = mock.Mock(side_effect=replies)
        with mock.patch.dict(DESKTOP_READY.__globals__, probe=calls):
            self.assertTrue(DESKTOP_READY(42))
        for call in calls.call_args_list:
            self.assertEqual(call.args[0][1:3], ['-display', ':99'])

    def test_shutdown_invalidates_readiness_and_stops_owned_descendants(self):
        child_pid = self.root / 'descendant'
        parent = self.start('control', 'import subprocess,time; from pathlib import Path; '
                            'child=subprocess.Popen(["sleep","60"]); '
                            'Path(' + repr(str(child_pid)) + ').write_text(str(child.pid)); time.sleep(60)')
        deadline = time.monotonic() + 3
        while not child_pid.exists() and time.monotonic() < deadline:
            time.sleep(.01)
        self.assertTrue(child_pid.exists())
        descendant = int(child_pid.read_text())
        self.start('sshd')
        self.supervisor.publish(False)
        parent.terminate()
        parent.wait()
        owned = list(self.supervisor.children.values())
        self.supervisor.close()
        self.assertFalse((self.root / 'services.json').exists())
        self.assertTrue(all(child.poll() is not None for child in owned))
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            try:
                MODULE['process_identity'](descendant)
            except (OSError, ValueError):
                break
            time.sleep(.01)
        else:
            self.fail('Owned service descendant survived shutdown')


if __name__ == '__main__':
    unittest.main()
