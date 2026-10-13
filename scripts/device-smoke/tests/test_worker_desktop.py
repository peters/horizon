"""A debug Horizon on the cloud worker desktop runs without the worker's identity (#1510)."""
from __future__ import annotations

import argparse
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import worker_desktop  # noqa: E402
import x11_windows  # noqa: E402

WORKER_ENVIRONMENT = {
    'HOME': '/workspace/home',
    'PATH': '/usr/local/bin:/usr/bin',
    'HORIZON_SESSION_DIR': '/workspace/session-data/s1',
    'DISPLAY': ':0',
    'WAYLAND_DISPLAY': 'wayland-0',
    'HORIZON': 'panel-identity',
    'HORIZON_BROWSER_ACTOR': 'horizon:cloud-s1',
    'HORIZON_BROWSER_HOST_INSTANCE': 'host-1',
    'HORIZON_BROWSER_ROOT': '/workspace/home/.horizon',
    'HORIZON_DEVICE_ACTOR': 'horizon:cloud-s1',
    'HORIZON_CLOUD_ACTOR': 'cloud:s1',
    'HORIZON_GPU_LOCK': '/workspace/locks/gpu.lock',
    'ANTHROPIC_API_KEY': 'synthetic-key-value',
    'ANTHROPIC_WORKSPACE_ID': 'synthetic-workspace',
    'ANTHROPIC_CUSTOM_HEADERS': 'anthropic-workspace-id: synthetic-workspace',
    'HORIZON_HOME': '/operator/horizon-home',
    'CODEX_HOME': '/workspace/home/.codex',
    'XDG_CONFIG_HOME': '/workspace/home/.config',
    'HORIZON_AGENT_INTEGRATIONS': 'on',
}


class ChildEnvironmentTests(unittest.TestCase):
    def setUp(self):
        self.state = Path('/workspace/session-data/s1/worker-desktop/run')
        self.env, self.removed = worker_desktop.child_environment(WORKER_ENVIRONMENT, self.state, ':99')

    def test_worker_identity_credentials_and_wayland_are_removed(self):
        for key in worker_desktop.STRIPPED_KEYS:
            self.assertNotIn(key, self.env)
        self.assertEqual(self.removed, sorted(worker_desktop.STRIPPED_KEYS))
        for value in WORKER_ENVIRONMENT.values():
            self.assertNotIn(value, self.removed, 'the manifest names variables, never values')

    def test_state_goes_to_the_state_directory_and_home_stays(self):
        self.assertEqual(self.env['DISPLAY'], ':99')
        self.assertEqual(self.env['HOME'], '/workspace/home')
        self.assertEqual(self.env['HORIZON_SESSION_DIR'], '/workspace/session-data/s1')
        self.assertEqual(self.env['HORIZON_HOME'], str(self.state / 'horizon-home'))
        self.assertEqual(self.env['HORIZON_AGENT_INTEGRATIONS'], 'off', 'nothing is installed in HOME')
        self.assertEqual(self.env['HISTFILE'], str(self.state / 'shell-history'), 'its terminals keep no history in HOME')
        for key in ('CODEX_HOME', 'GROK_HOME', 'XDG_DATA_HOME', 'XDG_CONFIG_HOME', 'XDG_CACHE_HOME'):
            self.assertTrue(Path(self.env[key]).is_relative_to(self.state), key)

    def test_only_present_variables_are_reported(self):
        env, removed = worker_desktop.child_environment({'HOME': '/h', 'HORIZON_GPU_LOCK': 'x'}, self.state, ':99')
        self.assertEqual(removed, ['HORIZON_GPU_LOCK'])
        self.assertEqual(env['HOME'], '/h')


