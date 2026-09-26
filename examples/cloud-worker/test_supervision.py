"""Required services remain owned and healthy across every bootstrap phase."""
import json
import os
import resource
import signal
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import threading
import time
from types import SimpleNamespace
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

    def stop_unreaped(self, child):
        os.kill(child.pid, signal.SIGTERM)
        deadline = time.monotonic() + 3
        while MODULE['exited'](child) is None and time.monotonic() < deadline:
            time.sleep(.01)
        self.assertIsNotNone(MODULE['exited'](child))

    def capabilities(self, desktop):
        (self.root / 'capabilities.json').write_text(json.dumps({'desktop': desktop}))

    def ready(self, desktop):
        self.capabilities(desktop)
        for name in ['control', 'sshd'] + (['xvfb', 'openbox', 'vnc'] if desktop else []):
            self.start(name)
        self.supervisor.publish(desktop)

    def test_descriptor_cap_is_limited_to_vnc_child(self):
        inherited = resource.getrlimit(resource.RLIMIT_NOFILE)
        for name in ('vnc', 'control', 'sshd'):
            output = self.root / (name + '-limits.json')
            self.start(name, 'import json,resource,time; from pathlib import Path; Path(' + repr(str(output))
                       + ').write_text(json.dumps(resource.getrlimit(resource.RLIMIT_NOFILE))); time.sleep(60)')
            deadline = time.monotonic() + 3
            while not output.exists() and time.monotonic() < deadline:
                time.sleep(.01)
            limits = tuple(json.loads(output.read_text()))
            soft = inherited[0]
            expected = (4096 if soft == resource.RLIM_INFINITY else min(soft, 4096)) if name == 'vnc' else soft
            self.assertEqual(limits, (expected, inherited[1]))
        self.assertEqual(resource.getrlimit(resource.RLIMIT_NOFILE), inherited)

    def test_vnc_cap_preserves_lower_soft_and_hard_limits(self):
        limit = MODULE['limit_vnc_descriptors']
        for limits, expected in [((128, 256), (128, 256)), ((4096, 8192), (4096, 8192)),
                                 ((8192, 16384), (4096, 16384)),
                                 ((resource.RLIM_INFINITY, resource.RLIM_INFINITY), (4096, resource.RLIM_INFINITY))]:
            with self.subTest(limits=limits):
                with mock.patch.object(resource, 'getrlimit', return_value=limits):
                    with mock.patch.object(resource, 'setrlimit') as apply:
                        limit()
                apply.assert_called_once_with(resource.RLIMIT_NOFILE, expected)

    def test_vnc_limit_failure_refuses_child_start(self):
        def denied():
            raise OSError('limit refused')
        with mock.patch.dict(Supervisor.start.__globals__, limit_vnc_descriptors=denied):
            with self.assertRaises(subprocess.SubprocessError):
                self.start('vnc')
        self.assertNotIn('vnc', self.supervisor.children)

    def test_early_desktop_exit_cannot_be_hidden_by_successful_configuration(self):
        for service in ['xvfb', 'openbox', 'vnc']:
            with self.subTest(service=service):
                child = self.start(service)
                self.stop_unreaped(child)
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
                self.stop_unreaped(child)
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
            self.stop_unreaped(self.supervisor.children['openbox'])
            self.assertIsNone(self.supervisor.children['vnc'].poll())
            self.assertIsNone(self.supervisor.children['control'].poll())
            with self.assertRaises((ValueError, OSError)):
                CHECK(self.root, self.root)

    def test_every_required_service_failure_after_configuration_is_unhealthy(self):
        for service in ['xvfb', 'openbox', 'vnc', 'control', 'sshd']:
            with self.subTest(service=service):
                self.ready(True)
                child = self.supervisor.children[service]
                self.stop_unreaped(child)
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

    def test_supervisor_declares_that_it_starts_the_idle_watcher(self):
        declared = subprocess.run([sys.executable, str(Path(__file__).with_name('horizon-worker-supervise')),
                                   '--idle-stop-contract'], capture_output=True, timeout=10)
        self.assertEqual((declared.returncode, declared.stdout), (0, b'horizon-idle-stop-contract=1\n'))

    def test_idle_watcher_is_owned_and_checked_only_when_started(self):
        self.ready(False)
        self.assertNotIn('idle', json.loads((self.root / 'services.json').read_text())['services'])
        child = self.start('idle')
        self.supervisor.publish(False)
        self.assertIn('idle', json.loads((self.root / 'services.json').read_text())['services'])
        CHECK(self.root, self.root)
        self.stop_unreaped(child)
        with self.assertRaisesRegex(ValueError, 'idle'):
            self.supervisor.assert_running()
        with self.assertRaisesRegex(ValueError, 'exited'):
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

    def monitor_desktop(self, outcomes, on_probe=None):
        clock = [0.0]
        calls = []
        def probe(_pid):
            delay, healthy = outcomes[len(calls)]
            calls.append(clock[0])
            clock[0] += delay
            if on_probe:
                on_probe()
            return healthy
        def sleep(seconds):
            clock[0] += seconds
            if len(calls) == len(outcomes):
                self.supervisor.stopping = True
        fake_time = SimpleNamespace(monotonic=lambda: clock[0], sleep=sleep)
        with mock.patch.dict(Supervisor.monitor.__globals__, time=fake_time, desktop_ready=probe):
            with self.assertRaises(ValueError) as error:
                self.supervisor.monitor(True)
        return str(error.exception), calls, clock[0]

    def test_single_probe_timeout_can_recover_without_replacing_services(self):
        self.ready(True)
        identities = dict(self.supervisor.identities)
        error, calls, _ = self.monitor_desktop([(2, False), (0, True)])
        self.assertEqual(error, 'Worker shutdown requested')
        self.assertEqual(len(calls), 2)
        self.assertEqual(self.supervisor.identities, identities)
        for name, child in self.supervisor.children.items():
            self.assertIsNone(child.poll())
            self.assertEqual(MODULE['process_identity'](child.pid), identities[name])

    def test_persistently_unhealthy_desktop_exhausts_bounded_grace(self):
        for duration in (2, 6):
            with self.subTest(probe_seconds=duration):
                self.ready(True)
                error, calls, elapsed = self.monitor_desktop([(duration, False)] * 20)
                self.assertEqual(error, 'Worker window manager stopped responding')
                self.assertGreater(len(calls), 1)
                self.assertGreaterEqual(elapsed, 10)
                self.assertLessEqual(elapsed, 10 + 1.2 + duration)
                self.supervisor.close()

    def test_readiness_check_rejects_unhealthy_desktop_without_runtime_grace(self):
        self.ready(True)
        with mock.patch.dict(CHECK.__globals__, desktop_ready=lambda _pid: False):
            with self.assertRaisesRegex(ValueError, 'Worker window manager is not ready'):
                CHECK(self.root, self.root)

    def test_successful_probe_resets_continuous_failure_window(self):
        self.ready(True)
        error, calls, elapsed = self.monitor_desktop(
            [(2, False), (2, False), (0, True), (2, False), (2, False), (0, True)])
        self.assertEqual(error, 'Worker shutdown requested')
        self.assertEqual(len(calls), 6)
        self.assertGreater(elapsed, 10)

    def test_exited_service_bypasses_desktop_grace(self):
        self.ready(True)
        child = self.supervisor.children['openbox']
        error, calls, _ = self.monitor_desktop([(2, False)] * 20,
                                             on_probe=lambda: self.stop_unreaped(child))
        self.assertEqual(error, 'Required worker service exited: openbox')
        self.assertEqual(len(calls), 1)

    def test_shutdown_during_probe_bypasses_desktop_grace(self):
        self.ready(True)
        error, calls, _ = self.monitor_desktop([(2, False)] * 20,
                                             on_probe=lambda: setattr(self.supervisor, 'stopping', True))
        self.assertEqual(error, 'Worker shutdown requested')
        self.assertEqual(len(calls), 1)

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
        self.stop_unreaped(parent)
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

    def test_exit_observation_reserves_leader_until_group_retirement(self):
        child = self.start('control')
        self.stop_unreaped(child)
        first = MODULE['exited'](child)
        self.assertEqual(MODULE['exited'](child), first)
        self.assertIsNone(child.returncode)
        self.assertTrue(MODULE['owns_group'](child, self.supervisor.identities['control']))
        self.supervisor.close()
        with self.assertRaises(ChildProcessError):
            MODULE['exited'](child)

    def test_external_reaping_and_changed_receipts_never_signal_an_unverified_group(self):
        for reaped in ['popen', 'external', 'changed']:
            with self.subTest(reaped=reaped):
                child = self.start('control')
                self.stop_unreaped(child)
                if reaped == 'popen':
                    child.wait(timeout=1)
                elif reaped == 'external':
                    os.waitpid(child.pid, 0)
                else:
                    self.supervisor.identities['control']['start'] = 'invalid'
                with mock.patch.object(os, 'killpg') as signaling:
                    with self.assertRaisesRegex(ValueError, 'unverified'):
                        self.supervisor.close()
                    signaling.assert_not_called()
                child.wait(timeout=1)

    def test_signaling_finishes_before_bounded_reaping_even_if_a_wait_times_out(self):
        events = []
        children = [mock.Mock(pid=101, returncode=None), mock.Mock(pid=102, returncode=None)]
        def waited(pid, timeout):
            events.append(('wait', pid))
            self.assertGreaterEqual(timeout, 0)
            self.assertLessEqual(timeout, 3)
            if pid == 101:
                raise subprocess.TimeoutExpired('service', timeout)
        for child in children:
            child.wait.side_effect = lambda timeout, pid=child.pid: waited(pid, timeout)
        stopping = MODULE['stop_groups']
        with mock.patch.dict(stopping.__globals__, owns_group=lambda child, receipt: True,
                             cleanup_exited=lambda child: True):
            with mock.patch.object(os, 'killpg', side_effect=lambda pid, sig: events.append((sig, pid))):
                with self.assertRaisesRegex(ValueError, 'cleanup deadline'):
                    stopping([(child, {}) for child in children])
        self.assertEqual(events, [(signal.SIGTERM, 101), (signal.SIGTERM, 102),
                                  (signal.SIGKILL, 101), (signal.SIGKILL, 102),
                                  ('wait', 101), ('wait', 102)])

    def test_signal_killed_configuration_fails_without_publishing(self):
        with self.assertRaisesRegex(ValueError, 'configuration failed'):
            self.supervisor.configure([sys.executable, '-c', 'import os,signal; os.kill(os.getpid(),signal.SIGTERM)'])
        self.assertFalse((self.root / 'services.json').exists())

    def test_signal_failure_does_not_skip_other_owned_groups_or_bounded_waits(self):
        stopping = MODULE['stop_groups']
        for error in [ProcessLookupError, PermissionError]:
            with self.subTest(error=error):
                children = [mock.Mock(pid=201, returncode=None), mock.Mock(pid=202, returncode=None)]
                events = []
                def signaling(pid, sig):
                    events.append((sig, pid))
                    if pid == 201:
                        raise error()
                with mock.patch.dict(stopping.__globals__, owns_group=lambda child, receipt: True,
                                     cleanup_exited=lambda child: True):
                    with mock.patch.object(os, 'killpg', side_effect=signaling):
                        with self.assertRaisesRegex(ValueError, 'could not signal'):
                            stopping([(child, {}) for child in children])
                self.assertEqual(events, [(signal.SIGTERM, 201), (signal.SIGTERM, 202),
                                          (signal.SIGKILL, 201), (signal.SIGKILL, 202)])
                for child in children:
                    child.wait.assert_called_once()
                    self.assertLessEqual(child.wait.call_args.kwargs['timeout'], 3)

    def test_configuration_kills_term_resistant_descendant_before_reaping_leader(self):
        marker = self.root / 'resistant-ready'
        descendant = self.root / 'resistant-pid'
        code = ('import signal,time; from pathlib import Path; '
                'signal.signal(signal.SIGTERM, signal.SIG_IGN); '
                'Path(' + repr(str(marker)) + ').touch(); time.sleep(60)')
        parent = ('import subprocess,time; from pathlib import Path; '
                  'child=subprocess.Popen([' + repr(sys.executable) + ',"-c",' + repr(code) + ']); '
                  'Path(' + repr(str(descendant)) + ').write_text(str(child.pid))\n'
                  'while not Path(' + repr(str(marker)) + ').exists(): time.sleep(.01)')
        self.supervisor.configure([sys.executable, '-c', parent])
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            try:
                MODULE['process_identity'](int(descendant.read_text()))
            except (OSError, ValueError):
                break
            time.sleep(.01)
        else:
            self.fail('TERM-resistant descendant survived group retirement')
        self.assertNotIn('configure', self.supervisor.children)


if __name__ == '__main__':
    unittest.main()