class BuildTests(unittest.TestCase):
    def test_the_build_is_the_locked_horizon_binary(self):
        self.assertEqual(worker_desktop.build_command()[-7:],
                         ['cargo', 'build', '--locked', '-p', 'horizon-ui', '--bin', 'horizon'])

    @unittest.skipUnless(hasattr(os, 'SCHED_IDLE'), 'Linux scheduler classes')
    def test_only_the_build_child_runs_at_idle_priority(self):
        before = os.nice(0)
        probe = subprocess.run(
            [sys.executable, '-c', 'import os; print(os.nice(0), os.sched_getscheduler(0))'],
            preexec_fn=worker_desktop.lower_priority, capture_output=True, text=True, check=True)
        nice, scheduler = probe.stdout.split()
        self.assertEqual(int(nice), 19)
        self.assertEqual(int(scheduler), os.SCHED_IDLE)
        self.assertEqual(os.nice(0), before, 'the launcher keeps its own priority')

    @unittest.skipUnless(hasattr(os, 'SCHED_IDLE'), 'Linux scheduler classes')
    def test_the_build_reports_the_priority_it_runs_at(self):
        with tempfile.TemporaryDirectory() as raw:
            original = worker_desktop.build_command
            worker_desktop.build_command = lambda: [sys.executable, '-c', 'import time; time.sleep(0.5)']
            try:
                built = worker_desktop.build(Path(raw) / 'target', Path(raw) / 'build.log')
            finally:
                worker_desktop.build_command = original
        self.assertEqual((built['nice'], built['scheduler']), (19, 'idle'))
        self.assertEqual(built['target_dir'], str(Path(raw) / 'target'))

    def test_the_build_failure_names_its_log(self):
        with tempfile.TemporaryDirectory() as raw:
            log = Path(raw) / 'build.log'
            original = worker_desktop.build_command
            worker_desktop.build_command = lambda: [sys.executable, '-c', 'import sys; print("broken"); sys.exit(3)']
            try:
                with self.assertRaisesRegex(SystemExit, 'exit code 3.*build.log'):
                    worker_desktop.build(Path(raw) / 'target', log)
            finally:
                worker_desktop.build_command = original
            self.assertIn('broken', log.read_text())


class PathTests(unittest.TestCase):
    def arguments(self, **values):
        defaults = {'state': None, 'horizon': None, 'target_dir': None}
        return argparse.Namespace(**{**defaults, **values})

    def test_state_and_target_live_in_the_session_directory(self):
        args = worker_desktop.default_paths(self.arguments(), {'HORIZON_SESSION_DIR': '/session'})
        self.assertEqual(args.state.parent, Path('/session/worker-desktop'))
        self.assertEqual(args.target_dir, Path('/session/target/device-smoke'))

    def test_a_given_binary_needs_no_target(self):
        args = worker_desktop.default_paths(self.arguments(horizon=Path('/bin/horizon')),
                                            {'HORIZON_SESSION_DIR': '/session'})
        self.assertIsNone(args.target_dir)

    def test_outside_a_worker_session_the_paths_must_be_given(self):
        with self.assertRaisesRegex(SystemExit, 'HORIZON_SESSION_DIR'):
            worker_desktop.default_paths(self.arguments(), {})
        args = worker_desktop.default_paths(
            self.arguments(state=Path('/s'), horizon=Path('/bin/horizon')), {})
        self.assertEqual(args.state, Path('/s'))


class FreezeAndStopTests(unittest.TestCase):
    def test_freeze_links_the_binary_and_records_its_hash(self):
        with tempfile.TemporaryDirectory() as raw:
            source = Path(raw) / 'horizon'
            source.write_bytes(b'synthetic binary')
            frozen, digest, method = worker_desktop.freeze(source, Path(raw))
            self.assertEqual(method, 'link', 'the same file system needs no copy')
            self.assertEqual(digest, hashlib.sha256(b'synthetic binary').hexdigest())
            self.assertTrue(os.path.samefile(frozen, source))
            self.assertEqual((Path(raw) / 'bin' / 'SHA256SUMS').read_text(), f'{digest}  horizon\n')
            # The next build replaces the file; the frozen link keeps this build.
            source.unlink()
            source.write_bytes(b'next build')
            self.assertEqual(frozen.read_bytes(), b'synthetic binary')

    def test_freeze_copies_when_a_link_is_impossible(self):
        with tempfile.TemporaryDirectory() as raw:
            source = Path(raw) / 'horizon'
            source.write_bytes(b'synthetic binary')
            original = os.link
            os.link = lambda *_args: (_ for _ in ()).throw(OSError(18, 'Invalid cross-device link'))
            try:
                frozen, digest, method = worker_desktop.freeze(source, Path(raw))
            finally:
                os.link = original
            self.assertEqual(method, 'copy')
            self.assertFalse(os.path.samefile(frozen, source))
            self.assertEqual(frozen.stat().st_mode & 0o777, 0o700)
            self.assertEqual(digest, hashlib.sha256(b'synthetic binary').hexdigest())

    def test_a_launch_without_a_window_fails_and_stops_the_process(self):
        started = []
        real_popen = subprocess.Popen

        def popen(command, *args, **kwargs):
            if not str(command[0]).endswith('/bin/horizon'):
                return real_popen(command, *args, **kwargs)
            # A candidate that stays alive but never maps a window.
            started.append(real_popen(['sleep', '60'], stdout=kwargs['stdout'], start_new_session=True))
            return started[0]
        with tempfile.TemporaryDirectory() as raw:
            binary = Path(raw) / 'horizon'
            binary.write_bytes(b'synthetic binary')
            arguments = ['--horizon', str(binary), '--state', str(Path(raw) / 'state'),
                         '--display', ':4242', '--window-timeout', '1']
            with mock.patch.object(worker_desktop.subprocess, 'Popen', side_effect=popen), \
                    mock.patch.object(worker_desktop, 'sha256', return_value='same'), \
                    mock.patch.object(worker_desktop.x11_windows, 'windows_of', return_value=[]), \
                    mock.patch.object(worker_desktop.x11_windows, 'request_close', return_value=0):
                with self.assertRaisesRegex(RuntimeError, 'No window of Horizon'):
                    worker_desktop.main(arguments)
            self.assertFalse((Path(raw) / 'state' / 'worker-desktop.json').exists(), 'no manifest')
        self.assertIsNotNone(started[0].poll(), 'the launcher stopped the candidate')

    def test_a_candidate_that_exits_before_its_window_is_named(self):
        real_popen = subprocess.Popen

        def popen(command, *args, **kwargs):
            if not str(command[0]).endswith('/bin/horizon'):
                return real_popen(command, *args, **kwargs)
            # As a missing runtime library makes the window library panic at start.
            return real_popen(['sh', '-c', 'sleep 0.2; exit 101'], stdout=kwargs['stdout'], start_new_session=True)
        with tempfile.TemporaryDirectory() as raw:
            binary = Path(raw) / 'horizon'
            binary.write_bytes(b'synthetic binary')
            arguments = ['--horizon', str(binary), '--state', str(Path(raw) / 'state'),
                         '--display', ':4242', '--window-timeout', '5']
            with mock.patch.object(worker_desktop.subprocess, 'Popen', side_effect=popen), \
                    mock.patch.object(worker_desktop, 'sha256', return_value='same'), \
                    mock.patch.object(worker_desktop.x11_windows, 'windows_of', return_value=[]), \
                    mock.patch.object(worker_desktop.x11_windows, 'request_close', return_value=0):
                with self.assertRaisesRegex(RuntimeError, 'Horizon exited with code 101 before its window showed'):
                    worker_desktop.main(arguments)

    def test_an_unreadable_display_is_named(self):
        process = subprocess.Popen(['sleep', '60'], start_new_session=True)
        self.addCleanup(worker_desktop.end_group, process)
        with mock.patch.object(worker_desktop.x11_windows, 'windows_of', side_effect=OSError('cannot open display')):
            with self.assertRaisesRegex(RuntimeError, 'Cannot read the windows of :4242: cannot open display'):
                worker_desktop.wait_for_window(process, ':4242', 5)

    def test_stop_ends_what_is_left_after_the_leader_exited(self):
        process = subprocess.Popen(['sh', '-c', 'sleep 60 & echo $!'], stdout=subprocess.PIPE, text=True,
                                   start_new_session=True)
        child = int(process.stdout.readline())
        process.stdout.close()
        process.wait(5)
        self.assertTrue(worker_desktop.group_alive(process.pid), 'the child outlives its leader')
        worker_desktop.stop(process, ':4242')
        self.assertFalse(worker_desktop.group_alive(process.pid))
        try:
            state = Path(f'/proc/{child}/stat').read_text().rsplit(')', 1)[1].split()[0]
        except FileNotFoundError:
            state = 'gone'
        self.assertIn(state, ('Z', 'gone'), 'only its new parent can reap the ended child')

    def test_stop_ends_a_process_that_has_no_window(self):
        process = subprocess.Popen(['sleep', '60'], start_new_session=True)
        started = time.monotonic()
        worker_desktop.stop(process, ':4242')
        self.assertIsNotNone(process.poll())
        self.assertLess(time.monotonic() - started, worker_desktop.STOP_SECONDS)


CLIENT = r'''
import ctypes, os, sys
sys.path.insert(0, sys.argv[1])
import x11_windows
x11 = x11_windows._library()
x11.XCreateSimpleWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong] + [ctypes.c_int] * 2 + [ctypes.c_uint] * 3 + [ctypes.c_ulong] * 2
x11.XCreateSimpleWindow.restype = ctypes.c_ulong
x11.XChangeProperty.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_int, ctypes.c_int, ctypes.c_void_p, ctypes.c_int]
x11.XSetWMProtocols.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong), ctypes.c_int]
x11.XMapWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
x11.XNextEvent.argtypes = [ctypes.c_void_p, ctypes.POINTER(x11_windows.XEvent)]
display = x11.XOpenDisplay(os.environ['DISPLAY'].encode())
window = x11.XCreateSimpleWindow(display, x11.XDefaultRootWindow(display), 0, 0, 64, 64, 0, 0, 0)
pid = ctypes.c_ulong(os.getpid())
x11.XChangeProperty(display, window, x11.XInternAtom(display, b'_NET_WM_PID', 0), x11_windows.XA_CARDINAL, 32, 0, ctypes.byref(pid), 1)
delete = ctypes.c_ulong(x11.XInternAtom(display, b'WM_DELETE_WINDOW', 0))
x11.XSetWMProtocols(display, window, ctypes.byref(delete), 1)
x11.XMapWindow(display, window)
x11.XSync(display, 0)
print('ready', flush=True)
event = x11_windows.XEvent()
while True:
    x11.XNextEvent(display, ctypes.byref(event))
    if event.xclient.type == x11_windows.CLIENT_MESSAGE and event.xclient.data[0] == delete.value:
        print('closed', flush=True)
        break
'''


@unittest.skipUnless(shutil.which('Xvfb'), 'needs Xvfb')
class CloseRequestTests(unittest.TestCase):
    def setUp(self):
        read_fd, write_fd = os.pipe()
        self.server = subprocess.Popen(['Xvfb', '-displayfd', str(write_fd), '-nolisten', 'tcp'],
                                       pass_fds=(write_fd,), stdout=subprocess.DEVNULL,
                                       stderr=subprocess.DEVNULL)
        os.close(write_fd)
        with os.fdopen(read_fd) as pipe:
            self.display = ':' + pipe.readline().strip()
        self.addCleanup(self.server.wait)
        self.addCleanup(self.server.terminate)

    def test_the_window_of_the_exact_process_is_asked_to_close(self):
        client = subprocess.Popen([sys.executable, '-c', CLIENT, str(Path(worker_desktop.__file__).parent)],
                                  env=dict(os.environ, DISPLAY=self.display), stdout=subprocess.PIPE,
                                  text=True)
        self.addCleanup(client.stdout.close)
        self.addCleanup(client.kill)
        self.assertEqual(client.stdout.readline().strip(), 'ready')
        self.assertTrue(x11_windows.windows_of(self.display, client.pid))
        self.assertEqual(x11_windows.request_close(self.display, client.pid + 1_000_000), 0)
        self.assertEqual(x11_windows.request_close(self.display, client.pid), 1)
        self.assertEqual(client.stdout.readline().strip(), 'closed')
        self.assertEqual(client.wait(5), 0)


if __name__ == '__main__':
    unittest.main()
